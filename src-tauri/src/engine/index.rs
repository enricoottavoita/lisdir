//! The in-memory index of one volume: entries in a flat vector, found by file reference number.
/// An entry as it is handed to the index (`Index::insert`). The index keeps its name in one block of bytes and the rest as
/// a `Rec`.
pub struct Entry {
    pub(super) parent: u64,
    pub name: String,
    pub is_dir: bool,
    /// last modification, unix seconds (0 = unknown). NTFS: from the file record; until that is read, the time of
    /// the file's last USN record.
    pub(super) time: i64,
    /// bytes (0 when unknown); for a folder, the sum of the files below it (see `sum_folders`)
    pub size: u64,
    /// indexed but kept out of the results by an "exclude" rule (itself or a folder above it)
    pub hidden: bool,
}

/// An entry as the index hands it out: a copy put together from `Item` and `Index::hot` (its name is in `Index::names`).
#[derive(Clone, Copy)]
pub struct Rec {
    pub(super) parent: u64,
    pub time: i64,
    pub size: u64,
    pub is_dir: bool,
    pub hidden: bool,
}

/// What the index keeps per entry, 28 bytes: the two flags are in `Index::hot`, and a time in 4 bytes lasts until 2106.
/// Packed, or the time would take 8.
#[repr(C, packed(4))]
struct Item {
    frn: u64,
    parent: u64,
    size: u64,
    time: u32,
}

/// One bit per letter a-z (case does not matter), one for all digits, one for anything else. A word can only match
/// a name whose bits include all of the word's own bits.
pub(super) fn letter_bits(s: &str) -> u32 {
    if s.is_ascii() {
        // nearly every name: by bytes (setting bit 5 turns a capital letter into its small one)
        return s.bytes().fold(0, |m, b| m | 1 << match b | 0x20 {
            l @ b'a'..=b'z' => (l - b'a') as u32,
            _ if b.is_ascii_digit() => 26,
            _ => 27,
        });
    }
    s.chars().flat_map(char::to_lowercase).fold(0, |m, c| m | 1 << match c {
        'a'..='z' => c as u32 - 'a' as u32,
        '0'..='9' => 26,
        _ => 27,
    })
}

impl Entry {
    pub(super) fn new(parent: u64, name: String, is_dir: bool, time: i64) -> Self {
        Entry { parent, name, is_dir, time, size: 0, hidden: false }
    }

    pub(super) fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }
}

/// File reference number to position in `Index::items`, as a table of 4-byte positions with open addressing: the number
/// itself is read back from `items`, so it is not kept twice. A map from numbers to positions takes about 28 bytes an
/// entry over a few million entries; this takes about 6.
#[derive(Default)]
struct Lookup {
    cells: Vec<u32>,
    /// positions in the table
    live: usize,
    /// cells that are not free: positions and the marks of removed ones
    used: usize,
}

const FREE: u32 = u32::MAX;
const GONE: u32 = u32::MAX - 1;

type Items = [Item];

impl Lookup {
    /// Cells for `n` positions, at most 70% full (a power of two, so that the number needs no division).
    fn cells_for(n: usize) -> usize {
        (n * 10 / 7 + 1).next_power_of_two().max(16)
    }

    fn with_capacity(n: usize) -> Self {
        Lookup { cells: vec![FREE; Self::cells_for(n)], live: 0, used: 0 }
    }

