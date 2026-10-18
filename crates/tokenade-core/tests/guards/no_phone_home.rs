//! Tokenade is open source with no account and no telemetry: nothing under
//! `crates/` may name the old licence / usage API (the `/api` path family of
//! tokenade.net). A source string is the only way such a call reaches the
//! binary, so a clean tree is a binary that cannot phone home there.

#[test]
fn no_source_file_names_the_tokenade_net_api() {
    // Split so this guard does not match itself.
    let needle = concat!("tokenade.net", "/api");
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    let mut stack = vec![crates];
    let (mut seen, mut hits) = (0usize, Vec::new());
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                if name != "target" && name != ".git" && name != "node_modules" {
                    stack.push(p);
                }
                continue;
            }
            // Text only; a binary fixture cannot carry a request URL we ship.
            let Ok(body) = std::fs::read_to_string(&p) else {
                continue;
            };
            seen += 1;
            for (i, line) in body.lines().enumerate() {
                if line.contains(needle) {
                    hits.push(format!("{}:{}: {}", p.display(), i + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        seen > 500,
        "only {seen} files read — the walk is not reading the tree"
    );
    assert!(
        hits.is_empty(),
        "`{needle}` must not appear under crates/:\n{}",
        hits.join("\n")
    );
}
