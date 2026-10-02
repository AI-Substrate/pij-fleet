// harness/driver/tmux.ts
//
// Tmux primitives. The ONLY file that calls child_process directly. All
// argv arrays — never shell strings. Encodes TC-01..TC-12 from the
// research dossier and closes D-014 (smoke shell-quoting).
//
// Public surface mirrors workshop 001 § Module: tmux.ts verbatim.

import { type ExecFileSyncOptions, execFileSync } from "node:child_process";

import {
	capturePane,
	pasteBuffer,
	pressKey,
	typeLiteral,
} from "../../.omp/extensions/pij/adapters/tmux-keys.js";
import { DriverBootError, DriverPaneDeadError } from "./errors.js";

// ─── Public types ──────────────────────────────────────────────────────────

export interface Target {
	session: string;
	window?: number; // default 0
	pane?: number; // default 0
	paneId?: string; // canonical "%N" id, captured at boot — rename-proof (TC-08)
}

export type Key =
	| "Enter"
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

export interface BootOpts {
	session: string;
	cwd: string;
	cmd: string; // direct command — no shell pipeline (workshop 001 D7)
	cols?: number; // default 200 (workshop 001 D5 / TC-09)
	rows?: number; // default 50
	env?: Record<string, string>;
}

export interface CaptureOpts {
	scrollback?: number; // -S -<N>; default 0 (visible only); pass 2000 for history
	join?: boolean; // -J join wrapped lines; default true (TC-04)
	ansi?: boolean; // -e include escape sequences; default false
}

export interface PaneInfo {
	paneId: string;
	pid: number;
	cmd: string;
	dead: boolean;
	cols: number;
	rows: number;
}

// ─── Internal primitive ─────────────────────────────────────────────────────

const TMUX_OPTS: ExecFileSyncOptions = {
	encoding: "utf8",
	stdio: ["ignore", "pipe", "pipe"],
};

const PIJ_IDENTITY_ENV_KEYS = [
	"CLAUDE_CODE_SESSION_ID",
	"CODEX_THREAD_ID",
	"COPILOT_AGENT_SESSION_ID",
	"PIJ_ANNOUNCE_TO",
	"PIJ_PARENT_ID",
	"PIJ_ROLE",
	"PIJ_SESSION_ID",
	"PIJ_SPAWN_ID",
	"PIJ_SPAWN_TASK",
	"PIJ_SPAWNER_ID",
	"PIJ_SPAWNER_PANE",
] as const;

const PIJ_ROUTING_ENV_KEYS = ["PIJ_DAEMON_GENERATION", "PIJ_RS_ADDR", "PIJ_RS_STATE_DIR"] as const;

/** Closed loopback port: a fixture child that names no daemon reaches none. */
export const UNREACHABLE_DAEMON_ADDR = "127.0.0.1:1";

/** A driver child is a fixture, never an incarnation of the agent running it.
 * Empty tmux overrides prevent ambient spawn identity from reaching the live
 * fleet, and a child with no explicit daemon routing is pointed at a closed
 * port rather than the production daemon. Explicit safe routing follows the
 * invoking check into a tmux server that an earlier test may already have
 * created; scenario values still win. */
export function isolatedChildEnvironment(
	explicit: Record<string, string> = {},
): Record<string, string> {
	const scrubbed: Record<string, string> = {
		PIJ_RS_ADDR: UNREACHABLE_DAEMON_ADDR,
	};
	for (const key of PIJ_ROUTING_ENV_KEYS) {
		const value = process.env[key];
		if (value !== undefined) scrubbed[key] = value;
	}
	Object.assign(scrubbed, explicit);
	const inheritedSpawnKeys = Object.keys(process.env).filter(
		(key) => key.startsWith("PIJ_SPAWN_") || key.startsWith("PIJ_SPAWNER"),
	);
	for (const key of [...new Set([...PIJ_IDENTITY_ENV_KEYS, ...inheritedSpawnKeys])].sort()) {
		if (!Object.hasOwn(scrubbed, key)) scrubbed[key] = "";
	}
	return scrubbed;
}

function tmux(args: string[]): string {
	// TC-01: argv array — never `args.join(" ")`. Closes D-014 shell-injection.
	const out = execFileSync("tmux", args, TMUX_OPTS);
	return typeof out === "string" ? out : out.toString("utf8");
}

