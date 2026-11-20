//! The on-disk snapshot and change log of a volume, and opening a volume from them.
use super::{
    apply::{apply, prune},
    index::{Index, Rec},
    usn::{open_volume, query_journal, rd, read_changes, scan_volume},
    Change, Volume,
};
use std::{io, path::PathBuf, sync::RwLock};
// ---- on-disk cache: "LSD3", journal_id u64, next_usn i64, count u64, then per entry
// ---- frn u64, parent u64, time i64, size u64, is_dir u8, name_len u16, utf8 name.
// ---- (Older caches lack sizes; they are ignored and rebuilt once.)
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("lisdir")
}

pub(super) fn cache_path(root: &str) -> PathBuf {
    let name: String = root.trim_end_matches(':').chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    data_dir().join(format!("{name}.idx"))
}

/// Write the cache file. Only reads the volume, so searches can run meanwhile.
pub(super) fn write_cache(v: &Volume) -> io::Result<()> {
    if v.journal_id == 0 && !v.walk {
        return Ok(()); // no journal to catch up from: a cache could only be stale
    }
    let p = cache_path(&v.root);
    std::fs::create_dir_all(p.parent().unwrap())?;
    use std::io::Write;
    let tmp = p.with_extension("tmp");
    // piece by piece: the whole file in memory next to the index would be the highest peak of the program
    let mut w = io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tmp)?);
    w.write_all(b"LSD3")?;
    w.write_all(&v.journal_id.to_le_bytes())?;
    w.write_all(&v.next_usn.to_le_bytes())?;
    w.write_all(&(v.idx.len() as u64).to_le_bytes())?;
    for (frn, e, name) in v.idx.iter_named() {
        w.write_all(&frn.to_le_bytes())?;
        w.write_all(&e.parent.to_le_bytes())?;
        w.write_all(&e.time.to_le_bytes())?;
        w.write_all(&e.size.to_le_bytes())?;
        w.write_all(&[e.is_dir as u8])?;
        w.write_all(&(name.len() as u16).to_le_bytes())?;
        w.write_all(name.as_bytes())?;
    }
    w.flush()?;
    drop(w);
    std::fs::rename(tmp, p)?; // atomic: a crash never leaves a half-written cache
    if v.walk {
        return Ok(()); // walked volumes are replaced as a whole, there is no journal to log
    }
    std::fs::write(log_path(&v.root), log_header(v.next_usn)) // changes from here on are appended to a new log
}

// ---- change log "LSL1": i64 next_usn of the snapshot it extends, then records. 1 = upsert (frn, parent, time,
// ---- is_dir u8, name_len u16, name), 2 = delete (frn), 3 = meta (frn, time, size), 4 = commit (next_usn): the
// ---- records before a commit are applied together. A torn tail (no commit) is dropped. The log is only a shortcut:
// ---- the USN journal replay at startup restores anything it misses, so losing it costs time, never correctness.
const LOG_LIMIT: u64 = 16 << 20;

pub(super) fn log_path(root: &str) -> PathBuf {
    cache_path(root).with_extension("log")
}

pub(super) fn log_header(snapshot_usn: i64) -> Vec<u8> {
    [b"LSL1".as_slice(), &snapshot_usn.to_le_bytes()].concat()
}

/// Append a batch of changes to the log instead of rewriting the snapshot. False if it could not be written
/// (the caller then falls back to a snapshot).
pub fn append_log(root: &str, changes: &[Change], next_usn: i64) -> bool {
    if changes.is_empty() {
        return true; // nothing to remember; the journal position is re-read from the snapshot at startup
    }
    let mut b = vec![];
    for c in changes {
        match c {
            Change::Upsert(frn, parent, name, is_dir, time) => {
                b.push(1);
                b.extend(frn.to_le_bytes());
                b.extend(parent.to_le_bytes());
                b.extend(time.to_le_bytes());
                b.push(*is_dir as u8);
                b.extend((name.len() as u16).to_le_bytes());
                b.extend(name.as_bytes());
            }
            Change::Delete(frn) => {
                b.push(2);
                b.extend(frn.to_le_bytes());
            }
            Change::Meta(frn, time, size) => {
                b.push(3);
                b.extend(frn.to_le_bytes());
                b.extend(time.to_le_bytes());
                b.extend(size.to_le_bytes());
            }
        }
    }
    b.push(4);
    b.extend(next_usn.to_le_bytes());
    use std::io::Write;
    // the log must already exist: the snapshot that started it is what its header refers to
    std::fs::OpenOptions::new().append(true).open(log_path(root)).and_then(|mut f| f.write_all(&b)).is_ok()
}

/// Size of the change log in bytes: when it grows past a limit it is time for a new snapshot.
pub fn log_is_big(root: &str) -> bool {
    std::fs::metadata(log_path(root)).is_ok_and(|m| m.len() > LOG_LIMIT)
}

