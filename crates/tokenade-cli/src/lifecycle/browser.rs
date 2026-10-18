//! Opening a local page (the web dashboard) in the user's browser.

use super::*;

/// True when there's almost certainly no local browser to open — an SSH
/// session, or a Unix host with no X11/Wayland display. On such hosts the
/// OS launcher fails noisily (e.g. macOS prints
/// `RBSRequestErrorDomain Code=5 "Launch failed."` to stderr), so we skip
/// the attempt entirely and let the caller show the URL for manual opening.
pub(crate) fn is_headless() -> bool {
    if std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || std::env::var_os("SSH_CLIENT").is_some()
    {
        return true;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return true;
        }
    }
    false
}

/// Best-effort "open this URL in the user's browser". Non-fatal — the URL is
/// always printed too, so headless / SSH sessions still work.
pub(crate) fn open_browser(url: &str) -> bool {
    use std::process::Stdio;
    // No browser on a server — don't even spawn the launcher (it errors
    // loudly). The caller prints the URL so the user opens it manually.
    if is_headless() {
        return false;
    }
    // Silence the launcher's own stdout/stderr so a failed open never spams
    // the terminal (the macOS RBSRequestErrorDomain message, etc.).
    #[cfg(target_os = "macos")]
    let cmd = {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
    };
    #[cfg(target_os = "windows")]
    let cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        tokenade_core::process_util::configure_no_window(&mut c);
        c.spawn()
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = {
        use std::os::unix::process::CommandExt;
        std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own process group: `tokenade` exits right after, and a
            // terminal or agent tool that reaps the caller's group killed the
            // launcher before it had handed the page over — the tab never
            // opened while the command reported success (2026-10-04).
            .process_group(0)
            .spawn()
    };
    match cmd {
        // Dropping the handle leaves a long-running launcher alive.
        Ok(mut child) => launcher_handed_over(&mut child),
        Err(_) => false,
    }
}

/// **Did the launcher hand the page to a browser?** A successful SPAWN said
/// nothing: `xdg-open` with no handler, or a broken one, exits non-zero a
/// moment later, and the caller had already claimed the page was open — so it
/// skipped its terminal fallback. Wait briefly for the exit status. A launcher
/// still running after the wait has handed over (`firefox <url>` with no
/// running instance blocks for the browser's whole life): left to run.
fn launcher_handed_over(child: &mut std::process::Child) -> bool {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(3);
    let started = std::time::Instant::now();
    while started.elapsed() < WAIT {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => return false,
        }
    }
    true
}
