//! User settings, kept in `%LOCALAPPDATA%\lisdir\settings.txt` as plain `key=value` lines (comments start with `#`).
//! A missing file or key means the default; an unreadable value is ignored, so a hand-edited file never breaks startup.
use crate::engine::data_dir;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::atomic::{AtomicU8, Ordering}};

pub const THEMES: &[&str] = &["auto", "dark", "light", "midnight", "paper"];
pub const LANGUAGES: &[&str] = &["auto", "en", "it"];

/// 0 = follow Windows, 1 = English, 2 = Italian. A global so the free `t()` helper can read it from anywhere.
pub static LANGUAGE: AtomicU8 = AtomicU8::new(0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings { hotkey: "alt+space".into(), language: "auto".into(), theme: "auto".into(), hide_tray: false, check_updates: true }
    }
}

fn truthy(v: &str) -> Option<bool> {
    match v {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
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

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "hotkey" if hotkey_problem(v).is_none() => s.hotkey = v.to_lowercase(),
                "language" if LANGUAGES.contains(&v) => s.language = v.into(),
                "theme" if THEMES.contains(&v) => s.theme = v.into(),
                "hide_tray" => s.hide_tray = truthy(v).unwrap_or(s.hide_tray),
                "check_updates" => s.check_updates = truthy(v).unwrap_or(s.check_updates),
                _ => {}
            }
        }
        s
    }

    pub fn to_text(&self) -> String {
        let b = |v: bool| if v { 1 } else { 0 };
        format!(
            "# lisdir settings (edited from the tray menu: Settings…)\nhotkey={}\nlanguage={}\ntheme={}\nhide_tray={}\ncheck_updates={}\n",
            self.hotkey, self.language, self.theme, b(self.hide_tray), b(self.check_updates)
        )
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

    /// Check a settings object coming from the window before it is used or saved.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(why) = hotkey_problem(&self.hotkey) {
            return Err(format!("shortcut: {why}"));
        }
        if !LANGUAGES.contains(&self.language.as_str()) || !THEMES.contains(&self.theme.as_str()) {
            return Err("unknown language or theme".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let d = Settings::default();
        assert_eq!(Settings::parse(""), d);
        let s = Settings { hotkey: "ctrl+alt+k".into(), language: "it".into(), theme: "midnight".into(), hide_tray: true, check_updates: false };
        assert_eq!(Settings::parse(&s.to_text()), s);
    }

    #[test]
    fn bad_values_fall_back_to_the_default() {
        let s = Settings::parse("# c\nhotkey=space\nlanguage=fr\ntheme=neon\nhide_tray=maybe\ncheck_updates=0\nnonsense\nunknown=1\n");
        assert_eq!(s, Settings { check_updates: false, ..Settings::default() });
    }

    #[test]
    fn shortcuts_need_a_strong_modifier() {
        for ok in ["alt+space", "ctrl+shift+f", "Ctrl+Alt+K", "super+s", "f9", "shift+f5", "ctrl+space"] {
            assert_eq!(hotkey_problem(ok), None, "{ok}");
        }
        for bad in ["", "space", "a", "shift+a", "ctrl", "alt+shift", "ctrl+", "+a", "hyper+a", "ctrl++a"] {
            assert!(hotkey_problem(bad).is_some(), "{bad}");
        }
        assert!(Settings { hotkey: "a".into(), ..Settings::default() }.validate().is_err());
        assert!(Settings::default().validate().is_ok());
    }
}
