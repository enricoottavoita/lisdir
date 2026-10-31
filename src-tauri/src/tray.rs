//! The notification-area icon with its menu, the global shortcut, and the end of the program. All of it is delivered to
//! the bar's window, which exists for as long as the program runs, so `bar::wndproc` passes those messages here first.
use crate::{bar, engine::wide, service, settings::{hotkey_keys, Settings}, t, typesearch, VERSION};
use std::{
    mem::{size_of, zeroed},
    ptr::null,
    sync::{atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::SeqCst}, Mutex},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT},
        Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW},
        WindowsAndMessaging::*,
    },
};

const WM_TRAY: u32 = WM_APP + 20;
const WM_EXIT: u32 = WM_APP + 21;

/// The text of the update item and whether it can be clicked, once the update check has said something.
static UPDATE: Mutex<Option<(String, bool)>> = Mutex::new(None);
static SHOWN: AtomicBool = AtomicBool::new(false);
/// The id of the registered shortcut. It alternates, so a new shortcut is in place before the old one goes.
static HOTKEY: AtomicI32 = AtomicI32::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

fn data(h: HWND) -> NOTIFYICONDATAW {
    unsafe {
        let mut n: NOTIFYICONDATAW = zeroed();
        n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = h;
        n.uID = 1;
        n
    }
}

/// Show or hide the icon.
pub fn set_visible(on: bool) {
    unsafe {
        let mut n = data(bar::hwnd());
        if !on {
            if SHOWN.swap(false, SeqCst) {
                Shell_NotifyIconW(NIM_DELETE, &n);
            }
            return;
        }
        if SHOWN.swap(true, SeqCst) {
            return;
        }
        n.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        n.uCallbackMessage = WM_TRAY;
        n.hIcon = LoadImageW(GetModuleHandleW(null()), 1 as *const u16, IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), 0);
        let tip: Vec<u16> = format!("lisdir {VERSION}").encode_utf16().take(n.szTip.len() - 1).collect();
        n.szTip[..tip.len()].copy_from_slice(&tip);
        Shell_NotifyIconW(NIM_ADD, &n);
    }
}

/// Put a shortcut in place of the current one. A shortcut taken by another program leaves the current one working.
pub fn set_hotkey(hotkey: &str) -> Result<(), &'static str> {
    let (mods, vk) = hotkey_keys(hotkey).ok_or("this key cannot be used")?;
    let old = HOTKEY.load(SeqCst);
    let new = old % 2 + 1;
    unsafe {
        if RegisterHotKey(bar::hwnd(), new, mods | MOD_NOREPEAT, vk) == 0 {
            return Err("taken by another program");
        }
        if old != 0 {
            UnregisterHotKey(bar::hwnd(), old);
        }
    }
    HOTKEY.store(new, SeqCst);
    Ok(())
}

/// The update item's text; `install_update` and `check_update` call this from their thread.
pub fn set_update(text: String, enabled: bool) {
    *UPDATE.lock().unwrap() = Some((text, enabled));
}

/// End the program from any thread.
pub fn quit() {
    unsafe { PostMessageW(bar::hwnd(), WM_EXIT, 0, 0) };
}

pub fn start(s: &Settings) {
    // the saved shortcut, or Alt+Space when it is taken by another program
    if set_hotkey(&s.hotkey).is_err() {
        let _ = set_hotkey("alt+space");
    }
    TASKBAR_CREATED.store(unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) }, SeqCst);
    set_visible(!s.hide_tray);
}

unsafe fn menu(h: HWND) {
    let (open, type_search, autostart, settings, roots, update, install, exit) = (1, 2, 3, 4, 5, 6, 7, 8);
    let m = CreatePopupMenu();
    let add = |id: usize, text: &str, flags: u32| AppendMenuW(m, MF_STRING | flags, id, wide(text).as_ptr());
    let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };
    add(0, &format!("lisdir {VERSION}"), MF_GRAYED);
    add(open, t("Open (Alt+Space)", "Apri (Alt+Spazio)"), 0);
    add(type_search, t("Type to search in Explorer/Desktop", "Digita per cercare in Esplora risorse/Desktop"), check(typesearch::ENABLED.load(SeqCst)));
    add(autostart, t("Start with Windows", "Avvia con Windows"), check(crate::autostart()));
    add(settings, t("Settings…", "Impostazioni…"), 0);
    add(roots, t("Network folders…", "Cartelle di rete…"), 0);
    let state = UPDATE.lock().unwrap().clone();
    let (text, enabled) = state.unwrap_or((t("Check for updates…", "Controlla aggiornamenti…").to_string(), true));
    add(update, &text, if enabled { 0 } else { MF_GRAYED });
    add(install, t("Install index service (admin)…", "Installa servizio di indicizzazione (admin)…"), 0);
    add(exit, t("Quit", "Esci"), 0);
    let mut at: POINT = zeroed();
    GetCursorPos(&mut at);
    SetForegroundWindow(h); // without this the menu stays open after a click elsewhere
    let picked = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, at.x, at.y, 0, h, null()) as usize;
    DestroyMenu(m);
    match picked {
        p if p == open => bar::toggle(),
        p if p == type_search => {
            let on = !typesearch::ENABLED.load(SeqCst);
            typesearch::ENABLED.store(on, SeqCst);
            let mut s = crate::app().settings.write().unwrap();
            s.type_search = on;
            let _ = s.save();
        }
        p if p == autostart => crate::set_autostart(!crate::autostart()),
        p if p == settings => crate::prefs::open(),
        p if p == roots => crate::edit_roots(),
        p if p == update => { std::thread::spawn(crate::update_clicked); }
        p if p == install => service::request_install(),
        p if p == exit => quit(),
        _ => {}
    }
}

/// The messages that are about the icon, the shortcut or quitting; None for the rest.
pub unsafe fn message(h: HWND, msg: u32, _w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_HOTKEY => bar::toggle(),
        WM_TRAY => match l as u32 {
            WM_LBUTTONUP => bar::toggle(),
            WM_RBUTTONUP => menu(h),
            _ => {}
        },
        WM_EXIT => {
            Shell_NotifyIconW(NIM_DELETE, &data(h));
            std::process::exit(0);
        }
        m if m != 0 && m == TASKBAR_CREATED.load(SeqCst) => {
            // Explorer restarted, and the icon went with it
            if SHOWN.swap(false, SeqCst) {
                set_visible(true);
            }
        }
        _ => return None,
    }
    Some(0)
}
