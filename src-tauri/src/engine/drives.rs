//! Which disks exist, and which of them lisdir indexes (and how).
use super::{cache::data_dir, wide};
use std::ptr::null_mut;
use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
/// A disk lisdir knows about, and how it is indexed.
pub struct Drive {
    pub letter: String,
    pub label: String,
    /// "ntfs" (MFT index by the helper), "walk" (another local file system, walked) or "network" (walked)
    pub kind: &'static str,
    /// false for a removable disk that was seen before and is unplugged now (its results are kept)
    pub present: bool,
    pub indexed: bool,
}

/// Volume label and file system name ("NTFS", "exFAT", ...); empty when the drive cannot be read.
fn volume_info(letter: char) -> (String, String) {
    let root = wide(&format!("{letter}:\\"));
    let (mut label, mut fs) = ([0u16; 261], [0u16; 64]);
    let ok = unsafe { GetVolumeInformationW(root.as_ptr(), label.as_mut_ptr(), 261, null_mut(), null_mut(), null_mut(), fs.as_mut_ptr(), 64) };
    let text = |b: &[u16]| String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]);
    if ok == 0 { (String::new(), String::new()) } else { (text(&label), text(&fs)) }
}

/// Drive letters of non-NTFS local disks seen in earlier sessions (walked.txt), so an unplugged stick keeps its results.
fn remembered() -> Vec<String> {
    let text = std::fs::read_to_string(data_dir().join("walked.txt")).unwrap_or_default();
    text.lines().map(|l| l.trim().to_uppercase()).filter(|l| l.len() == 1 && l.chars().all(|c| c.is_ascii_alphabetic())).collect()
}

fn remember(letter: &str) {
    if !remembered().iter().any(|l| l == letter) {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("walked.txt")).and_then(|mut f| {
            use std::io::Write;
            writeln!(f, "{letter}")
        });
    }
}

/// Every disk to index or not: mapped network drives, local disks (NTFS ones go to the helper, the others are walked),
/// and removable disks remembered from before. Optical drives and ReFS volumes are left out.
pub fn drives(disks_off: &[String]) -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let off = |l: &str| disks_off.iter().any(|d| d.eq_ignore_ascii_case(l));
    let mut out = vec![];
    for l in 'A'..='Z' {
        if mask >> (l as u8 - b'A') & 1 == 0 {
            continue;
        }
        let path = wide(&format!("{l}:\\"));
        let (label, fs) = volume_info(l);
        let kind = match unsafe { GetDriveTypeW(path.as_ptr()) } {
            4 => "network",
            2 | 3 if fs == "NTFS" => "ntfs",
            2 | 3 if !fs.is_empty() && fs != "ReFS" => "walk",
            _ => continue,
        };
        out.push(Drive { letter: l.to_string(), label, kind, present: true, indexed: !off(&l.to_string()) });
    }
    for r in remembered() {
        if !out.iter().any(|d| d.letter == r) {
            let indexed = !off(&r);
            out.push(Drive { letter: r, label: String::new(), kind: "walk", present: false, indexed });
        }
    }
    out
}

/// What the UI process walks: network drives and non-NTFS local disks that are switched on, the folders chosen in
/// the settings, and the paths of the older roots.txt (one per line, `#` comments).
pub fn walk_roots(folders: &[String], disks_off: &[String]) -> Vec<String> {
    let mut roots: Vec<String> = vec![];
    for d in drives(disks_off) {
        if d.kind == "walk" && d.present {
            remember(&d.letter);
        }
        if d.indexed && matches!(d.kind, "network" | "walk") {
            roots.push(format!("{}:", d.letter));
        }
    }
    let listed = std::fs::read_to_string(data_dir().join("roots.txt")).unwrap_or_default();
    let from_file = listed.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(String::from);
    for r in from_file.chain(folders.iter().cloned()) {
        let r = r.trim_end_matches('\\').to_string();
        if !r.is_empty() && !roots.iter().any(|x| x.eq_ignore_ascii_case(&r)) {
            roots.push(r);
        }
    }
    roots
}

/// Whether `letter` is a mapped network drive (the NTFS pass skips those).
pub fn is_network_drive(letter: char) -> bool {
    let p = wide(&format!("{letter}:\\"));
    unsafe { GetDriveTypeW(p.as_ptr()) == 4 } // DRIVE_REMOTE
}
