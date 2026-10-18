//! **Does the cost grow FASTER than the input?**
//!
//! The "hostile input" axis of the project's rules had no instrument: each
//! visit rebuilt its shapes in `/tmp`, which the rules forbid
//! precisely because a throwaway probe reintroduces its biases every time.
//!
//! What this instrument measures is not a duration — in debug it compares
//! to nothing — but a RATIO: the same shape at N and at 2N. A ratio
//! close to 2 says linear; above 3, the cost grows faster than
//! the input and an ordinary, somewhat large input will make the agent wait.
//!
//! It was written after finding TWO quadratic paths by hand:
//! `push_grouped` (forty thousand lines of `grep` without returning after
//! two minutes) and the removal of constant columns (`contains` per
//! cell). Both would have been named by one pass of this instrument.
use tokenade_bench::{Delivery, Stages};

/// An input shape, parameterised by its size.
struct Shape {
    name: &'static str,
    /// The program to measure it under, or the EXTENSION when the shape
    /// goes through the READ path — see [`Route`].
    prog: &'static str,
    gen: fn(usize) -> String,
    route: Route,
}

/// Where the shape enters the product.
///
/// The first twelve shapes all went through the command compactor.
/// The READ path is the one an agent takes most often, and it
/// has its own ladder of views: measuring it nowhere left the
/// most travelled one unanswered.
#[derive(Clone, Copy, PartialEq)]
enum Route {
    /// `filter --deliver <prog>`, input on stdin.
    Command,
    /// `read <file.ext>` — routing is done on the EXTENSION, so the
    /// shape must be written to a file that carries it.
    Read,
    /// `web <file.html>` — the page reader, which is neither the command
    /// compactor nor the read view: it has its own extractor.
    Web,
}

/// Many ordinary lines.
fn many_lines(n: usize) -> String {
    (0..n)
        .map(|i| format!("line {i} value{i} tail\n"))
        .collect()
}

/// Many COLUMNS: a single-spaced, very wide table.
fn many_columns(n: usize) -> String {
    let header = (0..n)
        .map(|i| format!("col{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let row = (0..n).map(|_| "v").collect::<Vec<_>>().join(" ");
    let mut out = header;
    out.push('\n');
    for _ in 0..40 {
        out.push_str(&row);
        out.push('\n');
    }
    out
}

/// A SINGLE, very long line.
fn one_long_line(n: usize) -> String {
    let mut s = "x".repeat(n * 40);
    s.push('\n');
    s
}

/// Many SEGMENTS separated by blank lines.
fn many_segments(n: usize) -> String {
    (0..n).map(|i| format!("seg{i} a  b  c\n\n")).collect()
}

/// A listing whose per-directory grouping is REFUSED on every line:
/// the shape that kept `grep` busy for more than two minutes.
fn a_refused_listing(n: usize) -> String {
    let mut out: String = (0..n)
        .map(|i| format!("/very/long/common/root/a/f{i}.rs:{i}:m\n"))
        .collect();
    out.push_str("/very/long/common/root/b/x.rs:1:m\n");
    out
}

/// A line with no END: eighty bytes per unit, no newline.
fn no_newline(n: usize) -> String {
    "w".repeat(n * 80)
}

/// NUL bytes in the middle of the text — legal in UTF-8, rare in command
/// output, and that is exactly why we pass them.
fn nul_bytes(n: usize) -> String {
    (0..n).map(|i| format!("line{i}\0value\0tail\n")).collect()
}

/// **Content that carries OUR markers.** The axis has named it for a long time: an
/// output that already contains an elision marker must not make the
/// product count a hole it did not dig.
fn marker_carrying(n: usize) -> String {
    (0..n)
        .map(|i| format!("row {i}\n… [40 lines elided by tokenade] …\n"))
        .collect()
}

/// Windows line endings in an aligned table.
fn crlf_table(n: usize) -> String {
    (0..n).map(|i| format!("a {i}  b  c\r\n")).collect()
}

/// As many distinct TEMPLATES as lines: the repeated-sentence fold
/// files each line into a group, and no group fills up.
fn many_templates(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!("warning: cannot resolve dependency `d{i}` in file \"/x/{i}.ts\" at line {i}\n")
        })
        .collect()
}

