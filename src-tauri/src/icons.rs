//! File-type icons for the result list: the shell's own icon (what Explorer shows), rendered to PNG.
//! Icons are looked up by *type* (extension) without touching the file, so it is instant even on a slow
//! network share; only programs and shortcuts, whose icon belongs to the individual file, are read by path.
use std::{
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Graphics::Gdi::{CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS},
    Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL},
    System::Com::{CoInitializeEx, CoTaskMemFree},
    UI::{
        Shell::{SHGetFileInfoW, SHParseDisplayName, SHFILEINFOW, SHGFI_ICON, SHGFI_PIDL, SHGFI_USEFILEATTRIBUTES},
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

/// PNG bytes of the 32x32 shell icon for a key from `key()`.
pub fn icon_png(key: &str, path: &str) -> Option<Vec<u8>> {
    // The shell wants COM on the calling thread; worker threads of the async runtime have none by default.
    thread_local! { static COM: () = unsafe { CoInitializeEx(null(), 2 /* COINIT_APARTMENTTHREADED */); }; }
    COM.with(|_| {});
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
    let wide: Vec<u16> = target.encode_utf16().chain([0]).collect();
    unsafe {
        let mut fi: SHFILEINFOW = zeroed();
        // flags 0 for size = SHGFI_LARGEICON (32x32)
        if SHGetFileInfoW(wide.as_ptr(), attrs, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | extra) == 0 || fi.hIcon.is_null() {
            return None;
        }
        let px = to_rgba(fi.hIcon);
        DestroyIcon(fi.hIcon);
        px.map(|(w, h, rgba)| png(w, h, &rgba))
    }
}

/// Icon of a Start menu / Store app, addressed through its `shell:AppsFolder` item.
unsafe fn app_icon(id: &str) -> Option<Vec<u8>> {
    let name: Vec<u16> = format!(r"shell:AppsFolder\{id}").encode_utf16().chain([0]).collect();
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
    let px = to_rgba(fi.hIcon);
    DestroyIcon(fi.hIcon);
    px.map(|(w, h, rgba)| png(w, h, &rgba))
}

/// Read an icon's pixels as RGBA. Old-style icons have no alpha channel; their transparency is in the mask.
unsafe fn to_rgba(icon: HICON) -> Option<(usize, usize, Vec<u8>)> {
    let mut ii: ICONINFO = zeroed();
    if GetIconInfo(icon, &mut ii) == 0 {
        return None;
    }
    let result = (|| {
        if ii.hbmColor.is_null() {
            return None; // monochrome icon: not worth supporting
        }
        let mut bm: BITMAP = zeroed();
        GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast());
        let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
        let dc = CreateCompatibleDC(std::ptr::null_mut());
        let bits = |bitmap| {
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
            let rows = GetDIBits(dc, bitmap, 0, h as u32, buf.as_mut_ptr().cast(), &mut bmi, DIB_RGB_COLORS);
            (rows != 0).then_some(buf)
        };
        let mut px = bits(ii.hbmColor);
        if let Some(p) = px.as_mut() {
            if p.chunks_exact(4).all(|c| c[3] == 0) {
                // no alpha channel: opaque where the mask is black (0), transparent where it is white
                if let Some(mask) = bits(ii.hbmMask) {
                    for (c, m) in p.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                        c[3] = if m[0] == 0 { 255 } else { 0 };
                    }
                }
            }
            for c in p.chunks_exact_mut(4) {
                c.swap(0, 2); // BGRA -> RGBA
            }
        }
        DeleteDC(dc);
        px.map(|p| (w, h, p))
    })();
    DeleteObject(ii.hbmColor);
    DeleteObject(ii.hbmMask);
    result
}

// ---- minimal PNG writer: uncompressed (stored) deflate blocks, good enough for a cached 32x32 icon.

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 == 1 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend(kind);
    out.extend(data);
    let crc = crc32(&out[start..]);
    out.extend(crc.to_be_bytes());
}

fn png(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for row in rgba.chunks_exact(w * 4) {
        raw.push(0); // filter: none
        raw.extend(row);
    }
    let mut z = vec![0x78, 0x01]; // zlib header, no compression
    let blocks = raw.chunks(65535).count();
    for (i, b) in raw.chunks(65535).enumerate() {
        z.push((i + 1 == blocks) as u8);
        z.extend((b.len() as u16).to_le_bytes());
        z.extend((!(b.len() as u16)).to_le_bytes());
        z.extend(b);
    }
    z.extend(adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn png_has_valid_structure() {
        let p = png(1, 1, &[1, 2, 3, 4]);
        assert_eq!(&p[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&p[p.len() - 12..], &[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
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
            let bytes = icon_png(if k == "exe" { p } else { k }, p).unwrap_or_default();
            println!("{k}: {} bytes", bytes.len());
            std::fs::write(dir.join(format!("{name}.png")), bytes).unwrap();
        }
    }
}
