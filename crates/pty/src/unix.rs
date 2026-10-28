//! Unix backend: `forkpty` via portable-pty.

use std::io;
use std::sync::Mutex;

use portable_pty::{native_pty_system, Child, ChildKiller, MasterPty, PtySize};

use crate::{build_command, ChildWait, Killer, Master, Spawned, SpawnConfig};

struct UnixMaster(Box<dyn MasterPty + Send>);

impl Master for UnixMaster {
    fn resize(&self, cols: u16, rows: u16, pixel_w: u16, pixel_h: u16) -> io::Result<()> {
        self.0
            .resize(PtySize {
                rows,
                cols,
                pixel_width: pixel_w,
                pixel_height: pixel_h,
            })
            .map_err(io::Error::other)
    }
}

struct UnixChild(Box<dyn Child + Send + Sync>);

impl ChildWait for UnixChild {
    fn wait(&mut self) -> Option<u32> {
        self.0.wait().ok().map(|s| s.exit_code())
    }
}

struct UnixKiller(Box<dyn ChildKiller + Send + Sync>);

impl Killer for UnixKiller {
    fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }
}

pub(crate) fn open(cfg: &SpawnConfig) -> io::Result<Spawned> {
    let size = PtySize {
        rows: cfg.rows.max(1),
        cols: cfg.cols.max(1),
        pixel_width: cfg.pixel_width,
        pixel_height: cfg.pixel_height,
    };
    // libc::openpty is not thread-safe on macOS (it uses ptsname(3)); two
    // concurrent calls intermittently fail with ENXIO. Serialize them.
    static OPENPTY_LOCK: Mutex<()> = Mutex::new(());
    let open_guard = OPENPTY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let pair = native_pty_system().openpty(size).map_err(io::Error::other)?;

    let builder = build_command(cfg);
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(io::Error::other)?;
    // Must drop the slave so the reader sees EOF once the child is gone.
    drop(pair.slave);
    drop(open_guard);

    let pid = child.process_id();
    let killer = child.clone_killer();
    let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
    let writer = pair.master.take_writer().map_err(io::Error::other)?;

    Ok(Spawned {
        master: Box::new(UnixMaster(pair.master)),
        reader,
        writer,
        child: Box::new(UnixChild(child)),
        killer: Box::new(UnixKiller(killer)),
        pid,
    })
}
