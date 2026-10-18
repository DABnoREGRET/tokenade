//! **Naming what was found.**
//!
//! A method's owner, the type that encloses it, a class's bases, the
//! decorators, a member's label. Nothing here WALKS the tree: everything
//! answers "what is this node called, as a human sees it".

use super::*;

/// Build a symbol enriched with decorators + bases extracted from the
/// node's tree-sitter children. Used by the walker for every declaration
/// kind — including modules, whose only annotation that matters is
/// `#[cfg(test)]`.
pub(super) fn symbol_rich(
    name: String,
    kind: &'static str,
    node: Node<'_>,
    src: &[u8],
    spec: &LangSpec,
) -> Symbol {
    let line = node.start_position().row + 1;
    Symbol {
        name,
        kind: kind.to_string(),
        file: String::new(),
        line,
        end_line: node.end_position().row + 1,
        decorators: extract_decorators(node, src),
        bases: extract_bases(node, src),
        owner: owner_of(node, src, spec),
        structural_hash: structural_hash_of(node, src, ident_span(node, src), None),
        self_hash: structural_hash_of(node, src, ident_span(node, src), Some(spec)),
    }
}

/// Decorator extraction. Python wraps the def/class in a
/// `decorated_definition` parent that holds `decorator` siblings;
/// TypeScript / Java / Kotlin / C# attach decorators directly. We try
/// both shapes — whichever fires for the grammar at hand.
pub(super) fn extract_decorators(node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // Python pattern.
    if let Some(parent) = node.parent() {
        if parent.kind() == "decorated_definition" {
            let mut cur = parent.walk();
            for child in parent.children(&mut cur) {
                if child.kind() == "decorator" {
                    out.push(text(child, src).trim().to_string());
                }
            }
        }
    }
    // Rust attributes (`#[derive(Serialize)]`, `#[tokio::test]`,
    // `#[get("/")]`) sit as `attribute_item` siblings *preceding* the
    // item, sharing its parent — pull the contiguous run just above it
    // so `tokenade query Serialize` / `query test` lands on the item.
    if let Some(parent) = node.parent() {
        let mut sib = node.prev_sibling();
        let mut rust_attrs: Vec<String> = Vec::new();
        while let Some(s) = sib {
            if s.kind() == "attribute_item" {
                rust_attrs.push(text(s, src).trim().to_string());
                sib = s.prev_sibling();
            } else if s.kind() == "line_comment" || s.kind() == "block_comment" {
                sib = s.prev_sibling();
            } else {
                break;
            }
        }
        // Preserve source order (we walked upwards).
        rust_attrs.reverse();
        out.extend(rust_attrs);
        let _ = parent;
    }
    // TS / Java / Kotlin / C# pattern (+ Rust attribute_item as a child,
    // which some grammar versions nest rather than make a sibling).
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            "decorator" | "annotation" | "marker_annotation" | "attribute_item" => {
                out.push(text(child, src).trim().to_string());
            }
            // Java buries annotations inside `modifiers`.
            "modifiers" => {
                let mut inner = child.walk();
                for ann in child.children(&mut inner) {
                    if matches!(ann.kind(), "annotation" | "marker_annotation") {
                        out.push(text(ann, src).trim().to_string());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// What a declaration HANGS OFF, as its language writes it.
///
/// Two shapes, one question. A Go method carries a `receiver` field:
/// `func (t *Task) Execute()`. Everything else — a field, an enum variant, a
/// function declared inside an `impl` block or a class — carries nothing, and
/// its owner is the nearest enclosing type declaration, which the parent
/// chain already knows.
///
/// The function case is the one that was missing longest. Rust's `impl Foo {
/// fn new() }` is a `function_item` like any other, so `agent_jsonl_usage.rs`
/// declared eight `fn agent` and the index printed eight identical lines.
/// The same shape covers a Python method, a TypeScript class method and a
/// Java one, all in the same walk.
///
/// Java and Kotlin share Go's `method_declaration` node kind and have no
/// `receiver` field, so `child_by_field_name` answers None for them and no
/// language test is needed.
///
/// The receiver is a `parameter_list` whose text is `(t *Task)` — the binding
/// name is noise, so descend to the declaration's `type` child and fall back
/// to the whole list only when the shape is not the expected one.
pub(super) fn owner_of(node: Node<'_>, src: &[u8], spec: &LangSpec) -> String {
    if let Some(recv) = node.child_by_field_name("receiver") {
        let mut cur = recv.walk();
        return recv
            .named_children(&mut cur)
            .find_map(|p| p.child_by_field_name("type"))
            .map(|t| text(t, src).trim().to_string())
            .unwrap_or_else(|| text(recv, src).trim().to_string());
    }
    if !spec.members.contains(&node.kind()) && !spec.functions.contains(&node.kind()) {
        return String::new();
    }
    enclosing_type_name(node, src, spec, true)
}

/// The nearest enclosing type's name, walking up from `node`.
///
/// `stop_at_function` is the whole disagreement between the two callers, and
/// it is a disagreement about what a function body MEANS. For a `const` or a
/// nested `fn`, a function body between here and the type says "this is a
/// local, not part of that type's surface" — so the walk stops and the symbol
/// is ownerless. For a receiver field it says the opposite: a constructor
/// body is exactly where the language puts the declaration, so the walk must
/// pass through it. One walk, one place, and the difference stated rather
/// than a second copy that quietly drops the rule.
pub(super) fn enclosing_type_name(
    node: Node<'_>,
    src: &[u8],
    spec: &LangSpec,
    stop_at_function: bool,
) -> String {
    let mut cur = node.parent();
    while let Some(p) = cur {
        if stop_at_function && spec.functions.contains(&p.kind()) {
            return String::new();
        }
        if spec.types.contains(&p.kind()) {
            // An `impl` block's first identifier is the TRAIT, not the
            // subject — the same trap the extractor names where it decides
            // what to call the block itself.
            let n = if p.kind().contains("impl") {
                impl_target_name(p, src)
            } else {
                ident_name(p, src)
            };
            if let Some(n) = n {
                return n;
            }
        }
        cur = p.parent();
    }
    String::new()
}

/// Base-class / interface extraction. Each grammar uses different
/// child node names; we try the common ones.
pub(super) fn extract_bases(node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // Rust: `impl Display for TokenadeError`. The trait is a FIELD, not a
    // child kind, so the loop below cannot see it — and it is the only thing
    // separating this impl block from the others in the same file, all of
    // which carry the implemented TYPE's name by the rule a few hundred lines
    // up. Measured on this repository: 76 of 262 impl rows shared file and
    // name with a sibling, six of them `impl TokenadeError` in one file.
    //
    // It goes in `bases`, never in `name`: `find`'s tier 2 is "matched only
    // via a decorator/base", so `query Drop` still ranks the real `trait Drop`
    // declaration above every file that implements it — which is exactly the
    // answer the naming rule was written to protect.
    if node.kind() == "impl_item" {
        if let Some(t) = node.child_by_field_name("trait") {
            out.push(text(t, src).trim().to_string());
        }
    }
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        match child.kind() {
            // Python: `class Foo(Base):` -> `argument_list` child.
            "argument_list" => {
                let mut inner = child.walk();
                for arg in child.named_children(&mut inner) {
                    let t = text(arg, src).trim().to_string();
                    if !t.is_empty() {
                        out.push(t);
                    }
                }
            }
            // TS / JS.
            "class_heritage" | "superclass" => {
                let mut inner = child.walk();
                for n in child.named_children(&mut inner) {
                    out.push(text(n, src).trim().to_string());
                }
            }
            // Java / Kotlin / C# / Scala.
            "extends_clause"
            | "super_class"
            | "super_interfaces"
            | "implements_clause"
            | "base_list"
            | "delegation_specifiers" => {
                let mut inner = child.walk();
                for n in child.named_children(&mut inner) {
                    out.push(text(n, src).trim().to_string());
                }
            }
            _ => {}
        }
    }
    out
}

/// Is this node kind an identifier in some grammar we ship?
pub(super) fn is_ident_kind(k: &str) -> bool {
    matches!(
        k,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "name"
            | "simple_identifier"
            | "constant"
            | "variable_name"
    )
}

/// Field name declared by an assignment through the object's own receiver:
/// Python `self.x = …`, Ruby `@x = …`, JS/TS `this.x = …`, PHP
/// `$this->x = …`. Returns `None` for every other assignment, which is the
/// point — an assignment to anything but the receiver is a write, not a
/// declaration.
///
/// Read off the LEFT side only. The value side is never touched, so this
/// cannot leak the identifiers that keep module-level `X = …` out of the
/// index (see `LangSpec::members` on `py`).
///
/// A computed access (`this[k]`, `$this->$k`, `$this->{'lit'}`) needs no
/// filter of its own: every grammar here models it as a different node with
/// no property/attribute/name field, so the `find_map` above already answers
/// None. A character check on the extracted name was written for this and was
/// unreachable — the mutation that removed it stayed green. The guard is the
/// test, which goes red if a grammar ever starts naming those.
pub(super) fn receiver_field_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let lhs = node.child_by_field_name("left")?;
    // Ruby names the receiver in the token itself: `@sample_window`.
    if lhs.kind() == "instance_variable" {
        let raw = lhs.utf8_text(src).ok()?;
        let name = raw.trim_start_matches('@');
        return (!name.is_empty() && name != raw).then(|| name.to_string());
    }
    let object = lhs.child_by_field_name("object")?;
    let receiver = object.utf8_text(src).ok()?.trim();
    if !matches!(receiver, "self" | "this" | "$this") {
        return None;
    }
    // The grammars disagree on the field's name for the same concept:
    // `attribute` (py), `property` (js/ts), `name` (php).
    let prop = ["attribute", "property", "name"]
        .into_iter()
        .find_map(|f| lhs.child_by_field_name(f))?;
    let name = prop.utf8_text(src).ok()?.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Human label for a data-member node kind (#3). Falls back to "var" so
/// an unmapped-but-listed kind still produces a usable symbol.
pub(super) fn member_label(kind: &str) -> &'static str {
    match kind {
        // A `#define` is neither a const nor a var: it is a macro, and the
        // label is the first thing a reader sees.
        k if k.starts_with("preproc") => "macro",
        k if k.contains("const") => "const",
        "static_item" => "static",
        k if k.contains("enum") || k.contains("variant") || k.contains("enumerator") => "variant",
        k if k.contains("field") || k.contains("property") => "field",
        _ => "var",
    }
}

/// Identifier name(s) declared by a data-member node (#3). A single
/// node can bind several names: Go `const ( A=…\n B=… )` specs and
/// `a, b int` field groups. We prefer the grammar's `name` field
/// (handles every single-name case cleanly, no false positives from a
/// type/value identifier) and fall back to the leading identifier-shaped
/// children when the grammar doesn't field-name them.
pub(super) fn member_names(node: Node<'_>, src: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    {
        let mut cursor = node.walk();
        for n in node.children_by_field_name("name", &mut cursor) {
            if is_ident_kind(n.kind()) {
                out.push(text(n, src));
            } else if let Some(s) = ident_name(n, src) {
                out.push(s);
            }
        }
    }
    if !out.is_empty() {
        return out;
    }
    // Fallback: leading identifier children, stopping once we pass the
    // name list into the type / `=` / value region.
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let k = child.kind();
        if is_ident_kind(k) {
            out.push(text(child, src));
        } else if k == "," {
            continue;
        } else if !out.is_empty() {
            break;
        }
    }
    out
}

/// Walk children looking for the first one whose `kind()` looks like
/// an identifier. Different grammars use different names — we accept
/// the common set so a single helper works across all languages.
/// The type an `impl` block is FOR — the grammar's `type` field, not the
/// leading identifier. `impl Drop for StashGuard` and `impl<T> Cache<T>`
/// both answer `StashGuard` / `Cache`: the subject, generics peeled off.
/// Falls back to the leading identifier when a grammar exposes no `type`
/// field, so an unfamiliar impl-shaped node still indexes something.
pub(super) fn impl_target_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let subject = node.child_by_field_name("type")?;
    if matches!(subject.kind(), "type_identifier" | "identifier") {
        return Some(text(subject, src));
    }
    // `Cache<T>`, `&mut Foo`, `Box<dyn X>` — the head identifier is the
    // subject; descend to the first one.
    fn first_type_ident(n: Node<'_>, src: &[u8]) -> Option<String> {
        if matches!(n.kind(), "type_identifier" | "identifier") {
            return Some(text(n, src));
        }
        let mut c = n.walk();
        let kids: Vec<Node<'_>> = n.children(&mut c).collect();
        kids.into_iter().find_map(|ch| first_type_ident(ch, src))
    }
    first_type_ident(subject, src).or_else(|| ident_name(node, src))
}

