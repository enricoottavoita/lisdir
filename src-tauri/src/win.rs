//! The search window: a normal window that stays open, with the results in columns that sort when you click their
//! titles. It is made when you open it and gone when you close it, so it costs nothing while it is not on screen.
//! Like the bar, it runs on the main thread and gets slow work back as posted messages carrying a boxed value.
use crate::{bar, engine::wide, gfx::{self, Canvas, Theme}, icons, peer, query::Clock, service::Hit, t};
use std::{
    collections::{HashMap, HashSet},
    ptr::{null, null_mut},
    sync::{atomic::{AtomicPtr, AtomicU64, Ordering::SeqCst}, mpsc::{channel, Sender}},
    time::Duration,
};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::{BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, GetMonitorInfoW, InvalidateRect, MonitorFromWindow, ScreenToClient, SetBkColor, SetTextColor, HBRUSH, HFONT, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, PAINTSTRUCT},
    },
    UI::{
        Controls::{EM_GETSEL, EM_REPLACESEL, EM_SETSEL, WM_MOUSELEAVE},
        HiDpi::{GetDpiForSystem, GetDpiForWindow},
        Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_ESCAPE, VK_MENU, VK_NEXT, VK_PRIOR, VK_SHIFT, VK_UP, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT},
        WindowsAndMessaging::*,
    },
};

/// How many of the best matches the window asks for. The columns sort these, not the whole disk.
const LIMIT: usize = 1000;

// Sizes in device-independent pixels.
const INPUT_H: f32 = 56.0;
const HEAD_H: f32 = 28.0;
const ROW_H: f32 = 30.0;
const FOOT_H: f32 = 28.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const SIZE_W: f32 = 84.0;
const TIME_W: f32 = 132.0;
/// the two buttons at the top right (reduce, close), and the width of the edge that resizes the window
const BTN_W: f32 = 44.0;
const BTN_H: f32 = 36.0;
const BORDER: f32 = 6.0;

const WM_RESULT: u32 = WM_APP + 1;
const WM_REMOTE: u32 = WM_APP + 2;
const WM_ICON: u32 = WM_APP + 3;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Col {
    Name,
    Path,
    Size,
    Time,
}

struct Win {
    hwnd: HWND,
    edit: HWND,
    canvas: Option<Canvas>,
    theme: Theme,
    brush: HBRUSH,
    font: HFONT,
    /// pixels per device-independent pixel
    scale: f32,
    /// this PC's answer and the other PCs' answer, in the order of relevance
    local: Vec<Hit>,
    more: Vec<Hit>,
    /// the rows as shown: both answers, sorted by the clicked column
    rows: Vec<Hit>,
    sel: usize,
    /// the results picked with Shift or Ctrl (by path, so a sort does not lose them); empty = just the selected row
    marked: HashSet<String>,
    /// where a Shift range starts
    anchor: usize,
    /// a button pressed on a row, not yet a click or a drag: (x, y, row)
    down: Option<(f32, f32, usize)>,
    /// how far the list is scrolled
    top: f32,
    seq: u64,
    /// column and direction (true = A to Z, small to large); None = by relevance
    sort: Option<(Col, bool)>,
    icons: HashMap<String, Option<ID2D1Bitmap>>,
    /// the button under the pointer: 0 reduce, 1 close
    hover: Option<usize>,
    tracking: bool,
    searches: Sender<(u64, String)>,
    icon_jobs: Sender<(String, String)>,
}

static WIN: AtomicPtr<Win> = AtomicPtr::new(null_mut());
/// the search in progress, for the thread that asks the other PCs
static SEQ: AtomicU64 = AtomicU64::new(0);

fn cur() -> Option<&'static mut Win> {
    // ponytail: one mutable global used only on the main thread, like the bar's
    unsafe { WIN.load(SeqCst).as_mut() }
}

/// Hand a value to the window from a worker thread. If the window is gone by now, the value is dropped.
fn post<T: Send>(hwnd: usize, msg: u32, value: T) {
    let p = Box::into_raw(Box::new(value));
    if unsafe { PostMessageW(hwnd as HWND, msg, 0, p as isize) } == 0 {
        drop(unsafe { Box::from_raw(p) });
    }
}

unsafe fn recv<T>(l: LPARAM) -> T {
    *Box::from_raw(l as *mut T)
}

// ---------------------------------------------------------------- sorting

/// What a click on a column title does: its natural direction, then the other one, then back to relevance.
fn next_sort(now: Option<(Col, bool)>, col: Col) -> Option<(Col, bool)> {
    let natural = matches!(col, Col::Name | Col::Path); // names A to Z, but the biggest and the newest first
    match now {
        Some((c, up)) if c == col => (up == natural).then_some((col, !natural)),
        _ => Some((col, natural)),
    }
}

