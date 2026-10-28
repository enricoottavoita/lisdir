//! NTFS file index: full MFT enumeration (FSCTL_ENUM_USN_DATA), live updates by tailing the USN journal,
//! an on-disk cache, and fuzzy name search. Needs an elevated process (raw volume handle).
use crate::{mft, query::{Only, Query}};
use std::{
    collections::HashMap,
    ffi::c_void,
    io,
    mem::size_of,
    path::PathBuf,
    ptr::null_mut,
    sync::{atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst}, Mutex, RwLock},
    time::{Duration, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_HANDLE_EOF, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, GetDriveTypeW, GetLogicalDrives, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            FSCTL_ENUM_USN_DATA, FSCTL_GET_NTFS_FILE_RECORD, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, MFT_ENUM_DATA_V0,
            NTFS_FILE_RECORD_INPUT_BUFFER, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
        },
    },
};

const REASON_CREATE: u32 = 0x100;
const REASON_DELETE: u32 = 0x200;
const REASON_RENAME_NEW: u32 = 0x2000;
// content changes, and the close that follows them (when NTFS has written the new size to the file record)
const REASON_DATA: u32 = 0x1 | 0x2 | 0x4;
const REASON_CLOSE: u32 = 0x8000_0000;

pub struct Entry {
    parent: u64,
    pub name: String,
    lower: String,
    pub is_dir: bool,
    /// last modification, unix seconds (0 = unknown). NTFS: from the file record; until that is read, the time of
    /// the file's last USN record.
    time: i64,
    /// bytes (0 for folders and when unknown)
    pub size: u64,
}

impl Entry {
    fn new(parent: u64, name: String, is_dir: bool, time: i64) -> Self {
        Entry { parent, lower: name.to_lowercase(), name, is_dir, time, size: 0 }
    }

    fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }
}

pub type Index = HashMap<u64, Entry>;

/// One volume's index plus the journal position it is current up to.
pub struct Volume {
    /// "C:" for an NTFS volume; a network path like `\\nas\share` (or a mapped "Z:") for a walked one
    pub root: String,
    /// walked (SMB etc.): no MFT/journal, refreshed by re-walking
    pub walk: bool,
    pub idx: Index,
    journal_id: u64,
    next_usn: i64,
    pub dirty: bool,
}

/// A decoded USN_RECORD_V2.
struct Rec {
    frn: u64,
    parent: u64,
    time: i64,
    reason: u32,
    is_dir: bool,
    name: String,
}

pub enum Change {
    Upsert(u64, u64, String, bool, i64),
    Delete(u64),
    /// modified time and size read from the file record, for an entry upserted earlier in the same batch
    Meta(u64, i64, u64),
}

fn rd<T: Copy>(b: &[u8], off: usize) -> T {
    unsafe { (b.as_ptr().add(off) as *const T).read_unaligned() }
}

/// FILETIME (100 ns ticks since 1601) to unix seconds.
pub fn filetime_to_unix(ft: i64) -> i64 {
    (ft / 10_000_000 - 11_644_473_600).max(0)
}

/// Decode the records of an ENUM/READ output buffer (after its 8-byte header).
fn parse_records(buf: &[u8]) -> Vec<Rec> {
    let (mut out, mut off) = (vec![], 0);
    while off + 60 <= buf.len() {
        let len = rd::<u32>(buf, off) as usize;
        if len == 0 || off + len > buf.len() {
            break;
        }
        // USN_RECORD_V2 layout: frn@8, parent@16, usn@24, time@32, reason@40, attrs@52, name_len@56, name_off@58
        let (nlen, noff) = (rd::<u16>(buf, off + 56) as usize, rd::<u16>(buf, off + 58) as usize);
        let units: Vec<u16> = buf[off + noff..off + noff + nlen]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        out.push(Rec {
            frn: rd(buf, off + 8),
            parent: rd(buf, off + 16),
            time: filetime_to_unix(rd(buf, off + 32)),
            reason: rd(buf, off + 40),
            is_dir: rd::<u32>(buf, off + 52) & 0x10 != 0,
            name: String::from_utf16_lossy(&units),
        });
        off += len;
    }
    out
}

struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open_volume(letter: char) -> Result<Handle, String> {
    let path: Vec<u16> = format!(r"\\.\{letter}:").encode_utf16().chain([0]).collect();
    let h = unsafe {
        CreateFileW(path.as_ptr(), 0x8000_0000 /* GENERIC_READ */, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null_mut(), OPEN_EXISTING, 0, null_mut())
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(format!("cannot open volume {letter}: (run as Administrator, NTFS only): error {}", unsafe { GetLastError() }));
    }
    Ok(Handle(h))
}

/// Raw DeviceIoControl; returns bytes written or the Win32 error.
fn ioctl<I, O>(h: &Handle, code: u32, input: &I, out: *mut O, out_len: usize) -> Result<u32, u32> {
    let mut got = 0u32;
    let ok = unsafe { DeviceIoControl(h.0, code, input as *const I as *const c_void, size_of::<I>() as u32, out as *mut c_void, out_len as u32, &mut got, null_mut()) };
    if ok == 0 { Err(unsafe { GetLastError() }) } else { Ok(got) }
}

