//! JSON/API-response compaction.
//!
//! Real-world API responses (GitHub, npm, Kubernetes, `cargo metadata`, …)
//! are *enormous* and ~80% noise: dozens of `*_url` permutations per object,
//! `etag` / `_links` / `_embedded` chrome, `null` placeholders, base64
//! payloads. This module walks a parsed JSON value and removes the noise an
//! agent rarely needs while preserving the signal (names, numbers, statuses,
//! the first N items of arrays, the first few hundred chars of strings).
//!
//! Design rules:
//!   - **Always lossless on signal**: never drop a key whose value is a
//!     non-trivial scalar the agent might need; truncate with a marker
//!     instead of dropping silently.
//!   - **Survives hostile input**: handles malformed JSON (returns original,
//!     never panics) and deeply nested structures (iterative walk caps depth).
//!   - **Configurable but sensible defaults**: a `CompactOptions` struct
//!     exposes the knobs; default is tuned for GitHub/npm/cargo workloads.

mod fold;
mod records;
pub use records::records_table;
mod rerun;
mod walk;
pub use fold::*;
pub use rerun::fold_unchanged_fields;
pub use walk::*;

use crate::tokenizer;
use serde_json::Value;
use std::fmt::Write;

/// Cap arrays to this many items by default; the rest is summarised as
/// `… N more items …` in a sibling key.
pub const DEFAULT_ARRAY_CAP: usize = 20;
/// Floor for the adaptive array cap — never trim a non-trivial array below this
/// many items, even when they look redundant, so the agent keeps some context.
pub const ARRAY_KEEP_MIN: usize = 3;
/// Truncate long string values to this many characters by default.
pub const DEFAULT_STR_CAP: usize = 200;
/// Safety cap on recursion depth so a 100k-level-deep JSON cannot blow the
/// stack (defended by `adv:json:deeply-nested`).
pub const MAX_DEPTH: usize = 256;

/// Keys we always drop (low signal, high noise).
///
/// Expanded from sqz's `json_projection.rs` finding (sweep #2): 50+
/// patterns covering hypermedia chrome, audit fields, telemetry tags,
/// and version counters. The principle: if an LLM reading the response
/// would skim past it, drop it. We ALWAYS preserve the actual data
/// shape (objects/arrays survive; only specific keys are stripped).
const DROP_KEYS: &[&str] = &[
    // Generic hypermedia / HAL chrome.
    "etag",
    "_links",
    "_embedded",
    "_meta",
    "node_id",
    // Common audit-trail fields agents don't need to read the data.
    // (See ai-codex schema parser: same SKIP_AUDIT_FIELDS set.)
    "createdAt",
    "updatedAt",
    "deletedAt",
    "isDeleted",
    "created_at",
    "updated_at",
    "deleted_at",
    "is_deleted",
    "modifiedAt",
    "modified_at",
    "lastModified",
    "last_modified",
    // Version / sequence counters that don't carry semantic meaning.
    //
    // #296 — `generation` and `resourceVersion` used to sit here AND in
    // `K8S_METADATA_DROPS`. Two homes for one name means the shape-specific
    // list is dead weight while the generic one over-reaches: a `generation`
    // field in a domain model is not a Kubernetes counter. They live in the
    // k8s list alone now, which is the list written for them.
    //
    // #294 — plain `version` is NOT one of them and used to sit here. It was
    // the most-removed key in the whole list: measured over 44 008 real `.json`
    // files, 5 851 of them carried it and 1 507 201 bytes went with it, half as
    // much again as the next entry. And it is the thing a reader looks up:
    // `{"name":"tokenade","version":"1.1.6"}` came back as
    // `{"name":"tokenade"}`, an OpenAPI `info.version` vanished, and a tool's
    // own `--version --json` answered without one. `__v`, `_version` and
    // `resourceVersion` ARE internal counters and stay.
    "__v",
    "_version",
    "observedGeneration",
    "revision",
    "sequence",
    "_seq",
    "_rev",
    // GitHub / Gitea registry chrome.
    "gravatar_id",
    "site_admin",
    // (Note: `html_url`/`avatar_url`/etc. are not listed here because
    // they're already covered by DROP_SUFFIXES `_url` ↔ KEEP_URL_KEYS
    // override. Adding them to DROP_KEYS would defeat the override.)
    // npm registry chrome.
    "_id",
    "_attachments",
    "_npmUser",
    "_npmVersion",
    "_nodeVersion",
    "_hasShrinkwrap",
    "_npmOperationalInternal",
    "dist-tags",
    // #293 — `users` moved to `NPM_REGISTRY_DROPS`. In an npm registry
    // document it is the legacy star list; everywhere else it is the payload,
    // and dropping it took the WHOLE document with it: `{"users":[{"id":1},
    // {"id":2}]}` came back as `{}`, and `{"users":[…3 rows…],"total":3}` as
    // `{"total":3}`. The object arm drops a key whose walked value is an empty
    // container, so removing the only key removes the document.
    // JSON-Schema markers. `$ref`, `$defs` and `definitions` are NOT here:
    // they are a schema's payload, not its chrome. Dropping them turned
    // `{"definitions":{"Pet":{…}}}` into `{}` and stripped the target out of
    // `{"$ref":"#/components/schemas/Pet","description":"a pet"}`. What stays
    // is the metadata a reader loses nothing by not seeing.
    "$schema",
    "$id",
    "$comment",
    // OpenAPI internals.
    "x-google-quota",
    "x-google-backend",
    // Telemetry / debug fields live in `telemetry_key_holding_a_value` now:
    // dropped when the key HOLDS an id, kept when it DESCRIBES one (#294).
];