/// The folder a result is in, for the Path column.
fn folder_of(h: &Hit) -> String {
    if h.app {
        return t("Application", "Applicazione").to_string();
    }
    let dir = match h.path.rsplit_once('\\') {
        Some((d, _)) if d.ends_with(':') => format!("{d}\\"),
        Some((d, _)) => d.to_string(),
        None => h.path.clone(),
    };
    if h.peer.is_empty() { dir } else { format!("🖥 {} · {dir}", h.peer) }
}

fn sorted(found: &[Hit], sort: Option<(Col, bool)>) -> Vec<Hit> {
    let mut rows = found.to_vec();
    let Some((col, up)) = sort else { return rows };
    match col {
        Col::Name => rows.sort_by_cached_key(|h| h.name.to_lowercase()),
        Col::Path => rows.sort_by_cached_key(|h| folder_of(h).to_lowercase()),
        Col::Size => rows.sort_by_key(|h| h.size),
        Col::Time => rows.sort_by_key(|h| h.time),
    }
    if !up {
        rows.reverse();
    }
    rows
}

// ---------------------------------------------------------------- layout

/// The columns for a window this wide: (name x, name width, path x, path width, size x, time x).
fn columns(w: f32) -> [f32; 6] {
    let free = (w - 2.0 * PAD - SIZE_W - TIME_W - 3.0 * GAP).max(120.0);
    let name_w = free * 0.4;
    let path_x = PAD + name_w + GAP;
    let size_x = w - PAD - TIME_W - GAP - SIZE_W;
    [PAD, name_w, path_x, size_x - GAP - path_x, size_x, w - PAD - TIME_W]
}

fn client(s: &Win) -> (f32, f32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(s.hwnd, &mut r) };
    ((r.right - r.left) as f32 / s.scale, (r.bottom - r.top) as f32 / s.scale)
}

fn list_top() -> f32 {
    INPUT_H + HEAD_H
}

fn list_view(s: &Win) -> f32 {
    (client(s).1 - list_top() - FOOT_H).max(ROW_H)
}

fn clamp_top(s: &mut Win) {
    let max = (s.rows.len() as f32 * ROW_H - list_view(s)).max(0.0);
    s.top = s.top.clamp(0.0, max);
}

fn scroll_to_selection(s: &mut Win) {
    let (y, view) = (s.sel as f32 * ROW_H, list_view(s));
    if y < s.top {
        s.top = y;
    } else if y + ROW_H > s.top + view {
        s.top = y + ROW_H - view;
    }
    clamp_top(s);
}

unsafe fn repaint(s: &Win) {
    InvalidateRect(s.hwnd, null(), 0);
}

/// The button at a point of the top right corner.
fn button_at(s: &Win, x: f32, y: f32) -> Option<usize> {
    let w = client(s).0;
    (y < BTN_H && x >= w - 2.0 * BTN_W && x < w).then(|| if x < w - BTN_W { 0 } else { 1 })
}

/// The column title at a point, or the row.
fn hit_test(s: &Win, x: f32, y: f32) -> (Option<Col>, Option<usize>) {
    if y >= INPUT_H && y < list_top() {
        let c = columns(client(s).0);
        let col = if x >= c[5] {
            Col::Time
        } else if x >= c[4] {
            Col::Size
        } else if x >= c[2] {
            Col::Path
        } else {
            Col::Name
        };
        return (Some(col), None);
    }
    if y < list_top() || y >= client(s).1 - FOOT_H {
        return (None, None);
    }
    let i = ((y - list_top() + s.top) / ROW_H) as usize;
    (None, (i < s.rows.len()).then_some(i))
}

// ---------------------------------------------------------------- picking several

/// The paths of the rows from `a` to `b`, both included, whichever comes first.
fn range_paths(rows: &[Hit], a: usize, b: usize) -> HashSet<String> {
    rows.iter().skip(a.min(b)).take(a.abs_diff(b) + 1).map(|h| h.path.clone()).collect()
}

/// Pick the rows from the anchor to `to` (Shift+click, Shift+arrows) and move the selection there.
fn pick_range(s: &mut Win, to: usize) {
    s.marked = range_paths(&s.rows, s.anchor, to);
    s.sel = to;
}

/// Back to a single selected row.
fn pick_one(s: &mut Win, i: usize) {
    s.marked.clear();
    (s.sel, s.anchor) = (i, i);
}

/// Ctrl+click: add the row to the picked ones, or take it out.
fn pick_toggle(s: &mut Win, i: usize) {
    if s.marked.is_empty() {
        if let Some(h) = s.rows.get(s.sel) {
            s.marked.insert(h.path.clone());
        }
    }
    let path = s.rows[i].path.clone();
    if !s.marked.remove(&path) {
        s.marked.insert(path);
    }
    (s.sel, s.anchor) = (i, i);
}