/// Above this, the cost grows faster than the input.
///
/// Two is linear; four is quadratic. Three leaves room for the
/// noise of a shared machine without letting a real n² through.
/// **Few lines, but HUGE ones.** All the previous shapes vary
/// the NUMBER of lines, and above two hundred the line ceiling folds
/// before the targeted compactor has to work. Sixty lines stay under
/// that ceiling: what varies here is the work PER LINE — value
/// recognition, column detection, slicing.
fn few_huge_lines(n: usize) -> String {
    let cell = "valeur-longue-avec-des-tirets-et-des_underscores_42 ";
    let line = cell.repeat(n / 12);
    (0..60).map(|i| format!("ligne{i} {line}\n")).collect()
}

/// A single OPTION carrying an endless list of paths: the shape
/// `mount` gives a Docker `overlay` mount, pushed to its extreme.
fn one_huge_path_list(n: usize) -> String {
    let paths: Vec<String> = (0..n)
        .map(|i| format!("/var/lib/docker/overlay2/l/COUCHE{i:08}"))
        .collect();
    format!(
        "overlay on /var/lib/docker/overlay2/x/merged type overlay \
         (rw,relatime,lowerdir={},index=off)\n",
        paths.join(":")
    )
}

/// `cat -n guard.ts | sed -n …`: the gutter is set aside, the content
/// folded, then each line delivered as is gets its number back
/// (`line_gutter::renumber`) — this commented TypeScript source is the shape that
/// really reaches that branch. The WORST case of the
/// renumbering (no matched line: 99.7 s on 60,000 lines with a
/// first algorithm that scanned the tail) is held by
/// `renumber_is_not_quadratic_when_nothing_matches` in `tk-fold`; this shape
/// holds the scale of the whole ROUTE.
fn a_numbered_document(n: usize) -> String {
    let mut src = String::from("import { Injectable } from '@nestjs/common';\n\n");
    for k in 0..n / 9 {
        src.push_str(&format!(
            "/**\n * Guard step {k}: checks the {} before the route runs.\n */\nexport function step{k}(ctx: Ctx, n: number): boolean {{\n  // reject when the {} is missing\n  if (!ctx.ok{k}) return false;\n  return n > {};\n}}\n\n",
            ["token", "actor", "scope"][k % 3],
            ["header", "claim", "grant"][k % 3],
            k * 7
        ));
    }
    src.lines()
        .enumerate()
        .map(|(i, l)| format!("{:>6}\t{l}\n", i + 1))
        .collect()
}

/// `diff -r` under a French locale: one `diff -r a b` echo per file,
/// translated frame lines, normal blocks. Reaches `served_whole` (the
/// frame read by its shape) then, above the delivery threshold, the diff
/// outline.
fn a_recursive_diff(n: usize) -> String {
    let mut out = String::new();
    for f in 0..n / 12 {
        out.push_str(&format!("diff -r old/src/m{f}.rs new/src/m{f}.rs\n"));
        out.push_str(&format!(
            "{}c{}\n< let a = old_{f}({f});\n---\n> let a = new_{f}({f}, opts);\n",
            f % 90 + 3,
            f % 90 + 3
        ));
        out.push_str(&format!(
            "{}a{},{}\n> trace!(\"m{f}\");\n> check({f});\n",
            f % 70 + 20,
            f % 70 + 21,
            f % 70 + 22
        ));
        if f % 9 == 0 {
            out.push_str(&format!("Seulement dans new/src: added_{f}.rs\n"));
        }
    }
    out
}

