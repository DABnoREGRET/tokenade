// tokenade plugin for Cline — https://tokenade.net
//
// Folds large tool results (logs, command output, file dumps) in the
// provider-bound message list before each model call, so Cline pays for the
// compacted form instead of the raw bytes. Uses Cline's SDK plugin seam
// `api.registerMessageBuilder` (capability "messageBuilders").
//
// CACHE-SAFE: build() runs every turn on the FULL history, but compaction is
// a DETERMINISTIC pure function of a block's bytes (same input -> same output)
// and memoized, so an already-folded block yields identical bytes on every
// later turn — the prompt cache holds. Only the turn a block first appears
// (already uncached) changes.
//
// SAFE-BY-DEFAULT: error results are kept verbatim (never fold an error line),
// small/binary blocks are skipped, and `tokenade read` itself only folds what
// it can fold losslessly (repetitive logs, structured dumps) and passes prose
// and code through untouched.
import type { AgentPlugin, Message } from "@cline/core";
import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";

// Absolute tokenade binary path, injected at install (env wins at runtime).
// `@@TOKENADE_BIN@@` is replaced with the absolute binary path by the
// installer. The env var still wins; if injection was skipped, the four other
// bridges fall back to the bare name on PATH and this one did NOT — it spawned
// the literal placeholder, got ENOENT, and folded nothing for ever, silently.
function tokenadeBin(): string {
	const injected = "@@TOKENADE_BIN@@";
	return process.env.TOKENADE_BIN || (injected.startsWith("@@") ? "tokenade" : injected);
}

