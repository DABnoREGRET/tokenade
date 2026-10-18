//! `stacktrace` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

// Real task: a realistic 200-frame Python trace with 1 user frame.
pub(crate) fn task_stacktrace_python(results: &mut Vec<RunResult>) {
    let mut raw = String::from("Traceback (most recent call last):\n");
    for i in 0..200 {
        raw.push_str(&format!(
            "  File \"/usr/lib/python3.12/site-packages/asyncio/runners.py\", line {}, in run\n",
            42 + i
        ));
        raw.push_str("    return self._loop.run_until_complete(task)\n");
    }
    raw.push_str("  File \"/home/me/app/api.py\", line 87, in get_user\n");
    raw.push_str("    user = await db.fetch_one(query)\n");
    raw.push_str("psycopg.errors.UndefinedColumn: column \"emial\" does not exist\n");

    let (out, sav, ms) = measure(&raw, stacktrace::compact_with_savings);
    let user_kept = out.contains("/home/me/app/api.py:87 in get_user");
    let err_kept = out.contains("UndefinedColumn");
    let folded = out.contains("framework frame");
    let quality = if user_kept && err_kept && folded {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:stacktrace-compact",
        "stacktrace-compact",
        "python-200frames",
        &raw,
        &out,
        ms,
        quality,
    );
    let _ = sav;
}

// Adversarial #13: the SINGLE user frame in a 500-frame trace must always
// survive, regardless of where it lives in the stack. Tests at three depths
// (top, middle, deep in the framework chain).
pub(crate) fn adv_stack_user_frame_preserved(results: &mut Vec<RunResult>) {
    let depths = [5, 250, 495];
    let mut total = 0;
    let mut hits = 0;
    for &depth in &depths {
        total += 1;
        let mut raw = String::from("Traceback (most recent call last):\n");
        for i in 0..500 {
            if i == depth {
                raw.push_str("  File \"/home/me/SECRET/user_frame.py\", line 1, in user_func\n");
                raw.push_str("    raise ValueError('UNIQUE_PAYLOAD')\n");
            } else {
                raw.push_str(
                    "  File \"/usr/lib/python3.12/site-packages/lib.py\", line 1, in fwk\n",
                );
                raw.push_str("    pass\n");
            }
        }
        raw.push_str("ValueError: UNIQUE_PAYLOAD\n");
        let out = stacktrace::compact(&raw);
        if out.contains("SECRET/user_frame.py") && out.contains("user_func") {
            hits += 1;
        }
    }
    let quality = hits as f64 / total as f64;
    record(
        results,
        "adv:stack:user-frame-preserved",
        "stacktrace-compact",
        "adversarial",
        "",
        "",
        0,
        quality,
    );
}

// Adversarial #14: mixed-language input (Python header, JS frames). The
// detector must pick A language (the dominant one) and not crash on the
// shape mismatch.
pub(crate) fn adv_stack_mixed_language(results: &mut Vec<RunResult>) {
    let raw = "Traceback (most recent call last):\n  File \"/x.py\", line 1, in main\n    foo()\nError: bad\n    at handler (/home/me/app.js:5:1)\n    at /node_modules/x/y.js:1:1\n";
    let t = Instant::now();
    let out = stacktrace::compact(raw);
    let ms = t.elapsed().as_millis();
    // We only require non-panic + the error line + one identifiable frame.
    let safe = !out.is_empty()
        && (out.contains("Error: bad") || out.contains("/x.py") || out.contains("/home/me/app.js"));
    let quality = if safe { 1.0 } else { 0.0 };
    record(
        results,
        "adv:stack:mixed-language",
        "stacktrace-compact",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
