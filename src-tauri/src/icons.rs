//! File-type icons for the result list: the shell's own icon (what Explorer shows), rendered to BMP.
//! Icons are looked up by *type* (extension) without touching the file, so it is instant even on a slow
//! network share; only programs and shortcuts, whose icon belongs to the individual file, are read by path.
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::SIZE,
    Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP},
    Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL},
    System::Com::{CoInitializeEx, CoTaskMemFree},
    UI::{
        Shell::{SHCreateItemFromParsingName, SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_ICON, SHGFI_PIDL, SHGFI_USEFILEATTRIBUTES},
        WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
    },
};

/// Extensions whose icon is stored in the file itself.
const PER_FILE: [&str; 5] = ["exe", "lnk", "ico", "url", "scr"];

/// Cache key for a result's icon: "dir", ".ext", "file" (no extension), or the full path for PER_FILE types.
pub fn key(name: &str, path: &str, is_dir: bool) -> String {
    if is_dir {
        return "dir".into();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            let ext = ext.to_lowercase();
            if PER_FILE.contains(&ext.as_str()) { path.to_string() } else { format!(".{ext}") }
        }
        _ => "file".into(),
    }
}

/// The shell wants COM on the calling thread; worker threads of the async runtime have none by default.
fn com() {
    thread_local! { static COM: () = unsafe { CoInitializeEx(null(), 2 /* COINIT_APARTMENTTHREADED */); }; }
    COM.with(|_| {});
}

/// BMP bytes of the 32x32 shell icon for a key from `key()`.
pub fn icon_image(key: &str, path: &str) -> Option<Vec<u8>> {
    com();
    // One at a time: a result list asks for several icons at once, and the shell's icon lookup is not
    // reliable when hammered from many threads. Each type is rendered once and cached, so this is cheap.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(id) = key.strip_prefix("app:") {
        return unsafe { app_icon(id) };
    }
    let (target, extra, attrs) = match key {
        "dir" => ("folder", SHGFI_USEFILEATTRIBUTES, FILE_ATTRIBUTE_DIRECTORY),
        "file" => ("file", SHGFI_USEFILEATTRIBUTES, FILE_ATTRIBUTE_NORMAL),
        k if k.starts_with('.') => (k, SHGFI_USEFILEATTRIBUTES, FILE_ATTRIBUTE_NORMAL),
        _ => (path, 0, 0),
    };
    let wide = crate::engine::wide(target);
    unsafe {
        let mut fi: SHFILEINFOW = zeroed();
        // flags 0 for size = SHGFI_LARGEICON (32x32)
        if SHGetFileInfoW(wide.as_ptr(), attrs, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | extra) == 0 || fi.hIcon.is_null() {
            return None;
        }
        let px = to_bgra(fi.hIcon);
        DestroyIcon(fi.hIcon);
        px.map(|(w, h, bgra)| to_bmp(w, h, &bgra))
    }
}

/// The shell's picture of a file, at most `px` wide or high: what Explorer shows in Large icons view (a thumbnail
/// for photos, videos, PDFs, and the big icon for the rest).
pub fn thumbnail_image(path: &str, px: i32) -> Option<Vec<u8>> {
    // IShellItemImageFactory, called through its vtable (windows-sys has no COM wrappers)
    #[repr(C)]
    struct Vtbl {
        _query_interface: usize,
        _add_ref: usize,
        release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        get_image: unsafe extern "system" fn(*mut std::ffi::c_void, SIZE, i32, *mut HBITMAP) -> i32,
    }
    const IID: windows_sys::core::GUID = windows_sys::core::GUID::from_u128(0xbcc18b79_ba16_442f_80c4_8a59c30c463b);
    com();
    let wide = crate::engine::wide(path);
    unsafe {
        let mut f: *mut *const Vtbl = null_mut();
        if SHCreateItemFromParsingName(wide.as_ptr(), null_mut(), &IID, (&mut f as *mut *mut *const Vtbl).cast()) != 0 {
            return None;
        }
        let mut bmp: HBITMAP = null_mut();
        let ok = ((**f).get_image)(f.cast(), SIZE { cx: px, cy: px }, 0 /* SIIGBF_RESIZETOFIT */, &mut bmp) == 0;
        ((**f).release)(f.cast());
        if !ok {
            return None;
        }
        let px = bitmap_bgra(bmp);
        DeleteObject(bmp);
        let (w, h, mut p) = px?;
        let opaque = p.chunks_exact(4).all(|c| c[3] == 0); // photos come without alpha
        if opaque {
            p.chunks_exact_mut(4).for_each(|c| c[3] = 255);
        }
        Some(to_bmp(w, h, &p))
    }
}

