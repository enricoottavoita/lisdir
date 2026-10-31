//! The search bar: a borderless window with a native text box, and the list, action menu and preview drawn by `gfx`.
//! Everything here runs on the main thread; slow work (search, icons, previews, status) runs on other threads and comes
//! back as a posted message carrying a boxed value.
use crate::{actions, engine::wide, gfx::{self, Canvas, Theme}, icons, peer, service::Hit, t, tray, typesearch};
use std::{
    collections::HashMap,
    ptr::{null, null_mut},
    sync::{atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering::SeqCst}, mpsc::{channel, Sender}},
    time::Duration,
};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::{BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, SetBkColor, SetTextColor, HBRUSH, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT},
    },
    UI::{
        Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETSEL},
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{GetKeyState, SendInput, SetFocus, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_UP},
        WindowsAndMessaging::*,
    },
};

// Sizes in device-independent pixels.
const LIST_W: f32 = 700.0;
const PANE_W: f32 = 400.0;
const HEIGHT: f32 = 440.0;
const INPUT_H: f32 = 60.0;
const FOOT_H: f32 = 33.0;
const HEAD_H: f32 = 24.0;
const ROW_H: f32 = 48.0;
const MENU_ROW: f32 = 32.0;
const MENU_W: f32 = 290.0;

const WM_RESULT: u32 = WM_APP + 1;
const WM_REMOTE: u32 = WM_APP + 2;
const WM_ICON: u32 = WM_APP + 3;
const WM_PREVIEW: u32 = WM_APP + 4;
const WM_STATUS: u32 = WM_APP + 5;
const WM_SHOW: u32 = WM_APP + 6;

const T_POLL: usize = 1;
const T_PREVIEW: usize = 2;
const T_PENDING: usize = 3;

/// The action menu: action, English and Italian label, shortcut in both languages. The shortcuts work with the menu closed too.
const ACTS: [(&str, &str, &str, &str, &str); 9] = [
    ("open", "Open", "Apri", "Enter", "Invio"),
    ("folder", "Show in folder", "Mostra nella cartella", "Ctrl+Enter", "Ctrl+Invio"),
    ("copy", "Copy", "Copia", "Ctrl+C", "Ctrl+C"),
    ("cut", "Cut", "Taglia", "Ctrl+X", "Ctrl+X"),
    ("path", "Copy path", "Copia percorso", "Ctrl+Shift+C", "Ctrl+Maiusc+C"),
    ("openas", "Open with…", "Apri con…", "", ""),
    ("terminal", "Open terminal here", "Apri terminale qui", "Ctrl+T", "Ctrl+T"),
    ("properties", "Properties", "Proprietà", "Alt+Enter", "Alt+Invio"),
    ("pin", "Pin to top", "Fissa in cima", "Ctrl+P", "Ctrl+P"),
];

struct Bar {
    hwnd: HWND,
    edit: HWND,
    /// the text of a preview; a native box, so it scrolls and wraps by itself
    pv_edit: HWND,
    canvas: Option<Canvas>,
    theme: Theme,
    brush: HBRUSH,
    font: HFONT,
    mono: HFONT,
    /// pixels per device-independent pixel
    scale: f32,
    hits: Vec<Hit>,
    sel: usize,
    /// how far the list is scrolled
    top: f32,
    seq: u64,
    /// the answer in progress keeps the selected item (a refresh) or starts from the top (typing)
    stay: bool,
    local: Option<Vec<Hit>>,
    more: Vec<Hit>,
    /// the last answer of the other PCs and its text, kept while a refresh asks again
    remote: Vec<Hit>,
    remote_q: Option<String>,
    /// highlighted row of the action menu; None = closed
    menu: Option<usize>,
    pv_on: bool,
    pv_path: Option<String>,
    pv_image: Option<(ID2D1Bitmap, f32, f32)>,
    /// normal and grey icon by `Hit::icon`; None while it is being read
    icons: HashMap<String, Option<(ID2D1Bitmap, ID2D1Bitmap)>>,
    status: String,
    version: String,
    searches: Sender<(u64, String)>,
    icon_jobs: Sender<(String, String)>,
}

static BAR: AtomicPtr<Bar> = AtomicPtr::new(null_mut());
static HANDLE: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());
/// set once the window really got focus after being shown; blur before that is a foreground-lock glitch, not a click-away
static FOCUSED: AtomicBool = AtomicBool::new(false);
static POLLING: AtomicBool = AtomicBool::new(false);
/// the search in progress, for the threads that ask the other PCs
static SEQ: AtomicU64 = AtomicU64::new(0);

/// ponytail: one mutable global used only on the main thread, which window messages re-enter while it is in use.
unsafe fn me() -> &'static mut Bar {
    &mut *BAR.load(SeqCst)
}

pub fn hwnd() -> HWND {
    HANDLE.load(SeqCst)
}

/// Hand a value from a worker thread to the main thread.
fn send<T: Send>(msg: u32, value: T) {
    unsafe { PostMessageW(hwnd(), msg, 0, Box::into_raw(Box::new(value)) as isize) };
}

