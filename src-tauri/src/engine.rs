//! NTFS file index: full MFT enumeration (FSCTL_ENUM_USN_DATA), live updates by tailing the USN journal,
//! an on-disk cache, and fuzzy name search. Needs an elevated process (raw volume handle).
use std::{
    collections::HashMap,
    ffi::c_void,
    io,
    mem::size_of,
    path::PathBuf,
    ptr::null_mut,
    sync::{atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst}, Mutex},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_HANDLE_EOF, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, GetDriveTypeW, GetLogicalDrives, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        IO::DeviceIoControl,
        Ioctl::{
            FSCTL_ENUM_USN_DATA, FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, MFT_ENUM_DATA_V0,
            READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0,
        },
    },
};

const REASON_CREATE: u32 = 0x100;
const REASON_DELETE: u32 = 0x200;
const REASON_RENAME_NEW: u32 = 0x2000;

pub struct Entry {
    parent: u64,
    pub name: String,
    lower: String,
    pub is_dir: bool,
}

impl Entry {
    fn new(parent: u64, name: String, is_dir: bool) -> Self {
        Entry { parent, lower: name.to_lowercase(), name, is_dir }
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
    reason: u32,
    is_dir: bool,
    name: String,
}

pub enum Change {
    Upsert(u64, u64, String, bool),
    Delete(u64),
}

fn rd<T: Copy>(b: &[u8], off: usize) -> T {
    unsafe { (b.as_ptr().add(off) as *const T).read_unaligned() }
}

/// Decode the records of an ENUM/READ output buffer (after its 8-byte header).
fn parse_records(buf: &[u8]) -> Vec<Rec> {
    let (mut out, mut off) = (vec![], 0);
    while off + 60 <= buf.len() {
        let len = rd::<u32>(buf, off) as usize;
        if len == 0 || off + len > buf.len() {
            break;
        }
        // USN_RECORD_V2 layout: frn@8, parent@16, usn@24, reason@40, attrs@52, name_len@56, name_off@58
        let (nlen, noff) = (rd::<u16>(buf, off + 56) as usize, rd::<u16>(buf, off + 58) as usize);
        let units: Vec<u16> = buf[off + noff..off + noff + nlen]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        out.push(Rec {
            frn: rd(buf, off + 8),
            parent: rd(buf, off + 16),
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
                    idx.insert(r.frn, Entry::new(r.parent, r.name, r.is_dir));
                }
            }
            Err(ERROR_HANDLE_EOF) => break,
            Err(e) => return Err(format!("FSCTL_ENUM_USN_DATA failed: error {e}")),
        }
    }
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
            ReasonMask: REASON_CREATE | REASON_DELETE | REASON_RENAME_NEW,
            ReturnOnlyOnClose: 0,
            Timeout: 0,
            BytesToWaitFor: 0,
            UsnJournalID: v.journal_id,
        };
        let got = ioctl(&h, FSCTL_READ_USN_JOURNAL, &q, buf.as_mut_ptr(), buf.len()).map_err(|e| format!("journal read failed: error {e}"))?;
        let new_next = rd::<i64>(&buf, 0);
        for r in parse_records(&buf[8..got as usize]) {
            out.push(if r.reason & REASON_DELETE != 0 { Change::Delete(r.frn) } else { Change::Upsert(r.frn, r.parent, r.name, r.is_dir) });
        }
        if got <= 8 {
            return Ok((out, new_next)); // no more records: new_next is the journal's current end
        }
        next = new_next;
    }
}

pub fn apply(v: &mut Volume, changes: Vec<Change>, next_usn: i64) {
    v.dirty |= !changes.is_empty();
    for c in changes {
        match c {
            Change::Upsert(frn, parent, name, is_dir) => { v.idx.insert(frn, Entry::new(parent, name, is_dir)); }
            Change::Delete(frn) => { v.idx.remove(&frn); }
        }
    }
    v.next_usn = next_usn;
}

// ---- on-disk cache: "LSD1", journal_id u64, next_usn i64, count u64, then per entry
// ---- frn u64, parent u64, is_dir u8, name_len u16, utf8 name.
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
    let mut b = Vec::with_capacity(v.idx.len() * 40);
    b.extend(b"LSD1");
    b.extend(v.journal_id.to_le_bytes());
    b.extend(v.next_usn.to_le_bytes());
    b.extend((v.idx.len() as u64).to_le_bytes());
    for (frn, e) in &v.idx {
        b.extend(frn.to_le_bytes());
        b.extend(e.parent.to_le_bytes());
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
    if b.get(..4)? != b"LSD1" {
        return None;
    }
    let (journal_id, next_usn, n) = (rd::<u64>(&b, 4), rd::<i64>(&b, 12), rd::<u64>(&b, 20) as usize);
    let (mut idx, mut off) = (Index::with_capacity(n), 28);
    for _ in 0..n {
        let nlen = rd::<u16>(&b, off + 17) as usize;
        let name = String::from_utf8(b.get(off + 19..off + 19 + nlen)?.to_vec()).ok()?;
        idx.insert(rd(&b, off), Entry::new(rd(&b, off + 8), name, b[off + 16] != 0));
        off += 19 + nlen;
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

/// Walk a directory tree with a pool of threads (SMB is latency-bound, not bandwidth-bound).
/// Ids are synthetic; 0 is the root. Symlinks are listed but not followed.
pub fn walk(root: &str) -> Index {
    let idx = Mutex::new(Index::new());
    let next_id = AtomicU64::new(1);
    let queue = Mutex::new(vec![(0u64, PathBuf::from(format!("{root}\\")))]);
    let pending = AtomicUsize::new(1); // directories queued or being read
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
                if let Ok(rd) = std::fs::read_dir(&dir) {
                    for e in rd.flatten() {
                        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                        let id = next_id.fetch_add(1, SeqCst);
                        if is_dir {
                            pending.fetch_add(1, SeqCst);
                            queue.lock().unwrap().push((id, e.path()));
                        }
                        found.push((id, Entry::new(parent, e.file_name().to_string_lossy().into_owned(), is_dir)));
                    }
                }
                idx.lock().unwrap().extend(found);
                pending.fetch_sub(1, SeqCst);
            });
        }
    });
    idx.into_inner().unwrap()
}