/// Keys ending in any of these suffixes are dropped unless explicitly
/// kept. We added `_at` (broader audit-stamp catch-all) and `_id`
/// (internal UUID/sequence references the LLM rarely needs) to cover
/// the long tail of `xxxCreatedAt` / `xxxAuditedAt` / `xxxRequestId`.
const DROP_SUFFIXES: &[&str] = &["_url"];

/// `*_url` keys we DO keep because they're actually useful.
const KEEP_URL_KEYS: &[&str] = &["html_url", "download_url", "url", "homepage_url"];

/// Per-shape extra drops applied AFTER the generic pass.
///
/// **Kubernetes object** (`apiVersion`+`kind`+`metadata`):
/// `kubectl get pods -o json` ships 30+ fields per pod that an LLM never
/// needs: `managedFields` (12+ stanzas, sometimes >50 % of the byte budget),
/// `resourceVersion` (just a generation counter), `uid`, `generation`,
/// `ownerReferences`, server-side `status.conditions[]` boilerplate,
/// `metadata.annotations["kubectl.kubernetes.io/last-applied-configuration"]`
/// (a JSON-in-JSON dump of the original manifest).
///
/// **GitHub API object** (presence of `node_id` plus typical PR/issue/user
/// markers): drop `id`, `node_id` (already in DROP_KEYS), `*_url` (DROP_SUFFIXES),
/// nested `head.repo` / `base.repo` (massive duplicate-of-repo blobs on every
/// PR), `assignees` if empty, `permissions`, `git_refs_url`/etc.
const K8S_METADATA_DROPS: &[&str] = &[
    "managedFields",
    "resourceVersion",
    "uid",
    "generation",
    "selfLink",
    "creationTimestamp",
    "deletionTimestamp",
    "deletionGracePeriodSeconds",
    "ownerReferences",
    "finalizers",
];

const K8S_ANNOTATION_DROPS: &[&str] = &[
    "kubectl.kubernetes.io/last-applied-configuration",
    "deployment.kubernetes.io/revision",
    "control-plane.alpha.kubernetes.io/leader",
];