unsafe fn recv<T>(l: LPARAM) -> T {
    *Box::from_raw(l as *mut T)
}

/// Open the bar from any thread, without waiting for it.
pub fn request_show() {
    unsafe { PostMessageW(hwnd(), WM_SHOW, 0, 0) };
}

/// 1536 -> "1.5 KB". Binary units, like Explorer.
fn fmt_size(n: u64) -> String {
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < 4 {
        v /= 1024.0;
        i += 1;
    }
    let unit = ["B", "KB", "MB", "GB", "TB"][i];
    if i > 0 && v < 10.0 { format!("{v:.1} {unit}") } else { format!("{} {unit}", v.round()) }
}

/// 1234567 -> "1,234,567" or "1.234.567".
fn fmt_count(n: usize, sep: char) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Char positions (as in `Hit::hl`) to runs of UTF-16 units.
fn marks(name: &str, hl: &[usize]) -> Vec<(u32, u32)> {
    let (mut out, mut at): (Vec<(u32, u32)>, u32) = (vec![], 0);
    for (i, c) in name.chars().enumerate() {
        let n = c.len_utf16() as u32;
        if hl.contains(&i) {
            match out.last_mut() {
                Some((start, len)) if *start + *len == at => *len += n,
                _ => out.push((at, n)),
            }
        }
        at += n;
    }
    out
}

/// Where the word before `at` starts, for Ctrl+Backspace (the text box has no such key of its own).
fn word_start(text: &[u16], at: usize) -> usize {
    let space = |i: usize| char::from_u32(text[i] as u32).is_some_and(char::is_whitespace);
    let mut i = at;
    while i > 0 && space(i - 1) {
        i -= 1;
    }
    while i > 0 && !space(i - 1) {
        i -= 1;
    }
    i
}

/// The window's size in device-independent pixels.
fn width(s: &Bar) -> f32 {
    if s.pv_on { LIST_W + PANE_W } else { LIST_W }
}

fn query(s: &Bar) -> String {
    gfx::text_of(s.edit)
}

/// Nothing typed: the list is your history, grey and titled, not search results.
fn history(s: &Bar) -> bool {
    !s.hits.is_empty() && query(s).trim().is_empty()
}

fn list_top(s: &Bar) -> f32 {
    INPUT_H + if history(s) { HEAD_H } else { 0.0 }
}

fn list_view(s: &Bar) -> f32 {
    HEIGHT - FOOT_H - list_top(s) - 12.0
}

/// apps open or pin; a file on another PC can only have its path copied
fn menu_acts(s: &Bar) -> Vec<usize> {
    match s.hits.get(s.sel) {
        Some(h) if !h.peer.is_empty() => vec![4],
        Some(h) if h.app => vec![0, 8],
        _ => (0..ACTS.len()).collect(),
    }
}

/// The action menu's rectangle: below the selected row, or above it if it does not fit.
fn menu_rect(s: &Bar) -> gfx::Rect {
    let h = menu_acts(s).len() as f32 * MENU_ROW + 8.0;
    let row = list_top(s) + 6.0 + s.sel as f32 * ROW_H - s.top;
    let y = if row + ROW_H + h <= HEIGHT { row + ROW_H } else { (row - h).max(INPUT_H) };
    (LIST_W - 18.0 - MENU_W, y, MENU_W, h)
}

unsafe fn repaint(s: &Bar) {
    InvalidateRect(s.hwnd, null(), 0);
}

fn scroll_to_selection(s: &mut Bar) {
    let (y, view) = (s.sel as f32 * ROW_H, list_view(s));
    if y < s.top {
        s.top = y;
    } else if y + ROW_H > s.top + view {
        s.top = y + ROW_H - view;
    }
    s.top = s.top.clamp(0.0, (s.hits.len() as f32 * ROW_H - view).max(0.0));
}

/// After any change to the list or the selection.
unsafe fn render(s: &mut Bar) {
    if s.sel >= s.hits.len() || s.hits.is_empty() {
        s.menu = None;
    }
    scroll_to_selection(s);
    show_preview(s);
    repaint(s);
}

/// Whichever answer comes last redraws the list: this PC's files first, then the other PCs'.
unsafe fn draw(s: &mut Bar, stay: bool) {
    let cur = s.hits.get(s.sel).map(|h| h.path.clone());
    s.hits = s.local.clone().unwrap_or_default();
    s.hits.extend(s.more.iter().cloned());
    s.sel = cur.filter(|_| stay).and_then(|c| s.hits.iter().position(|h| h.path == c)).unwrap_or(0);
    if !stay {
        s.top = 0.0;
    }
    render(s);
}

