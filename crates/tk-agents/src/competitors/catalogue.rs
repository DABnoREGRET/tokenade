//! **The list**, one entry per known competing tool.
//!
//! It carries each one's CLASS, and the class decides what `tokenade
//! install` offers: remove, advise against, or merely note. Getting
//! the class wrong offers to delete a tool the user
//! chose — the only action of this product that cannot be undone.
//!
//! The table is separate from the detectors because it changes for a different
//! reason than they do: an existing tool can change class without anything
//! changing in how to find it, and the reverse is true as well.

use super::*;

// =======================================================================
// Catalogue
// =======================================================================

/// All competitors we know about. Detection functions are pure (modulo
/// the `DetectionCtx` IO callbacks). Add a new entry here when a new
/// competitor warrants explicit handling.
pub const CATALOGUE: &[Competitor] = &[
    Competitor {
        name: "rtk",
        class: Class::DropInBetter,
        one_liner: "Output-filtering CLI proxy (filters/compresses bash output)",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_rtk,
    },
    Competitor {
        name: "lean-ctx",
        class: Class::DropInBetter,
        one_liner: "Context layer that replaces Read/Grep/Shell with its own MCP tools",
        rationale: "Tokenade already covers this. Its rules tell the agent to use `ctx_read` / `ctx_shell` INSTEAD of native tools, which contradicts Tokenade's own lesson in the same files — the agent gets two opposite orders. Recommended: keep only Tokenade.",
        detect: detect_lean_ctx,
    },
    Competitor {
        name: "codegraph",
        class: Class::DropInBetter,
        one_liner: "MCP symbol-graph + FTS5 index (regenerable from source)",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_codegraph,
    },
    Competitor {
        name: "tokensave",
        class: Class::DropInBetter,
        one_liner: "Rust MCP symbol+import graph",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_tokensave,
    },
    Competitor {
        name: "codesight",
        class: Class::DropInBetter,
        one_liner: "MCP structure-map + skeleton + memory",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_codesight,
    },
    Competitor {
        name: "claude-mem",
        class: Class::Adjacent,
        one_liner: "Cross-session observation memory (claude-mem)",
        rationale: "Cross-session memory layer. Tokenade relies on your agent's native memory and does not manage a separate memory store, so this is complementary — keep it.",
        detect: detect_claude_mem,
    },
    Competitor {
        name: "agentmemory",
        class: Class::Adjacent,
        one_liner: "Cross-agent persistent memory (agentmemory)",
        rationale: "Cross-session memory layer. Tokenade relies on your agent's native memory and does not manage a separate memory store, so this is complementary — keep it.",
        detect: detect_agentmemory,
    },
    Competitor {
        name: "claude-context",
        class: Class::Adjacent,
        one_liner: "Milvus-backed semantic code search (claude-context)",
        rationale: "Requires an external Milvus server. Tokenade's hybrid BM25 + e5 + reranker covers the same use case with zero external deps. We don't auto-migrate (Milvus data lives in the server, not on disk locally), but you can disable claude-context and use `tokenade ask`.",
        detect: detect_claude_context,
    },
    Competitor {
        name: "token-optimizer",
        class: Class::Adjacent,
        one_liner: "Plugin with dashboard + checkpoints (alexgreensh/token-optimizer)",
        rationale: "A dashboard-focused plugin on a non-commercial license. Co-installable if you accept its license; surfaced here so you know it was detected.",
        detect: detect_token_optimizer_alexgreensh,
    },
    Competitor {
        name: "claudeslim",
        class: Class::AntiPattern,
        one_liner: "API payload compressor (renames tool keys, hashes prompts)",
        rationale: "Mangles the Anthropic API payload (tool defs, system prompt) in a way that BREAKS Anthropic's prompt cache — the 90% server-side discount you'd otherwise get. Its headline savings can be net-negative on long sessions once the cache is lost. Removal recommended.",
        detect: detect_claudeslim,
    },
    Competitor {
        name: "lazy-mcp",
        class: Class::Adjacent,
        one_liner: "MCP tool-loading proxy (lazy-mcp)",
        rationale: "Tokenade already does lazy tool loading natively (`tools/list` returns compact summaries). lazy-mcp as a proxy in FRONT of us would only re-hide what we already lazy-load. Suggest removing the proxy when using tokenade directly; otherwise harmless.",
        detect: detect_lazy_mcp,
    },
    Competitor {
        name: "caveman",
        class: Class::DropInBetter,
        one_liner: "Output-side brevity skill (caveman)",
        rationale: "Tokenade already injects its own output-brevity and minimal-code rules. Adding this gives the agent a second, redundant set of prompt instructions — it doesn't save more, it only adds tokens (its rules are re-injected on top of Tokenade's every session). Recommended: keep only Tokenade.",
        detect: detect_caveman,
    },
    Competitor {
        name: "ponytail",
        class: Class::DropInBetter,
        one_liner: "Minimal-code behavioural skill / plugin (ponytail)",
        rationale: "Tokenade already injects its own output-brevity and minimal-code rules. Adding this gives the agent a second, redundant set of prompt instructions — it doesn't save more, it only adds tokens (its rules are re-injected on top of Tokenade's every session). Recommended: keep only Tokenade.",
        detect: detect_ponytail,
    },
    Competitor {
        name: "ccusage",
        class: Class::Adjacent,
        one_liner: "Usage / cost tracker (ccusage)",
        rationale: "Tracker, not optimizer. Composable with Tokenade — keep it.",
        detect: detect_ccusage,
    },
    Competitor {
        name: "graphify",
        class: Class::DropInBetter,
        one_liner: "Codebase dependency graph CLI (graphify)",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_graphify,
    },
    Competitor {
        name: "context-mode",
        class: Class::Adjacent,
        one_liner: "Sandbox MCP + session-event store (mksglu/context-mode, ELv2)",
        rationale: "Cross-session memory layer. Tokenade relies on your agent's native memory and does not manage a separate memory store, so this is complementary — keep it.",
        detect: detect_context_mode,
    },
    Competitor {
        name: "token-optimizer-mcp",
        class: Class::DropInBetter,
        one_liner: "MCP cache/compression layer (ooples/token-optimizer-mcp)",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_token_optimizer_mcp,
    },
    Competitor {
        name: "mcp-codebase-index",
        class: Class::DropInBetter,
        one_liner: "Pickle-based codebase index (mcp-codebase-index, AGPL-3)",
        rationale: "Tokenade already covers this. Running several tools for the same job leaves the agent unsure which to use and adds token overhead — every extra tool carries a fixed startup/per-call cost. Recommended: keep only Tokenade.",
        detect: detect_mcp_codebase_index,
    },
    Competitor {
        name: "claude-code-memory-setup",
        class: Class::Adjacent,
        one_liner: "Obsidian-vault-backed memory setup (lucasrosati)",
        rationale: "Cross-session memory layer. Tokenade relies on your agent's native memory and does not manage a separate memory store, so this is complementary — keep it.",
        detect: detect_claude_code_memory_setup,
    },
    Competitor {
        name: "mcp-memory-service",
        class: Class::Adjacent,
        one_liner: "Multi-backend memory MCP (doobidoo/mcp-memory-service)",
        rationale: "Cross-session memory layer. Tokenade relies on your agent's native memory and does not manage a separate memory store, so this is complementary — keep it.",
        detect: detect_mcp_memory_service,
    },
    Competitor {
        name: "mcp-memory-keeper",
        class: Class::Adjacent,
        one_liner: "Per-project SQLite memory MCP (mkreyman/mcp-memory-keeper)",
        rationale: "Cross-session memory layer. Tokenade relies on your agent's native memory and does not manage a separate memory store, so this is complementary — keep it.",
        detect: detect_mcp_memory_keeper,
    },
    Competitor {
        name: "headroom",
        class: Class::Adjacent,
        one_liner: "Python context-compression SDK for LLM message arrays (chopratejas/headroom)",
        rationale: "Headroom operates at the conversation layer (message-array scoring, prefix-cache alignment, CCR reversibility) while Tokenade operates at the tool-output layer. Different levels, composable. Headroom's content router handles shell outputs too, but with generic detectors; Tokenade has 120+ format-specific compactors that beat it on CLI noise. Co-installable: headroom manages the message window, Tokenade manages what enters it.",
        detect: detect_headroom,
    },
    Competitor {
        name: "get-shit-done",
        class: Class::Adjacent,
        one_liner: "Meta-prompting / spec-driven workflow system (get-shit-done-cc)",
        rationale: "Different layer entirely: GSD structures how you PROMPT (specs, plans, /gsd:* slash commands and skills) — it never touches tool outputs or the token flow. Its only hooks are SessionStart (update check + statusline), so it cannot conflict with Tokenade's PreToolUse interception. Fully composable — keep both. If your savings read 0% with GSD installed, the cause is Tokenade's own hook installation, not GSD.",
        detect: detect_gsd,
    },
];
