//! macOS: the work area of the screen a window is on, for the skin's own
//! maximize (a borderless NSWindow has no zoom button, and the system zoom
//! would not leave the menu bar and Dock alone anyway).

use objc2::rc::Retained;
use objc2_app_kit::{NSScreen, NSView};
use objc2_foundation::MainThreadMarker;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

/// The screen's visible frame (menu bar and Dock excluded) as
/// (x, y, w, h) in physical pixels, top-left origin: the space winit reports
/// window positions in. Must run on the main thread (the event loop is).
pub fn work_area(window: &Window) -> Option<(i32, i32, u32, u32)> {
    let mtm = MainThreadMarker::new()?;
    let RawWindowHandle::AppKit(h) = window.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: the handle's NSView pointer is valid while the window lives.
    let view: &NSView = unsafe { h.ns_view.cast().as_ref() };
    let screen: Retained<NSScreen> = view.window()?.screen()?;
    // Cocoa's origin is the bottom-left of the primary screen; winit's is the
    // top-left of it.
    let primary_h = NSScreen::screens(mtm).iter().next()?.frame().size.height;
    let vf = screen.visibleFrame();
    let k = window.scale_factor();
    let top = primary_h - (vf.origin.y + vf.size.height);
    Some(((vf.origin.x * k).round() as i32, (top * k).round() as i32, (vf.size.width * k).round().max(1.0) as u32, (vf.size.height * k).round().max(1.0) as u32))
}
