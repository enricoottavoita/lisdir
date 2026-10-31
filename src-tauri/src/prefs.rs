//! The settings window: native controls on a tall page that scrolls inside the window. Every control has an id and is
//! found through it; `layout` places whatever is visible from top to bottom, and runs again when the content changes.
use crate::{actions, bar, engine::{self, wide}, gfx::{self, Theme}, peer, settings::{key_name, Settings}, t};
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
    sync::atomic::{AtomicPtr, Ordering::SeqCst},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::HWND as ComHwnd,
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL},
        UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN_FILESYSPATH},
    },
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, SystemServices::{SS_NOPREFIX, SS_NOTIFY, SS_OWNERDRAW}},
    UI::{
        Controls::{SetScrollInfo, SetScrollPos, SetWindowTheme, DRAWITEMSTRUCT, ODS_FOCUS, ODS_SELECTED},
        HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem},
        Input::KeyboardAndMouse::{GetFocus, GetKeyState, SetFocus, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT, VK_TAB},
        WindowsAndMessaging::*,
    },
};

const WIDTH: i32 = 560;
const HEIGHT: i32 = 780;
const PAD: i32 = 24;
const EDIT_H: i32 = 28;
const AREA_H: i32 = 78;
const BADGE_W: i32 = 240;

const HOTKEY: i32 = 10;
const RECORD: i32 = 11;
const LANGUAGE: i32 = 12;
const THEME: i32 = 13;
const AUTOSTART: i32 = 14;
const TYPE_SEARCH: i32 = 15;
const UPDATES: i32 = 16;
const TRAY: i32 = 17;
const FOLDERS: i32 = 18;
const NO_INDEX: i32 = 19;
const EXCLUDE: i32 = 20;
/// the three "Add folder…" buttons; each one is its list's id + PICK
const PICK: i32 = 3;
const PEERS_ON: i32 = 24;
const PEER_LIST: i32 = 25;
const MY_CODE: i32 = 26;
const COPY_CODE: i32 = 27;
const PAIR_OPEN: i32 = 28;
const PAIR_MSG: i32 = 29;
const PAIR_WHO: i32 = 30;
const PAIR_YES: i32 = 31;
const PAIR_NO: i32 = 32;
const PAIR_CODE: i32 = 33;
const PAIR_GO: i32 = 34;
const PAIR_RES: i32 = 35;
const PEERS: i32 = 36;
const MSG: i32 = 37;
const RESTART: i32 = 38;
const CLOSE: i32 = 39;
const SAVE: i32 = 40;
const BADGE: i32 = 41;
/// the check box of each disk; its label, like every check box's, is the id + LABEL
const DRIVE: i32 = 200;
const LABEL: i32 = 100;

const WM_PAIRED: u32 = WM_APP + 1;
const TIMER: usize = 1;

/// One line of the page, top to bottom.
enum Line {
    Gap(i32),
    /// a thin line across the page, drawn in its background
    Rule,
    /// text across the whole width, as tall as it needs
    Text(i32),
    /// the same, for a control without an id of its own
    Plain(HWND),
    /// label and hint on the left, controls (id, width, height) on the right
    Row(HWND, Option<HWND>, Vec<(i32, i32, i32)>),
    /// a row of controls (id, width), aligned left or right
    Controls(Vec<(i32, i32)>, bool),
    /// a tall text box
    Area(i32),
    /// the list of paired PCs
    Peers,
}

struct Prefs {
    hwnd: HWND,
    page: HWND,
    theme: Theme,
    bg: HBRUSH,
    field: HBRUSH,
    fonts: Vec<HFONT>,
    scale: f32,
    lines: Vec<Line>,
    /// where `Rule` lines were placed, and the height of the whole page
    rules: Vec<i32>,
    height: i32,
    /// static texts in the dimmer colour
    dim: Vec<HWND>,
    /// letters of the disks, in check box order
    drives: Vec<String>,
    /// settings as last saved
    saved: Settings,
    recording: bool,
    message_is_error: bool,
    peers: Vec<peer::PeerState>,
    badge: (String, u32),
    running: bool,
    /// an answer to "Pair" is awaited
    busy: bool,
    was_open: bool,
    seen_done: Option<String>,
}

static PREFS: AtomicPtr<Prefs> = AtomicPtr::new(null_mut());
static HANDLE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());

/// ponytail: one mutable global used only on the main thread, like the bar's.
unsafe fn me() -> &'static mut Prefs {
    &mut *PREFS.load(SeqCst)
}

// fonts by index in `Prefs::fonts`
const NORMAL: usize = 0;
const SMALL: usize = 1;
const HEADING: usize = 2;
const TITLE: usize = 3;
const MONO: usize = 4;

unsafe fn get(s: &Prefs, id: i32) -> HWND {
    GetDlgItem(s.page, id)
}

fn px(s: &Prefs, v: i32) -> i32 {
    (v as f32 * s.scale) as i32
}

unsafe fn is_shown(h: HWND) -> bool {
    GetWindowLongW(h, GWL_STYLE) as u32 & WS_VISIBLE != 0
}

unsafe fn show(s: &Prefs, id: i32, on: bool) {
    ShowWindow(get(s, id), if on { SW_SHOWNA } else { SW_HIDE });
}

unsafe fn checked(s: &Prefs, id: i32) -> bool {
    SendMessageW(get(s, id), BM_GETCHECK, 0, 0) == 1
}

unsafe fn set_checked(s: &Prefs, id: i32, on: bool) {
    SendMessageW(get(s, id), BM_SETCHECK, on as usize, 0);
}

unsafe fn text(s: &Prefs, id: i32) -> String {
    gfx::text_of(get(s, id))
}

unsafe fn set(s: &Prefs, id: i32, value: &str) {
    gfx::set_text(get(s, id), value);
}

unsafe fn draw_text(dc: HDC, value: &str, r: &mut RECT, flags: u32) {
    let mut value = wide(value);
    DrawTextW(dc, value.as_mut_ptr(), -1, r, flags | DT_NOPREFIX);
}

/// The height of a text wrapped to this width; width 0 asks for the width of one line instead.
unsafe fn measure(s: &Prefs, h: HWND, width: i32) -> (i32, i32) {
    let dc = GetDC(s.page);
    let old = SelectObject(dc, SendMessageW(h, WM_GETFONT, 0, 0) as HGDIOBJ);
    let mut r = RECT { left: 0, top: 0, right: width, bottom: 0 };
    let flags = if width > 0 { DT_WORDBREAK } else { DT_SINGLELINE };
    draw_text(dc, &gfx::text_of(h), &mut r, DT_CALCRECT | flags);
    SelectObject(dc, old);
    ReleaseDC(s.page, dc);
    (r.right, r.bottom)
}

