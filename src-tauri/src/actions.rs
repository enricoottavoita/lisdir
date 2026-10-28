//! The action menu on a result: copy / cut as files (pasteable in Explorer), copy path, open with, terminal here,
//! properties.
use std::ffi::c_void;
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::GlobalFree;
use windows_sys::Win32::System::DataExchange::*;
use windows_sys::Win32::System::Memory::*;
use windows_sys::Win32::UI::Shell::*;

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn bytes(w: &[u16]) -> Vec<u8> {
    w.iter().flat_map(|c| c.to_le_bytes()).collect()
}

/// A DROPFILES block, what Explorer puts on the clipboard: the header, each path NUL-terminated, one more NUL.
fn drop_files(paths: &[&str]) -> Vec<u8> {
    let mut b: Vec<u8> = [20u32, 0, 0, 0, 1].iter().flat_map(|v| v.to_le_bytes()).collect(); // pFiles, pt, fNC, fWide
    for p in paths {
        b.extend(bytes(&wide(p)));
    }
    b.extend([0, 0]);
    b
}

/// Movable global memory holding `data`, as the clipboard and HDROP want it.
fn global(data: &[u8]) -> *mut c_void {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, data.len());
        if !h.is_null() {
            std::ptr::copy_nonoverlapping(data.as_ptr(), GlobalLock(h) as *mut u8, data.len());
            GlobalUnlock(h);
        }
        h
    }
}

/// Replace the clipboard with these (format, data) pairs. False if another app kept it busy.
fn set_clipboard(items: &[(u32, Vec<u8>)]) -> bool {
    unsafe {
        // another app may hold the clipboard for a moment
        if !(0..20).any(|_| OpenClipboard(null_mut()) != 0 || { std::thread::sleep(std::time::Duration::from_millis(10)); false }) {
            return false;
        }
        EmptyClipboard();
        let mut ok = true;
        for (fmt, data) in items {
            let h = global(data);
            if SetClipboardData(*fmt, h).is_null() {
                GlobalFree(h);
                ok = false;
            }
        }
        CloseClipboard();
        ok
    }
}

/// Copy or cut the file the way Explorer does, so Ctrl+V in a folder pastes (or moves) it.
pub fn copy_file(path: &str, cut: bool) -> bool {
    let effect = unsafe { RegisterClipboardFormatW(wide("Preferred DropEffect").as_ptr()) };
    let drop_effect: u32 = if cut { 2 } else { 1 }; // DROPEFFECT_MOVE / DROPEFFECT_COPY
    set_clipboard(&[(CF_HDROP, drop_files(&[path])), (effect, drop_effect.to_le_bytes().to_vec())])
}

pub fn copy_text(s: &str) -> bool {
    set_clipboard(&[(CF_UNICODETEXT, bytes(&wide(s)))])
}

/// Run a shell verb on the file: "openas" (the Open with dialog) or "properties".
pub fn verb(verb: &str, path: &str) {
    let (v, f) = (wide(verb), wide(path));
    let mut i = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST, // verbs that live on the context menu, such as properties
        lpVerb: v.as_ptr(),
        lpFile: f.as_ptr(),
        nShow: 1, // SW_SHOWNORMAL
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut i) };
}

/// Windows Terminal in the folder of the item (the item itself if it is a folder), else a plain console.
pub fn terminal(path: &str, is_dir: bool) {
    let p = Path::new(path);
    let dir = if is_dir { p } else { p.parent().unwrap_or(p) };
    // wt splits its command line at ';', so a folder named "x; cmd /c ..." would run that: escape it as wt expects
    let d = dir.to_string_lossy().replace(';', "\\;");
    if std::process::Command::new("wt").arg("-d").arg(d).spawn().is_err() {
        let _ = std::process::Command::new("cmd").current_dir(dir).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_files_reads_back_as_an_hdrop() {
        let paths = [r"C:\Users\x\report 2026.pdf", r"\\nas\share\città.txt"];
        let h = global(&drop_files(&paths));
        unsafe {
            assert_eq!(DragQueryFileW(h, u32::MAX, null_mut(), 0), 2);
            for (n, p) in paths.iter().enumerate() {
                let mut buf = [0u16; 260];
                let len = DragQueryFileW(h, n as u32, buf.as_mut_ptr(), buf.len() as u32) as usize;
                assert_eq!(String::from_utf16_lossy(&buf[..len]), *p);
            }
            GlobalFree(h);
        }
    }

    /// Replaces your clipboard, so run by hand: cargo test clipboard -- --ignored
    #[test]
    #[ignore]
    fn clipboard_holds_the_file() {
        let exe = std::env::current_exe().unwrap();
        assert!(copy_file(exe.to_str().unwrap(), false));
        let ps = |c: &str| String::from_utf8(std::process::Command::new("powershell").args(["-NoProfile", "-Command", c]).output().unwrap().stdout).unwrap();
        assert_eq!(ps("(Get-Clipboard -Format FileDropList).FullName").trim(), exe.to_str().unwrap());
        assert!(copy_text("C:\\città"));
        assert_eq!(ps("[Console]::OutputEncoding = [Text.Encoding]::UTF8; Get-Clipboard").trim(), "C:\\città");
    }
}
