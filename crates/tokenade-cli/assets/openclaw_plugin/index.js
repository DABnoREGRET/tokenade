// tokenade <-> OpenClaw bridge (auto-installed by `tokenade install`).
//
// REWRITTEN 2026-08-10 against openclaw 2026.1.30. The previous version was
// INERT and had been for some time: it imported `definePluginEntry` from
// `openclaw/plugin-sdk/plugin-entry` (gone), declared a contract named
// `agentToolResultMiddleware` (absent from the whole `dist/`), and was
// registered with `--force` (not an option). Three independent failures, none
// of which surfaced anywhere the user would look — `openclaw plugins list`
// simply never showed tokenade.
//
// The real surface in this version:
//   * a plugin is a DEFAULT-exported object `{ id, name, description,
//     register(api) }` — no SDK import is required at all;
//   * hooks are attached with `api.on(name, handler)`;
//   * `tool_result_persist` is the only hook that can REPLACE a tool result.
//     Its sibling `after_tool_call` is fire-and-forget, and in 2026.1.30 it has
//     no emitter in `dist/` at all.
//
// `tool_result_persist` is SYNCHRONOUS BY CONTRACT. OpenClaw's own comment:
// "this hook is intentionally synchronous: it runs in hot paths where session
// transcripts are appended synchronously", and it explicitly rejects a handler
// that returns a Promise — the result is dropped with a warning. So the call
// into the binary is `spawnSync`, and its cost lands on the turn. That is the
// accepted price of covering openclaw at all; the alternative was to stop
// claiming openclaw as covered.
//
// The degradation this header used to record — the event carries
// `{ toolName, toolCallId, message, isSynthetic }` and NOT the tool's input, so
// there was no command string to pass as `--cmd` and output whose fold depends
// on the hint (a `git log`, which does not compact at all without it) folded
// less here than on the four bridges that get the input — is CLOSED since
// 2026-08-21. `before_tool_call` carries `{ toolName, params, toolCallId }`,
// and `PENDING` below holds the params until the result comes back. The hint
// is still optional: on an openclaw too old to have that hook the fold runs
// exactly as it did before.
import { spawn, spawnSync } from "node:child_process";

// Only truly-interactive PTY tools are skipped.
const SKIP = new Set(["terminal"]);
const MIN_CHARS = 1000;
// A payload too small to be worth FOLDING is not too small to hold a SECRET.
// Measured 2026-08-08 over 8 940 real tool results: 78 % sit under MIN_CHARS,
// so that floor meant more than three quarters of everything an agent read was
// never scanned for credentials. An `AKIA…` key is 20 characters.
//
// Folding still needs MIN_CHARS — below it there is nothing to compact — so the
// two floors are now separate concerns rather than one number doing two jobs.
// Between the two, the binary redacts and does not fold, and the acceptance
// test below keeps the raw text unless something was actually scrubbed: a clean
// small payload therefore comes back byte-identical.
const REDACT_MIN_CHARS = 8;
const MAX_CHARS = 4000000; // above this, don't load+pipe it (a 200MB result stalls the turn)
const TIMEOUT_MS = 10000; // bounded synchronous stall per tool result

function tokenadeBin() {
  // `@@TOKENADE_BIN@@` is replaced with the absolute binary path by the
  // installer (cross-OS). The env var still wins; if injection was skipped we
  // fall back to the bare name resolved on PATH.
  const injected = "@@TOKENADE_BIN@@";
  return process.env.TOKENADE_BIN || (injected.startsWith("@@") ? "tokenade" : injected);
}

let resolvedBin = null;
// One tool call = at least one API call that re-reads everything tokenade put
// in this conversation. The host runs no hook process of ours per call, so the
// bridge tells the binary (`session-tick`), detached: the tool never waits.
// `reset`: the conversation was compacted or reset — what tokenade folded out
// of it or put into it is no longer re-read, on either side of the ledger.
function tickSession(session, agent, reset = false) {
  if (!session) return;
  try {
    const argv = ["session-tick", "--session", session, "--agent", agent];
    if (reset) argv.push("--reset");
    const c = spawn(resolvedBin || tokenadeBin(), argv, {
      stdio: "ignore",
      detached: true,
      // Windows: a detached child gets a console window of its own.
      windowsHide: true,
    });
    c.on("error", () => {});
    c.unref();
  } catch {
    /* fail-open */
  }
}

