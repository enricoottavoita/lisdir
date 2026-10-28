//! Modified time and size from a raw NTFS file record. The USN records the index is built from carry neither,
//! so `engine` fetches each file's record (FSCTL_GET_NTFS_FILE_RECORD) and reads these two values out of it.
use crate::engine::filetime_to_unix;

pub struct Meta {
    /// last modification, unix seconds
    pub mtime: i64,
    /// size of the unnamed data stream in bytes (0 for folders)
    pub size: u64,
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// Undo the update sequence: NTFS overwrites the last two bytes of every 512-byte sector with a check value.
/// A sector that does not carry the check value is left alone: the file-record ioctl may hand out records that
/// were already restored.
fn apply_fixups(rec: &mut [u8]) -> Option<()> {
    let (at, count) = (u16_at(rec, 4)? as usize, u16_at(rec, 6)? as usize);
    let usa = rec.get(at..at + 2 * count)?.to_vec();
    for i in 1..count {
        let end = i * 512;
        let tail = rec.get_mut(end - 2..end)?;
        if tail == &usa[..2] {
            tail.copy_from_slice(&usa[2 * i..2 * i + 2]);
        }
    }
    Some(())
}

/// Read the metadata of one in-use base file record (modified in place by the fixups).
/// ponytail: a file whose data attribute lives in an extension record (attribute list) reports size 0.
pub fn parse(rec: &mut [u8]) -> Option<Meta> {
    if rec.get(..4)? != b"FILE" {
        return None;
    }
    apply_fixups(rec)?;
    if u16_at(rec, 0x16)? & 1 == 0 || u64_at(rec, 0x20)? != 0 {
        return None; // not in use, or an extension of another record
    }
    let (mut mtime, mut size) = (None, None);
    let mut off = u16_at(rec, 0x14)? as usize;
    while let Some(kind) = u32_at(rec, off).filter(|&k| k != 0xFFFF_FFFF) {
        let len = u32_at(rec, off + 4)? as usize;
        if len < 16 || off + len > rec.len() {
            break;
        }
        let (non_resident, name_len) = (rec[off + 8] != 0, rec[off + 9]);
        match kind {
            // $STANDARD_INFORMATION: created, modified, record changed, accessed
            0x10 if !non_resident => {
                let content = off + u16_at(rec, off + 0x14)? as usize;
                mtime = Some(filetime_to_unix(u64_at(rec, content + 8)? as i64));
            }
            // $DATA without a name (named ones are alternate streams)
            0x80 if name_len == 0 && size.is_none() => {
                size = Some(if non_resident { u64_at(rec, off + 0x30)? } else { u32_at(rec, off + 0x10)? as u64 });
            }
            _ => {}
        }
        off += len;
    }
    Some(Meta { mtime: mtime?, size: size.unwrap_or(0) })
}

#[cfg(test)]
pub fn sample_record(mtime_unix: i64, data: Option<(bool, u64)>) -> Vec<u8> {
    // 1024-byte record: header, $STANDARD_INFORMATION, optional $DATA (resident or not), end marker
    let mut r = vec![0u8; 1024];
    r[..4].copy_from_slice(b"FILE");
    r[4..6].copy_from_slice(&0x30u16.to_le_bytes()); // update sequence array at 0x30
    r[6..8].copy_from_slice(&3u16.to_le_bytes()); // check value + two sectors
    r[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes()); // first attribute
    r[0x16..0x18].copy_from_slice(&1u16.to_le_bytes()); // in use
    let mut off = 0x38;
    r[off..off + 4].copy_from_slice(&0x10u32.to_le_bytes());
    r[off + 4..off + 8].copy_from_slice(&0x60u32.to_le_bytes());
    r[off + 0x14..off + 0x16].copy_from_slice(&0x18u16.to_le_bytes()); // content offset
    let ft = ((mtime_unix + 11_644_473_600) * 10_000_000) as u64;
    r[off + 0x18 + 8..off + 0x18 + 16].copy_from_slice(&ft.to_le_bytes());
    off += 0x60;
    if let Some((resident, size)) = data {
        r[off..off + 4].copy_from_slice(&0x80u32.to_le_bytes());
        r[off + 4..off + 8].copy_from_slice(&0x50u32.to_le_bytes());
        if resident {
            r[off + 0x10..off + 0x14].copy_from_slice(&(size as u32).to_le_bytes());
        } else {
            r[off + 8] = 1;
            r[off + 0x30..off + 0x38].copy_from_slice(&size.to_le_bytes());
        }
        off += 0x50;
    }
    r[off..off + 4].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    // fixups: the sector ends hold the check value 0xBEEF, the originals go to the array
    r[0x30..0x32].copy_from_slice(&0xBEEFu16.to_le_bytes());
    for i in 1..3 {
        let end = i * 512;
        let (a, b) = (r[end - 2], r[end - 1]);
        r[0x30 + 2 * i] = a;
        r[0x30 + 2 * i + 1] = b;
        r[end - 2..end].copy_from_slice(&0xBEEFu16.to_le_bytes());
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_time_and_size() {
        let m = parse(&mut sample_record(1_700_000_000, Some((false, 5_000_000_000)))).unwrap();
        assert_eq!((m.mtime, m.size), (1_700_000_000, 5_000_000_000)); // big file: non-resident
        let m = parse(&mut sample_record(1_700_000_000, Some((true, 120)))).unwrap();
        assert_eq!(m.size, 120); // small file: data stored in the record
        let m = parse(&mut sample_record(1_600_000_000, None)).unwrap();
        assert_eq!((m.mtime, m.size), (1_600_000_000, 0)); // folder: no data stream
    }

    #[test]
    fn accepts_records_whose_fixups_are_already_applied() {
        let mut r = sample_record(1_700_000_000, Some((false, 777)));
        let mut fixed = r.clone();
        apply_fixups(&mut fixed).unwrap(); // restore once ...
        assert_eq!(parse(&mut fixed).map(|m| m.size), Some(777)); // ... and parsing again must still work
        assert_eq!(parse(&mut r).map(|m| m.size), Some(777));
    }

    #[test]
    fn rejects_bad_records() {
        let mut r = sample_record(1, None);
        r[0x16] = 0; // not in use
        assert!(parse(&mut r).is_none());
        let mut r = sample_record(1, None);
        r[0] = b'X';
        assert!(parse(&mut r).is_none());
        assert!(parse(&mut [0u8; 8]).is_none());
    }
}
