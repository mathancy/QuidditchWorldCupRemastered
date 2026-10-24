//! The window's shape: on Windows its corners are cut off at 45 degrees ([`CUT`] points), the system clipping the
//! window to the octagon (`SetWindowRgn`), so the desktop shows through them and they take no clicks. Maximised it
//! is the whole rectangle again. Other systems keep a rectangle (their windows cannot be clipped this way).
//!
//! The clip is the client area's, placed where the client area is in the window: a frameless window that can be
//! resized keeps a sliver of its sizing frame above its client area, which Windows draws as a light line along the
//! top, and Windows 11 draws a border of its own round the window (turned off here). Neither is left showing.

use eframe::egui;

/// How far along each side the window's corners are cut.
pub(super) const CUT: f32 = 16.0;

/// Whether the window is cut now: on Windows, when not maximised.
pub(super) fn cut(ctx: &egui::Context) -> bool {
    cfg!(windows) && !ctx.input(|i| i.viewport().maximized).unwrap_or(false)
}

/// The client area (pixels, within the window) and the maximised state the window was last shaped for.
pub(super) type Shaped = Option<([i32; 4], bool)>;

/// Clip the window to its client area, its corners cut unless maximised, when that changed since `last`; Windows
/// 11's own border is turned off the first time (nothing elsewhere than Windows).
#[cfg(windows)]
pub(super) fn apply(last: &mut Shaped, frame: &eframe::Frame, ctx: &egui::Context) {
    use std::ffi::c_void;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[repr(C)]
    #[derive(Default)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn CreatePolygonRgn(points: *const Point, count: i32, mode: i32) -> *mut c_void;
    }
    #[link(name = "user32")]
    extern "system" {
        fn SetWindowRgn(hwnd: *mut c_void, region: *mut c_void, redraw: i32) -> i32;
        fn GetWindowRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
        fn GetClientRect(hwnd: *mut c_void, rect: *mut Rect) -> i32;
        fn ClientToScreen(hwnd: *mut c_void, point: *mut Point) -> i32;
    }
    #[link(name = "dwmapi")]
    extern "system" {
        fn DwmSetWindowAttribute(hwnd: *mut c_void, attribute: u32, value: *const c_void, size: u32) -> i32;
    }
    const WINDING: i32 = 2;
    /// `DWMWA_BORDER_COLOR` and `DWMWA_COLOR_NONE` (Windows 11; Windows 10 answers an error and draws none anyway).
    const BORDER_COLOR: u32 = 34;
    const COLOR_NONE: u32 = 0xFFFF_FFFE;

    let Ok(handle) = frame.window_handle() else { return };
    let RawWindowHandle::Win32(win) = handle.as_raw() else { return };
    let hwnd = win.hwnd.get() as *mut c_void;
    let max = !cut(ctx);
    let (mut window, mut client, mut origin) = (Rect::default(), Rect::default(), Point::default());
    // SAFETY: plain Win32 queries on the live window's handle (eframe's), into structs laid out as Windows expects
    let ok = unsafe {
        GetWindowRect(hwnd, &mut window) != 0
            && GetClientRect(hwnd, &mut client) != 0
            && ClientToScreen(hwnd, &mut origin) != 0
    };
    if !ok {
        return;
    }
    // the client area within the window
    let (x, y) = (origin.x - window.left, origin.y - window.top);
    let (w, h) = (client.right - client.left, client.bottom - client.top);
    if *last == Some(([x, y, w, h], max)) || w < 1 || h < 1 {
        return;
    }
    let area = egui::Rect::from_min_size(egui::pos2(x as f32, y as f32), egui::vec2(w as f32, h as f32));
    let cut_px = if max { 0.0 } else { CUT * ctx.pixels_per_point() };
    let points =
        qwc_style::ornate::octagon(area, cut_px, 0.0).map(|p| Point { x: p.x.round() as i32, y: p.y.round() as i32 });
    // SAFETY: the points outlive the call that copies them; a region given to SetWindowRgn belongs to the system from
    // then on; the border's colour is a u32, as the attribute takes
    unsafe {
        if last.is_none() {
            DwmSetWindowAttribute(hwnd, BORDER_COLOR, (&COLOR_NONE as *const u32).cast(), 4);
        }
        let region = CreatePolygonRgn(points.as_ptr(), points.len() as i32, WINDING);
        if !region.is_null() {
            SetWindowRgn(hwnd, region, 1);
        }
    }
    *last = Some(([x, y, w, h], max));
}

#[cfg(not(windows))]
pub(super) fn apply(_last: &mut Shaped, _frame: &eframe::Frame, _ctx: &egui::Context) {}
