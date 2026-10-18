// tokenade <-> Pi bridge (auto-installed by `tokenade install`).
//
// Subscribes to Pi's `tool_result` event and compacts large text results via
// the tokenade binary, returning the smaller version Pi then shows the model.
// Bash/terminal results are skipped — tokenade's universal shell proxy already
// compacts shell commands before Pi sees them. Mixed/non-text results (images,
// etc.) are left untouched.
//
// No type annotations / no SDK import: loaded as-is by Pi's jiti loader under
// Pi's own Node runtime, so there's no compile step and no dependency to ship.
// All real compaction logic lives in the Rust binary.
import { spawn, spawnSync } from "node:child_process";

// Only truly-interactive PTY tools are skipped. Bash/shell command RESULTS are
// folded HERE (shell-agnostic) rather than via the PATH shell-proxy, which
// can't shim cmd.exe on Windows → without this, cmd.exe command output reaches
// Pi uncompacted. Re-folding an already-shimmed result is a harmless no-op (we
// only replace when strictly shorter → no double-count).
const SKIP = new Set(["terminal"]);
const MIN_CHARS = 1000; // below this there's nothing to fold; skip the round-trip
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
const TIMEOUT_MS = 10000; // bounded synchronous stall per tool_result

function tokenadeBin() {
  // `@@TOKENADE_BIN@@` is replaced with the absolute binary path by the
  // installer (cross-OS). The env var still wins; if injection was skipped we
  // fall back to the bare name resolved on PATH.
  const injected = "@@TOKENADE_BIN@@";
  return process.env.TOKENADE_BIN || (injected.startsWith("@@") ? "tokenade" : injected);
}