    fn hash(frn: u64) -> usize {
        (frn.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize
    }

    /// The cell that holds `frn`.
    fn cell(&self, items: &Items, frn: u64) -> Option<usize> {
        if self.cells.is_empty() {
            return None;
        }
        let mask = self.cells.len() - 1;
        let mut c = Self::hash(frn) & mask;
        loop {
            match self.cells[c] {
                FREE => return None,
                GONE => {}
                p if items[p as usize].frn == frn => return Some(c),
                _ => {}
            }
            c = (c + 1) & mask;
        }
    }

    fn get(&self, items: &Items, frn: u64) -> Option<u32> {
        self.cell(items, frn).map(|c| self.cells[c])
    }

    /// Record that `frn` (not in the table yet) will be at position `pos`, the next one of `items`.
    fn add(&mut self, items: &Items, hot: &[u32], frn: u64, pos: u32) {
        if (self.used + 1) * 10 > self.cells.len() * 7 {
            self.rebuild(items, hot, self.live + 1);
        }
        let mask = self.cells.len() - 1;
        let mut c = Self::hash(frn) & mask;
        while self.cells[c] != FREE && self.cells[c] != GONE {
            c = (c + 1) & mask;
        }
        if self.cells[c] == FREE {
            self.used += 1;
        }
        self.cells[c] = pos;
        self.live += 1;
    }

    fn remove(&mut self, items: &Items, frn: u64) -> Option<u32> {
        let c = self.cell(items, frn)?;
        let pos = std::mem::replace(&mut self.cells[c], GONE);
        self.live -= 1;
        Some(pos)
    }

    /// Make the table again from the live entries in `items`, with room for `n`.
    fn rebuild(&mut self, items: &Items, hot: &[u32], n: usize) {
        self.cells = vec![FREE; Self::cells_for(n)];
        let mask = self.cells.len() - 1;
        self.live = 0;
        for (p, item) in items.iter().enumerate() {
            if hot[p] & DEAD == 0 {
                let mut c = Self::hash(item.frn) & mask;
                while self.cells[c] != FREE {
                    c = (c + 1) & mask;
                }
                self.cells[c] = p as u32;
                self.live += 1;
            }
        }
        self.used = self.live;
    }
}

/// Holes left by removed entries are squeezed out once there are this many (and over an eighth of the vector).
const COMPACT_MIN: usize = 10_000;

/// Marks a removed entry in `Index::hot` (the letter bits use the low 28 bits).
const DEAD: u32 = 1 << 31;

/// The letter bits of `Index::hot`.
const LETTERS: u32 = (1 << 28) - 1;

/// Marks a folder in `Index::hot`.
const DIR: u32 = 1 << 29;

/// Marks an entry kept out of the results in `Index::hot`.
const HIDDEN: u32 = 1 << 28;

/// Marks a name made only of ASCII characters in `Index::hot`: it can be compared without case by bytes.
const ASCII: u32 = 1 << 30;

/// The entries of a volume by file reference number. They live in one flat vector (plus a lookup table), so a
/// search walks them in order and can hand slices to several threads, which a hash map does not allow. A removed
/// entry leaves a hole that is cleaned up later.
///
/// What a search needs from every entry sits apart from the entries, in two small arrays and one block of bytes,
/// all in the same order as `items`: `hot` (which letters the name holds, see `letter_bits`; `DEAD` for a hole; `ASCII`,
/// `DIR`, `HIDDEN`),
/// `pos` (where the name starts in `names`) and `names` (each name as a 2-byte length and its UTF-8 bytes, back to back).
/// So a search reads about 8 bytes per entry to rule most of them out, and a name is kept once, as it was given: it is
/// compared without case as it is read.
#[derive(Default)]
pub struct Index {
    slot: Lookup,
    items: Vec<Item>,
    hot: Vec<u32>,
    pos: Vec<u32>,
    names: Vec<u8>,
    /// bytes of `names` that belong to removed or renamed entries (they are dropped when the block is rebuilt)
    waste: usize,
    /// The vectors were sized to fit (at load, or after a rebuild): growing them by doubling, as `Vec` does, would
    /// reserve as much again as the whole index for a few new files, so they grow by a thirty-second instead.
    tight: bool,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    pub fn with_capacity(n: usize) -> Self {
        Self::with_names(n, n * 32) // about 31 bytes per name
    }

