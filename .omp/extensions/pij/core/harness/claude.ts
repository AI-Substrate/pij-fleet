// pij-control-plane — Claude Code harness transport (pure, Plan 019).
//
// Three pure pieces of the Claude seam:
//   1. mangleCwd / transcriptDir — where Claude writes a session's JSONL.
//   2. discoverNewTranscript — deterministic identity binding by NEW PATH
//      APPEARANCE (a jsonl absent at spawn), never by mtime (AC-03, finding
//      03/05): a pre-existing active session in the same cwd is never chosen.
//   3. buildInitInjection — the exactly-once init text the daemon types into a
//      freshly-ready pane (pij-id + the confirmatory `pij phonehome` line, AC-02).
//
// All impure parts (reading the dir, send-keys, timers) live in the daemon.

import type { SessionId } from "../types.js";

/** Claude Code derives a session's transcript directory from the project cwd by
 *  replacing every non-alphanumeric character with `-`:
 *    /Users/jo/pi-hacking/pij  →  -Users-jo-pi-hacking-pij
 *  Verified against the live `~/.claude/projects` tree (telemetry parity,
 *  finding 07) — the binding + `pij tail` both resolve through this mangle. */
export function mangleCwd(cwd: string): string {
	return cwd.replace(/[^a-zA-Z0-9]/g, "-");
}

/** Absolute transcript directory for a cwd: `~/.claude/projects/<mangled cwd>/`. */
export function transcriptDir(home: string, cwd: string): string {
	return `${home}/.claude/projects/${mangleCwd(cwd)}`;
}

/** The harnessSessionId is the transcript file stem (basename without `.jsonl`). */
export function transcriptSessionId(path: string): string {
	const base = path.slice(path.lastIndexOf("/") + 1);
	return base.endsWith(".jsonl") ? base.slice(0, -".jsonl".length) : base;
}

/** Absolute transcript file for a BOUND claude session (`pij tail`, AC-09):
 *  `~/.claude/projects/<mangled cwd>/<harnessSessionId>.jsonl`. */
export function transcriptPathFor(home: string, cwd: string, harnessSessionId: string): string {
	return `${transcriptDir(home, cwd)}/${harnessSessionId}.jsonl`;
}

/** One human-readable line distilled from a raw transcript JSONL line, or `null`
 *  for lines that carry no conversational signal (mode/system/snapshot/etc).
 *  Used by `pij tail` to show what a bound claude session is doing (AC-09). */
export interface TranscriptEntry {
	readonly role: "user" | "assistant";
	readonly text: string;
}

export function summarizeTranscriptLine(raw: string): TranscriptEntry | null {
	let d: { type?: string; message?: { content?: unknown } };
	try {
		d = JSON.parse(raw);
	} catch {
		return null;
	}
	const content = d.message?.content;
	if (d.type === "user") {
		const text = textOf(content);
		return text ? { role: "user", text } : null;
	}
	if (d.type === "assistant") {
		const text = textOf(content);
		return text ? { role: "assistant", text } : null;
	}
	return null;
}

/** Flatten a transcript `content` (string, or an array of text / tool_use /
 *  tool_result blocks) into a one-line summary; tool calls show as `⚙ <name>`. */
function textOf(content: unknown): string {
	if (typeof content === "string") return content.trim();
	if (!Array.isArray(content)) return "";
	const parts: string[] = [];
	for (const c of content) {
		if (!c || typeof c !== "object") continue;
		const block = c as { type?: string; text?: string; name?: string };
		if (block.type === "text" && block.text) parts.push(block.text);
		else if (block.type === "tool_use" && block.name) parts.push(`⚙ ${block.name}`);
		else if (block.type === "tool_result") parts.push("⚙ ↩ result");
	}
	return parts.join(" ").replace(/\s+/g, " ").trim();
}

/** Outcome of one discovery poll over the transcript dir. */
export type TranscriptDiscovery =
	| { readonly status: "found"; readonly path: string; readonly sessionId: string }
	| { readonly status: "pending" } // nothing new yet — poll again
	| { readonly status: "ambiguous"; readonly paths: readonly string[] }; // concurrent boots

