//! **Putting the new binary in place of the old one.**
//!
//! The archive carries only ONE file. It is extracted next to the live
//! binary, `fsync`ed, then `rename(2)`d over it. On Unix this works WHILE the
//! binary is running: the kernel keeps the old inode alive as long as a
//! descriptor holds it open. On Windows the old one is first renamed to
//! `.old`, overwritten at the next start.
//!
//! The swap is atomic or it does not happen: a half-written binary is the
//! only failure the user cannot get out of with this tool.

use super::*;

// ─────────────────────────────────────────────────────────────────────────
// Tarball extract + atomic binary swap.
//
// The tarball ships ONE file: `tokenade` (or `tokenade.exe`). We extract
// it next to the current binary, fsync, then rename onto the live path.
// On Unix this works while the binary is currently executing because the
// kernel keeps the old inode alive until last fd closes. On Windows we
// rename the old file to `.old` first; on next start, it's overwritten.
// ─────────────────────────────────────────────────────────────────────────

pub(crate) fn extract_and_swap(tarball: &[u8], exe_path: &Path) -> Result<(), String> {
    let parent = exe_path
        .parent()
        .ok_or_else(|| format!("exe_path has no parent: {}", exe_path.display()))?;
    if !is_writable(parent) {
        return Err(format!(
            "binary parent dir {} is not writable — likely a system-managed install (npm -g as root, Homebrew, apt, etc.). Update through that package manager instead (e.g. `npm update -g` / `brew upgrade tokenade`).",
            parent.display()
        ));
    }
    // Inter-process swap lock (audit: a manual `tokenade upgrade` racing the
    // auto-background child could delete the other's `.old` safety net on
    // Windows mid-rollback). O_EXCL lockfile; a stale lock (>10 min, crashed
    // process) is broken.
    let lock_path = parent.join(".tokenade-swap.lock");
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(_) => {}
        Err(_) => {
            let stale = fs::metadata(&lock_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
                .map(|age| age.as_secs() > 600)
                .unwrap_or(true);
            if stale {
                let _ = fs::remove_file(&lock_path);
                let _ = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_path);
            } else {
                return Err("another tokenade update is already in progress".to_string());
            }
        }
    }
    // Everything below must release the lock on ALL exits.
    let out = extract_and_swap_locked(tarball, exe_path, parent);
    let _ = fs::remove_file(&lock_path);
    out
}

