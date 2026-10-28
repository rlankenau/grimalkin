//! Audible bell: play the system alert sound by handing it to a small OS
//! helper, so no audio or platform-API dependency is needed. Best effort;
//! failures are ignored.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

/// True while a helper process is playing, so a flood of BELs does not spawn
/// a flood of processes.
static PLAYING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn command() -> Command {
    let mut c = Command::new("afplay");
    c.arg("/System/Library/Sounds/Tink.aiff");
    c
}

#[cfg(windows)]
fn command() -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut c = Command::new("powershell");
    c.args(["-NoProfile", "-NonInteractive", "-Command", "[console]::beep(880,120)"]);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

#[cfg(not(any(target_os = "macos", windows)))]
fn command() -> Command {
    let mut c = Command::new("canberra-gtk-play");
    c.args(["-i", "bell"]);
    c
}

/// Play the system beep without blocking the caller.
pub fn beep() {
    if PLAYING.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut cmd = command();
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
                PLAYING.store(false, Ordering::Release);
            });
        }
        Err(e) => {
            log::debug!("cannot play bell: {e}");
            PLAYING.store(false, Ordering::Release);
        }
    }
}
