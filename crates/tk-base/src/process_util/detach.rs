//! **Launch a process that outlives us** — or that does not.
//!
//! Two opposite needs in the same file, on purpose: an update
//! check must survive the end of our process, a Windows
//! console window must never appear for a child whose output we
//! read.
//!
//! Both are settled in the same place — the creation flags — and
//! splitting them would let a caller set one while forgetting the other.

// Only the Unix arm reaches into the parent module.
#[cfg_attr(windows, allow(unused_imports))]
use super::*;

/// Spawn `exe args…` as a FULLY DETACHED background process and return
/// immediately (never waits). Best-effort: any spawn error is surfaced to
/// the caller, but the parent is never blocked or panicked.
///
/// **Why this exists**: the updater's `upgrade --auto-background` child and
/// the reporter's `reporter-run` child each do SECONDS of work (a ~50 MB
/// download, or verify + ledger-parse + upload) while their parent
/// is usually a hook invocation that exits in milliseconds. A plain
/// `Command::spawn()` leaves the child in the PARENT's session and process
/// group, so it is killed by `SIGHUP` (terminal close) or a `killpg` of the
/// parent's group (agent harnesses that reap the whole process tree) long
/// before the network work finishes — the exact "update never completes"
/// failure. Detaching moves the child out of that blast radius.
///
/// - **Unix**: `stdin/out/err = null` + `setsid()` in `pre_exec`, giving the
///   child a fresh session with no controlling terminal → immune to `SIGHUP`
///   and to a `killpg` of the parent's group. Only `setsid` runs in the
///   forked child (async-signal-safe); its result is ignored (it fails
///   harmlessly with `EPERM` if we're already a group leader).
/// - **Windows**: `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP |
///   CREATE_NO_WINDOW` creation flags → no inherited console, own process
///   group, no flashed window.
pub fn spawn_fully_detached(exe: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    spawn_fully_detached_in(exe, args, None)
}

/// [`spawn_fully_detached`], in a chosen working directory.
///
/// The disk GC needs one: several of its sweeps are scoped to the PROJECT
/// (`<root>/.tokenade/reads/`), and a detached child inherits the parent's cwd,
/// which for a hook is wherever the agent happened to be.
pub fn spawn_fully_detached_in(
    exe: &std::path::Path,
    args: &[&str],
    cwd: Option<&std::path::Path>,
) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    configure_detached(&mut cmd);
    cmd.spawn().map(|_child| ())
}

/// Apply the OS-specific detachment settings to an already-built `Command`.
/// Split out so callers that need custom stdio/env can reuse the exact same
/// detachment, and so it's unit-testable in isolation.
#[cfg(unix)]
pub fn configure_detached(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `pre_exec` runs in the forked child before `exec`. Only
    // async-signal-safe work is allowed here — we call ONLY `setsid()`
    // (a single syscall) and nothing else. Its return is ignored: EPERM
    // (already a session/group leader) is harmless, and we must return
    // `Ok(())` unconditionally so the exec proceeds.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

/// Windows creation flags: DETACHED_PROCESS (0x8) | CREATE_NEW_PROCESS_GROUP
/// (0x200) | CREATE_NO_WINDOW (0x0800_0000).
#[cfg(windows)]
pub const DETACHED_CREATION_FLAGS: u32 = 0x0000_0008 | 0x0000_0200 | 0x0800_0000;

/// Apply the OS-specific detachment settings to an already-built `Command`.
#[cfg(windows)]
pub fn configure_detached(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(DETACHED_CREATION_FLAGS);
}

/// Windows `CREATE_BREAKAWAY_FROM_JOB` (0x0100_0000).
#[cfg(windows)]
pub const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Start a child that must outlive the JOB its parent belongs to.
///
/// [`spawn_fully_detached`] detaches from the console and the process group,
/// which is everything a child needs to survive a shell. It is not enough to
/// survive a Windows **job object**: Task Scheduler puts each task instance in
/// one with kill-on-close, so a child started by a short-lived task action dies
/// the moment that action returns — detached or not.
///
/// Measured on the Windows guest 2026-08-20: with the scheduled task's action
/// changed to a one-second starter, the proxy it spawned was gone before the
/// next probe and `proxykill` read `Ko` where the long-running action had at
/// least read `Ok` after 220 s.
///
/// The flag is attempted and not assumed. `CREATE_BREAKAWAY_FROM_JOB` fails
/// with `ERROR_ACCESS_DENIED` when the job does not carry
/// `JOB_OBJECT_LIMIT_BREAKAWAY_OK`, and that is a normal configuration rather
/// than an error to report — so a refusal falls back to the plain detached
/// spawn, which is what every other caller gets.
///
/// No-op distinction off Windows: a unix child already survives its parent.
pub fn spawn_outliving_its_job(exe: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new(exe);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(DETACHED_CREATION_FLAGS | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
    }
    spawn_fully_detached(exe, args)
}

/// Windows `CREATE_NO_WINDOW` (0x0800_0000).
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Stop an internal child from FLASHING a console window on Windows.
///
/// tokenade shells out to console-subsystem programs (`cmd.exe`, `sh`,
/// `powershell`, `reg`, `curl`, `rg`, the wrapped real command, …). When
/// the tokenade process spawning them has **no console of its own** —
/// the normal case for a detached reporter/updater child
/// ([`configure_detached`] sets `CREATE_NO_WINDOW`, so it is windowless)
/// **or** a tokenade launched by a GUI agent that used `windowsHide` —
/// Windows allocates a *fresh* console for each such child and paints its
/// window, which flashes on screen and vanishes as the child exits. In a
/// hook-driven session that fires per prompt / per tool-call, so the user
/// sees a stream of console windows opening and closing.
///
/// Every internal hot-path spawn either pipes/nulls its stdio or is gated
/// on the absence of a real terminal, so no *visible* console is ever
/// needed; `CREATE_NO_WINDOW` suppresses the flash without touching the
/// inherited stdin/stdout/stderr handles (so pipes, exit codes and
/// captured output are unaffected). No-op off Windows.
#[cfg(windows)]
pub fn configure_no_window(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// No-op on non-Windows: there is no console-window concept to suppress.
#[cfg(not(windows))]
pub fn configure_no_window(_cmd: &mut std::process::Command) {}

/// Put a wrapped child in its OWN process group so the whole tree can be
/// signalled as a unit. Pairs with [`forward_termination_to`]; no-op off unix.
///
/// The group is the point. Signalling the direct child kills `sh` and leaves
/// its pipeline running — the same mistake one level down, and exactly how a
/// detached browser once outlived the test that spawned it.
#[cfg(unix)]
pub fn configure_own_group(cmd: &mut std::process::Command) {
    if terminal_owns_the_group() {
        return;
    }
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}
