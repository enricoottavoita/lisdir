//! System-wide "type to search": a low-level keyboard hook that, when an Explorer folder view or the
//! desktop file list has the focus, swallows plain letter/digit keys and hands them to the search bar.
//! ponytail: no "nothing selected" check (needs UI Automation); focus class alone decides.
use std::{
    mem::{size_of, zeroed},
    ptr::null_mut,
    sync::{atomic::{AtomicBool, Ordering::SeqCst}, Mutex, OnceLock},
};
use tauri::AppHandle;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, GetKeyState, GetKeyboardLayout, ToUnicodeEx, VK_CAPITAL, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT},
        WindowsAndMessaging::{
            CallNextHookEx, GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetMessageW, GetWindowThreadProcessId,
            SetWindowsHookExW, GUITHREADINFO, KBDLLHOOKSTRUCT, WH_KEYBOARD_LL, WM_KEYDOWN,
        },
    },
};

pub static ENABLED: AtomicBool = AtomicBool::new(true);
static PENDING: Mutex<String> = Mutex::new(String::new());
static SHOWING: AtomicBool = AtomicBool::new(false);
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Keys swallowed since the bar was asked to open; the UI claims them once it has focus.
pub fn take() -> String {
    std::mem::take(&mut *PENDING.lock().unwrap())
}

fn class_of(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, b.as_mut_ptr(), 64) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

/// The foreground thread id if a plain key press would land in an Explorer file list or on the desktop.
/// The focus class excludes rename boxes, the address bar and the search box (all "Edit"-like).
fn in_file_view() -> Option<u32> {
    unsafe {
        let fg = GetForegroundWindow();
        if !matches!(class_of(fg).as_str(), "CabinetWClass" | "Progman" | "WorkerW") {
            return None;
        }
        let tid = GetWindowThreadProcessId(fg, null_mut());
        let mut gi: GUITHREADINFO = zeroed();
        gi.cbSize = size_of::<GUITHREADINFO>() as u32;
        if GetGUIThreadInfo(tid, &mut gi) == 0 {
            return None;
        }
        matches!(class_of(gi.hwndFocus).as_str(), "DirectUIHWND" | "SysListView32").then_some(tid)
    }
}

fn typed_char(k: &KBDLLHOOKSTRUCT) -> Option<char> {
    let vk = k.vkCode;
    let alnum = (0x30..=0x39).contains(&vk) || (0x41..=0x5A).contains(&vk);
    if !alnum || k.flags & 0x10 != 0 {
        return None; // not a letter/digit, or injected by another program (AutoHotkey, remote desktop...)
    }
    unsafe {
        if [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN].iter().any(|&m| GetAsyncKeyState(m as i32) < 0) {
            return None; // a shortcut, not typing
        }
        let tid = in_file_view()?;
        // The hook thread has no key state of its own: rebuild just what layout translation needs.
        let mut st = [0u8; 256];
        if GetAsyncKeyState(VK_SHIFT as i32) < 0 { st[VK_SHIFT as usize] = 0x80; }
        if GetKeyState(VK_CAPITAL as i32) & 1 != 0 { st[VK_CAPITAL as usize] = 1; }
        let mut out = [0u16; 4];
        // flag 4: don't disturb pending dead-key state
        let n = ToUnicodeEx(vk, k.scanCode, st.as_ptr(), out.as_mut_ptr(), 4, 4, GetKeyboardLayout(tid));
        let c = char::from_u32(out[0] as u32)?;
        (n == 1 && c.is_alphanumeric()).then_some(c)
    }
}

unsafe extern "system" fn hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && w as u32 == WM_KEYDOWN && ENABLED.load(SeqCst) {
        if let Some(c) = typed_char(&*(l as *const KBDLLHOOKSTRUCT)) {
            PENDING.lock().unwrap().push(c);
            // Window calls wait on the main thread; never block the hook (Windows drops slow hooks), so show from a thread.
            if !SHOWING.swap(true, SeqCst) {
                if let Some(app) = APP.get().cloned() {
                    std::thread::spawn(move || {
                        crate::show(&app);
                        SHOWING.store(false, SeqCst);
                    });
                }
            }
            return 1; // swallow: Explorer must not also start its own type-ahead
        }
    }
    CallNextHookEx(null_mut(), code, w, l)
}

pub fn start(app: AppHandle) {
    let _ = APP.set(app);
    std::thread::spawn(|| unsafe {
        if SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), null_mut(), 0).is_null() {
            return;
        }
        let mut m = zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {} // a hook only fires while its thread pumps messages
    });
}
