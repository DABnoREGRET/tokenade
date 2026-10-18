//! `graphql` benchmark cases: realistic shape, then the adversarial ones.

use super::super::*;

pub(crate) fn synth_graphql(
    n_types: usize,
    fields_per_type: usize,
    docstring_size: usize,
) -> String {
    let mut s = String::from("schema { query: Query, mutation: Mutation }\n");
    s.push_str("scalar DateTime\nscalar JSON\n");
    s.push_str("directive @auth(role: Role!) on FIELD_DEFINITION\n");
    s.push_str("directive @deprecated(reason: String) on FIELD_DEFINITION\n");
    s.push_str("enum Role { ADMIN USER GUEST }\n");
    s.push_str("interface Node { id: ID! }\n");
    s.push_str("union SearchResult = Type0 | Type1\n");

    // Query block — half the types are queryable.
    s.push_str("type Query {\n");
    for i in 0..(n_types / 2) {
        s.push_str(&format!("  thing{i}(id: ID!, filter: String): Type{i}\n"));
    }
    s.push_str("}\n");

    s.push_str("type Mutation {\n");
    for i in 0..(n_types / 4) {
        s.push_str(&format!(
            "  createType{i}(input: Type{i}Input!): Type{i}!\n"
        ));
    }
    s.push_str("}\n");

    s.push_str("type Subscription {\n");
    for i in 0..(n_types / 10) {
        s.push_str(&format!("  type{i}Updated(id: ID!): Type{i}!\n"));
    }
    s.push_str("}\n");

    let pad = "lorem ipsum ".repeat(docstring_size.max(1) / 12);
    for i in 0..n_types {
        s.push_str("\"\"\"\n");
        s.push_str(&pad);
        s.push_str("\n\"\"\"\n");
        s.push_str(&format!("type Type{i} implements Node {{\n"));
        for f in 0..fields_per_type {
            s.push_str(&format!(
                "  # comment on field{f}\n  field{f}(arg{f}: String = \"default value {f}\"): String\n"
            ));
        }
        s.push_str("}\n");
        s.push_str(&format!(
            "input Type{i}Input {{\n  id: ID!\n  name: String!\n}}\n"
        ));
    }
    s
}

pub(crate) fn task_graphql_real(results: &mut Vec<RunResult>) {
    let raw = synth_graphql(60, 8, 1_500);
    let (out, sav, ms) = measure(&raw, graphql::compact_with_savings);
    let quality = if out.contains("[tokenade:graphql] 60 types,")
        && out.contains("30 queries,")
        && out.contains("15 mutations,")
        && out.contains("6 subscriptions")
        && out.contains("Type0")
        && out.contains("Type59")
        && out.contains("thing0(id: ID!, filter: String): Type0")
        && out.contains("createType0(input: Type0Input!): Type0!")
        && out.contains("type0Updated(id: ID!): Type0!")
        && !out.contains("lorem ipsum")
        && !out.contains("# comment on field0")
        && sav.percent() > 75.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "output:graphql-compact",
        "graphql",
        "synth-60types-8fields",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #49: github-graphql-style schema — 1 000 types, 12 fields each,
// 5 KB docstring per type. Must compact >85 % in under 2 s.
pub(crate) fn adv_graphql_huge(results: &mut Vec<RunResult>) {
    let raw = synth_graphql(1_000, 12, 5_000);
    let (out, sav, ms) = measure(&raw, graphql::compact_with_savings);
    let quality = if out.contains("1000 types")
        && out.contains("500 queries")
        && sav.percent() > 85.0
        && ms < 2000
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:graphql:huge-1000types",
        "graphql",
        "adversarial",
        &raw,
        &out,
        ms,
        quality,
    );
}

// Adversarial #50: docstring booby-trap + edge syntax. The docstring contains
// "type Sneaky { fake }" which would false-positive a naive line scanner.
// Unicode type names + multi-line field signatures + trailing extend block
// stress the parser further.
pub(crate) fn adv_graphql_hostile(results: &mut Vec<RunResult>) {
    let raw = r#"
"""
Beware: a docstring that mentions
  type Sneaky { fake }
  enum FakeEnum
  directive @notReal on FOO
should NOT leak into the output.
"""
schema { query: Query, mutation: Mutation }

scalar URL
directive @real on FIELD_DEFINITION
interface Node { id: ID! }

# Unicode identifier — GraphQL allows ASCII only per the spec, but
# many tooling chains allow underscored/numeric identifiers and we
# should keep working when handed lenient input.
pub(crate) type Müller implements Node {
  id: ID!
}

pub(crate) type Query {
  # multi-line field signature must collapse cleanly
  multiLine(
    a: String,
    b: Int = 42,
    c: [String!] = ["x", "y", "z"]
  ): String

  simple: String
}

extend type Query {
  extended(x: ID!): String
}

extend type Mutation {
  doIt: Boolean!
}
"#;
    let (out, sav, ms) = measure(raw, graphql::compact_with_savings);
    let quality = if !out.contains("Sneaky")        // docstring booby-trap not leaked
        && !out.contains("FakeEnum")
        && !out.contains("notReal")
        && out.contains("directives (1): real")     // only the real directive picked up
        && out.contains("multiLine( a: String, b: Int = 42, c: [String!] = [\"x\", \"y\", \"z\"] ): String")
        && out.contains("simple: String")
        && out.contains("extended(x: ID!): String") // extend type Query collected
        && out.contains("doIt: Boolean!")           // extend type Mutation collected
        && ms < 100
        && sav.percent() > 0.0
    {
        1.0
    } else {
        0.0
    };
    record(
        results,
        "adv:graphql:hostile",
        "graphql",
        "adversarial",
        raw,
        &out,
        ms,
        quality,
    );
}
