// pij-control-plane — Codex CLI harness transcript module (pure, Plan 022).
//
// Codex is a CLAUDE-STYLE harness: its interactive TUI auto-generates a session
// UUID (no launch flag sets it — F-01), so the daemon binds it by transcript
// DISCOVERY, exactly like claude. The one genuinely new surface is codex's
// transcript LAYOUT, which differs from claude's in three ways this module owns:
//
//   1. Location — codex logs to `~/.codex/sessions/<YYYY>/<MM>/<DD>/rollout-
//      <ISO>-<uuid>.jsonl`: a date-nested + GLOBAL tree (not cwd-scoped like
//      claude's `~/.claude/projects/<mangled cwd>/`). So discovery must walk the
//      date tree (`listCodexRollouts`) and confirm cwd via the file's session_meta
//      (`codexCwdFromMeta`) since the dir mixes every cwd (F-04, R-2).
//   2. Session id — the bind id is the filename's TRAILING UUID
//      (`codexSessionIdFromPath`), NOT the claude-style stem (the stem is
//      `rollout-<ISO>-<uuid>`, whose ISO segment also carries hyphens) — Finding 06.
//   3. Line schema — `{timestamp,type,payload}` with `type` of session_meta /
//      event_msg / response_item, NOT claude's `{type:user/assistant,message}`
//      (F-05). `summarizeCodexEvent` renders the same `[role] text` / `⚙ tool`
//      shape as the claude/copilot summarizers so `pij tail` looks identical.
//
// All impure parts (reading dirs/files, send-keys, timers) live in the daemon/bin;
// `listCodexRollouts` takes an injected directory reader so it stays pure-testable.

import type { TranscriptEntry } from "./claude.js";

/** Absolute root of codex's session log tree: `~/.codex/sessions`. GLOBAL — every
 *  cwd's rollouts live under here (date-nested), unlike claude's per-cwd dir. */
export function codexTranscriptRoot(home: string): string {
	return `${home}/.codex/sessions`;
}

/** UUID-shaped suffix matcher: the canonical 8-4-4-4-12 hex groups. */
const TRAILING_UUID_RE =
	/([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\.jsonl$/;

/** The harnessSessionId for a codex rollout is the filename's TRAILING UUID
 *  (`rollout-<ISO>-<uuid>.jsonl` → `<uuid>`), confirmed identical to
 *  `session_meta.id` by the POC. NOT the claude-style stem: the stem is
 *  `rollout-<ISO>-<uuid>` and its ISO segment carries its own hyphens, so a stem
 *  bind would be wrong AND a bare UUID cannot reconstruct the date-nested path
 *  (Finding 06 — the path is persisted as `transcriptPath` for tail instead). */
export function codexSessionIdFromPath(path: string): string {
	const base = path.slice(path.lastIndexOf("/") + 1);
	const m = TRAILING_UUID_RE.exec(base);
	if (m?.[1]) return m[1];
	// Fallback for a non-canonical name: the stem (shouldn't happen for real rollouts).
	return base.endsWith(".jsonl") ? base.slice(0, -".jsonl".length) : base;
}

/** Resolve an authoritative Codex native id to its date-nested rollout path. */
export function codexRolloutForSession(
	rolloutPaths: readonly string[],
	harnessSessionId: string,
	isReadable: (path: string) => boolean = () => true,
): string | null {
	return (
		rolloutPaths.find(
			(path) => codexSessionIdFromPath(path) === harnessSessionId && isReadable(path),
		) ?? null
	);
}

/** Recursively collect rollout `*.jsonl` ABSOLUTE paths under `root` (codex's
 *  date-nested `YYYY/MM/DD` tree). `readDir(dir)` returns the entry NAMES in a
 *  dir (`[]` if unreadable — a file, a missing dir). Recurses into non-`.jsonl`
 *  entries (the date subdirs) and collects only `rollout-*.jsonl` files. Walking
 *  the WHOLE tree naturally spans today + yesterday + any date dir (R-4 midnight
 *  rollover). Pure — the impure readdir is injected so this is unit-testable; the
 *  daemon/bin pass a real (best-effort) readdir. `depth` bounds the walk. */
export function listCodexRollouts(
	readDir: (dir: string) => string[],
	root: string,
	depth = 5,
): string[] {
	if (depth <= 0) return [];
	const out: string[] = [];
	for (const name of readDir(root)) {
		if (name.startsWith(".")) continue; // skip dotfiles (.DS_Store etc.)
		if (name.endsWith(".jsonl")) {
			if (name.startsWith("rollout-")) out.push(`${root}/${name}`);
		} else {
			// A non-.jsonl entry is a date subdir (recurse) or a stray file (readDir → []).
			out.push(...listCodexRollouts(readDir, `${root}/${name}`, depth - 1));
		}
	}
	return out;
}

/** Extract `payload.cwd` from a codex rollout's `session_meta` line (line 1), or
 *  `null` if the line is not a session_meta / has no cwd / is malformed. The cwd
 *  lives INSIDE the file (the path is cwd-agnostic), so this is the tiebreak that
 *  confirms a discovered rollout belongs to OUR pane when the global dir holds
 *  concurrent codex sessions (R-2). */
export function codexCwdFromMeta(rawLine: string): string | null {
	let d: { type?: string; payload?: { cwd?: unknown } };
	try {
		d = JSON.parse(rawLine);
	} catch {
		return null;
	}
	if (d.type !== "session_meta") return null;
	const cwd = d.payload?.cwd;
	return typeof cwd === "string" && cwd ? cwd : null;
}

/** One human-readable line distilled from a raw codex rollout JSONL line, or
 *  `null` for lines that carry no conversational signal (session_meta, reasoning,
 *  token_count, task markers, developer/system injections). Mirrors
 *  `summarizeTranscriptLine`/`summarizeCopilotEvent` so `pij tail` renders codex
 *  identically (`[role] text`, tool calls as `⚙ name`).
 *
 *  Conversational text rides codex's `event_msg` stream — `user_message` /
 *  `agent_message` carry the clean turn text (deduped vs the raw `response_item`
 *  model I/O). Tool calls ride `response_item/function_call`. Everything else is
 *  dropped (notably `response_item/message` with role `developer`, the permissions
 *  /collaboration injection — codex's analogue of copilot's empty system inject). */
export function summarizeCodexEvent(raw: string): TranscriptEntry | null {
	let d: { type?: string; payload?: Record<string, unknown> };
	try {
		d = JSON.parse(raw);
	} catch {
		return null;
	}
	const p = d.payload ?? {};
	const pType = p.type;
	if (d.type === "event_msg") {
		if (pType === "user_message") {
			const text = typeof p.message === "string" ? p.message.trim() : "";
			return text ? { role: "user", text } : null;
		}
		if (pType === "agent_message") {
			const text = typeof p.message === "string" ? p.message.trim() : "";
			return text ? { role: "assistant", text } : null;
		}
		return null;
	}
	if (d.type === "response_item" && pType === "function_call") {
		const name = typeof p.name === "string" ? p.name : "";
		return name ? { role: "assistant", text: `⚙ ${name}` } : null;
	}
	return null;
}
