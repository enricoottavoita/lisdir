//! The search language. Plain words match names fuzzily (all of them, in any order); the rest narrows:
//!
//! | You type                | Meaning                                              |
//! |-------------------------|------------------------------------------------------|
//! | `ext:pdf;docx`, `*.pdf` | only these extensions                                |
//! | `-draft`, `-ext:tmp`    | name must not contain `draft` / not this extension   |
//! | `"annual report"`       | exact phrase in the name                             |
//! | `docs\ report`          | "report" inside a folder whose name contains "docs" |
//! | `folder:` / `file:`     | only folders / only files                            |
//! | `doc:` `pic:` `video:` `music:` `zip:` | a whole category of extensions        |
//! | `app:`                  | only apps                                            |
//! | `date:today` ...        | last modified: today, yesterday, week, month, year (rolling 7/30/365 days), or `2026`, `2026-07`, `2026-07-31` |
//! | `date:3d` `2w` `6m` `1y` | the last N days / weeks / months (30 days) / years (365); Italian `g` `s` `m` `a` too |
//! | `date:monday`, `date:lunedì` | the most recent such day (today if it is one) |
//! | `date:lastweek` `lastmonth` `lastyear` | the previous calendar week (Mon–Sun) / month / year |
//! | `oggi` `ieri` `settimana` `mese` `anno`, `settimana-scorsa` `mese-scorso` `anno-scorso` | Italian keywords |
//! | `size:>1gb`, `size:10mb..1gb`, `size:empty` | file size: `>` `>=` `<` `<=`, a range, or `empty`; units b kb mb gb tb (1024) |
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

const DAY: i64 = 86_400;

const DOC: &[&str] = &["doc", "docx", "pdf", "txt", "rtf", "odt", "xls", "xlsx", "ppt", "pptx", "csv", "md"];
const PIC: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "tif", "tiff", "svg", "ico"];
const VIDEO: &[&str] = &["mp4", "mkv", "avi", "mov", "wmv", "webm", "flv", "m4v", "mpg", "mpeg", "ts"];
const MUSIC: &[&str] = &["mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus"];
const ZIP: &[&str] = &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "cab"];

/// "Now" and the local time zone, so `date:today` means the user's today. Injectable for tests.
#[derive(Clone, Copy)]
pub struct Clock {
    now: i64,
    /// seconds to add to UTC to get local time
    offset: i64,
}

impl Clock {
    #[cfg(test)]
    pub fn new(now: i64, offset: i64) -> Self {
        Clock { now, offset }
    }

    pub fn system() -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let mut tz: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
        let kind = unsafe { GetTimeZoneInformation(&mut tz) };
        // Bias is UTC minus local, in minutes; daylight saving adds its own bias
        let bias = tz.Bias + if kind == 2 { tz.DaylightBias } else { tz.StandardBias };
        Clock { now, offset: -(bias as i64) * 60 }
    }

    /// Unix time of the local midnight that starts today.
    fn today(&self) -> i64 {
        (self.now + self.offset).div_euclid(DAY) * DAY - self.offset
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Only {
    Files,
    Folders,
}

#[derive(Debug, Default, PartialEq)]
pub struct Query {
    /// fuzzy words; every one must match the name
    pub terms: Vec<String>,
    /// exact substrings of the name
    pub phrases: Vec<String>,
    /// substrings the name must not contain
    pub not: Vec<String>,
    pub ext: Vec<String>,
    pub not_ext: Vec<String>,
    /// text that some ancestor folder's name must contain
    pub dirs: Vec<String>,
    pub only: Option<Only>,
    /// last modification, unix seconds, [from, to)
    pub date: Option<(i64, i64)>,
    /// file size in bytes, [min, max]
    pub size: Option<(u64, u64)>,
    /// `app:` — apps only, no files
    pub apps: bool,
}

/// Split on spaces, keeping "quoted phrases" together. The flag says the token was quoted.
fn tokens(text: &str) -> Vec<(String, bool)> {
    let (mut out, mut cur, mut in_quote) = (vec![], String::new(), false);
    for c in text.chars() {
        match c {
            '"' => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), in_quote));
                }
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    out.push((std::mem::take(&mut cur), false));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push((cur, in_quote)); // an unterminated quote still counts as a phrase
    }
    out
}

