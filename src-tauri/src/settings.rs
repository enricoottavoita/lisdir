//! User settings, kept in `%LOCALAPPDATA%\lisdir\settings.txt` as plain `key=value` lines (comments start with `#`).
//! A missing file or key means the default; an unreadable value is ignored, so a hand-edited file never breaks startup.
//! The list settings (`folder`, `disk_off`, `no_index`, `exclude`) are written one line per entry.
use crate::engine::data_dir;
use std::{path::PathBuf, sync::atomic::{AtomicU8, Ordering}};

pub const THEMES: &[&str] = &["auto", "dark", "light", "midnight", "paper"];
pub const LANGUAGES: &[&str] = &["auto", "en", "it"];
/// More than this in one list is a mistake, not a configuration.
const MAX_LIST: usize = 200;
const MAX_LINE: usize = 260;

/// 0 = follow Windows, 1 = English, 2 = Italian. A global so the free `t()` helper can read it from anywhere.
pub static LANGUAGE: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// global shortcut that opens the bar, e.g. `alt+space`
    pub hotkey: String,
    /// `auto` (Windows), `en` or `it`
    pub language: String,
    /// one of `THEMES`
    pub theme: String,
    pub hide_tray: bool,
    /// look for a newer release at startup and daily
    pub check_updates: bool,
    /// typing in an Explorer folder or on the desktop opens the bar
    pub type_search: bool,
    /// extra folders to index by walking them (network shares, or any folder), besides the disks
    pub folders: Vec<String>,
    /// drive letters that are not indexed, e.g. `D`
    pub disks_off: Vec<String>,
    /// rules (see `rules`) for what is left out of the index
    pub no_index: Vec<String>,
    /// rules for what is indexed but not shown in results
    pub exclude: Vec<String>,
    /// search the other PCs that run lisdir, and let them search this one (off until switched on)
    pub peers_on: bool,
    /// trusted PCs, one `Name|code` per line
    pub peers: Vec<String>,
    /// the preview pane of the bar is open (`Alt+P`, remembered)
    pub preview: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            hotkey: "alt+space".into(),
            language: "auto".into(),
            theme: "auto".into(),
            hide_tray: false,
            check_updates: true,
            type_search: true,
            folders: vec![],
            disks_off: vec![],
            no_index: vec![],
            exclude: vec![],
            peers_on: false,
            peers: vec![],
            preview: false,
        }
    }
}

fn truthy(v: &str) -> Option<bool> {
    match v {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// "d", "D:" or "d:\" to "D".
fn drive_letter(v: &str) -> Option<String> {
    let t = v.trim().trim_end_matches('\\').trim_end_matches(':');
    (t.len() == 1 && t.chars().all(|c| c.is_ascii_alphabetic())).then(|| t.to_uppercase())
}

/// Why a shortcut cannot be used, or None when it is fine. A shortcut needs Ctrl, Alt or Win next to a key: a bare
/// letter, digit, `Space` or `Shift+letter` would be swallowed from everything you type. Function keys may stand alone.
pub fn hotkey_problem(hotkey: &str) -> Option<&'static str> {
    let parts: Vec<String> = hotkey.split('+').map(|p| p.trim().to_lowercase()).collect();
    let Some((key, mods)) = parts.split_last() else { return Some("empty") };
    if key.is_empty() || mods.iter().any(|m| m.is_empty()) {
        return Some("incomplete");
    }
    if mods.iter().any(|m| !matches!(m.as_str(), "ctrl" | "control" | "alt" | "shift" | "super" | "win" | "meta" | "cmd")) {
        return Some("unknown modifier");
    }
    if matches!(key.as_str(), "ctrl" | "control" | "alt" | "shift" | "super" | "win" | "meta" | "cmd") {
        return Some("needs a key besides the modifiers");
    }
    let function_key = key.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()).is_some_and(|n| (1..=24).contains(&n));
    let strong = mods.iter().any(|m| m != "shift");
    if !strong && !function_key {
        return Some("needs Ctrl, Alt or Win");
    }
    None
}

/// Named keys of a shortcut and their virtual-key codes (letters, digits and function keys are worked out).
const KEYS: &[(&str, u32)] = &[
    ("space", 0x20), ("enter", 0x0D), ("tab", 0x09), ("up", 0x26), ("down", 0x28), ("left", 0x25), ("right", 0x27),
    ("backquote", 0xC0), ("comma", 0xBC), ("period", 0xBE), ("slash", 0xBF), ("semicolon", 0xBA), ("quote", 0xDE),
    ("minus", 0xBD), ("equal", 0xBB), ("bracketleft", 0xDB), ("bracketright", 0xDD), ("backslash", 0xDC),
    ("home", 0x24), ("end", 0x23), ("pageup", 0x21), ("pagedown", 0x22), ("insert", 0x2D), ("delete", 0x2E),
    ("backspace", 0x08), ("escape", 0x1B),
];

