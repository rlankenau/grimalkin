//! The global pointer position and button state, independent of any window.
//!
//! Window-local `CursorMoved` coordinates are unreliable while the window is
//! being resized from the pointer (AppKit re-derives them against a frame that
//! is changing underneath the drag), so a pointer-driven resize reads the
//! pointer from the system instead. Only macOS needs this (elsewhere the
//! window system runs the resize); other platforms return `None`.

/// Pointer position on the screen in points, top-left origin (the same space
/// winit reports window positions in, before the scale factor).
#[cfg(target_os = "macos")]
pub fn global_position() -> Option<(f64, f64)> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *mut c_void;
        fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    // SAFETY: a null source is allowed; the event is released after reading.
    unsafe {
        let ev = CGEventCreate(std::ptr::null());
        if ev.is_null() {
            return None;
        }
        let p = CGEventGetLocation(ev);
        CFRelease(ev);
        Some((p.x, p.y))
    }
}

/// Whether the left button is down right now (system state, not the event
/// stream, so a lost mouse-up is noticed).
#[cfg(target_os = "macos")]
pub fn left_button_down() -> Option<bool> {
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventSourceButtonState(state_id: i32, button: u32) -> bool;
    }
    // kCGEventSourceStateCombinedSessionState = 0, kCGMouseButtonLeft = 0.
    // SAFETY: plain value arguments.
    Some(unsafe { CGEventSourceButtonState(0, 0) })
}

#[cfg(not(target_os = "macos"))]
pub fn global_position() -> Option<(f64, f64)> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn left_button_down() -> Option<bool> {
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn system_pointer_state_is_readable() {
        let (x, y) = super::global_position().expect("pointer position");
        assert!(x.is_finite() && y.is_finite());
        assert!(super::left_button_down().is_some());
    }
}