    /// Room for `n` entries whose names add up to `name_bytes` (the 2-byte lengths are added here), plus a little for
    /// what is added later.
    pub fn with_names(n: usize, name_bytes: usize) -> Self {
        let room = n + n / 64 + 4096;
        Index {
            slot: Lookup::with_capacity(room),
            items: Vec::with_capacity(room),
            hot: Vec::with_capacity(room),
            pos: Vec::with_capacity(room),
            names: Vec::with_capacity(2 * room + name_bytes + name_bytes / 32),
            waste: 0,
            tight: true,
        }
    }

    /// An empty index with room for as much as this one holds, so a new walk of the same disk does not grow by doubling.
    pub fn sized_like(&self) -> Self {
        Self::with_names(self.len(), self.names.len())
    }

    /// Give back the room the vectors do not use (after an index was built by adding entries one by one).
    pub fn shrink_to_fit(&mut self) {
        self.slot.rebuild(&self.items, &self.hot, self.slot.live);
        self.items.shrink_to_fit();
        self.hot.shrink_to_fit();
        self.pos.shrink_to_fit();
        self.names.shrink_to_fit();
        self.tight = true;
    }

    /// Make room for more entries in small steps (see `tight`).
    fn grow(&mut self) {
        let more = (self.items.len() / 32).max(4096);
        self.items.reserve_exact(more);
        self.hot.reserve_exact(more);
        self.pos.reserve_exact(more);
    }

    /// The name of the entry in position `i` of `items`, as it was given.
    pub(super) fn name_at(&self, i: usize) -> &str {
        let p = self.pos[i] as usize;
        let n = u16::from_le_bytes([self.names[p], self.names[p + 1]]) as usize;
        // SAFETY: `names` only ever receives the bytes of a `&str` (see `store_name`), cut at its own length.
        unsafe { std::str::from_utf8_unchecked(&self.names[p + 2..p + 2 + n]) }
    }

    /// Append `name` to `names`: its position and its bits for `hot`.
    fn store_name(&mut self, name: &str) -> (u32, u32) {
        if self.tight && self.names.capacity() - self.names.len() < 2 + name.len() {
            self.names.reserve_exact((self.names.len() / 32).max(1 << 16));
        }
        let at = self.names.len() as u32;
        let name = if name.len() <= u16::MAX as usize { name } else { "" }; // never happens: names are at most 255 characters
        self.names.extend_from_slice(&(name.len() as u16).to_le_bytes());
        self.names.extend_from_slice(name.as_bytes());
        (at, letter_bits(name) | if name.is_ascii() { ASCII } else { 0 })
    }

    /// The entry in position `i` of `items`.
    pub(super) fn rec_at(&self, i: usize) -> Rec {
        let (e, hot) = (&self.items[i], self.hot[i]);
        Rec { parent: e.parent, time: e.time as i64, size: e.size, is_dir: hot & DIR != 0, hidden: hot & HIDDEN != 0 }
    }

    /// The entry and its name.
    pub(super) fn get_named(&self, frn: &u64) -> Option<(Rec, &str)> {
        let i = self.slot.get(&self.items, *frn)? as usize;
        Some((self.rec_at(i), self.name_at(i)))
    }

    /// The name of the entry with this file reference number.
    pub fn name(&self, frn: &u64) -> Option<&str> {
        self.get_named(frn).map(|(_, name)| name)
    }

    /// The entry called `name` (any case) in the folder `parent`. A pass over `hot`: under a millisecond for a million
    /// entries, since few names hold exactly the same letters.
    pub(super) fn child(&self, parent: u64, name: &str) -> Option<u64> {
        let (bits, lower) = (letter_bits(name), name.to_lowercase());
        let i = (0..self.hot.len()).find(|&i| self.hot[i] & (DEAD | LETTERS) == bits && self.items[i].parent == parent && eq_fold(self.name_at(i), &lower, self.hot[i] & ASCII != 0))?;
        Some(self.items[i].frn)
    }

    pub fn len(&self) -> usize {
        self.slot.live
    }

    pub fn is_empty(&self) -> bool {
        self.slot.live == 0
    }

    pub fn contains_key(&self, frn: &u64) -> bool {
        self.slot.get(&self.items, *frn).is_some()
    }

