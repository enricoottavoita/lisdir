//! What the windows share: the theme colours, small Win32 helpers, and the drawing of the search bar with Direct2D and
//! DirectWrite. The drawing runs on the processor: a short list needs no graphics card, and no graphics driver in memory.
use crate::engine::wide;
use std::ptr::{null, null_mut};
use windows::{
    core::{Interface, PCWSTR},
    Win32::{
        Foundation::{HWND as ComHwnd, RECT as ComRect},
        Graphics::{
            Direct2D::{Common::*, *},
            DirectWrite::*,
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::HDC as ComHdc,
        },
    },
};
use windows_numerics::Vector2;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::HDC},
    System::{LibraryLoader::GetModuleHandleW, Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD}},
    UI::{
        Controls::SetWindowTheme,
        WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW, LoadCursorW, LoadIconW, RegisterClassW, SetWindowTextW, IDC_ARROW, WNDCLASSW},
    },
};

/// Colours as 0xRRGGBB.
#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub bg: u32,
    pub fg: u32,
    pub dim: u32,
    pub sel: u32,
    pub line: u32,
    pub hl: u32,
}

const DARK: Theme = Theme { bg: 0x1f2024, fg: 0xe8e8ea, dim: 0x8b8d94, sel: 0x34363d, line: 0x2e3036, hl: 0x7aa7ff };
const LIGHT: Theme = Theme { bg: 0xfafafa, fg: 0x1d1d1f, dim: 0x6b6e76, sel: 0xe6e8ee, line: 0xdcdde2, hl: 0x1a56db };
const MIDNIGHT: Theme = Theme { bg: 0x0f1626, fg: 0xdbe4ff, dim: 0x7d8bb0, sel: 0x1c2a4a, line: 0x1b2640, hl: 0x6fb1ff };
const PAPER: Theme = Theme { bg: 0xf6f0e4, fg: 0x2b2418, dim: 0x7d705a, sel: 0xe9dfc8, line: 0xddd2b8, hl: 0xa8480a };

/// Does Windows use its light look for apps?
fn windows_light() -> bool {
    let (key, name) = (wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"), wide("AppsUseLightTheme"));
    let (mut v, mut n) = (0u32, 4u32);
    let read = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&mut v as *mut u32).cast(), &mut n) };
    read == 0 && v != 0
}

/// The colours of a theme name from the settings.
pub fn theme(name: &str) -> Theme {
    match name {
        "dark" => DARK,
        "light" => LIGHT,
        "midnight" => MIDNIGHT,
        "paper" => PAPER,
        _ if windows_light() => LIGHT,
        _ => DARK,
    }
}

impl Theme {
    pub fn is_dark(&self) -> bool {
        self.bg & 0xff < 0x80
    }

    /// Give the scroll bars and check boxes of a window the dark or light look of Windows.
    pub fn style(&self, h: HWND) {
        let name = wide(if self.is_dark() { "DarkMode_Explorer" } else { "Explorer" });
        unsafe { SetWindowTheme(h, name.as_ptr(), null()) };
    }

    /// The same, for the title bar of a top-level window.
    pub fn style_title(&self, h: HWND) {
        let dark = self.is_dark() as i32;
        unsafe { DwmSetWindowAttribute(h, 20 /* DWMWA_USE_IMMERSIVE_DARK_MODE */, (&dark as *const i32).cast(), 4) };
    }
}

/// 0xRRGGBB as the 0x00BBGGRR that GDI wants.
pub fn colorref(rgb: u32) -> u32 {
    (rgb & 0xff) << 16 | (rgb & 0xff00) | (rgb >> 16 & 0xff)
}

/// Does this `WM_SETTINGCHANGE` say that Windows changed between its light and dark look?
pub unsafe fn look_changed(l: LPARAM) -> bool {
    l != 0 && PCWSTR(l as *const u16).to_string().is_ok_and(|s| s == "ImmersiveColorSet")
}

pub fn text_of(h: HWND) -> String {
    unsafe {
        let mut b = vec![0u16; GetWindowTextLengthW(h) as usize + 1];
        let n = GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }
}

pub fn set_text(h: HWND, text: &str) {
    unsafe { SetWindowTextW(h, wide(text).as_ptr()) };
}

