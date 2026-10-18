//! `tokenade_env` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

/// A realistic `env` dump (CI runner shape) with PATH/LS_COLORS/LD_LIBRARY_PATH
/// + secrets (OPENAI/GITHUB/AWS) + normal vars.
pub(crate) fn synth_env_dump() -> String {
    let path = (0..50)
        .map(|i| format!("/usr/local/path-{i}/bin"))
        .collect::<Vec<_>>()
        .join(":");
    let mut t = String::new();
    t.push_str(&format!("PATH={path}\n"));
    t.push_str("LS_COLORS=di=01;34:fi=01;33:ln=01;36:so=01;35:pi=40;33:bd=40;33;01:cd=40;33;01:or=40;31;01:mi=00:su=37;41:sg=30;43:tw=30;42:ow=34;42:st=37;44:ex=01;32\n");
    t.push_str("OPENAI_API_KEY=sk-proj-1234567890abcdef1234567890abcdef1234567890abcdef\n");
    t.push_str("GITHUB_TOKEN=ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n");
    t.push_str("AWS_SECRET_ACCESS_KEY=AbCdEfGhIjKlMnOpQrStUvWxYz0123456789ABCDEF\n");
    t.push_str("DATABASE_PASSWORD=hunter2-with-special-chars!@#$%^\n");
    t.push_str("HOME=/home/runner\n");
    t.push_str("USER=runner\n");
    t.push_str("PWD=/home/runner/work/repo\n");
    t.push_str("CI=true\n");
    t.push_str("TERM=xterm-256color\n");
    t
}

pub(crate) fn task_env_dump(results: &mut Vec<RunResult>) {
    let raw = synth_env_dump();
    let (out, sav, ms) = measure(&raw, tokenade_env::compact_with_savings);
    // Quality: secrets fully redacted, public values truncated, normal vars kept.
    let secrets_gone = !out.contains("sk-proj-1234567890")
        && !out.contains("ghp_aaaaaaaaaaaaaaaa")
        && !out.contains("AbCdEfGhIj")
        && !out.contains("hunter2-with-special");
    let keys_kept = out.contains("OPENAI_API_KEY=")
        && out.contains("GITHUB_TOKEN=")
        && out.contains("AWS_SECRET_ACCESS_KEY=")
        && out.contains("DATABASE_PASSWORD=");
    let public_kept = out.contains("HOME=/home/runner")
        && out.contains("USER=runner")
        && out.contains("PWD=/home/runner/work/repo")
        && out.contains("CI=true");
    let path_truncated = out.contains("PATH=") && !out.contains("/usr/local/path-49/bin");
    let quality = if secrets_gone && keys_kept && public_kept && path_truncated {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:env-compact",
        "env",
        "ci-env-dump",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #31: a 1MB env dump containing 100 secrets + 100 long PATH-style
// values must complete in <500ms AND NEVER leak a secret byte.
pub(crate) fn adv_env_secret_leak(results: &mut Vec<RunResult>) {
    let mut t = String::new();
    let mut secrets: Vec<String> = Vec::new();
    for i in 0..100 {
        let secret = format!("SECRET_VALUE_NO_LEAK_{i:04}_{}", "X".repeat(40));
        t.push_str(&format!("MY_API_TOKEN_{i}={secret}\n"));
        secrets.push(secret);
    }
    let path = (0..200)
        .map(|i| format!("/path/dir-{i:04}/bin"))
        .collect::<Vec<_>>()
        .join(":");
    t.push_str(&format!("PATH={path}\n"));
    let (out, sav, ms) = measure(&t, tokenade_env::compact_with_savings);
    // The primary contract here is **no-leak**, not absolute compression %.
    // A bulk dump of fixed-size secrets compacts to ~60–70% — anything in
    // that range proves the redaction loop is working.
    let no_leak = secrets.iter().all(|s| !out.contains(s));
    let fast = ms < 500;
    let compressed = sav.percent() > 50.0;
    let quality = if no_leak && fast && compressed {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:env:bulk-secret-no-leak",
        "env",
        "adversarial",
        &t,
        &out,
        ms,
        quality,
    );
}

// Adversarial #32: secret key with case-mixed name (`Bearer_Token`,
// `aws_Secret_Key`, `gitHub_TOKEN`) must STILL be detected as secret.
pub(crate) fn adv_env_case_insensitive_secret(results: &mut Vec<RunResult>) {
    let t = "Bearer_Token=eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.payload-no-leak\n\
aws_Secret_Key=abcdef0123456789-no-leak\n\
gitHub_TOKEN=ghp_should_redact_no_leak\n\
SomeNormalThing=ok\n";
    let (out, _sav, ms) = measure(t, tokenade_env::compact_with_savings);
    let no_leak = !out.contains("eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9")
        && !out.contains("abcdef0123456789-no-leak")
        && !out.contains("ghp_should_redact_no_leak");
    let keys_kept = out.contains("Bearer_Token=")
        && out.contains("aws_Secret_Key=")
        && out.contains("gitHub_TOKEN=");
    let normal_kept = out.contains("SomeNormalThing=ok");
    let quality = if no_leak && keys_kept && normal_kept {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:env:case-insensitive-secret",
        "env",
        "adversarial",
        t,
        &out,
        ms,
        quality,
    );
}
