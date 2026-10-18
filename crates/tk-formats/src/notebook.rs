//! Jupyter notebook (`.ipynb`) compaction.
//!
//! Real notebooks are **enormous and ~99% noise**: each cell carries
//! `metadata`, `id`, `execution_count`, and most damaging — every plot/image
//! is stored inline as base64 PNG/SVG (often megabytes per cell). On the
//! firecrawl 4.7 MB fixture only 0.6 % of the bytes are actual source code.
//!
//! [`compact_ipynb`] emits a compact markdown rendering: one section per
//! cell, source preserved, text outputs preserved, **binary outputs
//! summarised** as `[image: PNG 12.3 KB]` without their base64 payload.
//!
//! Survives malformed input (returns original verbatim, never panics).

use crate::tokenizer;
use serde_json::Value;
use std::fmt::Write;

/// Cells beyond this index are still rendered but their outputs are dropped
/// (the agent already saw the pattern by then).
pub const OUTPUTS_FOR_FIRST_N_CELLS: usize = 30;
/// Truncate long stdout/stderr blocks to this many characters per output.
pub const OUTPUT_TEXT_CAP: usize = 600;

/// Compact a Jupyter notebook JSON into agent-friendly markdown.
pub fn compact_ipynb(src: &str) -> String {
    let v: Value = match serde_json::from_str(src) {
        Ok(v) => v,
        Err(_) => return src.to_string(),
    };
    let mut out = String::with_capacity(src.len() / 50);
    let cells = match v.get("cells").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => return src.to_string(),
    };
    let language = v
        .get("metadata")
        .and_then(|m| m.get("kernelspec"))
        .and_then(|k| k.get("language"))
        .and_then(|l| l.as_str())
        .unwrap_or("python");
    let _ = write!(
        out,
        "# Jupyter notebook ({} cells, {language})\n\n",
        cells.len()
    );
    for (i, cell) in cells.iter().enumerate() {
        render_cell(&mut out, i, cell, language);
    }
    out.trim_end().to_string()
}

pub fn compact_ipynb_with_savings(src: &str) -> (String, crate::Savings) {
    let before = tokenizer::estimate(src);
    let out = compact_ipynb(src);
    let after = tokenizer::estimate(&out);
    (out, crate::Savings::new(before, after))
}

fn render_cell(out: &mut String, idx: usize, cell: &Value, lang: &str) {
    let kind = cell.get("cell_type").and_then(|c| c.as_str()).unwrap_or("");
    let source = join_source(cell.get("source"));
    if source.trim().is_empty() && kind != "code" {
        return; // empty markdown/raw cell: drop
    }
    match kind {
        "markdown" => {
            let _ = writeln!(out, "## Cell {} (markdown)", idx + 1);
            out.push_str(source.trim_end());
            out.push_str("\n\n");
        }
        "code" => {
            let exec = cell
                .get("execution_count")
                .and_then(|n| n.as_u64())
                .map(|n| format!(" [{n}]"))
                .unwrap_or_default();
            let _ = write!(
                out,
                "## Cell {} (code{exec})\n```{lang}\n{}\n```\n",
                idx + 1,
                source.trim_end()
            );
            // Outputs: keep stdout/stderr text, summarise binary.
            if idx < OUTPUTS_FOR_FIRST_N_CELLS {
                if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
                    for o in outputs {
                        render_output(out, o);
                    }
                }
            } else if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
                // Past the cap the ERROR still shows: the cell that failed is
                // often the last one run, deep in a long notebook, and it was
                // folded into "outputs dropped" with the plots and tables.
                let is_error =
                    |o: &&Value| o.get("output_type").and_then(|t| t.as_str()) == Some("error");
                for o in outputs.iter().filter(is_error) {
                    render_output(out, o);
                }
                if outputs.iter().any(|o| !is_error(&o)) {
                    out.push_str("> [outputs dropped after cell 30]\n");
                }
            }
            out.push('\n');
        }
        "raw" => {
            // raw cells: keep source as-is (LaTeX, etc.)
            let _ = write!(out, "## Cell {} (raw)\n{}\n\n", idx + 1, source.trim_end());
        }
        _ => {}
    }
}

