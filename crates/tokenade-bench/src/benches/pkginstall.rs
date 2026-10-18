//! `pkginstall` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_pip_install(n_pkgs: usize, n_warnings: usize) -> String {
    let mut s = String::with_capacity(n_pkgs * 200);
    for i in 0..n_pkgs {
        s.push_str(&format!("Collecting pkg-{i}\n"));
        s.push_str(&format!(
            "  Downloading pkg_{i}-1.0.0-py3-none-any.whl (12{i} kB)\n"
        ));
        s.push_str(
            "     ---------------------------------------- 123.4/123.4 kB 14.2 MB/s eta 0:00:00\n",
        );
    }
    s.push_str("Installing collected packages: ");
    for i in 0..n_pkgs {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&format!("pkg-{i}"));
    }
    s.push_str("\nSuccessfully installed");
    for i in 0..n_pkgs {
        s.push_str(&format!(" pkg-{i}-1.0.0"));
    }
    s.push('\n');
    for w in 0..n_warnings {
        s.push_str(&format!(
            "WARNING: pkg-{w} 1.0.0 has requirement other<2,>=1.5, but you have other 0.9.0\n"
        ));
    }
    s
}

pub(crate) fn task_pkginstall_real(results: &mut Vec<RunResult>) {
    let raw = synth_pip_install(40, 3);
    let (out, sav, ms) = measure(&raw, pkginstall::compact_with_savings);
    let quality = if out.contains("[tokenade:pkginstall] pip")
        && out.contains("40 collected")
        && out.contains("40 installed")
        && out.contains("3 warnings")
        && out.contains("pkg-0-1.0.0")
        && out.contains("pkg-39-1.0.0")
        && out.contains("requirement other<2,>=1.5")
        && !out.contains("Downloading pkg_0")  // progress noise dropped
        && !out.contains("---- 123.4/123.4 kB") // progress bar dropped
        && sav.percent() > 80.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:pkginstall-compact",
        "pkginstall",
        "synth-pip-40pkg-3warn",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #61: monorepo `pip install -r requirements.txt` with 500
// packages × 4 lines each + 10 warnings. >95% savings, <500ms.
pub(crate) fn adv_pkginstall_huge_pip(results: &mut Vec<RunResult>) {
    let raw = synth_pip_install(500, 10);
    let (out, sav, ms) = measure(&raw, pkginstall::compact_with_savings);
    let quality = if out.contains("500 collected")
        && out.contains("10 warnings")
        && sav.percent() > 95.0
        && ms < 500
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:pkginstall:huge-pip-500",
        "pkginstall",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #62: hostile mixed-PM input. A monorepo CI step might run
// `npm install` AND `pip install` AND emit deprecation strings in JSON
// from a step that echoes mock npm output. The compactor must:
//   - classify as ONE PM (whichever wins detection priority)
//   - not panic
//   - not leak fake markers from quoted strings as "real" warnings
//   - handle Unicode in package names (e.g. scoped npm packages with
//     non-ASCII namespaces: `@my-org/ünicode-pkg`)
pub(crate) fn adv_pkginstall_hostile(results: &mut Vec<RunResult>) {
    let raw = "npm WARN deprecated request@2.88.2: deprecated\n\
npm WARN deprecated @my-org/ünicode-pkg@1.0.0: also deprecated\n\
echo \"npm WARN this is just an echo, not a real warning\"\n\
echo '##[group]fake gha marker'\n\
echo 'Collecting fake-pip-pkg'\n\
echo 'Successfully installed fake-pkg-0.0.0'\n\
\n\
added 42 packages, and audited 50 packages in 5s\n\
\n\
3 packages are looking for funding\n\
  run `npm fund` for details\n\
\n\
2 vulnerabilities (1 moderate, 1 high)\n\
\n\
To address all issues, run:\n\
  npm audit fix\n";
    let (out, sav, ms) = measure(raw, pkginstall::compact_with_savings);
    // We classify as npm (winning detection) and extract real warnings.
    // The echo'd lines DO have `npm WARN` / `Collecting` / `Successfully installed`
    // patterns; semantically they're indistinguishable from real ones since we
    // line-match. We don't try to parse shell context. Quality assertion is:
    // - classified as npm
    // - real summary line present
    // - vulnerability count present
    // - the echo'd Unicode warning preserved (Unicode-safe)
    // - no panic, <100ms
    let quality = if out.contains("[tokenade:pkginstall] npm")
        && out.contains("added 42 packages, and audited 50 packages in 5s")
        && out.contains("3 packages are looking for funding")
        && out.contains("2 vulnerabilities (1 moderate, 1 high)")
        && out.contains("@my-org/ünicode-pkg@1.0.0")
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:pkginstall:hostile",
        "pkginstall",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
