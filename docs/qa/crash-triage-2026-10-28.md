# Crash triage: adversarial QA run, 2026-10-28

Source: 11 reports in `~/Library/Logs/DiagnosticReports/grimalkin-2026-10-28-17313*..17334*.ips`
(all SIGABRT / EXC_CRASH, all on the main thread, 17:31:29 - 17:33:47 CDT).
Attack transcripts: `.claude/projects/.../subagents/workflows/wf_eeb912c0-bed/agent-*.jsonl`
(timestamps there are UTC; local = UTC - 5h).

Key points

* All 11 crashes come from one attacker (`attack:config`, agent a7a7dd..) and one (`attack:code`, agent a80950..)
  feeding hostile `skin.toml` files to `--skin`. No VT/protocol/pty/graphics/input attack crashed the app.
* The `.ips` `asi` field is only `abort() called` in every report. Panic text is NOT in any report; it was recovered
  from the agents' captured stderr. Crash reports and stderr were matched by process launch time and by main-thread id
  (`thread 'main' (NNNNNNN)` in stderr == `threads[0].id` in the report).
* Four unique panic signatures (debug builds, integer-overflow checks) plus one native-exception signature
  (release build, no Rust panic).

## Summary table

| # | Signature | Build | Count | Trigger (skin.toml field) | Source |
|---|-----------|-------|-------|---------------------------|--------|
| E | `fatal runtime error: Rust cannot catch foreign exceptions` (ObjC NSException through Rust frame, in winit run-loop observer) | release (stripped) | 3 | skin with `[shade] height = 4294967295`, then window-shade (double-click titlebar / shade button) | `crates/gui/src/app.rs:1604` (`shade_bar`) -> `apply_plan` `app.rs:1634-1649` |
| B | `renderer.rs:616:40: attempt to add with overflow` | debug | 3 | `frame.viewport_inset` left/right/top/bottom = 4294967295 or 2147483647 | `crates/gui/src/renderer.rs:616` |
| A | `skin/parse.rs:204:12: attempt to add with overflow` | debug | 2 | `frame.slices.left = 4294967295` | `crates/gui/src/skin/parse.rs:204` |
| C | `skin/hit.rs:64:18: attempt to add with overflow` | debug | 2 | button `rect` x/y/w/h = 2147483647 (and titlebar rect x=2e9,w=2e9) | `crates/gui/src/skin/hit.rs:64` (`PxRect::intersect`) |
| D | `skin/hit.rs:82:28: attempt to subtract with overflow` | debug | 1 | titlebar text `w = -2147483648`, x/y = 2147483647 | `crates/gui/src/skin/hit.rs:82` (`resolve_rect`) |

Severity ranking: E > B > A > C > D (E ships in release and is a native exception; A-D only fire with overflow
checks, i.e. debug builds, but see "release behaviour" below).

## Per-crash table

| Report (grimalkin-2026-10-28-...) | Time CDT | pid | Build (UUID) | Sig | Attack run |
|---|---|---|---|---|---|
| 173135.000 | 17:31:30 | 38712 | debug 45f4ad5d | A | config: skin `ovf1` (slices 4294967295) |
| 173135.0002 | 17:31:32 | 38743 | debug 45f4ad5d | B | config: skin `ovf2` (viewport_inset u32::MAX) |
| 173136 | 17:31:33 | 38823 | debug 45f4ad5d | B | config: skin `ovf3` (viewport_inset i32::MAX) |
| 173138 | 17:31:37 | 39053 | debug 45f4ad5d | C | config: skin `ovf5` (close button rect i32::MAX) |
| 173146.000 | 17:31:45 | 39563 | debug 45f4ad5d | D | config: skin `ovf9` (title text w=i32::MIN) |
| 173238 | 17:32:36 | 42202 | release 2eb4f79b | E | config: release loop `runs.sh` with skin `ovf6` + scripted `dblclick` |
| 173255 | 17:32:54 | 43157 | release 2eb4f79b | E | config: `GRIMALKIN_SCALE_FACTOR=2 runs.sh ... "...;100:click;100:dblclick;..." --skin ovf6` (stderr captured, see below) |
| 173333 | 17:33:28 | 44806 | debug ab60d447 | A | code: skin `ovf_slices` |
| 173333.000 | 17:33:29 | 44967 | debug ab60d447 | B | code: skin `ovf_vi` |
| 173333.0002 | 17:33:29 | 44974 | debug ab60d447 | C | code: skin `rect_ovf` |
| 173348.000 | 17:33:47 | 45882 | release 2eb4f79b | E | config: `runs.sh 20 "800:move:50,5;200:dblclick;..." --skin ovf6` |

