//! Native function-calling for local sub-agents.
//!
//! Small local models (MiniCPM5, Qwen2.5) are TRAINED to call tools in their
//! own format — presented via the chat template's `tools` variable and emitted
//! as `<tool_call>` / `<function name=…>` markup. This module builds the tool
//! schemas, parses the model's tool-call output, and maps a call back to a
//! [`tools::Tool`] for sandboxed execution. (Our old prose "emit READ <path>"
//! protocol fought that training and small models ignored it.)

use super::sandbox::{Capability, Sandbox};
use super::tools::Tool;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// A parsed tool call: function name + string arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    pub args: BTreeMap<String, String>,
}

/// OpenAI-style tool schemas the model may call, gated by sandbox caps. Passed
/// to the chat template's `tools` variable.
pub fn tool_specs(sb: &Sandbox) -> Value {
    let mut specs = vec![func(
        "read_file",
        "Read a text file's contents. Use this to inspect files before answering.",
        json!({"path": {"type": "string", "description": "path to the file, relative to the working directory"}}),
        &["path"],
    )];
    if sb.has(Capability::Shell) {
        specs.push(func(
            "run_bash",
            "Run a one-line shell command (read-only filesystem, writable /tmp only) and get its output. Good for ls, grep, wc, cat.",
            json!({"command": {"type": "string", "description": "the shell command"}}),
            &["command"],
        ));
        specs.push(func(
            "exec_code",
            "Run a short script in an interpreter (python/node/ruby/bash…) and get its stdout. Sandboxed.",
            json!({
                "language": {"type": "string", "description": "python, bash, node, ruby, …"},
                "code": {"type": "string", "description": "the source code to run"}
            }),
            &["language", "code"],
        ));
    }
    if sb.has(Capability::Net) {
        specs.push(func(
            "web_search",
            "Search the web and get folded result snippets.",
            json!({"query": {"type": "string"}}),
            &["query"],
        ));
        specs.push(func(
            "fetch_url",
            "Fetch a web page (http/https) and get its content as compact markdown.",
            json!({"url": {"type": "string"}}),
            &["url"],
        ));
    }
    if sb.has(Capability::Write) {
        specs.push(func(
            "write_file",
            "Create or overwrite a file YOU created (cannot touch pre-existing files).",
            json!({"path": {"type": "string"}, "content": {"type": "string"}}),
            &["path", "content"],
        ));
    }
    if sb.has(Capability::Delete) {
        specs.push(func(
            "delete_file",
            "Delete a file YOU created this run.",
            json!({"path": {"type": "string"}}),
            &["path"],
        ));
    }
    Value::Array(specs)
}

fn func(name: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": desc,
            "parameters": {
                "type": "object",
                "properties": props,
                "required": required,
            }
        }
    })
}

/// Map a parsed tool call to an executable [`Tool`]. Unknown tool / missing
/// required arg → Err (fed back to the model, no blind guess).
pub fn call_to_tool(tc: &ToolCall) -> Result<Tool, String> {
    let arg = |k: &str| tc.args.get(k).cloned();
    let req = |k: &str| arg(k).ok_or_else(|| format!("`{}` needs argument `{k}`", tc.name));
    match tc.name.as_str() {
        "read_file" => Ok(Tool::Read {
            path: req("path")?,
            range: None,
        }),
        "run_bash" => Ok(Tool::Bash {
            cmd: req("command")?,
        }),
        "exec_code" => Ok(Tool::Exec {
            lang: req("language")?,
            code: req("code")?,
        }),
        "web_search" => Ok(Tool::Search {
            query: req("query")?,
        }),
        "fetch_url" => Ok(Tool::Web { url: req("url")? }),
        "write_file" => Ok(Tool::Write {
            path: req("path")?,
            content: req("content")?,
        }),
        "delete_file" => Ok(Tool::Delete { path: req("path")? }),
        other => Err(format!("unknown tool `{other}`")),
    }
}

