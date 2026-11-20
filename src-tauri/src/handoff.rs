//! Handing a value from a worker thread to a window: boxed, posted as a message, and unboxed by the window's thread.
use windows_sys::Win32::{Foundation::{HWND, LPARAM}, UI::WindowsAndMessaging::PostMessageW};

/// Post `value` to the window `hwnd` as message `msg`. If the window is gone by now, the value is dropped.
pub fn post<T: Send>(hwnd: usize, msg: u32, value: T) {
    let p = Box::into_raw(Box::new(value));
    if unsafe { PostMessageW(hwnd as HWND, msg, 0, p as isize) } == 0 {
        drop(unsafe { Box::from_raw(p) });
    }
}

/// The value a `post` of type `T` brought, from the message's `lparam`. Once per message: it takes the value out.
pub unsafe fn take<T>(l: LPARAM) -> T {
    *Box::from_raw(l as *mut T)
}
