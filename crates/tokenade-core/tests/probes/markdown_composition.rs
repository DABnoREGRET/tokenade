//! **What is a markdown document MADE of, in bytes?**
//!
//! `auto:markdown` stayed between 0.74 and 1.00 for weeks and every review
//! ended on "the prose IS the payload". That is a claim, not a measurement.
//! This counts, on a real corpus of documents small enough that the
//! `disclose` outline never rescues them.
//!
//! `#[ignore = "measurement"]`: run by hand, with `SWEEP_ROOT` (and
//! `SWEEP_CAP`). It lived in `markdown_tests.rs`; the `probes` target has
//! been its home since the 87 targets were grouped.

/// What is a markdown document actually MADE OF, by bytes?
///
/// `auto:markdown` has sat at 0.74-1.00 for weeks and every look at it
/// ended in "the prose IS the payload". That is an assertion, not a
/// measurement. This counts, over a real corpus of documents small enough
/// that the disclose outline never rescues them.
#[test]
#[ignore = "measurement"]
fn what_markdown_is_made_of() {
    let Ok(root) = std::env::var("SWEEP_ROOT") else {
        return;
    };
    let cap: usize = std::env::var("SWEEP_CAP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    let mut stack = vec![std::path::PathBuf::from(&root)];
    let (mut files, mut total) = (0usize, 0usize);
    let (mut fence, mut inline_link, mut ref_link, mut table, mut html, mut heading, mut blank) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let n = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if n.starts_with('.') || n == "target" || n == "node_modules" {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let ext = p
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if ext != "md" && ext != "mdx" {
                continue;
            }
            let Ok(t) = std::fs::read_to_string(&p) else {
                continue;
            };
            if t.len() < 4_000 || t.len() > 40_000 {
                continue;
            }
            if tokenade_core::tokenizer::estimate(&t) >= tokenade_core::disclose::threshold_tokens()
            {
                continue;
            }
            files += 1;
            if files > cap {
                stack.clear();
                break;
            }
            total += t.len();
            let mut in_fence = false;
            for line in t.lines() {
                let l = line.trim_start();
                let w = line.len() + 1;
                if l.starts_with("```") || l.starts_with("~~~") {
                    in_fence = !in_fence;
                    fence += w;
                    continue;
                }
                if in_fence {
                    fence += w;
                    continue;
                }
                if l.is_empty() {
                    blank += w;
                    continue;
                }
                if l.starts_with('#') {
                    heading += w;
                    continue;
                }
                if l.starts_with('|') && l.ends_with('|') {
                    table += w;
                    continue;
                }
                if l.starts_with('<') {
                    html += w;
                    continue;
                }
                // Reference definition: `[label]: https://…`
                if l.starts_with('[') && l.contains("]: ") {
                    ref_link += w;
                    continue;
                }
                // Inline links: count the URL bytes only.
                let mut rest = line;
                while let Some(i) = rest.find("](") {
                    let after = &rest[i + 2..];
                    let end = after.find(')').unwrap_or(after.len());
                    inline_link += end;
                    rest = &after[end.min(after.len())..];
                }
            }
        }
    }
    let prose =
        total.saturating_sub(fence + inline_link + ref_link + table + html + heading + blank);
    let pc = |x: usize| x as f64 * 100.0 / total.max(1) as f64;
    eprintln!("MD files={files} bytes={total}");
    for (k, v) in [
        ("prose", prose),
        ("code-fence", fence),
        ("blank", blank),
        ("heading", heading),
        ("table", table),
        ("html", html),
        ("inline-link-urls", inline_link),
        ("ref-link-defs", ref_link),
    ] {
        eprintln!("MD  {k:<18} {v:>9}  {:.1}%", pc(v));
    }
}
