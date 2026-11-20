//! Keeping an index current: the "do not index" / "exclude" rules, folder sizes, and applying a batch of changes.
use super::{index::{full_path, Entry, Index}, Change, Volume};
use crate::rules;
use std::{collections::HashMap, sync::RwLock};
/// What the rules decide for an entry. Whatever is decided for a folder holds for everything below it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    /// indexed and shown
    Keep,
    /// indexed but kept out of the results (an "exclude" rule)
    Hide,
    /// not indexed at all (a "do not index" rule)
    Skip,
}

impl Verdict {
    /// What the children of a folder start from: hidden if the folder is.
    pub(super) fn inherited(hidden: bool) -> Self {
        if hidden { Verdict::Hide } else { Verdict::Keep }
    }

    /// What the rules say about an entry, given what they said about its folder. `path` is the entry's lowercase path
    /// when it is a folder (and there are path rules), else None.
    pub(super) fn of(act: &rules::Active, name_lower: &str, path: Option<&str>, inherited: Verdict) -> Verdict {
        if inherited == Verdict::Skip || act.skip.hits(name_lower, path) {
            Verdict::Skip
        } else if inherited == Verdict::Hide || act.hide.hits(name_lower, path) {
            Verdict::Hide
        } else {
            Verdict::Keep
        }
    }
}

/// Apply the "do not index" and "exclude" rules to an index just built or loaded: remove what is not to be indexed
/// (with everything below it) and flag what is not to be shown.
pub fn prune(idx: &mut Index, root: &str) {
    prune_with(idx, root, &rules::active());
}

pub(super) fn prune_with(idx: &mut Index, root: &str, act: &rules::Active) {
    if act.skip.is_empty() && act.hide.is_empty() {
        return;
    }
    let need_path = act.skip.has_paths() || act.hide.has_paths();
    let mut memo: Vec<Option<Verdict>> = vec![None; idx.slots()]; // by position
    for i in 0..idx.slots() {
        if memo[i].is_some() || !idx.is_live(i) {
            continue;
        }
        // climb to the first folder whose verdict is known, then come back down
        let (mut chain, mut base, mut cur) = (vec![i], Verdict::Keep, i);
        while let Some(p) = idx.position(idx.parent_at(cur)) {
            if let Some(known) = memo[p] {
                base = known;
                break;
            }
            if p == cur || chain.len() > 4096 {
                break;
            }
            chain.push(p);
            cur = p;
        }
        for &c in chain.iter().rev() {
            let path = (need_path && idx.is_dir_at(c)).then(|| full_path(idx, idx.frn_at(c), root).to_lowercase());
            base = Verdict::of(act, &idx.name_at(c).to_lowercase(), path.as_deref(), base);
            memo[c] = Some(base);
        }
    }
    idx.retain(|i, _, hidden| match memo[i] {
        Some(Verdict::Skip) => false,
        Some(Verdict::Hide) => {
            *hidden = true;
            true
        }
        _ => true,
    });
}

/// Add `delta` bytes to the folder `parent` and every folder above it. Bounded: the NTFS root is its own parent,
/// and a damaged index could hold a loop.
pub(super) fn add_up(idx: &mut Index, mut parent: u64, delta: i64) {
    for _ in 0..256 {
        let Some(p) = idx.add_size(parent, delta) else { return };
        if p == parent {
            return;
        }
        parent = p;
    }
}

/// What each folder holds: one pass over the entries to total each folder's own files, then each total walks up.
/// Only reads the index, so searches can run meanwhile.
fn folder_totals(idx: &Index) -> HashMap<u64, u64> {
    let mut own: HashMap<u64, u64> = HashMap::new();
    for (_, e) in idx.iter().filter(|(_, e)| !e.is_dir && e.size > 0) {
        *own.entry(e.parent).or_default() += e.size;
    }
    let mut totals: HashMap<u64, u64> = HashMap::with_capacity(own.len());
    for (mut cur, size) in own {
        for _ in 0..256 {
            let Some(folder) = idx.get(&cur) else { break };
            *totals.entry(cur).or_default() += size;
            if folder.parent == cur {
                break;
            }
            cur = folder.parent;
        }
    }
    totals
}

/// Set each folder's size to the sum of the files below it. Once per scan or walk; `apply` keeps the totals current.
pub fn sum_folders(idx: &mut Index) {
    let totals = folder_totals(idx);
    idx.set_folder_sizes(&totals);
}

/// `sum_folders` for a volume that is already being searched: a cache brings its totals along, so this only repairs any
/// drift, without blocking the searches. Call it from the thread that updates the volume.
pub fn recount_shared(vol: &RwLock<Volume>) {
    let totals = folder_totals(&vol.read().unwrap().idx);
    vol.write().unwrap().idx.set_folder_sizes(&totals);
}

pub fn apply(v: &mut Volume, changes: Vec<Change>, next_usn: i64) {
    apply_with(v, changes, next_usn, &rules::active());
}

pub(super) fn apply_with(v: &mut Volume, changes: Vec<Change>, next_usn: i64, act: &rules::Active) {
    v.dirty |= !changes.is_empty();
    let ruled = !act.skip.is_empty() || !act.hide.is_empty();
    for c in changes {
        match c {
            Change::Upsert(frn, parent, name, is_dir, time) => {
                let mut e = Entry::new(parent, name, is_dir, time);
                if ruled {
                    // a folder we do not index leaves its children without a parent in the index (the volume's root,
                    // number 5, is never in it), which is how they are recognised
                    let parent_state = match v.idx.get(&parent) {
                        Some(p) => Verdict::inherited(p.hidden),
                        None if !act.skip.is_empty() && parent & 0xFFFF_FFFF_FFFF != 5 => Verdict::Skip,
                        None => Verdict::Keep,
                    };
                    let path = ((act.skip.has_paths() || act.hide.has_paths()) && is_dir).then(|| {
                        let base = if v.idx.contains_key(&parent) { full_path(&v.idx, parent, &v.root) } else { v.root.clone() };
                        format!("{base}\\{}", e.name).to_lowercase()
                    });
                    match Verdict::of(act, &e.name.to_lowercase(), path.as_deref(), parent_state) {
                        Verdict::Skip => {
                            if let Some(old) = v.idx.remove(&frn) {
                                add_up(&mut v.idx, old.parent, -(old.size as i64));
                            }
                            continue;
                        }
                        Verdict::Hide => e.hidden = true,
                        _ => {}
                    }
                }
                // a moved or renamed entry takes its bytes out of the old folders; a folder keeps its total
                if let Some(old) = v.idx.get(&frn) {
                    let (old_parent, old_size, was_dir) = (old.parent, old.size, old.is_dir);
                    add_up(&mut v.idx, old_parent, -(old_size as i64));
                    if was_dir && is_dir {
                        e.size = old_size;
                    }
                }
                let size = e.size;
                v.idx.insert(frn, e);
                add_up(&mut v.idx, parent, size as i64);
            }
            Change::Delete(frn) => {
                if let Some(old) = v.idx.remove(&frn) {
                    add_up(&mut v.idx, old.parent, -(old.size as i64));
                }
            }
            Change::Meta(frn, time, size) => {
                let Some(e) = v.idx.get(&frn) else { continue };
                v.idx.set_meta(&frn, time, if e.is_dir { e.size } else { size });
                if !e.is_dir {
                    add_up(&mut v.idx, e.parent, size as i64 - e.size as i64);
                }
            }
        }
    }
    v.next_usn = next_usn;
}
