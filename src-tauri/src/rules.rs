//! Rules for what is left out of the index ("do not index") or kept out of the results ("hide"). One rule per line:
//!
//! | Rule              | Matches                                                                         |
//! |-------------------|---------------------------------------------------------------------------------|
//! | `node_modules`    | every file or folder with that name, and everything inside a folder with it     |
//! | `*.tmp`, `~$*`    | names by pattern (`*` any text, `?` one character)                              |
//! | `C:\Users\me\tmp` | that folder (a rule with a backslash is a path) and everything inside it        |
//!
//! Matching ignores case. Path rules name folders; a file is covered when a folder above it is.
use std::sync::{Arc, RwLock};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    /// lowercase name patterns
    names: Vec<String>,
    /// lowercase folder paths, no trailing backslash
    paths: Vec<String>,
}

/// `*` and `?` wildcards, whole-string match, both sides already lowercase.
fn glob(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t, mut star, mut mark) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

impl Rules {
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Rules {
        let mut r = Rules::default();
        for l in lines.iter().map(|l| l.as_ref().trim()).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let l = l.to_lowercase();
            if l.contains('\\') || l.contains('/') {
                r.paths.push(l.replace('/', "\\").trim_end_matches('\\').to_string());
            } else {
                r.names.push(l);
            }
        }
        r
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }

    pub fn has_paths(&self) -> bool {
        !self.paths.is_empty()
    }

    /// Does a rule match this name (lowercase), or, when a path is given, this folder?
    pub fn hits(&self, name: &str, path: Option<&str>) -> bool {
        if !self.names.is_empty() {
            let text: Vec<char> = name.chars().collect();
            if self.names.iter().any(|n| glob(&n.chars().collect::<Vec<_>>(), &text)) {
                return true;
            }
        }
        path.is_some_and(|p| self.paths.iter().any(|r| p == r || p.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('\\'))))
    }
}

/// The two rule sets in force in this process. The helper and the UI each load them from the settings file.
#[derive(Default)]
pub struct Active {
    /// left out of the index altogether
    pub skip: Rules,
    /// indexed, but never shown in results
    pub hide: Rules,
}

static ACTIVE: RwLock<Option<Arc<Active>>> = RwLock::new(None);

pub fn set(skip: &[String], hide: &[String]) {
    *ACTIVE.write().unwrap() = Some(Arc::new(Active { skip: Rules::parse(skip), hide: Rules::parse(hide) }));
}

pub fn active() -> Arc<Active> {
    ACTIVE.read().unwrap().clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_patterns_and_paths() {
        let r = Rules::parse(&["node_modules", " *.TMP ", "~$*", "# comment", "", r"C:\Users\Me\Cache\", "d:/data/old"]);
        assert!(r.hits("node_modules", None) && r.hits("a.tmp", None) && r.hits("~$report.docx", None));
        assert!(!r.hits("node_modules2", None) && !r.hits("tmp", None) && !r.hits("a.tmp.txt", None));
        assert!(r.hits("cache", Some(r"c:\users\me\cache")));
        assert!(r.hits("x", Some(r"c:\users\me\cache\x"))); // below the folder
        assert!(!r.hits("x", Some(r"c:\users\me\cache2"))); // a different folder with the same prefix
        assert!(!r.hits("cache", None)); // path rules need a path
        assert!(r.hits("old", Some(r"d:\data\old")));
        assert!(r.has_paths() && !Rules::parse(&["a"]).has_paths());
    }

    #[test]
    fn glob_edges() {
        let g = |p: &str, t: &str| glob(&p.chars().collect::<Vec<_>>(), &t.chars().collect::<Vec<_>>());
        assert!(g("*", "") && g("a*b*c", "aXXbYYc") && g("?", "a") && g("*.rs", "x.rs") && g("a*", "a"));
        assert!(!g("?", "") && !g("a*b", "a") && !g("*.rs", "x.rss") && !g("abc", "abcd"));
    }

    #[test]
    fn empty_rules_match_nothing() {
        let r = Rules::default();
        assert!(r.is_empty() && !r.hits("anything", Some(r"c:\x")));
    }
}
