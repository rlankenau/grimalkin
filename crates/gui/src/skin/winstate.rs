//! The skinned window's size state as one coherent value, and the pure
//! geometry of the transitions between its states.
//!
//! With a skin the OS draws nothing, so the OS zoom is never used: Windows'
//! `set_maximized` zooms an undecorated `WS_THICKFRAME` window to a rectangle
//! 8 px larger than the monitor work area on every side (the region and the
//! chrome are laid out for the client area, which does not move with it), and
//! it leaves the window's own idea of "maximized" free to disagree with the
//! chrome. Instead the app moves and resizes the window itself to the work
//! area and remembers where to come back to.
//!
//! All rectangles are physical pixels: the outer position and the inner size
//! (for an undecorated window the two coincide).

/// A window position and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect { x, y, w, h }
    }
}

/// What a shaded window goes back to when it is unshaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    Normal,
    /// Maximized; `restore` is where un-maximizing goes.
    Maximized { restore: Rect },
}

/// The window's size state. Exactly one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WinState {
    #[default]
    Normal,
    /// Filling the monitor work area; `restore` is the size and place before.
    Maximized { restore: Rect },
    /// Collapsed to the shade bar. `prev` is the state to return to and
    /// `before` the window rectangle at the moment it was shaded.
    Shaded { prev: Base, before: Rect },
}

/// A transition: the new state and the window rectangle to apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub state: WinState,
    pub target: Rect,
}

impl WinState {
    pub fn is_shaded(&self) -> bool {
        matches!(self, WinState::Shaded { .. })
    }

    /// Maximized, or shaded out of a maximized window (the shade bar spans
    /// the work area width, so it still must not be resized).
    pub fn is_maximized(&self) -> bool {
        matches!(self, WinState::Maximized { .. } | WinState::Shaded { prev: Base::Maximized { .. }, .. })
    }

    fn base(&self) -> Base {
        match *self {
            WinState::Maximized { restore } => Base::Maximized { restore },
            WinState::Shaded { prev, .. } => prev,
            WinState::Normal => Base::Normal,
        }
    }

    /// Maximize / restore button or double-click. `cur` is the window now,
    /// `work` the monitor work area. Maximizing a shaded window unshades it
    /// first and ends maximized; restoring returns to the pre-maximize size.
    pub fn toggle_maximize(&self, cur: Rect, work: Rect) -> Plan {
        match *self {
            WinState::Normal => Plan { state: WinState::Maximized { restore: cur }, target: work },
            WinState::Maximized { restore } => Plan { state: WinState::Normal, target: restore },
            WinState::Shaded { prev, before } => {
                // The size to restore later is the unshaded one, placed where
                // the window is now.
                let restore = match prev {
                    Base::Normal => Rect { x: cur.x, y: cur.y, ..before },
                    Base::Maximized { restore } => restore,
                };
                Plan { state: WinState::Maximized { restore }, target: work }
            }
        }
    }

    /// Windowshade button or double-click. `bar` is the shade bar height.
    /// Unshading always restores the size from before the shade.
    pub fn toggle_shade(&self, cur: Rect, bar: u32, work: Rect) -> Plan {
        match *self {
            WinState::Shaded { prev, before } => match prev {
                Base::Normal => Plan {
                    state: WinState::Normal,
                    target: Rect { x: cur.x, y: cur.y, w: before.w, h: before.h },
                },
                Base::Maximized { restore } => Plan { state: WinState::Maximized { restore }, target: work },
            },
            other => Plan {
                state: WinState::Shaded { prev: other.base(), before: cur },
                target: Rect { x: cur.x, y: cur.y, w: cur.w, h: bar.max(1) },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 25, 2560, 1380);
    const WIN: Rect = Rect::new(100, 80, 1016, 597);
    const BAR: u32 = 32;

    #[test]
    fn maximize_fills_the_work_area_and_restore_comes_back() {
        let max = WinState::Normal.toggle_maximize(WIN, WORK);
        assert_eq!(max.target, WORK, "exactly the work area, no frame offset");
        assert!(max.state.is_maximized());
        let back = max.state.toggle_maximize(WORK, WORK);
        assert_eq!(back.state, WinState::Normal);
        assert_eq!(back.target, WIN);
    }

    #[test]
    fn shade_keeps_position_and_width_and_unshade_restores_the_size() {
        let shade = WinState::Normal.toggle_shade(WIN, BAR, WORK);
        assert_eq!(shade.target, Rect::new(100, 80, 1016, BAR));
        assert!(shade.state.is_shaded());
        // Moved while shaded: unshade restores the size at the new place.
        let moved = Rect::new(300, 40, 1016, BAR);
        let un = shade.state.toggle_shade(moved, BAR, WORK);
        assert_eq!(un.state, WinState::Normal);
        assert_eq!(un.target, Rect::new(300, 40, 1016, 597));
    }

    #[test]
    fn maximize_shade_unshade_returns_to_maximized_then_restores() {
        let max = WinState::Normal.toggle_maximize(WIN, WORK);
        let shade = max.state.toggle_shade(WORK, BAR, WORK);
        assert_eq!(shade.target, Rect::new(0, 25, 2560, BAR));
        assert!(shade.state.is_maximized(), "still must not be resizable");
        let un = shade.state.toggle_shade(shade.target, BAR, WORK);
        assert_eq!(un.target, WORK);
        let back = un.state.toggle_maximize(WORK, WORK);
        assert_eq!((back.state, back.target), (WinState::Normal, WIN));
    }

    #[test]
    fn maximize_while_shaded_unshades_and_ends_maximized() {
        let shade = WinState::Normal.toggle_shade(WIN, BAR, WORK);
        let max = shade.state.toggle_maximize(shade.target, WORK);
        assert_eq!(max.target, WORK);
        assert_eq!(max.state, WinState::Maximized { restore: WIN });
        let back = max.state.toggle_maximize(WORK, WORK);
        assert_eq!((back.state, back.target), (WinState::Normal, WIN));
        // Shaded out of maximized: maximize stays maximized (not a toggle off).
        let m = WinState::Normal.toggle_maximize(WIN, WORK);
        let s = m.state.toggle_shade(WORK, BAR, WORK);
        let again = s.state.toggle_maximize(s.target, WORK);
        assert_eq!(again.state, m.state);
        assert_eq!(again.target, WORK);
    }

    #[test]
    fn repeated_toggles_never_lose_the_original_size() {
        let mut state = WinState::Normal;
        let mut cur = WIN;
        let steps: [fn(&WinState, Rect) -> Plan; 3] = [
            |s, c| s.toggle_shade(c, BAR, WORK),
            |s, c| s.toggle_maximize(c, WORK),
            |s, c| s.toggle_shade(c, BAR, WORK),
        ];
        for step in steps.iter().cycle().take(30) {
            let plan = step(&state, cur);
            state = plan.state;
            cur = plan.target;
            assert_eq!(state.is_shaded(), cur.h == BAR, "{state:?} {cur:?}");
        }
        // Whatever the order, getting back to Normal returns the first size.
        while state != WinState::Normal {
            let plan = if state.is_shaded() { state.toggle_shade(cur, BAR, WORK) } else { state.toggle_maximize(cur, WORK) };
            state = plan.state;
            cur = plan.target;
        }
        assert_eq!(cur, WIN);
    }
}