fn query_journal(h: &Handle) -> Option<USN_JOURNAL_DATA_V0> {
    let mut j: USN_JOURNAL_DATA_V0 = unsafe { std::mem::zeroed() };
    ioctl(h, FSCTL_QUERY_USN_JOURNAL, &(), &mut j, size_of::<USN_JOURNAL_DATA_V0>()).ok().map(|_| j)
}

/// Modified time and size of one file, from its NTFS file record. None if the record cannot be read (deleted
/// meanwhile, or a layout we do not understand).
fn meta_of(h: &Handle, frn: u64, buf: &mut [u8]) -> Option<mft::Meta> {
    let want = frn & 0xFFFF_FFFF_FFFF; // the top 16 bits are a reuse counter
    let input = NTFS_FILE_RECORD_INPUT_BUFFER { FileReferenceNumber: want as i64 };
    let got = ioctl(h, FSCTL_GET_NTFS_FILE_RECORD, &input, buf.as_mut_ptr(), buf.len()).ok()? as usize;
    // output: file reference u64, record length u32, then the record. For an unused number Windows
    // answers with the previous used one, so the reference must be checked.
    let len = rd::<u32>(buf, 8) as usize;
    if got < 12 + len || rd::<u64>(buf, 0) & 0xFFFF_FFFF_FFFF != want {
        return None;
    }
    mft::parse(&mut buf[12..12 + len])
}

/// Read `meta_of` for every entry, on several threads (it is one system call per file).
fn fill_meta(h: &Handle, idx: &mut Index) {
    let frns: Vec<u64> = idx.keys().copied().collect();
    let found: Vec<Vec<(u64, mft::Meta)>> = std::thread::scope(|s| {
        let jobs: Vec<_> = frns
            .chunks(frns.len().div_ceil(8).max(1))
            .map(|chunk| s.spawn(move || {
                let mut buf = vec![0u8; 8192];
                chunk.iter().filter_map(|&f| meta_of(h, f, &mut buf).map(|m| (f, m))).collect()
            }))
            .collect();
        jobs.into_iter().map(|j| j.join().unwrap_or_default()).collect()
    });
    for (f, m) in found.into_iter().flatten() {
        if let Some(e) = idx.get_mut(&f) {
            e.time = m.mtime;
            e.size = m.size;
        }
    }
}

/// Full enumeration of the volume. The journal position is read first, so changes made while
/// scanning are replayed afterwards (upserts/deletes are idempotent).
pub fn scan_volume(letter: char) -> Result<Volume, String> {
    let h = open_volume(letter)?;
    let j = query_journal(&h);
    let mut idx = Index::new();
    let mut med = MFT_ENUM_DATA_V0 { StartFileReferenceNumber: 0, LowUsn: 0, HighUsn: i64::MAX };
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match ioctl(&h, FSCTL_ENUM_USN_DATA, &med, buf.as_mut_ptr(), buf.len()) {
            Ok(got) => {
                med.StartFileReferenceNumber = rd::<u64>(&buf, 0);
                for r in parse_records(&buf[8..got as usize]) {
                    idx.insert(r.frn, Entry::new(r.parent, r.name, r.is_dir, r.time));
                }
            }
            Err(ERROR_HANDLE_EOF) => break,
            Err(e) => return Err(format!("FSCTL_ENUM_USN_DATA failed: error {e}")),
        }
    }
    fill_meta(&h, &mut idx);
    // ponytail: no active journal => journal_id 0, no live updates and no cache (always rescanned).
    Ok(Volume { root: format!("{letter}:"), walk: false, idx, journal_id: j.map_or(0, |j| j.UsnJournalID), next_usn: j.map_or(0, |j| j.NextUsn), dirty: true })
}

/// Read pending journal records (non-blocking). Err means the position is no longer valid: rescan.
pub fn read_changes(v: &Volume) -> Result<(Vec<Change>, i64), String> {
    let h = open_volume(v.root.chars().next().unwrap_or('C'))?;
    let mut buf = vec![0u8; 1 << 18];
    let mut next = v.next_usn;
    let mut out = vec![];
    loop {
        let q = READ_USN_JOURNAL_DATA_V0 {
            StartUsn: next,
            ReasonMask: REASON_CREATE | REASON_DELETE | REASON_RENAME_NEW | REASON_DATA | REASON_CLOSE,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: v.journal_id,
        };
        let got = ioctl(&h, FSCTL_READ_USN_JOURNAL, &q, buf.as_mut_ptr(), buf.len()).map_err(|e| format!("journal read failed: error {e}"))?;
        let new_next = rd::<i64>(&buf, 0);
        for r in parse_records(&buf[8..got as usize]) {
            out.push(if r.reason & REASON_DELETE != 0 { Change::Delete(r.frn) } else { Change::Upsert(r.frn, r.parent, r.name, r.is_dir, r.time) });
        }
        if got <= 8 {
            // the file record of each entry that was upserted (and not deleted again) tells its current time and size
            let mut seen = std::collections::HashSet::new();
            let mut buf = vec![0u8; 8192];
            let mut metas = vec![];
            for c in out.iter().rev() {
                match c {
                    Change::Upsert(f, ..) if seen.insert(*f) => metas.extend(meta_of(&h, *f, &mut buf).map(|m| Change::Meta(*f, m.mtime, m.size))),
                    Change::Delete(f) => { seen.insert(*f); }
                    _ => {}
                }
            }
            out.extend(metas);
            return Ok((out, new_next)); // no more records: new_next is the journal's current end
        }
        next = new_next;
    }
}

