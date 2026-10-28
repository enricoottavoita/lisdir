//! What you open, and how often. It does two small things: gives a modest boost to results you open a lot
//! (how well the name matches still comes first), and lists your recent items before you type anything.
//! Stored locally in a plain tab-separated file; nothing leaves the machine.
use std::{collections::HashMap, io, path::PathBuf};

const MAX_ITEMS: usize = 500;

#[derive(Clone)]
pub struct Item {
    pub count: u32,
    pub last: i64,
    pub name: String,
    pub is_dir: bool,
    pub app: bool,
}

#[derive(Default)]
pub struct Usage {
    items: HashMap<String, Item>,
    file: Option<PathBuf>,
}

impl Usage {
    /// Read the history from `file` (missing or damaged lines are skipped) and remember where to save it.
    pub fn load(file: PathBuf) -> Usage {
        let mut items = HashMap::new();
        for line in std::fs::read_to_string(&file).unwrap_or_default().lines() {
            // count \t last \t is_dir \t app \t name \t path   (the path goes last: it is the only field that may be long)
            let f: Vec<&str> = line.splitn(6, '\t').collect();
            if let [count, last, is_dir, app, name, path] = f[..] {
                if let (Ok(count), Ok(last)) = (count.parse(), last.parse()) {
                    items.insert(path.to_string(), Item { count, last, name: name.into(), is_dir: is_dir == "1", app: app == "1" });
                }
            }
        }
        Usage { items, file: Some(file) }
    }

    /// Extra score for a result you open often: 4·ln(1+opens), at most 12 (about one word-start bonus), so a much
    /// better name match always wins.
    pub fn boost(&self, path: &str) -> f32 {
        self.items.get(path).map_or(0.0, |i| (4.0 * ((1 + i.count) as f32).ln()).min(12.0))
    }

    pub fn record(&mut self, path: &str, name: &str, is_dir: bool, app: bool, now: i64) {
        let item = self.items.entry(path.to_string()).or_insert(Item { count: 0, last: now, name: name.into(), is_dir, app });
        item.count += 1;
        item.last = now;
        if self.items.len() > MAX_ITEMS {
            // drop the least recently used
            if let Some(oldest) = self.items.iter().min_by_key(|(_, i)| i.last).map(|(p, _)| p.clone()) {
                self.items.remove(&oldest);
            }
        }
        let _ = self.save();
    }

    pub fn forget(&mut self, path: &str) {
        if self.items.remove(path).is_some() {
            let _ = self.save();
        }
    }

    /// The most recently opened items, newest first.
    pub fn recent(&self, n: usize) -> Vec<(&str, &Item)> {
        let mut all: Vec<_> = self.items.iter().map(|(p, i)| (p.as_str(), i)).collect();
        all.sort_by(|a, b| b.1.last.cmp(&a.1.last).then(a.0.cmp(b.0)));
        all.truncate(n);
        all
    }

    fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else { return Ok(()) };
        std::fs::create_dir_all(file.parent().unwrap_or(file))?;
        let text: String = self.items.iter()
            .map(|(p, i)| format!("{}\t{}\t{}\t{}\t{}\t{}\n", i.count, i.last, i.is_dir as u8, i.app as u8, i.name.replace('\t', " "), p))
            .collect();
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_is_modest_and_capped() {
        let mut u = Usage::default();
        assert_eq!(u.boost(r"C:\a.txt"), 0.0);
        u.record(r"C:\a.txt", "a.txt", false, false, 1);
        let once = u.boost(r"C:\a.txt");
        assert!(once > 2.0 && once < 3.5, "{once}");
        for t in 2..=100 {
            u.record(r"C:\a.txt", "a.txt", false, false, t);
        }
        assert_eq!(u.boost(r"C:\a.txt"), 12.0);
    }

    #[test]
    fn recent_forget_and_trim() {
        let mut u = Usage::default();
        u.record("x", "x", false, false, 10);
        u.record("y", "y", true, false, 20);
        u.record("x", "x", false, false, 30); // x is now the newest
        assert_eq!(u.recent(5).iter().map(|r| r.0).collect::<Vec<_>>(), ["x", "y"]);
        u.forget("x");
        assert_eq!(u.recent(5).len(), 1);
        for i in 0..(MAX_ITEMS as i64 + 20) {
            u.record(&format!("p{i}"), "n", false, false, 100 + i);
        }
        assert_eq!(u.items.len(), MAX_ITEMS);
        assert!(!u.items.contains_key("p0") && u.items.contains_key(&format!("p{}", MAX_ITEMS + 19)));
    }

    #[test]
    fn saves_and_loads() {
        let file = std::env::temp_dir().join("lisdir_usage_test").join("usage.txt");
        let _ = std::fs::remove_file(&file);
        let mut u = Usage::load(file.clone());
        u.record(r"C:\Docs\a b.txt", "a b.txt", false, false, 5);
        u.record(r"shell:AppsFolder\Calc", "Calculator", false, true, 6);
        u.record(r"C:\Docs", "Docs", true, false, 7);
        let back = Usage::load(file.clone());
        let r = back.recent(10);
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].0, r[0].1.is_dir), (r"C:\Docs", true));
        assert!(r[1].1.app && r[1].1.name == "Calculator");
        std::fs::remove_dir_all(file.parent().unwrap()).unwrap();
    }
}
