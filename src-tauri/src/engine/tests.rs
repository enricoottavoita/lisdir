use super::{apply::*, cache::*, index::*, search::*, usn::*, walk::*, *};
use crate::{query::{Clock, Query}, rules};
use std::{collections::HashSet, sync::{mpsc::Receiver, RwLock}, time::Duration};

// 2023-11-14 22:13:20 UTC; local time zone = UTC
const NOW: i64 = 1_700_000_000;
const DAYS: i64 = 86_400;

#[test]
fn a_slow_share_is_walked_at_most_a_tenth_of_the_time() {
    let min = |m: u64| Duration::from_secs(m * 60);
    assert_eq!(next_walk(WALK_INTERVAL, min(0)), WALK_INTERVAL, "a fast share keeps its 10 minutes");
    assert_eq!(next_walk(WALK_INTERVAL, min(20)), min(180), "a 20-minute walk waits 3 hours");
    assert_eq!(next_walk(WATCHED_WALK_INTERVAL, min(5)), WATCHED_WALK_INTERVAL);
}

fn q(s: &str) -> Query {
    Query::parse(s, &Clock::new(NOW, 0))
}

fn rec(frn: u64, parent: u64, attrs: u32, reason: u32, name: &str) -> Vec<u8> {
    let u: Vec<u8> = name.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let len = (60 + u.len() + 7) & !7;
    let mut b = vec![0u8; len];
    b[0..4].copy_from_slice(&(len as u32).to_le_bytes());
    b[8..16].copy_from_slice(&frn.to_le_bytes());
    b[16..24].copy_from_slice(&parent.to_le_bytes());
    b[32..40].copy_from_slice(&((NOW + 11_644_473_600) * 10_000_000).to_le_bytes()); // timestamp: NOW
    b[40..44].copy_from_slice(&reason.to_le_bytes());
    b[52..56].copy_from_slice(&attrs.to_le_bytes());
    b[56..58].copy_from_slice(&(u.len() as u16).to_le_bytes());
    b[58..60].copy_from_slice(&60u16.to_le_bytes());
    b[60..60 + u.len()].copy_from_slice(&u);
    b
}

fn vol(buf: &[u8]) -> Volume {
    let idx = parse_records(buf).into_iter().map(|r| (r.frn, Entry::new(r.parent, r.name, r.is_dir, r.time))).collect();
    Volume { root: "C:".into(), walk: false, idx, journal_id: 7, next_usn: 100, dirty: false }
}

/// C:\Docs\{report 2024.pdf, draft report.docx, notes.txt}, C:\Pics\{photo.jpg, report.jpg}, with ages in days.
fn tree() -> Index {
    let mut i = Index::new();
    let mut add = |frn, parent, name: &str, is_dir, age: i64| i.insert(frn, Entry::new(parent, name.into(), is_dir, NOW - age * DAYS));
    add(1, 0, "Docs", true, 0);
    add(2, 0, "Pics", true, 0);
    add(10, 1, "report 2024.pdf", false, 0); // today
    add(11, 1, "draft report.docx", false, 3);
    add(12, 1, "notes.txt", false, 40);
    add(20, 2, "photo.jpg", false, 1);
    add(21, 2, "report.jpg", false, 5);
    i
}

fn found(idx: &Index, query: &str) -> Vec<u64> {
    let mut ids: Vec<u64> = search(idx, &q(query), 50).into_iter().map(|h| h.1).collect();
    ids.sort();
    ids
}

#[test]
fn parse_search_and_path() {
    let mut buf = rec(10, 5, 0x10, 0, "Docs");
    buf.extend(rec(11, 10, 0, 0, "Reading List.txt"));
    buf.extend(rec(12, 10, 0, 0, "rm.txt"));
    let v = vol(&buf);
    assert_eq!(v.idx.len(), 3);
    assert_eq!(v.idx.at(&12).time, NOW); // the USN record's timestamp became the last-change time
    assert_eq!(full_path(&v.idx, 12, "C:"), r"C:\Docs\rm.txt");
    assert_eq!(search(&v.idx, &q("rl"), 5)[0].1, 11);
    assert_eq!(search(&v.idx, &q("rm"), 5)[0].1, 12);
    assert!(search(&v.idx, &q("zzz"), 5).is_empty());
    // the example used in the README
    let readme = vol(&rec(1, 5, 0, 0, "README.md"));
    assert_eq!(search(&readme.idx, &q("rdme"), 5)[0].1, 1);
}

#[test]
fn scoring_prefers_starts_and_runs() {
    let s = |w, n| score(w, n).unwrap();
    assert!(s("rep", "report.txt") > s("rep", "my deep report.txt")); // earlier and shorter
    assert!(s("rep", "report.txt") > s("rpt", "report.txt")); // consecutive letters beat scattered ones
    assert!(s("lst", "my list.txt") > s("lst", "mylist.txt")); // letter at a word start
    assert_eq!(score("zz", "report"), None);
    assert_eq!(score("tr", "rt"), None); // order matters
}