/// `php artisan route:list`: every line carries its leader dots
/// (`tk_base::leaders`), reduced to "…".
fn a_route_list(n: usize) -> String {
    (0..n)
        .map(|i| {
            let left = format!("  GET|HEAD  api/r{i}/{{id}} ");
            let right = format!(" r{i}.show › Api\\R{i}Controller@show");
            format!(
                "{left}{}{right}\n",
                ".".repeat(150usize.saturating_sub(left.len() + right.len()).max(3))
            )
        })
        .collect()
}

/// A green vitest suite: thousands of log lines from the code under test,
/// then the summary (`vitest_pass`) — the middle is counted, not read.
fn a_green_vitest_run(n: usize) -> String {
    let mut s = String::from("> app@0.0.0 test\n> vitest run\n\n RUN  v5.0.0 app\n\n");
    for i in 0..n {
        let lvl = ["LOG", "WARN", "ERROR"][i % 3];
        s.push_str(&format!(
            "[Nest] {}  - 02/10/2026 11:29:56    {lvl} [Svc{}] message {i}\n",
            1000 + i % 97,
            i % 41
        ));
    }
    s.push_str(" Test Files  179 passed | 22 skipped (201)\n      Tests  2281 passed (2472)\n   Start at  11:29:56\n   Duration  8.49s\n");
    s
}

/// A command help too long to be delivered: its filed summary is
/// the INDEX of its names (`help_index`), rendered at several widths.
fn a_long_help_text(n: usize) -> String {
    let mut s = String::from("A tool.\n\nUsage: tool [OPTIONS] <COMMAND>\n\nCommands:\n");
    for i in 0..n {
        s.push_str(&format!(
            "  cmd{i:05}     {}\n",
            "does a long and detailed thing ".repeat(1 + i % 4)
        ));
    }
    s
}

/// `grep -rn` results where one in ten is a minified bundle line:
/// each giant line is cut around the pattern (`clip_long_hits`)
/// and the others pass as is.
fn grep_hits_in_bundles(n: usize) -> String {
    let bundle: String = (0..300)
        .map(|i| format!("var a{i}=function(e){{return e.k{i}}};"))
        .collect();
    (0..n / 4)
        .map(|i| {
            if i % 10 == 0 {
                format!("dist/chunk-{i}.js:1:{bundle}\n")
            } else {
                format!(
                    "src/m{}.ts:{}:  if (row.revived) mark(row.id{i});\n",
                    i % 40,
                    i * 3 + 1
                )
            }
        })
        .collect()
}

/// A SOURCE file, for the read path and its ladder of views.
fn a_source_file(n: usize) -> String {
    (0..n / 4)
        .map(|i| {
            format!(
                "/// Ce que fait la fonction {i}, en une phrase de commentaire.\n                 pub fn traite_{i}(entree: &str, drapeau: bool) -> Option<String> {{\n                 \x20   let mut sortie = String::with_capacity(entree.len());\n                 \x20   if drapeau {{ sortie.push_str(entree); }}\n                 \x20   Some(sortie)\n}}\n\n"
            )
        })
        .collect()
}

/// A JSON document, the other big family of the read path.
fn a_json_document(n: usize) -> String {
    let items: Vec<String> = (0..n / 4)
        .map(|i| {
            format!(
                "{{\"id\":{i},\"name\":\"objet-{i}\",\"tags\":[\"a\",\"b\"],\"meta\":{{\"v\":{i},\"ok\":true}}}}"
            )
        })
        .collect();
    format!("{{\"items\":[{}]}}\n", items.join(","))
}

