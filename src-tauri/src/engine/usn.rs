//! Reading an NTFS volume: the MFT enumeration, the file records and the USN journal. Needs an elevated process.
use super::{apply::{prune, sum_folders}, index::{Entry, Index}, wide, Change, Volume};
use crate::mft;
use std::{ffi::c_void, mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_HANDLE_EOF, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
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

/// A decoded USN_RECORD_V2.
pub(super) struct UsnRec {
    pub(super) frn: u64,
    pub(super) parent: u64,
    pub(super) time: i64,
    pub(super) reason: u32,
    pub(super) is_dir: bool,
    pub(super) name: String,
}

pub(super) fn rd<T: Copy>(b: &[u8], off: usize) -> T {
    unsafe { (b.as_ptr().add(off) as *const T).read_unaligned() }
}

/// A name inside a Windows record: UTF-16, little endian.
pub(super) fn utf16le(b: &[u8]) -> String {
    String::from_utf16_lossy(&b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
}

/// FILETIME (100 ns ticks since 1601) to unix seconds.
pub fn filetime_to_unix(ft: i64) -> i64 {
    (ft / 10_000_000 - 11_644_473_600).max(0)
}

/// Decode the records of an ENUM/READ output buffer (after its 8-byte header).
pub(super) fn parse_records(buf: &[u8]) -> Vec<UsnRec> {
    let (mut out, mut off) = (vec![], 0);
    while off + 60 <= buf.len() {
        let len = rd::<u32>(buf, off) as usize;
        if len == 0 || off + len > buf.len() {
            break;
        }
        // USN_RECORD_V2 layout: frn@8, parent@16, usn@24, time@32, reason@40, attrs@52, name_len@56, name_off@58
        let (nlen, noff) = (rd::<u16>(buf, off + 56) as usize, rd::<u16>(buf, off + 58) as usize);
        out.push(UsnRec {
            frn: rd(buf, off + 8),
            parent: rd(buf, off + 16),
            time: filetime_to_unix(rd(buf, off + 32)),
            reason: rd(buf, off + 40),
            is_dir: rd::<u32>(buf, off + 52) & 0x10 != 0,
            name: utf16le(&buf[off + noff..off + noff + nlen]),
        });
        off += len;
    }
    out
}

pub(super) struct Handle(HANDLE);
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn open_volume(letter: char) -> Result<Handle, String> {
    let path = wide(&format!(r"\\.\{letter}:"));
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

pub(super) fn query_journal(h: &Handle) -> Option<USN_JOURNAL_DATA_V0> {
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
    let frns: Vec<u64> = idx.keys().collect();
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
        idx.set_meta(&f, m.mtime, m.size);
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
    prune(&mut idx, &format!("{letter}:"));
    sum_folders(&mut idx);
    idx.shrink_to_fit();
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

#[cfg(test)]
mod tests {
    use super::*;

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
            let raw = ioctl(&h, FSCTL_GET_NTFS_FILE_RECORD, &NTFS_FILE_RECORD_INPUT_BUFFER { FileReferenceNumber: frn as i64 }, probe.as_mut_ptr(), probe.len());
            eprintln!("record {frn}: ioctl {raw:?}, header frn {:#x} len {}, starts {:?}", rd::<u64>(&probe, 0), rd::<u32>(&probe, 8), &probe[12..16]);
            let head: Vec<u16> = [4, 6, 0x14, 0x16].iter().map(|&o| rd::<u16>(&probe, 12 + o)).collect();
            eprintln!("record {frn}: usa offset/count, first attr, flags = {head:x?}, base ref {:#x}", rd::<u64>(&probe, 12 + 0x20));
            let m = meta_of(&h, frn, &mut probe);
            eprintln!("record {frn}: {:?}", m.map(|m| (m.mtime, m.size)));
        }
        let t = std::time::Instant::now();
        fill_meta(&h, &mut idx);
        let known = idx.values().filter(|e| e.time != 0).count();
        let big = idx.iter_named().max_by_key(|(_, e, _)| e.size).unwrap();
        println!("{} entries, metadata in {:?}; biggest: {} ({} bytes)", idx.len(), t.elapsed(), big.2, big.1.size);
        assert!(idx.values().any(|e| e.size > 0), "no sizes were read");
        assert!(known * 10 > idx.len() * 9, "fewer than 90% of the records could be read");
    }
}
