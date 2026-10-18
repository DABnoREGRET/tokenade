//! **Who calls whom, and under which name.**
//!
//! A call rarely carries the name of what it calls: there is the receiver, the
//! qualifier, the import alias. Recovering the USEFUL name is the whole point,
//! and getting it wrong here fills the graph with edges to namesakes.

use super::*;

/// Variant of [`walk`] that also harvests call edges. Threads
/// `current_caller` through recursion so each call site is tagged
/// with the function/method that contains it.
#[allow(clippy::too_many_arguments)]
pub(super) fn walk_with_calls(
    node: Node<'_>,
    src: &[u8],
    spec: &LangSpec,
    current_caller: &str,
    syms: &mut Vec<Symbol>,
    imports: &mut Vec<String>,
    calls: &mut Vec<Call>,
    depth: usize,
) {
    if depth >= MAX_AST_DEPTH {
        return;
    }
    let kind = node.kind();
    let mut next_caller_owned: Option<String> = None;
    if spec.functions.contains(&kind) {
        if let Some(name) = ident_name(node, src) {
            let label = if kind.contains("macro") || kind.starts_with("preproc") {
                "macro"
            } else if kind.contains("method") {
                "method"
            } else {
                "fn"
            };
            // `symbol_rich` extracts decorators (Python @dataclass /
            // TS @Component / Java @Override) so the agent can ask
            // "show me all routes" by filtering for `@app.route` in
            // decorators without re-reading every file.
            syms.push(symbol_rich(name.clone(), label, node, src, spec));
            next_caller_owned = Some(name);
        }
    } else if spec.types.contains(&kind) {
        // An impl block is NOT a type declaration, and its first
        // identifier is the TRAIT, not the subject: `impl Drop for
        // StashGuard` was indexed as a declaration of `Drop`, so
        // "where is Drop defined?" answered with every file that
        // happens to implement it — a wrong answer, repeated once per
        // std trait per file. Index the type being implemented instead,
        // under its own kind, so it reads as "this file adds to X"
        // rather than "this file declares X".
        let is_impl = kind.contains("impl");
        let name = if is_impl {
            impl_target_name(node, src)
        } else {
            ident_name(node, src)
        };
        if let Some(name) = name {
            // Classes carry base lists — populate them so an agent
            // can answer "what does X inherit?" without a Read.
            let label = if is_impl { "impl" } else { "type" };
            syms.push(symbol_rich(name, label, node, src, spec));
        }
    } else if spec.modules.contains(&kind) {
        if let Some(name) = ident_name(node, src) {
            // `symbol_rich`, not `symbol`: a module carries attributes too, and
            // `#[cfg(test)]` is the one that says a whole block of declarations
            // is not part of what the file offers. Built through `symbol` it
            // had NO decorators at all, so the only test modules anything could
            // recognise were the ones literally named `test` or `tests` —
            // measured on this repository, 296 of 1 491 `#[cfg(test)]` modules
            // (20%) carry another name, and every helper inside them was
            // reported as a declaration of the file.
            syms.push(symbol_rich(name, "module", node, src, spec));
        }
    } else if spec.members.contains(&kind)
        && (current_caller == "<module>" || spec.function_scoped_members.contains(&kind))
    {
        // #3 — top-level data symbols (consts, statics, fields, enum
        // variants). The `current_caller == "<module>"` guard keeps
        // function-local `let`/`var` bindings out of the index: inside a
        // function body the caller is the enclosing fn name, so only
        // module-scope and type-member declarations reach here. One node
        // can declare several names (Go `const ( A = …; B = … )`, Rust
        // tuple fields) — harvest every identifier child.
        let label = member_label(kind);
        for name in member_names(node, src) {
            // `symbol_rich` rather than `symbol`: a member's OWNER is what
            // tells `Timeout` in one struct from `Timeout` in the next, and
            // 42 % of field symbols in this machine's indexes share a bare
            // name with a sibling in their own file.
            syms.push(symbol_rich(name, label, node, src, spec));
        }
    } else if spec.receiver_fields.contains(&kind) {
        // No scope gate: the declaration site for these languages IS a method
        // body (`__init__`, `constructor`). The enclosing class is the gate
        // instead — a `self.x = …` outside any class declares nothing.
        if let Some(name) = receiver_field_name(node, src) {
            let owner = enclosing_type_name(node, src, spec, false);
            if !owner.is_empty() {
                let mut sym = symbol_rich(name, "field", node, src, spec);
                sym.owner = owner;
                syms.push(sym);
            }
        }
    } else if spec.imports.contains(&kind) {
        if let Some(path) = first_named_string_child(node, src) {
            imports.push(path);
        } else if let Some(path) = first_dotted_or_scoped(node, src) {
            imports.push(path);
        } else if let Some(path) = ident_name(node, src) {
            imports.push(path);
        }
    } else if spec.calls.contains(&kind) {
        if let Some(callee) = callee_name(node, src) {
            calls.push(Call {
                caller_name: current_caller.to_string(),
                callee_name: callee,
                file: String::new(),
                line: node.start_position().row + 1,
                qualifier: callee_qualifier(node, src),
            });
        }
    }
    let next_caller: &str = next_caller_owned.as_deref().unwrap_or(current_caller);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_with_calls(
            child,
            src,
            spec,
            next_caller,
            syms,
            imports,
            calls,
            depth + 1,
        );
    }
}