    pub fn get(&self, frn: &u64) -> Option<Rec> {
        self.slot.get(&self.items, *frn).map(|i| self.rec_at(i as usize))
    }

    /// The entry with this file reference number, which must exist.
    pub fn at(&self, frn: &u64) -> Rec {
        self.get(frn).expect("no entry with this file reference")
    }

    /// Set the modified time and the size of an entry.
    pub(super) fn set_meta(&mut self, frn: &u64, time: i64, size: u64) {
        if let Some(i) = self.slot.get(&self.items, *frn) {
            let e = &mut self.items[i as usize];
            (e.time, e.size) = (unix32(time), size);
        }
    }

    /// Add `delta` to the size of an entry; its parent, or None if there is no such entry.
    pub(super) fn add_size(&mut self, frn: u64, delta: i64) -> Option<u64> {
        let i = self.slot.get(&self.items, frn)?;
        let e = &mut self.items[i as usize];
        e.size = { e.size }.saturating_add_signed(delta);
        Some(e.parent)
    }

    pub fn insert(&mut self, frn: u64, e: Entry) {
        let rec = Rec { parent: e.parent, time: e.time, size: e.size, is_dir: e.is_dir, hidden: e.hidden };
        self.insert_rec(frn, &e.name, rec)
    }

    /// `insert` for a name that is not in a `String` (the loader reads it straight from the file).
    pub(super) fn insert_rec(&mut self, frn: u64, name: &str, rec: Rec) {
        let (at, bits) = self.store_name(name);
        let bits = bits | if rec.is_dir { DIR } else { 0 } | if rec.hidden { HIDDEN } else { 0 };
        let item = Item { frn, parent: rec.parent, size: rec.size, time: unix32(rec.time) };
        if let Some(i) = self.slot.get(&self.items, frn) {
            let i = i as usize;
            self.waste += 2 + self.name_at(i).len();
            (self.pos[i], self.hot[i], self.items[i]) = (at, bits, item);
            // renames leave their old name behind: rebuild the block when that is a quarter of it
            if self.waste > 1 << 20 && self.waste * 4 > self.names.len() {
                self.reindex();
            }
            return;
        }
        if self.tight && self.items.len() == self.items.capacity() {
            self.grow();
        }
        self.slot.add(&self.items, &self.hot, frn, self.items.len() as u32);
        self.items.push(item);
        self.hot.push(bits);
        self.pos.push(at);
    }

    pub fn remove(&mut self, frn: &u64) -> Option<Rec> {
        let i = self.slot.remove(&self.items, *frn)? as usize;
        self.waste += 2 + self.name_at(i).len();
        let old = self.rec_at(i);
        self.hot[i] = DEAD;
        let holes = self.items.len() - self.slot.live;
        if holes >= COMPACT_MIN && holes * 8 > self.items.len() {
            self.reindex();
        }
        Some(old)
    }

    /// Squeeze the holes out of `items`, `hot`, `pos` and `names`, and point `slot` at the new positions.
    fn reindex(&mut self) {
        let n = self.hot.iter().filter(|&h| h & DEAD == 0).count();
        let mut names = Vec::with_capacity((self.names.len() - self.waste.min(self.names.len())).max(16));
        let (mut items, mut hot, mut pos) = (Vec::with_capacity(n), Vec::with_capacity(n), Vec::with_capacity(n));
        for (i, item) in std::mem::take(&mut self.items).into_iter().enumerate() {
            if self.hot[i] & DEAD != 0 {
                continue;
            }
            let p = self.pos[i] as usize;
            let len = 2 + u16::from_le_bytes([self.names[p], self.names[p + 1]]) as usize;
            pos.push(names.len() as u32);
            names.extend_from_slice(&self.names[p..p + len]);
            hot.push(self.hot[i]);
            items.push(item);
        }
        (self.items, self.hot, self.pos, self.names, self.waste, self.tight) = (items, hot, pos, names, 0, true);
        self.slot.rebuild(&self.items, &self.hot, n);
    }

