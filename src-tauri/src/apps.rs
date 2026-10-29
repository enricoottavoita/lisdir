//! Start menu and Store apps, so the bar doubles as a launcher: apps are matched with the same fuzzy
//! scorer as files and ranked together with them.
use crate::{engine, query::Query};
use std::{collections::HashSet, os::windows::process::CommandExt};

pub struct App {
    pub name: String,
    /// AppUserModelID or shell path as listed by Windows; launched with `shell:AppsFolder\<id>`
    pub id: String,
    lower: String,
}

/// Added to an app's match score so it outranks a file whose name matches equally well.
const BONUS: f32 = 30.0;

// Get-StartApps is what the Start menu shows, Store apps included. Tab separated, UTF-8.
const SCRIPT: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-StartApps | ForEach-Object { $_.Name + \"`t\" + $_.AppID }";

pub fn parse(out: &str) -> Vec<App> {
    let mut seen = HashSet::new();
    out.lines()
        .filter_map(|l| l.trim_end_matches('\r').split_once('\t'))
        .filter(|(name, id)| !name.is_empty() && !id.is_empty() && seen.insert(id.to_string()))
        .map(|(name, id)| App { lower: name.to_lowercase(), name: name.into(), id: id.into() })
        .collect()
}

/// Ask Windows for the app list. Takes a second or two (it starts PowerShell), so call it off the UI thread.
pub fn load() -> Vec<App> {
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .creation_flags(engine::NO_WINDOW)
        .output()
        .map(|o| parse(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Apps whose name matches every word of the query, as (score, app), best first. With no words (`app:` alone)
/// it lists apps alphabetically.
pub fn search<'a>(apps: &'a [App], query: &Query, limit: usize) -> Vec<(f32, &'a App)> {
    let mut hits: Vec<_> = apps
        .iter()
        .filter_map(|a| query.terms.iter().try_fold(BONUS, |s, w| engine::score(w, &a.lower).map(|x| s + x)).map(|s| (s, a)))
        .collect();
    if query.terms.is_empty() {
        hits.sort_by(|a, b| a.1.lower.cmp(&b.1.lower));
    } else {
        hits.sort_by(|a, b| b.0.total_cmp(&a.0));
    }
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Clock;

    #[test]
    fn parses_and_ranks_apps() {
        let apps = parse("Calcolatrice\tMicrosoft.WindowsCalculator_8wekyb3d8bbwe!App\r\nGoogle Chrome\tChrome\r\nBlocco note\tnotepad.exe\r\nDuplicato\tChrome\r\n\r\nSenzaId\t\r\n");
        assert_eq!(apps.len(), 3); // duplicate id, empty line and empty id are dropped
        let q = |s: &str| Query::parse(s, &Clock::new(0, 0));
        assert_eq!(search(&apps, &q("chr"), 5)[0].1.name, "Google Chrome");
        assert_eq!(search(&apps, &q("calc"), 5)[0].1.id, "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        assert_eq!(search(&apps, &q("note blo"), 5)[0].1.name, "Blocco note"); // every word, any order
        assert!(search(&apps, &q("zzz"), 5).is_empty());
        assert_eq!(search(&apps, &q("app:"), 5).len(), 3); // no words: everything, A to Z
        assert_eq!(search(&apps, &q("app:"), 5)[0].1.name, "Blocco note");
    }
}