unsafe fn run(s: &mut Bar, refresh: bool) {
    s.seq += 1;
    SEQ.store(s.seq, SeqCst);
    let (seq, q) = (s.seq, query(s));
    s.more = if s.remote_q.as_ref() == Some(&q) { s.remote.clone() } else { vec![] };
    s.local = None;
    s.stay = refresh;
    if q.trim().is_empty() {
        s.local = Some(crate::search(&q)); // no search behind this, and the list is there before the window shows
        return draw(s, refresh);
    }
    // the other PCs are asked at the same time as this one, after a short pause so quick typing makes one question
    if peer::running() {
        let (text, delay) = (q.clone(), if refresh && !s.more.is_empty() { 1000 } else { 60 });
        peer::rt().spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            if SEQ.load(SeqCst) == seq {
                let got = crate::remote_search(text.clone()).await;
                send(WM_REMOTE, (seq, text, got));
            }
        });
    }
    let _ = s.searches.send((seq, q));
}

unsafe fn poll() {
    if !POLLING.swap(true, SeqCst) {
        std::thread::spawn(|| {
            let st = crate::status();
            POLLING.store(false, SeqCst);
            send(WM_STATUS, st);
        });
    }
}

unsafe fn hide(s: &Bar) {
    ShowWindow(s.hwnd, SW_HIDE);
    KillTimer(s.hwnd, T_POLL);
}

/// Centre the window at its current size on its screen.
unsafe fn place(s: &mut Bar) {
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromWindow(s.hwnd, MONITOR_DEFAULTTOPRIMARY), &mut mi);
    let (w, h) = ((width(s) * s.scale) as i32, (HEIGHT * s.scale) as i32);
    let r = mi.rcMonitor;
    SetWindowPos(s.hwnd, HWND_TOPMOST, r.left + (r.right - r.left - w) / 2, r.top + (r.bottom - r.top - h) / 2, w, h, SWP_NOACTIVATE);
}

/// Bring the window forward. Windows refuses when the keyboard went to another program last (the Explorer hook): tapping
/// Alt first counts as input of ours and lifts the refusal.
unsafe fn foreground(h: HWND) {
    if SetForegroundWindow(h) != 0 && GetForegroundWindow() == h {
        return;
    }
    let key = |flags| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MENU, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
    let tap = [key(0), key(KEYEVENTF_KEYUP)];
    SendInput(2, tap.as_ptr(), size_of::<INPUT>() as i32);
    SetForegroundWindow(h);
}

/// Show and focus the bar if it is hidden. Keys typed in Explorer or on the desktop were parked by the hook; claim them
/// here. Otherwise a fresh open starts empty, which lists your recent items.
unsafe fn show(s: &mut Bar) {
    if IsWindowVisible(s.hwnd) != 0 {
        return;
    }
    FOCUSED.store(false, SeqCst);
    crate::reload_apps(crate::app());
    s.menu = None;
    s.icons.retain(|_, v| v.is_some()); // what failed is tried again
    place(s);
    gfx::set_text(s.edit, &typesearch::take());
    run(s, false);
    let end = gfx::text_of(s.edit).encode_utf16().count();
    SendMessageW(s.edit, EM_SETSEL, end, end as isize);
    poll();
    ShowWindow(s.hwnd, SW_SHOW);
    foreground(s.hwnd);
    SetFocus(s.edit);
    SetTimer(s.hwnd, T_PENDING, 150, None); // keys that raced the focus change
}

pub fn toggle() {
    unsafe {
        let s = me();
        if IsWindowVisible(s.hwnd) != 0 { hide(s) } else { show(s) }
    }
}

/// Language, theme and placeholder again, after a change in the settings or in Windows.
pub fn refresh() {
    unsafe {
        let s = me();
        s.theme = gfx::theme(&crate::app().settings.read().unwrap().theme);
        DeleteObject(s.brush as HGDIOBJ);
        s.brush = CreateSolidBrush(gfx::colorref(s.theme.bg));
        s.theme.style(s.pv_edit);
        SendMessageW(s.edit, 0x1501 /* EM_SETCUEBANNER */, 1, wide(t("Search files, folders and apps…", "Cerca file, cartelle e app…")).as_ptr() as isize);
        repaint(s);
    }
}

unsafe fn set_pane(s: &mut Bar, on: bool) {
    s.pv_on = on;
    {
        let mut settings = crate::app().settings.write().unwrap();
        settings.preview = on;
        let _ = settings.save();
    }
    ShowWindow(s.pv_edit, SW_HIDE);
    (s.pv_path, s.pv_image) = (None, None);
    place(s);
    show_preview(s);
    repaint(s);
}

/// Preview pane (Alt+P, remembered): the start of a text file, else the shell's thumbnail or big icon. Only once the
/// selection settles: holding ↓ reads no files.
unsafe fn show_preview(s: &mut Bar) {
    let p = s.hits.get(s.sel).filter(|h| s.pv_on && !h.app && h.peer.is_empty()).map(|h| h.path.clone());
    if p == s.pv_path {
        return;
    }
    s.pv_path = p;
    KillTimer(s.hwnd, T_PREVIEW);
    if s.pv_path.is_some() {
        SetTimer(s.hwnd, T_PREVIEW, 80, None);
    } else {
        ShowWindow(s.pv_edit, SW_HIDE);
        s.pv_image = None;
    }
}