    /// Keep the entries `keep` says yes to, given their position, file reference and hidden flag, which it may change.
    /// Only rebuilds if some are dropped: that takes as much memory again as the whole index.
    pub(super) fn retain(&mut self, mut keep: impl FnMut(usize, u64, &mut bool) -> bool) {
        let mut dropped = false;
        for i in 0..self.items.len() {
            if self.hot[i] & DEAD != 0 {
                continue;
            }
            let mut hidden = self.hot[i] & HIDDEN != 0;
            if keep(i, self.items[i].frn, &mut hidden) {
                self.hot[i] = self.hot[i] & !HIDDEN | if hidden { HIDDEN } else { 0 };
            } else {
                self.hot[i] = DEAD; // `reindex` below rebuilds the names and the table without it
                dropped = true;
            }
        }
        if dropped {
            self.reindex();
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = u64> + '_ {
        self.iter().map(|(f, _)| f)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, Rec)> + '_ {
        (0..self.items.len()).filter(|&i| self.hot[i] & DEAD == 0).map(|i| (self.items[i].frn, self.rec_at(i)))
    }

    /// Every entry with its name.
    pub fn iter_named(&self) -> impl Iterator<Item = (u64, Rec, &str)> {
        (0..self.items.len()).filter(|&i| self.hot[i] & DEAD == 0).map(|i| (self.items[i].frn, self.rec_at(i), self.name_at(i)))
    }

    #[cfg(test)]
    pub fn values(&self) -> impl Iterator<Item = Rec> + '_ {
        self.iter().map(|(_, e)| e)
    }

    // ---- positions: what the search and the upkeep loops walk. An entry's position in the flat vector is only good
    // ---- until the next insert or remove; a hole (a removed entry that is not yet squeezed out) has no entry.

    /// How many positions there are, holes included.
    pub(super) fn slots(&self) -> usize {
        self.items.len()
    }

    /// The position of the entry with this file reference number.
    pub(super) fn position(&self, frn: u64) -> Option<usize> {
        self.slot.get(&self.items, frn).map(|p| p as usize)
    }

    pub(super) fn is_live(&self, i: usize) -> bool {
        self.hot[i] & DEAD == 0
    }

    pub(super) fn is_dir_at(&self, i: usize) -> bool {
        self.hot[i] & DIR != 0
    }

    /// Whether the name in position `i` is plain ASCII (it can then be compared without case by bytes).
    pub(super) fn is_ascii_at(&self, i: usize) -> bool {
        self.hot[i] & ASCII != 0
    }

    pub(super) fn frn_at(&self, i: usize) -> u64 {
        self.items[i].frn
    }

    pub(super) fn parent_at(&self, i: usize) -> u64 {
        self.items[i].parent
    }

    /// True for a hole and for a name that lacks one of the letters in `need` (see `letter_bits`): ruled out by reading
    /// 4 bytes, before the name itself is touched.
    pub(super) fn lacks(&self, i: usize, need: u32) -> bool {
        self.hot[i] & (need | DEAD) != need
    }

    /// Set the size of every live folder from `totals` (by file reference number; a folder not in it holds nothing).
    pub(super) fn set_folder_sizes(&mut self, totals: &std::collections::HashMap<u64, u64>) {
        for (i, e) in self.items.iter_mut().enumerate() {
            if self.hot[i] & (DEAD | DIR) == DIR {
                e.size = totals.get(&{ e.frn }).copied().unwrap_or(0);
            }
        }
    }

    /// Items, name bytes and the used part of the name bytes, as reserved: for the tests of how the vectors grow.
    #[cfg(test)]
    pub(super) fn room(&self) -> (usize, usize, usize) {
        (self.items.capacity(), self.names.capacity(), self.names.len())
    }

    pub fn extend(&mut self, entries: impl IntoIterator<Item = (u64, Entry)>) {
        for (f, e) in entries {
            self.insert(f, e);
        }
    }
}