/// Parse tool calls from a model's raw output. Handles both shapes seen in the
/// wild:
///   * XML (MiniCPM5): `<function name="X"><param name="K">V</param>…</function>`
///   * JSON (Qwen/Hermes): `<tool_call>{"name":"X","arguments":{…}}</tool_call>`
/// Returns empty when the model produced a normal (final-answer) reply.
pub fn parse_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut calls = xml_calls(text);
    if calls.is_empty() {
        calls = json_calls(text);
    }
    // Reject anything whose name isn't a real tool: the repair pass is
    // deliberately aggressive, so prose that merely contains "name", a stray
    // `{…}`, or partial JSON could otherwise masquerade as a call. A plain
    // final answer must never be mistaken for a tool invocation.
    calls.retain(|c| is_known_tool(&c.name));
    calls
}

/// The tool names the sub-agent may emit (mirrors `tool_specs`/`call_to_tool`).
fn is_known_tool(name: &str) -> bool {
    matches!(
        name,
        "read_file"
            | "run_bash"
            | "exec_code"
            | "web_search"
            | "fetch_url"
            | "write_file"
            | "delete_file"
    )
}

fn xml_calls(text: &str) -> Vec<ToolCall> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<function") {
        rest = &rest[i..];
        // name="..."
        let Some(name) = between(rest, "name=\"", "\"") else {
            break;
        };
        // body up to </function> (or end)
        let body_end = rest.find("</function>").unwrap_or(rest.len());
        let body = &rest[..body_end];
        let mut args = BTreeMap::new();
        let mut p = body;
        while let Some(ppos) = p.find("<param name=\"") {
            let tag = &p[ppos..]; // starts at "<param name=\"…"
            let Some(k) = between(tag, "<param name=\"", "\"") else {
                break;
            };
            // value is between this tag's closing '>' and </param>
            let Some(gt) = tag.find('>') else { break };
            let after = &tag[gt + 1..];
            let v = after.split("</param>").next().unwrap_or("").trim();
            args.insert(k, v.to_string());
            p = after.split_once("</param>").map(|(_, r)| r).unwrap_or("");
        }
        out.push(ToolCall { name, args });
        rest = &rest[body_end.min(rest.len())..];
        if let Some(stripped) = rest.strip_prefix("</function>") {
            rest = stripped;
        }
    }
    out
}

fn json_calls(text: &str) -> Vec<ToolCall> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<tool_call>") {
        let after = &rest[start + "<tool_call>".len()..];
        let blob = after.split("</tool_call>").next().unwrap_or(after);
        push_json_call(blob, &mut out);
        rest = after.split("</tool_call>").nth(1).unwrap_or("");
    }
    // Bare object with name/arguments (no wrapper).
    if out.is_empty() {
        push_json_call(text, &mut out);
    }
    out
}

/// Parse a `{"name":…,"arguments":{…}}` object out of `blob`, tolerating models
/// that wrap it in extra braces (`{{…}}`, seen with Qwen) or surround it with
/// prose: we extract the balanced object around the first `"name"` key.
fn push_json_call(blob: &str, out: &mut Vec<ToolCall>) {
    if let Some(obj) = balanced_object_with_name(blob) {
        // Small models routinely emit slightly-broken JSON in tool args (single
        // quotes, Python literals, `\*` escapes, trailing commas, truncated
        // braces). Parse strict first, then retry a repaired copy.
        let v = serde_json::from_str::<Value>(&obj)
            .or_else(|_| serde_json::from_str::<Value>(&repair_json(&obj)));
        if let Ok(v) = v {
            if let Some(tc) = json_to_call(&v) {
                out.push(tc);
            }
        }
    }
}