const SHAPES: &[Shape] = &[
    Shape {
        name: "many_lines",
        prog: "ps",
        gen: many_lines,
        route: Route::Command,
    },
    Shape {
        name: "many_columns",
        prog: "ps",
        gen: many_columns,
        route: Route::Command,
    },
    Shape {
        name: "one_long_line",
        prog: "ps",
        gen: one_long_line,
        route: Route::Command,
    },
    Shape {
        name: "many_segments",
        prog: "ps",
        gen: many_segments,
        route: Route::Command,
    },
    Shape {
        name: "a_refused_listing",
        prog: "grep",
        gen: a_refused_listing,
        route: Route::Command,
    },
    Shape {
        name: "no_newline",
        prog: "ps",
        gen: no_newline,
        route: Route::Command,
    },
    Shape {
        name: "nul_bytes",
        prog: "ps",
        gen: nul_bytes,
        route: Route::Command,
    },
    Shape {
        name: "marker_carrying",
        prog: "ps",
        gen: marker_carrying,
        route: Route::Command,
    },
    Shape {
        name: "crlf_table",
        prog: "ps",
        gen: crlf_table,
        route: Route::Command,
    },
    Shape {
        name: "many_templates",
        prog: "npm",
        gen: many_templates,
        route: Route::Command,
    },
    Shape {
        name: "few_huge_lines",
        prog: "ps",
        gen: few_huge_lines,
        route: Route::Command,
    },
    Shape {
        name: "one_huge_path_list",
        prog: "mount",
        gen: one_huge_path_list,
        route: Route::Command,
    },
    Shape {
        name: "read_a_source_file",
        prog: "rs",
        gen: a_source_file,
        route: Route::Read,
    },
    Shape {
        name: "read_a_json_document",
        prog: "json",
        gen: a_json_document,
        route: Route::Read,
    },
    Shape {
        name: "read_a_markdown_document",
        prog: "md",
        gen: a_markdown_document,
        route: Route::Read,
    },
    Shape {
        name: "read_an_html_page",
        prog: "html",
        gen: an_html_page,
        route: Route::Read,
    },
    Shape {
        name: "web_an_html_page",
        prog: "html",
        gen: an_html_page,
        route: Route::Web,
    },
    Shape {
        name: "a_numbered_document",
        prog: "sed",
        gen: a_numbered_document,
        route: Route::Command,
    },
    Shape {
        name: "a_recursive_diff",
        prog: "diff",
        gen: a_recursive_diff,
        route: Route::Command,
    },
    Shape {
        name: "grep_hits_in_bundles",
        prog: "grep",
        gen: grep_hits_in_bundles,
        route: Route::Command,
    },
    Shape {
        name: "a_route_list",
        prog: "php artisan route:list",
        gen: a_route_list,
        route: Route::Command,
    },
    Shape {
        name: "a_green_vitest_run",
        prog: "pnpm test",
        gen: a_green_vitest_run,
        route: Route::Command,
    },
    Shape {
        name: "a_long_help_text",
        prog: "cargo --help",
        gen: a_long_help_text,
        route: Route::Command,
    },
];

/// **Does the declared door exist?**
///
/// A shape entering through the wrong door measures something else without saying so:
/// an extension passed as a program name routes to the generic
/// compactor, a program name passed as an extension writes a file that
/// `read` does not recognise. It is the same mistake at the scale of one table
/// row, and the instrument REFUSES at start-up rather than returning a
/// number about something else.
fn door_is_consistent(prog: &str, route: Route) -> bool {
    match route {
        // The PROGRAM is the first word: `php artisan route:list` enters by `php`.
        Route::Command => prog
            .split_whitespace()
            .next()
            .is_some_and(tokenade_core::filter::knows_program),
        // An extension: lowercase and digits, no dot, no dash.
        Route::Read | Route::Web => {
            !prog.is_empty()
                && prog
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        }
    }
}

/// A Markdown document: headings, lists, tables, code blocks.
fn a_markdown_document(n: usize) -> String {
    (0..n / 6)
        .map(|i| {
            format!(
                "## Section {i}\n\nUn paragraphe qui explique la section {i} en quelques \
                 mots.\n\n- premier point {i}\n- deuxième point {i}\n\n```rust\nlet x_{i} = \
                 {i};\n```\n\n"
            )
        })
        .collect()
}