/// The virtual-key code of a key name: `k`, `7`, `f9`, `space`.
fn key_code(name: &str) -> Option<u32> {
    let name = name.strip_prefix("key").or(name.strip_prefix("digit")).filter(|n| n.len() == 1).unwrap_or(name);
    if let [c] = name.as_bytes() {
        if c.is_ascii_alphanumeric() {
            return Some(c.to_ascii_uppercase() as u32);
        }
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6F + n); // VK_F1 is 0x70
    }
    KEYS.iter().find(|(k, _)| *k == name).map(|(_, vk)| *vk)
}

/// The name of a virtual key in a shortcut, None for a key that cannot be one (a bare modifier).
pub fn key_name(vk: u32) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some((vk as u8 as char).to_ascii_lowercase().to_string()),
        0x70..=0x87 => Some(format!("f{}", vk - 0x6F)),
        _ => KEYS.iter().find(|(_, v)| *v == vk).map(|(k, _)| k.to_string()),
    }
}

/// A shortcut as `RegisterHotKey` wants it: modifiers (MOD_ALT 1, MOD_CONTROL 2, MOD_SHIFT 4, MOD_WIN 8) and virtual key.
pub fn hotkey_keys(hotkey: &str) -> Option<(u32, u32)> {
    let parts: Vec<String> = hotkey.split('+').map(|p| p.trim().to_lowercase()).collect();
    let (key, mods) = parts.split_last()?;
    let mut bits = 0;
    for m in mods {
        bits |= match m.as_str() {
            "alt" => 1,
            "ctrl" | "control" => 2,
            "shift" => 4,
            "super" | "win" | "meta" | "cmd" => 8,
            _ => return None,
        };
    }
    Some((bits, key_code(key)?))
}