function runTokenade(argv, opts) {
  const first = resolvedBin || tokenadeBin();
  let r = spawnSync(first, argv, opts);
  if (r.error && r.error.code === "ENOENT" && first !== "tokenade") {
    r = spawnSync("tokenade", argv, opts);
    if (!r.error) resolvedBin = "tokenade";
  } else if (!r.error) {
    resolvedBin = first;
  }
  return r;
}

const REDACTED_SIGNAL = "[tokenade] redacted:";
function scrubbed(r) {
  return typeof r.stderr === "string" && r.stderr.includes(REDACTED_SIGNAL);
}

// A file the HOST could not read properly.
//
// The bridges fold a tool RESULT, which works for text and cannot work for a
// document: by the time a `.docx`, `.xlsx` or `.pdf` reaches this hook the host
// has already decoded its bytes lossily to UTF-8, and no amount of compaction
// puts the words back. `tokenade read <path>` opens the file itself and runs
// the extractor — the same one the Read HOOK gives the Claude family, which is
// why `docs` was green there and amber on every bridge agent.
//
// Substring matching, like the shell test above and for the same reason: hosts
// name it `read`, `read_file`, `readFile`, `view`, `Read`. A false positive is
// bounded twice — the tool must also carry a path in its arguments, and a
// RANGE read (offset/limit) is left alone, because the range targets the
// original file and our extraction is not it.
const READ_WORDS = ["read", "view"];
function isReadTool(name) {
  const n = String(name || "").toLowerCase();
  return READ_WORDS.some((w) => n.includes(w));
}
function readTargetPath(args) {
  if (!args || typeof args !== "object") return null;
  if (args.offset != null || args.limit != null || args.start_line != null) return null;
  const p = args.filePath ?? args.file_path ?? args.path ?? args.absolute_path ?? args.target_file;
  return typeof p === "string" && p.trim() ? p : null;
}
const SHELL_WORDS = ["bash", "shell", "run", "execute", "command"];
function isShellTool(name) {
  const n = String(name || "").toLowerCase();
  return SHELL_WORDS.some((w) => n.includes(w));
}


// ── Session identity, so a re-delivery can be folded ──────────────────
//
// `tokenade read` folds a repeated delivery to a §ref pointer only when it is
// told which session provably received those bytes; with no session it never
// folds, by design — it cannot claim "you already have this" about a context
// it knows nothing about. Nothing on the bridge path ever supplied one, so
// every bridge agent went without read-dedup. Measured on the compat bench
// 2026-08-18, all three OS: the same bytes through `tokenade read -` twice
// came back the same size both times.
//
// The host's own conversation id is preferred, because that is what the claim
// is actually about. The process is the fallback and is a session's lifetime
// on every host that spawns one agent per process.
//
// The pid alone is NOT the fallback, and the bridge e2e proved why: it drives
// all four JS bridges in ONE node process, they shared a key, and the second
// bridge to see a payload folded it to a pointer into the FIRST one's context.
// Production spawns one agent per process, so this was a harness artefact —
// but a key that can name two contexts is wrong wherever it happens.
const PROCESS_SESSION = `p${process.pid}-openclaw`;
// `ctx.sessionKey` is the one session field every openclaw hook context
// carries (tool, persist, agent): one gateway runs many conversations, and the
// process key alone pooled them — a compaction in one reset the others.
function sessionKey(...candidates) {
  for (const c of candidates) {
    if (typeof c === "string" && c.trim()) return c;
  }
  return PROCESS_SESSION;
}

