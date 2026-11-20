//! Fuzzy name search over an index.
use super::index::{eq_fold, letter_bits, Index};
use crate::query::{Only, Query};
// ---- search

/// Fuzzy match of one lowercased word against a lowercased name: its letters must appear in order. None if
/// they do not. Higher is better: word starts and consecutive letters score extra, long names and late
/// matches lose a little. Single pass, no allocation: this runs once per indexed name on every keystroke.
pub fn score(word: &str, name: &str) -> Option<f32> {
    score_core::<false>(word, name)
}

/// One letter folded to lowercase (the letter by letter version of `str::to_lowercase`; they differ only for a few
/// rare letters such as the capital dotted i, which this keeps as one letter).
fn fold(c: char) -> char {
    if c.is_ascii() { c.to_ascii_lowercase() } else { c.to_lowercase().next().unwrap_or(c) }
}

/// `score` for a name as it was given (`FOLD`): each letter is folded to lowercase as it is read, so no lowercase copy
/// of the name has to be kept or made.
fn score_core<const FOLD: bool>(word: &str, name: &str) -> Option<f32> {
    let mut want = word.chars();
    let mut cur = want.next();
    if cur.is_none() {
        return Some(0.0);
    }
    let (mut s, mut first, mut last, mut prev) = (0f32, 0usize, -1isize, ' ');
    for (p, c) in name.chars().enumerate() {
        let c = if FOLD { fold(c) } else { c };
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



fn ext_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

/// Does `name` contain the lowercase `needle` once its case is ignored?
fn contains_fold(name: &str, needle: &str, ascii: bool) -> bool {
    if ascii && needle.is_ascii() {
        let n = needle.as_bytes();
        n.is_empty() || name.as_bytes().windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
    } else {
        name.to_lowercase().contains(needle)
    }
}

/// Does some ancestor folder's name contain `word`? (the `docs\` part of a query). A plain substring, not the
/// fuzzy match used for file names: a folder filter must be precise, or `docs\` would also pick up "Discografia".
fn in_folder(idx: &Index, mut parent: u64, word: &str) -> bool {
    while let Some(i) = idx.position(parent) {
        if contains_fold(idx.name_at(i), word, idx.is_ascii_at(i)) {
            return true;
        }
        parent = idx.parent_at(i);
    }
    false
}

/// Score of an entry for a query, or None if it does not qualify. Cheap checks first.
pub(super) fn matches(q: &Query, idx: &Index, i: usize) -> Option<f32> {
    let e = idx.rec_at(i);
    if e.hidden {
        return None;
    }
    match q.only {
        Some(Only::Files) if e.is_dir => return None,
        Some(Only::Folders) if !e.is_dir => return None,
        _ => {}
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
    // the checks above need only the entry; from here on the name is read (a pure date or size filter never does)
    let (name, ascii) = (idx.name_at(i), idx.is_ascii_at(i));
    if !q.ext.is_empty() || !q.not_ext.is_empty() {
        let x = ext_of(name);
        if (!q.ext.is_empty() && !q.ext.iter().any(|v| eq_fold(x, v, ascii))) || q.not_ext.iter().any(|v| eq_fold(x, v, ascii)) {
            return None;
        }
    }
    if q.not.iter().any(|w| contains_fold(name, w, ascii)) || !q.phrases.iter().all(|p| contains_fold(name, p, ascii)) {
        return None;
    }
    let mut s = 0.0;
    for t in &q.terms {
        s += score_core::<true>(t, name)?;
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
    // a search by name lists the folders before the files (apps join the folders, see `FIRST`); a pure filter has
    // no name to rank by and keeps the newest or the biggest first, files a hair ahead of folders
    let tier = match (q.terms.is_empty(), e.is_dir) {
        (false, true) => FIRST,
        (false, false) | (true, true) => 0.0,
        (true, false) => 1.0,
    };
    Some(base + tier + name_bonus(name, &q.terms, e.is_dir))
}

/// Added to the score of a folder (and of an app) found by name, so that they come before the files. Far more than any
/// name match is worth (a whole word typed in order scores about 200), and small enough for `f32` to keep the order
/// among the folders themselves.
pub const FIRST: f32 = 1000.0;

/// Added on top when the name is exactly what was typed (`EXACT`), or begins with the typed words (`BEGINS`): more than
/// `FIRST` and any name match together, so a folder or a file named like the query is never lost behind loose matches.
pub const EXACT: f32 = 10_000.0;
const BEGINS: f32 = 5_000.0;

/// Does the name start with the words typed, in order? `.`, `_`, `-` and space all separate words, a typed word may be
/// the start of a longer one (`termina` begins `terminal`), and with `whole` nothing may follow the last, except the
/// extension of a file.
fn begins_with(name: &str, terms: &[String], whole: bool, is_dir: bool) -> bool {
    let sep = |c: char| matches!(c, '.' | '_' | '-' | ' ');
    let mut n = name.chars().peekable();
    for (k, t) in terms.iter().enumerate() {
        while n.next_if(|&c| sep(c)).is_some() {}
        for tc in t.chars() {
            if !n.next().is_some_and(|c| c.to_lowercase().eq(tc.to_lowercase())) {
                return false;
            }
        }
        if k + 1 < terms.len() {
            while n.next_if(|&c| !sep(c)).is_some() {} // each word may be unfinished: `termina` begins `terminal`
        }
    }
    if !whole {
        return true;
    }
    let rest: String = n.collect();
    let rest = rest.trim_start_matches(sep);
    rest.is_empty() || (!is_dir && name.ends_with(&format!(".{rest}")) && !rest.contains('.'))
}

/// What a name earns for resembling the query: the most for being it, then for beginning with it. The same for a
/// folder, a file and an app; `FIRST` then puts the folders and apps ahead of the files within each step.
pub fn name_bonus(name: &str, terms: &[String], is_dir: bool) -> f32 {
    if terms.is_empty() {
        0.0
    } else if begins_with(name, terms, true, is_dir) {
        EXACT
    } else if begins_with(name, terms, false, is_dir) {
        BEGINS
    } else {
        0.0
    }
}

/// Below this many entries a search runs on one thread (starting threads would cost more than it saves).
pub(super) const PARALLEL_FROM: usize = 50_000;

/// Best `limit` matches as (score, id), highest score first.
pub fn search(idx: &Index, q: &Query, limit: usize) -> Vec<(f32, u64)> {
    if !q.searches_files() || q.is_empty() {
        return vec![];
    }
    let best_first = |a: &(f32, u64), b: &(f32, u64)| b.0.total_cmp(&a.0);
    // a one-letter query matches most of the disk: keep only the best few without sorting everything
    let top = |mut hits: Vec<(f32, u64)>| {
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit, best_first);
            hits.truncate(limit);
        }
        hits
    };
    let need = q.terms.iter().fold(0, |m, t| m | letter_bits(t));
    // a short query matches most of the disk: prune now and then and ignore what cannot beat the kept ones
    let scan = |range: std::ops::Range<usize>| {
        let (mut hits, mut floor) = (Vec::with_capacity(limit * 8), f32::NEG_INFINITY);
        for i in range {
            // a hole, or a name without one of the letters: ruled out by reading 4 bytes
            if idx.lacks(i, need) {
                continue;
            }
            let Some(s) = matches(q, idx, i) else { continue };
            if s < floor {
                continue;
            }
            hits.push((s, idx.frn_at(i)));
            if hits.len() >= limit * 8 {
                hits = top(hits);
                floor = hits.iter().map(|h| h.0).fold(f32::INFINITY, f32::min);
            }
        }
        top(hits)
    };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(8);
    let n = idx.slots();
    let mut hits = if idx.len() < PARALLEL_FROM || threads == 1 {
        scan(0..n)
    } else {
        let (scan, size) = (&scan, n.div_ceil(threads));
        let parts: Vec<Vec<(f32, u64)>> = std::thread::scope(|s| {
            let jobs: Vec<_> = (0..n).step_by(size).map(|a| s.spawn(move || scan(a..(a + size).min(n)))).collect();
            jobs.into_iter().map(|j| j.join().unwrap_or_default()).collect()
        });
        top(parts.into_iter().flatten().collect())
    };
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
