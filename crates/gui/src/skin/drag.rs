//! Latched pointer-driven window resize, for platforms where winit cannot
//! start a native resize (`drag_resize_window` is unsupported on macOS).
//!
//! Everything is captured once, when the button goes down: the resize
//! direction, the pointer position on the screen, the window origin and
//! size, and the minimum size. Every later pointer position is turned into a
//! target frame from those latched values alone, so the result never depends
//! on what the window did in response to an earlier step, nor on what is
//! under the pointer now.

use super::hit::ResizeDir;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizeDrag {
    dir: ResizeDir,
    /// Pointer position on the screen at press time (physical pixels).
    anchor: (f64, f64),
    /// Window origin at press time (screen, physical pixels).
    origin: (i32, i32),
    /// Inner size at press time.
    size: (u32, u32),
    min: (u32, u32),
}

impl ResizeDrag {
    pub fn begin(dir: ResizeDir, anchor: (f64, f64), origin: (i32, i32), size: (u32, u32), min: (u32, u32)) -> Self {
        Self { dir, anchor, origin, size, min }
    }

    pub fn dir(&self) -> ResizeDir {
        self.dir
    }

    /// The window frame (origin, inner size) for a pointer at `pointer` (screen
    /// coordinates). The edge opposite to the dragged one stays where it was
    /// at press time: a bottom-right drag never moves the top-left corner, a
    /// left or top drag moves the origin by exactly what the size lost or gained.
    pub fn target(&self, pointer: (f64, f64)) -> ((i32, i32), (u32, u32)) {
        use ResizeDir::*;
        let (dx, dy) = (pointer.0 - self.anchor.0, pointer.1 - self.anchor.1);
        let west = matches!(self.dir, W | NW | SW);
        let east = matches!(self.dir, E | NE | SE);
        let north = matches!(self.dir, N | NW | NE);
        let south = matches!(self.dir, S | SW | SE);
        let (sw, sh) = (self.size.0 as f64, self.size.1 as f64);
        let mut w = sw;
        let mut h = sh;
        if east {
            w = sw + dx;
        } else if west {
            w = sw - dx;
        }
        if south {
            h = sh + dy;
        } else if north {
            h = sh - dy;
        }
        let w = w.round().max(self.min.0.max(1) as f64) as u32;
        let h = h.round().max(self.min.1.max(1) as f64) as u32;
        let mut pos = self.origin;
        if west {
            pos.0 += self.size.0 as i32 - w as i32;
        }
        if north {
            pos.1 += self.size.1 as i32 - h as i32;
        }
        (pos, (w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag(dir: ResizeDir) -> ResizeDrag {
        ResizeDrag::begin(dir, (500.0, 400.0), (100, 50), (800, 600), (300, 200))
    }

    #[test]
    fn bottom_right_grows_by_the_full_delta_and_keeps_top_left() {
        let d = drag(ResizeDir::SE);
        assert_eq!(d.target((540.0, 430.0)), ((100, 50), (840, 630)));
        // The result is absolute, not cumulative: moving on, or back, depends
        // only on the latched press state.
        assert_eq!(d.target((580.0, 490.0)), ((100, 50), (880, 690)));
        assert_eq!(d.target((500.0, 400.0)), ((100, 50), (800, 600)));
        // A pointer far outside any grip rect still drives the drag.
        assert_eq!(d.target((2000.0, 1500.0)), ((100, 50), (2300, 1700)));
    }

    #[test]
    fn minimum_size_is_honored() {
        let d = drag(ResizeDir::SE);
        assert_eq!(d.target((-900.0, -900.0)), ((100, 50), (300, 200)));
    }

    #[test]
    fn west_and_north_move_the_origin() {
        let d = drag(ResizeDir::NW);
        assert_eq!(d.target((470.0, 380.0)), ((70, 30), (830, 620)));
        // Shrinking past the minimum pins the far edge, not the near one.
        assert_eq!(d.target((5000.0, 5000.0)), ((600, 450), (300, 200)));
        let e = drag(ResizeDir::E);
        assert_eq!(e.target((530.0, 999.0)), ((100, 50), (830, 600)));
        let s = drag(ResizeDir::S);
        assert_eq!(s.target((999.0, 410.0)), ((100, 50), (800, 610)));
    }
}