/// Icon of a Start menu / Store app, addressed through its `shell:AppsFolder` item.
unsafe fn app_icon(id: &str) -> Option<Vec<u8>> {
    let name = crate::engine::wide(&format!(r"shell:AppsFolder\{id}"));
    let mut pidl = null_mut();
    if SHParseDisplayName(name.as_ptr(), null_mut(), &mut pidl, 0, null_mut()) != 0 {
        return None;
    }
    let mut fi: SHFILEINFOW = zeroed();
    let ok = SHGetFileInfoW(pidl as *const u16, 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_PIDL);
    CoTaskMemFree(pidl.cast());
    if ok == 0 || fi.hIcon.is_null() {
        return None;
    }
    let px = to_bgra(fi.hIcon);
    DestroyIcon(fi.hIcon);
    px.map(|(w, h, bgra)| to_bmp(w, h, &bgra))
}

/// Read an icon's pixels as BGRA. Old-style icons have no alpha channel; their transparency is in the mask.
unsafe fn to_bgra(icon: HICON) -> Option<(usize, usize, Vec<u8>)> {
    let mut ii: ICONINFO = zeroed();
    if GetIconInfo(icon, &mut ii) == 0 {
        return None;
    }
    let result = (|| {
        if ii.hbmColor.is_null() {
            return None; // monochrome icon: not worth supporting
        }
        let mut px = bitmap_bgra(ii.hbmColor);
        if let Some((_, _, p)) = px.as_mut() {
            if p.chunks_exact(4).all(|c| c[3] == 0) {
                // no alpha channel: opaque where the mask is black (0), transparent where it is white
                if let Some((_, _, mask)) = bitmap_bgra(ii.hbmMask) {
                    for (c, m) in p.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                        c[3] = if m[0] == 0 { 255 } else { 0 };
                    }
                }
            }
        }
        px
    })();
    DeleteObject(ii.hbmColor);
    DeleteObject(ii.hbmMask);
    result
}

/// A bitmap's pixels as top-down BGRA rows.
unsafe fn bitmap_bgra(bitmap: HBITMAP) -> Option<(usize, usize, Vec<u8>)> {
    let mut bm: BITMAP = zeroed();
    GetObjectW(bitmap, size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast());
    let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
    let mut bmi: BITMAPINFO = zeroed();
    bmi.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w as i32,
        biHeight: -(h as i32), // negative: top-down rows
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..zeroed()
    };
    let mut buf = vec![0u8; w * h * 4];
    let dc = CreateCompatibleDC(null_mut());
    let rows = GetDIBits(dc, bitmap, 0, h as u32, buf.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS);
    DeleteDC(dc);
    (rows != 0 && w > 0).then_some((w, h, buf))
}

// ---- BMP writer. The webview decodes BMP itself, so there is no need for a PNG encoder (and its checksums). It
// ---- has to be the V4 header with channel masks: with the plain 40-byte header the alpha channel is ignored.