fn exts(list: &str) -> impl Iterator<Item = String> + '_ {
    list.split([';', ',']).map(|e| e.trim().trim_start_matches('.').to_string()).filter(|e| !e.is_empty())
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// The inverse of `days_from_civil`: (year, month, day).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

const WEEKDAYS: [&[&str]; 7] = [
    &["monday", "mon", "lunedì", "lunedi", "lun"],
    &["tuesday", "tue", "martedì", "martedi", "mar"],
    &["wednesday", "wed", "mercoledì", "mercoledi", "mer"],
    &["thursday", "thu", "giovedì", "giovedi", "gio"],
    &["friday", "fri", "venerdì", "venerdi", "ven"],
    &["saturday", "sat", "sabato", "sab"],
    &["sunday", "sun", "domenica", "dom"],
];

/// "1.5gb", "300", "2k" to bytes. Binary units (1 kb = 1024 bytes), like Explorer.
fn bytes(v: &str) -> Option<u64> {
    let at = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(v.len());
    let (num, unit) = v.split_at(at);
    let mult: f64 = match unit {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let n: f64 = num.parse().ok()?;
    (n.is_finite() && n >= 0.0).then(|| (n * mult) as u64)
}

/// `>1gb`, `>=1gb`, `<10mb`, `<=10mb`, `10mb..1gb` (either end may be left out), `empty`, or a bare size.
fn size_range(v: &str) -> Option<(u64, u64)> {
    if v == "empty" {
        return Some((0, 0));
    }
    if let Some(r) = v.strip_prefix(">=") {
        return Some((bytes(r)?, u64::MAX));
    }
    if let Some(r) = v.strip_prefix('>') {
        return Some((bytes(r)?.saturating_add(1), u64::MAX));
    }
    if let Some(r) = v.strip_prefix("<=") {
        return Some((0, bytes(r)?));
    }
    if let Some(r) = v.strip_prefix('<') {
        return Some(match bytes(r)? {
            0 => (1, 0), // nothing is smaller than zero
            n => (0, n - 1),
        });
    }
    if let Some((a, b)) = v.split_once("..") {
        let (lo, hi) = (if a.is_empty() { 0 } else { bytes(a)? }, if b.is_empty() { u64::MAX } else { bytes(b)? });
        return Some((lo, hi));
    }
    bytes(v).map(|n| (n, n))
}

fn date_range(v: &str, c: &Clock) -> Option<(i64, i64)> {
    let t = c.today();
    let local = |y, m, d| days_from_civil(y, m, d) * DAY - c.offset;
    let (y, m, _) = civil_from_days((c.now + c.offset).div_euclid(DAY));
    let wd = ((c.now + c.offset).div_euclid(DAY) + 3).rem_euclid(7); // 1970-01-01 was a Thursday; Monday = 0
    let last = |days: i64| (t - (days - 1) * DAY, t + DAY);
    if let Some(i) = WEEKDAYS.iter().position(|names| names.contains(&v)) {
        let back = (wd - i as i64).rem_euclid(7);
        return Some((t - back * DAY, t - back * DAY + DAY));
    }
    if let Some(at) = v.find(|ch: char| !ch.is_ascii_digit()).filter(|&at| at > 0 && v[at..].chars().all(char::is_alphabetic)) {
        let n: i64 = v[..at].parse().ok().filter(|&n| n <= 100_000)?;
        return Some(last(n * match &v[at..] {
            "d" | "g" => 1,
            "w" | "s" => 7,
            "m" => 30,
            "y" | "a" => 365,
            _ => return None,
        }));
    }
    Some(match v.replace(['-', '_'], "").as_str() {
        "today" | "oggi" => (t, t + DAY),
        "yesterday" | "ieri" => (t - DAY, t),
        "week" | "settimana" => last(7),
        "month" | "mese" => last(30),
        "year" | "anno" => last(365),
        "lastweek" | "settimanascorsa" => (t - (wd + 7) * DAY, t - wd * DAY),
        "lastmonth" | "mesescorso" => (if m == 1 { local(y - 1, 12, 1) } else { local(y, m - 1, 1) }, local(y, m, 1)),
        "lastyear" | "annoscorso" => (local(y - 1, 1, 1), local(y, 1, 1)),
        _ => {
            let n = v.split('-').map(|p| p.parse::<i64>().ok()).collect::<Option<Vec<_>>>()?;
            if !(1970..=2100).contains(n.first()?) {
                return None;
            }
            match n[..] {
                [y] => (local(y, 1, 1), local(y + 1, 1, 1)),
                [y, m] if (1..=12).contains(&m) => (local(y, m, 1), if m == 12 { local(y + 1, 1, 1) } else { local(y, m + 1, 1) }),
                [y, m, d] if (1..=12).contains(&m) && (1..=31).contains(&d) => (local(y, m, d), local(y, m, d) + DAY),
                _ => return None,
            }
        }
    })
}

impl Query {
    pub fn parse(text: &str, clock: &Clock) -> Query {
        let mut q = Query::default();
        for (tok, quoted) in tokens(text) {
            let t = tok.to_lowercase();
            if quoted {
                q.phrases.push(t);
            } else if let Some(rest) = t.strip_prefix('-').filter(|r| !r.is_empty()) {
                match rest.strip_prefix("ext:") {
                    Some(e) => q.not_ext.extend(exts(e)),
                    None => q.not.push(rest.to_string()),
                }
            } else if let Some(e) = t.strip_prefix("ext:").or_else(|| t.strip_prefix("*.")) {
                q.ext.extend(exts(e));
            } else if let Some(v) = t.strip_prefix("date:") {
                q.date = date_range(v, clock).or(q.date); // an unreadable date is ignored, not an error
            } else if let Some(v) = t.strip_prefix("size:") {
                q.size = size_range(v).or(q.size); // an unreadable size is ignored, like an unreadable date
            } else if t.ends_with('\\') {
                let d = t.trim_end_matches('\\');
                if !d.is_empty() {
                    q.dirs.push(d.to_string());
                }
            } else {
                let category = |list: &[&str], q: &mut Query| q.ext.extend(list.iter().map(|e| e.to_string()));
                match t.as_str() {
                    "folder:" | "dir:" => q.only = Some(Only::Folders),
                    "file:" => q.only = Some(Only::Files),
                    "app:" => q.apps = true,
                    "doc:" => category(DOC, &mut q),
                    "pic:" => category(PIC, &mut q),
                    "video:" | "vid:" => category(VIDEO, &mut q),
                    "music:" | "audio:" => category(MUSIC, &mut q),
                    "zip:" | "archive:" => category(ZIP, &mut q),
                    _ => q.terms.push(t),
                }
            }
        }
        q
    }

    /// Anything besides plain words (these make a query about files only, never apps).
    pub fn has_filters(&self) -> bool {
        !(self.phrases.is_empty() && self.not.is_empty() && self.ext.is_empty() && self.not_ext.is_empty()
            && self.dirs.is_empty() && self.only.is_none() && self.date.is_none() && self.size.is_none())
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && !self.has_filters() && !self.apps
    }

    pub fn searches_files(&self) -> bool {
        !self.apps
    }

    pub fn wants_apps(&self) -> bool {
        self.apps || (!self.terms.is_empty() && !self.has_filters())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2023-11-14 22:13:20 UTC, one hour east of UTC (so local time is already Nov 14 23:13)
    const CLOCK: Clock = Clock { now: 1_700_000_000, offset: 3600 };

    fn p(s: &str) -> Query {
        Query::parse(s, &CLOCK)
    }

    #[test]
    fn plain_words_and_phrases() {
        assert_eq!(p("Report 2024").terms, ["report", "2024"]);
        let q = p(r#"invoice "Annual Report" -Draft"#);
        assert_eq!((q.terms, q.phrases, q.not), (vec!["invoice".to_string()], vec!["annual report".to_string()], vec!["draft".to_string()]));
        assert_eq!(p("\"unterminated phrase").phrases, ["unterminated phrase"]);
        assert!(p("   ").is_empty());
    }

    #[test]
    fn extensions_categories_and_flags() {
        let q = p("ext:PDF;.docx *.md -ext:tmp");
        assert_eq!((q.ext, q.not_ext), (vec!["pdf".to_string(), "docx".into(), "md".into()], vec!["tmp".to_string()]));
        assert!(p("pic:").ext.contains(&"png".to_string()));
        assert_eq!(p("folder:").only, Some(Only::Folders));
        assert_eq!(p("file: x").only, Some(Only::Files));
        assert!(p("app: calc").apps && p("app: calc").wants_apps() && !p("app: calc").searches_files());
        assert!(p("ext:").is_empty()); // a keyword with no value changes nothing
    }

    #[test]
    fn folder_narrowing_and_app_rules() {
        let q = p(r"docs\ report");
        assert_eq!((q.dirs, q.terms), (vec!["docs".to_string()], vec!["report".to_string()]));
        assert!(p("calc").wants_apps()); // plain words also look for apps
        assert!(!p("calc ext:exe").wants_apps()); // any filter makes it a file query
        assert!(!p("-old calc").wants_apps());
    }

    #[test]
    fn sizes() {
        const KB: u64 = 1024;
        const MB: u64 = KB * KB;
        const GB: u64 = MB * KB;
        assert_eq!(p("size:>1gb").size, Some((GB + 1, u64::MAX)));
        assert_eq!(p("size:>=1gb").size, Some((GB, u64::MAX)));
        assert_eq!(p("size:<10mb").size, Some((0, 10 * MB - 1)));
        assert_eq!(p("size:<=10MB").size, Some((0, 10 * MB)));
        assert_eq!(p("size:10mb..1gb").size, Some((10 * MB, GB)));
        assert_eq!(p("size:..5k").size, Some((0, 5 * KB)));
        assert_eq!(p("size:1.5mb..").size, Some((MB + MB / 2, u64::MAX)));
        assert_eq!(p("size:empty").size, Some((0, 0)));
        assert_eq!(p("size:300").size, Some((300, 300)));
        assert_eq!(p("size:<0").size, Some((1, 0))); // matches nothing
        assert_eq!(p("size:big").size, None); // unreadable: ignored
        assert_eq!(p("size:>1xb").size, None);
        assert!(p("size:>1gb").has_filters() && !p("size:>1gb").wants_apps());
    }

    #[test]
    fn dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        // local midnight of Nov 14 is 23:00 UTC on Nov 13
        let midnight = days_from_civil(2023, 11, 14) * DAY - 3600;
        assert_eq!(p("date:today").date, Some((midnight, midnight + DAY)));
        assert_eq!(p("date:yesterday").date, Some((midnight - DAY, midnight)));
        assert_eq!(p("date:week").date, Some((midnight - 6 * DAY, midnight + DAY)));
        assert_eq!(p("date:2023-11-14").date, Some((midnight, midnight + DAY)));
        let nov = p("date:2023-11").date.unwrap();
        assert_eq!(nov.1 - nov.0, 30 * DAY);
        let dec = p("date:2023-12").date.unwrap();
        assert_eq!(dec.0, nov.1);
        assert_eq!(p("date:2023").date.map(|d| d.1 - d.0), Some(365 * DAY));
        assert_eq!(p("date:nonsense").date, None);
        assert_eq!(p("date:2023-13").date, None);
    }

    #[test]
    fn natural_dates() {
        for z in [0, 11_017, 19_675, -1, 50_000] {
            assert_eq!(days_from_civil(civil_from_days(z).0, civil_from_days(z).1, civil_from_days(z).2), z);
        }
        let t = days_from_civil(2023, 11, 14) * DAY - 3600; // CLOCK's local today is Tuesday 2023-11-14
        assert_eq!(p("date:3d").date, Some((t - 2 * DAY, t + DAY)));
        assert_eq!(p("date:1w").date, p("date:week").date);
        assert_eq!(p("date:2s").date, Some((t - 13 * DAY, t + DAY)));
        assert_eq!(p("date:1y").date, p("date:anno").date);
        assert_eq!(p("date:oggi").date, p("date:today").date);
        assert_eq!(p("date:Tuesday").date, Some((t, t + DAY))); // today counts
        assert_eq!(p("date:lunedì").date, Some((t - DAY, t)));
        assert_eq!(p("date:wed").date, Some((t - 6 * DAY, t - 5 * DAY)));
        assert_eq!(p("date:lastweek").date, Some((t - 8 * DAY, t - DAY))); // Mon Nov 6 .. Mon Nov 13
        assert_eq!(p("date:settimana-scorsa").date, p("date:last-week").date);
        let oct = days_from_civil(2023, 10, 1) * DAY - 3600;
        assert_eq!(p("date:lastmonth").date, Some((oct, oct + 31 * DAY)));
        assert_eq!(p("date:anno-scorso").date, p("date:2022").date);
        assert_eq!(p("date:3x").date, None);
        assert_eq!(p("date:99999999999d").date, None);
    }
}
