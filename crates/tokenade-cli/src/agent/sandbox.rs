//! Orchestrator-set limits for a spawned light-agent.
//!
//! Ported in spirit from opencode's permission model
//! (`packages/opencode/src/permission/index.ts` — allow/deny/ask rules
//! evaluated *before* a tool runs), but reduced to what a zero-cost
//! delegate actually needs: a directory jail, a capability set, provenance-
//! gated file writes/deletes, OS-sandboxed script execution, and a hard ban
//! on spawning further sub-agents.
//!
//! The design invariant: the ORCHESTRATOR (Claude) decides the box; the
//! light-agent can never widen it. Reads/writes/deletes are gated by
//! `gate_read`/`gate_write`/`gate_delete` before touching disk; shell/script
//! execution is confined by bubblewrap (see `tokenade execute --sandbox`).
//! Anything not explicitly allowed is denied — there is no interactive "ask"
//! tier because the light-agent runs headless.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Capabilities an orchestrator can grant/deny.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capability {
    /// Read file contents / list dirs (always within [`Sandbox::roots`]).
    Read,
    /// Run scripts / shell commands — executed OS-sandboxed (bubblewrap:
    /// read-only filesystem, writable `/tmp` only), so they cannot persist
    /// files. Persistent file changes go through Write/Delete.
    Shell,
    /// Reach the network (web search / fetch).
    Net,
    /// Create files, and rewrite files the agent itself created (WRITE tool).
    Write,
    /// Delete files the agent itself created (RM tool).
    Delete,
}

impl Capability {
    pub fn parse(s: &str) -> Option<Capability> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" => Some(Capability::Read),
            "shell" | "bash" | "exec" => Some(Capability::Shell),
            "net" | "web" | "network" => Some(Capability::Net),
            "write" => Some(Capability::Write),
            "delete" | "rm" => Some(Capability::Delete),
            _ => None,
        }
    }
}

/// The box an orchestrator draws around a light-agent.
#[derive(Clone, Debug)]
pub struct Sandbox {
    /// Directories the agent may read under (canonicalised). Empty ⇒ cwd.
    pub roots: Vec<PathBuf>,
    /// Granted capabilities. Read is implied; the rest are opt-in.
    pub caps: Vec<Capability>,
}

/// Why a tool call was refused — surfaced back to the light-agent as a
/// tool result so it can adapt instead of silently failing.
#[derive(Debug)]
pub enum Denial {
    Capability(Capability),
    OutsideJail(PathBuf),
    /// Tried to overwrite/delete a file the agent did not create this run.
    NotOwned(PathBuf),
}

impl std::fmt::Display for Denial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denial::Capability(c) => {
                write!(
                    f,
                    "DENIED: capability `{c:?}` not granted to this light-agent"
                )
            }
            Denial::OutsideJail(p) => write!(
                f,
                "DENIED: path `{}` is outside the allowed directory jail",
                p.display()
            ),
            Denial::NotOwned(p) => write!(
                f,
                "DENIED: `{}` was not created by you this run — you may only overwrite or delete \
                 files you created yourself",
                p.display()
            ),
        }
    }
}

impl Sandbox {
    /// Read-only jail rooted at `cwd`, no extra capabilities.
    #[allow(dead_code)] // used by tests + a convenient public constructor
    pub fn read_only(cwd: &Path) -> Sandbox {
        Sandbox {
            roots: vec![canon(cwd)],
            caps: vec![Capability::Read],
        }
    }

    /// The default box: jailed to `cwd` but productive — read, web, run
    /// scripts, and create/edit/delete files **it created itself** (Write and
    /// Delete are provenance-gated, not a licence to touch pre-existing
    /// files). This is what `tokenade agent` uses unless `--read-only` or an
    /// explicit `--cap` list narrows it.
    #[allow(dead_code)] // convenience constructor + used by tests
    pub fn default_box(cwd: &Path) -> Sandbox {
        Sandbox {
            roots: vec![canon(cwd)],
            caps: vec![
                Capability::Read,
                Capability::Net,
                Capability::Shell,
                Capability::Write,
                Capability::Delete,
            ],
        }
    }

    pub fn has(&self, c: Capability) -> bool {
        c == Capability::Read || self.caps.contains(&c)
    }

    /// The primary jail root (working dir for sandboxed execution).
    pub fn primary_root(&self) -> String {
        self.roots
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".to_string())
    }

    /// Is `p` inside the jail? Paths are canonicalised so `..` escapes and
    /// symlink hops can't smuggle the agent out of its roots.
    pub fn path_allowed(&self, p: &Path) -> bool {
        let cp = canon(p);
        let roots = if self.roots.is_empty() {
            // Defensive: an empty jail means cwd only.
            return std::env::current_dir()
                .map(|c| cp.starts_with(canon(&c)))
                .unwrap_or(false);
        } else {
            &self.roots
        };
        roots.iter().any(|r| cp.starts_with(r))
    }

