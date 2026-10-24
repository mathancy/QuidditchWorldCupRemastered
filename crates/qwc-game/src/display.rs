//! The game's window as the player's settings want it (`qwc_settings`): windowed, borderless over the monitor or the
//! monitor switched to the chosen resolution, and the frames in step with the monitor's refresh or not. Not the
//! original's.

use qwc_render::wgpu;
use qwc_settings::WindowMode;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Icon, Window, WindowAttributes};

/// The logo (the launcher's, `qwc-style/assets/qwc_logo.png`) as the window's icon.
pub fn icon() -> Option<Icon> {
    let bytes: &[u8] = include_bytes!("../../qwc-style/assets/qwc_logo.png");
    let mut reader = png::Decoder::new(bytes).read_info().ok()?;
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut rgba).ok()?;
    if (info.color_type, info.bit_depth) != (png::ColorType::Rgba, png::BitDepth::Eight) {
        return None;
    }
    rgba.truncate(info.buffer_size());
    Icon::from_rgba(rgba, info.width, info.height).ok()
}

/// The window's attributes: `title`, `size` (pixels) and `mode`. Fullscreen takes the primary monitor's mode of that
/// size with the highest refresh, and is borderless when the monitor has none.
pub fn attributes(el: &ActiveEventLoop, title: &str, size: (u32, u32), mode: WindowMode) -> WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1))
        .with_window_icon(icon());
    #[cfg(windows)]
    let attrs = winit::platform::windows::WindowAttributesExtWindows::with_taskbar_icon(attrs, icon());
    let monitor = el.primary_monitor().or_else(|| el.available_monitors().next());
    let full = match mode {
        WindowMode::Windowed => None,
        WindowMode::Borderless => Some(Fullscreen::Borderless(monitor)),
        WindowMode::Fullscreen => {
            let exact = monitor.as_ref().and_then(|m| {
                m.video_modes()
                    .filter(|v| (v.size().width, v.size().height) == size)
                    .max_by_key(|v| (v.refresh_rate_millihertz(), v.bit_depth()))
            });
            match exact {
                Some(v) => Some(Fullscreen::Exclusive(v)),
                None => {
                    eprintln!("no {}x{} mode on the monitor: borderless instead", size.0, size.1);
                    Some(Fullscreen::Borderless(monitor))
                }
            }
        }
    };
    attrs.with_fullscreen(full)
}

/// The sizes the in-game Video options offer (`TeamTentOptionsVideo`: 640 x 480 to 1600 x 1200).
const OPTIONS: [(u32, u32); 5] = [(640, 480), (800, 600), (1024, 768), (1152, 864), (1600, 1200)];

/// The display modes `General_VerifyDisplay` finds (`FrontEnd::display_modes`): every size the window's monitor
/// has, and in a window the Video options' sizes that fit on it, each at 32 and 16 bits (the remaster draws both
/// the same); `current`, the window's size, is among them. Not the original's: its list is the adapter's modes.
pub fn modes(window: &Window, mode: WindowMode, current: (u32, u32)) -> Vec<(u32, u32, u32)> {
    let monitor = window.current_monitor().or_else(|| window.primary_monitor());
    let mut sizes: Vec<(u32, u32)> =
        monitor.iter().flat_map(|m| m.video_modes().map(|v| (v.size().width, v.size().height))).collect();
    let largest = sizes.iter().copied().max_by_key(|&(w, h)| w * h).unwrap_or(current);
    if mode == WindowMode::Windowed {
        sizes.extend(OPTIONS.iter().filter(|&&(w, h)| w <= largest.0 && h <= largest.1));
    }
    sizes.push(current);
    sizes.sort_unstable();
    sizes.dedup();
    sizes.iter().flat_map(|&(w, h)| [(w, h, 32), (w, h, 16)]).collect()
}

/// [`modes`] without a window (a headless picture of `current`'s size): the Video options' sizes and `current`.
pub fn headless_modes(current: (u32, u32)) -> Vec<(u32, u32, u32)> {
    let mut sizes: Vec<(u32, u32)> = OPTIONS.iter().copied().chain([current]).collect();
    sizes.sort_unstable();
    sizes.dedup();
    sizes.iter().flat_map(|&(w, h)| [(w, h, 32), (w, h, 16)]).collect()
}

/// Change the window to `size` as `mode` keeps it: a window takes the size, fullscreen switches the monitor to its
/// mode of that size (borderless when it has none), borderless stays the monitor's.
pub fn apply(window: &Window, size: (u32, u32), mode: WindowMode) {
    match mode {
        WindowMode::Windowed => {
            let _ = window.request_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1));
        }
        WindowMode::Borderless => {}
        WindowMode::Fullscreen => {
            let monitor = window.current_monitor().or_else(|| window.primary_monitor());
            let exact = monitor.as_ref().and_then(|m| {
                m.video_modes()
                    .filter(|v| (v.size().width, v.size().height) == size)
                    .max_by_key(|v| (v.refresh_rate_millihertz(), v.bit_depth()))
            });
            window.set_fullscreen(Some(match exact {
                Some(v) => Fullscreen::Exclusive(v),
                None => Fullscreen::Borderless(monitor),
            }));
        }
    }
}

/// The surface's present mode: in step with the refresh (`vsync`), else the fastest the surface offers without it.
pub fn present_mode(vsync: bool, caps: &wgpu::SurfaceCapabilities) -> wgpu::PresentMode {
    if vsync {
        return wgpu::PresentMode::Fifo;
    }
    [wgpu::PresentMode::Mailbox, wgpu::PresentMode::Immediate]
        .into_iter()
        .find(|m| caps.present_modes.contains(m))
        .unwrap_or(wgpu::PresentMode::Fifo)
}

#[cfg(test)]
mod tests {
    #[test]
    fn headless_modes_hold_the_options_and_the_picture() {
        let m = super::headless_modes((1280, 720));
        assert_eq!(m.len(), 12);
        assert!(m.contains(&(1280, 720, 32)) && m.contains(&(1600, 1200, 16)));
        assert_eq!(super::headless_modes((800, 600)).len(), 10, "a size of the options' is not listed twice");
    }
}