/// #293 — npm registry chrome, dropped ONLY inside an npm registry document.
/// `users` is the legacy star list there and the payload everywhere else.
const NPM_REGISTRY_DROPS: &[&str] = &["users"];

const GH_PR_NESTED_DROPS: &[&str] = &[
    "head",
    "base",
    // #296 — `_links` is HAL hypermedia, not a GitHub invention, so it stays in
    // the generic list and does not need a second home here.
    "active_lock_reason",
    "performed_via_github_app",
];

/// What shape this JSON object looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// Kubernetes resource (or list item): has `apiVersion` + `kind` + `metadata`.
    Kubernetes,
    /// npm registry packument: has `dist-tags` + `versions`.
    NpmRegistry,
    /// GitHub REST API resource (issue / PR / commit / user / repo): has
    /// `node_id` AND at least one of the URL-chrome keys.
    GitHub,
    /// Anything else — generic compaction only.
    Generic,
}

fn detect_shape(obj: &serde_json::Map<String, Value>) -> Shape {
    if obj.contains_key("apiVersion") && obj.contains_key("kind") && obj.contains_key("metadata") {
        return Shape::Kubernetes;
    }
    // An npm registry document: the packument carries `dist-tags` together with
    // the per-version `versions` map. Both, so an ordinary object that happens
    // to have one of them is not mistaken for a packument.
    if obj.contains_key("dist-tags") && obj.contains_key("versions") {
        return Shape::NpmRegistry;
    }
    if obj.contains_key("node_id")
        && (obj.contains_key("html_url")
            || obj.contains_key("pull_request")
            || obj.contains_key("issue_url")
            || obj.contains_key("commits_url"))
    {
        return Shape::GitHub;
    }
    Shape::Generic
}

#[derive(Debug, Clone)]
pub struct CompactOptions {
    pub array_cap: usize,
    pub str_cap: usize,
    pub drop_nulls: bool,
    pub drop_empty: bool,
    pub max_depth: usize,
}

impl Default for CompactOptions {
    fn default() -> Self {
        Self {
            array_cap: DEFAULT_ARRAY_CAP,
            str_cap: DEFAULT_STR_CAP,
            drop_nulls: true,
            drop_empty: true,
            max_depth: MAX_DEPTH,
        }
    }
}

/// Compact a JSON document.
///
/// L65 — for top-level homogeneous arrays (≥5 items, schema-match
/// ≥80 %), tries the Option-C-style schema-fold first. It's a
/// dramatic win on REST list responses where the body is `[{...},
/// {...}, ...×N]` of structurally identical objects. When the
/// fold doesn't apply, falls back to the existing per-key compactor.
///
/// On malformed input, returns the original string verbatim (never panics).
/// How many distinct values per key the schema fold counts before it stops
/// counting and says so. Bounds the memory on a million-row array; the summary
/// only needs "one value or many".
const DISTINCT_CAP: usize = 200;

pub fn compact(json: &str) -> String {
    // Security pre-pass: a browser cookie export (EditThisCookie /
    // Cookie-Editor: `[{"name":…,"value":…,"domain":…,"httpOnly":…}]`) carries
    // every cookie's raw value. Redact the `value` field STRUCTURALLY before
    // any fold runs — the lossless TOON fold would otherwise dump every value
    // verbatim. Gated per-object by the cookie signature (see
    // [`redact_cookie_values`]) so generic `{"name","value"}` pairs survive.
    if let Ok(mut v) = serde_json::from_str::<Value>(json) {
        if crate::redact::redact_cookie_values(&mut v) {
            let sanitized = serde_json::to_string(&v).unwrap_or_else(|_| json.to_string());
            if let Some(folded) = schema_fold(&sanitized) {
                return folded;
            }
            return compact_with(&sanitized, &CompactOptions::default());
        }
    }
    if let Some(folded) = schema_fold(json) {
        return folded;
    }
    compact_with(json, &CompactOptions::default())
}

#[cfg(test)]
include!("json_tests.rs");