/** Discover the binding transcript by NEW PATH APPEARANCE, never by mtime.
 *
 *  `before` = the `*.jsonl` paths present in the dir at spawn time;
 *  `after`  = the paths present now. The session's transcript is the path that
 *  did NOT exist at spawn — so a pre-existing active session in the same cwd
 *  (whose mtime advances as it works) is never chosen (AC-03, finding 03/05).
 *
 *  - exactly one new path → `found` (bind it)
 *  - none new            → `pending` (keep polling until ready / timeout)
 *  - more than one new    → `ambiguous` (concurrent boots — the daemon falls
 *                           back to the confirmatory `pij phonehome`). */
export function discoverNewTranscript(
	before: readonly string[],
	after: readonly string[],
): TranscriptDiscovery {
	const seen = new Set(before);
	const fresh = after.filter((p) => p.endsWith(".jsonl") && !seen.has(p));
	const [first] = fresh;
	if (fresh.length === 1 && first) {
		return { status: "found", path: first, sessionId: transcriptSessionId(first) };
	}
	if (fresh.length === 0) return { status: "pending" };
	return { status: "ambiguous", paths: fresh };
}

/** The exactly-once init injection for a freshly-ready Claude pane. Kept as
 *  data so the daemon owns the actual send-keys: `body` is typed once when the
 *  pane is ready (AC-02); `phonehomeLine` is the single confirmatory line the
 *  watchdog may re-send verbatim, alone, without touching `initInjectedAt`
 *  (AC-04). */
export interface InitInjection {
	readonly pijId: SessionId;
	readonly phonehomeLine: string;
	readonly body: string;
}

/** Build the init injection for a pre-allocated pij-id. When `branched` (this
 *  session was forked from another via `pij spawn --branch`, Plan 020 Finding 08),
 *  prepend a fork-reframe: the inherited transcript is context, NOT a task to
 *  resume — otherwise the fork acts on the parent's "continue where you left off"
 *  prompt and autonomously continues the parent's agenda (it once stood up a
 *  runaway peer). The reframe stops it cold and tells it new instructions follow. */
export function buildInitInjection(
	pijId: SessionId,
	branched = false,
	spawnedBy?: SessionId,
	revived = false,
): InitInjection {
	const phonehomeLine = "pij phonehome";
	const contextReframe = revived
		? "You are a REVIVED session. Your prior conversation is context only, NOT a task to resume. " +
			"Do NOT continue the old work, spawn peers, or message anyone; wait for new instructions. First, "
		: branched
			? "You are a FORK of another session — its prior conversation is yours for CONTEXT only, " +
				"NOT a task to resume. Do NOT continue the parent's previous work, and do NOT spawn or " +
				"message other sessions; new instructions are coming. First, "
			: "";
	// Name the pij instance that spawned this peer (control-plane peers boot
	// pij-blind), and give the concrete reply form so it can reach its spawner
	// without being told. Omitted for adopted/root peers (no known spawner).
	const spawnedByClause = spawnedBy ? `, spawned by ${spawnedBy}` : "";
	const replyHint = spawnedBy
		? ` (reply to your spawner with \`pij send ${spawnedBy} "<text>"\`)`
		: "";
	const body =
		`${contextReframe}You are now a pij peer (id: ${pijId})${spawnedByClause}. ` +
		`Message other sessions with \`pij send <id> "<text>"\`${replyHint} and list peers with \`pij list\`. ` +
		"Add `--fyi` to hold a message until their next turn instead of waking them, only when their next action doesn't depend on it; questions, blockers, hand-offs, review verdicts and work done are normal sends. " +
		// AC-12: pij's own onboarding used to teach ONLY the quoted form, to every
		// peer on the box — which is how a fleet that relays log lines and source
		// excerpts all day learned the one form that lets the caller's shell
		// execute them. The quoted form stays (it is right for text you author);
		// the safe form is now taught alongside it, where the peer reads it.
		"For RELAYED or UNTRUSTED text (a log line, a peer's report, a source excerpt) send it as a file " +
		"or on stdin instead: `pij send <id> --body-file <path|->` — a double-quoted body is expanded by " +
		"YOUR shell before pij ever runs, so backticks and $( ) in it execute. " +
		`To confirm your binding, run: ${phonehomeLine}`;
	return { pijId, phonehomeLine, body };
}