fn load_preview(path: String, is_dir: bool) {
    std::thread::spawn(move || {
        let text = (!is_dir).then(|| actions::text_start(&path)).flatten();
        let image = text.is_none().then(|| icons::thumbnail_image(&path, 512)).flatten();
        send(WM_PREVIEW, (path, text, image));
    });
}

unsafe fn go(s: &mut Bar, i: usize, folder: bool) {
    let Some(h) = s.hits.get(i).cloned() else { return };
    if h.app && folder {
        return; // an app has no folder to reveal
    }
    if !h.peer.is_empty() {
        s.sel = i;
        return act(s, "path"); // not on this PC: copy its path instead of opening
    }
    crate::open(&h, folder);
    hide(s);
}

unsafe fn act(s: &mut Bar, a: &str) {
    let h = s.hits.get(s.sel).cloned();
    s.menu = None;
    repaint(s);
    let Some(h) = h else { return };
    if !h.peer.is_empty() {
        if a != "pin" {
            hide(s);
            crate::act("path", &h);
        }
    } else if a == "open" || a == "folder" {
        go(s, s.sel, a == "folder");
    } else if a == "pin" {
        crate::pin(&h);
        run(s, true);
    } else if !h.app {
        hide(s); // first, so the dialog or terminal comes to the front
        crate::act(a, &h);
    }
}

/// The action a key combination stands for, if any. With text selected in the box, Ctrl+C / Ctrl+X edit the text.
fn key_act(vk: u16, ctrl: bool, alt: bool, shift: bool, selected: bool) -> Option<&'static str> {
    if vk == VK_RETURN {
        return Some(if alt { "properties" } else if ctrl { "folder" } else { "open" });
    }
    if !ctrl || alt {
        return None;
    }
    match vk as u8 {
        b'T' => Some("terminal"),
        b'P' => Some("pin"),
        _ if selected => None,
        b'C' => Some(if shift { "path" } else { "copy" }),
        b'X' => Some("cut"),
        _ => None,
    }
}

/// A key pressed in the text box, before the box sees it. True when it was ours.
pub unsafe fn key(m: &MSG) -> bool {
    let s = me();
    if m.hwnd != s.edit || !(m.message == WM_KEYDOWN || m.message == WM_SYSKEYDOWN) {
        return false;
    }
    let vk = m.wParam as u16;
    let down = |k: u16| GetKeyState(k as i32) < 0;
    let (ctrl, alt, shift) = (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT));
    let (mut from, mut to) = (0u32, 0u32);
    SendMessageW(s.edit, EM_GETSEL, &mut from as *mut u32 as usize, &mut to as *mut u32 as isize);
    let text: Vec<u16> = query(s).encode_utf16().collect();
    let n = menu_acts(s).len();
    match (s.menu, vk) {
        (Some(i), VK_DOWN) => s.menu = Some((i + 1) % n),
        (Some(i), VK_UP) => s.menu = Some((i + n - 1) % n),
        (Some(i), VK_RETURN) if !ctrl && !alt => {
            act(s, ACTS[menu_acts(s)[i]].0);
            return true;
        }
        (Some(_), VK_ESCAPE | VK_LEFT) => s.menu = None,
        (_, VK_RIGHT) if to as usize == text.len() => s.menu = Some(0),
        (_, 0x4F /* O */) if ctrl => s.menu = Some(0),
        (_, VK_DOWN) => s.sel = (s.sel + 1).min(s.hits.len().saturating_sub(1)),
        (_, VK_UP) => s.sel = s.sel.saturating_sub(1),
        (_, VK_ESCAPE) => {
            hide(s);
            return true;
        }
        (_, 0x50 /* P */) if alt => {
            set_pane(s, !s.pv_on);
            return true;
        }
        (_, 0xBC /* comma */) if ctrl => {
            crate::prefs::open();
            hide(s);
            return true;
        }
        (_, VK_DELETE) if query(s).trim().is_empty() => {
            // forget a recent item, or all of them with Shift
            if let Some(h) = s.hits.get(s.sel) {
                crate::forget((!shift).then_some(h.path.as_str()));
                run(s, false);
            }
            return true;
        }
        (_, 0x41 /* A */) if ctrl => {
            SendMessageW(s.edit, EM_SETSEL, 0, -1);
            return true;
        }
        (_, VK_BACK) if ctrl => {
            if from == to {
                SendMessageW(s.edit, EM_SETSEL, word_start(&text, to as usize), to as isize);
            }
            SendMessageW(s.edit, EM_REPLACESEL, 1, wide("").as_ptr() as isize);
            return true;
        }
        _ => match key_act(vk, ctrl, alt, shift, from != to) {
            Some(a) => {
                act(s, a);
                return true;
            }
            None => return false,
        },
    }
    render(s);
    true
}