Build identification: `procPath` is anonymised in every report (`/Users/USER/*/grimalkin`), so the build was
determined from `usedImages[0]`: size 55.7 MB with Rust symbols = debug build (45f4ad5d is an earlier debug build no
longer on disk; ab60d447 == current `target/debug/grimalkin`); size 18.0 MB with no symbols = release
(2eb4f79b == `target/release/deps/grimalkin-0de6a4e8cab96035`, `strip = true`).

## Exact panic / fatal messages found (from agent stderr)

```
thread 'main' (5150450) panicked at crates/gui/src/skin/parse.rs:204:12:
attempt to add with overflow
thread 'main' (5150450) panicked at library/core/src/panicking.rs:230:5:
panic in a function that cannot unwind
```
```
thread 'main' (5150579) panicked at crates/gui/src/renderer.rs:616:40:
attempt to add with overflow
... panic in a function that cannot unwind
```
```
thread 'main' (5151515) panicked at crates/gui/src/skin/hit.rs:64:18:
attempt to add with overflow
... panic in a function that cannot unwind
```
```
thread 'main' (5152878) panicked at crates/gui/src/skin/hit.rs:82:28:
attempt to subtract with overflow
... panic in a function that cannot unwind
```
```
2026-10-28 17:32:53.950 grimalkin[43157:5162762] Warning: Window move completed without beginning
[... INFO grimalkin::app] debug script: dblclick
fatal runtime error: Rust cannot catch foreign exceptions, aborting
```
(`parse.rs:204:12`, `renderer.rs:616:40` and `hit.rs:64:18` also reproduced in the later debug build with ab60d447.)

The second line, "panic in a function that cannot unwind", is the abort mechanism: the overflow panic occurs inside
a winit Objective-C callback (`app_did_finish_launching`, i.e. `resumed` -> skin load / window + renderer setup), which
is `extern "C"`, so Rust turns the unwinding panic into `panic_cannot_unwind` -> `abort()`.

## Does any crash lack a panic message / is any a true native crash?

* Reports 1-5 and 8-10 (sigs A-D, 8 crashes): Rust panics. Not in the `.ips` (asi has only `abort() called`), but the
  panic location is certain: top frames are `panic_cannot_unwind <- app_did_finish_launching` (symbolicated), and
  stderr in the transcripts gives file:line for each, matched by thread id.