// The command that produced a shell result, when the event carries it.
//
// `tokenade read -` detects by CONTENT, and content cannot say which program
// wrote it: measured 2026-08-07, `git log --oneline` came back unchanged
// without this and folded to a quarter with it. Defensive about WHERE the
// command lives — the event shape is the host's, not ours — and restricted to
// shell-ish tools, because a `read`/`grep` tool's "command" would send the
// dispatcher to the wrong compactor. A miss means today's behaviour exactly.
// tokenade prints this on stderr when its output has SECRETS REMOVED. That
// output is often LONGER than what we sent (`API_KEY=k0` becomes
// `API_KEY=<redacted>`), so the "strictly shorter" rule below would throw the
// scrubbed version away and hand the model the raw key. Measured 2026-08-08:
// three ~2 kB payloads well above MIN_CHARS each grew about a tenth once
// scrubbed, and all of them were discarded. Length answers "did it fold"; it
// cannot answer "was a secret removed", and only tokenade knows that.
// An injected ABSOLUTE path goes stale the moment tokenade is moved, upgraded
// into a different prefix, or uninstalled and reinstalled. Every spawn then
// fails with ENOENT, the catch below swallows it, and this bridge folds
// nothing for the rest of the install's life — with no signal to the user.
// "The binary exists at install time" is not the same question as "it exists
// now". Fall back to the bare name on PATH once, and remember what worked.
let resolvedBin = null;
// One tool call = at least one API call that re-reads everything tokenade put
// in this conversation. The host runs no hook process of ours per call, so the
// bridge tells the binary (`session-tick`), detached: the tool never waits.
// `reset`: the conversation was compacted or replaced (`/new`) — what tokenade
// folded out of it or put into it is no longer re-read, on either side.
function tickSession(session, agent, reset = false, transcript = undefined) {
  if (!session) return;
  try {
    const argv = ["session-tick", "--session", session, "--agent", agent];
    if (reset) argv.push("--reset");
    if (transcript) argv.push("--transcript", transcript);
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

// Substring, not exact match, and the difference is total rather than partial.
// Measured 2026-08-08 with the real binary behind the bridge: a tool named
// `execute_command` (which is what Cline calls its shell tool — this file's
// twin had already been patched for it, alone) produced NO hint, and without a
// hint `tokenade read -` leaves `git log` output untouched. So the result did
// not shrink, the fail-open rule declined to replace it, and the model got all
// 29 115 bytes. With the hint: 4 569. Not a degraded fold — no fold at all.
//
// Hermes' bridge has always matched by substring; the four JS/TS twins used a
// closed set of five names, and one of them had been quietly extended. Aligning
// on the broader test removes the drift and covers `run_command`,
// `shell_exec`, and whatever the next host calls it. A false positive is
// bounded: a non-shell tool has no `command` string in its input, so the hint
// comes back null anyway.
//
// The list stops at five ON PURPOSE. Hermes has a sixth, `terminal`, and that
// is not drift: on these four hosts `terminal` is a LIVE PTY session, skipped
// outright (`SKIP`), while hermes only ever sees finished results. Making the
// five lists identical would add a word whose only known bearer these bridges
// must not touch. Asserted, with the reason, in
// `the_bridge_assets_agree_on_the_constants_they_share`.
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
function commandHint(event) {
  if (!isShellTool(event?.toolName)) return null;
  const c =
    event?.input?.command ??
    event?.input?.cmd ??
    event?.args?.command ??
    event?.toolInput?.command ??
    event?.command;
  if (typeof c !== "string") return null;
  const t = c.trim();
  return t ? t.slice(0, 400) : null;
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
const PROCESS_SESSION = `p${process.pid}-pi`;
// pi's events carry no session id; its context does (`ctx.sessionManager`,
// `ReadonlySessionManager.getSessionId` / `getSessionFile`). Every handler
// keys by it, so ticks, folds and resets land on the same session.
function piKey(event, ctx) {
  let id;
  try {
    id = ctx?.sessionManager?.getSessionId?.();
  } catch {
    /* older pi */
  }
  return sessionKey(event?.sessionId, event?.sessionID, event?.taskId, id);
}

// The session's JSONL, where pi records every reply's usage and every
// compaction: the binary counts re-reads off it call by call.
function piFile(ctx) {
  try {
    return ctx?.sessionManager?.getSessionFile?.() || undefined;
  } catch {
    return undefined;
  }
}

function sessionKey(...candidates) {
  for (const c of candidates) {
    if (typeof c === "string" && c.trim()) return c;
  }
  return PROCESS_SESSION;
}

// DELEGATION tools and the key that carries their instructions. Broad because
// the name varies from one host to another; without a prompt key the handler
// does nothing, so a false positive costs nothing.
const DELEGATION_WORDS = ["task", "agent", "delegate", "subagent", "spawn"];
function isDelegationTool(name) {
  const n = String(name || "").toLowerCase();
  return DELEGATION_WORDS.some((w) => n.includes(w));
}
const PROMPT_KEYS = ["prompt", "message", "instructions", "task"];
function promptKey(args) {
  if (!args || typeof args !== "object") return null;
  for (const k of PROMPT_KEYS) {
    if (typeof args[k] === "string" && args[k].trim()) return k;
  }
  return null;
}

// ── The savings counter, in pi's footer.
//
// pi has no configured `statusLine` like Claude Code, Copilot CLI or Cursor
// CLI; it has something BETTER for a bridge: `ctx.ui.setStatus(key, text)`,
// documented as "Set status text in the footer/status bar. Pass undefined to
// clear." The footer renders each extension status on its own line
// (`footer.js`: `getExtensionStatuses()`, sorted by key), and pi-tui's
// `visibleWidth` explicitly strips "OSC hyperlinks" from the width computation
// — so an OSC-8 link survives and stays clickable; it is neither counted as
// visible characters nor spat out in clear.
//
// The text comes from `tokenade statusline`, the SAME command the three agents
// above call, with the SAME JSON on stdin. One rendering to maintain, one gate
// rule.
const STATUS_KEY = "tokenade";
const STATUS_MIN_INTERVAL_MS = 1500; // the footer redraws on every turn
let lastStatusAt = 0;
let lastSession = PROCESS_SESSION;

function refreshStatus(ctx, session, force) {
  try {
    const setStatus = ctx?.ui?.setStatus;
    if (typeof setStatus !== "function") return; // print/RPC mode, no footer
    if (session) lastSession = session;
    const now = Date.now();
    if (!force && now - lastStatusAt < STATUS_MIN_INTERVAL_MS) return;
    lastStatusAt = now;
    // `session_id` must be EXACTLY the key under which this session's folds
    // were credited (`TOKENADE_SESSION` below), otherwise the ticker reads an
    // empty session and shows zero while the bridge is folding.
    const payload = JSON.stringify({
      session_id: lastSession,
      model: { id: String(ctx?.model?.id ?? ctx?.model?.name ?? "") },
    });
    const r = runTokenade(["statusline"], {
      input: payload,
      encoding: "utf8",
      timeout: 3000,
      maxBuffer: 256 * 1024,
      env: { ...process.env, TOKENADE_AGENT: "pi", TOKENADE_SESSION: lastSession },
    });
    // Zero savings (or gate closed) → `statusline` writes nothing: clear it
    // instead of leaving a stale line in the footer.
    const line = r.status === 0 && typeof r.stdout === "string" ? r.stdout.trim() : "";
    setStatus.call(ctx.ui, STATUS_KEY, line.length > 0 ? line : undefined);
  } catch {
    // fail-open: a footer is an ornament, never an outage
  }
}

export default function (pi) {
  // At startup, then at the end of each turn: that is when the session total
  // has just changed. `agent_settled` covers interrupted turns.
  pi.on("session_start", (event, ctx) => {
    const session = piKey(event, ctx);
    // `new` / `resume` / `fork` swap the conversation under the same process
    // key: the context tokenade counted as re-read is gone.
    if (event?.reason === "new" || event?.reason === "resume" || event?.reason === "fork") {
      tickSession(session, "pi", true);
    }
    refreshStatus(ctx, session, true);
  });
  // `session_compact` (`SessionCompactEvent`, pi-coding-agent's types).
  pi.on("session_compact", (event, ctx) => {
    tickSession(piKey(event, ctx), "pi", true);
  });
  pi.on("turn_end", (event, ctx) => {
    refreshStatus(ctx, piKey(event, ctx), false);
  });
  pi.on("agent_settled", (event, ctx) => {
    refreshStatus(ctx, piKey(event, ctx), true);
  });

  // A subagent inherits the FOLDING because it runs on the same host; it does
  // not inherit the INSTRUCTIONS, which live in the delegation prompt. pi
  // documents `tool_call` as "fired before the tool executes" with
  // "`event.input` is mutable. Mutate it in place to patch tool arguments"
  // (docs/extensions.md of the shipped package): it is the only place where
  // that prompt can still be modified.
  //
  // The text is NOT copied here: `tokenade subagent-brief` builds it, checks
  // the gate and counts the overhead exactly once.
  pi.on("tool_call", (event, ctx) => {
    tickSession(piKey(event, ctx), "pi", false, piFile(ctx));
    try {
      if (!isDelegationTool(event?.toolName ?? event?.name)) return;
      const args = event?.input ?? event?.args;
      const key = promptKey(args);
      if (!key) return;
      const original = args[key];
      const r = runTokenade(
        ["subagent-brief", "--type", String(args?.subagent_type ?? args?.agent ?? "")],
        {
          input: original,
          encoding: "utf8",
          timeout: TIMEOUT_MS,
          maxBuffer: 4 * 1024 * 1024,
          env: {
            ...process.env,
            TOKENADE_AGENT: "pi",
            TOKENADE_SESSION: piKey(event, ctx),
          },
        },
      );
      if (r.status === 0 && typeof r.stdout === "string" && r.stdout.length > original.length) {
        args[key] = r.stdout;
      }
    } catch {
      // fail-open: never block a delegation over a prefix
    }
  });

  // The HISTORY, not just the current turn. The result seam folds what just
  // arrived; the conversation, however, is re-sent IN FULL on every request,
  // and that is where the context grows. pi documents
  // `before_provider_request` as "fired after the provider-specific payload
  // is built, right before the request is sent … returning any other value
  // replaces the payload for the actual request": that is rank 4, in-agent,
  // with no port and no service.
  //
  // Never claim a saving twice: `authority::may_fold` looks at the CONTENT, so
  // a block already folded by the result seam carries its marker and is
  // refused here. The two seams coexist without counting the same load twice.
  pi.on("before_provider_request", (event, ctx) => {
    try {
      const payload = event?.payload;
      if (!payload || typeof payload !== "object") return undefined;
      const raw = JSON.stringify(payload);
      if (raw.length < MIN_CHARS || raw.length > MAX_CHARS) return undefined;
      const r = runTokenade(["fold-payload"], {
        input: raw,
        encoding: "utf8",
        timeout: TIMEOUT_MS,
        maxBuffer: 64 * 1024 * 1024,
        env: { ...process.env, TOKENADE_AGENT: "pi" },
      });
      if (r.status !== 0 || typeof r.stdout !== "string") return undefined;
      if (r.stdout.length >= raw.length) return undefined;
      const next = JSON.parse(r.stdout);
      return next;
    } catch {
      // fail-open: never break a request for the sake of a fold
      return undefined;
    }
  });

  pi.on("tool_result", async (event, ctx) => {
    if (!event || event.isError) return;
    if (SKIP.has(String(event.toolName || "").toLowerCase())) return;

    const blocks = Array.isArray(event.content) ? event.content : [];
    // Only fold a pure-text result; leave images / mixed content alone.
    if (
      blocks.length === 0 ||
      blocks.some((b) => !b || b.type !== "text" || typeof b.text !== "string")
    ) {
      return;
    }
    const text = blocks.map((b) => b.text).join("");
    if (text.length > MAX_CHARS) return;
    // The floor below is about FOLDING and about scanning for secrets, and a
    // document read is neither: the words are in the FILE, not in what the host
    // handed back. A host that decoded nothing must not close the door on the
    // extractor; everything else still stops here.
    const docPath = isReadTool(event?.toolName) ? readTargetPath(event?.args ?? event?.input) : null;
    if (text.length < REDACT_MIN_CHARS && !docPath) return;
    // A base64/data-URI blob won't fold and could be mangled by text compaction.
    if (text.startsWith("data:") || text.charCodeAt(0) === 0) return;

    try {
      const session = piKey(event, ctx);
      const rp = docPath;
      if (rp) {
        const d = runTokenade(["read", rp], {
          encoding: "utf8",
          timeout: TIMEOUT_MS,
          maxBuffer: 64 * 1024 * 1024,
          env: { ...process.env, TOKENADE_AGENT: "pi", TOKENADE_SESSION: session },
        });
        // Not a fold, so not the fold's rule: with an empty host result "is it
        // smaller?" is false for every extraction, and the words are dropped.
        if (d.status === 0 && d.stdout && (text.length === 0 || d.stdout.length < text.length || scrubbed(d))) {
          return { content: [{ type: "text", text: d.stdout }] };
        }
      }
      const hint = commandHint(event);
      const argv = hint ? ["read", "-", "--cmd", hint] : ["read", "-"];
      const r = runTokenade(argv, {
        input: text,
        encoding: "utf8",
        timeout: TIMEOUT_MS,
        maxBuffer: 64 * 1024 * 1024,
        env: {
          ...process.env,
          TOKENADE_AGENT: "pi",
          TOKENADE_SESSION: session,
        },
      });
      const out = r.stdout;
      // Fail-open: only replace when tokenade actually shrank it.
      if (r.status === 0 && out && (out.length < text.length || scrubbed(r))) {
        return { content: [{ type: "text", text: out }] };
      }
    } catch {
      // any failure -> return nothing, Pi keeps the original result
    }
    return;
  });
}