/// Register a window class with the program's icon; the result is the class name.
pub unsafe fn register(name: &str, proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT, style: u32) -> Vec<u16> {
    let class = wide(name);
    let module = GetModuleHandleW(null());
    RegisterClassW(&WNDCLASSW {
        style,
        lpfnWndProc: Some(proc_),
        hInstance: module,
        hIcon: LoadIconW(module, 1 as *const u16),
        hCursor: LoadCursorW(null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..std::mem::zeroed()
    });
    class
}

/// A rectangle in device-independent pixels: x, y, width, height.
pub type Rect = (f32, f32, f32, f32);

fn color(rgb: u32) -> D2D1_COLOR_F {
    let c = |v: u32| (v & 0xff) as f32 / 255.0;
    D2D1_COLOR_F { r: c(rgb >> 16), g: c(rgb >> 8), b: c(rgb), a: 1.0 }
}

fn rect((x, y, w, h): Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

/// A rounded shape with smooth edges on a GDI drawing, whose own shapes have jagged ones. `r` and `radius` are in pixels;
/// `bg` is the colour behind it. ponytail: a factory and a target per shape, keep them if scrolling ever stutters.
pub unsafe fn round(dc: HDC, r: RECT, bg: u32, fill: u32, line: u32, radius: f32) {
    let draw = || -> windows::core::Result<()> {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let target = d2d.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            dpiX: 96.0,
            dpiY: 96.0,
            ..Default::default()
        })?;
        target.BindDC(ComHdc(dc), &ComRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })?;
        let brush = target.CreateSolidColorBrush(&color(fill), None)?;
        let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
        target.BeginDraw();
        target.Clear(Some(&color(bg)));
        target.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect((0.0, 0.0, w, h)), radiusX: radius, radiusY: radius }, &brush);
        if line != fill {
            brush.SetColor(&color(line));
            target.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect((0.5, 0.5, w - 1.0, h - 1.0)), radiusX: radius, radiusY: radius }, &brush, 1.0, None);
        }
        target.EndDraw(None, None)
    };
    let _ = draw();
}

/// A window's drawing surface. Sizes are device-independent: Direct2D scales by the window's DPI.
pub struct Canvas {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    target: Option<ID2D1HwndRenderTarget>,
    brush: Option<ID2D1SolidColorBrush>,
    /// (size, bold) and its text format
    formats: Vec<((u32, bool), IDWriteTextFormat)>,
}