/// A button's width from its text.
unsafe fn button_w(s: &Prefs, id: i32) -> i32 {
    measure(s, get(s, id), 0).0 + px(s, 26)
}

unsafe fn place(h: HWND, x: i32, y: i32, w: i32, height: i32) {
    SetWindowPos(h, null_mut(), x, y, w, height, SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Place everything that is visible, and size the page and the scroll bar to fit.
unsafe fn layout(s: &mut Prefs) {
    let mut c: RECT = zeroed();
    GetClientRect(s.hwnd, &mut c);
    let (x, w) = (px(s, PAD), c.right - px(s, PAD) * 2);
    let (mut y, gap) = (px(s, 20), px(s, 6));
    let mut rules = vec![];
    for line in &s.lines {
        match line {
            Line::Gap(v) => y += px(s, *v),
            Line::Rule => {
                y += px(s, 8);
                rules.push(y);
                y += px(s, 8);
            }
            Line::Text(id) => {
                let h = get(s, *id);
                if is_shown(h) {
                    let tall = if gfx::text_of(h).is_empty() { 0 } else { measure(s, h, w).1 };
                    place(h, x, y, w, tall);
                    y += tall + gap;
                }
            }
            Line::Plain(h) => {
                let tall = measure(s, *h, w).1;
                place(*h, x, y, w, tall);
                y += tall + gap;
            }
            Line::Row(label, hint, controls) => {
                let right: i32 = controls.iter().map(|(_, cw, _)| px(s, *cw) + gap).sum();
                let text_w = w - right - px(s, 6);
                let top = measure(s, *label, text_w).1;
                let more = hint.map_or(0, |hint| measure(s, hint, text_w).1 + 2);
                let tall = controls.iter().map(|(_, _, ch)| px(s, *ch)).max().unwrap_or(0).max(top + more);
                let first = y + (tall - top - more) / 2;
                place(*label, x, first, text_w, top);
                if let Some(hint) = hint {
                    place(*hint, x, first + top + 2, text_w, more - 2);
                }
                let mut cx = x + w;
                for (id, cw, ch) in controls.iter().rev() {
                    let (cw, ch) = (px(s, *cw), px(s, *ch));
                    cx -= cw;
                    // a list box's height includes its drop-down
                    let drop = if *id == LANGUAGE || *id == THEME { px(s, 160) } else { 0 };
                    place(get(s, *id), cx, y + (tall - ch) / 2, cw, ch + drop);
                    cx -= gap;
                }
                y += tall;
            }
            Line::Controls(controls, right) => {
                if !controls.iter().any(|(id, _)| is_shown(get(s, *id))) {
                    continue;
                }
                let shown: Vec<(i32, i32)> = controls.iter().filter(|(id, _)| is_shown(get(s, *id))).map(|&(id, cw)| (id, if cw == 0 { button_w(s, id) } else { px(s, cw) })).collect();
                let total: i32 = shown.iter().map(|(_, cw)| cw + gap).sum::<i32>() - gap;
                let mut cx = if *right { x + w - total } else { x };
                for (id, cw) in shown {
                    place(get(s, id), cx, y, cw, px(s, EDIT_H));
                    cx += cw + gap;
                }
                y += px(s, EDIT_H) + gap;
            }
            Line::Area(id) => {
                place(get(s, *id), x, y, w, px(s, AREA_H));
                y += px(s, AREA_H) + gap;
            }
            Line::Peers => {
                let h = get(s, PEER_LIST);
                if is_shown(h) {
                    let tall = px(s, 24) * s.peers.len() as i32;
                    place(h, x, y, w, tall);
                    y += tall + gap;
                }
            }
        }
    }
    s.rules = rules;
    s.height = y + px(s, 20);
    let info = SCROLLINFO { cbSize: size_of::<SCROLLINFO>() as u32, fMask: SIF_RANGE | SIF_PAGE, nMin: 0, nMax: s.height - 1, nPage: c.bottom as u32, nPos: 0, nTrackPos: 0 };
    SetScrollInfo(s.hwnd, SB_VERT as i32, &info, 1);
    scroll_to(s, GetScrollPos(s.hwnd, SB_VERT as i32));
    InvalidateRect(s.page, null(), 1);
}

unsafe fn scroll_to(s: &mut Prefs, pos: i32) {
    let mut c: RECT = zeroed();
    GetClientRect(s.hwnd, &mut c);
    let pos = pos.clamp(0, (s.height - c.bottom).max(0));
    SetScrollPos(s.hwnd, SB_VERT as i32, pos, 1);
    SetWindowPos(s.page, null_mut(), 0, -pos, c.right, s.height, SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Bring a control that got the focus with Tab into view.
unsafe fn reveal(s: &mut Prefs, h: HWND) {
    let (mut r, mut c): (RECT, RECT) = (zeroed(), zeroed());
    GetWindowRect(h, &mut r);
    GetClientRect(s.hwnd, &mut c);
    MapWindowPoints(null_mut(), s.hwnd, (&mut r as *mut RECT).cast(), 2);
    let pos = GetScrollPos(s.hwnd, SB_VERT as i32);
    if r.top < 0 {
        scroll_to(s, pos + r.top - px(s, 12));
    } else if r.bottom > c.bottom {
        scroll_to(s, pos + r.bottom - c.bottom + px(s, 12));
    }
}

fn lines_of(value: &str) -> Vec<String> {
    value.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn joined(list: &[String]) -> String {
    list.join("\r\n")
}

const LANGUAGES: [&str; 3] = ["auto", "en", "it"];
const THEMES: [&str; 5] = ["auto", "dark", "light", "midnight", "paper"];

unsafe fn choice(s: &Prefs, id: i32, values: &[&str]) -> String {
    let i = SendMessageW(get(s, id), CB_GETCURSEL, 0, 0).max(0) as usize;
    values[i.min(values.len() - 1)].to_string()
}

/// The form as settings; what is not on the form stays as saved.
unsafe fn form(s: &Prefs) -> Settings {
    let mut n = s.saved.clone();
    n.hotkey = text(s, HOTKEY).trim().to_lowercase();
    n.language = choice(s, LANGUAGE, &LANGUAGES);
    n.theme = choice(s, THEME, &THEMES);
    n.hide_tray = checked(s, TRAY);
    n.check_updates = checked(s, UPDATES);
    n.type_search = checked(s, TYPE_SEARCH);
    n.folders = lines_of(&text(s, FOLDERS));
    n.disks_off = s.drives.iter().enumerate().filter(|(i, _)| !checked(s, DRIVE + *i as i32)).map(|(_, d)| d.clone()).collect();
    n.no_index = lines_of(&text(s, NO_INDEX));
    n.exclude = lines_of(&text(s, EXCLUDE));
    n.peers_on = checked(s, PEERS_ON);
    n.peers = lines_of(&text(s, PEERS));
    n
}

unsafe fn fill(s: &Prefs, c: &Settings) {
    set(s, HOTKEY, &c.hotkey);
    set(s, FOLDERS, &joined(&c.folders));
    set(s, NO_INDEX, &joined(&c.no_index));
    set(s, EXCLUDE, &joined(&c.exclude));
    set(s, PEERS, &joined(&c.peers));
}

unsafe fn say(s: &mut Prefs, value: &str, error: bool) {
    s.message_is_error = error;
    set(s, MSG, value);
    layout(s);
}

unsafe fn record(s: &mut Prefs, on: bool) {
    s.recording = on;
    set(s, RECORD, if on { t("Press keys…", "Premi i tasti…") } else { t("Record", "Registra") });
    InvalidateRect(get(s, RECORD), null(), 0);
    layout(s);
}

unsafe fn save(s: &mut Prefs) {
    let next = form(s);
    let look = (next.language != s.saved.language, next.theme != s.saved.theme);
    match crate::save_settings(next) {
        Ok(restart) => {
            if checked(s, AUTOSTART) != crate::autostart() {
                crate::set_autostart(checked(s, AUTOSTART));
            }
            s.saved = crate::app().settings.read().unwrap().clone();
            fill(s, &s.saved);
            show(s, RESTART, restart);
            let note = if restart { t("Saved. Restart lisdir to apply the changes to what is indexed.", "Salvato. Riavvia lisdir per applicare le modifiche a ciò che viene indicizzato.") } else { t("Saved", "Salvato") };
            if look != (false, false) {
                return rebuild(s, note, false);
            }
            say(s, note, false);
        }
        Err(e) => say(s, &e, true),
    }
}

/// Make the window again, with what is on the form: language, theme and sizes are set when controls are created.
unsafe fn rebuild(s: &mut Prefs, message: &str, error: bool) {
    let (settings, autostart, restart) = (form(s), checked(s, AUTOSTART), is_shown(get(s, RESTART)));
    let (mut r, pos) = (zeroed(), GetScrollPos(s.hwnd, SB_VERT as i32));
    GetWindowRect(s.hwnd, &mut r);
    let saved = s.saved.clone();
    DestroyWindow(s.hwnd);
    build(&settings, saved, autostart, Some((r.left, r.top)));
    let s = me();
    show(s, RESTART, restart);
    say(s, message, error);
    scroll_to(s, pos);
}

/// The folder chooser of Windows. None if the user cancels.
unsafe fn pick_folder(owner: HWND) -> Option<String> {
    let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
    dialog.SetOptions(dialog.GetOptions().ok()? | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
    dialog.SetTitle(PCWSTR(wide(t("Choose a folder", "Scegli una cartella")).as_ptr())).ok()?;
    dialog.Show(Some(ComHwnd(owner))).ok()?;
    let raw = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let path = raw.to_string().ok();
    CoTaskMemFree(Some(raw.0 as *const _));
    Some(path?.trim_end_matches('\\').to_string())
}

/// `Add folder…`: the path goes on a new line of that list, unless it is there already.
unsafe fn add_folder(s: &Prefs, list: i32) {
    let Some(path) = pick_folder(s.hwnd) else { return };
    let mut have = lines_of(&text(s, list));
    if !have.iter().any(|l| l.eq_ignore_ascii_case(&path)) {
        have.push(path);
        set(s, list, &joined(&have));
    }
}

/// Ask the PC with this code to pair; its person confirms there.
unsafe fn pair_with(s: &mut Prefs) {
    let code = text(s, PAIR_CODE).trim().to_string();
    if code.is_empty() {
        return;
    }
    s.busy = true;
    set(s, PAIR_RES, t("Waiting for the other PC to confirm…", "In attesa che l'altro PC confermi…"));
    layout(s);
    peer::rt().spawn(async move {
        let answer = peer::pair(code).await;
        PostMessageW(HANDLE.load(SeqCst), WM_PAIRED, 0, Box::into_raw(Box::new(answer)) as isize);
    });
}

unsafe fn peers_refresh(s: &Prefs) {
    set(s, PEERS, &joined(&crate::app().settings.read().unwrap().peers));
}

/// Once a second, only while the window is open: the badge, the list of PCs, and whether a PC is asking to pair.
unsafe fn tick(s: &mut Prefs) {
    if IsIconic(s.hwnd) != 0 {
        return;
    }
    let (running, list) = (peer::running(), peer::status());
    let (saved, n, it) = (s.saved.peers_on, list.iter().filter(|p| p.online).count(), crate::italian());
    let label = match (running, saved) {
        (true, true) if n == 0 => t("Active · no PC connected", "Attiva · nessun PC connesso").to_string(),
        (true, true) if it => format!("Attiva · {n} {}", if n == 1 { "connesso" } else { "connessi" }),
        (true, true) => format!("Active · {n} connected"),
        (true, false) => t("Off after a restart", "Si spegne al riavvio").to_string(),
        (false, true) => t("Restart lisdir to turn it on", "Riavvia lisdir per accenderla").to_string(),
        (false, false) => t("Off", "Spenta").to_string(),
    };
    // green when searching others, amber when half-way
    let color = if running && saved && n > 0 { 0x2da44e } else if running || saved { 0xd29922 } else { s.theme.sel };
    let badge_changed = (label.clone(), color) != s.badge;
    let drawn = |p: &peer::PeerState| (p.name.clone(), p.online, p.seen.map(|v| v / 60));
    let rows_changed = !list.iter().map(drawn).eq(s.peers.iter().map(drawn));
    let mut moved = badge_changed || list.len() != s.peers.len() || running != s.running;
    s.badge = (label, color);
    s.running = running;
    s.peers = list;
    show(s, PEER_LIST, running && !s.peers.is_empty());
    // only what changed: repainting every second makes it blink
    if badge_changed {
        InvalidateRect(get(s, BADGE), null(), 0);
    }
    if rows_changed {
        InvalidateRect(get(s, PEER_LIST), null(), 0);
    }

    let st = peer::pairing_state();
    let asking = !st.name.is_empty();
    if asking != is_shown(get(s, PAIR_WHO)) {
        moved = true;
    }
    for id in [PAIR_WHO, PAIR_YES, PAIR_NO] {
        show(s, id, asking);
    }
    if asking {
        let (start, end): (String, String) = (st.code.chars().take(8).collect(), st.code.chars().skip(st.code.chars().count().saturating_sub(6)).collect());
        let who = if it {
            format!("«{}» vuole abbinarsi. Codice di verifica: {}. Deve essere uguale su quel PC, che lo mostra ora. (Il suo codice inizia con {start} e finisce con {end}.)", st.name, st.check)
        } else {
            format!("“{}” wants to pair. Verification code: {}. It must be the same on that PC, which shows it now. (Its code starts with {start} and ends with {end}.)", st.name, st.check)
        };
        if who != text(s, PAIR_WHO) {
            set(s, PAIR_WHO, &who);
            moved = true;
        }
    }
    let wait = t("Waiting for the other PC to confirm…", "In attesa che l'altro PC confermi…");
    if s.busy && !st.check.is_empty() {
        let check = if it { format!("Codice di verifica: {}. Controlla che sia uguale sull'altro PC prima che confermi.", st.check) } else { format!("Verification code: {}. Check it is the same on the other PC before it confirms.", st.check) };
        let value = format!("{wait} {check}");
        if value != text(s, PAIR_RES) {
            set(s, PAIR_RES, &value);
            moved = true;
        }
    }
    let mut message = None;
    if st.open {
        message = Some(t("Waiting for the other PC…", "In attesa dell'altro PC…").to_string());
    } else if s.was_open {
        message = Some(String::new());
        peers_refresh(s); // closed: either paired or timed out; show the list as it is now
    }
    s.was_open = st.open;
    // a PC was just paired with this one (the first look only takes note of what was there before)
    if s.seen_done.as_ref().is_some_and(|d| !st.done.is_empty() && *d != st.done) {
        message = Some(if it { format!("Abbinato: {} è ora nell'elenco di questo PC.", st.done) } else { format!("Paired: {} is now in this PC's list.", st.done) });
    }
    s.seen_done = Some(st.done);
    if let Some(m) = message.filter(|m| *m != text(s, PAIR_MSG)) {
        set(s, PAIR_MSG, &m);
        moved = true;
    }
    if moved {
        layout(s);
    }
}

unsafe fn draw_button(s: &Prefs, d: &DRAWITEMSTRUCT) {
    let primary = d.CtlID as i32 == SAVE || d.CtlID as i32 == PAIR_YES;
    let (fill_rgb, text_rgb) = if primary { (s.theme.hl, 0xffffff) } else { (s.theme.sel, s.theme.fg) };
    let r = d.rcItem;
    gfx::round(d.hDC, r, s.theme.bg, fill_rgb, if primary { fill_rgb } else { s.theme.line }, px(s, 5) as f32);
    SetBkMode(d.hDC, TRANSPARENT as _);
    SetTextColor(d.hDC, gfx::colorref(text_rgb));
    let old = SelectObject(d.hDC, s.fonts[NORMAL] as HGDIOBJ);
    let mut at = r;
    if d.itemState & ODS_SELECTED != 0 {
        at.top += 2;
    }
    draw_text(d.hDC, &gfx::text_of(d.hwndItem), &mut at, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    SelectObject(d.hDC, old);
    if d.itemState & ODS_FOCUS != 0 {
        let mut focus = r;
        InflateRect(&mut focus, -3, -3);
        DrawFocusRect(d.hDC, &focus);
    }
}

unsafe fn draw_badge(s: &Prefs, d: &DRAWITEMSTRUCT) {
    FillRect(d.hDC, &d.rcItem, s.bg);
    if s.badge.0.is_empty() {
        return;
    }
    let old = SelectObject(d.hDC, s.fonts[SMALL] as HGDIOBJ);
    let mut size = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    draw_text(d.hDC, &s.badge.0, &mut size, DT_CALCRECT | DT_SINGLELINE);
    let r = d.rcItem;
    let mut pill = RECT { left: (r.right - size.right - px(s, 16)).max(r.left), top: r.top, right: r.right, bottom: r.bottom };
    gfx::round(d.hDC, pill, s.theme.bg, s.badge.1, s.badge.1, (pill.bottom - pill.top) as f32 / 2.0);
    SetBkMode(d.hDC, TRANSPARENT as _);
    SetTextColor(d.hDC, gfx::colorref(if s.badge.1 == s.theme.sel { s.theme.dim } else { 0xffffff }));
    draw_text(d.hDC, &s.badge.0, &mut pill, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
    SelectObject(d.hDC, old);
}

unsafe fn draw_peers(s: &Prefs, d: &DRAWITEMSTRUCT) {
    FillRect(d.hDC, &d.rcItem, s.bg);
    SetBkMode(d.hDC, TRANSPARENT as _);
    let (row, dot) = (px(s, 24), px(s, 8));
    let it = crate::italian();
    for (i, p) in s.peers.iter().enumerate() {
        let top = d.rcItem.top + i as i32 * row;
        let color = if p.online { 0x2da44e } else { s.theme.dim };
        let at = RECT { left: 0, top: top + (row - dot) / 2, right: dot, bottom: top + (row + dot) / 2 };
        gfx::round(d.hDC, at, s.theme.bg, color, color, dot as f32 / 2.0);
        let info = if p.online {
            t("connected", "connesso").to_string()
        } else {
            let minutes = p.seen.map(|v| v / 60);
            let seen = match (minutes, it) {
                (None, _) => String::new(),
                (Some(0), false) => " · seen just now".to_string(),
                (Some(0), true) => " · visto adesso".to_string(),
                (Some(m), false) => format!(" · seen {m} min ago"),
                (Some(m), true) => format!(" · visto {m} min fa"),
            };
            format!("{}{seen}", t("not connected", "non connesso"))
        };
        let mut r = RECT { left: dot + px(s, 8), top, right: d.rcItem.right, bottom: top + row };
        let old = SelectObject(d.hDC, s.fonts[NORMAL] as HGDIOBJ);
        SetTextColor(d.hDC, gfx::colorref(s.theme.fg));
        draw_text(d.hDC, &p.name, &mut r, DT_VCENTER | DT_SINGLELINE);
        let mut size = r;
        draw_text(d.hDC, &p.name, &mut size, DT_CALCRECT | DT_SINGLELINE);
        r.left = size.right + px(s, 8);
        SelectObject(d.hDC, s.fonts[SMALL] as HGDIOBJ);
        SetTextColor(d.hDC, gfx::colorref(s.theme.dim));
        draw_text(d.hDC, &info, &mut r, DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        SelectObject(d.hDC, old);
    }
}

unsafe fn command(s: &mut Prefs, id: i32, code: u32) {
    let clicked = code == BN_CLICKED; // the same value as STN_CLICKED
    if !clicked {
        return;
    }
    match id {
        1 => {} // Enter outside a text box: the window has no default action
        2 | CLOSE => { DestroyWindow(s.hwnd); }
        RECORD => record(s, !s.recording),
        FOLDERS..=EXCLUDE => {}
        i if (FOLDERS + PICK..=EXCLUDE + PICK).contains(&i) => add_folder(s, i - PICK),
        SAVE => save(s),
        RESTART => crate::restart_app(),
        COPY_CODE => { actions::copy_text(&text(s, MY_CODE)); }
        PAIR_OPEN => {
            let on = peer::open_pairing();
            set(s, PAIR_MSG, if on { t("Waiting for the other PC…", "In attesa dell'altro PC…") } else { t("Switch the option on, save, and restart lisdir first.", "Accendi l'opzione, salva e riavvia prima lisdir.") });
            layout(s);
        }
        PAIR_YES => peer::answer_pairing(true),
        PAIR_NO => peer::answer_pairing(false),
        PAIR_GO => pair_with(s),
        i if i >= LABEL && GetDlgItem(s.page, i - LABEL) != null_mut() => {
            set_checked(s, i - LABEL, !checked(s, i - LABEL)); // a click on a check box's text counts
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let raw = PREFS.load(SeqCst);
    if raw.is_null() {
        return DefWindowProcW(h, msg, w, l);
    }
    let s = &mut *raw;
    match msg {
        WM_COMMAND if h == s.page => command(s, (w & 0xffff) as i32, (w >> 16) as u32),
        WM_COMMAND => command(s, (w & 0xffff) as i32, (w >> 16) as u32), // Enter and Esc come from the dialog keys to the window
        WM_DRAWITEM => {
            let d = &*(l as *const DRAWITEMSTRUCT);
            match d.CtlID as i32 {
                BADGE => draw_badge(s, d),
                PEER_LIST => draw_peers(s, d),
                _ => draw_button(s, d),
            }
            return 1;
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
            let edit = l as HWND == get(s, MY_CODE);
            let error = s.message_is_error && l as HWND == get(s, MSG);
            let rgb = if error { 0xe5484d } else if s.dim.contains(&(l as HWND)) { s.theme.dim } else { s.theme.fg };
            SetTextColor(w as _, gfx::colorref(rgb));
            SetBkColor(w as _, gfx::colorref(if edit { s.theme.sel } else { s.theme.bg }));
            return if edit { s.field } else { s.bg } as LRESULT;
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
            SetTextColor(w as _, gfx::colorref(s.theme.fg));
            SetBkColor(w as _, gfx::colorref(s.theme.sel));
            return s.field as LRESULT;
        }
        WM_ERASEBKGND => {
            let mut r: RECT = zeroed();
            GetClientRect(h, &mut r);
            FillRect(w as _, &r, s.bg);
            return 1;
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut r: RECT = zeroed();
            GetClientRect(h, &mut r);
            if h == s.page {
                let line = CreateSolidBrush(gfx::colorref(s.theme.line));
                for y in &s.rules {
                    FillRect(dc, &RECT { left: px(s, PAD), top: *y, right: r.right - px(s, PAD), bottom: *y + 1 }, line);
                }
                DeleteObject(line as HGDIOBJ);
            }
            EndPaint(h, &ps);
        }
        WM_VSCROLL if h == s.hwnd => {
            let mut info: SCROLLINFO = zeroed();
            info.cbSize = size_of::<SCROLLINFO>() as u32;
            info.fMask = SIF_ALL;
            GetScrollInfo(h, SB_VERT as i32, &mut info);
            let pos = match (w & 0xffff) as i32 {
                v if v == SB_LINEUP as i32 => info.nPos - px(s, 30),
                v if v == SB_LINEDOWN as i32 => info.nPos + px(s, 30),
                v if v == SB_PAGEUP as i32 => info.nPos - info.nPage as i32,
                v if v == SB_PAGEDOWN as i32 => info.nPos + info.nPage as i32,
                v if v == SB_THUMBTRACK as i32 => info.nTrackPos,
                _ => info.nPos,
            };
            scroll_to(s, pos);
        }
        WM_MOUSEWHEEL => {
            let notches = (w >> 16) as i16 as i32 / 120;
            scroll_to(s, GetScrollPos(s.hwnd, SB_VERT as i32) - notches * px(s, 60));
        }
        WM_TIMER => tick(s),
        WM_PAIRED => {
            let answer = *Box::from_raw(l as *mut Result<String, String>);
            s.busy = false;
            match answer {
                Ok(name) => {
                    set(s, PAIR_RES, &if crate::italian() { format!("Abbinato con {name}.") } else { format!("Paired with {name}.") });
                    set(s, PAIR_CODE, "");
                    peers_refresh(s);
                }
                Err(e) => set(s, PAIR_RES, &e),
            }
            layout(s);
        }
        WM_DPICHANGED if h == s.hwnd => {
            let message = text(s, MSG);
            let error = s.message_is_error;
            rebuild(s, &message, error);
        }
        WM_SETTINGCHANGE if h == s.hwnd && gfx::look_changed(l) => {
            let message = text(s, MSG);
            let error = s.message_is_error;
            rebuild(s, &message, error);
        }
        WM_CLOSE => {
            DestroyWindow(h);
        }
        WM_DESTROY if h == s.hwnd => {
            HANDLE.store(null_mut(), SeqCst);
            PREFS.store(null_mut(), SeqCst);
            peer::close_pairing();
            DeleteObject(s.bg as HGDIOBJ);
            DeleteObject(s.field as HGDIOBJ);
            s.fonts.iter().for_each(|f| { DeleteObject(*f as HGDIOBJ); });
            drop(Box::from_raw(raw));
        }
        _ => return DefWindowProcW(h, msg, w, l),
    }
    0
}

/// The wheel moves the page, unless it is over a multi-line box that was clicked and still has text to show that way.
unsafe fn box_takes_wheel(h: HWND, up: bool) -> bool {
    if h != GetFocus() || GetWindowLongW(h, GWL_STYLE) as u32 & ES_MULTILINE as u32 == 0 {
        return false;
    }
    let mut info: SCROLLINFO = zeroed();
    info.cbSize = size_of::<SCROLLINFO>() as u32;
    info.fMask = SIF_ALL;
    if GetScrollInfo(h, SB_VERT as i32, &mut info) == 0 {
        return false;
    }
    if up { info.nPos > 0 } else { info.nPos + (info.nPage as i32).max(1) <= info.nMax }
}

/// Handle the keys of the window: the keys of a recording shortcut, and Tab, Enter and Esc as in a dialog. True when used.
pub unsafe fn key(m: &MSG) -> bool {
    let raw = PREFS.load(SeqCst);
    if raw.is_null() {
        return false;
    }
    let s = &mut *raw;
    if IsChild(s.hwnd, m.hwnd) == 0 && m.hwnd != s.hwnd {
        return false;
    }
    if m.message == WM_MOUSEWHEEL && !box_takes_wheel(m.hwnd, (m.wParam >> 16) as i16 > 0) {
        SendMessageW(s.hwnd, WM_MOUSEWHEEL, m.wParam, m.lParam);
        return true;
    }
    if s.recording && (m.message == WM_KEYDOWN || m.message == WM_SYSKEYDOWN) {
        let vk = m.wParam as u32;
        let down = |k: u16| GetKeyState(k as i32) < 0;
        if vk == VK_ESCAPE as u32 {
            record(s, false);
        } else if let Some(name) = key_name(vk) {
            let mods = [(VK_CONTROL, "ctrl"), (VK_MENU, "alt"), (VK_SHIFT, "shift")];
            let mut parts: Vec<String> = mods.iter().filter(|(k, _)| down(*k)).map(|(_, n)| n.to_string()).collect();
            if down(VK_LWIN) || down(VK_RWIN) {
                parts.push("super".to_string());
            }
            parts.push(name);
            set(s, HOTKEY, &parts.join("+"));
            record(s, false);
        }
        return true;
    }
    let used = IsDialogMessageW(s.hwnd, m) != 0;
    if used && m.message == WM_KEYDOWN && m.wParam as u16 == VK_TAB {
        reveal(s, GetFocus());
    }
    used
}

/// Make the window and its controls from these values.
unsafe fn build(c: &Settings, saved: Settings, autostart: bool, at: Option<(i32, i32)>) {
    let class = gfx::register("lisdir_settings", wndproc, 0);
    let module = GetModuleHandleW(null());
    let dpi = GetDpiForSystem(); // the window moves to its real screen's DPI by itself: WM_DPICHANGED makes it again
    let scale = dpi as f32 / 96.0;
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VSCROLL;
    let mut r = RECT { left: 0, top: 0, right: (WIDTH as f32 * scale) as i32, bottom: (HEIGHT as f32 * scale) as i32 };
    AdjustWindowRectExForDpi(&mut r, style, 0, 0, dpi);
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let (x, y) = at.unwrap_or(((GetSystemMetrics(SM_CXSCREEN) - w) / 2, (GetSystemMetrics(SM_CYSCREEN) - h) / 2));
    let title = wide(t("lisdir settings", "Impostazioni di lisdir"));
    let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, x, y, w, h, null_mut(), null_mut(), module, null());
    let page = CreateWindowExW(WS_EX_CONTROLPARENT, class.as_ptr(), null(), WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN, 0, 0, 0, 0, hwnd, null_mut(), module, null());
    SetWindowLongW(hwnd, GWL_EXSTYLE, GetWindowLongW(hwnd, GWL_EXSTYLE) | WS_EX_CONTROLPARENT as i32);
    let theme = gfx::theme(&c.theme);
    theme.style_title(hwnd);
    theme.style(hwnd);
    let font = |px: f32, weight: i32, name: &str| CreateFontW(-((px * scale) as i32), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(name).as_ptr());
    let fonts = vec![font(14.0, 400, "Segoe UI"), font(12.0, 400, "Segoe UI"), font(13.0, 600, "Segoe UI"), font(18.0, 600, "Segoe UI"), font(12.0, 400, "Consolas")];
    let s = Box::leak(Box::new(Prefs {
        hwnd, page, theme,
        bg: CreateSolidBrush(gfx::colorref(theme.bg)),
        field: CreateSolidBrush(gfx::colorref(theme.sel)),
        fonts, scale,
        lines: vec![], rules: vec![], height: 0, dim: vec![], drives: vec![],
        saved,
        recording: false, message_is_error: false, peers: vec![], badge: (String::new(), 0), running: false, busy: false, was_open: false, seen_done: None,
    }));

    let (st, ed, bt, cb) = (wide("STATIC"), wide("EDIT"), wide("BUTTON"), wide("COMBOBOX"));
    let child = |class: &[u16], label: &str, style: u32, ex: u32, id: i32, font: usize| {
        let h = CreateWindowExW(ex, class.as_ptr(), wide(label).as_ptr(), WS_CHILD | style, 0, 0, 0, 0, page, id as usize as HMENU, module, null());
        SendMessageW(h, WM_SETFONT, s.fonts[font] as usize, 0);
        h
    };
    let visible = WS_VISIBLE;
    let tab = WS_VISIBLE | WS_TABSTOP;
    let mut lines = vec![];
    let mut dim = vec![];
    let hint = |value: &str, dim: &mut Vec<HWND>| {
        let h = child(&st, value, visible | SS_NOPREFIX, 0, 0, SMALL);
        dim.push(h);
        h
    };
    let label = |value: &str, id: i32| child(&st, value, visible | SS_NOPREFIX | SS_NOTIFY, 0, id, NORMAL);
    let heading = |value: &str, dim: &mut Vec<HWND>| {
        let h = child(&st, &value.to_uppercase(), visible | SS_NOPREFIX, 0, 0, HEADING);
        dim.push(h);
        h
    };
    let button = |value: &str, id: i32, style: u32| child(&bt, value, style | BS_OWNERDRAW as u32, 0, id, NORMAL);
    let field = |id: i32, extra: u32| child(&ed, "", tab | ES_AUTOHSCROLL as u32 | extra, WS_EX_CLIENTEDGE, id, NORMAL);
    let area = |id: i32| {
        let h = child(&ed, "", tab | WS_VSCROLL | ES_MULTILINE as u32 | ES_WANTRETURN as u32 | ES_AUTOVSCROLL as u32 | ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, id, MONO);
        theme.style(h);
        h
    };
    let list_box = |id: i32, values: &[&str], selected: usize| {
        let h = child(&cb, "", tab | CBS_DROPDOWNLIST as u32 | WS_VSCROLL, 0, id, NORMAL);
        let name = wide(if theme.is_dark() { "DarkMode_CFD" } else { "Explorer" });
        SetWindowTheme(h, name.as_ptr(), null());
        for v in values {
            SendMessageW(h, CB_ADDSTRING, 0, wide(v).as_ptr() as isize);
        }
        SendMessageW(h, CB_SETCURSEL, selected, 0);
    };
    let check = |id: i32, on: bool| {
        let h = child(&bt, "", tab | BS_AUTOCHECKBOX as u32, 0, id, NORMAL);
        theme.style(h);
        SendMessageW(h, BM_SETCHECK, on as usize, 0);
    };
    let at_of = |values: &[&str], v: &str| values.iter().position(|x| *x == v).unwrap_or(0);

    let title = child(&st, t("Settings", "Impostazioni"), visible | SS_NOPREFIX, 0, 0, TITLE);
    lines.push(Line::Plain(title));
    lines.push(Line::Gap(6));
    lines.push(Line::Plain(heading(t("General", "Generali"), &mut dim)));

    let (l, h) = (label(t("Shortcut to open the search bar", "Scorciatoia per aprire la barra di ricerca"), 0), hint(t("Needs Ctrl, Alt or Win, for example Alt+Space", "Serve Ctrl, Alt o Win, ad esempio Alt+Spazio"), &mut dim));
    field(HOTKEY, 0);
    button(t("Record", "Registra"), RECORD, tab);
    lines.push(Line::Row(l, Some(h), vec![(HOTKEY, 110, EDIT_H), (RECORD, 100, EDIT_H)]));
    lines.push(Line::Rule);
    let l = label(t("Language", "Lingua"), 0);
    list_box(LANGUAGE, &[t("Same as Windows", "Come Windows"), "English", "Italiano"], at_of(&LANGUAGES, &c.language));
    lines.push(Line::Row(l, None, vec![(LANGUAGE, 200, EDIT_H)]));
    lines.push(Line::Rule);
    let l = label(t("Theme", "Tema"), 0);
    list_box(THEME, &[t("Same as Windows", "Come Windows"), t("Dark", "Scuro"), t("Light", "Chiaro"), t("Midnight", "Notte"), t("Paper", "Carta")], at_of(&THEMES, &c.theme));
    lines.push(Line::Row(l, None, vec![(THEME, 200, EDIT_H)]));
    let options: [(i32, bool, &str, &str, &str, &str); 4] = [
        (AUTOSTART, autostart, "Start with Windows", "Avvia con Windows", "", ""),
        (TYPE_SEARCH, c.type_search, "Type to search in Explorer and on the desktop", "Digita per cercare in Esplora risorse e sul desktop", "Typing a letter opens the search bar with it already entered", "Digitando una lettera si apre la barra di ricerca con quella lettera già scritta"),
        (UPDATES, c.check_updates, "Check for updates", "Controlla gli aggiornamenti", "At startup and once a day; nothing installs without your click", "All'avvio e una volta al giorno; non installa nulla senza un tuo clic"),
        (TRAY, c.hide_tray, "Hide the tray icon", "Nascondi l'icona nell'area di notifica", "You can still open the bar with the shortcut, and the settings with Ctrl and comma in the bar", "Potrai aprire la barra con la scorciatoia, e le impostazioni con Ctrl e virgola nella barra"),
    ];
    for (id, on, en, it, hint_en, hint_it) in options {
        lines.push(Line::Rule);
        let l = label(t_owned(en, it), id + LABEL);
        let h = (!hint_en.is_empty()).then(|| hint(t_owned(hint_en, hint_it), &mut dim));
        check(id, on);
        lines.push(Line::Row(l, h, vec![(id, 20, 20)]));
    }

    lines.push(Line::Gap(22));
    lines.push(Line::Plain(heading(t("What is indexed", "Cosa viene indicizzato"), &mut dim)));
    lines.push(Line::Plain(label(t("Disks", "Dischi"), 0)));
    lines.push(Line::Plain(hint(t("Untick a disk to leave it out of the search", "Togli la spunta a un disco per escluderlo dalla ricerca"), &mut dim)));
    let mut drives = vec![];
    for (i, d) in engine::drives(&c.disks_off).into_iter().enumerate() {
        let id = DRIVE + i as i32;
        let name = if d.label.is_empty() { format!("{}:", d.letter) } else { format!("{}: {}", d.letter, d.label) };
        let kind = if !d.present {
            t("not connected, results kept", "non collegato, risultati mantenuti")
        } else {
            match d.kind {
                "ntfs" => t("NTFS, read from the MFT", "NTFS, letto dalla MFT"),
                "walk" => t("read folder by folder (not NTFS)", "scansione cartelle (non NTFS)"),
                _ => t("network, read folder by folder", "rete, scansione cartelle"),
            }
        };
        let l = label(&format!("{name} · {kind}"), id + LABEL);
        check(id, !c.disks_off.contains(&d.letter));
        lines.push(Line::Row(l, None, vec![(id, 20, 24)]));
        drives.push(d.letter);
    }
    let lists: [(i32, &[String], &str, &str, &str, &str); 3] = [
        (FOLDERS, &c.folders, "More folders", "Altre cartelle", "One path per line, for example \\\\nas\\share or D:\\Projects. Read again every 10 minutes", "Un percorso per riga, ad esempio \\\\nas\\condivisione o D:\\Progetti. Riletta ogni 10 minuti"),
        (NO_INDEX, &c.no_index, "Do not index", "Non indicizzare", "Left out of the index altogether (saves memory)", "Lasciato fuori dall'indice (risparmia memoria)"),
        (EXCLUDE, &c.exclude, "Hide from results", "Nascondi dai risultati", "Indexed, but never shown", "Indicizzato, ma mai mostrato"),
    ];
    for (id, value, en, it, hint_en, hint_it) in lists {
        lines.push(Line::Rule);
        lines.push(Line::Plain(label(t_owned(en, it), 0)));
        lines.push(Line::Plain(hint(t_owned(hint_en, hint_it), &mut dim)));
        area(id);
        gfx::set_text(GetDlgItem(page, id), &joined(value));
        lines.push(Line::Area(id));
        button(t("Add folder…", "Aggiungi cartella…"), id + PICK, tab);
        lines.push(Line::Controls(vec![(id + PICK, 0)], true));
    }
    lines.push(Line::Plain(hint(t("One rule per line: a name or pattern (node_modules, *.tmp), or a folder path (C:\\Users\\me\\Cache). Case does not matter.", "Una regola per riga: un nome o un modello (node_modules, *.tmp), oppure il percorso di una cartella (C:\\Users\\me\\Cache). Maiuscole e minuscole non contano."), &mut dim)));

    lines.push(Line::Gap(22));
    let l = heading(t("Other PCs", "Altri PC"), &mut dim);
    child(&st, "", visible | SS_OWNERDRAW as u32, 0, BADGE, SMALL);
    lines.push(Line::Row(l, None, vec![(BADGE, BADGE_W, 20)]));
    lines.push(Line::Gap(8));
    let (l, h) = (label(t("Search my other PCs", "Cerca nei miei altri PC"), PEERS_ON + LABEL), hint(t("Off by default. When on, this PC asks your other PCs running lisdir, and answers them", "Spento di default. Se acceso, questo PC interroga i tuoi altri PC con lisdir e risponde loro"), &mut dim));
    check(PEERS_ON, c.peers_on);
    lines.push(Line::Row(l, Some(h), vec![(PEERS_ON, 20, 20)]));
    lines.push(Line::Gap(8));
    child(&st, "", SS_OWNERDRAW as u32, 0, PEER_LIST, NORMAL);
    lines.push(Line::Peers);
    lines.push(Line::Rule);
    lines.push(Line::Plain(label(t("The code of this PC", "Il codice di questo PC"), 0)));
    lines.push(Line::Plain(hint(t("Give it to your other PC. Not a password: it only names this PC", "Daglielo all'altro PC. Non è una password: dà solo il nome a questo PC"), &mut dim)));
    field(MY_CODE, ES_READONLY as u32);
    gfx::set_text(GetDlgItem(page, MY_CODE), &peer::my_code());
    button(t("Copy", "Copia"), COPY_CODE, tab);
    lines.push(Line::Controls(vec![(MY_CODE, 420), (COPY_CODE, 0)], false));
    lines.push(Line::Rule);
    lines.push(Line::Plain(label(t("Allow a new PC", "Permetti un nuovo PC"), 0)));
    lines.push(Line::Plain(hint(t("On this PC: open for two minutes, then paste the code shown here on the other PC and press Pair", "Su questo PC: apri per due minuti, poi incolla sull'altro PC il codice mostrato qui e premi Abbina"), &mut dim)));
    button(t("Allow for 2 minutes", "Permetti per 2 minuti"), PAIR_OPEN, tab);
    lines.push(Line::Controls(vec![(PAIR_OPEN, 0)], false));
    dim.push(child(&st, "", visible | SS_NOPREFIX, 0, PAIR_MSG, SMALL));
    lines.push(Line::Text(PAIR_MSG));
    child(&st, "", SS_NOPREFIX, 0, PAIR_WHO, NORMAL);
    lines.push(Line::Text(PAIR_WHO));
    button(t("Allow", "Consenti"), PAIR_YES, WS_TABSTOP);
    button(t("Refuse", "Rifiuta"), PAIR_NO, WS_TABSTOP);
    lines.push(Line::Controls(vec![(PAIR_YES, 0), (PAIR_NO, 0)], false));
    lines.push(Line::Rule);
    lines.push(Line::Plain(label(t("Pair with another PC", "Abbina un altro PC"), 0)));
    lines.push(Line::Plain(hint(t("On the other PC: press Allow a new PC here, paste that PC's code, then Pair", "Sull'altro PC: premi Permetti un nuovo PC, incolla qui il codice di quel PC, poi Abbina"), &mut dim)));
    field(PAIR_CODE, 0);
    button(t("Pair", "Abbina"), PAIR_GO, tab);
    lines.push(Line::Controls(vec![(PAIR_CODE, 420), (PAIR_GO, 0)], false));
    dim.push(child(&st, "", visible | SS_NOPREFIX, 0, PAIR_RES, SMALL));
    lines.push(Line::Text(PAIR_RES));
    lines.push(Line::Rule);
    lines.push(Line::Plain(label(t("My other PCs", "I miei altri PC"), 0)));
    lines.push(Line::Plain(hint(t("One per line: Name|code, with the code shown on that PC. Add this PC on the other one too", "Uno per riga: Nome|codice, con il codice mostrato su quel PC. Aggiungi questo PC anche sull'altro"), &mut dim)));
    area(PEERS);
    gfx::set_text(GetDlgItem(page, PEERS), &joined(&c.peers));
    lines.push(Line::Area(PEERS));

    lines.push(Line::Gap(6));
    dim.push(child(&st, "", visible | SS_NOPREFIX, 0, MSG, NORMAL));
    lines.push(Line::Text(MSG));
    button(t("Restart now", "Riavvia ora"), RESTART, WS_TABSTOP);
    button(t("Close", "Chiudi"), CLOSE, tab);
    button(t("Save", "Salva"), SAVE, tab);
    lines.push(Line::Controls(vec![(RESTART, 0), (CLOSE, 0), (SAVE, 0)], true));

    gfx::set_text(GetDlgItem(page, HOTKEY), &c.hotkey);
    (s.lines, s.dim, s.drives) = (lines, dim, drives);
    HANDLE.store(hwnd, SeqCst);
    PREFS.store(s, SeqCst);
    tick(s);
    layout(s);
    SetTimer(hwnd, TIMER, 1000, None);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(get(s, HOTKEY));
}

/// `t` for strings that come out of a table.
fn t_owned<'a>(en: &'a str, it: &'a str) -> &'a str {
    if crate::italian() { it } else { en }
}

/// Open the settings window, or bring it forward.
pub fn open() {
    unsafe {
        let h = HANDLE.load(SeqCst);
        if !h.is_null() {
            ShowWindow(h, SW_RESTORE);
            SetForegroundWindow(h);
            return;
        }
        let saved = crate::app().settings.read().unwrap().clone();
        build(&saved.clone(), saved, crate::autostart(), None);
    }
    let _ = bar::hwnd();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_box_of_text_becomes_clean_lines() {
        assert_eq!(lines_of("  a \r\n\r\nb\n"), vec!["a", "b"]);
        assert_eq!(joined(&lines_of("a\nb")), "a\r\nb");
        assert!(lines_of(" \r\n ").is_empty());
    }
}