/// Extract the callee identifier from a call-expression node. Each
/// grammar puts the callee in a different place — `function`/`method`/
/// the first child. We look at the documented field name when
/// possible, fall back to the first identifier-shaped child.
/// The path segment written immediately before the callee at the call site.
///
/// `crate::logs::fold(…)` → `logs`; `archetypes::kv_records(…)` →
/// `archetypes`; `x.fold(…)` → `x`; `fold(…)` → empty.
///
/// This is the caller SAYING which home it meant. `impact_of` abandons any
/// callee declared in more than one file, and the names that get abandoned
/// are exactly the generic ones — `fold`, `compact`, `new` — which is also
/// exactly why people qualify them.
///
/// Text-level rather than kind-level on purpose: `scoped_identifier`,
/// `field_expression`, `member_expression`, `selector_expression` and
/// `attribute` are five grammars' names for the same written shape, and the
/// separator is `::` or `.` depending on the language. Splitting the callee
/// expression's own text on both is one rule for all of them.
pub(super) fn callee_qualifier(node: Node<'_>, src: &[u8]) -> String {
    let Some(callee) = ["function", "method", "callee"]
        .iter()
        .find_map(|f| node.child_by_field_name(f))
    else {
        return String::new();
    };
    let text = text(callee, src);
    // Only a simple written path counts. Anything carrying a call, an index
    // or a generic argument is an expression whose value we cannot name.
    if text.contains(['(', ')', '[', ']', '<', '>', ' ', '\n']) {
        return String::new();
    }
    let segments: Vec<&str> = text.split("::").flat_map(|s| s.split('.')).collect();
    if segments.len() < 2 {
        return String::new();
    }
    segments[segments.len() - 2].trim().to_string()
}

pub(super) fn callee_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    // Most grammars expose the callee under a `function` field
    // (`function_call`, `call_expression`, `call`); some use `method`
    // or the first child positionally.
    // `constructor` is the JS/TS field on `new_expression`. Constructing a
    // class IS calling it, and for a class in JS/TS/C++/Scala/PHP it is
    // usually the ONLY call shape — Java and C# have carried
    // `object_creation_expression` under `calls` since they were added, so
    // the rule was already in this file, spelled for two of the seven
    // languages that need it. Measured on workweave-router 2026-08-28: 175
    // `new X()` over 131 TS/JS files, `LspClient` in 19 of them, and not one
    // of those edges reached the call graph.
    for field in ["function", "method", "callee", "constructor", "name"] {
        if let Some(child) = node.child_by_field_name(field) {
            // The field might itself be a `field_expression`
            // (`obj.method`) or `member_expression`; pull the rightmost
            // identifier — that's the called method.
            if let Some(name) = rightmost_ident(child, src) {
                return Some(name);
            }
        }
    }
    // Last-resort positional walk.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(name) = rightmost_ident(child, src) {
            return Some(name);
        }
    }
    None
}

/// Walk inwards through wrapper nodes (`field_expression`,
/// `member_expression`, `scoped_identifier`, …) to find the final
/// identifier — the actually-called method/function name. Returns
/// `None` if we hit something that isn't identifier-shaped.
pub(super) fn rightmost_ident(node: Node<'_>, src: &[u8]) -> Option<String> {
    let kind = node.kind();
    if matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "simple_identifier"
            // PHP spells every identifier `name`. Its absence here is why
            // four declared call kinds produced ZERO edges, as a sweep over
            // every language's call kinds showed.
            | "name"
    ) {
        return Some(text(node, src));
    }
    // For dotted / member access nodes, descend into the last named child.
    if matches!(
        kind,
        "field_expression"
            | "member_expression"
            | "method_invocation"
            | "scoped_identifier"
            | "navigation_expression"
            | "attribute"
            // Go, C#, PHP and Swift each name the same "receiver dot method"
            // shape differently, and each absence cost that language its
            // method-call edges.
            | "selector_expression"
            | "member_access_expression"
            | "member_call_expression"
            | "scoped_call_expression"
            | "qualified_name"
            | "navigation_suffix"
    ) {
        let mut last: Option<Node> = None;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            last = Some(child);
        }
        if let Some(c) = last {
            return rightmost_ident(c, src);
        }
    }
    None
}