#[test]
fn words_match_in_any_order() {
    let t = tree();
    assert_eq!(found(&t, "2024 report"), [10]);
    assert_eq!(found(&t, "report"), [10, 11, 21]);
}

#[test]
fn filters_combine() {
    let t = tree();
    assert_eq!(found(&t, "ext:pdf"), [10]);
    assert_eq!(found(&t, "*.jpg"), [20, 21]);
    assert_eq!(found(&t, "report -draft"), [10, 21]);
    assert_eq!(found(&t, "report -ext:jpg"), [10, 11]);
    assert_eq!(found(&t, r#""report 2024""#), [10]);
    assert_eq!(found(&t, "folder:"), [1, 2]);
    assert_eq!(found(&t, "file: pic:"), [20, 21]);
    assert_eq!(found(&t, r"docs\ report"), [10, 11]); // only reports inside a "docs" folder
    assert_eq!(found(&t, r"pics\ report"), [21]);
    assert_eq!(found(&t, r"nowhere\ report"), Vec::<u64>::new());
    assert_eq!(found(&t, r"dcs\ report"), Vec::<u64>::new()); // folder names are matched as text, not fuzzily
    assert_eq!(found(&t, "ext:pdf;docx -draft"), [10]);
}

#[test]
fn dates_and_filter_only_order() {
    let t = tree();
    assert_eq!(found(&t, "date:today file:"), [10]);
    assert_eq!(found(&t, "date:week file:"), [10, 11, 20, 21]);
    assert_eq!(found(&t, "date:2023-11 file:"), [10, 11, 20, 21]);
    assert_eq!(found(&t, "date:2023 file:"), [10, 11, 12, 20, 21]);
    // no words to rank by: newest first
    let order: Vec<u64> = search(&t, &q("date:week file:"), 10).into_iter().map(|h| h.1).collect();
    assert_eq!(order, [10, 20, 11, 21]);
}

#[test]
fn apps_flag_returns_no_files() {
    assert!(search(&tree(), &q("app: report"), 10).is_empty());
    assert!(search(&tree(), &q(""), 10).is_empty());
}

#[test]
fn highlights_matched_letters() {
    let hl = |query: &str, name: &str| highlight(&q(query), name);
    assert_eq!(hl("rdme", "README.md"), [0, 3, 4, 5]); // r, d, m, e of "readme.md"
    assert_eq!(hl("rep 24", "report 2024.pdf"), [0, 1, 2, 7, 10]); // "rep" + 2 and 4 of "2024"
    assert_eq!(hl(r#""2024""#, "report 2024.pdf"), [7, 8, 9, 10]);
    assert_eq!(hl("x", "report"), Vec::<usize>::new());
}

#[test]
fn live_changes_and_cache_roundtrip() {
    let mut v = vol(&[rec(10, 5, 0x10, 0, "Docs"), rec(11, 10, 0, 0, "old.txt")].concat());
    apply(&mut v, vec![
        Change::Upsert(11, 10, "new name.txt".into(), false, NOW + 5), // rename
        Change::Upsert(12, 10, "fresh.txt".into(), false, NOW + 9),    // create
        Change::Delete(10),                                             // delete
        Change::Meta(12, NOW + 20, 4096),                               // size and time from the file record
        Change::Meta(99, 1, 1),                                         // unknown entry: ignored
    ], 200);
    assert_eq!((v.idx.len(), v.next_usn, v.dirty), (2, 200, true));
    assert_eq!(search(&v.idx, &q("nn"), 5)[0].1, 11);
    assert!(!v.idx.contains_key(&10));

    v.root = "Z:".into(); // never a real drive: keeps the test cache apart
    save(&mut v).unwrap();
    let back = load("Z:", false).unwrap();
    std::fs::remove_file(cache_path("Z:")).unwrap();
    assert_eq!((back.idx.len(), back.next_usn, back.journal_id), (2, 200, 7));
    assert_eq!((back.idx.name(&12).unwrap(), back.idx.at(&12).time, back.idx.at(&12).size), ("fresh.txt", NOW + 20, 4096));
}

#[test]
fn parallel_search_equals_one_thread() {
    let mut i = Index::new();
    for k in 0..(PARALLEL_FROM as u64 * 3) {
        i.insert(k, Entry::new(0, format!("file{} report{}.txt", k % 997, k), false, NOW - (k % 400) as i64 * DAYS).with_size(k * 7));
    }
    for qs in ["rep 99", "report1", "ext:txt size:>100000", "date:week"] {
        let query = q(qs);
        let got: Vec<f32> = search(&i, &query, 20).iter().map(|h| h.0).collect();
        let mut want: Vec<f32> = (0..i.slots()).filter_map(|p| matches(&query, &i, p)).collect();
        want.sort_by(|a, b| b.total_cmp(a));
        want.truncate(20);
        assert_eq!(got, want, "{qs}"); // same best scores whether or not the work was split across threads
    }
}

#[test]
fn change_log_extends_the_snapshot() {
    let mut v = vol(&[rec(10, 5, 0x10, 0, "Docs"), rec(11, 10, 0, 0, "old.txt")].concat());
    v.root = "Y:".into(); // never a real drive
    v.next_usn = 100;
    save(&mut v).unwrap();
    let batch1 = vec![Change::Upsert(12, 10, "fresh.txt".into(), false, NOW + 9), Change::Meta(12, NOW + 20, 4096), Change::Delete(11)];
    assert!(append_log("Y:", &batch1, 150));
    let batch2 = vec![Change::Upsert(13, 10, "later.txt".into(), false, NOW + 30)];
    assert!(append_log("Y:", &batch2, 180));
    assert!(append_log("Y:", &[], 190)); // nothing to write

    let back = load("Y:", false).unwrap();
    assert_eq!((back.next_usn, back.dirty), (180, false)); // no need to write again what the log holds
    assert!(!back.idx.contains_key(&11) && back.idx.at(&12).size == 4096 && back.idx.contains_key(&13));

    // a half-written last batch is dropped, the committed ones stay
    let log = log_path("Y:");
    let mut raw = std::fs::read(&log).unwrap();
    raw.extend([1, 99, 0, 0, 0, 0, 0, 0, 0, 1, 2]); // an upsert cut short
    std::fs::write(&log, &raw).unwrap();
    let torn = load("Y:", false).unwrap();
    assert_eq!((torn.next_usn, torn.idx.len()), (180, back.idx.len()));

    // a new snapshot starts a new log; a log from another snapshot is ignored
    let mut b = load("Y:", false).unwrap();
    save(&mut b).unwrap();
    assert_eq!(std::fs::metadata(&log).unwrap().len(), 12);
    std::fs::write(&log, [log_header(7), raw[12..].to_vec()].concat()).unwrap();
    assert_eq!(load("Y:", false).unwrap().next_usn, 180); // unchanged: the foreign log was not applied
    assert!(!log_is_big("Y:"));
    let _ = std::fs::remove_file(cache_path("Y:"));
    let _ = std::fs::remove_file(log);
}

fn rules(skip: &[&str], hide: &[&str]) -> rules::Active {
    rules::Active { skip: rules::Rules::parse(skip), hide: rules::Rules::parse(hide) }
}

/// C:\ with Users\me\{src\{main.rs, node_modules\{dep.js}}, Cache\x.tmp, notes.txt}; frn = parent*10 + n
fn rule_tree() -> Index {
    let mut i = Index::new();
    let mut add = |frn, parent, name: &str, dir| i.insert(frn, Entry::new(parent, name.into(), dir, NOW).with_size(100));
    add(10, 5, "Users", true);
    add(11, 10, "me", true);
    add(12, 11, "src", true);
    add(13, 12, "main.rs", false);
    add(14, 12, "node_modules", true);
    add(15, 14, "dep.js", false);
    add(16, 11, "Cache", true);
    add(17, 16, "x.tmp", false);
    add(18, 11, "notes.txt", false);
    i
}

#[test]
fn do_not_index_rules_remove_whole_subtrees() {
    let mut i = rule_tree();
    prune_with(&mut i, "C:", &rules(&["node_modules", "*.tmp", r"C:\Users\me\Cache"], &[]));
    let left: Vec<u64> = (10..=18).filter(|f| i.contains_key(f)).collect();
    assert_eq!(left, [10, 11, 12, 13, 18]); // dep.js went with its folder, x.tmp and Cache by path
    assert_eq!(i.len(), 5);
    assert_eq!(search(&i, &q("dep"), 5).len(), 0);
}

#[test]
fn exclude_rules_hide_but_keep() {
    let mut i = rule_tree();
    prune_with(&mut i, "C:", &rules(&[], &["node_modules", r"c:\users\me\cache"]));
    assert_eq!(i.len(), 9); // everything is still indexed
    assert!(i.at(&14).hidden && i.at(&15).hidden && i.at(&16).hidden && i.at(&17).hidden && !i.at(&13).hidden);
    assert!(search(&i, &q("dep"), 5).is_empty() && search(&i, &q("x.tmp"), 5).is_empty());
    assert_eq!(search(&i, &q("main"), 5).len(), 1);
    prune_with(&mut i, "C:", &rules(&[], &[])); // no rules: nothing changes (hiding is recomputed on load)
    assert!(i.at(&14).hidden);
}

#[test]
fn live_changes_follow_the_rules() {
    let act = rules(&["node_modules"], &["secret*"]);
    let mut v = vol(&[rec(10, 5, 0x10, 0, "Docs")].concat());
    apply_with(&mut v, vec![
        Change::Upsert(20, 10, "node_modules".into(), true, NOW),   // skipped folder
        Change::Upsert(21, 20, "a.js".into(), false, NOW),          // inside it: its parent is not in the index
        Change::Upsert(22, 10, "secret plans".into(), true, NOW),   // hidden folder
        Change::Upsert(23, 22, "plan.txt".into(), false, NOW),      // inherits the hiding
        Change::Upsert(24, 10, "visible.txt".into(), false, NOW),
        Change::Upsert(25, 5, "top.txt".into(), false, NOW),        // directly in the root: kept
    ], 150, &act);
    assert!(!v.idx.contains_key(&20) && !v.idx.contains_key(&21));
    assert!(v.idx.at(&22).hidden && v.idx.at(&23).hidden && !v.idx.at(&24).hidden && v.idx.contains_key(&25));
    // renaming something into a skipped name removes it
    apply_with(&mut v, vec![Change::Upsert(24, 10, "node_modules".into(), true, NOW)], 160, &act);
    assert!(!v.idx.contains_key(&24));
}

#[test]
fn walk_skips_and_hides() {
    let root = std::env::temp_dir().join("lisdir_rules_walk");
    let _ = std::fs::remove_dir_all(&root);
    for d in ["keep", "node_modules/inner", "private/deep"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    for f in ["keep/a.txt", "node_modules/inner/b.js", "private/deep/c.txt", "d.tmp"] {
        std::fs::write(root.join(f), b"x").unwrap();
    }
    let r = root.to_str().unwrap();
    let w = walk_with(r, None, Index::new(), &rules(&["node_modules", "*.tmp"], &["private"]));
    let names: Vec<&str> = w.idx.iter_named().map(|(_, _, n)| n).collect();
    assert!(!names.contains(&"node_modules") && !names.contains(&"b.js") && !names.contains(&"d.tmp"));
    let hidden: Vec<&str> = w.idx.iter_named().filter(|(_, e, _)| e.hidden).map(|(_, _, n)| n).collect();
    assert_eq!(hidden.len(), 3); // private, deep, c.txt
    assert!(names.contains(&"a.txt") && names.contains(&"c.txt"));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn index_retain_keeps_lookups_working() {
    let mut i = rule_tree();
    i.retain(|_, f, hidden| {
        *hidden = true;
        f % 2 == 0
    });
    assert_eq!(i.len(), 5);
    assert!(i.get(&12).is_some_and(|e| e.hidden) && i.get(&13).is_none());
    assert_eq!(i.iter().count(), 5);
}

/// `changes` as `refresh` handles them: wait for the reports to arrive, then look at each path.
fn settle(vol: &RwLock<Volume>, changes: &Receiver<Option<String>>, act: &rules::Active) {
    let first = changes.recv_timeout(Duration::from_secs(10)).expect("the watch reported nothing");
    let mut paths = HashSet::from([first.expect("no overflow for a few files")]);
    while let Ok(c) = changes.recv_timeout(Duration::from_millis(500)) {
        paths.insert(c.expect("no overflow for a few files"));
    }
    paths.iter().for_each(|p| refresh_path_with(vol, p, act));
}

#[test]
fn reported_changes_reach_the_index() {
    let root = std::env::temp_dir().join("lisdir_watch");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/old.txt"), b"12345").unwrap();
    let r = root.to_str().unwrap();
    let act = rules(&["*.tmp"], &["private"]);
    let v = walk_with(r, None, Index::new(), &act);
    let vol = RwLock::new(Volume { root: r.into(), walk: true, idx: v.idx, journal_id: 0, next_usn: 0, dirty: false });
    let changes = watch(r).unwrap();
    // the watch counts from its first call, in its own thread: touch a file until it is seen
    let ready = root.join("ready");
    while { std::fs::write(&ready, b"x").unwrap(); changes.recv_timeout(Duration::from_millis(50)).is_err() } {}
    std::fs::remove_file(&ready).unwrap();
    settle(&vol, &changes, &act);
    let at = |rel| { let v = vol.read().unwrap(); find(&v.idx, rel).map(|f| v.idx.at(&f)) };
    let sub_size = || at("sub").unwrap().size;
    assert_eq!(sub_size(), 5);

    // a new file, and a file the rules leave out
    std::fs::write(root.join("sub/new.txt"), b"123").unwrap();
    std::fs::write(root.join("sub/skip.tmp"), b"123").unwrap();
    settle(&vol, &changes, &act);
    assert_eq!(at("sub\\NEW.TXT").unwrap().size, 3); // names in any case, as Windows has them
    assert!(at("sub\\skip.tmp").is_none());
    assert_eq!(sub_size(), 8);

    // a changed file; a new folder with what is already in it, in a hidden one
    std::fs::write(root.join("sub/new.txt"), b"1234567").unwrap();
    std::fs::create_dir_all(root.join("sub2/private/deep")).unwrap();
    std::fs::write(root.join("sub2/private/deep/a.txt"), b"12").unwrap();
    settle(&vol, &changes, &act);
    assert_eq!(sub_size(), 12);
    assert_eq!(at("sub2").unwrap().size, 2);
    let a = at("sub2\\private\\deep\\a.txt").unwrap();
    assert!(a.hidden && a.size == 2 && !at("sub2").unwrap().hidden);

    // a renamed folder keeps what it holds; a removed one takes it along
    std::fs::rename(root.join("sub2"), root.join("sub3")).unwrap();
    std::fs::remove_file(root.join("sub/old.txt")).unwrap();
    settle(&vol, &changes, &act);
    assert!(at("sub2").is_none() && at("sub3\\private\\deep\\a.txt").is_some());
    assert_eq!(sub_size(), 7);
    std::fs::remove_dir_all(root.join("sub3")).unwrap();
    settle(&vol, &changes, &act);
    assert!(at("sub3").is_none() && at("sub3\\private\\deep\\a.txt").is_none());
    assert_eq!(vol.read().unwrap().idx.len(), 2); // sub and new.txt
    refresh_path_with(&vol, "sub\\new.txt", &act); // seeing the same path again changes nothing
    assert_eq!((vol.read().unwrap().idx.len(), sub_size()), (2, 7));

    // a walk gives the same totals as all of the above
    let again = walk_with(r, None, vol.read().unwrap().idx.sized_like(), &act).idx;
    assert_eq!(again.at(&find(&again, "sub").unwrap()).size, 7);
    drop(changes);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn drives_lists_the_system_disk() {
    let all = drives(&[]);
    let c = all.iter().find(|d| d.letter == "C").expect("C: is listed");
    assert!(c.present && c.indexed);
    let off = drives(&["C".into()]);
    assert!(!off.iter().find(|d| d.letter == "C").unwrap().indexed);
    assert!(walk_roots(&[r"X:\Extra\".into()], &[]).iter().any(|r| r == r"X:\Extra"));
}

#[test]
fn size_filter_and_order() {
    let mut i = Index::new();
    i.insert(1, Entry::new(0, "folder".into(), true, NOW));
    i.insert(2, Entry::new(1, "small.bin".into(), false, NOW).with_size(10));
    i.insert(3, Entry::new(1, "big.bin".into(), false, NOW).with_size(5_000_000_000));
    i.insert(4, Entry::new(1, "mid.bin".into(), false, NOW).with_size(3_000_000));
    i.insert(5, Entry::new(1, "empty.txt".into(), false, NOW));
    let ids = |s: &str| search(&i, &q(s), 10).into_iter().map(|h| h.1).collect::<Vec<_>>();
    assert_eq!(ids("size:>1mb"), [3, 4]); // biggest first, folders never match a size
    assert_eq!(ids("size:1mb..10gb ext:bin"), [3, 4]);
    assert_eq!(ids("size:<100"), [2, 5]);
    assert_eq!(ids("size:empty"), [5]);
    assert_eq!(ids("big size:>1gb"), [3]); // combines with words
    assert!(ids("size:>1tb").is_empty());
}

#[test]
fn folder_sizes_follow_changes() {
    // C:\a\b\x.bin (100), C:\a\y.bin (10), C:\c\ (empty); the NTFS root 5 is its own parent
    let mut v = sample_volume();
    v.idx = Index::new();
    v.idx.insert(5, Entry::new(5, "".into(), true, NOW));
    v.idx.insert(10, Entry::new(5, "a".into(), true, NOW));
    v.idx.insert(11, Entry::new(10, "b".into(), true, NOW).with_size(4096)); // a folder's record size is not its total
    v.idx.insert(12, Entry::new(11, "x.bin".into(), false, NOW).with_size(100));
    v.idx.insert(13, Entry::new(10, "y.bin".into(), false, NOW).with_size(10));
    v.idx.insert(14, Entry::new(5, "c".into(), true, NOW));
    sum_folders(&mut v.idx);
    let sizes = |v: &Volume| [10, 11, 14].map(|f| v.idx.at(&f).size);
    assert_eq!(sizes(&v), [110, 100, 0]);

    apply(&mut v, vec![Change::Meta(12, NOW, 300)], 1); // x.bin grows
    assert_eq!(sizes(&v), [310, 300, 0]);
    apply(&mut v, vec![Change::Upsert(11, 14, "b".into(), true, NOW), Change::Meta(11, NOW, 4096)], 2); // move b into c
    assert_eq!(sizes(&v), [10, 300, 300]);
    apply(&mut v, vec![Change::Upsert(13, 10, "z.bin".into(), false, NOW), Change::Meta(13, NOW, 10)], 3); // rename y.bin
    assert_eq!(sizes(&v), [10, 300, 300]);
    apply(&mut v, vec![Change::Upsert(15, 10, "new.bin".into(), false, NOW), Change::Meta(15, NOW, 5), Change::Delete(12)], 4);
    assert_eq!(sizes(&v), [15, 0, 0]);
    let live = sizes(&v);
    sum_folders(&mut v.idx);
    assert_eq!(sizes(&v), live, "the running totals equal a recount");
    assert_eq!(v.idx.at(&5).size, 15);
}

/// The one-time recount at load must stay small next to reading the cache (about a second for 1.6 million
/// entries). Run `cargo test --release folder_sum_cost -- --nocapture` for the number.
#[test]
fn folder_sum_cost() {
    if cfg!(debug_assertions) {
        return;
    }
    // 1.6 million entries: 160k folders, 10 levels deep in a chain of 16k-folder trees, 9 files each
    let mut idx = Index::with_capacity(1_600_000);
    for d in 1..=160_000u64 {
        idx.insert(d, Entry::new(if d % 10 == 1 { 0 } else { d - 1 }, format!("folder {d}"), true, NOW));
        for k in 0..9 {
            idx.insert(1_000_000 + d * 9 + k, Entry::new(d, format!("file {k}.bin"), false, NOW).with_size(1000));
        }
    }
    let t = std::time::Instant::now();
    sum_folders(&mut idx);
    let took = t.elapsed();
    println!("sum_folders over {} entries: {:.0} ms", idx.len(), took.as_secs_f64() * 1000.0);
    assert_eq!(idx.at(&1).size, 90_000); // the top of a 10-folder chain holds all 90 files
    assert!(took < Duration::from_millis(1500), "{took:?}");
}

#[test]
fn walk_finds_nested_files() {
    let root = std::env::temp_dir().join("lisdir_walk_test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a").join("b")).unwrap();
    std::fs::write(root.join("a").join("b").join("deep file.txt"), b"x").unwrap();
    std::fs::write(root.join("top.txt"), b"x").unwrap();
    let r = root.to_str().unwrap();
    let w = walk(r, None, Index::new());
    assert!(w.reliable());
    let idx = w.idx;
    assert_eq!(idx.len(), 4); // a, b, deep file.txt, top.txt
    let hit = search(&idx, &q("deepf"), 5)[0].1;
    assert_eq!(full_path(&idx, hit, r), format!("{r}\\a\\b\\deep file.txt"));
    assert!(idx.at(&hit).time > NOW, "walked entries carry their modified time"); // written just now
    let folder = |n: &str| idx.iter_named().find(|(_, _, name)| *name == n).unwrap().1.size;
    assert_eq!((folder("a"), folder("b")), (1, 1)); // folders add up the files below them
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn streamed_walk_fills_the_volume_directly() {
    let root = std::env::temp_dir().join("lisdir_stream_test");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("a")).unwrap();
    std::fs::write(root.join("a").join("one.txt"), b"x").unwrap();
    let r = root.to_str().unwrap();
    let vol = RwLock::new(Volume { root: r.into(), walk: true, idx: Index::new(), journal_id: 0, next_usn: 0, dirty: false });
    let w = walk(r, Some(&vol), Index::new());
    assert!(w.reliable() && w.idx.is_empty());
    assert_eq!(vol.read().unwrap().idx.len(), 2); // a, one.txt
    assert_eq!(vol.read().unwrap().idx.iter().find(|(_, e)| e.is_dir).unwrap().1.size, 1);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unreachable_walk_is_not_trusted() {
    // a share that is offline: the root itself cannot be read
    let w = walk(r"\\lisdir-no-such-host-0\nothing", None, Index::new());
    assert!(w.idx.is_empty());
    assert!(!w.reliable());
    // a missing local folder behaves the same way
    assert!(!walk(r"C:\lisdir-no-such-folder-0", None, Index::new()).reliable());
    // the 2% rule: a few vanished folders are fine, a mass failure is not
    let few = Walk { idx: Index::new(), dirs: 1000, failed: 10 };
    let many = Walk { idx: Index::new(), dirs: 1000, failed: 200 };
    assert!(few.reliable() && !many.reliable());
}

/// An index of `n` plausible names spread over a folder tree.
fn synthetic(n: u64) -> Index {
    let (words, exts) = (["report", "invoice", "holiday", "notes", "backup", "draft", "photo"], ["pdf", "docx", "jpg", "txt", "mp4"]);
    let mut i = Index::with_capacity(n as usize + 100);
    for d in 1..=100 {
        i.insert(d, Entry::new(0, format!("folder {d}"), true, NOW));
    }
    for k in 0..n {
        let name = format!("{}_{}_{}.{}", words[(k % 7) as usize], k, words[(k / 7 % 7) as usize], exts[(k % 5) as usize]);
        i.insert(1000 + k, Entry::new(1 + k % 100, name, false, NOW - (k % 400) as i64 * DAYS));
    }
    i
}

/// A search by name lists the folders (best first) before the files, however good the file's name; a pure filter
/// keeps its own order.
#[test]
fn folders_come_before_files_for_a_name_search() {
    let mut i = Index::new();
    i.insert(1, Entry::new(0, "report".into(), true, NOW));
    i.insert(2, Entry::new(0, "my old reports archive".into(), true, NOW));
    i.insert(3, Entry::new(1, "report.pdf".into(), false, NOW));
    let ids = |text: &str| search(&i, &q(text), 10).iter().map(|h| h.1).collect::<Vec<_>>();
    assert_eq!(ids("report"), [1, 3, 2], "the exact folder, the exact file, then the other folder");
    assert_eq!(ids("reports"), [2], "a partial match still lists the folder before any file");
    assert_eq!(ids("date:month")[0], 3, "no name to rank by: the file keeps its small lead");
}

/// The name that is exactly what was typed comes first: folders, then files (the extension does not count), then the rest.
#[test]
fn exact_names_come_first_folders_before_files() {
    let mut i = Index::new();
    i.insert(1, Entry::new(0, "my report folder".into(), true, NOW));
    i.insert(2, Entry::new(0, "report.pdf".into(), false, NOW));
    i.insert(3, Entry::new(0, "Report".into(), true, NOW));
    i.insert(4, Entry::new(0, "report".into(), false, NOW));
    i.insert(5, Entry::new(0, "reports.txt".into(), false, NOW));
    let ids = search(&i, &q("report"), 10).iter().map(|h| h.1).collect::<Vec<_>>();
    assert_eq!(&ids[..3], [3, 4, 2].as_slice(), "exact folder, then the exact files");
    assert_eq!(&ids[3..], [5, 1].as_slice(), "then names that begin with it, then the rest");
}

/// Names that begin with the typed words (dots, dashes and underscores separate words) come before loose matches.
#[test]
fn names_that_begin_with_the_words_come_before_loose_matches() {
    let mut i = Index::new();
    i.insert(4, Entry::new(0, "The.Terminal.List.v2.txt".into(), false, NOW));
    i.insert(5, Entry::new(0, "the terminal listing archive".into(), true, NOW));
    i.insert(6, Entry::new(0, "tools - the old terminal for a list".into(), true, NOW));
    let ids = search(&i, &q("the termina list"), 10).iter().map(|h| h.1).collect::<Vec<_>>();
    assert_eq!(ids, [5, 4, 6]);
}

/// An index sized at load grows by small steps: doubling its vectors for a few new files would hold as much memory
/// again as the whole index.
#[test]
fn a_sized_index_grows_in_small_steps() {
    let mut i = Index::with_names(10_000, 10_000 * 12);
    let (items, names, _) = i.room();
    for k in 0..items as u64 + 1 {
        i.insert(k, Entry::new(0, format!("file {k}.txt"), false, NOW));
    }
    let (now_items, now_names, used) = i.room();
    assert!(now_items < items * 3 / 2, "items grew from {items} to {now_items}: doubling would give {}", items * 2);
    assert!(now_names < used * 3 / 2, "names grew from {names} to {now_names}");
    i.shrink_to_fit();
    let (items, names, _) = i.room();
    assert!(names < used + used / 50 && items == i.len(), "shrink gives the room back");
    assert_eq!(search(&i, &q("file 77.txt"), 1)[0].1, 77, "still searchable after growing and shrinking");
}

/// Guards against an accidental slowdown (say, an allocation back in the hot loop). A search over 300k names takes
/// a few milliseconds; the limit leaves room for a busy machine but still catches a real regression. Debug builds
/// are far slower; skip them.
#[test]
fn search_latency_budget() {
    if cfg!(debug_assertions) {
        return;
    }
    let idx = synthetic(300_000);
    for query in ["rep", "inv 2024", r"folder\ photo ext:jpg", "ext:pdf date:month", "a"] {
        let t = std::time::Instant::now();
        let hits = search(&idx, &q(query), 20);
        let took = t.elapsed();
        assert!(took < Duration::from_millis(150), "{query:?} took {took:?} over 300k names ({} hits)", hits.len());
    }
}

/// Startup, save and search speed on the real caches in %LOCALAPPDATA%\lisdir (C: and D:), no elevation needed:
/// cargo test --release bench_real_cache -- --ignored --nocapture
/// Caches in the old LSD2 format are read without sizes; sizes are then made up so the size filter has work to do.
#[test]
#[ignore]
fn bench_real_cache() {
    let ms = |d: std::time::Duration| d.as_secs_f64() * 1000.0;
    let mut idx = Index::new();
    let t = std::time::Instant::now();
    for letter in ['C', 'D'] {
        let Ok(b) = std::fs::read(cache_path(&format!("{letter}:"))) else { continue };
        let (lsd3, mut off) = (&b[..4] == b"LSD3", 28);
        for _ in 0..rd::<u64>(&b, 20) {
            let (sz, ext) = if lsd3 { (rd::<u64>(&b, off + 24), 8) } else { (0, 0) };
            let nlen = rd::<u16>(&b, off + 25 + ext) as usize;
            let name = String::from_utf8_lossy(&b[off + 27 + ext..off + 27 + ext + nlen]).into_owned();
            let frn = rd::<u64>(&b, off) | (letter as u64) << 56; // keep the two disks apart
            let size = if lsd3 { sz } else { (frn.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) * 64 };
            idx.insert(frn, Entry::new(rd::<u64>(&b, off + 8) | (letter as u64) << 56, name, b[off + 24 + ext] != 0, rd::<i64>(&b, off + 16)).with_size(size));
            off += 27 + ext + nlen;
        }
    }
    println!("read + build in memory: {} entries in {:.0} ms (raw files only, no cache validation)", idx.len(), ms(t.elapsed()));

    let queries = ["a", "re", "rep", "report", "rapporto 2024", "ext:pdf", "date:week", "size:>100mb", r"docs\ report", "zzzqq"];
    for qs in queries {
        let query = q(qs);
        let mut times = vec![];
        let mut n = 0;
        for _ in 0..7 {
            let t = std::time::Instant::now();
            n = search(&idx, &query, 20).len();
            times.push(ms(t.elapsed()));
        }
        times.sort_by(f64::total_cmp);
        println!("search {qs:<16} median {:>6.1} ms  (min {:>6.1}, max {:>6.1})  {n} results", times[3], times[0], times[6]);
    }

    // save and reload through the real code, in a scratch folder so the real cache is untouched
    let scratch = std::env::temp_dir().join("lisdir_bench_cache");
    let _ = std::fs::remove_dir_all(&scratch);
    std::env::set_var("LOCALAPPDATA", &scratch);
    let mut v = Volume { root: "Z:".into(), walk: false, idx, journal_id: 1, next_usn: 1, dirty: true };
    let t = std::time::Instant::now();
    save(&mut v).unwrap();
    let (saved, size) = (ms(t.elapsed()), std::fs::metadata(cache_path("Z:")).unwrap().len());
    println!("save {} entries: {:.0} ms, file {:.1} MB", v.idx.len(), saved, size as f64 / 1e6);
    let t = std::time::Instant::now();
    let back = load("Z:", false).unwrap();
    println!("load (LSD3) {} entries: {:.0} ms", back.idx.len(), ms(t.elapsed()));
    let _ = std::fs::remove_dir_all(&scratch);
}

/// What a start costs, phase by phase, on a copy of the real cache; LOCALAPPDATA must point at that copy:
/// cargo test --release bench_start -- --ignored --nocapture
#[test]
#[ignore]
fn bench_start() {
    let peak = || {
        let c = format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id());
        let out = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &c]).output().unwrap().stdout;
        String::from_utf8_lossy(&out).trim().parse::<f64>().unwrap_or(0.0) / 1e6
    };
    crate::rules::set(&[], &crate::settings::Settings::load().exclude);
    let mut t = std::time::Instant::now();
    let mut phase = |what: &str| {
        println!("{what:<14} {:>6.0} ms, peak so far {:>4.0} MB", t.elapsed().as_secs_f64() * 1000.0, peak());
        t = std::time::Instant::now();
    };
    for root in ["C:", "Y:"] {
        let mut v = load(root, root == "Y:").expect("no cache at LOCALAPPDATA");
        phase("load + log");
        prune(&mut v.idx, root);
        phase("prune");
        sum_folders(&mut v.idx);
        phase("sum_folders");
        write_cache(&v).unwrap();
        phase("save");
    }
}

/// Manual benchmark on a few million names: cargo test --release bench_search -- --ignored --nocapture
#[test]
#[ignore]
fn bench_search() {
    let idx = synthetic(2_000_000);
    for query in ["rep", "inv 2024", "rpt", r"folder\ photo ext:jpg", "ext:pdf date:month", "a", "zzzz"] {
        let t = std::time::Instant::now();
        let hits = search(&idx, &q(query), 20);
        println!("{query:<26} {:>8.1?}  ({} hits)", t.elapsed(), hits.len());
    }
}