// Fold one string. Returns the original when there is nothing to gain: the
// caller must be able to tell "unchanged" by identity.
// The document path, when the host's event names a file it read. Returns the
// extracted text, or null to fall through to the generic fold.
function foldDocument(rp, text, session) {
  try {
    const d = runTokenade(["read", rp], {
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      maxBuffer: 64 * 1024 * 1024,
      env: {
        ...process.env,
        TOKENADE_AGENT: "openclaw",
        TOKENADE_SESSION: sessionKey(session),
      },
    });
    // The DOCUMENT path is not a fold, so it is not judged by the fold's rule.
    // "Is it smaller?" has no answer when the host produced NO text at all: a
    // Read the host declined to decode comes back empty, `0 < 0` is false, and
    // the extractor's words were thrown away in favour of the empty string.
    // Measured 2026-08-21 on the compat bench, where a `.docx` read through
    // this bridge returned nothing while `tokenade read <path>` had the words.
    if (d.status === 0 && d.stdout && (text.length === 0 || d.stdout.length < text.length || scrubbed(d))) {
      return d.stdout;
    }
  } catch {
    // fail-open: never break the host on a folding error
  }
  return null;
}

function foldText(text, session, cmd) {
  if (typeof text !== "string") return text;
  if (text.length < REDACT_MIN_CHARS || text.length > MAX_CHARS) return text;
  if (text.startsWith("data:") || text.charCodeAt(0) === 0) return text;
  try {
    const argv = cmd ? ["read", "-", "--cmd", cmd] : ["read", "-"];
    const x = runTokenade(argv, {
      input: text,
      encoding: "utf8",
      timeout: TIMEOUT_MS,
      maxBuffer: 64 * 1024 * 1024,
      env: {
        ...process.env,
        TOKENADE_AGENT: "openclaw",
        TOKENADE_SESSION: sessionKey(session),
      },
    });
    // Accept a LONGER result only when something was actually scrubbed:
    // `API_KEY=…` becomes `API_KEY=<redacted>`, and a length-only rule would
    // throw the safe version away and hand the model the raw key.
    if (x.status === 0 && x.stdout && (x.stdout.length < text.length || scrubbed(x))) {
      return x.stdout;
    }
  } catch {
    // fail-open: never break the host on a folding error
  }
  return text;
}

// ── What the tool was ASKED, kept until its result comes back ─────────
//
// `tool_result_persist` carries `{ toolName, toolCallId, message,
// isSynthetic }` and NOT the tool's input, so this bridge folded blind: no
// `--cmd` hint, and no path for the document extractor either. The header of
// this file recorded that as a KNOWN DEGRADATION of the host's hook surface,
// and it stopped being true. Measured on the published tarballs 2026-08-21,
// `before_tool_call` has carried `{ toolName, params, toolCallId }` since
// 2026.6.10 — the same release that stopped refusing this plugin at install.
//
// So we remember the params under the call id and read them back one hook
// later. The map is bounded and every entry is deleted the moment it is used;
// a tool whose result never arrives (a cancelled turn) is evicted by age.
//
// Registered defensively: on an openclaw that does not know the hook,
// `api.on` may throw, and a bridge that dies while registering folds nothing
// at all. The fold still works without the hint, exactly as it did before.
const PENDING = new Map();
const PENDING_MAX = 64;
function rememberCall(event) {
  const id = event && event.toolCallId;
  if (typeof id !== "string" || !id) return;
  const params = event.params;
  if (!params || typeof params !== "object") return;
  if (PENDING.size >= PENDING_MAX) PENDING.delete(PENDING.keys().next().value);
  PENDING.set(id, params);
}
function takeCall(id) {
  if (typeof id !== "string" || !id) return null;
  const params = PENDING.get(id);
  if (params === undefined) return null;
  PENDING.delete(id);
  return params;
}
// The command a shell tool was given, in whichever field this host named it.
function commandOf(params) {
  if (!params || typeof params !== "object") return null;
  const c = params.command ?? params.cmd ?? params.script ?? params.shellCommand ?? params.shell_command;
  return typeof c === "string" && c.trim() ? c : null;
}