unsafe fn paint(s: &mut Bar) {
    let (theme, w, hist, top, view) = (s.theme, width(s), history(s), list_top(s), list_view(s));
    let wanted: Vec<(String, String)> = s.hits.iter().map(|h| (h.icon.clone(), h.path.clone())).collect();
    for (key, path) in wanted {
        if !s.icons.contains_key(&key) {
            s.icons.insert(key.clone(), None);
            let _ = s.icon_jobs.send((key, path));
        }
    }
    let Some(c) = s.canvas.as_mut() else { return };
    c.begin(theme.bg);
    c.fill((0.0, INPUT_H - 1.0, w, 1.0), theme.line, 0.0);
    if hist {
        c.text(&t("Recently opened", "Aperti di recente").to_uppercase(), 12.0, true, (18.0, INPUT_H + 8.0, LIST_W - 36.0, 16.0), theme.dim);
    }
    c.clip((0.0, top, LIST_W, HEIGHT - FOOT_H - top));
    let (x, row_w) = (6.0, LIST_W - 12.0);
    for (i, h) in s.hits.iter().enumerate() {
        let y = top + 6.0 + i as f32 * ROW_H - s.top;
        if y + ROW_H < top || y > HEIGHT - FOOT_H {
            continue;
        }
        if i == s.sel {
            c.fill((x, y, row_w, ROW_H), theme.sel, 6.0);
        }
        if let Some(Some((normal, grey))) = s.icons.get(&h.icon) {
            let (bmp, opacity) = if hist { (grey, 0.65) } else { (normal, 1.0) };
            c.image(bmp, (x + 12.0, y + 12.0, 24.0, 24.0), opacity);
        }
        let mut right = x + row_w - 12.0;
        if h.size > 0 {
            right -= c.text_right(&fmt_size(h.size), 12.0, (right - 90.0, y + 16.0, 90.0, 16.0), theme.dim) + 10.0;
        }
        if h.pinned {
            right -= c.text_right("📌", 12.0, (right - 24.0, y + 16.0, 24.0, 16.0), theme.dim) + 10.0;
        }
        let (tx, tw) = (x + 46.0, right - x - 46.0);
        c.text_marked(&h.name, 15.0, (tx, y + 6.0, tw, 20.0), if hist { theme.dim } else { theme.fg }, &marks(&h.name, &h.hl), theme.hl);
        let path = if h.app { t("Application", "Applicazione").to_string() } else if h.peer.is_empty() { h.path.clone() } else { format!("🖥 {} · {}", h.peer, h.path) };
        c.text(&path, 12.0, false, (tx, y + 26.0, tw, 16.0), theme.dim);
    }
    let content = s.hits.len() as f32 * ROW_H;
    if content > view {
        c.fill((LIST_W - 5.0, top + 6.0 + s.top / content * view, 3.0, view * view / content), theme.line, 1.5); // ponytail: shows the position, is not dragged; the wheel and the keys scroll
    }
    c.unclip();

    c.fill((0.0, HEIGHT - FOOT_H, w, 1.0), theme.line, 0.0);
    let ver = c.text_right(&s.version, 12.0, (w - 118.0, HEIGHT - 25.0, 100.0, 16.0), theme.dim);
    let hint = if hist {
        t("Recent · ↑↓ select · Enter open · Del remove · Shift+Del clear all · Esc close", "Recenti · ↑↓ seleziona · Invio apri · Canc rimuovi · Maiusc+Canc svuota tutto · Esc chiudi")
    } else {
        t("↑↓ select · Enter open · → or Ctrl+O actions · Alt+P preview · Esc close", "↑↓ seleziona · Invio apri · → o Ctrl+O azioni · Alt+P anteprima · Esc chiudi")
    };
    c.text(&format!("{} · {hint}", s.status), 12.0, false, (18.0, HEIGHT - 25.0, w - 36.0 - ver - 12.0, 16.0), theme.dim);

    if s.pv_on {
        c.fill((LIST_W, INPUT_H, 1.0, HEIGHT - INPUT_H - FOOT_H), theme.line, 0.0);
        if let Some((bmp, iw, ih)) = &s.pv_image {
            let (bw, bh) = (PANE_W - 24.0, HEIGHT - INPUT_H - FOOT_H - 24.0);
            let k = (bw / iw).min(bh / ih).min(1.0);
            let (dw, dh) = (iw * k, ih * k);
            c.image(bmp, (LIST_W + 12.0 + (bw - dw) / 2.0, INPUT_H + 12.0 + (bh - dh) / 2.0, dw, dh), 1.0);
        }
    }

    if let Some(active) = s.menu {
        let (mx, my, mw, mh) = {
            let hits = &s.hits;
            let acts = match hits.get(s.sel) { Some(h) if !h.peer.is_empty() => 1, Some(h) if h.app => 2, _ => ACTS.len() };
            let h = acts as f32 * MENU_ROW + 8.0;
            let row = top + 6.0 + s.sel as f32 * ROW_H - s.top;
            (LIST_W - 18.0 - MENU_W, if row + ROW_H + h <= HEIGHT { row + ROW_H } else { (row - h).max(INPUT_H) }, MENU_W, h)
        };
        c.fill((mx, my, mw, mh), theme.line, 8.0);
        c.fill((mx + 1.0, my + 1.0, mw - 2.0, mh - 2.0), theme.bg, 7.0);
        let it = crate::italian();
        let list: &[usize] = match s.hits.get(s.sel) { Some(h) if !h.peer.is_empty() => &[4], Some(h) if h.app => &[0, 8], _ => &[0, 1, 2, 3, 4, 5, 6, 7, 8] };
        let pinned = s.hits.get(s.sel).is_some_and(|h| h.pinned);
        for (i, &a) in list.iter().enumerate() {
            let y = my + 4.0 + i as f32 * MENU_ROW;
            if i == active {
                c.fill((mx + 4.0, y, mw - 8.0, MENU_ROW), theme.sel, 6.0);
            }
            let (_, en, itl, ken, kit) = ACTS[a];
            let label = if ACTS[a].0 == "pin" && pinned { t("Unpin", "Togli dalla cima") } else if it { itl } else { en };
            c.text_right(if it { kit } else { ken }, 12.0, (mx + 16.0, y + 8.0, mw - 32.0, 16.0), theme.dim);
            c.text(label, 15.0, false, (mx + 16.0, y + 6.0, mw - 32.0, 20.0), theme.fg);
        }
    }

    if !c.end() {
        // the drawing surface was lost: make it and everything on it again
        s.icons.clear();
        (s.pv_path, s.pv_image) = (None, None);
        resize(s);
        repaint(s);
    }
}

