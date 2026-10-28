//! Windows: clip the window to the skin shape with a window region.
//!
//! Per-pixel alpha swapchains are unreliable across wgpu backends, so the
//! swapchain stays opaque and the OS clips drawing and hit testing to an
//! `HRGN` built from the scaled frame mask (run-length rectangles from
//! `hit::shape_rects`). Re-applied whenever the window size or DPI changes.

#[cfg(windows)]
pub use imp::set_window_region;

#[cfg(windows)]
mod imp {
    use std::mem::size_of;

    use windows_sys::Win32::Foundation::{HWND, RECT};
    use windows_sys::Win32::Graphics::Gdi::{DeleteObject, ExtCreateRegion, SetWindowRgn, RDH_RECTANGLES, RGNDATAHEADER};
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
        // Whole window inside the shape: no region needed.
        if rects.len() == 1 && rects[0] == [0, 0, width, height] {
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
            left: 0,
            top: 0,
            right: width,
            bottom: height,
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
                        left: r[0],
                        top: r[1],
                        right: r[2],
                        bottom: r[3],
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