/// An HTML page: the shape `web` receives, and that `read` routes differently.
fn an_html_page(n: usize) -> String {
    let body: String = (0..n / 6)
        .map(|i| {
            format!(
                "<div class=\"item\" data-id=\"{i}\"><h2>Titre {i}</h2><p>Texte de \
                 l'élément {i}, avec un <a href=\"/lien/{i}\">lien</a>.</p></div>"
            )
        })
        .collect();
    format!("<html><head><title>Page</title><style>.item{{color:red}}</style></head><body>{body}</body></html>\n")
}

/// Is the shape in the selection?
///
/// `--only` takes a name fragment. **A selection that names NO
/// shape is a refusal**, not an empty sweep: without it, `--only zzz` prints
/// "no shape grows faster than its input", which is the
/// all-is-well sentence for zero measurements. Same pattern, one door further.
fn selected(name: &str, only: Option<&str>) -> bool {
    match only {
        Some(fragment) => name.contains(fragment),
        None => true,
    }
}

const SUPERLINEAR: f64 = 3.0;

/// Below this, start-up noise dominates and the ratio says nothing.
const TOO_FAST_MS: u128 = 100;

/// A shape's verdict, as a pure function — an instrument's decision gets
/// tested, otherwise it is only checked by the shape that made it be written.
#[derive(Debug, PartialEq)]
enum Verdict {
    TooFast,
    Linear(f64),
    Superlinear(f64),
}