pub(crate) fn extract_and_swap_locked(
    tarball: &[u8],
    exe_path: &Path,
    parent: &Path,
) -> Result<(), String> {
    // Strip gzip then tar.
    let raw = gunzip_inline(tarball)?;
    let entry = read_single_tar_entry(&raw)?;

    let exe_name = exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("tokenade");
    let tmp_path = parent.join(format!(".{exe_name}.new.{}", std::process::id()));

    {
        let mut f = fs::File::create(&tmp_path).map_err(|e| format!("create tmp: {e}"))?;
        if let Err(e) = f.write_all(&entry) {
            // Audit: a disk-full mid-write leaked a ~50 MB partial binary
            // next to the live one, worsening the very condition that
            // caused the failure.
            drop(f);
            let _ = fs::remove_file(&tmp_path);
            return Err(format!("write tmp: {e}"));
        }
        f.sync_all().ok();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = f.set_permissions(fs::Permissions::from_mode(0o755));
        }
    }

    // macOS: make the freshly-written binary runnable BEFORE the probe.
    // On Apple Silicon the kernel SIGKILLs (exit 137) any arm64 Mach-O that
    // carries no code signature at all — an unsigned binary never reaches
    // main(), so tokenade cannot self-heal once installed. The distributed
    // binary is ad-hoc (linker-)signed today, but any byte mutation, a
    // strip-after-sign, or a future cross-compiled darwin build would drop
    // that signature and every arm64 auto-update would then fail its probe
    // and silently refuse (freezing the user forever, masked by the hook's
    // fail-open). Belt-and-suspenders: strip com.apple.quarantine (harmless
    // if absent) and ad-hoc re-sign when the signature is missing/invalid.
    ensure_macos_runnable(&tmp_path);

    // PRE-swap health probe (audit: sha256+signature prove transfer
    // integrity, not that the build RUNS here — the glibc-2.38-on-Debian-12
    // incident shipped a perfectly signed binary that couldn't start).
    // `--version` on the extracted tmp binary must succeed before we touch
    // the live one; otherwise the update is refused and the user keeps a
    // working install.
    //
    // Exec'ing a binary we just wrote is racy in a multithreaded process: a
    // sibling thread that forks while our write fd was open inherits it, and the
    // kernel then refuses the exec with ETXTBSY. Left unretried that aborts a
    // perfectly good update and pins the user to an old build — the freeze this
    // very function exists to avoid. See `retry_while_text_file_busy`.
    let probe_status = crate::process_util::retry_while_text_file_busy(|| {
        let mut probe = std::process::Command::new(&tmp_path);
        probe
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::process_util::configure_no_window(&mut probe);
        probe.status()
    });
    match probe_status {
        Ok(st) if st.success() => {}
        Ok(st) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!(
                "new binary failed its startup probe (exit {st}) — update refused, keeping the current version"
            ));
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(format!(
                "new binary failed to start on this system ({e}) — update refused, keeping the current version"
            ));
        }
    }

    #[cfg(windows)]
    let old_path = {
        // Rename live binary out of the way first (works on Win10+).
        let old_path = parent.join(format!("{exe_name}.old"));
        let _ = fs::remove_file(&old_path);
        if exe_path.exists() {
            fs::rename(exe_path, &old_path).map_err(|e| format!("rename live → .old: {e}"))?;
        }
        old_path
    };

    let swap_result = fs::rename(&tmp_path, exe_path);
    if let Err(e) = swap_result {
        // #957 — Windows rollback: if the final tmp→exe rename fails
        // (AV scanner, locked file, transient FS error), the live binary
        // was already renamed to `.old` above, leaving the user with NO
        // binary at `exe_path`. Without a rollback the install is
        // effectively bricked until the user manually shuffles files.
        // Try to restore `.old → exe_path` so the user keeps the working
        // older version. The original error is still surfaced so the
        // updater reports the swap failure (the rollback is best-effort
        // status info).
        let _ = fs::remove_file(&tmp_path);
        #[cfg(windows)]
        {
            if old_path.exists() && !exe_path.exists() {
                let rollback = fs::rename(&old_path, exe_path);
                return Err(match rollback {
                    Ok(()) => format!("atomic swap failed (old binary restored): {e}"),
                    Err(re) => format!(
                        "atomic swap failed: {e}; ALSO rollback of .old → exe failed: {re} — \
                         manually rename {} → {} to recover",
                        old_path.display(),
                        exe_path.display(),
                    ),
                });
            }
        }
        return Err(format!("atomic swap: {e}"));
    }

    // #1025 — Windows: best-effort cleanup of `.old`. The pre-swap
    // block renamed the live binary out of the way (Windows can't
    // overwrite a running .exe); after the new binary is in place,
    // the `.old` shadow is no longer needed. Pre-fix it lingered
    // until the NEXT upgrade (which removes it at line 651 before
    // the next rename), so a user who upgrades once and never again
    // keeps a stale ~50MB `tokenade.exe.old` in their install dir
    // forever. If removal fails (AV scanner, file-lock on the
    // running process image), let it slide — the next upgrade
    // will clear it; this is purely tidying.
    #[cfg(windows)]
    {
        let _ = fs::remove_file(&old_path);
    }

    // This process stays on the old inode. The temp file's `--version` probe
    // refuses to write the config (`.tokenade.new.<pid>`). The live path
    // already holds the new binary: we launch it to heal the global lectures
    // right away — `tokenade upgrade` AND the daily check
    // (`upgrade --auto-background`). `--no-auto-update`: the child does not
    // start another download. Null outputs: the daily path never speaks on
    // stdout.
    let _ = crate::process_util::retry_while_text_file_busy(|| {
        let mut cmd = std::process::Command::new(exe_path);
        cmd.args(["--version", "--no-auto-update"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        crate::process_util::configure_no_window(&mut cmd);
        cmd.status()
    });

    Ok(())
}

/// Ensure a just-written Mach-O binary can actually execute on macOS.
///
/// Two macOS-specific hazards, both silent:
///   1. Apple Silicon requires *at least* an ad-hoc code signature; the
///      kernel SIGKILLs an entirely-unsigned arm64 binary at exec (exit
///      137, "killed: 9") before it runs a single instruction.
///   2. `com.apple.quarantine` (set by browser/zip downloads, some AV) can
///      make Gatekeeper reject a GUI launch of the binary.
///
/// Best-effort: strip quarantine, and if the signature is missing/invalid,
/// apply an ad-hoc signature (`codesign --sign -`). A still-broken binary is
/// caught by the caller's `--version` probe, which refuses the swap. No-op on
/// non-macOS.
#[allow(unused_variables)]
pub(crate) fn ensure_macos_runnable(path: &Path) {
    #[cfg(target_os = "macos")]
    {
        let run = |args: &[&std::ffi::OsStr]| -> bool {
            std::process::Command::new(args[0])
                .args(&args[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        use std::ffi::OsStr;
        // 1. Strip quarantine (recursive; harmless if the attr is absent).
        let _ = run(&[
            OsStr::new("/usr/bin/xattr"),
            OsStr::new("-dr"),
            OsStr::new("com.apple.quarantine"),
            path.as_os_str(),
        ]);
        // 2. Ad-hoc sign only when the current signature does not verify —
        //    re-signing an already-valid linker signature is wasted work and
        //    could disturb a properly notarized build we might ship later.
        let verified = run(&[
            OsStr::new("/usr/bin/codesign"),
            OsStr::new("--verify"),
            path.as_os_str(),
        ]);
        if !verified {
            let _ = run(&[
                OsStr::new("/usr/bin/codesign"),
                OsStr::new("--force"),
                OsStr::new("--sign"),
                OsStr::new("-"),
                path.as_os_str(),
            ]);
        }
    }
}

pub(crate) fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".tokenade-write-probe.{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Decompress gzip-wrapped bytes. We use `flate2` (already a transitive
/// workspace dep). Cap output size at BINARY_MAX_BYTES.
pub(crate) fn gunzip_inline(bytes: &[u8]) -> Result<Vec<u8>, String> {
    use flate2::read::GzDecoder;
    let mut out = Vec::with_capacity(bytes.len() * 3);
    GzDecoder::new(bytes)
        .take(BINARY_MAX_BYTES)
        .read_to_end(&mut out)
        .map_err(|e| format!("gunzip: {e}"))?;
    Ok(out)
}

/// Minimal POSIX tar reader: pull the FIRST regular file. We don't care
/// about preserving names or metadata — the manifest already pinned the
/// SHA-256 of the whole tarball. The release pipeline ships exactly one
/// binary per tarball; anything else is a misconfigured release and we
/// fail closed.
pub(crate) fn read_single_tar_entry(raw: &[u8]) -> Result<Vec<u8>, String> {
    // POSIX tar: each entry is a 512-byte header + ceil(size/512) data
    // blocks. typeflag at offset 156, size at offset 124 (12-byte octal).
    const BLOCK: usize = 512;
    let mut off = 0;
    while off + BLOCK <= raw.len() {
        let hdr = &raw[off..off + BLOCK];
        // All-zero block = end of archive.
        if hdr.iter().all(|&b| b == 0) {
            break;
        }
        // Tar's size field is 12 bytes of null/space-padded octal. Both
        // GNU and BSD tar emit either "00000000005\0" or "  00000005  ".
        // Strip BOTH \0 and ASCII whitespace from either end before
        // parsing — `.trim()` alone leaves trailing nulls, and earlier
        // trim_end_matches('\0') alone left trailing spaces.
        let size_field =
            std::str::from_utf8(&hdr[124..124 + 12]).map_err(|_| "tar size field not utf-8")?;
        let cleaned = size_field.trim_matches(|c: char| c == '\0' || c.is_ascii_whitespace());
        let size = if cleaned.is_empty() {
            0
        } else {
            u64::from_str_radix(cleaned, 8).map_err(|e| format!("tar size parse: {e}"))?
        };
        let typeflag = hdr[156];
        // 0 / '0' = regular file.
        if typeflag == 0 || typeflag == b'0' {
            let data_start = off + BLOCK;
            let data_end = data_start + size as usize;
            if data_end > raw.len() {
                return Err("tar: truncated data block".into());
            }
            return Ok(raw[data_start..data_end].to_vec());
        }
        // Skip non-regular entries (long-name extensions, dirs, etc.).
        let blocks = size.div_ceil(BLOCK as u64) as usize;
        off += BLOCK + blocks * BLOCK;
    }
    Err("tar: no regular file entry found".into())
}