/// What an action works on: the picked rows in the order shown, or the selected row.
fn targets(s: &Win) -> Vec<Hit> {
    if s.marked.is_empty() {
        return s.rows.get(s.sel).cloned().into_iter().collect();
    }
    s.rows.iter().filter(|h| s.marked.contains(&h.path)).cloned().collect()
}

// ---------------------------------------------------------------- state changes

/// Show `local` and `more` in the order the columns ask for; the selection stays on the same result when it can.
unsafe fn rebuild(s: &mut Win, keep: bool) {
    let cur = s.rows.get(s.sel).map(|h| h.path.clone());
    let mut found = s.local.clone();
    found.extend(s.more.iter().cloned());
    s.rows = sorted(&found, s.sort);
    s.sel = cur.filter(|_| keep).and_then(|c| s.rows.iter().position(|h| h.path == c)).unwrap_or(0);
    if !keep {
        s.top = 0.0;
        s.marked.clear();
        s.anchor = 0;
    }
    scroll_to_selection(s);
    repaint(s);
}

unsafe fn run(s: &mut Win) {
    s.seq += 1;
    SEQ.store(s.seq, SeqCst);
    let (seq, q) = (s.seq, gfx::text_of(s.edit));
    s.more.clear();
    if q.trim().is_empty() {
        s.local.clear();
        return rebuild(s, false);
    }
    if peer::running() {
        let (text, h) = (q.clone(), s.hwnd as usize);
        peer::rt().spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            if SEQ.load(SeqCst) == seq {
                let got = crate::remote_search(text.clone()).await;
                post(h, WM_REMOTE, (seq, got));
            }
        });
    }
    let _ = s.searches.send((seq, q));
}

/// Take the files that are gone (deleted, or moved by a drop) out of the list.
unsafe fn forget_missing(s: &mut Win) {
    let gone = |h: &Hit| h.peer.is_empty() && !h.app && !std::path::Path::new(&h.path).exists();
    s.local.retain(|h| !gone(h));
    s.marked.clear();
    rebuild(s, true);
}

/// More than this many files and Enter does nothing, so a "pick all" cannot open a thousand windows.
const OPEN_MAX: usize = 10;

unsafe fn act(s: &mut Win, action: &str) {
    let Some(cur) = s.rows.get(s.sel).cloned() else { return };
    let all = targets(s);
    // files on this PC; the other PCs' results and the applications cannot be copied, cut or deleted
    let local: Vec<&str> = all.iter().filter(|h| h.peer.is_empty() && !h.app).map(|h| h.path.as_str()).collect();
    match action {
        "path" => {
            crate::actions::copy_text(&all.iter().map(|h| h.path.as_str()).collect::<Vec<_>>().join("\r\n"));
        }
        "copy" | "cut" if !local.is_empty() => {
            crate::actions::copy_files(&local, action == "cut");
        }
        "delete" => {
            if !local.is_empty() && crate::actions::recycle(&local, s.hwnd) {
                forget_missing(s);
            }
        }
        "open" if all.iter().any(|h| h.peer.is_empty()) => {
            if all.len() <= OPEN_MAX {
                all.iter().filter(|h| h.peer.is_empty()).for_each(|h| crate::open(h, false));
            }
        }
        "open" | "copy" | "cut" if !cur.peer.is_empty() => crate::act("path", &cur), // not on this PC: only its path can be copied
        "folder" if cur.peer.is_empty() && !cur.app => crate::open(&cur, true),
        "open" | "folder" | "copy" | "cut" => {}
        _ if !cur.app && cur.peer.is_empty() => crate::act(action, &cur),
        _ => {}
    }
}

/// A key pressed in the text box, before the box sees it. True when it was ours.
pub unsafe fn key(m: &MSG) -> bool {
    let Some(s) = cur() else { return false };
    if m.hwnd != s.edit || !(m.message == WM_KEYDOWN || m.message == WM_SYSKEYDOWN) {
        return false;
    }
    let vk = m.wParam as u16;
    let down = |k: u16| GetKeyState(k as i32) < 0;
    let (ctrl, alt, shift) = (down(VK_CONTROL), down(VK_MENU), down(VK_SHIFT));
    let (mut from, mut to) = (0u32, 0u32);
    SendMessageW(s.edit, EM_GETSEL, &mut from as *mut u32 as usize, &mut to as *mut u32 as isize);
    let page = ((list_view(s) / ROW_H) as usize).max(1);
    let last = s.rows.len().saturating_sub(1);
    match vk {
        VK_DOWN | VK_UP | VK_NEXT | VK_PRIOR => {
            s.sel = match vk {
                VK_DOWN => (s.sel + 1).min(last),
                VK_UP => s.sel.saturating_sub(1),
                VK_NEXT => (s.sel + page).min(last),
                _ => s.sel.saturating_sub(page),
            };
            if shift {
                pick_range(s, s.sel);
            } else {
                pick_one(s, s.sel);
            }
        }
        0x41 /* A */ if ctrl && shift => {
            s.marked = s.rows.iter().map(|h| h.path.clone()).collect();
            repaint(s);
            return true;
        }
        VK_DELETE if ctrl => {
            act(s, "delete");
            return true;
        }
        VK_ESCAPE => {
            DestroyWindow(s.hwnd);
            return true;
        }
        0x41 /* A */ if ctrl => {
            SendMessageW(s.edit, EM_SETSEL, 0, -1);
            return true;
        }
        VK_BACK if ctrl => {
            let text: Vec<u16> = gfx::text_of(s.edit).encode_utf16().collect();
            if from == to {
                SendMessageW(s.edit, EM_SETSEL, bar::word_start(&text, to as usize), to as isize);
            }
            SendMessageW(s.edit, EM_REPLACESEL, 1, wide("").as_ptr() as isize);
            return true;
        }
        _ => match bar::key_act(vk, ctrl, alt, shift, from != to) {
            Some(a) => {
                act(s, a);
                return true;
            }
            None => return false,
        },
    }
    scroll_to_selection(s);
    repaint(s);
    true
}

