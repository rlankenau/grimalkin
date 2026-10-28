//! Test hooks for automated checks (frame dumps, CI), all driven by
//! environment variables so a normal run never touches them.
//!
//! * `GRIMALKIN_SCALE_FACTOR=2` pretends the display scale is 2 (HiDPI chrome
//!   and glyph checks on a 1x screen).
//! * `GRIMALKIN_DEBUG_SCRIPT="1000:hover:close;2000:shade;3000:size:900x500"`
//!   runs commands (`delay-ms:command`, delays are relative to the previous
//!   step) through the real event paths, one per `UserEvent::Debug`.
//!
//! Commands: `move:X,Y` (window pixels), `hover:BUTTON`, `down`, `up`,
//! `click`, `dblclick`, `press:BUTTON` (move onto the button and press
//! without releasing), `shade`, `maximize`, `size:WxH` (physical inner size), `check` (log whether
//! chrome and window agree), `dump:PATH` (redirect the frame dump), `menu:ID` (run a menu action, e.g.
//! `menu:theme:nord`), `rightclick` (context menu at the pointer), `prefs` (open the Preferences window), `exit`.

use std::time::Duration;

use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;

const SCALE_ENV: &str = "GRIMALKIN_SCALE_FACTOR";
const SCRIPT_ENV: &str = "GRIMALKIN_DEBUG_SCRIPT";

/// The display scale to use: `GRIMALKIN_SCALE_FACTOR` if set and sane, else
/// what the window system reported.
pub fn effective_scale(reported: f64) -> f64 {
    std::env::var(SCALE_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.5 && *s <= 8.0)
        .unwrap_or(reported)
}

/// Parse `delay:command;delay:command...` into (delay, command) pairs.
pub fn parse_script(text: &str) -> Vec<(Duration, String)> {
    text.split(';')
        .filter_map(|step| {
            let (ms, cmd) = step.trim().split_once(':')?;
            Some((Duration::from_millis(ms.trim().parse().ok()?), cmd.trim().to_string()))
        })
        .collect()
}

/// Start the script thread if `GRIMALKIN_DEBUG_SCRIPT` is set; returns the
/// commands, indexed by `UserEvent::Debug`.
pub fn start(proxy: &EventLoopProxy<UserEvent>) -> Vec<String> {
    let Ok(text) = std::env::var(SCRIPT_ENV) else { return Vec::new() };
    let steps = parse_script(&text);
    let cmds: Vec<String> = steps.iter().map(|(_, c)| c.clone()).collect();
    let proxy = proxy.clone();
    std::thread::spawn(move || {
        for (i, (delay, _)) in steps.into_iter().enumerate() {
            std::thread::sleep(delay);
            if proxy.send_event(UserEvent::Debug(i)).is_err() {
                return;
            }
        }
    });
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_steps() {
        let s = parse_script("1000:hover:close; 250:shade;bad;x:y");
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], (Duration::from_millis(1000), "hover:close".to_string()));
        assert_eq!(s[1].1, "shade");
    }
}
