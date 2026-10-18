//! **One rule per program** — the flag that should have been asked for.
//!
//! A compactor works AFTER the fact; here the terse output is requested from the
//! start. `git status --porcelain=v2 --branch` is ~5× smaller before anything
//! even reads it, and the compactor still applies on top of it.
//!
//! The rules are CONSERVATIVE by construction: never a change of
//! semantics (`pytest -q` is display, `pytest -x` is not), and never
//! on top of a flag the user already wrote — hence `already_has`,
//! which is half of every rule.

use super::*;

/// True when the user already passed one of `flags`, bare or as `--flag=value`.
///
/// Was a closure inside `apply_rule`; the per-program rewrites below need it
/// too, and a closure cannot be shared with them.
pub(crate) fn already_has(args: &[&str], flags: &[&str]) -> bool {
    args.iter().any(|a| {
        flags
            .iter()
            .any(|f| a == f || a.starts_with(&format!("{f}=")))
    })
}

/// `<env prefix><prog> <new args…> <original args…>` — the rewritten command.
pub(crate) fn prepend_args(
    env_prefix: &str,
    prog: &str,
    args: &[&str],
    new_args: &[&str],
) -> String {
    let mut out = String::with_capacity(env_prefix.len() + 32);
    out.push_str(env_prefix);
    out.push_str(prog);
    for a in new_args {
        out.push(' ');
        out.push_str(a);
    }
    for a in args {
        out.push(' ');
        out.push_str(a);
    }
    out
}