fn render_output(out: &mut String, o: &Value) {
    let kind = o.get("output_type").and_then(|s| s.as_str()).unwrap_or("");
    match kind {
        "stream" => {
            let name = o.get("name").and_then(|s| s.as_str()).unwrap_or("stdout");
            let text = as_a_terminal_shows(&join_source(o.get("text")));
            quoted(
                out,
                &format!("{name}: {}", truncate_for_output(&text, OUTPUT_TEXT_CAP)),
            );
        }
        "error" => {
            let ename = o.get("ename").and_then(|s| s.as_str()).unwrap_or("Error");
            let evalue = o.get("evalue").and_then(|s| s.as_str()).unwrap_or("");
            // **The frames that say WHERE.** IPython's traceback list opens
            // with a dashed separator and a `KeyError  Traceback (most recent
            // call last)` header — the first two entries, which were all this
            // kept, in raw ANSI colour (`[0;31mKeyError[0m`). The user's cell
            // (`Cell In[4], line 1 ----> 1 df['q9']`) and the frame that raised
            // come after; the last entry repeats `ename: evalue`, said above.
            let frames: Vec<String> = o
                .get("traceback")
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|f| crate::ansi::strip_ansi(f).trim_end().to_string())
                        .filter(|f| !f.trim().is_empty() && !f.trim().chars().all(|c| c == '-'))
                        .filter(|f| !f.contains("Traceback (most recent call last)"))
                        .collect()
                })
                .unwrap_or_default();
            let last_line = format!("{ename}: {evalue}");
            let frames: Vec<&String> = frames.iter().filter(|f| f.trim() != last_line).collect();
            let cell = frames
                .iter()
                .position(|f| f.contains("Cell In[") || f.contains("ipython-input"));
            let mut keep: Vec<&String> = Vec::new();
            if let Some(c) = cell {
                keep.push(frames[c]);
            }
            if let Some(last) = frames.last() {
                if cell != Some(frames.len() - 1) {
                    keep.push(last);
                }
            }
            let tb = keep
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let _ = writeln!(out, "> ERROR {ename}: {evalue}");
            if !tb.is_empty() {
                quoted(out, &truncate_for_output(&tb, OUTPUT_TEXT_CAP));
            }
        }
        "execute_result" | "display_data" => {
            if let Some(data) = o.get("data").and_then(|d| d.as_object()) {
                // Prefer text/plain. Drop image/* and application/* binary data.
                // Prefer text/plain; without it, the richest TEXT the cell
                // gave. A pandas Styler, `display(HTML(…))` or
                // `display(Markdown(…))` carry no text/plain, and the cell's
                // whole result vanished without a mark.
                let shown = if let Some(t) = data.get("text/plain") {
                    Some(join_source(Some(t)))
                } else if let Some(t) = data.get("text/markdown").or_else(|| data.get("text/latex"))
                {
                    Some(join_source(Some(t)))
                } else {
                    data.get("text/html")
                        .map(|t| html_as_text(&join_source(Some(t))))
                };
                if let Some(s) = shown.filter(|s| !s.trim().is_empty()) {
                    quoted(
                        out,
                        &format!("result: {}", truncate_for_output(&s, OUTPUT_TEXT_CAP)),
                    );
                }
                for (mime, val) in data {
                    if mime.starts_with("image/") || mime.starts_with("application/") {
                        let size = approx_b64_bytes(val);
                        let _ = writeln!(out, "> [{mime}: {} bytes (stripped)]", size);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Notebook `source` and `text` fields are either a `String` or an array of
/// strings (one per line). Join either form.
/// Every line under `> `: a multi-line result quoted on its first line only
/// left the table rows (`0  EMEA  100`) outside the output they belong to.
fn quoted(out: &mut String, text: &str) {
    for line in text.trim_end().lines() {
        let _ = writeln!(out, "> {line}");
    }
}

/// **A stream as the notebook SHOWED it.** A progress bar (tqdm, pip, Keras)
/// rewrites its line with `\r`, and the notebook stores every state: fifty
/// `\r  2%|…`, `\r  4%|…` on one line, cut by the output cap before the
/// last one — so the reader saw `6%` and never `100%`, the state the cell
/// ended on. Each line keeps what follows its last carriage return.
fn as_a_terminal_shows(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let shown = line
            .rsplit('\r')
            .find(|seg| !seg.trim().is_empty())
            .unwrap_or("");
        out.push(shown);
    }
    out.join("\n")
}

/// An HTML result as text: table rows as `| a | b |`, other markup dropped
/// with its block breaks kept.
pub(crate) fn html_as_text(html: &str) -> String {
    let re = tk_base::regexes::re;
    let tag = re(r"(?s)<[^>]*>");
    let clean = |s: &str| {
        let t = tag.replace_all(s, "");
        t.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&nbsp;", " ")
            .replace("&quot;", "\"")
            .trim()
            .to_string()
    };
    let rows: Vec<String> = re(r"(?is)<tr\b[^>]*>(.*?)</tr>")
        .captures_iter(html)
        .map(|r| {
            let cells: Vec<String> = re(r"(?is)<t[hd]\b[^>]*>(.*?)</t[hd]>")
                .captures_iter(&r[1])
                .map(|c| clean(&c[1]))
                .collect();
            format!("| {} |", cells.join(" | "))
        })
        .collect();
    if !rows.is_empty() {
        return rows.join("\n");
    }
    let broken = re(r"(?i)<br\s*/?>|</(p|div|li|h[1-6])>").replace_all(html, "\n");
    broken
        .lines()
        .map(clean)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn join_source(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str())
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn truncate_for_output(s: &str, cap: usize) -> String {
    crate::head_tail::head_and_tail(s, cap)
}

/// Approximate byte size of a (possibly base64) value without keeping the string.
fn approx_b64_bytes(v: &Value) -> usize {
    match v {
        Value::String(s) => (s.len() * 3) / 4,
        Value::Array(a) => a.iter().map(approx_b64_bytes).sum(),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nb(cells: &str) -> String {
        format!(
            r#"{{"cells":[{cells}],"metadata":{{"kernelspec":{{"language":"python"}}}},"nbformat":4,"nbformat_minor":5}}"#
        )
    }

    #[test]
    fn keeps_source_drops_metadata_and_id() {
        let raw = nb(
            r#"{"cell_type":"code","source":["print('x')\n"],"metadata":{"a":1},"id":"abc","execution_count":3,"outputs":[]}"#,
        );
        let out = compact_ipynb(&raw);
        assert!(out.contains("print('x')"));
        assert!(!out.contains("\"id\""));
        assert!(!out.contains("\"metadata\""));
    }

    #[test]
    fn strips_base64_image_data() {
        let big = "A".repeat(100_000);
        let raw = nb(&format!(
            r#"{{"cell_type":"code","source":["plot()"],"outputs":[{{"output_type":"display_data","data":{{"image/png":"{big}","text/plain":"<Figure>"}}}}]}}"#
        ));
        let out = compact_ipynb(&raw);
        assert!(out.contains("plot()"));
        assert!(out.contains("image/png"));
        assert!(out.contains("(stripped)"));
        assert!(!out.contains(&big), "base64 payload leaked");
    }

    #[test]
    fn renders_markdown_and_code_cells() {
        let raw = nb(
            r##"{"cell_type":"markdown","source":["# Title\nIntro."]},{"cell_type":"code","source":["x=1"]}"##,
        );
        let out = compact_ipynb(&raw);
        assert!(out.contains("Cell 1 (markdown)"));
        assert!(out.contains("# Title"));
        assert!(out.contains("Cell 2 (code)"));
        assert!(out.contains("```python"));
        assert!(out.contains("x=1"));
    }

    /// IPython's own traceback list: separator, header, the user's cell, the
    /// library frame that raised, then `ename: evalue` again — in ANSI colour.
    #[test]
    fn an_error_keeps_the_cell_and_the_raising_frame_without_colour() {
        let tb = [
            "\u{1b}[0;31m---------------------------------------------------------------------------\u{1b}[0m",
            "\u{1b}[0;31mKeyError\u{1b}[0m                                  Traceback (most recent call last)",
            "Cell \u{1b}[0;32mIn[4], line 1\u{1b}[0m\n\u{1b}[0;32m----> 1\u{1b}[0m df[\u{1b}[38;5;124m'q9'\u{1b}[39m]",
            "File \u{1b}[0;32m~/env/pandas/core/frame.py:3761\u{1b}[0m, in \u{1b}[0;36mDataFrame.__getitem__\u{1b}[0;34m(self, key)\u{1b}[0m",
            "\u{1b}[0;31mKeyError\u{1b}[0m: 'q9'",
        ];
        let tb_json: Vec<String> = tb
            .iter()
            .map(|t| serde_json::to_string(t).unwrap())
            .collect();
        let cells = format!(
            r#"{{"cell_type":"code","execution_count":4,"metadata":{{}},"source":["df['q9']"],"outputs":[{{"output_type":"error","ename":"KeyError","evalue":"'q9'","traceback":[{}]}}]}},{{"cell_type":"code","execution_count":2,"metadata":{{}},"source":["df.head()"],"outputs":[{{"output_type":"execute_result","execution_count":2,"metadata":{{}},"data":{{"text/plain":["   region  q1\n","0  EMEA  100"]}}}}]}}"#,
            tb_json.join(",")
        );
        let out = compact_ipynb(&nb(&cells));
        assert!(out.contains("> ERROR KeyError: 'q9'"), "{out}");
        assert!(
            out.contains("> Cell In[4], line 1\n> ----> 1 df['q9']"),
            "{out}"
        );
        assert!(
            out.contains(
                "> File ~/env/pandas/core/frame.py:3761, in DataFrame.__getitem__(self, key)"
            ),
            "{out}"
        );
        assert!(!out.contains("[0;"), "colour codes leaked: {out}");
        assert!(!out.contains("------"), "{out}");
        assert!(!out.contains("Traceback (most recent"), "{out}");
        assert!(
            out.contains("> result:    region  q1\n> 0  EMEA  100"),
            "{out}"
        );
    }

    /// A progress bar stored with every `\r` state shows its LAST one.
    #[test]
    fn a_progress_bar_shows_the_state_it_ended_on() {
        let prog: String = (0..=100)
            .step_by(2)
            .map(|p| format!("\r{p:3}%| {p}/100"))
            .collect::<String>()
            + "\n";
        let cells = format!(
            r#"{{"cell_type":"code","execution_count":1,"metadata":{{}},"source":["train()"],"outputs":[{{"output_type":"stream","name":"stderr","text":[{}]}}]}}"#,
            serde_json::to_string(&prog).unwrap()
        );
        let out = compact_ipynb(&nb(&cells));
        assert!(out.contains("> stderr: 100%| 100/100"), "{out}");
        assert!(!out.contains(" 2%|") && !out.contains('\r'), "{out}");
    }

    /// Past the output cap, a cell's ERROR still shows; its other outputs go.
    #[test]
    fn an_error_past_the_output_cap_still_shows() {
        let filler: Vec<String> = (0..35)
            .map(|i| format!(r#"{{"cell_type":"code","execution_count":{i},"metadata":{{}},"source":["x{i}"],"outputs":[]}}"#))
            .collect();
        let late = r#"{"cell_type":"code","execution_count":36,"metadata":{},"source":["boom()"],"outputs":[{"output_type":"stream","name":"stdout","text":["LATE-STDOUT\n"]},{"output_type":"error","ename":"ValueError","evalue":"bad shape","traceback":["Cell In[36], line 1\n----> 1 boom()"]}]}"#;
        let out = compact_ipynb(&nb(&format!("{},{late}", filler.join(","))));
        assert!(out.contains("> ERROR ValueError: bad shape"), "{out}");
        assert!(out.contains("> Cell In[36], line 1"), "{out}");
        assert!(
            !out.contains("LATE-STDOUT") && out.contains("[outputs dropped after cell 30]"),
            "{out}"
        );
    }

    /// A result with no text/plain — a Styler's HTML table, a Markdown display
    /// — shows its own text instead of vanishing.
    #[test]
    fn a_result_without_plain_text_still_shows() {
        let cells = r#"{"cell_type":"code","execution_count":1,"metadata":{},"source":["styled"],"outputs":[{"output_type":"execute_result","execution_count":1,"metadata":{},"data":{"text/html":["<table><thead><tr><th>region</th><th>q1</th></tr></thead><tbody><tr><td>EMEA</td><td>100</td></tr><tr><td>R&amp;D</td><td>80</td></tr></tbody></table>"]}}]},{"cell_type":"code","execution_count":2,"metadata":{},"source":["md"],"outputs":[{"output_type":"display_data","metadata":{},"data":{"text/markdown":["**Total: 180**"],"text/html":["<b>ignored</b>"]}}]},{"cell_type":"code","execution_count":3,"metadata":{},"source":["h"],"outputs":[{"output_type":"display_data","metadata":{},"data":{"text/html":["<p>First</p><p>Second<br>line</p>"]}}]}"#;
        let out = compact_ipynb(&nb(cells));
        assert!(
            out.contains("> result: | region | q1 |\n> | EMEA | 100 |\n> | R&D | 80 |"),
            "{out}"
        );
        assert!(
            out.contains("> result: **Total: 180**") && !out.contains("ignored"),
            "{out}"
        );
        assert!(out.contains("> result: First\n> Second\n> line"), "{out}");
    }

    /// A long output keeps its last lines — the final epoch — not only its first.
    #[test]
    fn a_long_output_keeps_its_end() {
        let log: String = (1..=50)
            .map(|e| {
                format!(
                    "Epoch {e:02}/50 loss=0.{:03} acc=0.{:03}\n",
                    900 - e * 10,
                    500 + e * 9
                )
            })
            .collect();
        let out = truncate_for_output(&log, OUTPUT_TEXT_CAP);
        assert!(out.starts_with("Epoch 01/50"), "{out}");
        assert!(out.ends_with("Epoch 50/50 loss=0.400 acc=0.950"), "{out}");
        assert!(
            out.contains("line(s)") && out.chars().count() < OUTPUT_TEXT_CAP + 60,
            "{out}"
        );
        // One long line falls back to characters.
        let long = "x".repeat(2000);
        assert!(truncate_for_output(&long, OUTPUT_TEXT_CAP).ends_with("…[+1400 chars]"));
    }

    #[test]
    fn malformed_returns_original() {
        let raw = "{not json";
        assert_eq!(compact_ipynb(raw), raw);
    }

    #[test]
    fn savings_real_shape() {
        let big = "B".repeat(50_000);
        let raw = nb(&format!(
            r#"{{"cell_type":"code","source":["a=1\n"],"outputs":[{{"output_type":"display_data","data":{{"image/png":"{big}"}}}}]}}"#
        ));
        let (_md, sav) = compact_ipynb_with_savings(&raw);
        assert!(sav.percent() > 90.0, "got {:?}", sav);
    }
}