pub(super) fn ident_name(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let k = child.kind();
        // Common identifier kinds across the 17 grammars we ship.
        if matches!(
            k,
            "identifier"
                | "type_identifier"
                | "field_identifier"
                | "property_identifier"
                | "name"
                | "simple_identifier"
                | "scoped_identifier"
                | "constant"
                | "variable_name"
        ) {
            return Some(text(child, src));
        }
        // Some grammars wrap the identifier inside a *_declarator (C
        // family) — recurse one level.
        if k.ends_with("_declarator") {
            if let Some(inner) = ident_name(child, src) {
                return Some(inner);
            }
        }
    }
    None
}

/// For import-shaped nodes: the path lives in a string literal child.
/// Walk children looking for the first dotted-path-shaped node — the
/// node families used by Python / Rust / Java / Kotlin to express
/// the import target (`os.path`, `crate::a::b`, etc.). Returns the
/// node's raw text, which the indexer treats opaquely.
pub(super) fn first_dotted_or_scoped(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "dotted_name"
                | "scoped_identifier"
                | "scoped_use_list"
                | "use_clause"
                | "use_list"
                | "qualified_name"
                | "namespace_name"
                | "package_identifier"
                | "module_path"
        ) {
            return Some(text(child, src));
        }
    }
    None
}

pub(super) fn first_named_string_child(node: Node<'_>, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "string" | "string_literal" | "raw_string_literal" | "interpreted_string_literal"
        ) {
            // Strip the surrounding quotes.
            let raw = text(child, src);
            let trimmed = raw.trim_matches('"').trim_matches('\'');
            return Some(trimmed.to_string());
        }
    }
    None
}

pub(super) fn text(node: Node<'_>, src: &[u8]) -> String {
    src.get(node.byte_range())
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default()
}
