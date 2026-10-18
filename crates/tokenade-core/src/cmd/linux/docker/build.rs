//! `docker build` / `docker buildx build` / `podman build` — image builds
//! (legacy and BuildKit output).
//!
//! Build output is highly multi-phase and noisy: a context upload, then one
//! block per layer (download → extract → RUN spew), then an export/tag phase.
//! A flat head/tail or generic dedup either keeps the per-layer download
//! chatter or risks dropping the final image / a failing step. The SIGNAL is:
//!
//!   - the step/stage OUTLINE — `Step N/M : …` (legacy), `#N [i/k] …` (BuildKit)
//!   - each step's terminal status — `#N DONE`, `#N CACHED`, `#N ERROR`
//!   - the final image — `writing image sha256:…`, `naming to …`,
//!     `Successfully built …`, `Successfully tagged …`
//!   - any error / warning line, wherever it appears
//!
//! Everything else — layer download progress, `resolve`/`extracting`,
//! intermediate `--->` hashes, clean RUN output — is folded into a count, the
//! same shape the terraform / gh-actions compactors use.

use crate::compress::generic;

/// Cheap guard: only compact output that actually looks like a docker build,
/// so an unexpected shape falls through to the generic fold untouched.
fn is_build_output(text: &str) -> bool {
    text.lines().take(400).any(|l| {
        let t = l.trim_start();
        (t.starts_with("Step ") && t.contains('/') && t.contains(':'))
            || (t.starts_with('#')
                && (t.contains(" [")
                    || t.contains(" DONE")
                    || t.contains(" CACHED")
                    || t.contains(" ERROR")))
    })
}

/// True for the lines worth keeping (outline / status / final image / diag).
fn keep_line(l: &str) -> bool {
    let t = l.trim_start();
    // Legacy step outline + final image.
    if t.starts_with("Step ") && t.contains(':') {
        return true;
    }
    if t.starts_with("Successfully built") || t.starts_with("Successfully tagged") {
        return true;
    }
    // BuildKit stage headers (`#4 [1/5] FROM …`) and terminal status.
    if t.starts_with('#')
        && (t.contains(" [")
            || t.contains(" DONE")
            || t.contains(" CACHED")
            || t.contains(" ERROR"))
    {
        return true;
    }
    // BuildKit export / image naming.
    if l.contains("exporting to image") || l.contains("writing image") || l.contains("naming to ")
    {
        return true;
    }
    // Build-failure result lines (legacy: `… returned a non-zero code: 2`;
    // BuildKit: `ERROR: process … did not complete successfully: exit code: 2`).
    if l.contains("returned a non-zero code")
        || l.contains("did not complete successfully")
        || l.contains("exit code:")
    {
        return true;
    }
    // Diagnostics anywhere — never fold an error or warning.
    //
    // `ERR!` is listed because the substring test below does NOT catch it:
    // npm, pnpm and yarn prefix every diagnostic line with `npm ERR!`, which
    // contains "err" but not "error". A failing `RUN npm ci` therefore had its
    // 900 `Step` outline lines kept and its two lines of actual cause folded
    // away as noise — measured, the exact inverse of this function's purpose.
    let low = l.to_ascii_lowercase();
    low.contains("error")
        || low.contains("err!")
        || low.contains("warning")
        || low.contains("warn ")
        || low.contains("failed")
        || low.contains("fatal")
        || low.contains("panic")
}

/// `Step 412/900 : RUN …` — the legacy build outline.
///
/// One line per instruction, and on a 900-step build that outline IS the
/// output. It is progress, not diagnosis: the only one that matters is the
/// last, because docker stops there when a step fails.
fn is_step_outline(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("Step ") && t.contains(':')
}