// ---------------------------------------------------------------- drawing

unsafe fn paint(s: &mut Win) {
    let (theme, (w, h)) = (s.theme, client(s));
    let c = columns(w);
    // icons of the rows in view; a type is read once, whatever the number of files
    let first = (s.top / ROW_H) as usize;
    let shown = ((list_view(s) / ROW_H) as usize + 2).min(s.rows.len().saturating_sub(first));
    let wanted: Vec<(String, String)> = s.rows.iter().skip(first).take(shown).map(|h| (h.icon.clone(), h.path.clone())).collect();
    for (key, path) in wanted {
        if !s.icons.contains_key(&key) {
            s.icons.insert(key.clone(), None);
            let _ = s.icon_jobs.send((key, path));
        }
    }
    let (n, it) = (fmt(s.rows.len()), crate::italian());
    let picked = s.marked.len();
    let mut status = if gfx::text_of(s.edit).trim().is_empty() {
        t("Type to search", "Scrivi per cercare").to_string()
    } else if s.rows.len() >= LIMIT {
        if it { format!("I migliori {n} risultati") } else { format!("The best {n} results") }
    } else if it {
        format!("{n} risultati")
    } else {
        format!("{n} results")
    };
    if picked > 1 {
        status += &if it { format!(" · {picked} selezionati") } else { format!(" · {picked} selected") };
    }
    let clock = Clock::system();
    let Some(cv) = s.canvas.as_mut() else { return };
    cv.begin(theme.bg);
    cv.fill((0.0, INPUT_H - 1.0, w, 1.0), theme.line, 0.0);
    if s.hover == Some(0) {
        cv.fill((w - 2.0 * BTN_W, 0.0, BTN_W, BTN_H), theme.sel, 0.0);
    } else if s.hover == Some(1) {
        cv.fill((w - BTN_W, 0.0, BTN_W, BTN_H), 0xc42b1c, 0.0);
    }
    cv.fill((w - 2.0 * BTN_W + 16.0, 18.0, 12.0, 1.0), theme.fg, 0.0); // reduce: a line
    cv.text("✕", 12.0, false, (w - BTN_W + 15.0, 9.0, 18.0, 18.0), if s.hover == Some(1) { 0xffffff } else { theme.fg });

    // column titles; the sorted one is bold and has an arrow
    let titles = [
        (Col::Name, c[0], c[1], t("Name", "Nome"), false),
        (Col::Path, c[2], c[3], t("Folder", "Cartella"), false),
        (Col::Size, c[4], SIZE_W, t("Size", "Dimensione"), true),
        (Col::Time, c[5], TIME_W, t("Modified", "Modificato"), false),
    ];
    for (col, x, cw, title, right) in titles {
        let on = s.sort.filter(|&(k, _)| k == col);
        let label = match on {
            Some((_, up)) => format!("{title} {}", if up { "▲" } else { "▼" }),
            None => title.to_string(),
        };
        let rect = (x, INPUT_H + 5.0, cw, 18.0);
        let rgb = if on.is_some() { theme.fg } else { theme.dim };
        if right {
            cv.text_right(&label, 12.0, rect, rgb);
        } else {
            cv.text(&label, 12.0, on.is_some(), rect, rgb);
        }
    }
    cv.fill((0.0, list_top() - 1.0, w, 1.0), theme.line, 0.0);

    cv.clip((0.0, list_top(), w, h - FOOT_H - list_top()));
    for i in first..(first + shown) {
        let hit = &s.rows[i];
        let y = list_top() + i as f32 * ROW_H - s.top;
        if if s.marked.is_empty() { i == s.sel } else { s.marked.contains(&hit.path) } {
            cv.fill((6.0, y, w - 12.0, ROW_H), theme.sel, 6.0);
        }
        if let Some(Some(bmp)) = s.icons.get(&hit.icon) {
            cv.image(bmp, (c[0], y + 6.0, 18.0, 18.0), 1.0);
        }
        let name = (c[0] + 26.0, y + 5.0, c[1] - 26.0, 20.0);
        cv.text_marked(&hit.name, 14.0, name, theme.fg, &bar::marks(&hit.name, &hit.hl), theme.hl);
        cv.text(&folder_of(hit), 12.0, false, (c[2], y + 7.0, c[3], 16.0), theme.dim);
        if hit.size > 0 {
            cv.text_right(&bar::fmt_size(hit.size), 12.0, (c[4], y + 7.0, SIZE_W, 16.0), theme.dim);
        }
        cv.text(&clock.stamp(hit.time), 12.0, false, (c[5], y + 7.0, TIME_W, 16.0), theme.dim);
    }
    let content = s.rows.len() as f32 * ROW_H;
    let view = h - FOOT_H - list_top();
    if content > view {
        cv.fill((w - 6.0, list_top() + s.top / content * view, 3.0, (view * view / content).max(16.0)), theme.line, 1.5); // ponytail: shows the position, is not dragged; the wheel and the keys scroll
    }
    cv.unclip();

    cv.fill((0.0, h - FOOT_H, w, 1.0), theme.line, 0.0);
    // a thin edge, so the window can be told from what is behind it
    for r in [(0.0, 0.0, w, 1.0), (0.0, h - 1.0, w, 1.0), (0.0, 0.0, 1.0, h), (w - 1.0, 0.0, 1.0, h)] {
        cv.fill(r, theme.line, 0.0);
    }
    let hint = t("Shift/Ctrl+click several · Enter open · Ctrl+C copy · Ctrl+Del delete · drag out · Esc close", "Maiusc/Ctrl+clic più file · Invio apri · Ctrl+C copia · Ctrl+Canc elimina · trascina fuori · Esc chiudi");
    cv.text(&format!("{status} · {hint}"), 12.0, false, (PAD, h - FOOT_H + 6.0, w - 2.0 * PAD, 16.0), theme.dim);

    if !cv.end() {
        // the drawing surface was lost: make it and everything on it again
        s.icons.clear();
        resize(s);
        repaint(s);
    }
}

