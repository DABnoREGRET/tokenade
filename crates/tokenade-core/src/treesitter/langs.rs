//! **What each language calls a declaration.**
//!
//! A table, one entry per language: the syntax-tree node names that count as
//! a declaration, an import, a call. It is hand-written because each grammar
//! names things its own way — and that is why it lives alone: a table one
//! re-reads must not be drowned in the code that walks it.

use super::*;

/// Master table of supported languages. Adding a new grammar = one
/// new entry here. Node-kind lists derived from each grammar's
/// official `node-types.json`.
pub(super) static LANGS: &[LangSpec] = &[
    LangSpec {
        name: "rs",
        language: || tree_sitter_rust::LANGUAGE.into(),
        // `macro_definition` is `macro_rules! name { … }`. It declares a
        // NAME the rest of the crate calls, and the index answers "where is
        // X declared" — `tokenade query cmd_entry_points` said "no symbol
        // matching" for a macro invoked in ~600 files of this repo. Counted
        // 2026-08-27 on the benchmark corpus (ruff, pydantic-core, …): 105
        // definitions, 90 distinct names, none of them reachable.
        functions: &[
            "function_item",
            "function_signature_item",
            "macro_definition",
        ],
        types: &[
            "struct_item",
            "enum_item",
            "trait_item",
            "type_item",
            "union_item",
            "impl_item",
            // `type Item;` inside a trait declares a name an implementor
            // must supply. Its sibling `type_binding` — the `Item = u8` of
            // `Iterator<Item = u8>` — is deliberately NOT here: that is a
            // constraint at a use site, not a declaration, and indexing it
            // would answer "where is Item declared" with every generic bound
            // that mentions it.
            "associated_type",
        ],
        modules: &["mod_item"],
        members: &[
            "const_item",
            "static_item",
            "field_declaration",
            "enum_variant",
        ],
        function_scoped_members: &["const_item", "static_item"],
        receiver_fields: &[],
        imports: &["use_declaration", "extern_crate_declaration"],
        calls: &["call_expression", "macro_invocation"],
    },
    LangSpec {
        name: "py",
        language: || tree_sitter_python::LANGUAGE.into(),
        functions: &["function_definition"],
        types: &["class_definition"],
        modules: &[],
        // Module/class-level `X = …` is `expression_statement > assignment`,
        // too noisy (and value-side identifiers leak) to harvest reliably;
        // left out deliberately. Python's queryable extras are decorators,
        // already captured on functions/classes.
        members: &[],
        function_scoped_members: &[],
        receiver_fields: &["assignment"],
        imports: &["import_statement", "import_from_statement"],
        calls: &["call"],
    },
    LangSpec {
        name: "js",
        language: || tree_sitter_javascript::LANGUAGE.into(),
        functions: &[
            "function_declaration",
            "method_definition",
            "generator_function_declaration",
        ],
        types: &["class_declaration"],
        modules: &[],
        members: &["field_definition"],
        function_scoped_members: &[],
        receiver_fields: &["assignment_expression"],
        imports: &["import_statement"],
        calls: &["call_expression", "new_expression"],
    },
    LangSpec {
        name: "ts",
        language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        functions: &[
            "function_declaration",
            "method_definition",
            "method_signature",
            "generator_function_declaration",
            // The half of a contract somebody must implement, for the third
            // time in three entries (Rust `function_signature_item`,
            // `abstract class`, here). `declare function foo(): void`
            // is a `function_signature`, a different node kind from
            // `function_declaration`, and an `abstract` member is an
            // `abstract_method_signature`, not a `method_signature`. Counted
            // 2026-08-27 outside `node_modules`, which this indexer skips:
            // 2 681 declared functions and 131 abstract methods.
            "function_signature",
            "abstract_method_signature",
        ],
        types: &[
            "class_declaration",
            // `abstract class Foo` is a DIFFERENT node kind from `class Foo`,
            // so an abstract class was a class the index could not find.
            // 195 of them on this machine's TypeScript.
            "abstract_class_declaration",
            "interface_declaration",
            "type_alias_declaration",
            "enum_declaration",
        ],
        modules: &["module"],
        members: &[
            "public_field_definition",
            "property_signature",
            "enum_assignment",
        ],
        function_scoped_members: &[],
        receiver_fields: &["assignment_expression"],
        imports: &["import_statement"],
        calls: &["call_expression", "new_expression"],
    },
    LangSpec {
        name: "go",
        language: || tree_sitter_go::LANGUAGE.into(),
        functions: &["function_declaration", "method_declaration"],
        // `type X = Y` is a `type_alias`, a different node from the
        // `type_spec` of `type X Y`. 77 on this machine.
        types: &["type_declaration", "type_spec", "type_alias"],
        modules: &[],
        members: &["const_spec", "var_spec", "field_declaration"],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["import_declaration"],
        calls: &["call_expression"],
    },
    LangSpec {
        name: "java",
        language: || tree_sitter_java::LANGUAGE.into(),
        functions: &["method_declaration", "constructor_declaration"],
        types: &[
            "class_declaration",
            "interface_declaration",
            "enum_declaration",
            "record_declaration",
        ],
        modules: &["package_declaration", "module_declaration"],
        members: &["field_declaration", "constant_declaration", "enum_constant"],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["import_declaration"],
        calls: &["method_invocation", "object_creation_expression"],
    },
    LangSpec {
        name: "c",
        language: || tree_sitter_c::LANGUAGE.into(),
        // `preproc_function_def` is `#define SQUARE(x) …` — callable, so it
        // belongs with the functions.
        functions: &["function_definition", "preproc_function_def"],
        types: &[
            "struct_specifier",
            "union_specifier",
            "enum_specifier",
            "type_definition",
        ],
        modules: &[],
        // `preproc_def` is `#define MAX_PATH 4096`, which is what a C
        // constant IS — the language has no other way to spell one at file
        // scope. Counted 2026-08-27 on this machine: 106 989 named `#define`
        // in `.h` files, and `tokenade query` could reach none of them.
        members: &["field_declaration", "enumerator", "preproc_def"],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["preproc_include"],
        calls: &["call_expression"],
    },
    LangSpec {
        name: "cpp",
        language: || tree_sitter_cpp::LANGUAGE.into(),
        functions: &[
            "function_definition",
            "template_function",
            "preproc_function_def",
        ],
        types: &[
            "class_specifier",
            "struct_specifier",
            "union_specifier",
            "enum_specifier",
            "type_definition",
            "template_declaration",
            // `using Alias = T;` and `concept Foo = …;` name a type as
            // plainly as `typedef` does, in the spelling modern C++ uses.
            "alias_declaration",
            "concept_definition",
        ],
        modules: &["namespace_definition"],
        members: &["field_declaration", "enumerator", "preproc_def"],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["preproc_include"],
        calls: &["call_expression", "new_expression"],
    },
    LangSpec {
        name: "cs",
        language: || tree_sitter_c_sharp::LANGUAGE.into(),
        functions: &[
            "method_declaration",
            "constructor_declaration",
            "local_function_statement",
        ],
        types: &[
            "class_declaration",
            "interface_declaration",
            "struct_declaration",
            "enum_declaration",
            "record_declaration",
        ],
        modules: &["namespace_declaration", "file_scoped_namespace_declaration"],
        members: &[
            "field_declaration",
            "property_declaration",
            "enum_member_declaration",
            "event_field_declaration",
        ],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["using_directive"],
        calls: &["invocation_expression", "object_creation_expression"],
    },
    LangSpec {
        name: "php",
        language: || tree_sitter_php::LANGUAGE_PHP.into(),
        functions: &["function_definition", "method_declaration"],
        types: &[
            "class_declaration",
            "interface_declaration",
            "trait_declaration",
            "enum_declaration",
        ],
        modules: &["namespace_definition"],
        members: &["property_declaration", "const_declaration", "enum_case"],
        function_scoped_members: &[],
        receiver_fields: &["assignment_expression"],
        imports: &["namespace_use_declaration"],
        calls: &[
            "object_creation_expression",
            "function_call_expression",
            "member_call_expression",
            "scoped_call_expression",
        ],
    },
    LangSpec {
        name: "rb",
        language: || tree_sitter_ruby::LANGUAGE.into(),
        functions: &["method", "singleton_method"],
        types: &["class", "module"],
        modules: &[],
        // Ruby attrs/constants are dynamic (`attr_accessor`, `CONST = …`);
        // the former are call nodes, the latter `assignment` — neither maps
        // cleanly to a member kind without noise. Left out.
        members: &[],
        function_scoped_members: &[],
        receiver_fields: &["assignment"],
        imports: &[],
        calls: &["call", "method_call"],
    },
    LangSpec {
        name: "swift",
        language: || tree_sitter_swift::LANGUAGE.into(),
        functions: &[
            "function_declaration",
            "init_declaration",
            "deinit_declaration",
        ],
        types: &[
            "class_declaration",
            "protocol_declaration",
            "extension_declaration",
        ],
        modules: &[],
        members: &["property_declaration", "enum_entry"],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["import_declaration"],
        calls: &["call_expression"],
    },
    LangSpec {
        // `tree-sitter-kotlin-ng` (1.x) is the actively maintained
        // fork tracking the modern API; the original
        // `tree-sitter-kotlin` 0.3.x pins tree-sitter 0.20 and
        // collides with our 0.26 base.
        name: "kt",
        language: || tree_sitter_kotlin_ng::LANGUAGE.into(),
        functions: &["function_declaration"],
        types: &["class_declaration", "object_declaration"],
        modules: &["package_header"],
        members: &["property_declaration", "enum_entry"],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["import_header"],
        calls: &["call_expression"],
    },
    LangSpec {
        name: "scala",
        language: || tree_sitter_scala::LANGUAGE.into(),
        functions: &["function_definition", "function_declaration"],
        types: &[
            "class_definition",
            "object_definition",
            "trait_definition",
            "enum_definition",
        ],
        modules: &["package_clause"],
        members: &[
            "val_definition",
            "var_definition",
            "val_declaration",
            "var_declaration",
        ],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["import_declaration"],
        calls: &["call_expression", "instance_expression"],
    },
    LangSpec {
        name: "html",
        language: || tree_sitter_html::LANGUAGE.into(),
        // HTML/CSS/SQL don't really have "symbols" in the code-search
        // sense. We still register them so a future feature
        // (`structure_map` for templates) can use the parser.
        functions: &[],
        types: &[],
        modules: &[],
        members: &[],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &[],
        calls: &[],
    },
    LangSpec {
        name: "css",
        language: || tree_sitter_css::LANGUAGE.into(),
        functions: &[],
        types: &[],
        modules: &[],
        members: &[],
        function_scoped_members: &[],
        receiver_fields: &[],
        imports: &["import_statement"],
        calls: &[],
    },
];