fn verdict(small_ms: u128, big_ms: u128) -> Verdict {
    if small_ms < TOO_FAST_MS {
        return Verdict::TooFast;
    }
    let ratio = big_ms as f64 / small_ms as f64;
    if ratio > SUPERLINEAR {
        Verdict::Superlinear(ratio)
    } else {
        Verdict::Linear(ratio)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    tokenade_bench::reject_unknown_args(&["--big", "--only"], &args);
    let scale = if args.iter().any(|a| a == "--big") {
        4
    } else {
        1
    };
    // **A removed shape is a question we stop asking.** The sweep
    // gets longer with every shape added; when it exceeds the time we want
    // to give it, we run ONE of them, we do not delete any.
    let only = args
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let shapes = SHAPES;
    for s in SHAPES {
        if !door_is_consistent(s.prog, s.route) {
            eprintln!(
                "`{}` enters through a door that does not exist (`{}`) — the instrument \
                 would measure something else without saying so",
                s.name, s.prog
            );
            std::process::exit(2);
        }
    }
    let bench = match Delivery::open() {
        Ok(b) => b,
        Err(note) => {
            eprintln!("{note}");
            std::process::exit(2);
        }
    };
    println!(
        "COST vs INPUT: the same shape at N then 2N, on the delivery path.\n\
             \x20 A ratio close to 2 is linear; above {SUPERLINEAR} the cost \
             grows faster than the input."
    );
    let mut superlinear = Vec::new();
    let mut measured = 0usize;
    for s in shapes {
        if !selected(s.name, only.as_deref()) {
            continue;
        }
        measured += 1;
        let n = 25_000 * scale;
        let small = (s.gen)(n);
        let big = (s.gen)(n * 2);
        let measure = |body: &str, tag: &str| match s.route {
            Route::Command => bench.measure_stages_timed(s.prog, body, Stages::Delivered),
            Route::Read | Route::Web => {
                let path = bench.fixture(&format!("{}_{tag}", s.name), s.prog, body)?;
                let verb = if s.route == Route::Web { "web" } else { "read" };
                bench.measure_timed(&[verb, &path.to_string_lossy()])
            }
        };
        let one = measure(&small, "n");
        let two = measure(&big, "2n");
        match (one, two) {
            (Ok((_, a)), Ok((_, b))) => {
                // A measurement that is too short says nothing: under a hundred milliseconds
                // start-up noise dominates the ratio.
                match verdict(a, b) {
                    Verdict::TooFast => {
                        println!("  {:<18} {a} ms → {b} ms · too fast to conclude", s.name);
                    }
                    Verdict::Linear(r) => {
                        println!("  {:<18} {a} ms → {b} ms · ×{r:.1}", s.name);
                    }
                    Verdict::Superlinear(r) => {
                        println!("  {:<18} {a} ms → {b} ms · ×{r:.1}", s.name);
                        superlinear.push(format!("{} (×{r:.1})", s.name));
                    }
                }
            }
            (Err(note), _) | (_, Err(note)) => {
                eprintln!("{note}");
                std::process::exit(2);
            }
        }
    }
    if measured == 0 {
        eprintln!(
            "\n`--only {}` names no shape — nothing was measured, and this \
             is not a clean sweep.",
            only.unwrap_or_default()
        );
        std::process::exit(2);
    }
    if superlinear.is_empty() {
        println!("\n{measured} shape(s) measured: none grows faster than its input.");
    } else {
        println!("\nSUPER-LINEAR COST: {}", superlinear.join(", "));
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{verdict, Verdict};

    /// **The verdict, on BOTH sides of the bar.**
    ///
    /// The numbers are those measured on 2026-09-09 on the removal of
    /// constant columns: before the fix 66,671 → 252,936 ms, after
    /// 4,564 → 9,156. This instrument's canary lives in its test.
    /// **Every shape enters through a door that exists.**
    ///
    /// The table is DATA: a row added tomorrow with an extension
    /// where a program name belongs would measure the generic compactor while
    /// believing it measures a reader.
    #[test]
    fn every_shape_enters_by_a_door_that_exists() {
        for s in super::SHAPES {
            assert!(
                super::door_is_consistent(s.prog, s.route),
                "`{}` enters through `{}`, which is not a door of its type",
                s.name,
                s.prog
            );
        }
        assert!(super::SHAPES.len() >= 17, "the sweep has shrunk");
        // And the question is asked on BOTH sides: an extension is not a
        // program, a program is not an extension.
        assert!(!super::door_is_consistent("rs", super::Route::Command));
        assert!(!super::door_is_consistent("ps aux", super::Route::Read));
        assert!(!super::door_is_consistent("ps aux", super::Route::Web));
    }

    /// **A selection that names nothing must not pass for a sweep.**
    #[test]
    fn a_selection_that_names_nothing_selects_nothing() {
        assert!(super::selected("read_an_html_page", None));
        assert!(super::selected("read_an_html_page", Some("html")));
        assert!(!super::selected("read_an_html_page", Some("zzz")));
        // And on the real table: the unknown fragment keeps NO shape,
        // which `main` turns into a refusal rather than an all-is-well.
        assert_eq!(
            super::SHAPES
                .iter()
                .filter(|s| super::selected(s.name, Some("zzz")))
                .count(),
            0
        );
        assert!(super::SHAPES
            .iter()
            .any(|s| super::selected(s.name, Some("html"))));
    }

    #[test]
    fn the_bar_separates_the_two_measured_shapes() {
        assert!(matches!(verdict(66_671, 252_936), Verdict::Superlinear(_)));
        assert!(matches!(verdict(4_564, 9_156), Verdict::Linear(_)));
        // A measurement that is too short returns NO verdict: a ratio over
        // a few milliseconds measures process start-up.
        assert_eq!(verdict(40, 900), Verdict::TooFast);
        // And the bar itself: exactly three passes, above it does not.
        assert!(matches!(verdict(1_000, 3_000), Verdict::Linear(_)));
        assert!(matches!(verdict(1_000, 3_001), Verdict::Superlinear(_)));
    }

    // A "is the shape even COMPACTED?" guard was written here then
    // REMOVED. It could not fire: at these sizes the line ceiling
    // folds anything, including noise chosen to be
    // incompressible — the canary stayed silent. A guard whose predicate is
    // satisfied by ANOTHER mechanism proves nothing, and the reason is written
    // here rather than dressed up as a precaution.
}