* Reports 6, 7, 11 (sig E, 3 crashes): NO Rust panic message. The only text is
  `fatal runtime error: Rust cannot catch foreign exceptions`, which means an Objective-C exception (NSException,
  almost certainly from AppKit via winit's `request_inner_size` / `set_min_inner_size` / `set_outer_position`) was
  thrown through a Rust frame. This is the worse class: not a Rust bug that overflow checks catch, not a wgpu/Metal
  validation error, and the NSException name/reason was not captured anywhere (no stderr dump of it, and the release
  binary is stripped). The stripped release stack is identical across all three: abort <- Rust foreign-exception abort
  <- winit run-loop observer callback (`__CFRUNLOOP_IS_CALLING_OUT_TO_AN_OBSERVER_CALLBACK_FUNCTION__`)
  <- `__CFRunLoopDoObservers`. No wgpu/Metal frames are in any crashed thread.
* No crash was a segfault/EXC_BAD_ACCESS inside wgpu/Metal/objc code. wgpu "Validation Error" lines were seen (skin
  `sh100000`, shade height 100000) but those are logged, not fatal.

## Signature details

### E. Foreign (ObjC) exception on window-shade with huge shade height (3 crashes, release) - HIGH
* Trigger: skin whose `[shade] height = 4294967295` (skin `ovf6`; otherwise a normal skin), then `dblclick`
  (or shade button) on the titlebar. Startup alone does not crash (the debug-loop ovf6 run without a click exited 0).
  Reproduced 3/3 in release; `shx2147483647` and `shx1000000000` (smaller heights) did NOT crash, `sh100000` only
  produced wgpu validation errors.
* Mechanism (inferred; the exception text was not captured): `shade_bar()` at `app.rs:1604` computes
  `sk.skin.shade_height() * sk.scale()` in u32. With scale 2 this wraps in release to 4294967294 (the transcript logs
  `bar 4294967294`; in a debug build it would be a sig-A-style overflow panic at this line instead). `apply_plan`
  (`app.rs:1634-1649`) passes that as the `PhysicalSize` height to `set_min_inner_size` and `request_inner_size`;
  winit converts to a ~2.1e9-point logical size, and AppKit throws an NSException that Rust cannot unwind through.
* Fix direction: (1) clamp `ShadeDef.height` at parse time (`parse.rs:~165`, next to the existing font_px/border
  clamps) to a sane maximum, e.g. 256, and clamp `shade_height()` (`skin/mod.rs:297`) too; (2) use `saturating_mul`
  in `shade_bar` and clamp the result to the work-area/monitor height; (3) clamp all sizes passed to winit (also in
  `debug_script` `size:WxH`, `app.rs:1379`) to the monitor size; (4) defence in depth: get the NSException reason by
  reproducing once under lldb with `break set -n objc_exception_throw` (do not run on the user's session), and
  consider wrapping AppKit window calls so a bad geometry is rejected rather than thrown.

### B. renderer.rs:616:40 add overflow (3 crashes, debug)
* Trigger: `frame.viewport_inset` fields of 4294967295 (ovf2) or 2147483647 (ovf3, ovf_vi) - u32 sum
  `self.origin.0 + self.inset.0 + 2 * self.pad` in `grid_size_for` overflows.
* Fix: clamp `viewport_inset` in `SkinDef` parsing (like `resize.border.min(64)` at `parse.rs:197`) and make
  `grid_size_for` / `surface_size_for` use `saturating_add`/`saturating_mul`; validate inset against frame image size.

### A. skin/parse.rs:204:12 add overflow (2 crashes, debug)
* Trigger: `frame.slices.left = 4294967295` (ovf1, ovf_slices); `SkinDef::validate` does
  `s.left + s.right > img_w` in u32.
* Fix: `s.left.checked_add(s.right).is_none_or(|v| v > img_w)` (or compare in u64), same for top/bottom; return the
  existing `Err("frame slices ... exceed ...")`. This is the cleanest fix (the validator is meant to reject exactly this).

### C. skin/hit.rs:64:18 add overflow (2 crashes, debug)
* Trigger: button/titlebar `rect` with i32::MAX / 2e9 x,y,w,h (ovf5 close button, rect_ovf). `PxRect::intersect`
  computes `self.x + self.w` in i32.
* Fix: do all PxRect arithmetic with `saturating_add`/`i64`, and clamp `RectDef` coordinates at parse time (e.g.
  +/-16384) or reject rects outside the frame image in `validate`.

### D. skin/hit.rs:82:28 subtract overflow (1 crash, debug)
* Trigger: `w = -2147483648` (i32::MIN) in `resolve_rect`: `win + l * s - p`.
* Fix: same as C - saturating/i64 math in `resolve_rect`, and clamp or reject extreme RectDef values on load.

## Cross-cutting observations

* Release behaviour for A-D: overflow checks are off, so ovf1-ovf5, ovf7-ovf9 exit 0 in release (wrapped values, no
  abort). They are latent: wrapped sizes can produce garbage geometry (and sig E shows how a wrapped value reaches
  AppKit). Fixing at the parse/validation layer fixes all five signatures; every unchecked u32/i32 field in
  `skin.toml` should be bounded at load.
* Any panic inside a winit callback aborts the whole process (panic in `extern "C"`), with no recoverable error
  and a macOS "quit unexpectedly" dialog. Consider `std::panic::catch_unwind` around event-loop handlers, or
  rejecting bad skins with an error dialog and fallback to the built-in skin before the window is created. Also
  consider putting the panic message into the crash report (e.g. a panic hook that logs to a file) so future `.ips`
  files are self-describing.
* The 11 dialogs were all launched by two QA agents' skin-fuzzing runs; nothing in the vt/graphics/pty/input/render/
  protocol/process attacks aborted the app.