// ---- search

/// Subsequence match on a lowercased name; None if no match. Higher is better.
pub fn score(query: &str, name: &str) -> Option<f32> {
    let n: Vec<char> = name.chars().collect();
    let (mut i, mut s, mut prev) = (0, 0f32, usize::MAX);
    let mut first = None;
    for c in query.to_lowercase().chars() {
        let p = n[i..].iter().position(|&x| x == c)? + i;
        first.get_or_insert(p);
        s += 10.0;
        if p == 0 || matches!(n[p - 1], ' ' | '.' | '_' | '-') { s += 15.0; }
        if p == prev.wrapping_add(1) { s += 20.0; }
        prev = p;
        i = p + 1;
    }
    Some(s - n.len() as f32 * 0.1 - first.unwrap_or(0) as f32 * 0.5)
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

/// Best `limit` matches as (score, id), highest score first.
pub fn search(idx: &Index, query: &str, limit: usize) -> Vec<(f32, u64)> {
    let mut hits: Vec<(f32, u64)> = idx.iter().filter_map(|(&f, e)| score(query, &e.lower).map(|s| (s + if e.is_dir { 0.0 } else { 1.0 }, f))).collect();
    hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(frn: u64, parent: u64, attrs: u32, reason: u32, name: &str) -> Vec<u8> {
        let u: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        let len = (60 + u.len() + 7) & !7;
        let mut b = vec![0u8; len];
        b[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        b[8..16].copy_from_slice(&frn.to_le_bytes());
        b[16..24].copy_from_slice(&parent.to_le_bytes());
        b[40..44].copy_from_slice(&reason.to_le_bytes());
        b[52..56].copy_from_slice(&attrs.to_le_bytes());
        b[56..58].copy_from_slice(&(u.len() as u16).to_le_bytes());
        b[58..60].copy_from_slice(&60u16.to_le_bytes());
        b[60..60 + u.len()].copy_from_slice(&u);
        b
    }

    fn vol(buf: &[u8]) -> Volume {
        let idx = parse_records(buf).into_iter().map(|r| (r.frn, Entry::new(r.parent, r.name, r.is_dir))).collect();
        Volume { root: "C:".into(), walk: false, idx, journal_id: 7, next_usn: 100, dirty: false }
    }

    #[test]
    fn parse_search_and_path() {
        let mut buf = rec(10, 5, 0x10, 0, "Docs");
        buf.extend(rec(11, 10, 0, 0, "Reading List.txt"));
        buf.extend(rec(12, 10, 0, 0, "rm.txt"));
        let v = vol(&buf);
        assert_eq!(v.idx.len(), 3);
        assert_eq!(full_path(&v.idx, 12, "C:"), r"C:\Docs\rm.txt");
        assert_eq!(search(&v.idx, "rl", 5)[0].1, 11);
        assert_eq!(search(&v.idx, "rm", 5)[0].1, 12);
        assert!(search(&v.idx, "zzz", 5).is_empty());
    }

    #[test]
    fn live_changes_and_cache_roundtrip() {
        let mut v = vol(&[rec(10, 5, 0x10, 0, "Docs"), rec(11, 10, 0, 0, "old.txt")].concat());
        apply(&mut v, vec![
            Change::Upsert(11, 10, "new name.txt".into(), false), // rename
            Change::Upsert(12, 10, "fresh.txt".into(), false),    // create
            Change::Delete(10),                                    // delete
        ], 200);
        assert_eq!((v.idx.len(), v.next_usn, v.dirty), (2, 200, true));
        assert_eq!(search(&v.idx, "nn", 5)[0].1, 11);
        assert!(!v.idx.contains_key(&10));

        v.root = "Z:".into(); // never a real drive: keeps the test cache apart
        save(&mut v).unwrap();
        let back = load("Z:", false).unwrap();
        std::fs::remove_file(cache_path("Z:")).unwrap();
        assert_eq!((back.idx.len(), back.next_usn, back.journal_id), (2, 200, 7));
        assert_eq!(back.idx[&12].name, "fresh.txt");
    }

    #[test]
    fn walk_finds_nested_files() {
        let root = std::env::temp_dir().join("lisdir_walk_test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::write(root.join("a").join("b").join("deep file.txt"), b"x").unwrap();
        std::fs::write(root.join("top.txt"), b"x").unwrap();
        let r = root.to_str().unwrap();
        let idx = walk(r);
        assert_eq!(idx.len(), 4); // a, b, deep file.txt, top.txt
        let hit = search(&idx, "deepf", 5)[0].1;
        assert_eq!(full_path(&idx, hit, r), format!("{r}\\a\\b\\deep file.txt"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