impl FromIterator<(u64, Entry)> for Index {
    fn from_iter<I: IntoIterator<Item = (u64, Entry)>>(it: I) -> Self {
        let mut idx = Index::new();
        idx.extend(it);
        idx
    }
}

/// Unix seconds as `Item` keeps them.
pub(super) fn unix32(t: i64) -> u32 {
    t.clamp(0, u32::MAX as i64) as u32
}

pub fn full_path(idx: &Index, mut frn: u64, root: &str) -> String {
    let mut parts = vec![];
    while let Some((e, name)) = idx.get_named(&frn) {
        parts.push(name);
        frn = e.parent;
    }
    parts.reverse();
    format!("{root}\\{}", parts.join("\\"))
}

/// Does `part` of a name equal the lowercase `lower` once its case is ignored? (`ascii`: the name is plain ASCII.)
pub(super) fn eq_fold(part: &str, lower: &str, ascii: bool) -> bool {
    if ascii { part.eq_ignore_ascii_case(lower) } else { part.to_lowercase() == lower }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_survives_removals_and_compaction() {
        let mut i = Index::new();
        let n = COMPACT_MIN as u64 * 4;
        for k in 0..n {
            i.insert(k, Entry::new(0, format!("f{k}"), false, 0));
        }
        i.insert(5, Entry::new(0, "renamed".into(), false, 0)); // replaced in place
        assert_eq!((i.name(&5), i.len()), (Some("renamed"), n as usize));
        for k in 0..n / 2 {
            assert!(i.remove(&k).is_some()); // enough holes to trigger a compaction along the way
        }
        assert!(i.remove(&0).is_none());
        assert_eq!(i.len(), n as usize / 2);
        assert_eq!(i.iter().count(), n as usize / 2);
        assert!(i.items.len() < n as usize, "holes were squeezed out");
        assert_eq!(i.name(&(n - 1)).unwrap(), format!("f{}", n - 1));
        assert!(i.get(&(n / 2 - 1)).is_none() && i.get(&(n / 2)).is_some());
        i.add_size(n / 2, 9);
        assert_eq!(i.at(&(n / 2)).size, 9);
        i.insert(3, Entry::new(0, "again".into(), false, 0)); // a removed key can come back
        assert!(i.contains_key(&3) && i.keys().count() == n as usize / 2 + 1);
    }

    /// The lookup table and the block of names give the same answers as a plain map through many inserts, renames,
    /// removals and a `retain` (which rebuilds both).
    #[test]
    fn lookup_agrees_with_a_map_through_changes() {
        let mut idx = Index::new();
        let mut model = std::collections::HashMap::new();
        let mut x = 88172645463325252u64;
        for step in 0..80_000u64 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let frn = (x % 6000) | ((x / 6000 % 3) << 48); // numbers that collide and come back
            match x % 5 {
                0 | 1 => {
                    let name = format!("N{step}.Txt");
                    idx.insert(frn, Entry::new(0, name.clone(), false, 0));
                    model.insert(frn, name);
                }
                2 => assert_eq!(idx.remove(&frn).is_some(), model.remove(&frn).is_some()),
                _ => assert_eq!(idx.name(&frn), model.get(&frn).map(|n| n.as_str()), "step {step}"),
            }
            if step == 40_000 {
                idx.retain(|_, f, _| f % 7 != 0);
                model.retain(|f, _| f % 7 != 0);
            }
        }
        assert_eq!(idx.len(), model.len());
        for (f, n) in &model {
            assert_eq!(idx.name(f), Some(n.as_str()));
        }
        assert_eq!(idx.keys().count(), model.len());
        assert_eq!(idx.iter_named().count(), model.len());
        idx.shrink_to_fit();
        assert!(model.iter().all(|(f, n)| idx.name(f) == Some(n.as_str())), "still the same after shrinking");
    }

    #[test]
    fn an_item_takes_28_bytes() {
        assert_eq!(std::mem::size_of::<Item>(), 28);
    }

}