/// 1234 -> "1,234" or "1.234".
fn fmt(n: usize) -> String {
    let sep = if crate::italian() { '.' } else { ',' };
    bar::fmt_count(n, sep)
}

unsafe fn resize(s: &mut Win) {
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(s.hwnd, &mut r);
    if let Some(c) = s.canvas.as_mut() {
        c.resize((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32, s.scale * 96.0);
    }
    let px = |v: f32| (v * s.scale) as i32;
    SetWindowPos(s.edit, null_mut(), px(PAD), px(16.0), (r.right - r.left - 2 * px(PAD) - 2 * px(BTN_W)).max(1), px(28.0), SWP_NOZORDER | SWP_NOACTIVATE);
    clamp_top(s);
}

unsafe fn fonts(s: &mut Win) {
    let font = CreateFontW(-((20.0 * s.scale) as i32), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide("Segoe UI").as_ptr());
    DeleteObject(s.font as HGDIOBJ);
    s.font = font;
    SendMessageW(s.edit, WM_SETFONT, font as usize, 1);
}

/// Language and theme again, after a change in the settings or in Windows.
pub fn refresh() {
    let Some(s) = cur() else { return };
    unsafe {
        s.theme = gfx::theme(&crate::app().settings.read().unwrap().theme);
        DeleteObject(s.brush as HGDIOBJ);
        s.brush = CreateSolidBrush(gfx::colorref(s.theme.bg));
        let round = 2i32; // DWMWCP_ROUND
        DwmSetWindowAttribute(s.hwnd, 33 /* DWMWA_WINDOW_CORNER_PREFERENCE */, (&round as *const i32).cast(), 4);
        SendMessageW(s.edit, 0x1501 /* EM_SETCUEBANNER */, 1, wide(t("Search files and folders…", "Cerca file e cartelle…")).as_ptr() as isize);
        repaint(s);
    }
}

/// Drag the picked files (or the pressed row) out of the window, into Explorer or any program that takes files.
unsafe fn drag(s: &mut Win, i: usize) {
    if !s.marked.contains(&s.rows[i].path) {
        pick_one(s, i);
    }
    let all = targets(s);
    let paths: Vec<&str> = all.iter().filter(|h| h.peer.is_empty() && !h.app).map(|h| h.path.as_str()).collect();
    if !paths.is_empty() {
        crate::actions::drag(&paths);
        forget_missing(s); // a drop that moves leaves the files elsewhere
    }
}

// ---------------------------------------------------------------- the window

unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCALCSIZE {
        // no frame: the window is all client. This comes first of all, also while the window is being made: Windows asks
        // once at the start and, if the size does not change later, never again. Maximised, Windows puts the frame
        // outside the screen: take it back.
        if w != 0 && IsZoomed(h) != 0 {
            let p = &mut *(l as *mut NCCALCSIZE_PARAMS);
            let pad = GetSystemMetrics(SM_CXPADDEDBORDER);
            let (fx, fy) = (GetSystemMetrics(SM_CXSIZEFRAME) + pad, GetSystemMetrics(SM_CYSIZEFRAME) + pad);
            p.rgrc[0] = RECT { left: p.rgrc[0].left + fx, top: p.rgrc[0].top + fy, right: p.rgrc[0].right - fx, bottom: p.rgrc[0].bottom - fy };
        }
        return 0;
    }
    let Some(s) = cur() else { return DefWindowProcW(h, msg, w, l) };
    if s.hwnd.is_null() {
        return DefWindowProcW(h, msg, w, l); // still being made
    }
    let at = || ((l & 0xffff) as i16 as f32 / s.scale, (l >> 16 & 0xffff) as i16 as f32 / s.scale);
    match msg {
        WM_RESULT => {
            let (seq, hits): (u64, Vec<Hit>) = recv(l);
            if seq == s.seq {
                s.local = hits;
                rebuild(s, false);
            }
        }
        WM_REMOTE => {
            let (seq, got): (u64, Vec<Hit>) = recv(l);
            if seq == s.seq && !got.is_empty() {
                s.more = got;
                rebuild(s, true);
            }
        }
        WM_ICON => {
            let (key, px): (String, Option<icons::Pixels>) = recv(l);
            let made = px.and_then(|(iw, ih, bgra)| s.canvas.as_ref()?.bitmap(iw, ih, &bgra));
            if made.is_some() {
                s.icons.insert(key, made);
                repaint(s);
            }
        }
        WM_COMMAND => {
            if l == s.edit as isize && (w >> 16) as u32 == EN_CHANGE {
                run(s);
            }
        }
        WM_ACTIVATE | WM_SETFOCUS => {
            if msg == WM_SETFOCUS || w & 0xffff != 0 {
                SetFocus(s.edit);
            }
        }
        WM_CTLCOLOREDIT => {
            SetTextColor(w as _, gfx::colorref(s.theme.fg));
            SetBkColor(w as _, gfx::colorref(s.theme.bg));
            return s.brush as LRESULT;
        }
        WM_NCHITTEST => {
            let mut p = POINT { x: (l & 0xffff) as i16 as i32, y: (l >> 16 & 0xffff) as i16 as i32 };
            ScreenToClient(h, &mut p);
            let (x, y, (cw, ch)) = (p.x as f32 / s.scale, p.y as f32 / s.scale, client(s));
            if IsZoomed(h) == 0 {
                let (west, east, north, south) = (x < BORDER, x >= cw - BORDER, y < BORDER, y >= ch - BORDER);
                let edge = match (west, east, north, south) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, ..) => HTLEFT,
                    (_, true, ..) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => 0,
                };
                if edge != 0 {
                    return edge as LRESULT;
                }
            }
            // the top row, beside the text box and the buttons, moves the window
            let hit = if y < INPUT_H && button_at(s, x, y).is_none() { HTCAPTION } else { HTCLIENT };
            return hit as LRESULT;
        }
        WM_MOUSEMOVE => {
            let (x, y) = at();
            let hover = button_at(s, x, y);
            if hover != s.hover {
                s.hover = hover;
                repaint(s);
            }
            if let Some((x0, y0, i)) = s.down.filter(|_| w & 1 != 0) {
                if (x - x0).hypot(y - y0) > 5.0 {
                    s.down = None;
                    drag(s, i);
                    return 0;
                }
            }
            if !s.tracking {
                let mut e = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut e);
                s.tracking = true;
            }
        }
        WM_MOUSELEAVE => {
            (s.hover, s.tracking) = (None, false);
            repaint(s);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_RBUTTONUP => {
            let (x, y) = at();
            if msg == WM_LBUTTONDOWN {
                match button_at(s, x, y) {
                    Some(0) => {
                        ShowWindow(h, SW_MINIMIZE);
                        return 0;
                    }
                    Some(_) => {
                        DestroyWindow(h);
                        return 0;
                    }
                    None => {}
                }
            }
            match hit_test(s, x, y) {
                (Some(col), _) if msg == WM_LBUTTONDOWN => {
                    s.sort = next_sort(s.sort, col);
                    rebuild(s, true);
                }
                (_, Some(i)) => {
                    let (ctrl, shift) = (GetKeyState(VK_CONTROL as i32) < 0, GetKeyState(VK_SHIFT as i32) < 0);
                    if msg == WM_LBUTTONDBLCLK {
                        s.sel = i;
                        act(s, "open");
                    } else if shift {
                        pick_range(s, i);
                    } else if ctrl {
                        pick_toggle(s, i);
                    } else if s.marked.len() > 1 && s.marked.contains(&s.rows[i].path) {
                        // maybe the start of a drag of the whole pick: decided when the button goes up
                        s.sel = i;
                        s.down = Some((x, y, i));
                    } else {
                        pick_one(s, i);
                        s.down = (msg == WM_LBUTTONDOWN).then_some((x, y, i));
                    }
                    repaint(s);
                }
                _ => {}
            }
            SetFocus(s.edit);
        }
        WM_LBUTTONUP => {
            // a click on one of several picked rows, without a drag: that row alone
            if let Some((_, _, i)) = s.down.take() {
                pick_one(s, i);
                repaint(s);
            }
        }
        WM_MOUSEWHEEL => {
            let delta = (w >> 16) as i16 as f32 / 120.0;
            s.top -= delta * ROW_H * 3.0;
            clamp_top(s);
            repaint(s);
        }
        WM_GETMINMAXINFO => {
            let m = &mut *(l as *mut MINMAXINFO);
            m.ptMinTrackSize.x = (560.0 * s.scale) as i32;
            m.ptMinTrackSize.y = (320.0 * s.scale) as i32;
        }
        WM_SIZE => {
            resize(s);
            repaint(s);
        }
        WM_DPICHANGED => {
            s.scale = (w & 0xffff) as f32 / 96.0;
            fonts(s);
            s.icons.clear();
            let r = &*(l as *const RECT);
            SetWindowPos(h, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
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
        WM_CLOSE => {
            DestroyWindow(h);
        }
        WM_DESTROY => {
            // the threads end when their channels are dropped with the state
            let p = WIN.swap(null_mut(), SeqCst);
            let b = Box::from_raw(p);
            DeleteObject(b.font as HGDIOBJ);
            DeleteObject(b.brush as HGDIOBJ);
            drop(b);
            SEQ.store(0, SeqCst);
        }
        _ => return DefWindowProcW(h, msg, w, l),
    }
    0
}

/// Open the window with this search, or bring it forward if it is already open.
pub fn open(query: &str) {
    unsafe {
        if let Some(s) = cur() {
            if IsIconic(s.hwnd) != 0 {
                ShowWindow(s.hwnd, SW_RESTORE);
            }
            if !query.is_empty() {
                gfx::set_text(s.edit, query);
            }
            bar::foreground(s.hwnd); // Windows refuses a plain SetForegroundWindow when another program had the keyboard
            SetFocus(s.edit);
            return;
        }
        let class = gfx::register("lisdir_window", wndproc, CS_DBLCLKS | CS_DROPSHADOW);
        let module = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(null());
        let sys = GetDpiForSystem() as f32 / 96.0;
        // the state exists before the window, so the first messages find it; the handles are filled in just after
        let (searches, asked) = channel::<(u64, String)>();
        let (icon_jobs, wanted) = channel::<(String, String)>();
        let settings = crate::app().settings.read().unwrap().clone();
        let state = Box::new(Win {
            hwnd: null_mut(), edit: null_mut(),
            canvas: None,
            theme: gfx::theme(&settings.theme),
            brush: CreateSolidBrush(0),
            font: null_mut(),
            scale: sys,
            local: vec![], more: vec![], rows: vec![], sel: 0, marked: HashSet::new(), anchor: 0, down: None, top: 0.0, seq: 0, sort: None,
            icons: HashMap::new(),
            hover: None, tracking: false,
            searches, icon_jobs,
        });
        WIN.store(Box::into_raw(state), SeqCst);
        // no title bar or frame, like the bar; WS_THICKFRAME is kept so Windows still resizes and snaps it (see WM_NCCALCSIZE)
        let hwnd = CreateWindowExW(WS_EX_APPWINDOW, class.as_ptr(), wide("lisdir").as_ptr(), WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_CLIPCHILDREN, CW_USEDEFAULT, CW_USEDEFAULT, (1000.0 * sys) as i32, (620.0 * sys) as i32, null_mut(), null_mut(), module, null());
        let s = cur().unwrap();
        if hwnd.is_null() {
            drop(Box::from_raw(WIN.swap(null_mut(), SeqCst)));
            return;
        }
        s.hwnd = hwnd;
        SetWindowPos(hwnd, null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED); // the frame is asked again
        s.scale = GetDpiForWindow(hwnd) as f32 / 96.0;
        s.edit = CreateWindowExW(0, wide("EDIT").as_ptr(), null(), WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32, 0, 0, 0, 0, hwnd, null_mut(), module, null());
        s.canvas = Canvas::new(hwnd);
        let handle = hwnd as usize;
        std::thread::spawn(move || {
            while let Ok(mut job) = asked.recv() {
                while let Ok(newer) = asked.try_recv() {
                    job = newer; // only the latest text matters
                }
                post(handle, WM_RESULT, (job.0, crate::search_n(&job.1, LIMIT)));
            }
        });
        std::thread::spawn(move || {
            while let Ok((key, path)) = wanted.recv() {
                post(handle, WM_ICON, (key.clone(), icons::icon_image(&key, &path)));
            }
        });
        fonts(s);
        refresh();
        // centred on the screen, as large as the design asks for unless the screen is smaller
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY), &mut mi);
        let r = mi.rcWork;
        let (w, h) = (((1000.0 * s.scale) as i32).min(r.right - r.left), ((620.0 * s.scale) as i32).min(r.bottom - r.top));
        SetWindowPos(hwnd, null_mut(), r.left + (r.right - r.left - w) / 2, r.top + (r.bottom - r.top - h) / 2, w, h, SWP_NOZORDER);
        resize(s);
        gfx::set_text(s.edit, query);
        ShowWindow(hwnd, SW_SHOW);
        bar::foreground(hwnd);
        SetFocus(s.edit);
        if query.is_empty() {
            repaint(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, path: &str, size: u64, time: i64) -> Hit {
        Hit::new(name.into(), path.into(), false, 1.0).with_size(size).with_time(time)
    }

    #[test]
    fn a_click_cycles_through_the_directions_and_back_to_relevance() {
        let first = next_sort(None, Col::Name);
        assert_eq!(first, Some((Col::Name, true))); // A to Z
        let second = next_sort(first, Col::Name);
        assert_eq!(second, Some((Col::Name, false)));
        assert_eq!(next_sort(second, Col::Name), None);
        assert_eq!(next_sort(None, Col::Size), Some((Col::Size, false))); // the biggest first
        assert_eq!(next_sort(Some((Col::Size, false)), Col::Time), Some((Col::Time, false))); // another column starts over
        assert_eq!(next_sort(Some((Col::Size, true)), Col::Size), None); // the second step of a size, then relevance
    }

    #[test]
    fn rows_sort_by_the_column_and_keep_relevance_without_one() {
        let found = vec![hit("b.txt", r"C:\x\b.txt", 30, 200), hit("A.txt", r"C:\y\A.txt", 10, 300), hit("c.txt", r"C:\w\c.txt", 20, 100)];
        let names = |rows: Vec<Hit>| rows.into_iter().map(|h| h.name).collect::<Vec<_>>();
        assert_eq!(names(sorted(&found, None)), ["b.txt", "A.txt", "c.txt"]);
        assert_eq!(names(sorted(&found, Some((Col::Name, true)))), ["A.txt", "b.txt", "c.txt"]); // case does not matter
        assert_eq!(names(sorted(&found, Some((Col::Name, false)))), ["c.txt", "b.txt", "A.txt"]);
        assert_eq!(names(sorted(&found, Some((Col::Size, false)))), ["b.txt", "c.txt", "A.txt"]);
        assert_eq!(names(sorted(&found, Some((Col::Time, false)))), ["A.txt", "b.txt", "c.txt"]);
        assert_eq!(names(sorted(&found, Some((Col::Path, true)))), ["c.txt", "b.txt", "A.txt"]);
    }

    #[test]
    fn the_folder_column_shows_the_folder_not_the_file() {
        assert_eq!(folder_of(&hit("a.txt", r"C:\docs\a.txt", 0, 0)), r"C:\docs");
        assert_eq!(folder_of(&hit("a.txt", r"C:\a.txt", 0, 0)), r"C:\");
        assert_eq!(folder_of(&hit("a.txt", r"\\nas\share\a.txt", 0, 0)), r"\\nas\share");
    }

    #[test]
    fn a_range_runs_from_the_anchor_either_way() {
        let rows: Vec<Hit> = (0..5).map(|i| hit(&format!("f{i}"), &format!(r"C:\f{i}"), 0, 0)).collect();
        let paths = |a, b| { let mut v: Vec<_> = range_paths(&rows, a, b).into_iter().collect(); v.sort(); v };
        assert_eq!(paths(1, 3), [r"C:\f1", r"C:\f2", r"C:\f3"]);
        assert_eq!(paths(3, 1), paths(1, 3));
        assert_eq!(paths(2, 2), [r"C:\f2"]);
    }

    #[test]
    fn the_columns_fill_the_width() {
        let c = columns(1000.0);
        assert_eq!(c[0], PAD);
        assert!(c[2] > c[0] + c[1]); // the path starts after the name
        assert!(c[4] > c[2] + c[3]); // the size after the path
        assert_eq!(c[5] + TIME_W, 1000.0 - PAD);
    }
}