unsafe fn resize(s: &mut Bar) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(s.hwnd, &mut r);
    let dpi = s.scale * 96.0;
    if let Some(c) = s.canvas.as_mut() {
        c.resize((r.right - r.left) as u32, (r.bottom - r.top) as u32, dpi);
    }
    let px = |v: f32| (v * s.scale) as i32;
    SetWindowPos(s.edit, null_mut(), px(18.0), px(16.0), px(width(s) - 36.0), px(28.0), SWP_NOZORDER | SWP_NOACTIVATE);
    SetWindowPos(s.pv_edit, null_mut(), px(LIST_W + 12.0), px(INPUT_H + 12.0), px(PANE_W - 18.0), px(HEIGHT - INPUT_H - FOOT_H - 24.0), SWP_NOZORDER | SWP_NOACTIVATE);
}

unsafe fn fonts(s: &mut Bar) {
    let make = |px: f32, name: &str| CreateFontW(-((px * s.scale) as i32), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(name).as_ptr());
    let (font, mono) = (make(20.0, "Segoe UI"), make(12.0, "Consolas"));
    DeleteObject(s.font as HGDIOBJ);
    DeleteObject(s.mono as HGDIOBJ);
    (s.font, s.mono) = (font, mono);
    SendMessageW(s.edit, WM_SETFONT, font as usize, 1);
    SendMessageW(s.pv_edit, WM_SETFONT, mono as usize, 1);
}

/// The menu row or list row under a point, in device-independent pixels.
fn menu_at(s: &Bar, x: f32, y: f32) -> Option<Option<usize>> {
    s.menu?;
    let (mx, my, mw, mh) = menu_rect(s);
    let inside = x >= mx && x < mx + mw && y >= my + 4.0 && y < my + mh - 4.0;
    Some(inside.then(|| (((y - my - 4.0) / MENU_ROW) as usize).min(menu_acts(s).len() - 1)))
}