pub fn apply(v: &mut Volume, changes: Vec<Change>, next_usn: i64) {
    v.dirty |= !changes.is_empty();
    for c in changes {
        match c {
            Change::Upsert(frn, parent, name, is_dir, time) => { v.idx.insert(frn, Entry::new(parent, name, is_dir, time)); }
            Change::Delete(frn) => { v.idx.remove(&frn); }
            Change::Meta(frn, time, size) => {
                if let Some(e) = v.idx.get_mut(&frn) {
                    e.time = time;
                    e.size = size;
                }
            }
        }
    }
    v.next_usn = next_usn;
}

// ---- on-disk cache: "LSD3", journal_id u64, next_usn i64, count u64, then per entry
// ---- frn u64, parent u64, time i64, size u64, is_dir u8, name_len u16, utf8 name.
// ---- (Older caches lack sizes; they are ignored and rebuilt once.)
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("lisdir")
}

fn cache_path(root: &str) -> PathBuf {
    let name: String = root.trim_end_matches(':').chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    data_dir().join(format!("{name}.idx"))
}

pub fn save(v: &mut Volume) -> io::Result<()> {
    if v.journal_id == 0 && !v.walk {
        return Ok(()); // no journal to catch up from: a cache could only be stale
    }
    let p = cache_path(&v.root);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let mut b = Vec::with_capacity(v.idx.len() * 56);
    b.extend(b"LSD3");
    b.extend(v.journal_id.to_le_bytes());
    b.extend(v.next_usn.to_le_bytes());
    b.extend((v.idx.len() as u64).to_le_bytes());
    for (frn, e) in &v.idx {
        b.extend(frn.to_le_bytes());
        b.extend(e.parent.to_le_bytes());
        b.extend(e.time.to_le_bytes());
        b.extend(e.size.to_le_bytes());
        b.push(e.is_dir as u8);
        b.extend((e.name.len() as u16).to_le_bytes());
        b.extend(e.name.as_bytes());
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, b)?;
    std::fs::rename(tmp, p)?; // atomic: a crash never leaves a half-written cache
    v.dirty = false;
    Ok(())
}

fn load(root: &str, walk: bool) -> Option<Volume> {
    let b = std::fs::read(cache_path(root)).ok()?;
    if b.get(..4)? != b"LSD3" {
        return None;
    }
    let (journal_id, next_usn, n) = (rd::<u64>(&b, 4), rd::<i64>(&b, 12), rd::<u64>(&b, 20) as usize);
    let (mut idx, mut off) = (Index::with_capacity(n), 28);
    for _ in 0..n {
        let nlen = rd::<u16>(&b, off + 33) as usize;
        let name = String::from_utf8(b.get(off + 35..off + 35 + nlen)?.to_vec()).ok()?;
        idx.insert(rd(&b, off), Entry::new(rd(&b, off + 8), name, b[off + 32] != 0, rd(&b, off + 16)).with_size(rd(&b, off + 24)));
        off += 35 + nlen;
    }
    Some(Volume { root: root.to_string(), walk, idx, journal_id, next_usn, dirty: false })
}

/// Cached index caught up through the journal, or a fresh full scan when the cache is
/// missing, from another journal, or too old.
pub fn open(letter: char) -> Result<Volume, String> {
    if let Some(mut v) = load(&format!("{letter}:"), false) {
        let live = open_volume(letter).ok().and_then(|h| query_journal(&h));
        if live.is_some_and(|j| j.UsnJournalID == v.journal_id && v.next_usn >= j.FirstUsn) {
            if let Ok((c, n)) = read_changes(&v) {
                apply(&mut v, c, n);
                return Ok(v);
            }
        }
    }
    scan_volume(letter)
}

pub fn live(v: &Volume) -> bool {
    v.journal_id != 0
}

// ---- network (SMB) volumes: no MFT or journal, so walk the directory tree

/// Re-walk interval for network roots. ponytail: full re-walk, no change detection over SMB.
pub const WALK_INTERVAL: Duration = Duration::from_secs(600);

/// How soon to retry a walk that could not be trusted (share offline, drive unplugged).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60);

fn is_remote(letter: char) -> bool {
    let p: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    unsafe { GetDriveTypeW(p.as_ptr()) == 4 } // DRIVE_REMOTE
}