function tmuxSafe(args: string[]): void {
	try {
		tmux(args);
	} catch {
		/* swallow */
	}
}

// ─── Targeting (TC-08) ──────────────────────────────────────────────────────

export function targetStr(t: Target): string {
	if (t.paneId) return t.paneId; // %N — rename-proof
	const w = t.window ?? 0;
	const p = t.pane ?? 0;
	return `${t.session}:${w}.${p}`;
}

// ─── Lifecycle (TC-09) ──────────────────────────────────────────────────────

export function boot(opts: BootOpts): Target {
	tmuxSafe(["kill-session", "-t", opts.session]); // idempotent

	const args = [
		"new-session",
		"-d",
		"-s",
		opts.session,
		"-x",
		String(opts.cols ?? 200),
		"-y",
		String(opts.rows ?? 50),
		"-c",
		opts.cwd,
	];
	for (const [key, value] of Object.entries(isolatedChildEnvironment(opts.env))) {
		args.push("-e", `${key}=${value}`); // tmux 3.0+ supports repeated -e
	}
	args.push(opts.cmd);

	try {
		tmux(args);
	} catch (e) {
		throw new DriverBootError(opts, e as Error);
	}

	const paneId = tmux(["display-message", "-p", "-t", `${opts.session}:0.0`, "#{pane_id}"]).trim();
	if (!/^%\d+$/.test(paneId)) {
		tmuxSafe(["kill-session", "-t", opts.session]);
		throw new DriverBootError(opts, new Error(`bad pane_id: ${paneId}`));
	}

	return { session: opts.session, paneId };
}

export function teardown(t: Target): void {
	tmuxSafe(["kill-session", "-t", t.session]);
}

export function hasSession(name: string): boolean {
	try {
		tmux(["has-session", "-t", name]);
		return true;
	} catch {
		return false;
	}
}

// ─── Inspection (TC-11) ─────────────────────────────────────────────────────

export function inspect(t: Target): PaneInfo {
	const fmt =
		"#{pane_id}\t#{pane_pid}\t#{pane_current_command}\t#{pane_dead}\t#{pane_width}\t#{pane_height}";
	const out = tmux(["list-panes", "-t", targetStr(t), "-F", fmt]).trim();
	const [paneId, pid, cmd, dead, cols, rows] = out.split("\t");
	return {
		paneId: paneId ?? "",
		pid: Number(pid),
		cmd: cmd ?? "",
		dead: dead === "1",
		cols: Number(cols),
		rows: Number(rows),
	};
}

export function assertAlive(t: Target): void {
	const info = inspect(t);
	if (info.dead) throw new DriverPaneDeadError(t, info);
}

// ─── Input (TC-02) ──────────────────────────────────────────────────────────

// Function name kept as `type` to match workshop 001's public surface; the
// re-export in index.ts aliases it to avoid clashing with the `type`
// keyword in consumer call sites. The argv construction now lives once in the
// shared lib (adapters/tmux-keys.ts) — these delegate for parity (finding 02).
export function type(t: Target, text: string): void {
	typeLiteral(targetStr(t), text);
}

export function press(t: Target, key: Key, n: number = 1): void {
	pressKey(targetStr(t), key, n);
}

export function paste(t: Target, data: string, opts: { bracketed?: boolean } = {}): void {
	pasteBuffer(targetStr(t), data, opts);
}

// ─── Capture (TC-04) ────────────────────────────────────────────────────────

export function capture(t: Target, opts: CaptureOpts = {}): string {
	return capturePane(targetStr(t), opts);
}

// ─── Recording (TC-06) ──────────────────────────────────────────────────────

export function record(t: Target, path: string): { stop: () => void } {
	// pipe-pane's <shell-command> argument is interpreted by tmux's shell;
	// shellQuote here is correct (this is NOT D-014 — tmux owns the shell
	// invocation, we just hand it a single safely-quoted command string).
	tmux(["pipe-pane", "-O", "-t", targetStr(t), `cat >> ${shellQuote(path)}`]);
	return {
		stop: () => tmuxSafe(["pipe-pane", "-t", targetStr(t)]),
	};
}

function shellQuote(s: string): string {
	return `'${s.replace(/'/g, "'\\''")}'`;
}