export default {
  id: "tokenade",
  name: "Tokenade",
  description: "Compacts tool results via the tokenade binary to save tokens.",

  register(api) {
    try {
      api.on("before_tool_call", (event, ctx) => {
        rememberCall(event);
        tickSession(sessionKey(event?.sessionId, event?.sessionID, ctx?.sessionKey), "openclaw");
        // Observe only. Returning a `params` object here would hand the host a
        // rewritten tool call, which is not what this bridge is for.
        return undefined;
      });
    } catch {
      // An older openclaw has no such hook; the fold below still runs.
    }
    // `after_compaction` / `before_reset` (openclaw's `PluginHookName`). Keyed
    // like the ticks above, or the reset would land on another session.
    for (const hook of ["after_compaction", "before_reset"]) {
      try {
        api.on(hook, (event, ctx) => {
          tickSession(sessionKey(event?.sessionId, event?.sessionID, ctx?.sessionKey), "openclaw", true);
        });
      } catch {
        // An older openclaw has no such hook.
      }
    }

    api.on("tool_result_persist", (event, ctx) => {
      const message = event && event.message;
      if (!message) return;
      if (SKIP.has(String(event.toolName || "").toLowerCase())) return;
      // A synthetic result is one OpenClaw invented to repair a transcript
      // (a missing tool result). Folding it would compact an apology.
      if (event.isSynthetic) return;

      const content = message.content;
      const session = sessionKey(event?.sessionId, event?.sessionID, message?.sessionId, ctx?.sessionKey);
      // The event itself has never carried the input; `before_tool_call` did.
      // The three older spellings stay as a fallback for a host that grows one.
      const params = takeCall(event?.toolCallId) ?? event?.args ?? event?.toolInput ?? event?.input;
      const rp = isReadTool(event?.toolName) ? readTargetPath(params) : null;
      const cmd = isShellTool(event?.toolName) ? commandOf(params) : null;

      if (typeof content === "string") {
        const doc = rp ? foldDocument(rp, content, session) : null;
        const next = doc ?? foldText(content, session, cmd);
        return next === content ? undefined : { message: { ...message, content: next } };
      }
      if (!Array.isArray(content)) return;

      let changed = false;
      const blocks = content.map((b) => {
        if (!b || typeof b !== "object" || b.type !== "text" || typeof b.text !== "string") {
          // Images and non-text blocks pass through untouched.
          return b;
        }
        const doc = rp ? foldDocument(rp, b.text, session) : null;
        const next = doc ?? foldText(b.text, session, cmd);
        if (next === b.text) return b;
        changed = true;
        return { ...b, text: next };
      });
      // Returning nothing leaves the message exactly as it was, which matters:
      // this hook chains, and handing the next handler a rebuilt object for no
      // reason is how a "no-op" starts changing things.
      return changed ? { message: { ...message, content: blocks } } : undefined;
    });

    // ── The response-shape preamble ────────────────────────────────────
    //
    // `before_agent_start` carries the user's prompt and accepts
    // `prependContext` in its answer. Copied from OpenClaw's OWN shipped
    // extension (`extensions/memory-lancedb/index.ts`, which returns
    // `{ prependContext }` the same way), not from a reading of its docs —
    // this bridge registered one event out of the several the host has, and
    // the compat bench reported that as "no prompt-time hook exists for this
    // agent".
    //
    // The binary still owns the decision: `style --preamble` resolves the
    // per-project setting, the auto-classification and `off`, and prints
    // nothing when the answer is "inject nothing".
    api.on("before_agent_start", (event, ctx) => {
      const prompt = typeof event?.prompt === "string" ? event.prompt : "";
      if (!prompt) return;
      try {
        const session = sessionKey(event?.sessionId, event?.sessionID, ctx?.sessionKey);
        const r = runTokenade(["style", "--preamble", "--book", "--session", session, "--prompt", prompt.slice(0, 400)], {
          encoding: "utf8",
          timeout: TIMEOUT_MS,
          maxBuffer: 1024 * 1024,
          env: { ...process.env, TOKENADE_AGENT: "openclaw" },
        });
        const text = typeof r.stdout === "string" ? r.stdout.trim() : "";
        if (r.status === 0 && text) return { prependContext: text };
      } catch {
        // fail-open: never break a turn over a preamble
      }
      return undefined;
    });

    api.logger?.info?.("tokenade: folding tool results via tool_result_persist");
  },
};

// Exported for the bridge test: the host is not available there, so the shape
// checks run against these directly.
export { SKIP, MIN_CHARS, REDACT_MIN_CHARS, MAX_CHARS, isShellTool, scrubbed, rememberCall, takeCall, commandOf, PENDING_MAX };
