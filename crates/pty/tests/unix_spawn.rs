#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grimalkin_pty::{Pty, PtyEvent, SpawnConfig};

/// Collect output until `pred(accumulated)` is true or timeout; returns the
/// accumulated bytes and any Exited event seen.
fn read_until(
    rx: &Receiver<PtyEvent>,
    timeout: Duration,
    pred: impl Fn(&[u8]) -> bool,
) -> (Vec<u8>, Option<Option<u32>>) {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut exited = None;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(PtyEvent::Output(b)) => {
                assert!(b.len() <= grimalkin_pty::MAX_CHUNK);
                out.extend_from_slice(&b);
            }
            Ok(PtyEvent::Exited(c)) => exited = Some(c),
            Err(_) => {}
        }
        if pred(&out) {
            break;
        }
        if exited.is_some() {
            break;
        }
    }
    (out, exited)
}

fn contains(hay: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(hay).contains(needle)
}

#[test]
fn echo_hello_then_exit() {
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "echo hello".to_string()];
    let pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let mut out = Vec::new();
    let mut exited = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while exited.is_none() && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PtyEvent::Output(b)) => out.extend(b),
            Ok(PtyEvent::Exited(c)) => exited = Some(c),
            Err(_) => {}
        }
    }
    assert!(contains(&out, "hello"), "output was {:?}", String::from_utf8_lossy(&out));
    assert_eq!(exited, Some(Some(0)));
    assert_eq!(pty.try_wait(), Some(Some(0)));
    assert!(!pty.is_alive());
}

#[test]
fn nonzero_exit_code() {
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "exit 7".to_string()];
    let _pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert_eq!(exited, Some(Some(7)));
}

#[test]
fn cat_echo_and_kill() {
    let (tx, rx) = channel();
    let mut pty = Pty::spawn("cat", &[], None, &[], 80, 24, tx).unwrap();
    assert!(pty.is_alive());
    assert!(pty.pid().is_some());
    pty.write_all(b"ping-pong\n").unwrap();
    pty.flush().unwrap();
    let (out, exited) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "ping-pong"));
    assert!(contains(&out, "ping-pong"), "got {:?}", String::from_utf8_lossy(&out));
    assert!(exited.is_none());

    pty.kill().unwrap();
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert!(exited.is_some(), "no Exited after kill");
    assert!(!pty.is_alive());
    // Killing again is harmless.
    pty.kill().unwrap();
}

#[test]
fn writer_clone_works_from_thread() {
    let (tx, rx) = channel();
    let pty = Pty::spawn("cat", &[], None, &[], 80, 24, tx).unwrap();
    let w = pty.writer();
    std::thread::spawn(move || {
        w.write_all(b"from-thread\n").unwrap();
        w.flush().unwrap();
    })
    .join()
    .unwrap();
    let (out, _) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "from-thread"));
    assert!(contains(&out, "from-thread"));
}

#[test]
fn env_and_cwd_applied() {
    let (tx, rx) = channel();
    let args = vec![
        "-c".to_string(),
        "echo T=$TERM C=$COLORTERM P=$TERM_PROGRAM X=$MY_VAR; pwd".to_string(),
    ];
    let cwd = std::fs::canonicalize("/tmp").unwrap();
    let _pty = Pty::spawn(
        "/bin/sh",
        &args,
        Some(&cwd),
        &[("MY_VAR".into(), "zzz".into())],
        80,
        24,
        tx,
    )
    .unwrap();
    let (out, _) = read_until(&rx, Duration::from_secs(10), |_| false);
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("T=xterm-kitty"), "{s}");
    assert!(s.contains("C=truecolor"), "{s}");
    assert!(s.contains("P=grimalkin"), "{s}");
    assert!(s.contains("X=zzz"), "{s}");
    assert!(s.contains(cwd.to_str().unwrap()), "{s}");
}

#[test]
fn resize_is_visible_to_child() {
    let (tx, rx) = channel();
    let mut pty = Pty::spawn("cat", &[], None, &[], 80, 24, tx).unwrap();
    pty.resize(100, 40, 0, 0).unwrap();
    // `stty size` in a separate shell would not share the pty, so ask via the
    // master instead: resize must at least not fail for sane and degenerate sizes.
    pty.resize(0, 0, 0, 0).unwrap();
    pty.kill().unwrap();
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert!(exited.is_some());
}

#[test]
fn resize_reflected_by_stty() {
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "sleep 1; stty size".to_string()];
    let pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    pty.resize(100, 40, 0, 0).unwrap();
    let (out, _) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "40 100"));
    assert!(contains(&out, "40 100"), "{:?}", String::from_utf8_lossy(&out));
}

#[test]
fn login_shell_gets_dash_argv0() {
    let (tx, rx) = channel();
    let pty = Pty::spawn("/bin/sh", &[], None, &[], 80, 24, tx).unwrap();
    pty.write_all(b"echo ARGV0=$0\nexit\n").unwrap();
    let (out, _) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "ARGV0=-"));
    assert!(contains(&out, "ARGV0=-"), "{:?}", String::from_utf8_lossy(&out));
}

#[test]
fn on_wake_called() {
    let (tx, rx) = channel();
    let n = Arc::new(AtomicUsize::new(0));
    let n2 = n.clone();
    let txs = tx;
    let _pty = Pty::spawn_with(SpawnConfig {
        cmd: "/bin/sh".into(),
        args: vec!["-c".into(), "echo hi".into()],
        cwd: None,
        env: vec![],
        cols: 80,
        rows: 24,
        pixel_width: 0,
        pixel_height: 0,
        login_shell: false,
        tx: txs,
        on_wake: Some(Box::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        })),
    })
    .unwrap();
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert!(exited.is_some());
    // Output wake(s) plus the Exited wake (which fires right after the send).
    let deadline = Instant::now() + Duration::from_secs(2);
    while n.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(n.load(Ordering::SeqCst) >= 2);
}

