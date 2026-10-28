//! Windows backend: a native ConPTY implementation.
//!
//! `cmdline` holds the pure string/UTF-16 helpers (command-line quoting,
//! environment blocks, program lookup) and is compiled on every platform
//! under `cfg(test)` so it can be unit-tested off Windows.

#[cfg(any(windows, test))]
pub(crate) mod cmdline;
#[cfg(windows)]
mod conpty;

#[cfg(windows)]
pub(crate) use conpty::open;
