//! Network (SMB) volumes and other file systems without a journal: no MFT, so the directory tree is walked.
use super::{
    apply::{add_up, sum_folders, Verdict},
    cache::{cache_path, load},
    index::{Entry, Index, Rec},
    usn::{rd, utf16le},
    wide, Volume,
};
use crate::rules;
use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{atomic::{AtomicU64, AtomicUsize, Ordering::SeqCst}, mpsc::{channel, Receiver}, Mutex, RwLock},
    time::{Duration, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    },
};
// ---- network (SMB) volumes: no MFT or journal, so walk the directory tree

/// Re-walk interval for network roots, until the share has shown that it reports its changes (see `watch`).
pub const WALK_INTERVAL: Duration = Duration::from_secs(600);

/// Re-walk interval for a share that reports its changes: only a safety net. It misses what the server does not tell, such
/// as files written on a NAS by the NAS itself.
pub const WATCHED_WALK_INTERVAL: Duration = Duration::from_secs(3600);

/// The wait before the next walk. A share that takes long to walk (big, or reached over a VPN) is walked at most a
/// tenth of the time: a 20-minute walk waits 3 hours, not 10 minutes, so the server is not read almost without a pause.
pub fn next_walk(interval: Duration, took: Duration) -> Duration {
    interval.max(took * 9)
}

/// Has this share ever reported a change? Remembered in a file, because a quiet share (a home folder) may report
/// nothing for hours after a start, and it would be walked every 10 minutes all that time.
pub fn reported(root: &str) -> bool {
    cache_path(root).with_extension("watch").exists()
}

pub fn set_reported(root: &str) {
    let _ = std::fs::write(cache_path(root).with_extension("watch"), b"");
}

/// More changed paths than this at once are left to a walk.
pub const MAX_CHANGES: usize = 1000;

/// How soon to retry a walk that could not be trusted (share offline, drive unplugged).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60);

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
    pub(super) dirs: usize,
    pub(super) failed: usize,
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
/// `room` is the index to fill: `Index::sized_like` the last one, when there is one.
pub fn walk(root: &str, stream: Option<&RwLock<Volume>>, room: Index) -> Walk {
    walk_with(root, stream, room, &rules::active())
}