/// Mapped network drives plus the paths listed in roots.txt (one per line, `#` comments).
pub fn network_roots() -> Vec<String> {
    let mask = unsafe { GetLogicalDrives() };
    let mut roots: Vec<String> = ('A'..='Z').filter(|&l| mask >> (l as u8 - b'A') & 1 == 1 && is_remote(l)).map(|l| format!("{l}:")).collect();
    let listed = std::fs::read_to_string(data_dir().join("roots.txt")).unwrap_or_default();
    for l in listed.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let r = l.trim_end_matches('\\').to_string();
        if !roots.iter().any(|x| x.eq_ignore_ascii_case(&r)) {
            roots.push(r);
        }
    }
    roots
}

/// Whether `letter` is a mapped network drive (the NTFS pass skips those).
pub fn is_network_drive(letter: char) -> bool {
    is_remote(letter)
}

/// Cached (or empty) volume for a network root, and whether the cache is recent enough to skip a walk.
pub fn open_walk(root: &str) -> (Volume, bool) {
    let fresh = std::fs::metadata(cache_path(root)).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a < WALK_INTERVAL);
    let v = load(root, true).unwrap_or(Volume { root: root.to_string(), walk: true, idx: Index::new(), journal_id: 0, next_usn: 0, dirty: false });
    (v, fresh)
}

/// Result of a directory walk, with enough bookkeeping to tell a real listing from one cut short by an
/// unreachable share.
pub struct Walk {
    pub idx: Index,
    dirs: usize,
    failed: usize,
}

impl Walk {
    /// Safe to replace the previous index with. "Access denied" folders are normal and do not count; any other
    /// read failure does, and a walk where more than 2% of the folders failed (or the root itself) is rejected,
    /// so an offline NAS never wipes its old results.
    pub fn reliable(&self) -> bool {
        self.failed == 0 || self.failed * 50 < self.dirs
    }
}