/// Best-effort repair of malformed JSON small models emit. A single
/// string-aware state-machine pass that: converts single-quoted strings/keys to
/// double-quoted (escaping any inner `"`), doubles invalid backslash escapes
/// (`\*` → literal), maps Python literals (`True`/`False`/`None`) to JSON,
/// quotes bare identifier keys/values, strips `//` and `/* */` comments and
/// trailing commas, closes an unterminated string, and balances open brackets.
/// Only runs after a strict parse fails, so it never corrupts valid JSON.
fn repair_json(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut stack: Vec<char> = Vec::new(); // expected closers
    let mut in_str = false;
    let mut quote = '"';
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if in_str {
            if c == '\\' {
                match cs.get(i + 1) {
                    Some(&n)
                        if matches!(n, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u') =>
                    {
                        out.push('\\');
                        out.push(n);
                        i += 2;
                    }
                    _ => {
                        out.push_str("\\\\"); // stray backslash → literal
                        i += 1;
                    }
                }
            } else if c == quote {
                out.push('"'); // normalize the closing delimiter to "
                in_str = false;
                i += 1;
            } else if c == '"' {
                out.push_str("\\\""); // literal " inside a single-quoted string
                i += 1;
            } else {
                out.push(c);
                i += 1;
            }
            continue;
        }
        // Outside a string.
        if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && cs.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < cs.len() && !(cs[i] == '*' && cs[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if c == '"' || c == '\'' {
            in_str = true;
            quote = c;
            out.push('"');
            i += 1;
        } else if c == '{' || c == '[' {
            stack.push(if c == '{' { '}' } else { ']' });
            out.push(c);
            i += 1;
        } else if c == '}' || c == ']' {
            drop_trailing_comma(&mut out);
            stack.pop();
            out.push(c);
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '_') {
                i += 1;
            }
            let word: String = cs[start..i].iter().collect();
            match word.as_str() {
                "True" | "true" => out.push_str("true"),
                "False" | "false" => out.push_str("false"),
                "None" | "null" | "NULL" | "nil" => out.push_str("null"),
                // Bare identifier (unquoted key or single-word value) → quote it.
                _ => {
                    out.push('"');
                    out.push_str(&word);
                    out.push('"');
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    if in_str {
        out.push('"');
    }
    drop_trailing_comma(&mut out);
    while let Some(closer) = stack.pop() {
        out.push(closer);
    }
    out
}

/// Remove a trailing comma (and any whitespace before it) from `out`.
fn drop_trailing_comma(out: &mut String) {
    let n = out.trim_end().len();
    out.truncate(n);
    if out.ends_with(',') {
        out.pop();
    }
}

fn balanced_object_with_name(s: &str) -> Option<String> {
    // Locate the `name` key — quoted ("name"/'name') or bare (unquoted key).
    let key = s
        .find("\"name\"")
        .or_else(|| s.find("'name'"))
        .or_else(|| s.find("name"))?;
    let start = s[..key].rfind('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut quote = '"';
    let mut prev = '\0';
    for (i, c) in s[start..].char_indices() {
        if in_str {
            if c == quote && prev != '\\' {
                in_str = false;
            }
        } else if c == '"' || c == '\'' {
            in_str = true;
            quote = c;
        } else if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(s[start..start + i + c.len_utf8()].to_string());
            }
        }
        prev = c;
    }
    // Truncated (generation cut off before the closing brace): return the tail
    // from the opening `{` so `repair_json` can balance it.
    Some(s[start..].to_string())
}

fn json_to_call(v: &Value) -> Option<ToolCall> {
    let name = v.get("name")?.as_str()?.to_string();
    let mut args = BTreeMap::new();
    // Qwen/Hermes use "arguments"; Llama-3.x uses "parameters". Either may be a
    // nested object or a JSON-encoded string.
    let params = v.get("arguments").or_else(|| v.get("parameters"));
    let owned_str;
    let obj = match params {
        Some(Value::Object(o)) => Some(o),
        Some(Value::String(s)) => {
            owned_str = serde_json::from_str::<Value>(s).ok();
            owned_str.as_ref().and_then(|x| x.as_object())
        }
        _ => None,
    };
    if let Some(obj) = obj {
        for (k, val) in obj {
            let s = val
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| val.to_string());
            args.insert(k.clone(), s);
        }
    }
    Some(ToolCall { name, args })
}

/// Substring between the first `open` and the next `close` after it.
fn between(hay: &str, open: &str, close: &str) -> Option<String> {
    let i = hay.find(open)? + open.len();
    let j = hay[i..].find(close)? + i;
    Some(hay[i..j].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minicpm_xml() {
        let t =
            r#"sure<function name="read_file"><param name="path">Cargo.toml</param></function>"#;
        let c = parse_tool_calls(t);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "read_file");
        assert_eq!(c[0].args.get("path").unwrap(), "Cargo.toml");
    }

    #[test]
    fn parses_two_params_xml() {
        let t = r#"<function name="exec_code"><param name="language">python</param><param name="code">print(6*7)</param></function>"#;
        let c = parse_tool_calls(t);
        assert_eq!(c[0].name, "exec_code");
        assert_eq!(c[0].args.get("language").unwrap(), "python");
        assert_eq!(c[0].args.get("code").unwrap(), "print(6*7)");
    }

    #[test]
    fn parses_json_tool_call() {
        let t = r#"<tool_call>{"name":"run_bash","arguments":{"command":"ls src"}}</tool_call>"#;
        let c = parse_tool_calls(t);
        assert_eq!(c[0].name, "run_bash");
        assert_eq!(c[0].args.get("command").unwrap(), "ls src");
    }

    #[test]
    fn parses_double_brace_json() {
        // Qwen sometimes wraps the call in extra braces.
        let t = "<tool_call>\n{{\"name\": \"read_file\", \"arguments\": {\"path\": \"Cargo.toml\"}}}\n</tool_call>";
        let c = parse_tool_calls(t);
        assert_eq!(c.len(), 1, "got {c:?}");
        assert_eq!(c[0].name, "read_file");
        assert_eq!(c[0].args.get("path").unwrap(), "Cargo.toml");
    }

    #[test]
    fn parses_llama_parameters_key() {
        // Llama-3.x: bare object, "parameters" key, no wrapper.
        let t = r#"{"name": "read_file", "parameters": {"path": "Cargo.toml"}}"#;
        let c = parse_tool_calls(t);
        assert_eq!(c.len(), 1, "got {c:?}");
        assert_eq!(c[0].args.get("path").unwrap(), "Cargo.toml");
    }

    #[test]
    fn parses_stringified_arguments() {
        let t = r#"<tool_call>{"name":"run_bash","arguments":"{\"command\":\"ls\"}"}</tool_call>"#;
        let c = parse_tool_calls(t);
        assert_eq!(c[0].args.get("command").unwrap(), "ls");
    }

    #[test]
    fn parses_invalid_backslash_escape() {
        // `\*` is not a legal JSON escape; the repair pass must recover the call.
        let t = r#"<tool_call>{"name":"run_bash","arguments":{"command":"find src -name \*.rs | wc -l"}}</tool_call>"#;
        let c = parse_tool_calls(t);
        assert_eq!(c.len(), 1, "got {c:?}");
        assert_eq!(c[0].name, "run_bash");
        assert!(c[0].args.get("command").unwrap().contains("*.rs"));
    }

    /// Adversarial battery: escalating JSON malformations small models emit.
    /// Each case wraps a call in `<tool_call>` and must still parse to the right
    /// name + recover the key argument. Add the nastiest thing you can think of.
    #[test]
    fn adversarial_repair_battery() {
        // (raw inner json, expected name, arg key, substring the arg must contain)
        let cases: &[(&str, &str, &str, &str)] = &[
            // 0. valid (control)
            (r#"{"name":"read_file","arguments":{"path":"Cargo.toml"}}"#, "read_file", "path", "Cargo.toml"),
            // 1. invalid \* escape in a shell command
            (r#"{"name":"run_bash","arguments":{"command":"find src -name \*.rs"}}"#, "run_bash", "command", "*.rs"),
            // 2. single-quoted strings & keys
            (r#"{'name':'read_file','arguments':{'path':'Cargo.toml'}}"#, "read_file", "path", "Cargo.toml"),
            // 3. Python literals as extra values
            (r#"{"name":"read_file","arguments":{"path":"x","deep":True,"skip":None}}"#, "read_file", "path", "x"),
            // 4. trailing commas
            (r#"{"name":"read_file","arguments":{"path":"Cargo.toml",},}"#, "read_file", "path", "Cargo.toml"),
            // 5. unquoted identifier keys
            (r#"{name:"read_file",arguments:{path:"Cargo.toml"}}"#, "read_file", "path", "Cargo.toml"),
            // 6. // and /* */ comments
            ("{\"name\":\"read_file\", // pick the file\n\"arguments\":{\"path\":\"Cargo.toml\"}}", "read_file", "path", "Cargo.toml"),
            // 7. truncated: outer brace never emitted (generation cut off)
            (r#"{"name":"read_file","arguments":{"path":"Cargo.toml"}"#, "read_file", "path", "Cargo.toml"),
            // 8. double-quote literal inside a single-quoted value
            (r#"{'name':'run_bash','arguments':{'command':'grep "fn main" src'}}"#, "run_bash", "command", "fn main"),
            // 9. everything at once: single quotes + \d + inner " + trailing comma + truncation
            (r#"{'name': 'run_bash', 'arguments': {'command': 'grep \d "x" -r',},"#, "run_bash", "command", "grep"),
        ];
        for (i, (raw, name, key, needle)) in cases.iter().enumerate() {
            let wrapped = format!("<tool_call>{raw}</tool_call>");
            let c = parse_tool_calls(&wrapped);
            assert_eq!(c.len(), 1, "case {i}: no call parsed from {raw}");
            assert_eq!(&c[0].name, name, "case {i}: wrong name");
            let got = c[0]
                .args
                .get(*key)
                .unwrap_or_else(|| panic!("case {i}: missing arg {key}: {:?}", c[0].args));
            assert!(
                got.contains(needle),
                "case {i}: arg {key}={got:?} lacks {needle:?}"
            );
        }
    }

    /// 100+ generated cases, each a base call put through escalating layers of
    /// corruption (singly, then cumulatively) — the adversarial strategy from
    /// the Lea `decode_json` module, reproduced. All must recover.
    #[test]
    fn adversarial_repair_100() {
        fn ins_before_last_brace(s: &str, ins: &str) -> String {
            match s.rfind('}') {
                Some(p) => {
                    let mut o = s.to_string();
                    o.insert_str(p, ins);
                    o
                }
                None => s.to_string(),
            }
        }
        fn c_ws(s: &str) -> String {
            s.replace(':', " : ").replace(',', " ,\n ")
        }
        fn c_block(s: &str) -> String {
            s.replacen('{', "{/* pick */", 1)
        }
        fn c_line(s: &str) -> String {
            s.replacen(',', ", // note\n", 1)
        }
        fn c_bool(s: &str) -> String {
            ins_before_last_brace(s, r#","_a":True"#)
        }
        fn c_none(s: &str) -> String {
            ins_before_last_brace(s, r#","_b":None"#)
        }
        fn c_esc(s: &str) -> String {
            ins_before_last_brace(s, r#","re":"\d+\w\*""#)
        }
        fn c_trail(s: &str) -> String {
            s.replace('}', ",}").replace(']', ",]")
        }
        fn c_unquote(s: &str) -> String {
            let mut o = s.to_string();
            for k in [
                "name",
                "arguments",
                "path",
                "command",
                "language",
                "code",
                "query",
                "_a",
                "_b",
                "re",
            ] {
                o = o.replace(&format!("\"{k}\":"), &format!("{k}:"));
            }
            o
        }
        fn c_single(s: &str) -> String {
            s.replace('"', "'")
        }
        fn c_dbrace(s: &str) -> String {
            format!("{{{s}}}")
        }
        fn c_trunc(s: &str) -> String {
            let t = s.trim_end();
            if t.ends_with('}') {
                t[..t.len() - 1].to_string()
            } else {
                s.to_string()
            }
        }
        fn c_prose(s: &str) -> String {
            format!("Sure, I'll call it:\n{s}\nDone.")
        }

        let corruptors: [fn(&str) -> String; 12] = [
            c_ws, c_block, c_line, c_bool, c_none, c_esc, c_trail, c_unquote, c_single, c_dbrace,
            c_trunc, c_prose,
        ];
        // Bases chosen so no value contains a `:` or `,` (keeps corruptors safe).
        let bases: [(&str, &str, &str, &str); 4] = [
            (
                r#"{"name":"read_file","arguments":{"path":"Cargo.toml"}}"#,
                "read_file",
                "path",
                "Cargo.toml",
            ),
            (
                r#"{"name":"run_bash","arguments":{"command":"grep foo src"}}"#,
                "run_bash",
                "command",
                "grep foo",
            ),
            (
                r#"{"name":"exec_code","arguments":{"language":"python","code":"print(1)"}}"#,
                "exec_code",
                "code",
                "print(1)",
            ),
            (
                r#"{"name":"web_search","arguments":{"query":"rust tokio release"}}"#,
                "web_search",
                "query",
                "rust tokio",
            ),
        ];

        let mut cases: Vec<(String, &str, &str, &str)> = Vec::new();
        for (b, n, k, v) in bases {
            // Each corruptor alone.
            for c in corruptors {
                cases.push((c(b), n, k, v));
            }
            // Cumulative: layer them on, one harder each level.
            let mut cur = b.to_string();
            for c in corruptors {
                cur = c(&cur);
                cases.push((cur.clone(), n, k, v));
            }
            // Fully-corrupted, wrapped in <tool_call> too.
            cases.push((format!("<tool_call>{cur}</tool_call>"), n, k, v));
        }

        assert!(cases.len() >= 100, "only {} cases generated", cases.len());
        let mut fails = Vec::new();
        for (i, (inp, n, k, v)) in cases.iter().enumerate() {
            let c = parse_tool_calls(inp);
            let ok =
                c.len() == 1 && &c[0].name == n && c[0].args.get(*k).is_some_and(|a| a.contains(v));
            if !ok {
                fails.push(format!("#{i}: {inp:?}\n     -> {c:?}"));
            }
        }
        assert!(
            fails.is_empty(),
            "{}/{} failed:\n{}",
            fails.len(),
            cases.len(),
            fails.join("\n")
        );
    }

    #[test]
    fn plain_answer_has_no_calls() {
        assert!(parse_tool_calls("The version is 1.0.102.").is_empty());
    }

    /// Non-tool text must NOT be coerced into a call, even when it contains the
    /// word "name", stray braces, or non-tool JSON. No false positives.
    #[test]
    fn rejects_non_tool_calls() {
        let negatives = [
            "The version is 1.0.102.",
            "My name is Claude and the answer is 42.",
            "See the config {name: the setting} which is off.",
            "Here are the results:\n- item one\n- item two",
            "{ this is not json at all }",
            "",
            "I couldn't find the file you asked about.",
            r#"{"result": 42, "status": "ok"}"#, // valid JSON, but no tool
            r#"{"name": "frobnicate", "arguments": {}}"#, // unknown tool
            "function names in rust use snake_case",
            "```\nsome code block\n```",
            "the file to read_file is over there", // tool word in prose, no JSON
        ];
        for n in negatives {
            let c = parse_tool_calls(n);
            assert!(c.is_empty(), "false positive on {n:?}: {c:?}");
        }
    }

    #[test]
    fn maps_calls_to_tools() {
        let tc = ToolCall {
            name: "read_file".into(),
            args: [("path".to_string(), "a.rs".to_string())].into(),
        };
        assert!(matches!(call_to_tool(&tc), Ok(Tool::Read { .. })));
        let bad = ToolCall {
            name: "nope".into(),
            args: BTreeMap::new(),
        };
        assert!(call_to_tool(&bad).is_err());
    }

    #[test]
    fn specs_gated_by_caps() {
        let ro = Sandbox::read_only(std::path::Path::new("."));
        let specs = tool_specs(&ro);
        let names: Vec<String> = specs
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(
            !names.contains(&"run_bash".to_string()),
            "read-only box exposes no shell"
        );
    }
}
