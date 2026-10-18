//! **The STRUCTURAL fingerprint of a fragment.**
//!
//! Two fragments that differ only by their names have the same fingerprint:
//! that is what makes it possible to recognise a renamed copy-paste. FNV
//! rather than a cryptographic hash — we compare, we protect nothing.

use super::*;

/// Byte span of a declaration's own NAME, so it can be excluded from the
/// structural hash. `None` when the node has no identifier child.
pub(super) fn ident_span(node: Node<'_>, src: &[u8]) -> Option<(usize, usize)> {
    let mut cur = node.walk();
    for child in node.children(&mut cur) {
        if child.kind().contains("identifier") || child.kind() == "name" {
            let _ = src;
            return Some((child.start_byte(), child.end_byte()));
        }
    }
    None
}

/// Is this node kind one the index turns into its own `Symbol`?
pub(super) fn is_declaration(spec: &LangSpec, kind: &str) -> bool {
    spec.functions.contains(&kind)
        || spec.types.contains(&kind)
        || spec.modules.contains(&kind)
        || spec.members.contains(&kind)
}

/// FNV-1a 64. Stable across processes and Rust versions — the property that
/// matters, since this value is written to `index.json` and compared against a
/// value computed by a different build months later. `DefaultHasher` (SipHash
/// with a per-process key) would be a silent correctness bug here.
pub(super) struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// Hash of a declaration's AST shape.
///
/// What it deliberately ignores: comments, whitespace, and — when `exclude` is
/// given — the declaration's own name. What it deliberately keeps: the `kind`
/// of every internal node, so `x = foo(bar)` and `foo(bar) = x` do not collide
/// despite holding identical tokens. That is the difference between a hash of
/// the CODE and a hash of the characters.
///
/// Renaming a function changes its name and nothing else, so excluding the name
/// span makes a pure rename detectable by equality — no similarity threshold,
/// no tuning, no false positives from two unrelated functions that happen to
/// share 80 % of their tokens.
pub(super) fn structural_hash_of(
    node: Node<'_>,
    src: &[u8],
    exclude: Option<(usize, usize)>,
    skip_nested: Option<&LangSpec>,
) -> String {
    let mut h = Fnv::new();
    let mut cur = node.walk();
    let mut descend = true;
    loop {
        let n = cur.node();
        // INVARIANT: every path out of this block reassigns `descend` before
        // the loop reads it again — `goto_next_sibling` sets it true, the
        // parent walk sets it false. So the branches below only decide whether
        // to `continue` into a child; writing `descend = false` in them was a
        // dead store the compiler flagged four times, and it read like a
        // decision that mattered.
        if descend {
            let kind = n.kind();
            // Comments carry no structure. Skipping the subtree (rather than
            // just the node) also drops doc-comment bodies, which is what makes
            // "same code, reworded docs" hash equal.
            if kind.contains("comment") {
            } else if skip_nested.is_some_and(|sp| n.id() != node.id() && is_declaration(sp, kind))
            {
                // A nested declaration is somebody else's row in the index and
                // carries its own hash. Skipping it whole is what lets
                // `entity_match::suppress_redundant_parents` tell "this impl
                // block changed" from "a method inside it changed".
            } else if n.child_count() == 0 {
                // Leaf: hash its text, minus any excluded span.
                let (s, e) = (n.start_byte(), n.end_byte());
                match exclude {
                    Some((xs, xe)) if s >= xs && e <= xe => {}
                    _ => h.write(src.get(s..e).unwrap_or(&[])),
                }
                h.write(b"\x00");
            } else {
                h.write(kind.as_bytes());
                h.write(b"\x01");
                if cur.goto_first_child() {
                    continue;
                }
            }
        }
        if cur.goto_next_sibling() {
            descend = true;
            continue;
        }
        if !cur.goto_parent() || cur.node().id() == node.id() {
            break;
        }
        descend = false;
    }
    format!("{:016x}", h.0)
}
