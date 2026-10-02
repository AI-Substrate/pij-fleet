// pij-control-plane — shared tmux send-keys / paste / capture primitives.
//
// The "keys" layer of the tmux seam, operating on a raw pane TARGET STRING (a
// canonical "%N" pane id, or a "session:win.pane" address). Distinct from
// adapters/tmux.ts (TmuxPort: window/pane LIFECYCLE — newWindow/split/kill).
//
// Extracted from harness/driver/tmux.ts so ONE argv-only implementation serves
// both the pij control plane (which holds raw paneId strings from split -P) and
// the harness Driver (which holds Target objects). harness/driver/tmux.ts now
// re-delegates its type/press/paste/capture here (parity — finding 02).
//
// Discipline: argv arrays ONLY — never shell strings (TC-01 / AC-09). The tmux
// invocation is injectable (TmuxRunner) so tests assert exact argv against a
// fake without spawning tmux.

import { type ExecFileSyncOptions, execFileSync } from "node:child_process";

// ─── Runner seam ─────────────────────────────────────────────────────────────

/** Runs one tmux command: takes argv (never a shell string), returns stdout.
 *  Injectable — tests pass a recorder; production passes {@link execFileRunner}. */
export type TmuxRunner = (args: string[]) => string;

const EXEC_OPTS: ExecFileSyncOptions = {
	encoding: "utf8",
	stdio: ["ignore", "pipe", "pipe"],
};

/** Default runner: real `tmux` via execFileSync (argv-only — no shell). */
export const execFileRunner: TmuxRunner = (args) => {
	const out = execFileSync("tmux", args, EXEC_OPTS);
	return typeof out === "string" ? out : out.toString("utf8");
};

// ─── Keys ────────────────────────────────────────────────────────────────────

/** Named keys send-keys understands (mirrors harness/driver/tmux.ts `Key`). */
export type TmuxKey =
	| "Enter"
	// Digit answer keys — send-keys types the literal character (interstitial
	// auto-answer, e.g. copilot folder-trust option 1 = trust once).
	| "1"
	| "2"
	| "C-c"
	| "C-d"
	| "C-z"
	| "C-l"
	| "C-u"
	| "Escape"
	| "Tab"
	| "BSpace"
	| "BTab"
	| "Space"
	| "Up"
	| "Down"
	| "Left"
	| "Right"
	| "Home"
	| "End"
	| "PageUp"
	| "PageDown"
	| "F1"
	| "F2"
	| "F3"
	| "F4"
	| "F5"
	| "F6"
	| "F7"
	| "F8"
	| "F9"
	| "F10"
	| "F11"
	| "F12";

export interface CaptureOpts {
	/** -S -<N>: lines of scrollback to include; default 0 (visible only). */
	scrollback?: number;
	/** -J join wrapped lines; default true (TC-04). */
	join?: boolean;
	/** -e include ANSI escape sequences; default false. */
	ansi?: boolean;
}

export interface PasteOpts {
	/** -p bracketed paste — keeps multi-line bodies intact (R-04). */
	bracketed?: boolean;
	/** Override the generated buffer name (tests assert a stable argv). */
	bufferName?: string;
}

// ─── Primitives ──────────────────────────────────────────────────────────────

/** `send-keys -t <target> -l <text>` — literal mode, no key-name lookup (TC-02).
 *  Use {@link pressKey} to submit (Enter); type + press are always separate. */
export function typeLiteral(target: string, text: string, run: TmuxRunner = execFileRunner): void {
	run(["send-keys", "-t", target, "-l", text]);
}

/** `send-keys -t <target> -H 1b 5b 49` — inject a raw FOCUS-IN escape (CSI I,
 *  `ESC [ I`) into the pane. With tmux `focus-events on`, switching away from a
 *  copilot pane sends it a focus-OUT (CSI O) and copilot then IGNORES Enter-as-
 *  submit — a peer message types into the composer but the Return is swallowed
 *  and the text strands (operator-reported "stuck in the input box"; a SIGWINCH
 *  redraw does NOT clear it, because copilot gates submit on FOCUS state, not
 *  render). Injecting focus-IN flips copilot back to focused-input mode so the
 *  next Enter submits. Verified against a live backgrounded copilot: focus-OUT →
 *  stuck; focus-IN → submits. Argv-only (`-H` = hex bytes). */
export function sendFocusIn(target: string, run: TmuxRunner = execFileRunner): void {
	run(["send-keys", "-t", target, "-H", "1b", "5b", "49"]);
}

/** `send-keys -t <target> [-N n] <key>` — a named key, optionally repeated. */
export function pressKey(
	target: string,
	key: TmuxKey,
	n = 1,
	run: TmuxRunner = execFileRunner,
): void {
	const args = ["send-keys", "-t", target];
	if (n > 1) args.push("-N", String(n));
	args.push(key);
	run(args);
}

/** `set-buffer` + `paste-buffer` — safe for any payload (TC-03). Pass
 *  `bracketed` for multi-line bodies so the receiver treats it as one paste. */
export function pasteBuffer(
	target: string,
	data: string,
	opts: PasteOpts = {},
	run: TmuxRunner = execFileRunner,
): void {
	const buf = opts.bufferName ?? `pij-${process.pid}-${Date.now()}`;
	run(["set-buffer", "-b", buf, data]);
	const args = ["paste-buffer", "-d", "-b", buf, "-t", target];
	if (opts.bracketed) args.splice(1, 0, "-p");
	run(args);
}

/** `capture-pane -t <target> -p` — the pane's text (TC-04). Joins wrapped
 *  lines by default; pass `scrollback` for history, `ansi` to keep escapes. */
export function capturePane(
	target: string,
	opts: CaptureOpts = {},
	run: TmuxRunner = execFileRunner,
): string {
	const args = ["capture-pane", "-t", target, "-p"];
	if (opts.join !== false) args.push("-J");
	if (opts.ansi) args.push("-e");
	if (opts.scrollback && opts.scrollback > 0) {
		args.push("-S", `-${opts.scrollback}`, "-E", "-");
	}
	return run(args);
}