impl Canvas {
    pub fn new(hwnd: HWND) -> Option<Canvas> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
            Some(Canvas { hwnd, d2d, dw, target: None, brush: None, formats: vec![] })
        }
    }

    /// Build the render target for the window's size in pixels at this DPI. Bitmaps of the old target are dropped by the caller.
    pub fn resize(&mut self, w: u32, h: u32, dpi: f32) {
        unsafe {
            if let Some(t) = &self.target {
                t.SetDpi(dpi, dpi);
                if t.Resize(&D2D_SIZE_U { width: w, height: h }).is_ok() {
                    return;
                }
            }
            self.target = self.d2d.CreateHwndRenderTarget(
                &D2D1_RENDER_TARGET_PROPERTIES {
                    r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    dpiX: dpi,
                    dpiY: dpi,
                    ..Default::default()
                },
                &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                    hwnd: ComHwnd(self.hwnd),
                    pixelSize: D2D_SIZE_U { width: w, height: h },
                    presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                },
            ).ok();
            self.brush = self.target.as_ref().and_then(|t| t.CreateSolidColorBrush(&color(0), None).ok());
        }
    }

    pub fn begin(&self, bg: u32) {
        if let Some(t) = &self.target {
            unsafe {
                t.BeginDraw();
                t.Clear(Some(&color(bg)));
            }
        }
    }

    /// False when the target was lost: it and its bitmaps must be made again.
    pub fn end(&mut self) -> bool {
        let Some(t) = &self.target else { return true };
        let ok = unsafe { t.EndDraw(None, None) }.is_ok();
        if !ok {
            (self.target, self.brush) = (None, None);
        }
        ok
    }

    pub fn clip(&self, r: Rect) {
        if let Some(t) = &self.target {
            unsafe { t.PushAxisAlignedClip(&rect(r), D2D1_ANTIALIAS_MODE_ALIASED) };
        }
    }

    pub fn unclip(&self) {
        if let Some(t) = &self.target {
            unsafe { t.PopAxisAlignedClip() };
        }
    }

    fn brush(&self, rgb: u32) -> Option<(&ID2D1HwndRenderTarget, &ID2D1SolidColorBrush)> {
        let (t, b) = (self.target.as_ref()?, self.brush.as_ref()?);
        unsafe { b.SetColor(&color(rgb)) };
        Some((t, b))
    }

    pub fn fill(&self, r: Rect, rgb: u32, radius: f32) {
        if let Some((t, b)) = self.brush(rgb) {
            unsafe { t.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(r), radiusX: radius, radiusY: radius }, b) };
        }
    }

    pub fn image(&self, bmp: &ID2D1Bitmap, r: Rect, opacity: f32) {
        if let Some(t) = &self.target {
            unsafe { t.DrawBitmap(bmp, Some(&rect(r)), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        }
    }

    /// A bitmap from top-down BGRA rows with plain alpha (the target wants it premultiplied).
    pub fn bitmap(&self, w: usize, h: usize, bgra: &[u8]) -> Option<ID2D1Bitmap> {
        let mut px = bgra.to_vec();
        for c in px.chunks_exact_mut(4) {
            let a = c[3] as u32;
            c[..3].iter_mut().for_each(|v| *v = (*v as u32 * a / 255) as u8);
        }
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe { self.target.as_ref()?.CreateBitmap(D2D_SIZE_U { width: w as u32, height: h as u32 }, Some(px.as_ptr().cast()), (w * 4) as u32, &props).ok() }
    }

    /// One line of Segoe UI, cut with an ellipsis when it does not fit.
    fn format(&mut self, size: f32, bold: bool) -> Option<IDWriteTextFormat> {
        let key = ((size * 4.0) as u32, bold);
        if let Some((_, f)) = self.formats.iter().find(|(k, _)| *k == key) {
            return Some(f.clone());
        }
        unsafe {
            let weight = if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL };
            let (family, locale) = (wide("Segoe UI"), wide("en-us"));
            let f = self.dw.CreateTextFormat(PCWSTR(family.as_ptr()), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, PCWSTR(locale.as_ptr())).ok()?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).ok()?;
            let sign = self.dw.CreateEllipsisTrimmingSign(&f).ok()?;
            f.SetTrimming(&DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, ..Default::default() }, &sign).ok()?;
            self.formats.push((key, f.clone()));
            Some(f)
        }
    }

    fn layout(&mut self, text: &str, size: f32, bold: bool, w: f32, h: f32) -> Option<IDWriteTextLayout> {
        let f = self.format(size, bold)?;
        let t: Vec<u16> = text.encode_utf16().collect();
        unsafe { self.dw.CreateTextLayout(&t, &f, w.max(0.0), h).ok() }
    }

    fn put(&self, l: &IDWriteTextLayout, x: f32, y: f32, rgb: u32) {
        if let Some((t, b)) = self.brush(rgb) {
            unsafe { t.DrawTextLayout(Vector2 { X: x, Y: y }, l, b, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT) };
        }
    }

    pub fn text(&mut self, text: &str, size: f32, bold: bool, (x, y, w, h): Rect, rgb: u32) {
        if let Some(l) = self.layout(text, size, bold, w, h) {
            self.put(&l, x, y, rgb);
        }
    }

    /// Text that ends at the right edge of the rectangle; the answer is its width.
    pub fn text_right(&mut self, text: &str, size: f32, (x, y, w, h): Rect, rgb: u32) -> f32 {
        let Some(l) = self.layout(text, size, false, w, h) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = unsafe { l.GetMetrics(&mut m) };
        let _ = unsafe { l.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING) };
        self.put(&l, x, y, rgb);
        m.width
    }

    /// Text with some ranges (UTF-16 start, length) bold and in another colour.
    pub fn text_marked(&mut self, text: &str, size: f32, (x, y, w, h): Rect, rgb: u32, marks: &[(u32, u32)], mark_rgb: u32) {
        let Some(l) = self.layout(text, size, false, w, h) else { return };
        let mark = self.target.as_ref().and_then(|t| unsafe { t.CreateSolidColorBrush(&color(mark_rgb), None) }.ok());
        for &(start, length) in marks {
            let range = DWRITE_TEXT_RANGE { startPosition: start, length };
            unsafe {
                let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, range);
                if let Some(m) = &mark {
                    let _ = l.SetDrawingEffect(&m.cast::<windows::core::IUnknown>().unwrap(), range);
                }
            }
        }
        self.put(&l, x, y, rgb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::*;

    #[test]
    fn a_round_shape_has_soft_edges() {
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            SelectObject(dc, CreateCompatibleBitmap(screen, 60, 20) as HGDIOBJ);
            round(dc, RECT { left: 0, top: 0, right: 60, bottom: 20 }, 0, 0xffffff, 0xffffff, 10.0);
            assert_eq!(GetPixel(dc, 0, 0), 0);
            assert_eq!(GetPixel(dc, 30, 10), 0xffffff);
            let soft = (0..10).flat_map(|x| (0..10).map(move |y| (x, y))).filter(|&(x, y)| !matches!(GetPixel(dc, x, y), 0 | 0xffffff)).count();
            assert!(soft > 3, "only {soft} blended pixels");
        }
    }
}
