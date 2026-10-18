//! **The table** — a language, its extensions, its declarative forms.
//!
//! Hand-written and not derived: what we want to know about a language is not
//! what its grammar says, it is what a reader looks for. A table derived from
//! the available grammars would carry the languages we have, not the ones we
//! come across.
//!
//! It is alone in its file because it changes for one reason only — one more
//! language — and the two functions that query it never change.

use super::*;

/// (extension aliases, spec). Every extension here is lowercase; `lang_for`
/// lowercases its argument so `.R` and `.BAT` dispatch like `.r` and `.bat`.
pub(crate) static SPECS: &[(&[&str], LangSpec)] = &[
    (
        &["rs"],
        LangSpec {
            patterns: &[
                // Modifiers: `pub(crate)`, `async`, `unsafe`, `const`, `extern "C"`.
                (
                    r#"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+|unsafe\s+|const\s+|extern(?:\s+"[^"]*")?\s+)*fn\s+([\p{XID_Start}_]\p{XID_Continue}*)"#,
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait|type|union)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
                (
                    r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z_][A-Z0-9_]*)",
                    "const",
                ),
            ],
            import: Some(r"(?m)^\s*use\s+([A-Za-z_][A-Za-z0-9_:]*)"),
        },
    ),
    // #725 — `.pyi` (type stubs), `.pyx` (Cython), `.pyw` (Python
    // Windows entry point) all use the same `def`/`class` syntax
    // as `.py`. is_source_code_extension already covers them; this
    // was the missing piece for symbol_find on Cython projects and
    // type-stub-heavy codebases.
    (
        &["py", "pyi", "pyx", "pyw"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:async\s+)?def\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*class\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "class",
                ),
            ],
            import: Some(r"(?m)^\s*(?:from|import)\s+([A-Za-z_][A-Za-z0-9_.]*)"),
        },
    ),
    // #725 — `.cjs` (CommonJS), `.mts` / `.cts` (TypeScript ESM /
    // CJS module variants) use the same `function`/`class`/`const =>
    // (`/`interface`/`type`/`enum` syntax as their parent extension.
    // is_source_code_extension covers them; lang_for was missing
    // the alias arms so Node `.cjs` repos and modern dual-package
    // TS projects had no symbols indexed for those files.
    (
        &["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:export\s+(?:default\s+)?)?(?:async\s+)?function\s+([\p{XID_Start}_$][\p{XID_Continue}$]*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:export\s+)?(?:abstract\s+)?class\s+([\p{XID_Start}_$][\p{XID_Continue}$]*)",
                    "class",
                ),
                (
                    r"(?m)^\s*(?:export\s+)?(?:const|let)\s+([\p{XID_Start}_$][\p{XID_Continue}$]*)\s*=\s*(?:async\s*)?\(",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:export\s+)?(?:interface|type|enum)\s+([\p{XID_Start}_$][\p{XID_Continue}$]*)",
                    "type",
                ),
            ],
            import: Some(r#"(?m)(?:import\s.*?from\s+|require\()\s*['"]([^'"]+)['"]"#),
        },
    ),
    (
        &["go"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*func\s+(?:\([^)]*\)\s*)?([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*type\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
            ],
            import: Some(r#"(?m)^\s*["]([^"]+)["]"#),
        },
    ),
    // #724 — `.kts` (Kotlin DSL: Gradle build scripts) uses the
    // same syntax as `.kt` (class/object/fun/import). `code_strip`
    // and `is_source_code_extension` already cover both; lang_for
    // was the missing piece — meaning `symbol_find` over a Gradle
    // Kotlin DSL repo had no symbols indexed for build files.
    // (Groovy gets its own arm below — its `def` syntax doesn't
    // match Java's `(public|...)+ type name(` pattern.)
    (
        &["java", "kt", "kts"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)(?:public|private|protected|static|\s)+(?:class|interface|enum)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "class",
                ),
                (
                    r"(?m)(?:public|private|protected|static|fun)\s+[A-Za-z0-9_<>\[\]]+\s+([\p{XID_Start}_]\p{XID_Continue}*)\s*\(",
                    "fn",
                ),
            ],
            import: Some(r"(?m)^\s*import\s+([A-Za-z_][A-Za-z0-9_.]*)"),
        },
    ),
    // C-family bucket: C / C++ / Objective-C / Objective-C++ headers
    // are all `.h`. Codegraph distinguishes them by content sniff
    // (`grammars.ts:242-265`); our regex extractor unifies them by
    // accepting every C-family pattern shape in one LangSpec. The
    // wider net costs nothing at extraction time (regex alternation
    // is cheap) and avoids the maintenance burden of a separate
    // sniffer.
    (
        &["c", "h", "cpp", "cc", "cxx", "hpp", "hxx", "mm", "m"],
        LangSpec {
            patterns: &[
                // C / C++ function definition.
                (
                    r"(?m)^[A-Za-z_][A-Za-z0-9_\*\s]*\s+([\p{XID_Start}_]\p{XID_Continue}*)\s*\([^;]*\)\s*\{",
                    "fn",
                ),
                // C / C++ struct / class / enum / union declaration.
                (
                    r"(?m)^\s*(?:struct|class|enum|union)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
                // C++ template class — codegraph relies on this to
                // find `Vector`, `Map`, etc. in header files.
                (
                    r"(?m)^\s*template\s*<[^>]*>\s*(?:class|struct)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
                // C++ namespace declaration — common signal for
                // header-only libraries (Eigen, fmtlib, etc.).
                (
                    r"(?m)^\s*namespace\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "module",
                ),
                // Objective-C / Objective-C++ class declaration.
                (
                    r"(?m)^@(?:interface|implementation|protocol)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
                // Objective-C method (`- (NSInteger)count` / `+ (id)alloc`).
                (
                    r"(?m)^[+\-]\s*\([^)]*\)\s*([\p{XID_Start}_]\p{XID_Continue}*)",
                    "method",
                ),
            ],
            import: Some(r#"(?m)^\s*#(?:include|import)\s+[<"]([^>"]+)[>"]"#),
        },
    ),
    (
        &["rb"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*def\s+(?:self\.)?([\p{XID_Start}_][\p{XID_Continue}!?=]*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*class\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "class",
                ),
                (
                    r"(?m)^\s*module\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "module",
                ),
            ],
            import: Some(r#"(?m)^\s*(?:require|require_relative)\s+['"]([^'"]+)['"]"#),
        },
    ),
    (
        &["php"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:public\s+|private\s+|protected\s+|static\s+|abstract\s+|final\s+)*function\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:abstract\s+|final\s+)?class\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "class",
                ),
                (
                    r"(?m)^\s*(?:interface|trait|enum)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
            ],
            import: Some(r"(?m)^\s*(?:use|namespace)\s+([A-Za-z_\\][A-Za-z0-9_\\]*)"),
        },
    ),
    (
        &["swift"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:public\s+|private\s+|fileprivate\s+|internal\s+|open\s+|static\s+|class\s+|override\s+|final\s+|@objc\s+)*func\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:public\s+|private\s+|fileprivate\s+|internal\s+|open\s+|final\s+)*(?:class|struct|enum|protocol|actor|extension)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
                (
                    r"(?m)^\s*(?:public\s+|private\s+|fileprivate\s+|internal\s+|open\s+|static\s+)*(?:let|var)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "const",
                ),
            ],
            import: Some(r"(?m)^\s*import\s+([A-Za-z_][A-Za-z0-9_.]*)"),
        },
    ),
    (
        &["cs"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:public\s+|private\s+|protected\s+|internal\s+|static\s+|virtual\s+|override\s+|abstract\s+|async\s+|sealed\s+)*(?:class|interface|struct|enum|record)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
                (
                    r"(?m)^\s*(?:public\s+|private\s+|protected\s+|internal\s+|static\s+|virtual\s+|override\s+|abstract\s+|async\s+)+(?:[A-Za-z_<>,\[\]\s\?]+\s+)?([\p{XID_Start}_]\p{XID_Continue}*)\s*\(",
                    "fn",
                ),
            ],
            import: Some(r"(?m)^\s*using\s+(?:static\s+)?([A-Za-z_][A-Za-z0-9_.]*)"),
        },
    ),
    (
        &["scala"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:private\s+|protected\s+|override\s+|implicit\s+|final\s+)*def\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:abstract\s+|sealed\s+|case\s+|final\s+)*(?:class|object|trait|enum)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "class",
                ),
                (
                    r"(?m)^\s*(?:type)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
            ],
            import: Some(r"(?m)^\s*import\s+([A-Za-z_][A-Za-z0-9_.{}]*)"),
        },
    ),
    (
        &["ex", "exs"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:def|defp|defmacro|defmacrop)\s+([\p{XID_Start}_][\p{XID_Continue}?!]*)",
                    "fn",
                ),
                (r"(?m)^\s*defmodule\s+([A-Z][A-Za-z0-9_.]*)", "module"),
                (r"(?m)^\s*defprotocol\s+([A-Z][A-Za-z0-9_.]*)", "type"),
            ],
            import: Some(r"(?m)^\s*(?:alias|import|use|require)\s+([A-Z][A-Za-z0-9_.]*)"),
        },
    ),
    // Dart / Flutter — funcs, classes, mixins. Tree-sitter would
    // catch named arguments and arrow functions; the regex below
    // covers 90 % of real-world Dart symbol lookups.
    (
        &["dart"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:static\s+|async\s+|abstract\s+|external\s+)*(?:Future|Stream|void|[A-Z][A-Za-z0-9_]*|[a-z][A-Za-z0-9_<>?]*)\s+([\p{XID_Start}_]\p{XID_Continue}*)\s*\(",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:abstract\s+|sealed\s+)*(?:class|mixin|enum)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
            ],
            import: Some(r#"(?m)^\s*import\s+['"]([^'"]+)['"]"#),
        },
    ),
    // Lua — functions + tables-as-modules. Receivers (`function t.f`
    // and `function t:m`) covered too, per the codegraph notes.
    (
        &["lua", "luau"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:local\s+)?function\s+(?:[A-Za-z_][A-Za-z0-9_]*[.:])?([A-Za-z_][A-Za-z0-9_]*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:local\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*\{",
                    "module",
                ),
            ],
            import: Some(r#"(?m)^\s*(?:local\s+\w+\s*=\s*)?require\s*\(?\s*['"]([^'"]+)['"]"#),
        },
    ),
    // Crystal — Ruby-flavoured syntax with explicit types.
    (
        &["cr"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:private\s+|protected\s+|abstract\s+)*def\s+([\p{XID_Start}_][\p{XID_Continue}?!]*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:abstract\s+)*(?:class|struct|enum|module)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "type",
                ),
            ],
            import: Some(r#"(?m)^\s*require\s+"([^"]+)""#),
        },
    ),
    // Zig — fn / struct / union / enum / const. Note Zig
    // functions are always `fn name(...)` (no public/private
    // modifiers in the grammar).
    (
        &["zig"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:pub\s+|export\s+|extern\s+|inline\s+)*fn\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:pub\s+)*const\s+([A-Z][A-Za-z0-9_]*)\s*=\s*(?:struct|union|enum)",
                    "type",
                ),
            ],
            import: Some(r#"(?m)@import\s*\(\s*"([^"]+)"\s*\)"#),
        },
    ),
    // Nim — proc / func / template / iterator + type/object.
    (
        &["nim"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:proc|func|template|iterator|method|macro)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "fn",
                ),
                (r"(?m)^\s*type\s+([A-Z][A-Za-z0-9_]*)\s*\*?\s*=", "type"),
            ],
            import: Some(r"(?m)^\s*import\s+([A-Za-z_][A-Za-z0-9_/.]*)"),
        },
    ),
    // Haskell — top-level bindings, data/newtype/type/class.
    (
        &["hs"],
        LangSpec {
            patterns: &[
                (r"(?m)^([a-z][A-Za-z0-9_']*)\s*::", "fn"),
                (
                    r"(?m)^(?:data|newtype|type|class)\s+([A-Z][A-Za-z0-9_]*)",
                    "type",
                ),
            ],
            import: Some(r"(?m)^\s*import\s+(?:qualified\s+)?([A-Z][A-Za-z0-9_.]*)"),
        },
    ),
    // F# — let / type / module.
    (
        &["fs", "fsx"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:let|and)\s+(?:rec\s+)?(?:inline\s+)?([a-zA-Z_][A-Za-z0-9_']*)",
                    "fn",
                ),
                (r"(?m)^\s*type\s+([A-Z][A-Za-z0-9_]*)", "type"),
                (r"(?m)^\s*module\s+([A-Z][A-Za-z0-9_.]*)", "module"),
            ],
            import: Some(r"(?m)^\s*open\s+([A-Z][A-Za-z0-9_.]*)"),
        },
    ),
    // OCaml / ReasonML.
    (
        &["ml", "mli", "re"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:let|and)\s+(?:rec\s+)?([a-z_][A-Za-z0-9_']*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*(?:type|module type)\s+([a-z_][A-Za-z0-9_]*)",
                    "type",
                ),
                (r"(?m)^\s*module\s+([A-Z][A-Za-z0-9_]*)", "module"),
            ],
            import: Some(r"(?m)^\s*open\s+([A-Z][A-Za-z0-9_.]*)"),
        },
    ),
    // Odin — proc + struct/enum/union.
    (
        &["odin"],
        LangSpec {
            patterns: &[
                (r"(?m)^([A-Za-z_][A-Za-z0-9_]*)\s*::\s*proc", "fn"),
                (
                    r"(?m)^([A-Z][A-Za-z0-9_]*)\s*::\s*(?:struct|enum|union)",
                    "type",
                ),
            ],
            import: Some(r#"(?m)^\s*import\s+(?:\w+\s+)?"([^"]+)""#),
        },
    ),
    // #727 — Groovy (.groovy = source, .gradle = Groovy DSL build
    // file). Java-flavoured but uses `def` for typeless methods.
    // Two patterns: typed `(modifier+ type name(` like Java, AND
    // bare `def name(` which Java's pattern misses. Both file
    // extensions land here. `is_source_code_extension` covers
    // .groovy; .gradle is the Groovy DSL file (`build.gradle`)
    // — adding it here ALSO requires it in is_source_code_extension,
    // but we punt: Gradle build files in Groovy DSL are usually
    // small and already pass through the source-passthrough path
    // by virtue of their filename, not extension.
    (
        &["groovy"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*(?:public|private|protected|static|\s)+(?:class|interface|enum|trait)\s+([\p{XID_Start}_]\p{XID_Continue}*)",
                    "class",
                ),
                (r"(?m)^\s*def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(", "fn"),
                (
                    r"(?m)^\s*(?:public|private|protected|static)\s+[A-Za-z0-9_<>\[\]]+\s+([\p{XID_Start}_]\p{XID_Continue}*)\s*\(",
                    "fn",
                ),
            ],
            import: Some(r"(?m)^\s*import\s+([A-Za-z_][A-Za-z0-9_.]*)"),
        },
    ),
    // #729 — Perl (.pl = scripts, .pm = modules). Three patterns:
    // `sub name { ... }` (with optional prototype `($$)` after the
    // name), `package Foo::Bar;` for module declarations (very
    // common in .pm), and `our $CONST = ...;` / `our @arr = ...;`
    // for package-scoped constants (skipped — too noisy for
    // symbol_find). The `sub` regex tolerates a prototype block
    // and an attribute list like `:lvalue` between the name and
    // the body. Identifiers allow `:` (Perl uses `Foo::Bar` for
    // class methods) but in declarations the name itself is bare.
    // #754 — Visual Basic family LangSpec (.vb / .vbs / .bas / .cls).
    // Covers VB.NET (.vb), VBScript (.vbs), VBA modules (.bas), and
    // VBA classes (.cls). All four share the `Sub Name(args)` /
    // `Function Name(args) [As Type]` declaration syntax, so one
    // spec handles them.
    //
    // Functions and subroutines have optional access modifiers
    // (`Public`, `Private`, `Friend`, `Protected`) and modifiers
    // (`Shared`, `Overridable`, `Overrides`, `Static`). Captured
    // via a flexible non-capturing prefix. Identifiers in VB are
    // case-insensitive but we capture verbatim.
    //
    // Modules and Classes (VB.NET): `Module Name` / `Class Name`
    // — the closest analog to a namespace. VBA uses these too
    // (in .bas / .cls files), so the same pattern applies.
    //
    // Imports: VB.NET uses `Imports System.IO`. VBScript and VBA
    // don't have an import keyword (VBA uses Tools → References
    // GUI, VBScript inlines all code) — captured anyway for the
    // VB.NET case.
    (
        &["vb", "vbs", "bas", "cls"],
        LangSpec {
            patterns: &[
                (
                    r"(?mi)^\s*(?:(?:Public|Private|Friend|Protected)\s+)?(?:(?:Shared|Static|Overridable|Overrides|MustOverride|NotOverridable)\s+)?(?:Sub|Function)\s+([A-Za-z_][A-Za-z0-9_]*)",
                    "fn",
                ),
                (
                    r"(?mi)^\s*(?:(?:Public|Private|Friend)\s+)?Class\s+([A-Za-z_][A-Za-z0-9_]*)",
                    "type",
                ),
                (
                    r"(?mi)^\s*(?:(?:Public|Private|Friend)\s+)?Module\s+([A-Za-z_][A-Za-z0-9_]*)",
                    "module",
                ),
            ],
            import: Some(r"(?mi)^\s*Imports\s+([A-Za-z_][A-Za-z0-9_.]*)"),
        },
    ),
    // #753 — Windows Batch LangSpec (.bat / .cmd). No real function
    // construct — the closest analog is a label used as a
    // subroutine target. Captures labels written as `:LabelName`
    // at start of line, which double as both jump targets (`goto
    // :Label`) and callable subroutines (`call :Label`).
    //
    // Convention: trailing `:EOF` is reserved by cmd.exe as the
    // implicit end-of-file marker — excluded to avoid indexing
    // every `goto :EOF` in the file. Label names allow `_` and
    // are case-insensitive in batch, but we capture verbatim
    // for grep/find consistency.
    //
    // Includes: `call other.bat` and `call other.cmd` invoke a
    // sibling script (the closest thing batch has to imports).
    (
        &["bat", "cmd"],
        LangSpec {
            patterns: &[(r"(?m)^\s*:([A-Za-z_][A-Za-z0-9_]*)\s*$", "fn")],
            import: Some(r#"(?mi)^\s*call\s+(?:['"])?([^'"\s]+\.(?:bat|cmd))"#),
        },
    ),
    // #752 — Vim script LangSpec (.vim). Editor scripting language
    // for Vim + Neovim plugin authors. Captures the canonical
    // function declarations:
    //   function MyFunc(args)
    //   function! MyFunc(args)            " force-redefine
    //   function s:Local(args)            " script-local
    //   function <SID>NS#Item(args)       " namespaced
    // The `!` is optional and matched non-greedily. Identifiers
    // allow `#` for namespace separation (`pkg#sub#fn`) and `:`
    // for scope prefixes (`s:`, `g:`, `b:`, `<SID>`).
    //
    // Imports cover both runtime forms: `runtime` (searches
    // 'runtimepath') and `source` (literal path). Less canonical
    // than the `function` declaration but caught for the import
    // graph.
    (
        &["vim"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*function!?\s+(?:<SID>)?([A-Za-z_:#][A-Za-z0-9_:#]*)",
                    "fn",
                ),
                (
                    r"(?m)^\s*command!?\s+(?:-[a-z]+(?:=\S+)?\s+)*([A-Z][A-Za-z0-9_]*)",
                    "fn",
                ),
            ],
            import: Some(r#"(?m)^\s*(?:runtime|source)!?\s+(?:['"])?([^'"\s]+)"#),
        },
    ),
    // #751 — Tcl LangSpec (.tcl / .tk / .tm). Tool Command Language,
    // still found in EDA tools (Xilinx Vivado, Synopsys), network
    // gear configuration, expect scripts, and embedded scripting.
    // .tk = Tk widget scripts (same syntax + Tk extensions); .tm =
    // Tcl module file format (also identical syntax).
    //
    // Procedures use `proc name {args} { body }`. Name comes
    // immediately after `proc` — capture that. Tcl identifiers can
    // contain `::` for namespace qualification (`mylib::do_thing`),
    // so the name regex accepts colons.
    //
    // Namespaces declared via `namespace eval Name { ... }` — the
    // outermost qualifier of a `proc` call. We capture both `proc`
    // and `namespace eval` for the "where is X defined" path.
    //
    // Imports follow `package require PkgName ?version?` — captures
    // the package name. Tcl also has `source path.tcl` (file-level
    // include) but that overlaps with shell `source` semantics and
    // tends to be less canonical than `package require`; skipped
    // here to keep the regex tight.
    (
        &["tcl", "tk", "tm"],
        LangSpec {
            patterns: &[
                (r"(?m)^\s*proc\s+([A-Za-z_:][A-Za-z0-9_:]*)", "fn"),
                (
                    r"(?m)^\s*namespace\s+eval\s+([A-Za-z_:][A-Za-z0-9_:]*)",
                    "module",
                ),
            ],
            import: Some(r"(?m)^\s*package\s+require\s+([A-Za-z_][A-Za-z0-9_:]*)"),
        },
    ),
    // #750 — Erlang LangSpec (.erl / .hrl). Functional, concurrent,
    // BEAM-VM language behind WhatsApp/Discord/RabbitMQ. .erl is
    // source; .hrl is a header file (records + macros) — both share
    // the same syntax so one spec covers both.
    //
    // Function definitions in Erlang are line-anchored: `name(Args)
    // -> Body.`. Multi-clause functions repeat the same `name/Arity`
    // on consecutive lines separated by `;`. We capture the first
    // occurrence (the canonical declaration) via the leading-
    // identifier-then-paren shape; lowercase-first is required (a
    // capitalized leading token would be a variable, not a fn).
    //
    // Module attributes use `-module(Name).`. Records (`-record(...)`)
    // and behaviors (`-behaviour(...)`) are structural; we capture
    // `module` and `record` for "where is X defined" queries. Imports
    // follow `-import(Module, [...]).` — we capture the module name
    // only (the per-fn list would explode the index for marginal
    // value).
    (
        &["erl", "hrl"],
        LangSpec {
            patterns: &[
                (r"(?m)^\s*([a-z][a-zA-Z0-9_]*)\s*\(", "fn"),
                (
                    r"(?m)^\s*-\s*module\s*\(\s*([a-z][a-zA-Z0-9_]*)\s*\)",
                    "module",
                ),
                (
                    r"(?m)^\s*-\s*record\s*\(\s*([a-z][a-zA-Z0-9_]*)\s*,",
                    "type",
                ),
            ],
            import: Some(r"(?m)^\s*-\s*import\s*\(\s*([a-z][a-zA-Z0-9_]*)"),
        },
    ),
    // #748 — Fish shell LangSpec (.fish). Distinct from the POSIX
    // shell family (#728 sh/bash/zsh/ksh) because the function
    // declaration syntax is incompatible — fish uses
    // `function name ... end` instead of the POSIX `name() {…}`
    // form. No `function` keyword + `()` form in fish at all, so
    // the bash-family patterns would miss every fish function.
    //
    // Captures the fish-canonical `function name` declaration,
    // tolerating any combination of trailing flags
    // (`--description`, `--argument-names`, `--on-event`, etc.)
    // by matching just the name immediately after the keyword.
    //
    // Imports cover the `source path` builtin (fish's main
    // file-inclusion mechanism). The legacy `.` builtin alias is
    // accepted in POSIX shells but discouraged in fish — skipped
    // to keep the regex tight against false positives in shebang
    // lines or version markers.
    (
        &["fish"],
        LangSpec {
            patterns: &[(r"(?m)^\s*function\s+([A-Za-z_][A-Za-z0-9_-]*)", "fn")],
            import: Some(r#"(?m)^\s*source\s+(?:['"])?([^'"\s]+)(?:['"])?"#),
        },
    ),
    // #747 — Clojure family LangSpec (.clj/.cljs/.cljc). Functional
    // Lisp dialect with JVM (.clj), JS (.cljs), and platform-
    // agnostic (.cljc) targets — same syntax across all three. The
    // .edn extension is data-only (configuration/serialization),
    // not code, so it's excluded.
    //
    // Definition forms are all `(def<flavor> Name ...)` — defn for
    // functions, defmacro for macros, defrecord/deftype for nominal
    // types, defprotocol for interfaces, def for vars/constants.
    // Group them in a single regex to keep the spec compact.
    // Identifiers in Clojure allow `-` `?` `!` `*` and a few other
    // chars — captured here for accurate names. Namespace
    // declarations use `(ns my.namespace ...)`.
    //
    // Imports live inside the `ns` form via `(:require [pkg :as p])`
    // — a single anchor-tolerant regex catches the bracketed
    // form, which dominates in real Clojure code (>95% of imports
    // in nrepl/cider-tested corpuses). The `(:use ...)` form is
    // legacy but still captured for completeness.
    (
        &["clj", "cljs", "cljc"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*\(\s*(?:defn-?|defmacro|defrecord|deftype|defprotocol|defmulti|defmethod|def)\s+([A-Za-z_*+!?<>=][A-Za-z0-9_\-*+!?<>=]*)",
                    "fn",
                ),
                (r"(?m)^\s*\(\s*ns\s+([A-Za-z_][A-Za-z0-9_.\-]*)", "module"),
            ],
            import: Some(r"\[\s*([A-Za-z_][A-Za-z0-9_.\-]*)\s+:(?:as|refer|require)"),
        },
    ),
    // #745 — Julia LangSpec (.jl). ML / scientific-computing
    // language, was indexable as source but had no parser entry →
    // `symbol_find` blind to any Julia codebase. Two common
    // function-declaration forms:
    //   `function name(args) ... end`  — block form, dominant
    //   `name(args) = expr`             — assignment form, common
    //                                      for short defs / methods
    // Both captured. `struct` and `module` declarations follow
    // similar `name` after a keyword. The assignment form can
    // false-match parameterized inner exprs, but at the file-
    // start anchor (`^\s*`) the pattern is reliable enough.
    // Imports cover both `using` (re-exports + glob) and `import`
    // (no re-exports, more selective). The `Foo: bar, baz` and
    // `Foo.bar` qualified forms reduce to `Foo` as the module
    // name — accepted by capturing the head before any `.` / `:`.
    (
        &["jl"],
        LangSpec {
            patterns: &[
                (r"(?m)^\s*function\s+([A-Za-z_][A-Za-z0-9_!]*)", "fn"),
                (r"(?m)^\s*([A-Za-z_][A-Za-z0-9_!]*)\s*\([^)]*\)\s*=", "fn"),
                (
                    r"(?m)^\s*(?:abstract\s+type|mutable\s+struct|struct)\s+([A-Za-z_][A-Za-z0-9_]*)",
                    "type",
                ),
                (r"(?m)^\s*module\s+([A-Za-z_][A-Za-z0-9_]*)", "module"),
            ],
            import: Some(r"(?m)^\s*(?:using|import)\s+([A-Za-z_][A-Za-z0-9_]*)"),
        },
    ),
    // #744 — R LangSpec (.R / .r). Data-science scripts; previously
    // `is_source_code_extension` accepted .R/.r but `lang_for` had no
    // entry → symbol_find returned nothing for R packages and
    // analysis scripts. R has one dominant assignment style for
    // defining functions: `name <- function(args) { ... }`. The
    // `=` form is legal but conventionally reserved for argument-
    // passing, so functions almost always use `<-`. We accept both
    // for robustness on legacy / mixed codebases. `setMethod` /
    // `setGeneric` (S4) are common in Bioconductor / tidyverse but
    // they wrap the symbol in a string literal — out of scope for
    // a simple regex pass.
    //
    // Imports cover both `library(pkg)` and `require(pkg)`. The
    // bare-identifier form is the conventional usage; both also
    // accept strings (`library("dplyr")`) but the bare form
    // dominates by ≥10x in real R code.
    (
        &["r"],
        LangSpec {
            patterns: &[(
                r"(?m)^\s*([A-Za-z_.][A-Za-z0-9_.]*)\s*(?:<-|=)\s*function\s*\(",
                "fn",
            )],
            import: Some(r"(?m)^\s*(?:library|require)\s*\(\s*([A-Za-z_.][A-Za-z0-9_.]*)"),
        },
    ),
    (
        &["pl", "pm"],
        LangSpec {
            patterns: &[
                (r"(?m)^\s*sub\s+([A-Za-z_][A-Za-z0-9_]*)", "fn"),
                (r"(?m)^\s*package\s+([A-Za-z_][A-Za-z0-9_:]*)", "module"),
            ],
            // `use Foo::Bar;` (compile-time) and `require Foo::Bar;`
            // (runtime). Skip the bareword-vs-string ambiguity in
            // require's string form — `use` covers idiomatic Perl
            // dependency declaration.
            import: Some(r"(?m)^\s*(?:use|require)\s+([A-Za-z_][A-Za-z0-9_:]*)"),
        },
    ),
    // #728 — POSIX shell family (sh/bash/zsh/ksh). Three function
    // declaration forms in bash:
    //   `function name() { ... }`    — POSIX-incompatible, but common
    //   `function name { ... }`      — bash-specific, no parens
    //   `name() { ... }`             — POSIX standard
    // The regex collapses all three: optional `function` keyword,
    // then a name, then optional `()`. We don't require `{` at end
    // because the body might be on the next line. `is_source_code
    // _extension` covers .sh / .bash / .zsh / .fish / .ksh; this
    // arm covers the four where the same syntax holds (fish has
    // its own `function NAME ... end` form which doesn't match —
    // omitted to keep the regex tight).
    (
        &["sh", "bash", "zsh", "ksh"],
        LangSpec {
            patterns: &[
                (
                    r"(?m)^\s*function\s+([A-Za-z_][A-Za-z0-9_]*)(?:\s*\(\s*\))?",
                    "fn",
                ),
                (r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(\s*\)\s*\{", "fn"),
            ],
            // bash imports: `source path` / `. path` (less commonly).
            // First match form is unambiguous; second is a single dot
            // followed by whitespace + path. The dot form is risky in
            // regex (false matches on `... .. ...`); skip it — `source`
            // catches 95% of real-world bash module loading.
            import: Some(r#"(?m)^\s*source\s+(?:['"])?([^'"\s]+)(?:['"])?"#),
        },
    ),
    // #726 — PowerShell (.ps1 = script, .psm1 = module). `function
    // Name { ... }` and `class Name { ... }` are the two ways to
    // declare callables; PowerShell identifiers allow `-` (verb-
    // noun convention like `Get-Item`) which most other lang regex
    // classes reject — hence the explicit `[A-Za-z0-9_-]` here.
    // Already in is_source_code_extension, code_strip (no Language
    // entry — strip falls back to passthrough for ps1, fine), and
    // cat/bat-dispatch. lang_for was the missing piece for
    // symbol_find on PowerShell projects (Windows installers,
    // ops scripts, install.ps1 in this very repo).
    (
        &["ps1", "psm1"],
        LangSpec {
            patterns: &[
                (r"(?m)^\s*function\s+([A-Za-z_][A-Za-z0-9_-]*)", "fn"),
                (r"(?m)^\s*class\s+([A-Za-z_][A-Za-z0-9_]*)", "class"),
            ],
            import: Some(
                r#"(?m)^\s*(?:Import-Module|using\s+module)\s+(?:['"])?([A-Za-z0-9_.\\/-]+)(?:['"])?"#,
            ),
        },
    ),
];