pub(super) fn walk_with(root: &str, stream: Option<&RwLock<Volume>>, room: Index, act: &rules::Active) -> Walk {
    let idx = Mutex::new(room);
    let next_id = AtomicU64::new(1);
    let queue = Mutex::new(vec![(0u64, PathBuf::from(format!("{root}\\")), false)]);
    let need_path = act.skip.has_paths() || act.hide.has_paths();
    let pending = AtomicUsize::new(1); // directories queued or being read
    let (dirs, failed) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..16 {
            s.spawn(|| loop {
                let job = queue.lock().unwrap().pop();
                let Some((parent, dir, parent_hidden)) = job else {
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
                            let name = e.file_name().to_string_lossy().into_owned();
                            let path = (need_path && is_dir).then(|| e.path().to_string_lossy().to_lowercase());
                            let verdict = Verdict::of(act, &name.to_lowercase(), path.as_deref(), Verdict::inherited(parent_hidden));
                            if verdict == Verdict::Skip {
                                continue; // not indexed, and not entered
                            }
                            // the listing already carries the modified time, so this costs no extra round trip
                            let meta = e.metadata().ok();
                            let time = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
                            let size = if is_dir { 0 } else { meta.map_or(0, |m| m.len()) };
                            let id = next_id.fetch_add(1, SeqCst);
                            if is_dir {
                                pending.fetch_add(1, SeqCst);
                                queue.lock().unwrap().push((id, e.path(), verdict == Verdict::Hide));
                            }
                            let mut entry = Entry::new(parent, name, is_dir, time).with_size(size);
                            entry.hidden = verdict == Verdict::Hide;
                            found.push((id, entry));
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
    let mut idx = idx.into_inner().unwrap();
    match stream {
        Some(v) => {
            let mut v = v.write().unwrap();
            sum_folders(&mut v.idx);
            v.idx.shrink_to_fit();
        }
        None => {
            sum_folders(&mut idx);
            idx.shrink_to_fit();
        }
    }
    Walk { idx, dirs: dirs.into_inner(), failed: failed.into_inner() }
}

/// The paths in a buffer of FILE_NOTIFY_INFORMATION records, relative to the watched folder.
fn notified_paths(buf: &[u8]) -> Vec<String> {
    let (mut out, mut off) = (vec![], 0);
    while off + 12 <= buf.len() {
        let (next, len) = (rd::<u32>(buf, off) as usize, rd::<u32>(buf, off + 8) as usize);
        let Some(name) = buf.get(off + 12..off + 12 + len) else { break };
        out.push(utf16le(name));
        if next == 0 {
            break;
        }
        off += next;
    }
    out
}

/// Watch a folder tree: the channel gets the relative path of whatever changes, and None when too much changed at once to
/// tell (time for a walk). It closes when the watch fails, as it does at once on a server that cannot report changes.
pub fn watch(root: &str) -> Option<Receiver<Option<String>>> {
    let h = unsafe { CreateFileW(wide(&format!("{root}\\")).as_ptr(), FILE_LIST_DIRECTORY, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, null_mut()) };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let (tx, rx) = channel();
    let handle = h as usize; // a raw handle is not `Send`
    std::thread::spawn(move || {
        let h = handle as HANDLE;
        let mut buf = vec![0u32; 64 * 1024 / 4]; // 64 KB is the most a network watch takes; aligned for the records
        let what = FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE;
        loop {
            let mut got = 0u32;
            // waits for a change; what happens between two calls is kept for the next one
            let ok = unsafe { ReadDirectoryChangesW(h, buf.as_mut_ptr().cast(), 64 * 1024, 1, what, &mut got, null_mut(), None) };
            if ok == 0 {
                break;
            }
            let sent = if got == 0 {
                tx.send(None).is_ok()
            } else {
                let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), got as usize) };
                notified_paths(bytes).into_iter().all(|p| tx.send(Some(p)).is_ok())
            };
            if !sent {
                break;
            }
        }
        unsafe { CloseHandle(h) };
    });
    Some(rx)
}

/// The entry at this path relative to the root; 0 (the root itself) for an empty path.
pub(super) fn find(idx: &Index, rel: &str) -> Option<u64> {
    rel.split('\\').filter(|p| !p.is_empty()).try_fold(0, |at, part| idx.child(at, part))
}

/// Take an entry out, with everything below it.
fn remove_tree(idx: &mut Index, frn: u64) {
    let Some(top) = idx.get(&frn) else { return };
    add_up(idx, top.parent, -(top.size as i64));
    let mut gone = HashSet::from([frn]);
    // children can sit before or after their folder in the vector: repeat until a pass finds no more
    if top.is_dir {
        loop {
            let more: Vec<u64> = idx.iter().filter(|(f, e)| gone.contains(&e.parent) && !gone.contains(f)).map(|(f, _)| f).collect();
            if more.is_empty() {
                break;
            }
            gone.extend(more);
        }
    }
    for f in gone {
        idx.remove(&f);
    }
}

/// Bring the index up to date with one path that was reported as changed. The report only says where to look: whether it
/// was added, modified, renamed or removed, what is there now tells what to do, and doing it twice does no harm.
/// The share is read without holding any lock.
pub fn refresh_path(vol: &RwLock<Volume>, rel: &str) {
    refresh_path_with(vol, rel, &rules::active());
}

pub(super) fn refresh_path_with(vol: &RwLock<Volume>, rel: &str, act: &rules::Active) {
    let root = vol.read().unwrap().root.clone();
    let full = format!("{root}\\{rel}");
    let now = match std::fs::symlink_metadata(&full) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(_) => return, // no way to tell: the next walk will
    };
    let (folder, name) = rel.rsplit_once('\\').unwrap_or(("", rel));
    let (parent, known) = {
        let v = vol.read().unwrap();
        (find(&v.idx, folder), find(&v.idx, rel).and_then(|f| v.idx.get(&f).map(|e| (f, e))))
    };
    let Some(parent) = parent else { return }; // in a folder that is not indexed, or that a change still to come will add
    let Some(m) = now else {
        if let Some((f, _)) = known {
            remove_tree(&mut vol.write().unwrap().idx, f);
        }
        return;
    };
    let is_dir = m.is_dir();
    let time = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    match known {
        Some((f, old)) if old.is_dir == is_dir => {
            let mut v = vol.write().unwrap();
            v.idx.set_meta(&f, time, if is_dir { old.size } else { m.len() });
            if !is_dir {
                add_up(&mut v.idx, old.parent, m.len() as i64 - old.size as i64);
            }
            return;
        }
        Some((f, _)) => remove_tree(&mut vol.write().unwrap().idx, f), // a file became a folder or the other way round
        None => {}
    }
    let hidden = vol.read().unwrap().idx.get(&parent).is_some_and(|p| p.hidden);
    let path = (is_dir && (act.skip.has_paths() || act.hide.has_paths())).then(|| full.to_lowercase());
    let verdict = Verdict::of(act, &name.to_lowercase(), path.as_deref(), Verdict::inherited(hidden));
    if verdict == Verdict::Skip {
        return;
    }
    // a new folder, or one renamed or moved here: nothing is reported for what it holds
    let below = if is_dir { walk_with(&full, None, Index::new(), act).idx } else { Index::new() };
    let size = if is_dir { below.iter().filter(|(_, e)| e.parent == 0).map(|(_, e)| e.size).sum() } else { m.len() };
    let mut v = vol.write().unwrap();
    let idx = &mut v.idx;
    // ids are made up, so any free ones will do: the walk's own start at 1, and so the new folder's id can be their offset
    let id = idx.keys().max().unwrap_or(0) + 1;
    let hide = verdict == Verdict::Hide;
    idx.insert_rec(id, name, Rec { parent, time, size, is_dir, hidden: hide });
    for (f, e, n) in below.iter_named() {
        idx.insert_rec(id + f, n, Rec { parent: if e.parent == 0 { id } else { id + e.parent }, hidden: e.hidden || hide, ..e });
    }
    add_up(idx, parent, size as i64);
}