    /// Gate a filesystem read.
    pub fn gate_read(&self, p: &Path) -> Result<(), Denial> {
        if !self.has(Capability::Read) {
            return Err(Denial::Capability(Capability::Read));
        }
        if !self.path_allowed(p) {
            return Err(Denial::OutsideJail(p.to_path_buf()));
        }
        Ok(())
    }

    /// Resolve `p` to the exact filesystem target a WRITE/RM would touch,
    /// airtight against symlink and `..` escapes:
    ///   - the PARENT directory is canonicalised (resolving any symlinked
    ///     dirs / `..`) and must lie inside a jail root;
    ///   - the leaf is joined onto that real parent and inspected with
    ///     `symlink_metadata` (lstat — never follows a final symlink).
    /// Returns the canonical target path (also the provenance key).
    fn resolve_target(&self, p: &Path) -> Result<PathBuf, Denial> {
        let deny = || Denial::OutsideJail(p.to_path_buf());
        let name = p.file_name().ok_or_else(deny)?;
        let parent = p
            .parent()
            .filter(|s| !s.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let cparent = parent.canonicalize().map_err(|_| deny())?;
        if !self.roots.iter().any(|r| cparent.starts_with(r)) {
            return Err(deny());
        }
        Ok(cparent.join(name))
    }

    /// Gate a WRITE. Requires the Write cap; the resolved target must be
    /// inside the jail; and if it already exists it must be a REGULAR file
    /// (never a symlink or dir) that the agent created this run (`owned`).
    /// Creating a brand-new file, or rewriting one it owns, is allowed.
    /// Returns the canonical path to write (and to record as owned).
    pub fn gate_write(&self, p: &Path, owned: &HashSet<PathBuf>) -> Result<PathBuf, Denial> {
        if !self.has(Capability::Write) {
            return Err(Denial::Capability(Capability::Write));
        }
        let target = self.resolve_target(p)?;
        if let Ok(md) = std::fs::symlink_metadata(&target) {
            // Exists: must be a plain file the agent owns. Symlinks/dirs/
            // special files are always refused (the agent never makes them).
            if md.file_type().is_symlink() || !md.is_file() || !owned.contains(&target) {
                return Err(Denial::NotOwned(target));
            }
        }
        Ok(target)
    }

    /// Gate an RM. Requires the Delete cap; the resolved target must exist,
    /// be a regular file (not a symlink/dir), lie in the jail, and have been
    /// created by the agent this run. Returns the canonical path to remove.
    pub fn gate_delete(&self, p: &Path, owned: &HashSet<PathBuf>) -> Result<PathBuf, Denial> {
        if !self.has(Capability::Delete) {
            return Err(Denial::Capability(Capability::Delete));
        }
        let target = self.resolve_target(p)?;
        match std::fs::symlink_metadata(&target) {
            Ok(md) if md.is_file() && !md.file_type().is_symlink() && owned.contains(&target) => {
                Ok(target)
            }
            _ => Err(Denial::NotOwned(target)),
        }
    }

    pub fn gate_net(&self) -> Result<(), Denial> {
        if !self.has(Capability::Net) {
            return Err(Denial::Capability(Capability::Net));
        }
        Ok(())
    }

    /// Capabilities + limits, spelled out for the agent's own system prompt
    /// so it knows exactly what it can and cannot do.
    pub fn describe(&self) -> String {
        let dirs = self
            .roots
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let mut can: Vec<&str> = vec!["read files"];
        let mut cannot: Vec<&str> = Vec::new();
        if self.has(Capability::Net) {
            can.push("search/fetch the web");
        } else {
            cannot.push("no web access");
        }
        if self.has(Capability::Shell) {
            can.push("run scripts & read-only shell commands");
        } else {
            cannot.push("no shell");
        }
        if self.has(Capability::Write) {
            can.push("create files & edit files you created (WRITE)");
        } else {
            cannot.push("no file writes");
        }
        if self.has(Capability::Delete) {
            can.push("delete files you created (RM)");
        } else {
            cannot.push("no deletes");
        }
        cannot.push("cannot touch files you did NOT create");
        cannot.push("cannot leave the jail dir");
        cannot.push("cannot mutate files from raw shell (use WRITE/RM)");
        cannot.push("cannot spawn sub-agents");
        format!(
            "jail=[{dirs}]\n  CAN: {}\n  CANNOT: {}",
            can.join(" · "),
            cannot.join(" · ")
        )
    }
}

/// Canonicalise, falling back to a lexical absolutisation if the path
/// doesn't exist yet (canonicalize fails on missing paths).
fn canon(p: &Path) -> PathBuf {
    if let Ok(c) = p.canonicalize() {
        return c;
    }
    // canonicalize() fails on paths that don't exist yet. Fall back to a
    // LEXICAL normalisation so `..` components still collapse — otherwise a
    // `<jail>/../../etc/x` that points at a non-existent file would keep the
    // jail prefix literally and slip past the `starts_with` check. Collapsing
    // `..` errs toward the safe (narrower) side.
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|c| c.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    normalize_lexical(&abs)
}

/// Collapse `.` and `..` components without touching the filesystem.
fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_delete_provenance() {
        let cwd = std::env::current_dir().unwrap();
        let sb = Sandbox::default_box(&cwd);
        let mut owned: HashSet<PathBuf> = HashSet::new();
        // A brand-new file may be created.
        let fresh = cwd.join("__agent_new_file_probe.tmp");
        assert!(sb.gate_write(&fresh, &owned).is_ok());
        // A pre-existing file the agent did NOT create can't be clobbered...
        assert!(sb.gate_write(&cwd.join("Cargo.toml"), &owned).is_err());
        // ...but once it's owned, rewriting it is fine.
        owned.insert(cwd.join("Cargo.toml").canonicalize().unwrap());
        assert!(sb.gate_write(&cwd.join("Cargo.toml"), &owned).is_ok());
        // Delete only works on owned paths (validation only — no removal here).
        assert!(sb.gate_delete(&cwd.join("Cargo.toml"), &owned).is_ok());
        assert!(sb
            .gate_delete(&cwd.join("Cargo.toml"), &HashSet::new())
            .is_err());
        // Jail still applies.
        assert!(sb.gate_write(Path::new("/etc/x"), &owned).is_err());
        // A read-only box can't write at all.
        let ro = Sandbox::read_only(&cwd);
        assert!(ro.gate_write(&fresh, &owned).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn write_delete_defeat_symlink_and_dotdot_escapes() {
        use std::os::unix::fs::symlink;
        let pid = std::process::id();
        let base = std::env::temp_dir().join(format!("tk_jail_{pid}"));
        let victim = std::env::temp_dir().join(format!("tk_victim_{pid}"));
        let _ = std::fs::create_dir_all(&base);
        std::fs::write(&victim, "secret").unwrap();
        let sb = Sandbox::default_box(&base);
        let owned = HashSet::new();

        // A symlink INSIDE the jail pointing at an outside victim must be
        // refused for both write and delete (leaf is a symlink → never owned).
        let link = base.join("link");
        let _ = symlink(&victim, &link);
        assert!(
            sb.gate_write(&link, &owned).is_err(),
            "symlink write refused"
        );
        assert!(
            sb.gate_delete(&link, &owned).is_err(),
            "symlink delete refused"
        );

        // `..` traversal out of the jail is refused (parent canonicalises out).
        let escape = base.join("..").join(victim.file_name().unwrap());
        assert!(sb.gate_write(&escape, &owned).is_err(), "..-escape refused");

        // The victim is untouched.
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "secret");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(&victim);
    }

    #[test]
    fn jail_confines_reads() {
        let sb = Sandbox::read_only(Path::new("."));
        let cwd = std::env::current_dir().unwrap();
        assert!(sb.path_allowed(&cwd.join("Cargo.toml")));
        assert!(!sb.path_allowed(Path::new("/etc/passwd")));
    }

    #[test]
    fn jail_blocks_dotdot_escape() {
        // A `..` that climbs out of the jail must be rejected even though
        // it's written relative to an allowed root.
        let cwd = std::env::current_dir().unwrap();
        let sb = Sandbox {
            roots: vec![cwd.clone()],
            caps: vec![Capability::Read],
        };
        assert!(!sb.path_allowed(&cwd.join("../../../etc/passwd")));
        assert!(sb.path_allowed(&cwd.join("src/../Cargo.toml")));
    }

    #[test]
    fn capability_parse_aliases() {
        assert_eq!(Capability::parse("bash"), Some(Capability::Shell));
        assert_eq!(Capability::parse("NET"), Some(Capability::Net));
        assert_eq!(Capability::parse("rm"), Some(Capability::Delete));
        assert_eq!(Capability::parse("nonsense"), None);
    }

    #[test]
    fn net_gated_by_cap() {
        let mut sb = Sandbox::read_only(Path::new("."));
        assert!(sb.gate_net().is_err());
        sb.caps.push(Capability::Net);
        assert!(sb.gate_net().is_ok());
    }
}