/// Apply the committed batches of the log that extends this snapshot.
fn replay_log(v: &mut Volume) {
    let Ok(b) = std::fs::read(log_path(&v.root)) else { return };
    if b.get(..4) != Some(b"LSL1") || b.get(4..12).map(|h| rd::<i64>(h, 0)) != Some(v.next_usn) {
        return; // belongs to another snapshot
    }
    let (mut off, mut batch) = (12, vec![]);
    while let Some(&kind) = b.get(off) {
        off += 1;
        let ok = (|| -> Option<()> {
            match kind {
                1 => {
                    let (frn, parent, time) = (rd::<u64>(b.get(off..off + 8)?, 0), rd::<u64>(b.get(off + 8..off + 16)?, 0), rd::<i64>(b.get(off + 16..off + 24)?, 0));
                    let (is_dir, nlen) = (*b.get(off + 24)? != 0, rd::<u16>(b.get(off + 25..off + 27)?, 0) as usize);
                    let name = String::from_utf8(b.get(off + 27..off + 27 + nlen)?.to_vec()).ok()?;
                    batch.push(Change::Upsert(frn, parent, name, is_dir, time));
                    off += 27 + nlen;
                }
                2 => {
                    batch.push(Change::Delete(rd::<u64>(b.get(off..off + 8)?, 0)));
                    off += 8;
                }
                3 => {
                    let f = b.get(off..off + 24)?;
                    batch.push(Change::Meta(rd(f, 0), rd(f, 8), rd(f, 16)));
                    off += 24;
                }
                4 => {
                    let next = rd::<i64>(b.get(off..off + 8)?, 0);
                    off += 8;
                    apply(v, std::mem::take(&mut batch), next);
                }
                _ => return None,
            }
            Some(())
        })();
        if ok.is_none() {
            break;
        }
    }
}

pub fn save(v: &mut Volume) -> io::Result<()> {
    write_cache(v)?;
    v.dirty = false;
    Ok(())
}

/// Save a shared volume without blocking searches: the file is written under a read lock (a second or two for
/// a big disk) and the write lock is taken only to clear the flag. Call it from the thread that updates the volume.
pub fn save_shared(vol: &RwLock<Volume>) -> io::Result<()> {
    write_cache(&vol.read().unwrap())?;
    vol.write().unwrap().dirty = false;
    Ok(())
}

/// Make sure `buf[*off..*have]` holds at least `want` bytes, moving what is left to the front and reading more.
fn ensure(r: &mut impl io::Read, buf: &mut [u8], have: &mut usize, off: &mut usize, want: usize) -> Option<()> {
    while *have - *off < want {
        if *off > 0 {
            buf.copy_within(*off..*have, 0);
            *have -= *off;
            *off = 0;
        }
        let got = r.read(&mut buf[*have..]).ok()?;
        if got == 0 {
            return None; // the file ends too soon
        }
        *have += got;
    }
    Some(())
}

pub(super) fn load(root: &str, walk: bool) -> Option<Volume> {
    let mut file = std::fs::File::open(cache_path(root)).ok()?;
    let len = file.metadata().ok()?.len() as usize;
    // read in pieces of 1 MB: the file can be 100 MB, and holding all of it next to the index it becomes is a peak
    // for nothing
    let mut buf = vec![0u8; 1 << 20];
    let (mut have, mut off) = (0, 0);
    ensure(&mut file, &mut buf, &mut have, &mut off, 28)?;
    if &buf[..4] != b"LSD3" {
        return None;
    }
    let (journal_id, next_usn, n) = (rd::<u64>(&buf, 4), rd::<i64>(&buf, 12), rd::<u64>(&buf, 20) as usize);
    if n > len / 35 {
        return None; // every record is at least its 35-byte head: a damaged header
    }
    off = 28;
    // the names are what is left of the file after the heads, so the index can be sized exactly
    let mut idx = Index::with_names(n, len.saturating_sub(28 + 35 * n));
    for _ in 0..n {
        ensure(&mut file, &mut buf, &mut have, &mut off, 35)?;
        let nlen = rd::<u16>(&buf, off + 33) as usize;
        ensure(&mut file, &mut buf, &mut have, &mut off, 35 + nlen)?;
        let name = std::str::from_utf8(&buf[off + 35..off + 35 + nlen]).ok()?;
        let rec = Rec { parent: rd(&buf, off + 8), time: rd(&buf, off + 16), size: rd(&buf, off + 24), is_dir: buf[off + 32] != 0, hidden: false };
        idx.insert_rec(rd(&buf, off), name, rec);
        off += 35 + nlen;
    }
    let mut v = Volume { root: root.to_string(), walk, idx, journal_id, next_usn, dirty: false };
    if !walk {
        replay_log(&mut v);
        v.dirty = false; // what the log brought in is safe on disk
    }
    Some(v)
}

/// Cached index caught up through the journal, or a fresh full scan when the cache is
/// missing, from another journal, or too old.
pub fn open(letter: char) -> Result<Volume, String> {
    if let Some(mut v) = load(&format!("{letter}:"), false) {
        let live = open_volume(letter).ok().and_then(|h| query_journal(&h));
        if live.is_some_and(|j| j.UsnJournalID == v.journal_id && v.next_usn >= j.FirstUsn) {
            if let Ok((c, n)) = read_changes(&v) {
                // into the log, so a start does not rewrite the snapshot (see `recount_shared` for the folder totals)
                let logged = append_log(&v.root, &c, n);
                apply(&mut v, c, n);
                let (root, before) = (v.root.clone(), v.idx.len());
                prune(&mut v.idx, &root);
                v.dirty = !logged || v.idx.len() != before;
                return Ok(v);
            }
        }
    }
    scan_volume(letter)
}

pub fn live(v: &Volume) -> bool {
    v.journal_id != 0
}

/// Delete the caches of the one-letter disks, so that the next start rebuilds them (used after a "do not index" rule
/// was removed or changed: what it had left out has to be found again).
pub fn wipe_local_caches() {
    let Ok(rd) = std::fs::read_dir(data_dir()) else { return };
    for p in rd.flatten().map(|e| e.path()) {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem.len() == 1 && matches!(p.extension().and_then(|s| s.to_str()), Some("idx" | "log")) {
            let _ = std::fs::remove_file(p);
        }
    }
}
