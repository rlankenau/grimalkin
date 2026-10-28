//! Windows: clip the window to the skin shape with a window region.
//!
//! Per-pixel alpha swapchains are unreliable across wgpu backends, so the
//! swapchain stays opaque and the OS clips drawing and hit testing to an
//! `HRGN` built from the scaled frame mask (run-length rectangles from
//! `hit::shape_rects`). Re-applied whenever the window size or DPI changes.
//!
//! Coordinates: the shape, the chrome and every hit test are in *client*
//! coordinates, but `SetWindowRgn` takes *window* coordinates. winit strips
//! the non-client area of an undecorated window (`WM_NCCALCSIZE`), so the two
//! agree except when the OS itself zooms or snaps the window: then the window
//! rectangle extends past the client area by the invisible resize frame (8 px
//! on every side, `-8,-8..2568,1418` for a 2560x1410 work area). The region is
//! therefore shifted by the client origin's offset inside the window rectangle.
//! The skinned window never asks the OS to maximize (`skin/winstate.rs`).

#[cfg(windows)]
pub use imp::{set_window_region, work_area};

#[cfg(windows)]
mod imp {
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{
        ClientToScreen, DeleteObject, ExtCreateRegion, GetMonitorInfoW, MonitorFromWindow, SetWindowRgn, MONITORINFO,
        MONITOR_DEFAULTTONEAREST, RDH_RECTANGLES, RGNDATAHEADER,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    /// Restrict `window` to the union of `rects` (x0, y0, x1, y1; exclusive
    /// max; window pixels). An empty list or a failure leaves the window
    /// unclipped.
    pub fn set_window_region(window: &Window, rects: &[[i32; 4]], width: i32, height: i32) {
        let Some(hwnd) = hwnd_of(window) else { return };
        if rects.is_empty() {
            return;
        }
        let (dx, dy) = client_offset(hwnd);
        // Whole window inside the shape: no region needed.
        if rects.len() == 1 && rects[0] == [0, 0, width, height] && (dx, dy) == (0, 0) {
            unsafe { SetWindowRgn(hwnd, std::ptr::null_mut(), 1) };
            return;
        }
        // RGNDATA = RGNDATAHEADER followed by the RECT array.
        let header = size_of::<RGNDATAHEADER>();
        let rect_sz = size_of::<RECT>();
        let total = header + rects.len() * rect_sz;
        // u32-aligned backing store (the header is all u32/i32 fields).
        let mut buf = vec![0u32; total.div_ceil(4)];
        let base = buf.as_mut_ptr() as *mut u8;
        let bound = RECT {
            left: dx,
            top: dy,
            right: dx + width,
            bottom: dy + height,
        };
        let hdr = RGNDATAHEADER {
            dwSize: header as u32,
            iType: RDH_RECTANGLES,
            nCount: rects.len() as u32,
            nRgnSize: (rects.len() * rect_sz) as u32,
            rcBound: bound,
        };
        // SAFETY: `buf` is at least `total` bytes and 4-byte aligned, which is
        // enough for RGNDATAHEADER and RECT (all 32-bit fields).
        unsafe {
            std::ptr::write(base as *mut RGNDATAHEADER, hdr);
            let out = base.add(header) as *mut RECT;
            for (i, r) in rects.iter().enumerate() {
                std::ptr::write(
                    out.add(i),
                    RECT {
                        left: r[0] + dx,
                        top: r[1] + dy,
                        right: r[2] + dx,
                        bottom: r[3] + dy,
                    },
                );
            }
            let rgn = ExtCreateRegion(std::ptr::null(), total as u32, base as *const _);
            if rgn.is_null() {
                log::warn!("skin: ExtCreateRegion failed");
                return;
            }
            // On success the system owns the region; on failure we must free it.
            if SetWindowRgn(hwnd, rgn, 1) == 0 {
                log::warn!("skin: SetWindowRgn failed");
                DeleteObject(rgn);
            }
        }
    }

    /// Where the client area's origin lies inside the window rectangle.
    fn client_offset(hwnd: HWND) -> (i32, i32) {
        let mut wr = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        let mut origin = POINT { x: 0, y: 0 };
        // SAFETY: valid out pointers to local structs.
        unsafe {
            if GetWindowRect(hwnd, &mut wr) == 0 || ClientToScreen(hwnd, &mut origin) == 0 {
                return (0, 0);
            }
        }
        (origin.x - wr.left, origin.y - wr.top)
    }

    /// The work area (monitor minus taskbar) of the monitor the window is
    /// mostly on, as (x, y, w, h) in physical screen pixels.
    pub fn work_area(window: &Window) -> Option<(i32, i32, u32, u32)> {
        let hwnd = hwnd_of(window)?;
        // SAFETY: `info` is a properly sized MONITORINFO; the monitor handle
        // comes from the system.
        unsafe {
            let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            if monitor.is_null() {
                return None;
            }
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return None;
            }
            let r = info.rcWork;
            Some((r.left, r.top, (r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32))
        }
    }

    fn hwnd_of(window: &Window) -> Option<HWND> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get() as HWND),
            _ => None,
        }
    }
}

/// Non-Windows platforms clip with a transparent swapchain instead.
#[cfg(not(windows))]
#[allow(dead_code)]
pub fn set_window_region(_window: &winit::window::Window, _rects: &[[i32; 4]], _width: i32, _height: i32) {}