fn row_at(s: &Bar, x: f32, y: f32) -> Option<usize> {
    let top = list_top(s) + 6.0;
    if x >= LIST_W || y < top || y >= HEIGHT - FOOT_H - 6.0 {
        return None;
    }
    let i = ((y - top + s.top) / ROW_H) as usize;
    (i < s.hits.len()).then_some(i)
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if BAR.load(SeqCst).is_null() {
        return DefWindowProcW(h, msg, w, l);
    }
    if let Some(r) = tray::message(h, msg, w, l) {
        return r;
    }
    let s = me();
    let at = || ((l & 0xffff) as i16 as f32 / s.scale, (l >> 16 & 0xffff) as i16 as f32 / s.scale);
    match msg {
        WM_RESULT => {
            let (seq, hits): (u64, Vec<Hit>) = recv(l);
            if seq == s.seq {
                s.local = Some(hits);
                let stay = s.stay;
                draw(s, stay);
            }
        }
        WM_REMOTE => {
            let (seq, q, got): (u64, String, Vec<Hit>) = recv(l);
            if seq == s.seq {
                (s.remote, s.remote_q) = (got.clone(), Some(q));
                if !got.is_empty() || !s.more.is_empty() {
                    s.more = got;
                    if s.local.is_some() {
                        draw(s, true);
                    }
                }
            }
        }
        WM_ICON => {
            let (key, px): (String, Option<icons::Pixels>) = recv(l);
            let made = px.and_then(|(iw, ih, mut bgra)| {
                let c = s.canvas.as_ref()?;
                let normal = c.bitmap(iw, ih, &bgra)?;
                for p in bgra.chunks_exact_mut(4) {
                    let g = (p[0] as u32 * 114 + p[1] as u32 * 587 + p[2] as u32 * 299) / 1000;
                    p[..3].fill(g as u8);
                }
                Some((normal, c.bitmap(iw, ih, &bgra)?))
            });
            if made.is_some() {
                s.icons.insert(key, made);
                repaint(s);
            }
        }
        WM_PREVIEW => {
            let (path, text, image): (String, Option<String>, Option<icons::Pixels>) = recv(l);
            if s.pv_path.as_ref() == Some(&path) {
                ShowWindow(s.pv_edit, SW_HIDE);
                s.pv_image = None;
                if let Some(text) = text {
                    gfx::set_text(s.pv_edit, &text.replace("\r\n", "\n").replace('\n', "\r\n"));
                    ShowWindow(s.pv_edit, SW_SHOWNA);
                } else if let Some((iw, ih, px)) = image {
                    s.pv_image = s.canvas.as_ref().and_then(|c| c.bitmap(iw, ih, &px)).map(|b| (b, iw as f32, ih as f32));
                }
                repaint(s);
            }
        }
        WM_STATUS => {
            let (busy, n, up): (bool, usize, bool) = recv(l);
            s.status = if !up {
                t("Index service not running (tray menu → Install index service)", "Servizio di indicizzazione non attivo (menu icona → Installa servizio di indicizzazione)").to_string()
            } else if busy {
                t("Indexing…", "Indicizzazione…").to_string()
            } else if crate::italian() {
                format!("{} elementi", fmt_count(n, '.'))
            } else {
                format!("{} items", fmt_count(n, ','))
            };
            repaint(s);
            if IsWindowVisible(h) != 0 {
                if busy || !up {
                    SetTimer(h, T_POLL, 1000, None);
                } else {
                    KillTimer(h, T_POLL);
                }
                if busy {
                    run(s, true);
                }
            }
        }
        WM_SHOW => show(s),
        WM_TIMER => match w {
            T_POLL => poll(),
            T_PREVIEW => {
                KillTimer(h, T_PREVIEW);
                if let (Some(path), Some(hit)) = (s.pv_path.clone(), s.hits.get(s.sel)) {
                    load_preview(path, hit.is_dir);
                }
            }
            T_PENDING => {
                KillTimer(h, T_PENDING);
                let more = typesearch::take();
                if !more.is_empty() {
                    let (at, _) = (gfx::text_of(s.edit).encode_utf16().count(), 0);
                    SendMessageW(s.edit, EM_SETSEL, at, at as isize);
                    SendMessageW(s.edit, EM_REPLACESEL, 0, wide(&more).as_ptr() as isize);
                }
            }
            _ => {}
        },
        WM_COMMAND => {
            let code = (w >> 16) as u32;
            if l == s.edit as isize && code == EN_CHANGE {
                s.menu = None;
                run(s, false);
            } else if l == s.pv_edit as isize && code == EN_SETFOCUS {
                SetFocus(s.edit); // the keys stay with the search
            }
        }
        WM_ACTIVATE => {
            if w & 0xffff == 0 {
                if FOCUSED.swap(false, SeqCst) {
                    hide(s);
                }
            } else {
                FOCUSED.store(true, SeqCst);
                SetFocus(s.edit);
            }
        }
        WM_SETFOCUS => {
            SetFocus(s.edit);
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
            SetTextColor(w as _, gfx::colorref(s.theme.fg));
            SetBkColor(w as _, gfx::colorref(s.theme.bg));
            return s.brush as LRESULT;
        }
        WM_LBUTTONDOWN => {
            let (x, y) = at();
            match menu_at(s, x, y) {
                Some(Some(i)) => {
                    let a = ACTS[menu_acts(s)[i]].0;
                    act(s, a);
                }
                Some(None) => {
                    s.menu = None;
                    repaint(s);
                }
                None => {
                    if let Some(i) = row_at(s, x, y) {
                        go(s, i, false);
                    }
                }
            }
        }
        WM_RBUTTONUP => {
            let (x, y) = at();
            if let Some(i) = row_at(s, x, y) {
                (s.sel, s.menu) = (i, Some(0));
                render(s);
            }
        }
        WM_MOUSEMOVE => {
            let (x, y) = at();
            if let Some(Some(i)) = menu_at(s, x, y) {
                if s.menu != Some(i) {
                    s.menu = Some(i);
                    repaint(s);
                }
            }
        }
        WM_MOUSEWHEEL => {
            let delta = (w >> 16) as i16 as f32 / 120.0;
            s.top = (s.top - delta * ROW_H).clamp(0.0, (s.hits.len() as f32 * ROW_H - list_view(s)).max(0.0));
            s.menu = None;
            repaint(s);
        }
        WM_SIZE => resize(s),
        WM_DPICHANGED => {
            s.scale = (w & 0xffff) as f32 / 96.0;
            fonts(s);
            s.icons.clear();
            place(s);
            resize(s);
        }
        WM_SETTINGCHANGE => {
            if gfx::look_changed(l) {
                refresh();
            }
        }
        WM_ERASEBKGND => return 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(h, &mut ps);
            paint(s);
            EndPaint(h, &ps);
        }
        _ => return DefWindowProcW(h, msg, w, l),
    }
    0
}