/// Trim, drop empty lines and duplicates, and cap the length.
fn clean(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for l in list.iter().map(|l| l.trim()).filter(|l| !l.is_empty() && !l.starts_with('#') && l.len() <= MAX_LINE) {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(l)) && out.len() < MAX_LIST {
            out.push(l.to_string());
        }
    }
    out
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        let (mut folders, mut disks, mut skip, mut hide, mut peers) = (vec![], vec![], vec![], vec![], vec![]);
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "hotkey" if hotkey_problem(v).is_none() => s.hotkey = v.to_lowercase(),
                "language" if LANGUAGES.contains(&v) => s.language = v.into(),
                "theme" if THEMES.contains(&v) => s.theme = v.into(),
                "hide_tray" => s.hide_tray = truthy(v).unwrap_or(s.hide_tray),
                "check_updates" => s.check_updates = truthy(v).unwrap_or(s.check_updates),
                "type_search" => s.type_search = truthy(v).unwrap_or(s.type_search),
                "folder" => folders.push(v.trim_end_matches('\\').to_string()),
                "disk_off" => disks.extend(drive_letter(v)),
                "no_index" => skip.push(v.to_string()),
                "exclude" => hide.push(v.to_string()),
                "peers_on" => s.peers_on = truthy(v).unwrap_or(s.peers_on),
                "peer" => peers.push(v.to_string()),
                "preview" => s.preview = truthy(v).unwrap_or(s.preview),
                _ => {}
            }
        }
        (s.folders, s.disks_off, s.no_index, s.exclude, s.peers) = (clean(&folders), clean(&disks), clean(&skip), clean(&hide), clean(&peers));
        s
    }

    pub fn to_text(&self) -> String {
        let b = |v: bool| if v { 1 } else { 0 };
        let mut t = format!(
            "# lisdir settings (edited from the tray menu: Settings…)\nhotkey={}\nlanguage={}\ntheme={}\nhide_tray={}\ncheck_updates={}\ntype_search={}\npeers_on={}\npreview={}\n",
            self.hotkey, self.language, self.theme, b(self.hide_tray), b(self.check_updates), b(self.type_search), b(self.peers_on), b(self.preview)
        );
        for (key, list) in [("folder", &self.folders), ("disk_off", &self.disks_off), ("no_index", &self.no_index), ("exclude", &self.exclude), ("peer", &self.peers)] {
            for item in list {
                t += &format!("{key}={item}\n");
            }
        }
        t
    }

    fn path() -> PathBuf {
        data_dir().join("settings.txt")
    }

    pub fn load() -> Settings {
        let s = std::fs::read_to_string(Self::path()).map(|t| Settings::parse(&t)).unwrap_or_default();
        s.apply_language();
        s
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        std::fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(tmp, p)
    }

    /// Make `t()` follow the chosen language.
    pub fn apply_language(&self) {
        LANGUAGE.store(LANGUAGES.iter().position(|l| *l == self.language).unwrap_or(0) as u8, Ordering::SeqCst);
    }

    /// Is this drive switched off for indexing?
    pub fn disk_is_off(&self, letter: char) -> bool {
        self.disks_off.iter().any(|d| d.eq_ignore_ascii_case(&letter.to_string()))
    }

    /// Do the indexed disks, folders or rules differ, so that a restart is needed to apply them?
    pub fn index_differs(&self, other: &Settings) -> bool {
        (&self.folders, &self.disks_off, &self.no_index, &self.exclude) != (&other.folders, &other.disks_off, &other.no_index, &other.exclude)
    }

    /// Was the other-PCs option switched on or off? That is read at startup, so a restart applies it (the list of PCs
    /// applies at once).
    pub fn peers_differ(&self, other: &Settings) -> bool {
        self.peers_on != other.peers_on
    }

    /// Remember a paired PC (or its new name).
    pub fn add_peer(&mut self, name: &str, code: &str) {
        self.peers.retain(|l| !l.ends_with(code));
        self.peers.push(format!("{name}|{code}"));
    }

    /// Turn a settings object coming from the window into a clean one, or say what is wrong with it.
    pub fn checked(mut self) -> Result<Settings, String> {
        if let Some(why) = hotkey_problem(&self.hotkey) {
            return Err(format!("shortcut: {why}"));
        }
        if !LANGUAGES.contains(&self.language.as_str()) || !THEMES.contains(&self.theme.as_str()) {
            return Err("unknown language or theme".into());
        }
        self.hotkey = self.hotkey.trim().to_lowercase();
        self.folders = clean(&self.folders.iter().map(|f| f.trim().trim_end_matches('\\').to_string()).collect::<Vec<_>>());
        self.disks_off = clean(&self.disks_off.iter().filter_map(|d| drive_letter(d)).collect::<Vec<_>>());
        (self.no_index, self.exclude) = (clean(&self.no_index), clean(&self.exclude));
        self.peers = clean(&self.peers);
        if let Some(bad) = self.peers.iter().find(|l| crate::peer::parse_peer(l).is_none()) {
            return Err(format!("other PCs: \"{bad}\" is not a valid code (use Name|code)"));
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let d = Settings::default();
        assert_eq!(Settings::parse(""), d);
        let s = Settings {
            hotkey: "ctrl+alt+k".into(), language: "it".into(), theme: "midnight".into(), hide_tray: true, check_updates: false,
            type_search: false, folders: vec![r"\\nas\share".into(), r"E:\Photos".into()], disks_off: vec!["D".into()],
            no_index: vec!["node_modules".into(), "*.tmp".into()], exclude: vec![r"C:\Windows".into()],
            peers_on: true, peers: vec!["Office|abc".into()], preview: true,
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
    }

    #[test]
    fn bad_values_fall_back_to_the_default() {
        let s = Settings::parse("# c\nhotkey=space\nlanguage=fr\ntheme=neon\nhide_tray=maybe\ncheck_updates=0\nnonsense\nunknown=1\ndisk_off=zz\ndisk_off=d:\\\ndisk_off=D\n");
        assert_eq!(s, Settings { check_updates: false, disks_off: vec!["D".into()], ..Settings::default() });
    }

    #[test]
    fn lists_are_cleaned() {
        let s = Settings { folders: vec![" X:\\a\\ ".into(), "x:\\A".into(), "".into()], no_index: vec!["  .git ".into(), ".GIT".into()], ..Settings::default() }.checked().unwrap();
        assert_eq!(s.folders, vec![r"X:\a"]);
        assert_eq!(s.no_index, vec![".git"]);
        assert!(s.disk_is_off('D') == false);
        let off = Settings { disks_off: vec!["d:".into(), "bad".into()], ..Settings::default() }.checked().unwrap();
        assert!(off.disk_is_off('d') && off.disk_is_off('D') && !off.disk_is_off('C'));
        assert!(off.index_differs(&Settings::default()) && !Settings::default().index_differs(&Settings::default()));
    }

    #[test]
    fn shortcuts_become_key_codes_and_back() {
        assert_eq!(hotkey_keys("alt+space"), Some((1, 0x20)));
        assert_eq!(hotkey_keys("Ctrl+Shift+K"), Some((2 | 4, 0x4B)));
        assert_eq!(hotkey_keys("super+7"), Some((8, 0x37)));
        assert_eq!(hotkey_keys("f9"), Some((0, 0x78)));
        assert_eq!(hotkey_keys("ctrl+keyp"), Some((2, 0x50)));
        assert_eq!(hotkey_keys("ctrl+digit1"), Some((2, 0x31)));
        assert_eq!(hotkey_keys("ctrl+f25"), None);
        assert_eq!(hotkey_keys("hyper+a"), None);
        assert_eq!(hotkey_keys("ctrl+nosuchkey"), None);
        for vk in [0x20, 0x41, 0x5A, 0x30, 0x70, 0x87, 0xBC, 0xC0] {
            assert_eq!(hotkey_keys(&key_name(vk).unwrap()).unwrap().1, vk);
        }
        assert_eq!(key_name(0x11), None); // Ctrl alone
    }

    #[test]
    fn shortcuts_need_a_strong_modifier() {
        for ok in ["alt+space", "ctrl+shift+f", "Ctrl+Alt+K", "super+s", "f9", "shift+f5", "ctrl+space"] {
            assert_eq!(hotkey_problem(ok), None, "{ok}");
        }
        for bad in ["", "space", "a", "shift+a", "ctrl", "alt+shift", "ctrl+", "+a", "hyper+a", "ctrl++a"] {
            assert!(hotkey_problem(bad).is_some(), "{bad}");
        }
        assert!(Settings { hotkey: "a".into(), ..Settings::default() }.checked().is_err());
        assert!(Settings::default().checked().is_ok());
    }
}