// An injected ABSOLUTE path goes stale the moment tokenade is moved, upgraded
// into a different prefix, or uninstalled and reinstalled. Every spawn then
// fails with ENOENT, the catch below swallows it, and this bridge folds
// nothing for the rest of the install's life — with no signal to the user.
// "The binary exists at install time" is not the same question as "it exists
// now". Fall back to the bare name on PATH once, and remember what worked.
let resolvedBin: string | null = null;
// Messages each conversation sent on its last request.
const historyLength = new Map<string, number>();
// The conversation the last stored-message pass named.
let lastConversation: string | null = null;
// One tool call = at least one API call that re-reads everything tokenade put
// in this conversation. The host runs no hook process of ours per call, so the
// bridge tells the binary (`session-tick`), detached: the tool never waits.
// `reset`: the conversation was compacted — what tokenade folded out of it or
// put into it is no longer re-read, on either side of the ledger.
function tickSession(session: string, agent: string, reset = false) {
  if (!session) return;
  try {
    // `--per-call`: this bridge ticks once per API request, not per tool call.
    const argv = ["session-tick", "--session", session, "--agent", agent, "--per-call"];
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

function runTokenade(argv: string[], opts: any) {
	const first = resolvedBin || tokenadeBin();
	let r = spawnSync(first, argv, opts);
	if (r.error && (r.error as any).code === "ENOENT" && first !== "tokenade") {
		r = spawnSync("tokenade", argv, opts);
		if (!r.error) resolvedBin = "tokenade";
	} else if (!r.error) {
		resolvedBin = first;
	}
	return r;
}
// 1000, like the four sibling bridges. This said 2000, with the comment
// "below this, folding can't beat its own overhead" — measured 2026-08-08 with
// the real binary, that is false at 1000 chars:
//
//   log-shaped   1000 -> 165 bytes    ls-shaped   1000 -> 620 bytes
//   log-shaped   1500 -> 167 bytes    ls-shaped   1500 -> 756 bytes
//
// A 6x fold is not overhead-bound. Cline was silently dropping every tool
// result between 1000 and 2000 chars that its four siblings fold. And nothing
// below can inflate: the fail-open rule only substitutes a result that
// actually shrank.
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
const MAX_CHARS = 4_000_000; // hard safety cap on a single block

// full-content hash -> folded output. Determinism + memoization make
// build() idempotent across turns (cache-safe) and cheap (no re-spawn).
const memo = new Map<string, string>();

// The command behind a tool result, when the history carries it.
//
// `tokenade read -` detects by CONTENT, and content cannot say which program
// produced it: measured 2026-08-07, `git log --oneline` came back unchanged
// without this and folded to a quarter with it. Unlike the other bridges,
// Cline sees the WHOLE history, so the command is not in the block being
// folded — it is in the matching `tool_use` block of an earlier assistant
// message, linked by `tool_use_id`. Restricted to shell-ish tools: a
// `read`/`grep` tool's "command" would send the dispatcher to the wrong
// compactor.
// tokenade prints this on stderr when its output has SECRETS REMOVED. That
// output is often LONGER than what we sent (`API_KEY=k0` becomes
// `API_KEY=<redacted>`), so the "strictly shorter" rule below would throw the
// scrubbed version away and hand the model the raw key. Measured 2026-08-08:
// three ~2 kB payloads well above MIN_CHARS each grew about a tenth once
// scrubbed, and all of them were discarded. Length answers "did it fold"; it
// cannot answer "was a secret removed", and only tokenade knows that.
const REDACTED_SIGNAL = "[tokenade] redacted:";
function scrubbed(r: { stderr?: unknown }): boolean {
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
/// The file each READ tool call named, keyed by its tool_use_id — the twin of
/// `commandsByToolUseId`, one tool family over. Cline's messageBuilder only
/// ever sees the RESULT, so without this map the document path has no path.
function readPathsByToolUseId(messages: any[]): Map<string, string> {
	const out = new Map<string, string>();
	for (const m of messages) {
		if (!Array.isArray(m?.content)) continue;
		for (const b of m.content) {
			if (!b || b.type !== "tool_use" || typeof b.id !== "string") continue;
			if (!isReadTool(b.name)) continue;
			const p = readTargetPath(b.input);
			if (p) out.set(b.id, p);
		}
	}
	return out;
}

function commandsByToolUseId(messages: any[]): Map<string, string> {
	const out = new Map<string, string>();
	for (const m of messages) {
		if (!Array.isArray(m?.content)) continue;
		for (const b of m.content) {
			if (!b || b.type !== "tool_use" || typeof b.id !== "string") continue;
			if (!isShellTool(b.name)) continue;
			const c = b.input?.command ?? b.input?.cmd;
			if (typeof c === "string" && c.trim()) out.set(b.id, c.trim().slice(0, 400));
		}
	}
	return out;
}


// ── Session identity, so a re-delivery can be folded ──────────────────
//
// `tokenade read` folds a repeated delivery to a §ref pointer only when it is
// told which session provably received those bytes; with no session it never
// folds, by design — it cannot claim "you already have this" about a context
// it knows nothing about. Nothing on the bridge path ever supplied one, so
// every bridge agent went without read-dedup.
//
// Cline is the host where this changes the LEAST, and the reason is the memo
// right below. `build()` re-runs on the full history every turn, so stability
// is the requirement: two blocks with identical bytes must fold identically or
// the prompt cache breaks. The memo enforces that by never asking the binary
// twice for the same bytes — which is also why the binary never sees a repeat
// here and the §ref path stays cold. The session still travels, for two
// reasons: the ledger rows get attributed, and the memo key carries it, so a
// host process serving two tasks can never hand task B a pointer into task A's
// context.
//
// The pid alone is NOT the fallback, and the bridge e2e proved why: it drives
// all four JS bridges in ONE node process, they shared a key, and the second
// bridge to see a payload folded it to a pointer into the FIRST one's context.
// Production spawns one agent per process, so this was a harness artefact —
// but a key that can name two contexts is wrong wherever it happens.
const PROCESS_SESSION = `p${process.pid}-cline`;
function sessionKey(...candidates: unknown[]): string {
	for (const c of candidates) {
		if (typeof c === "string" && c.trim()) return c;
	}
	return PROCESS_SESSION;
}

function fold(text: string, cmd?: string, session?: string): string {
	if (text.length < REDACT_MIN_CHARS || text.length > MAX_CHARS) return text;
	if (text.startsWith("data:")) return text; // data: URI / embedded binary
	if (text.includes("\u0000")) return text; // embedded NUL => binary
	// The hint is part of the key: the same bytes fold differently depending
	// on which command produced them, and a shared key would serve one
	// command's fold for another's output.
	//
	// The rest of the key is a HASH of the whole text, not a sample of it. It
	// used to be `length:first96:last96`, which cannot answer "are these the
	// same bytes" — only "do they look alike at the ends". Demonstrated
	// 2026-08-08 with two 2 140-byte `kubectl get pods` snapshots differing in
	// one middle STATUS field (`Running` vs `Failed `): identical length,
	// identical first and last 96 chars, identical key. Cline would have served
	// the FIRST snapshot's fold for the second, so the model reads `Running`
	// while the truth is `Failed` — silently wrong content, which is worse than
	// not folding at all. Fixed-width middle fields (statuses, counters,
	// timestamps) are the commonest shape in tool output, so this is not a
	// corner case.
	const skey = sessionKey(session);
	const key = `${skey}\u0000${cmd ?? ""}\u0000${createHash("sha1").update(text).digest("hex")}`;
	const hit = memo.get(key);
	if (hit !== undefined) return hit;
	let out = text;
	try {
		const r = runTokenade(cmd ? ["read", "-", "--cmd", cmd] : ["read", "-"], {
			input: text,
			encoding: "utf8",
			maxBuffer: 64 * 1024 * 1024,
			timeout: 10_000,
			env: {
				...process.env,
				TOKENADE_AGENT: "cline",
				TOKENADE_WRAPPING: "1",
				TOKENADE_SESSION: skey,
			},
		});
		// Only accept a STRICTLY shorter, non-empty result; else keep raw.
		if (r.status === 0 && r.stdout && r.stdout.length > 0 && (r.stdout.length < text.length || scrubbed(r))) {
			out = r.stdout;
		}
	} catch {
		out = text; // fail-open: never break Cline on a folding error
	}
	memo.set(key, out);
	return out;
}

// Fold a tool_result / file block's content in place, structurally.
// The document path. Cline's messageBuilder sees the tool RESULT, and a
// `.docx` has already lost its words by then; `tokenade read <path>` opens the
// file. Memoised through the same `fold` cache by keying on the path, so the
// full-history rebuild stays byte-stable turn after turn.
function foldDocument(rp: string, text: string, session?: string): string | null {
	const skey = sessionKey(session);
	const key = `doc\u0000${skey}\u0000${rp}\u0000${text.length}`;
	const hit = memo.get(key);
	if (hit !== undefined) return hit === text ? null : hit;
	try {
		const r = runTokenade(["read", rp], {
			encoding: "utf8",
			maxBuffer: 64 * 1024 * 1024,
			timeout: 10_000,
			env: {
				...process.env,
				TOKENADE_AGENT: "cline",
				TOKENADE_WRAPPING: "1",
				TOKENADE_SESSION: skey,
			},
		});
		// Not a fold, so not the fold's rule: with an empty host result "is it
		// smaller?" is false for every extraction, and the words are dropped.
		if (r.status === 0 && r.stdout && (text.length === 0 || r.stdout.length < text.length || scrubbed(r))) {
			memo.set(key, r.stdout);
			return r.stdout;
		}
	} catch {
		// fail-open: never break Cline on a folding error
	}
	memo.set(key, text);
	return null;
}

function foldContent(content: unknown, cmd?: string, session?: string, rp?: string | null): unknown {
	if (typeof content === "string") {
		const doc = rp ? foldDocument(rp, content, session) : null;
		return doc ?? fold(content, cmd, session);
	}
	if (Array.isArray(content)) {
		let changed = false;
		const next = content.map((b: any) => {
			if (b && b.type === "text" && typeof b.text === "string") {
				const doc = rp ? foldDocument(rp, b.text, session) : null;
				const t = doc ?? fold(b.text, cmd, session);
				if (t !== b.text) {
					changed = true;
					return { ...b, text: t };
				}
			}
			return b;
		});
		return changed ? next : content;
	}
	return content;
}

// ── The response-shape preamble ──────────────────────────────────────
//
// The preamble had exactly one way out of the binary: a Claude-shaped
// `UserPromptSubmit` hook. Cline has no hook at all — it has a message
// builder, which is a BETTER seam for this and was simply never used, so the
// compat bench reported "no prompt-time hook exists for this agent".
// `tokenade style --preamble` is the ask-for-it form; the binary still owns
// the resolution (auto-classification, `off`, per-project setting).
//
// Injected on the FIRST user message, not the last, and that is a cache
// decision rather than a stylistic one: `build()` re-runs over the whole
// history every turn, so anything appended to the LATEST message rewrites a
// block the prompt cache had already accepted. The first message never moves.
let preambleMemo: string | null | undefined;
function stylePreamble(firstPrompt: string): string | null {
	if (preambleMemo !== undefined) return preambleMemo;
	preambleMemo = null;
	try {
		const r = runTokenade(["style", "--preamble", "--prompt", firstPrompt.slice(0, 400)], {
			encoding: "utf8",
			timeout: 10_000,
			maxBuffer: 1024 * 1024,
			env: { ...process.env, TOKENADE_AGENT: "cline" },
		});
		const out = typeof r.stdout === "string" ? r.stdout.trim() : "";
		if (r.status === 0 && out) preambleMemo = out;
	} catch {
		// fail-open: never break Cline over a preamble
	}
	return preambleMemo;
}

/// The preamble is in every request of a session (one copy, rebuilt each
/// time): charged once per session as injected context, then re-read per call
/// by the session's ticks.
const bookedSessions = new Set<string>();
function bookPreambleOnce(session: string, firstPrompt: string) {
	if (!session || bookedSessions.has(session) || !preambleMemo) return;
	bookedSessions.add(session);
	try {
		const c = spawn(resolvedBin || tokenadeBin(), ["style", "--preamble", "--book", "--session", session, "--prompt", firstPrompt.slice(0, 400)], {
			stdio: "ignore",
			detached: true,
      // Windows: a detached child gets a console window of its own.
      windowsHide: true,
			env: { ...process.env, TOKENADE_AGENT: "cline" },
		});
		c.on("error", () => {});
		c.unref();
	} catch {
		/* fail-open */
	}
}

/// Text of the first user message, for the auto-classifier.
function firstUserText(messages: any[]): string {
	for (const m of messages) {
		if (m?.role !== "user") continue;
		if (typeof m.content === "string") return m.content;
		if (Array.isArray(m.content)) {
			const t = m.content.find((b: any) => b?.type === "text" && typeof b.text === "string");
			if (t) return t.text;
		}
	}
	return "";
}

/// Put the preamble in front of the first user message, once.
function withPreamble(messages: any[]): any[] {
	const text = stylePreamble(firstUserText(messages));
	if (!text) return messages;
	const i = messages.findIndex((m: any) => m?.role === "user");
	if (i === -1) return messages;
	const m: any = messages[i];
	const already = (blocks: any[]) =>
		blocks.some((b: any) => typeof b?.text === "string" && b.text.startsWith("[tokenade:style="));
	if (typeof m.content === "string") {
		if (m.content.startsWith("[tokenade:style=")) return messages;
		const next = messages.slice();
		next[i] = { ...m, content: `${text}\n\n${m.content}` };
		return next;
	}
	if (Array.isArray(m.content)) {
		if (already(m.content)) return messages;
		const next = messages.slice();
		next[i] = { ...m, content: [{ type: "text", text }, ...m.content] };
		return next;
	}
	return messages;
}

export const plugin: AgentPlugin = {
	name: "tokenade",
	manifest: { capabilities: ["messageBuilders"] },
	setup(api) {
		api.registerMessageBuilder({
			name: "tokenade-fold-tool-results",
			build(messages: Message[]): Message[] {
				messages = withPreamble(messages as any[]) as Message[];
				const cmds = commandsByToolUseId(messages as any[]);
				const readPaths = readPathsByToolUseId(messages as any[]);
				// The conversation's own identity where the host gives one, so a
				// fold never claims across tasks; the process otherwise.
				const first: any = (messages as any[])[0];
				// Cline runs the builder TWICE per request (observed on cline 3.0.71):
				// on the stored messages (`id`, `ts`…), then on their bare
				// provider form (`role`, `content`). Only the first names the
				// conversation, and only one request goes out: the bare pass takes
				// the conversation's key and counts no call — it used to tick a
				// phantom process session, with its own rules-file charge.
				const own = sessionKey(first?.taskId, first?.conversationId, first?.id, first?.ts && String(first.ts));
				const bare = own === PROCESS_SESSION && lastConversation !== null;
				const session = bare ? (lastConversation as string) : own;
				if (own !== PROCESS_SESSION) lastConversation = own;
				// The history only grows between two requests; shorter, it was
				// compacted. The builder sees it whole, so no event is needed.
				const seen = historyLength.get(session) ?? 0;
				historyLength.set(session, messages.length);
				// The stored pass runs once per request: Cline's exact call clock.
				// One detached call either way — two could run out of order.
				if (!bare) {
					tickSession(session, "cline", messages.length < seen);
					bookPreambleOnce(session, firstUserText(messages as any[]));
				}
				return messages.map((m: any) => {
					if (!Array.isArray(m.content)) return m; // plain-string messages untouched
					let changed = false;
					const blocks = m.content.map((b: any) => {
						if (!b || typeof b !== "object") return b;
						if (b.type === "tool_result") {
							if (b.is_error || b.isError) return b; // keep error output verbatim
							const id = b.tool_use_id ?? b.toolUseId;
							const nc = foldContent(b.content, cmds.get(id), session, readPaths.get(id));
							if (nc !== b.content) {
								changed = true;
								return { ...b, content: nc };
							}
						} else if (b.type === "file" && typeof b.content === "string") {
							const nc = fold(b.content, undefined, session);
							if (nc !== b.content) {
								changed = true;
								return { ...b, content: nc };
							}
						}
						return b;
					});
					return changed ? { ...m, content: blocks } : m;
				});
			},
		});
	},
};

export default plugin;