pub fn compact(output: &str) -> String {
    if !is_build_output(output) {
        return generic(output);
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut folded = 0usize;
    // BuildKit replays the FAILING step's output between `------` delimiter
    // lines (plus `> `-prefixed context). Folding non-keyword lines there
    // dropped e.g. the frames of a Python traceback, keeping only the final
    // `Error:` — the diagnostic context matters most on exactly those runs.
    let mut in_excerpt = false;
    for line in output.lines() {
        let t = line.trim();
        let is_delim = t.len() >= 6 && t.chars().all(|c| c == '-');
        if is_delim {
            in_excerpt = !in_excerpt;
            kept.push(line);
            continue;
        }
        if in_excerpt || t.starts_with("> ") {
            kept.push(line);
            continue;
        }
        if keep_line(line) {
            kept.push(line);
        } else if !t.is_empty() {
            folded += 1;
        }
    }
    // Fold the step outline down to its LAST entry. Everything before it
    // succeeded; the last one is where the build stopped. Declared like any
    // other fold, and skipped entirely on short builds where the outline IS
    // the useful shape.
    const OUTLINE_KEEP_FROM: usize = 12;
    let outline_total = kept.iter().filter(|l| is_step_outline(l)).count();
    if outline_total > OUTLINE_KEEP_FROM {
        let mut seen = 0usize;
        let mut trimmed: Vec<&str> = Vec::with_capacity(kept.len());
        for line in kept {
            if is_step_outline(line) {
                seen += 1;
                if seen < outline_total {
                    continue;
                }
            }
            trimmed.push(line);
        }
        folded += outline_total - 1;
        kept = trimmed;
    }
    if folded == 0 {
        return output.to_string();
    }
    let header = format!("[tokenade:docker-build] {folded} noise line(s) folded\n");
    format!("{header}{}", kept.join("\n").trim_end())
}

#[cfg(test)]
mod tests {

    #[test]
    fn npm_style_error_lines_are_never_folded() {
        // `npm ERR!` contains "err" but not "error", so the diagnostic test
        // missed it and folded the only lines that said WHY the build failed.
        let mut input = String::new();
        for i in 0..40 {
            input.push_str(&format!("Step {i}/40 : RUN echo layer{i}\n"));
        }
        input.push_str("npm ERR! code ERESOLVE\n");
        input.push_str("npm ERR! unable to resolve dependency tree\n");
        input.push_str("The command '/bin/sh -c npm ci' returned a non-zero code: 1\n");
        let out = super::compact(&input);
        assert!(out.contains("npm ERR! code ERESOLVE"), "got:\n{out}");
        assert!(
            out.contains("unable to resolve dependency tree"),
            "the reason survives:\n{out}"
        );
    }

    #[test]
    fn a_long_step_outline_folds_to_where_the_build_stopped() {
        let mut input = String::new();
        for i in 0..300 {
            input.push_str(&format!("Step {i}/300 : RUN echo layer{i}\n"));
        }
        input.push_str("error: something broke\n");
        let out = super::compact(&input);
        assert!(out.contains("Step 299/300"), "the last step is where it stopped:\n{out}");
        assert!(!out.contains("Step 150/300"), "earlier steps fold:\n{out}");
        assert!(out.contains("error: something broke"));
        assert!(out.contains("noise line(s) folded"), "the fold is declared");
    }

    #[test]
    fn a_short_build_keeps_its_whole_outline() {
        // On a small build the outline IS the useful shape; folding it would
        // cost more than it saves.
        let mut input = String::new();
        for i in 0..5 {
            input.push_str(&format!("Step {i}/5 : RUN echo layer{i}\n"));
        }
        input.push_str("Successfully built abc123\n");
        let out = super::compact(&input);
        for i in 0..5 {
            assert!(out.contains(&format!("Step {i}/5")), "step {i} kept:\n{out}");
        }
    }
    use super::*;

    #[test]
    fn buildkit_keeps_outline_and_image_folds_noise() {
        let out = "\
#1 [internal] load build definition from Dockerfile
#1 transferring dockerfile: 234B done
#1 DONE 0.0s

#4 [1/3] FROM docker.io/library/node:18
#4 resolve docker.io/library/node:18 done
#4 sha256:abc 45.3MB / 45.3MB 2.1s done
#4 extracting sha256:abc done
#4 DONE 5.3s

#6 [3/3] RUN npm install
#6 12.34 npm WARN deprecated foo@1.0
#6 45.67 added 234 packages
#6 DONE 46.2s

#8 exporting to image
#8 writing image sha256:def done
#8 naming to docker.io/library/myapp:latest done
#8 DONE 0.5s";
        let c = compact(out);
        assert!(c.contains("#4 [1/3] FROM"), "stage header kept: {c}");
        assert!(c.contains("#6 [3/3] RUN npm install"), "run step kept: {c}");
        assert!(c.contains("#4 DONE 5.3s"), "status kept: {c}");
        assert!(c.contains("npm WARN deprecated"), "warning kept: {c}");
        assert!(c.contains("writing image sha256:def"), "final image kept: {c}");
        assert!(!c.contains("45.3MB / 45.3MB"), "download progress folded: {c}");
        assert!(!c.contains("added 234 packages"), "clean run spew folded: {c}");
        assert!(c.contains("noise line(s) folded"), "fold header present: {c}");
    }

    #[test]
    fn legacy_keeps_steps_and_result() {
        let out = "\
Sending build context to Docker daemon  2.048kB
Step 1/3 : FROM node:18
 ---> abc123
Step 2/3 : COPY package.json .
 ---> Using cache
 ---> def456
Step 3/3 : RUN npm install
 ---> Running in 789abc
added 234 packages
Removing intermediate container 789abc
 ---> ghi789
Successfully built ghi789
Successfully tagged myapp:latest";
        let c = compact(out);
        assert!(c.contains("Step 1/3 : FROM node:18"), "step kept: {c}");
        assert!(c.contains("Step 3/3 : RUN npm install"), "step kept: {c}");
        assert!(c.contains("Successfully tagged myapp:latest"), "tag kept: {c}");
        assert!(!c.contains("Removing intermediate container"), "noise folded: {c}");
        assert!(!c.contains("Sending build context"), "noise folded: {c}");
    }

    #[test]
    fn keeps_build_failure() {
        let out = "\
Step 3/3 : RUN make
 ---> Running in abc
gcc: error: no input files
The command '/bin/sh -c make' returned a non-zero code: 2";
        let c = compact(out);
        assert!(c.contains("gcc: error: no input files"), "error kept: {c}");
        assert!(c.contains("returned a non-zero code"), "failure kept: {c}");
    }

    #[test]
    fn non_build_falls_through() {
        let out = "just some\nrandom text\nnot a build";
        // Should not panic and should not add the fold header.
        let c = compact(out);
        assert!(!c.contains("[tokenade:docker-build]"), "no header on non-build: {c}");
    }
}