/// A top-down 32-bit BMP with alpha from BGRA pixels.
fn to_bmp(w: usize, h: usize, bgra: &[u8]) -> Vec<u8> {
    const HEADERS: usize = 14 + 108; // file header + BITMAPV4HEADER
    let mut out = Vec::with_capacity(HEADERS + bgra.len());
    out.extend(b"BM");
    out.extend(((HEADERS + bgra.len()) as u32).to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((HEADERS as u32).to_le_bytes());
    out.extend(108u32.to_le_bytes()); // header size
    out.extend((w as i32).to_le_bytes());
    out.extend((-(h as i32)).to_le_bytes()); // negative: rows run top to bottom
    out.extend([1, 0, 32, 0]); // planes, bits per pixel
    out.extend(3u32.to_le_bytes()); // BI_BITFIELDS: the masks below say where each channel is
    out.extend((bgra.len() as u32).to_le_bytes());
    out.extend([0u8; 16]); // resolution, colour counts
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        out.extend(mask.to_le_bytes()); // red, green, blue, alpha
    }
    out.extend([0u8; 52]); // colour space, endpoints, gamma
    out.extend(bgra);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnail_of_a_photo_and_a_folder() {
        let photo = std::env::temp_dir().join("lisdir_thumb.bmp");
        // a 2x2 24-bit BMP: the shell renders its thumbnail itself
        let mut bmp = b"BM".to_vec();
        for v in [70u32, 0, 54, 40, 2, 2] { bmp.extend(v.to_le_bytes()); }
        bmp.extend([1, 0, 24, 0]);
        for v in [0u32, 16, 0, 0, 0, 0] { bmp.extend(v.to_le_bytes()); }
        bmp.extend([0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0, 0, 0]);
        std::fs::write(&photo, bmp).unwrap();
        for p in [photo.to_str().unwrap(), r"C:\Windows"] {
            let img = thumbnail_image(p, 256).unwrap_or_else(|| panic!("no thumbnail for {p}"));
            assert_eq!(&img[..2], b"BM");
        }
        assert!(thumbnail_image(r"C:\no\such\file.jpg", 256).is_none());
        let _ = std::fs::remove_file(photo);
    }

    #[test]
    fn bmp_has_a_v4_header_with_alpha_and_top_down_rows() {
        let b = to_bmp(2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!(&b[..2], b"BM");
        assert_eq!(u32_at(2) as usize, b.len()); // file size
        assert_eq!(u32_at(10), 122); // pixels start after the file header and the 108-byte header
        assert_eq!((u32_at(14), u32_at(18) as i32, u32_at(22) as i32), (108, 2, -1)); // size, width, negative height
        assert_eq!((u32_at(30), u32_at(54), u32_at(66)), (3, 0x00FF_0000, 0xFF00_0000)); // bitfields, red and alpha masks
        assert_eq!(&b[122..], &[1, 2, 3, 4, 5, 6, 7, 8]); // pixels untouched
    }

    #[test]
    fn keys() {
        assert_eq!(key("Docs", r"C:\Docs", true), "dir");
        assert_eq!(key("Photo.JPG", r"C:\a\Photo.JPG", false), ".jpg");
        assert_eq!(key("notes", r"C:\a\notes", false), "file");
        assert_eq!(key(".gitignore", r"C:\a\.gitignore", false), "file");
        assert_eq!(key("Setup.EXE", r"C:\a\Setup.EXE", false), r"C:\a\Setup.EXE");
    }

    /// Writes real shell icons to %TEMP%\lisdir_icons for eyeballing: cargo test --release dump_icons -- --ignored
    #[test]
    #[ignore]
    fn dump_icons() {
        let dir = std::env::temp_dir().join("lisdir_icons");
        std::fs::create_dir_all(&dir).unwrap();
        for (k, p) in [("dir", ""), (".txt", ""), (".mp4", ""), (".jpg", ""), (".pdf", ""), (".zip", ""), (".docx", ""), (".mp3", ""), ("file", ""), ("exe", r"C:\Windows\notepad.exe")] {
            let name = k.trim_start_matches('.').replace(['\\', ':'], "_");
            let bytes = icon_image(if k == "exe" { p } else { k }, p).unwrap_or_default();
            println!("{k}: {} bytes", bytes.len());
            std::fs::write(dir.join(format!("{name}.bmp")), bytes).unwrap();
        }
    }
}