/// Git: quiet flags and pager suppression, per sub-command.
pub(crate) fn rewrite_git(
    args: &[&str],
    env_prefix: &str,
) -> Option<(Cow<'static, str>, &'static str)> {
    let sub = args.first().copied()?;
    let sub_args = &args[1..];
    let has = |flags: &[&str]| -> bool {
        sub_args.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    match sub {
        "status" if !has(&["--porcelain", "-s", "--short", "--long"]) => {
            let mut out = format!("{env_prefix}git status --porcelain=v2 --branch");
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            Some((Cow::Owned(out), "git-status-porcelain"))
        }
        "log"
            if !has(&[
                "--oneline",
                "--pretty",
                "--format",
                "-p",
                "--patch",
                "--stat",
                "--graph",
            ]) =>
        {
            let mut out = format!("{env_prefix}git log --oneline -n 50");
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            Some((Cow::Owned(out), "git-log-oneline"))
        }
        "diff" if !has(&["--stat", "--name-only", "--name-status", "--shortstat"]) => {
            // `git diff` defaults are already terse-ish; we only
            // touch it to add `--no-color` when piped output is
            // requested. Skip for now to stay conservative.
            None
        }
        "branch" if !has(&["--list", "-v", "-vv", "--format"]) => {
            let mut out = format!("{env_prefix}git branch --list");
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            Some((Cow::Owned(out), "git-branch-list"))
        }
        _ => None,
    }
}

/// Cargo: quiet flags, per sub-command.
pub(crate) fn rewrite_cargo(
    args: &[&str],
    env_prefix: &str,
) -> Option<(Cow<'static, str>, &'static str)> {
    let sub = args.first().copied()?;
    let sub_args = &args[1..];
    let has = |flags: &[&str]| -> bool {
        sub_args.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    match sub {
        "build" | "check" | "clippy" | "test" if !has(&["-q", "--quiet", "-v", "--verbose"]) => {
            let mut out = format!("{env_prefix}cargo {sub} --quiet");
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            Some((Cow::Owned(out), "cargo-quiet"))
        }
        _ => None,
    }
}

/// kubectl: narrow the output format when the user did not.
pub(crate) fn rewrite_kubectl(
    args: &[&str],
    env_prefix: &str,
) -> Option<(Cow<'static, str>, &'static str)> {
    let sub = args.first().copied()?;
    let sub_args = &args[1..];
    let has = |flags: &[&str]| -> bool {
        sub_args.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    match sub {
        "get" if !has(&["--no-headers", "-o", "--output", "-w", "--watch"]) => {
            let mut out = format!("{env_prefix}kubectl get");
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            out.push_str(" -o name");
            Some((Cow::Owned(out), "kubectl-name-only"))
        }
        _ => None,
    }
}

/// Docker: quiet flags and narrower formats, per sub-command.
pub(crate) fn rewrite_docker(
    args: &[&str],
    env_prefix: &str,
) -> Option<(Cow<'static, str>, &'static str)> {
    let sub = args.first().copied()?;
    let sub_args = &args[1..];
    let has = |flags: &[&str]| -> bool {
        sub_args.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    match sub {
        "ps" if !has(&["--format", "-q", "--quiet"]) => {
            let mut out = format!(
                "{env_prefix}docker ps --format {{{{.Names}}}}\\t{{{{.Image}}}}\\t{{{{.Status}}}}"
            );
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            Some((Cow::Owned(out), "docker-ps-format"))
        }
        "images" if !has(&["--format", "-q", "--quiet"]) => {
            let mut out = format!(
                "{env_prefix}docker images --format {{{{.Repository}}}}:{{{{.Tag}}}}\\t{{{{.Size}}}}"
            );
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            Some((Cow::Owned(out), "docker-images-format"))
        }
        _ => None,
    }
}

/// Helm: quiet flags, per sub-command.
pub(crate) fn rewrite_helm(
    args: &[&str],
    env_prefix: &str,
) -> Option<(Cow<'static, str>, &'static str)> {
    let sub = args.first().copied()?;
    let sub_args = &args[1..];
    let has = |flags: &[&str]| -> bool {
        sub_args.iter().any(|a| {
            flags
                .iter()
                .any(|f| a == f || a.starts_with(&format!("{f}=")))
        })
    };
    match sub {
        "list" | "ls" if !has(&["-o", "--output", "--short", "-q"]) => {
            let mut out = format!("{env_prefix}helm {sub}");
            for a in sub_args {
                out.push(' ');
                out.push_str(a);
            }
            out.push_str(" --short");
            Some((Cow::Owned(out), "helm-short"))
        }
        _ => None,
    }
}

/// Apply the rule table to a single, whitespace-tokenised segment.
/// Returns `Some((rewritten, rule_slug))` if a rule fired.
pub(crate) fn apply_rule(tokens: &[&str]) -> Option<(Cow<'static, str>, &'static str)> {
    let first = *tokens.first()?;

    // Strip an optional leading env-var pair: `FOO=bar pytest …` — we
    // care about the program name, not the env wrapper.
    let (env_prefix_len, prog) = if first.contains('=') && tokens.len() > 1 {
        let mut k = 0;
        while k < tokens.len() && tokens[k].contains('=') && !tokens[k].starts_with('=') {
            k += 1;
        }
        if k >= tokens.len() {
            return None;
        }
        (k, tokens[k])
    } else {
        (0, first)
    };
    let args = &tokens[env_prefix_len + 1..];
    let env_prefix = if env_prefix_len > 0 {
        tokens[..env_prefix_len].join(" ") + " "
    } else {
        String::new()
    };

    let already = |flags: &[&str]| already_has(args, flags);
    let prepend = |new_args: &[&str]| prepend_args(&env_prefix, prog, args, new_args);

    match prog {
        // ── git ────────────────────────────────────────────────────────────
        "git" => rewrite_git(args, &env_prefix),
        // ── pytest ─────────────────────────────────────────────────────────
        "pytest" | "py.test" if !already(&["-q", "--quiet", "-v", "--verbose"]) => {
            Some((Cow::Owned(prepend(&["-q", "--tb=line"])), "pytest-quiet"))
        }
        // ── cargo ──────────────────────────────────────────────────────────
        "cargo" => rewrite_cargo(args, &env_prefix),
        // ── npm / pnpm / yarn ──────────────────────────────────────────────
        "npm" | "pnpm" if !already(&["--silent", "--loglevel"]) => {
            // Only the install/run/test verbs are noisy enough to be worth it.
            let sub = args.first().copied()?;
            if matches!(sub, "install" | "i" | "ci" | "run" | "test" | "audit") {
                Some((Cow::Owned(prepend(&["--silent"])), "npm-silent"))
            } else {
                None
            }
        }
        // ── tsc / vue-tsc ──────────────────────────────────────────────────
        // vue-tsc emits the exact same `file(line,col): error TS…` shape as tsc
        // (the tsc compactor already folds it by shape); it also honours
        // `--pretty false`, so apply the same de-prettify rewrite.
        "tsc" | "vue-tsc" if !already(&["--pretty"]) => {
            Some((Cow::Owned(prepend(&["--pretty", "false"])), "tsc-no-pretty"))
        }
        // ── kubectl ────────────────────────────────────────────────────────
        "kubectl" => rewrite_kubectl(args, &env_prefix),
        // ── docker ─────────────────────────────────────────────────────────
        "docker" => rewrite_docker(args, &env_prefix),
        // ── ls ─────────────────────────────────────────────────────────────
        // Token-Savior heuristic: bare `ls -la` includes timestamps + owner
        // that the agent rarely needs. We keep that data when the user asked
        // for it explicitly (`-l`); the rewrite only fires on bare `ls`.
        "ls" if args.is_empty() => Some((Cow::Owned(prepend(&["-1"])), "ls-one-per-line")),
        // ── yarn ──────────────────────────────────────────────────────────
        // Mirror of npm: install/run/test verbs are the noisy ones. `--silent`
        // suppresses the package banner + progress chatter.
        "yarn" if !already(&["--silent", "-s"]) => {
            let sub = args.first().copied()?;
            if matches!(
                sub,
                "install" | "i" | "add" | "remove" | "upgrade" | "run" | "test" | "build" | "start"
            ) {
                Some((Cow::Owned(prepend(&["--silent"])), "yarn-silent"))
            } else {
                None
            }
        }
        // ── make ──────────────────────────────────────────────────────────
        // `make -s` suppresses the per-recipe command echo. Many makefiles
        // already use `@` per-line, so this only fires when neither `-s`
        // nor `--silent` was passed, and only on conservative subcommands.
        "make" if !already(&["-s", "--silent", "--no-print-directory"]) => Some((
            Cow::Owned(prepend(&["-s", "--no-print-directory"])),
            "make-silent",
        )),
        // ── mvn ───────────────────────────────────────────────────────────
        // Maven default is extraordinarily verbose. `-q` keeps only WARN/ERROR
        // and the final BUILD SUCCESS/FAILURE line — typically ≥80% reduction.
        "mvn" | "mvnw" | "./mvnw"
            if !already(&["-q", "--quiet", "-X", "--debug", "-e", "--errors"]) =>
        {
            Some((Cow::Owned(prepend(&["-q"])), "mvn-quiet"))
        }
        // ── gradle / gradlew ──────────────────────────────────────────────
        // Gradle's `-q` shows only errors + the lifecycle minimum.
        "gradle" | "gradlew" | "./gradlew"
            if !already(&["-q", "--quiet", "-i", "--info", "-d", "--debug"]) =>
        {
            Some((Cow::Owned(prepend(&["-q"])), "gradle-quiet"))
        }
        // ── helm ──────────────────────────────────────────────────────────
        // `helm list` defaults to multi-column tabular output; `-o name` (or
        // `--short`) collapses to one name per line.
        "helm" => rewrite_helm(args, &env_prefix),
        // ── ansible-playbook ─────────────────────────────────────────────
        // Default output is per-task `PLAY [...]` headers + recap; with
        // `--one-line` we get one line per host instead of multi-line task
        // breakdowns. Skip when -v / --verbose was explicitly requested.
        "ansible-playbook"
            if !already(&["-v", "-vv", "-vvv", "-vvvv", "--verbose", "--one-line"]) =>
        {
            Some((Cow::Owned(prepend(&["--one-line"])), "ansible-one-line"))
        }
        _ => None,
    }
}
