//! The panic hook must survive a closed stderr pipe: `eprintln!` panics when
//! the write fails, and a panic inside the hook aborts the process (SIGABRT,
//! exit 134 and an OS crash report) instead of the clean exit 101.

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn closed_stderr_pipe_still_exits_101() {
    let dir = std::env::temp_dir().join(format!("grimalkin-panichook-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_grimalkin"))
        .env("GRIMALKIN_DEBUG_PANIC", "300")
        // Keep the panic log out of the real data directory.
        .env("XDG_DATA_HOME", &dir)
        .env("HOME", &dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn grimalkin");
    // Close the read end before the child panics: its writes now fail (EPIPE).
    drop(child.stderr.take());
    std::thread::sleep(Duration::from_millis(50));
    let status = child.wait().expect("wait");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(status.code(), Some(101), "expected a clean exit 101, got {status:?}");
}