#[test]
fn large_output_is_chunked_and_complete() {
    let (tx, rx) = channel();
    let args = vec![
        "-c".to_string(),
        "i=0; while [ $i -lt 3000 ]; do echo 0123456789012345678901234567890123456789012345678901234567; i=$((i+1)); done; echo DONE".to_string(),
    ];
    let _pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let mut out = Vec::new();
    let mut exited = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !exited && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PtyEvent::Output(b)) => {
                assert!(b.len() <= grimalkin_pty::MAX_CHUNK);
                out.extend(b)
            }
            Ok(PtyEvent::Exited(_)) => exited = true,
            Err(_) => {}
        }
    }
    assert!(exited);
    assert!(contains(&out, "DONE"), "tail: {:?}", String::from_utf8_lossy(&out[out.len().saturating_sub(100)..]));
    assert!(out.len() > 3000 * 58);
}

#[test]
fn spawn_missing_program_errors() {
    let (tx, _rx) = channel();
    assert!(Pty::spawn("/definitely/not/a/program", &[], None, &[], 80, 24, tx).is_err());
}

#[test]
fn drop_kills_child() {
    let (tx, rx) = channel();
    let pty = Pty::spawn("cat", &[], None, &[], 80, 24, tx).unwrap();
    drop(pty);
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert!(exited.is_some());
}

#[test]
fn relative_shell_name_is_honored_as_login_shell() {
    // Regression: a bare name like "sh" used to be replaced by the passwd
    // shell because portable-pty rejects a non-absolute $SHELL.
    let (tx, rx) = channel();
    let pty = Pty::spawn("sh", &[], None, &[], 80, 24, tx).unwrap();
    pty.write_all(b"echo ARGV0=$0 SH=$SHELL\nexit\n").unwrap();
    let (out, _) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "SH=/"));
    let s = String::from_utf8_lossy(&out);
    assert!(s.contains("ARGV0=-sh"), "{s}");
}

#[test]
fn writes_never_block_on_a_child_that_is_not_reading() {
    let (tx, _rx) = channel();
    let args = vec!["5".to_string()];
    let mut pty = Pty::spawn("sleep", &args, None, &[], 80, 24, tx).unwrap();
    let big = vec![b'x'; 4 * 1024 * 1024];
    let t = Instant::now();
    pty.write_all(&big).unwrap();
    pty.flush().unwrap();
    assert!(t.elapsed() < Duration::from_secs(1), "write blocked for {:?}", t.elapsed());
    pty.kill().unwrap();
}

#[test]
fn write_after_exit_eventually_fails_instead_of_hanging() {
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "exit 0".to_string()];
    let pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert!(exited.is_some());
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut failed = false;
    while Instant::now() < deadline {
        if pty.write_all(b"x\n").is_err() {
            failed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(failed, "writer never reported the dead pty");
}

#[test]
fn kill_escalates_when_child_ignores_sighup() {
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "trap '' HUP; echo READY; while :; do sleep 1; done".to_string()];
    let mut pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let (_, _) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "READY"));
    pty.kill().unwrap();
    let (_, exited) = read_until(&rx, Duration::from_secs(10), |_| false);
    assert!(exited.is_some(), "child ignoring SIGHUP was never killed");
}

#[test]
fn no_output_after_exited() {
    // A background process that outlives the shell and keeps writing must not
    // produce Output events after Exited.
    let (tx, rx) = channel();
    let args = vec![
        "-c".to_string(),
        "(trap '' HUP; sleep 1; echo LATE) & exit 0".to_string(),
    ];
    let _pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut exited = false;
    let mut late = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(PtyEvent::Exited(_)) => assert!(!std::mem::replace(&mut exited, true), "two Exited"),
            Ok(PtyEvent::Output(_)) if exited => late = true,
            _ => {}
        }
    }
    assert!(exited);
    assert!(!late, "Output delivered after Exited");
}

#[test]
fn big_output_just_before_exit_is_not_truncated() {
    // Exit detection must not cut off output that is still being drained.
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "head -c 3000000 /dev/zero | tr '\\0' 'a'; echo END".to_string()];
    let _pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(PtyEvent::Output(b)) => out.extend(b),
            Ok(PtyEvent::Exited(_)) => break,
            Err(_) => {}
        }
    }
    assert!(out.len() >= 3_000_000, "only {} bytes", out.len());
    assert!(contains(&out[out.len() - 20..], "END"));
}

#[test]
fn concurrent_spawns_do_not_race_in_openpty() {
    // Regression: concurrent openpty() calls intermittently failed with ENXIO
    // on macOS.
    let handles: Vec<_> = (0..16)
        .map(|_| {
            std::thread::spawn(|| {
                for _ in 0..4 {
                    let (tx, _rx) = channel();
                    let args = vec!["-c".to_string(), "true".to_string()];
                    Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).expect("spawn failed");
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn shutdown_kills_child_that_ignores_hup() {
    let (tx, rx) = channel();
    let args = vec!["-c".to_string(), "trap '' HUP TERM; echo ready; while :; do sleep 1; done".to_string()];
    let mut pty = Pty::spawn("/bin/sh", &args, None, &[], 80, 24, tx).unwrap();
    let (out, _) = read_until(&rx, Duration::from_secs(10), |o| contains(o, "ready"));
    assert!(contains(&out, "ready"));
    let start = Instant::now();
    pty.shutdown(Duration::from_millis(300));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(!pty.is_alive());
}
