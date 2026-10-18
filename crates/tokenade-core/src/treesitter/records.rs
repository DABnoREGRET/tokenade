//! **What the extractor PRODUCES.**
//!
//! `Symbol` and `Call` are the two records the rest of the repository
//! consumes; `index::Symbol` is in fact the spelling most callers know. They
//! live here, with the extractor that writes them.

/// `end_line == 0` means "unknown" (regex extractor), so it stays out of the
/// serialized form entirely rather than claiming line 0.
pub(super) fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String, // "fn" | "type" | "class" | "const" | ...
    pub file: String,
    pub line: usize,
    /// Decorators / attributes attached to the declaration. Tree-sitter
    /// extractor populates this for Python (`@dataclass`, `@property`,
    /// `@app.route('/x')`), TypeScript (`@Component`, `@Injectable`),
    /// Java (`@Override`, `@SpringBootApplication`). Empty otherwise.
    ///
    /// Serialized as a default-empty `Vec` so older index.json files
    /// (pre-3B) load without migration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decorators: Vec<String>,
    /// Hash of this declaration's AST SHAPE, with its own name excluded.
    ///
    /// Two symbols with the same value are the same code under a different
    /// name — a pure rename, detected by equality instead of by a similarity
    /// score. Empty when the file was indexed by the regex extractor (no
    /// grammar) or by a tokenade older than this field.
    ///
    /// `#[serde(default)]` is the whole migration: an `index.json` written
    /// before this field loads with an empty hash and gets a real one the next
    /// time the scanner visits the file. Same idiom as `decorators` above —
    /// no reindex, no schema version, no second table to keep in sync.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub structural_hash: String,
    /// Like [`Self::structural_hash`], but with every NESTED declaration's
    /// subtree skipped — the shape of this declaration's own body only.
    ///
    /// The pair answers the question a line diff cannot: `structural_hash`
    /// changed but `self_hash` did not means the change is entirely inside a
    /// child, so reporting the parent is noise. This is what makes
    /// "`impl Foo` modified" disappear when the only edit was in one method.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub self_hash: String,
    /// Last line of the declaration (1-based, inclusive). `0` when unknown
    /// (regex extractor). Together with [`Self::line`] it gives the span that
    /// decides which declarations are nested inside which.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub end_line: usize,
    /// Base classes / superclasses / implemented interfaces. Helps
    /// agents answer "what does Foo inherit from?" without re-reading
    /// the file. Python `class Foo(Base)`, TypeScript `class Foo
    /// extends Base implements Iface`, Java `class Foo extends Bar
    /// implements Baz`.
    ///
    /// Same `#[serde(default)]` policy as `decorators` for back-compat.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bases: Vec<String>,
    /// What this symbol HANGS OFF — the Go receiver's type for a method, the
    /// struct or class for a field or an enum variant. Empty when there is
    /// none (a free function, a module-scope const).
    ///
    /// Distinct from [`Self::bases`] on purpose. `bases` is what a
    /// declaration INHERITS — a Python superclass, a TS `implements`, the
    /// trait an `impl` block provides — and an owner is not a supertype. The
    /// receiver lived in `bases` for one iteration and that was one meaning
    /// too many in one field.
    ///
    /// Same `#[serde(default)]` policy as `bases` for back-compat.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
}

/// One call edge: `caller_name` at `file:line` references `callee_name`.
/// The caller_name is the enclosing function/method symbol's name;
/// global-scope calls get the synthetic caller `"<module>"`.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Call {
    pub caller_name: String,
    pub callee_name: String,
    pub file: String,
    pub line: usize,
    /// The path segment written immediately before the callee at the call
    /// site — `logs` in `crate::logs::fold(…)`, `archetypes` in
    /// `archetypes::kv_records(…)`. Empty for an unqualified call.
    ///
    /// It is what resolves a call the bare name cannot. [`Index::impact_of`]
    /// gives up on any callee declared in more than one file, because
    /// attributing `new` or `compact` would trade a recall gap for a
    /// precision one — but a caller that WROTE the module has already said
    /// which home it meant, and 2 036 call sites in this repository do.
    ///
    /// `#[serde(default)]` for back-compat: an index written before this
    /// field simply attributes nothing through it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub qualifier: String,
}