/// Make the window, hidden, and the threads that work for it. Call once, before anything posts to it.
pub fn create() {
    unsafe {
        let class = gfx::register("lisdir_bar", wndproc, CS_DROPSHADOW);
        let module = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null());
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class.as_ptr(), wide("lisdir").as_ptr(), WS_POPUP | WS_CLIPCHILDREN, 0, 0, 700, 440, null_mut(), null_mut(), module, null());
        HANDLE.store(hwnd, SeqCst);
        let round = 2i32; // DWMWCP_ROUND
        DwmSetWindowAttribute(hwnd, 33 /* DWMWA_WINDOW_CORNER_PREFERENCE */, (&round as *const i32).cast(), 4);
        let edit_class = wide("EDIT");
        let child = |style: u32| CreateWindowExW(0, edit_class.as_ptr(), null(), WS_CHILD | style, 0, 0, 0, 0, hwnd, null_mut(), module, null());
        let edit = child(WS_VISIBLE | ES_AUTOHSCROLL as u32);
        let pv_edit = child(WS_VSCROLL | ES_MULTILINE as u32 | ES_READONLY as u32 | ES_AUTOVSCROLL as u32);

        let (searches, asked) = channel::<(u64, String)>();
        std::thread::spawn(move || {
            while let Ok(mut job) = asked.recv() {
                while let Ok(newer) = asked.try_recv() {
                    job = newer; // only the latest text matters
                }
                send(WM_RESULT, (job.0, crate::search(&job.1)));
            }
        });
        let (icon_jobs, wanted) = channel::<(String, String)>();
        std::thread::spawn(move || {
            while let Ok((key, path)) = wanted.recv() {
                send(WM_ICON, (key.clone(), icons::icon_image(&key, &path)));
            }
        });

        let settings = crate::app().settings.read().unwrap().clone();
        let bar = Box::new(Bar {
            hwnd, edit, pv_edit,
            canvas: Canvas::new(hwnd),
            theme: gfx::theme(&settings.theme),
            brush: CreateSolidBrush(0),
            font: null_mut(), mono: null_mut(),
            scale: GetDpiForWindow(hwnd) as f32 / 96.0,
            hits: vec![], sel: 0, top: 0.0, seq: 0, stay: false, local: None, more: vec![], remote: vec![], remote_q: None, menu: None,
            pv_on: settings.preview, pv_path: None, pv_image: None,
            icons: HashMap::new(),
            status: String::new(),
            version: format!("v{}", crate::VERSION),
            searches, icon_jobs,
        });
        BAR.store(Box::into_raw(bar), SeqCst);
        let s = me();
        fonts(s);
        refresh();
        place(s);
        resize(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_like_explorer() {
        assert_eq!(fmt_size(512), "512 B");
        assert_eq!(fmt_size(1536), "1.5 KB");
        assert_eq!(fmt_size(20 * 1024), "20 KB");
        assert_eq!(fmt_size(3 << 30), "3.0 GB");
    }

    #[test]
    fn counts_have_separators() {
        assert_eq!(fmt_count(7, ','), "7");
        assert_eq!(fmt_count(1234, '.'), "1.234");
        assert_eq!(fmt_count(1234567, ','), "1,234,567");
    }

    #[test]
    fn highlights_become_utf16_runs() {
        assert_eq!(marks("abcd", &[1, 2]), vec![(1, 2)]);
        assert_eq!(marks("😀ab", &[1, 2]), vec![(2, 2)]); // the emoji is two units, one position
        assert_eq!(marks("a😀b", &[0, 1, 2]), vec![(0, 4)]);
        assert_eq!(marks("abc", &[0, 2]), vec![(0, 1), (2, 1)]);
    }

    #[test]
    fn ctrl_backspace_takes_a_word() {
        let t: Vec<u16> = "foo bar  ".encode_utf16().collect();
        assert_eq!(word_start(&t, 9), 4);
        assert_eq!(word_start(&t, 7), 4);
        assert_eq!(word_start(&t, 3), 0);
        assert_eq!(word_start(&t, 0), 0);
    }

    #[test]
    fn keys_map_to_actions() {
        assert_eq!(key_act(VK_RETURN, false, false, false, false), Some("open"));
        assert_eq!(key_act(VK_RETURN, true, false, false, false), Some("folder"));
        assert_eq!(key_act(VK_RETURN, false, true, false, false), Some("properties"));
        assert_eq!(key_act(b'C' as u16, true, false, false, false), Some("copy"));
        assert_eq!(key_act(b'C' as u16, true, false, true, false), Some("path"));
        assert_eq!(key_act(b'C' as u16, true, false, false, true), None); // edits the selected text
        assert_eq!(key_act(b'T' as u16, true, false, false, true), Some("terminal"));
        assert_eq!(key_act(b'C' as u16, false, false, false, false), None);
    }
}
