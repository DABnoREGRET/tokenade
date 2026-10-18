//! A `--prompt` reply on a GENERATED document.
//!
//! Changelogs, status reports, audit journals and ticket exports are written by
//! a template: the same sentence, thousands of times, one value changed. A
//! reply built from such a document spends most of its budget re-reading the
//! sentence — measured before this guard existed: fifteen passages carrying
//! three distinct facts.
//!
//! Both properties matter and only together. The reply must get SHORTER, and
//! every planted fact must still be in it — a reply that shrinks by dropping an
//! answer is a regression, not an improvement.

/// 900 sections from 4 templates, with three facts planted in the noise.
fn generated_document() -> String {
    let mut doc = String::from("# Operations journal\n\n");
    for i in 0..900 {
        let ticket = format!("OPS-{}", 1000 + i);
        match i % 4 {
            0 => doc.push_str(&format!(
                "## {ticket}\n\nThe nightly reconciliation job for {ticket} completed with no \
                 exceptions and the ledger balance matched the upstream statement.\n\n"
            )),
            1 => doc.push_str(&format!(
                "## {ticket}\n\nA retry was scheduled for {ticket} after the upstream provider \
                 returned a transient error during the settlement window.\n\n"
            )),
            2 => doc.push_str(&format!(
                "## {ticket}\n\nThe operator acknowledged {ticket} and closed it without further \
                 action because the alert cleared on its own within the grace period.\n\n"
            )),
            _ => doc.push_str(&format!(
                "## {ticket}\n\nCapacity headroom for {ticket} stayed within the agreed envelope \
                 for the whole of the reporting period covered by this entry.\n\n"
            )),
        }
        if i == 311 {
            doc.push_str(
                "## Retention policy\n\nArchived settlement records are retained for 400 days \
                 before deletion.\n\n",
            );
        }
        if i == 604 {
            doc.push_str(
                "## Escalation contact\n\nOut-of-hours escalations route to the payments duty \
                 rota, reachable at rota-payments@example.org.\n\n",
            );
        }
        if i == 799 {
            doc.push_str(
                "## Encryption\n\nLedger snapshots are sealed with the XChaCha20-Poly1305 cipher \
                 before they leave the cluster.\n\n",
            );
        }
    }
    doc
}

#[test]
fn a_generated_document_answers_without_repeating_itself() {
    let doc = generated_document();
    let budget = 4_000;
    let reply = tokenade_core::rag::prompt_scoped_extract_rag(
        &doc,
        "how long are archived settlement records retained, who takes out-of-hours escalations, \
         which cipher seals ledger snapshots",
        budget,
    )
    .expect("the document is large enough to be scoped");
    let cost = tokenade_core::tokenizer::estimate(&reply);
    println!("reply: {cost} tokens, {} lines", reply.lines().count());

    for answer in [
        "400 days",
        "rota-payments@example.org",
        "XChaCha20-Poly1305",
    ] {
        assert!(
            reply.contains(answer),
            "the reply no longer answers {answer:?}:\n{}",
            &reply[..reply.len().min(1200)]
        );
    }
    // Measured on this document: 1 875 tokens before folding, 473 after, same
    // three answers. The ceiling sits between the two, well clear of both, so
    // it fails on the repetition coming back and not on noise.
    assert!(
        cost <= 900,
        "the reply costs {cost} tokens — the template repetition is back"
    );
}

/// What the templated fold is worth on the markdown that actually exists on
/// this machine, rather than on a document written to be folded. `#[ignore]`d:
/// it reads paths outside the repo and reports, it does not gate.
#[test]
#[ignore = "count on THIS machine's markdown, not on a fixture"]
fn survey_real_markdown() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>, depth: usize) {
        if depth > 6 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.filter_map(|e| e.ok()) {
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
                walk(&p, out, depth + 1);
            } else if n.ends_with(".md") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for root in std::env::var("SURVEY_ROOTS").unwrap_or_default().split(':') {
        if !root.is_empty() {
            walk(std::path::Path::new(root), &mut files, 0);
        }
    }
    files.sort();
    let (mut fired, mut tot_b, mut tot_a) = (0usize, 0usize, 0usize);
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        if text.len() < 2_000 {
            continue;
        }
        let plain = tokenade_core::markdown::compact(&text);
        let folded = tokenade_core::template_fold::fold_if_it_pays_outside_code(&plain);
        let (b, a) = (
            tokenade_core::tokenizer::estimate(&plain),
            tokenade_core::tokenizer::estimate(&folded),
        );
        tot_b += b;
        tot_a += a;
        if a < b {
            fired += 1;
            println!(
                "{:<70} {b:>7} -> {a:>7}  ({:.1}%)",
                f.display().to_string(),
                100.0 * (b - a) as f64 / b as f64
            );
        }
    }
    println!(
        "{fired}/{} documents folded; corpus {tot_b} -> {tot_a} ({:.1}%)",
        files.len(),
        100.0 * (tot_b - tot_a) as f64 / tot_b.max(1) as f64
    );
}