/// Walk a directory tree with a pool of threads (SMB is latency-bound, not bandwidth-bound).
/// Ids are synthetic; 0 is the root. Symlinks are listed but not followed.
/// With `stream` (a volume that has no index yet) each folder's entries go straight into that volume, so results
/// show up while the first walk is still running; the returned `idx` is then empty.
pub fn walk(root: &str, stream: Option<&RwLock<Volume>>) -> Walk {
    let idx = Mutex::new(Index::new());
    let next_id = AtomicU64::new(1);
    let queue = Mutex::new(vec![(0u64, PathBuf::from(format!("{root}\\")))]);
    let pending = AtomicUsize::new(1); // directories queued or being read
    let (dirs, failed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| loop {
                let job = queue.lock().unwrap().pop();
                let Some((parent, dir)) = job else {
                    if pending.load(SeqCst) == 0 {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                let mut found = vec![];
                dirs.fetch_add(1, SeqCst);
                match std::fs::read_dir(&dir) {
                    Ok(rd) => {
                        for e in rd.flatten() {
                            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                            // the listing already carries the modified time, so this costs no extra round trip
                            let meta = e.metadata().ok();
                            let time = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
                            let size = if is_dir { 0 } else { meta.map_or(0, |m| m.len()) };
                            let id = next_id.fetch_add(1, SeqCst);
                            if is_dir {
                                pending.fetch_add(1, SeqCst);
                                queue.lock().unwrap().push((id, e.path()));
                            }
                            found.push((id, Entry::new(parent, e.file_name().to_string_lossy().into_owned(), is_dir, time).with_size(size)));
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
                    Err(_) => {
                        failed.fetch_add(1, SeqCst);
                    }
                }
                match stream {
                    Some(v) => v.write().unwrap().idx.extend(found),
                    None => idx.lock().unwrap().extend(found),
                }
                pending.fetch_sub(1, SeqCst);
            });
        }
    });
    Walk { idx: idx.into_inner().unwrap(), dirs: dirs.into_inner(), failed: failed.into_inner() }
}

// ---- search

/// Fuzzy match of one lowercased word against a lowercased name: its letters must appear in order. None if
/// they do not. Higher is better: word starts and consecutive letters score extra, long names and late
/// matches lose a little. Single pass, no allocation: this runs once per indexed name on every keystroke.
pub fn score(word: &str, name: &str) -> Option<f32> {
    let mut want = word.chars();
    let mut cur = want.next();
    if cur.is_none() {
        return Some(0.0);
    }
    let (mut s, mut first, mut last, mut prev) = (0f32, 0usize, -1isize, ' ');
    for (p, c) in name.chars().enumerate() {
        if Some(c) == cur {
            if last < 0 {
                first = p;
            }
            s += 10.0;
            if p == 0 || matches!(prev, ' ' | '.' | '_' | '-') { s += 15.0; }
            if p as isize == last + 1 { s += 20.0; }
            last = p as isize;
            cur = want.next();
            if cur.is_none() {
                return Some(s - name.chars().count() as f32 * 0.1 - first as f32 * 0.5);
            }
        }
        prev = c;
    }
    None
}

pub fn full_path(idx: &Index, mut frn: u64, root: &str) -> String {
    let mut parts = vec![];
    while let Some(e) = idx.get(&frn) {
        parts.push(e.name.as_str());
        frn = e.parent;
    }
    parts.reverse();
    format!("{root}\\{}", parts.join("\\"))
}

fn ext_of(lower_name: &str) -> &str {
    match lower_name.rfind('.') {
        Some(i) if i > 0 && i + 1 < lower_name.len() => &lower_name[i + 1..],
        _ => "",
    }
}

/// Does some ancestor folder's name contain `word`? (the `docs\` part of a query). A plain substring, not the
/// fuzzy match used for file names: a folder filter must be precise, or `docs\` would also pick up "Discografia".
fn in_folder(idx: &Index, mut parent: u64, word: &str) -> bool {
    while let Some(p) = idx.get(&parent) {
        if p.lower.contains(word) {
            return true;
        }
        parent = p.parent;
    }
    false
}

/// Score of an entry for a query, or None if it does not qualify. Cheap checks first.
fn matches(q: &Query, idx: &Index, e: &Entry) -> Option<f32> {
    match q.only {
        Some(Only::Files) if e.is_dir => return None,
        Some(Only::Folders) if !e.is_dir => return None,
        _ => {}
    }
    if !q.ext.is_empty() || !q.not_ext.is_empty() {
        let x = ext_of(&e.lower);
        if (!q.ext.is_empty() && !q.ext.iter().any(|v| v == x)) || q.not_ext.iter().any(|v| v == x) {
            return None;
        }
    }
    if q.not.iter().any(|w| e.lower.contains(w.as_str())) || !q.phrases.iter().all(|p| e.lower.contains(p.as_str())) {
        return None;
    }
    if let Some((from, to)) = q.date {
        if e.time < from || e.time >= to {
            return None;
        }
    }
    if let Some((min, max)) = q.size {
        if e.is_dir || e.size < min || e.size > max {
            return None;
        }
    }
    let mut s = 0.0;
    for t in &q.terms {
        s += score(t, &e.lower)?;
    }
    if !q.dirs.iter().all(|d| in_folder(idx, e.parent, d)) {
        return None;
    }
    // with no words to rank by (a pure filter): biggest first for a size filter, otherwise newest first
    let base = match (q.terms.is_empty(), q.size.is_some()) {
        (false, _) => s,
        (true, true) => e.size as f32,
        (true, false) => e.time as f32 / 1e6,
    };
    Some(base + if e.is_dir { 0.0 } else { 1.0 })
}

/// Best `limit` matches as (score, id), highest score first.
pub fn search(idx: &Index, q: &Query, limit: usize) -> Vec<(f32, u64)> {
    if !q.searches_files() || q.is_empty() {
        return vec![];
    }
    let mut hits: Vec<(f32, u64)> = idx.iter().filter_map(|(&f, e)| matches(q, idx, e).map(|s| (s, f))).collect();
    let best_first = |a: &(f32, u64), b: &(f32, u64)| b.0.total_cmp(&a.0);
    if hits.len() > limit {
        // a one-letter query matches most of the disk: pick the best few without sorting everything
        hits.select_nth_unstable_by(limit, best_first);
        hits.truncate(limit);
    }
    hits.sort_by(best_first);
    hits
}

/// Char positions of `name` that the query matched, for highlighting. Empty if positions cannot be mapped.
pub fn highlight(q: &Query, name: &str) -> Vec<usize> {
    let lower: Vec<char> = name.to_lowercase().chars().collect();
    if lower.len() != name.chars().count() {
        return vec![]; // lowercasing changed the length: indexes would not line up
    }
    let mut marked = vec![false; lower.len()];
    for w in &q.terms {
        let mut i = 0;
        for c in w.chars() {
            match lower[i..].iter().position(|&x| x == c) {
                Some(k) => {
                    marked[i + k] = true;
                    i += k + 1;
                }
                None => break,
            }
        }
    }
    for p in &q.phrases {
        let p: Vec<char> = p.chars().collect();
        if let Some(at) = lower.windows(p.len().max(1)).position(|w| w == &p[..]) {
            marked[at..at + p.len()].iter_mut().for_each(|m| *m = true);
        }
    }
    marked.iter().enumerate().filter(|(_, &m)| m).map(|(i, _)| i).collect()
}

/// A small fixed volume for tests in other modules.
#[cfg(test)]
pub fn sample_volume() -> Volume {
    let mut idx = Index::new();
    idx.insert(1, Entry::new(0, "Docs".into(), true, 1_700_000_000));
    idx.insert(2, Entry::new(1, "report 2024.pdf".into(), false, 1_700_000_000));
    idx.insert(3, Entry::new(1, "draft report.docx".into(), false, 1_600_000_000));
    Volume { root: "C:".into(), walk: false, idx, journal_id: 7, next_usn: 0, dirty: false }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Clock;

    // 2023-11-14 22:13:20 UTC; local time zone = UTC
    const NOW: i64 = 1_700_000_000;
    const DAYS: i64 = 86_400;

    fn q(s: &str) -> Query {
        Query::parse(s, &Clock::new(NOW, 0))
    }

    fn rec(frn: u64, parent: u64, attrs: u32, reason: u32, name: &str) -> Vec<u8> {
        let u: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let len = (60 + u.len() + 7) & !7;
        let mut b = vec![0u8; len];
        b[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        b[8..16].copy_from_slice(&frn.to_le_bytes());
        b[16..24].copy_from_slice(&parent.to_le_bytes());
        b[32..40].copy_from_slice(&((NOW + 11_644_473_600) * 10_000_000).to_le_bytes()); // timestamp: NOW
        b[40..44].copy_from_slice(&reason.to_le_bytes());
        b[52..56].copy_from_slice(&attrs.to_le_bytes());
        b[56..58].copy_from_slice(&(u.len() as u16).to_le_bytes());
        b[58..60].copy_from_slice(&60u16.to_le_bytes());
        b[60..60 + u.len()].copy_from_slice(&u);
        b
    }

    fn vol(buf: &[u8]) -> Volume {
        let idx = parse_records(buf).into_iter().map(|r| (r.frn, Entry::new(r.parent, r.name, r.is_dir, r.time))).collect();
        Volume { root: "C:".into(), walk: false, idx, journal_id: 7, next_usn: 100, dirty: false }
    }

    /// C:\Docs\{report 2024.pdf, draft report.docx, notes.txt}, C:\Pics\{photo.jpg, report.jpg}, with ages in days.
    fn tree() -> Index {
        let mut i = Index::new();
        let mut add = |frn, parent, name: &str, is_dir, age: i64| i.insert(frn, Entry::new(parent, name.into(), is_dir, NOW - age * DAYS));
        add(1, 0, "Docs", true, 0);
        add(2, 0, "Pics", true, 0);
        add(10, 1, "report 2024.pdf", false, 0); // today
        add(11, 1, "draft report.docx", false, 3);
        add(12, 1, "notes.txt", false, 40);
        add(20, 2, "photo.jpg", false, 1);
        add(21, 2, "report.jpg", false, 5);
        i
    }

    fn found(idx: &Index, query: &str) -> Vec<u64> {
        let mut ids: Vec<u64> = search(idx, &q(query), 50).into_iter().map(|h| h.1).collect();
        ids.sort();
        ids
    }

    #[test]
    fn parse_search_and_path() {
        let mut buf = rec(10, 5, 0x10, 0, "Docs");
        buf.extend(rec(11, 10, 0, 0, "Reading List.txt"));
        buf.extend(rec(12, 10, 0, 0, "rm.txt"));
        let v = vol(&buf);
        assert_eq!(v.idx.len(), 3);
        assert_eq!(v.idx[&12].time, NOW); // the USN record's timestamp became the last-change time
        assert_eq!(full_path(&v.idx, 12, "C:"), r"C:\Docs\rm.txt");
        assert_eq!(search(&v.idx, &q("rl"), 5)[0].1, 11);
        assert_eq!(search(&v.idx, &q("rm"), 5)[0].1, 12);
        assert!(search(&v.idx, &q("zzz"), 5).is_empty());
        // the example used in the README
        let readme = vol(&rec(1, 5, 0, 0, "README.md"));
        assert_eq!(search(&readme.idx, &q("rdme"), 5)[0].1, 1);
    }

    #[test]
    fn scoring_prefers_starts_and_runs() {
        let s = |w, n| score(w, n).unwrap();
        assert!(s("rep", "report.txt") > s("rep", "my deep report.txt")); // earlier and shorter
        assert!(s("rep", "report.txt") > s("rpt", "report.txt")); // consecutive letters beat scattered ones
        assert!(s("lst", "my list.txt") > s("lst", "mylist.txt")); // letter at a word start
        assert_eq!(score("zz", "report"), None);
        assert_eq!(score("tr", "rt"), None); // order matters
    }

    #[test]
    fn words_match_in_any_order() {
        let t = tree();
        assert_eq!(found(&t, "2024 report"), [10]);
        assert_eq!(found(&t, "report"), [10, 11, 21]);
    }

    #[test]
    fn filters_combine() {
        let t = tree();
        assert_eq!(found(&t, "ext:pdf"), [10]);
        assert_eq!(found(&t, "*.jpg"), [20, 21]);
        assert_eq!(found(&t, "report -draft"), [10, 21]);
        assert_eq!(found(&t, "report -ext:jpg"), [10, 11]);
        assert_eq!(found(&t, r#""report 2024""#), [10]);
        assert_eq!(found(&t, "folder:"), [1, 2]);
        assert_eq!(found(&t, "file: pic:"), [20, 21]);
        assert_eq!(found(&t, r"docs\ report"), [10, 11]); // only reports inside a "docs" folder
        assert_eq!(found(&t, r"pics\ report"), [21]);
        assert_eq!(found(&t, r"nowhere\ report"), Vec::<u64>::new());
        assert_eq!(found(&t, r"dcs\ report"), Vec::<u64>::new()); // folder names are matched as text, not fuzzily
        assert_eq!(found(&t, "ext:pdf;docx -draft"), [10]);
    }

    #[test]
    fn dates_and_filter_only_order() {
        let t = tree();
        assert_eq!(found(&t, "date:today file:"), [10]);
        assert_eq!(found(&t, "date:week file:"), [10, 11, 20, 21]);
        assert_eq!(found(&t, "date:2023-11 file:"), [10, 11, 20, 21]);
        assert_eq!(found(&t, "date:2023 file:"), [10, 11, 12, 20, 21]);
        // no words to rank by: newest first
        let order: Vec<u64> = search(&t, &q("date:week file:"), 10).into_iter().map(|h| h.1).collect();
        assert_eq!(order, [10, 20, 11, 21]);
    }

    #[test]
    fn apps_flag_returns_no_files() {
        assert!(search(&tree(), &q("app: report"), 10).is_empty());
        assert!(search(&tree(), &q(""), 10).is_empty());
    }

    #[test]
    fn highlights_matched_letters() {
        let hl = |query: &str, name: &str| highlight(&q(query), name);
        assert_eq!(hl("rdme", "README.md"), [0, 3, 4, 5]); // r, d, m, e of "readme.md"
        assert_eq!(hl("rep 24", "report 2024.pdf"), [0, 1, 2, 7, 10]); // "rep" + 2 and 4 of "2024"
        assert_eq!(hl(r#""2024""#, "report 2024.pdf"), [7, 8, 9, 10]);
        assert_eq!(hl("x", "report"), Vec::<usize>::new());
    }

    #[test]
    fn live_changes_and_cache_roundtrip() {
        let mut v = vol(&[rec(10, 5, 0x10, 0, "Docs"), rec(11, 10, 0, 0, "old.txt")].concat());
        apply(&mut v, vec![
            Change::Upsert(11, 10, "new name.txt".into(), false, NOW + 5), // rename
            Change::Upsert(12, 10, "fresh.txt".into(), false, NOW + 9),    // create
            Change::Delete(10),                                             // delete
            Change::Meta(12, NOW + 20, 4096),                               // size and time from the file record
            Change::Meta(99, 1, 1),                                         // unknown entry: ignored
        ], 200);
        assert_eq!((v.idx.len(), v.next_usn, v.dirty), (2, 200, true));
        assert_eq!(search(&v.idx, &q("nn"), 5)[0].1, 11);
        assert!(!v.idx.contains_key(&10));

        v.root = "Z:".into(); // never a real drive: keeps the test cache apart
        save(&mut v).unwrap();
        let back = load("Z:", false).unwrap();
        std::fs::remove_file(cache_path("Z:")).unwrap();
        assert_eq!((back.idx.len(), back.next_usn, back.journal_id), (2, 200, 7));
        assert_eq!((back.idx[&12].name.as_str(), back.idx[&12].time, back.idx[&12].size), ("fresh.txt", NOW + 20, 4096));
    }

    #[test]
    fn size_filter_and_order() {
        let mut i = Index::new();
        i.insert(1, Entry::new(0, "folder".into(), true, NOW));
        i.insert(2, Entry::new(1, "small.bin".into(), false, NOW).with_size(10));
        i.insert(3, Entry::new(1, "big.bin".into(), false, NOW).with_size(5_000_000_000));
        i.insert(4, Entry::new(1, "mid.bin".into(), false, NOW).with_size(3_000_000));
        i.insert(5, Entry::new(1, "empty.txt".into(), false, NOW));
        let ids = |s: &str| search(&i, &q(s), 10).into_iter().map(|h| h.1).collect::<Vec<_>>();
        assert_eq!(ids("size:>1mb"), [3, 4]); // biggest first, folders never match a size
        assert_eq!(ids("size:1mb..10gb ext:bin"), [3, 4]);
        assert_eq!(ids("size:<100"), [2, 5]);
        assert_eq!(ids("size:empty"), [5]);
        assert_eq!(ids("big size:>1gb"), [3]); // combines with words
        assert!(ids("size:>1tb").is_empty());
    }

    #[test]
    fn walk_finds_nested_files() {
        let root = std::env::temp_dir().join("lisdir_walk_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::write(root.join("a").join("b").join("deep file.txt"), b"x").unwrap();
        std::fs::write(root.join("top.txt"), b"x").unwrap();
        let r = root.to_str().unwrap();
        let w = walk(r, None);
        assert!(w.reliable());
        let idx = w.idx;
        assert_eq!(idx.len(), 4); // a, b, deep file.txt, top.txt
        let hit = search(&idx, &q("deepf"), 5)[0].1;
        assert_eq!(full_path(&idx, hit, r), format!("{r}\\a\\b\\deep file.txt"));
        assert!(idx[&hit].time > NOW, "walked entries carry their modified time"); // written just now
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn streamed_walk_fills_the_volume_directly() {
        let root = std::env::temp_dir().join("lisdir_stream_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a").join("one.txt"), b"x").unwrap();
        let r = root.to_str().unwrap();
        let vol = RwLock::new(Volume { root: r.into(), walk: true, idx: Index::new(), journal_id: 0, next_usn: 0, dirty: false });
        let w = walk(r, Some(&vol));
        assert!(w.reliable() && w.idx.is_empty());
        assert_eq!(vol.read().unwrap().idx.len(), 2); // a, one.txt
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unreachable_walk_is_not_trusted() {
        // a share that is offline: the root itself cannot be read
        let w = walk(r"\\lisdir-no-such-host-0\nothing", None);
        assert!(w.idx.is_empty());
        assert!(!w.reliable());
        // a missing local folder behaves the same way
        assert!(!walk(r"C:\lisdir-no-such-folder-0", None).reliable());
        // the 2% rule: a few vanished folders are fine, a mass failure is not
        let few = Walk { idx: Index::new(), dirs: 1000, failed: 10 };
        let many = Walk { idx: Index::new(), dirs: 1000, failed: 200 };
        assert!(few.reliable() && !many.reliable());
    }

    /// An index of `n` plausible names spread over a folder tree.
    fn synthetic(n: u64) -> Index {
        let (words, exts) = (["report", "invoice", "holiday", "notes", "backup", "draft", "photo"], ["pdf", "docx", "jpg", "txt", "mp4"]);
        let mut i = Index::with_capacity(n as usize + 100);
        for d in 1..=100 {
            i.insert(d, Entry::new(0, format!("folder {d}"), true, NOW));
        }
        for k in 0..n {
            let name = format!("{}_{}_{}.{}", words[(k % 7) as usize], k, words[(k / 7 % 7) as usize], exts[(k % 5) as usize]);
            i.insert(1000 + k, Entry::new(1 + k % 100, name, false, NOW - (k % 400) as i64 * DAYS));
        }
        i
    }

    /// Guards against an accidental slowdown (say, an allocation back in the hot loop). The limit is generous
    /// on purpose: it catches order-of-magnitude regressions, not noise. Debug builds are far slower; skip them.
    #[test]
    fn search_latency_budget() {
        if cfg!(debug_assertions) {
            return;
        }
        let idx = synthetic(300_000);
        for query in ["rep", "inv 2024", r"folder\ photo ext:jpg", "ext:pdf date:month", "a"] {
            let t = std::time::Instant::now();
            let hits = search(&idx, &q(query), 20);
            let took = t.elapsed();
            assert!(took < Duration::from_millis(1500), "{query:?} took {took:?} over 300k names ({} hits)", hits.len());
        }
    }

    /// Needs an elevated shell: cargo test --release real_file_records -- --ignored --nocapture
    /// Reads size and time of every file on C: through the file records and reports the speed.
    #[test]
    #[ignore]
    fn real_file_records() {
        let h = open_volume('C').expect("run elevated");
        eprintln!("volume opened");
        let mut idx = Index::new();
        let mut med = MFT_ENUM_DATA_V0 { StartFileReferenceNumber: 0, LowUsn: 0, HighUsn: i64::MAX };
        let mut buf = vec![0u8; 1 << 20];
        while let Ok(got) = ioctl(&h, FSCTL_ENUM_USN_DATA, &med, buf.as_mut_ptr(), buf.len()) {
            med.StartFileReferenceNumber = rd::<u64>(&buf, 0);
            for r in parse_records(&buf[8..got as usize]) {
                idx.insert(r.frn, Entry::new(r.parent, r.name, r.is_dir, r.time));
            }
        }
        eprintln!("{} entries enumerated", idx.len());
        let mut probe = vec![0u8; 8192];
        for frn in [5u64, 6, 100_000] {
            let m = meta_of(&h, frn, &mut probe);
            eprintln!("record {frn}: {:?}", m.map(|m| (m.mtime, m.size)));
        }
        let t = std::time::Instant::now();
        fill_meta(&h, &mut idx);
        let known = idx.values().filter(|e| e.time != 0).count();
        let big = idx.values().max_by_key(|e| e.size).unwrap();
        println!("{} entries, metadata in {:?}; biggest: {} ({} bytes)", idx.len(), t.elapsed(), big.name, big.size);
        assert!(idx.values().any(|e| e.size > 0), "no sizes were read");
        assert!(known * 10 > idx.len() * 9, "fewer than 90% of the records could be read");
    }

    /// Manual benchmark on a few million names: cargo test --release bench_search -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_search() {
        let idx = synthetic(2_000_000);
        for query in ["rep", "inv 2024", "rpt", r"folder\ photo ext:jpg", "ext:pdf date:month", "a", "zzzz"] {
            let t = std::time::Instant::now();
            let hits = search(&idx, &q(query), 20);
            println!("{query:<26} {:>8.1?}  ({} hits)", t.elapsed(), hits.len());
        }
    }
}
