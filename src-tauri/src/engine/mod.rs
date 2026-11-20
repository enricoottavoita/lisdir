//! NTFS file index: full MFT enumeration (FSCTL_ENUM_USN_DATA), live updates by tailing the USN journal,
//! an on-disk cache, and fuzzy name search. Needs an elevated process (raw volume handle).

mod apply;
mod cache;
mod drives;
mod index;
mod search;
mod usn;
mod walk;

pub use apply::{apply, recount_shared};
pub use cache::{append_log, data_dir, live, log_is_big, open, save, save_shared, wipe_local_caches};
pub use drives::{drives, is_network_drive, walk_roots};
pub use index::{full_path, Index};
pub use search::{highlight, name_bonus, score, search, FIRST};
pub use usn::{filetime_to_unix, read_changes, scan_volume};
pub use walk::{next_walk, open_walk, refresh_path, reported, set_reported, walk, watch, MAX_CHANGES, RETRY_INTERVAL, WALK_INTERVAL, WATCHED_WALK_INTERVAL};
/// CREATE_NO_WINDOW: no console flash for a child process.
pub const NO_WINDOW: u32 = 0x0800_0000;

/// A string as the NUL-terminated UTF-16 that Windows APIs take.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

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

pub enum Change {
    Upsert(u64, u64, String, bool, i64),
    Delete(u64),
    /// modified time and size read from the file record, for an entry upserted earlier in the same batch
    Meta(u64, i64, u64),
}

/// A small fixed volume for tests in other modules.
#[cfg(test)]
pub fn sample_volume() -> Volume {
    use index::Entry;
    let mut idx = Index::new();
    idx.insert(1, Entry::new(0, "Docs".into(), true, 1_700_000_000));
    idx.insert(2, Entry::new(1, "report 2024.pdf".into(), false, 1_700_000_000).with_size(2048));
    idx.insert(3, Entry::new(1, "draft report.docx".into(), false, 1_600_000_000));
    Volume { root: "C:".into(), walk: false, idx, journal_id: 7, next_usn: 0, dirty: false }
}

#[cfg(test)]
mod tests;
