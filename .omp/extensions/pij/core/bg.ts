// pij-control-plane — background jobs that report themselves when they finish.
//
// PURE. Builds the job spec and the completion message; every side effect
// (spawning, writing, delivering) belongs to the caller's ports.
//
// Why this exists: an agent that shells out to a long command must either block
// its whole turn or poll. Neither is acceptable in a pushed-turn harness — the
// seat is idle-but-not-done, which is exactly the shape the watchdog derives as
// a stall. `pij bg` inverts it: the command runs detached and the RESULT arrives
// as an injected turn, so the agent's turn ends immediately and it is woken by
// the completion instead of waiting for it.
//
// The delivery is stamped `pij-bg`, not the caller. That is not a workaround for
// the E-SELF guard (`cannot send to yourself`) — it is the honest actor: the
// message genuinely comes from the runner, not from the seat that queued it.
// Same precedent as `pij-watchdog`, which injects turns the same way. The actor
// is a fixed literal and never caller-supplied, so this adds no spoofing vector.

import type { Result, SessionId } from "./types.js";
import { err, ok } from "./types.js";

/** The pseudo-actor every bg completion is delivered from. Fixed literal —
 *  never caller-supplied (see the spoofing note in the module header). */
export const BG_ACTOR = "pij-bg";

/** Environment variable names the detached wrapper reads. Passing the user's
 *  command through the ENVIRONMENT rather than interpolating it into the wrapper
 *  script is what makes quoting safe: no amount of quoting in `--command` can
 *  break out of the wrapper, because the wrapper never contains it. */
export const BG_ENV = {
	command: "PIJ_BG_COMMAND",
	out: "PIJ_BG_OUT",
	title: "PIJ_BG_TITLE",
	to: "PIJ_BG_TO",
	jobId: "PIJ_BG_JOB",
} as const;

/** How much captured output rides inline on the completion turn. The full log is
 *  always on disk and always pointed at; this is the "did it work" preview so the
 *  common case needs no file read. Deliberately small — a bg job may emit
 *  megabytes, and a delivery is an injected TURN whose cost is context. */
export const BG_TAIL_LIMIT = 1200;

/** Longest title we accept. A title is a routing key a human reads in a nudge,
 *  not a description. */
export const BG_TITLE_MAX = 120;

export interface BgJobSpec {
	readonly jobId: string;
	readonly title: string;
	readonly command: string;
	readonly to: SessionId;
	readonly outPath: string;
}

/** Validate and shape a launch request. Rejects an empty command or title rather
 *  than launching a job whose completion nobody can attribute. */
export function planBgJob(input: {
	readonly title: string;
	readonly command: string;
	readonly to: SessionId;
	readonly jobId: string;
	readonly outDir: string;
}): Result<BgJobSpec> {
	const title = input.title.trim();
	const command = input.command.trim();
	if (title === "") return err("E-ARG", "--title must not be empty");
	if (title.length > BG_TITLE_MAX) {
		return err("E-ARG", `--title must be at most ${BG_TITLE_MAX} characters`);
	}
	if (command === "") return err("E-ARG", "--command must not be empty");
	if (title.includes("\n")) return err("E-ARG", "--title must be a single line");
	return ok({
		jobId: input.jobId,
		title,
		command,
		to: input.to,
		outPath: `${input.outDir}/${input.jobId}.log`,
	});
}

/** Last {@link BG_TAIL_LIMIT} characters, trimmed, with an explicit marker when
 *  content was dropped — a silently truncated log reads as a complete one. */
export function tailOf(output: string, limit: number = BG_TAIL_LIMIT): string {
	const trimmed = output.trimEnd();
	if (trimmed.length <= limit) return trimmed;
	return `…[truncated ${trimmed.length - limit} chars — full log at the path above]…\n${trimmed.slice(-limit)}`;
}

/** Human-readable elapsed time. Whole seconds under a minute; `NmNNs` above. */
export function formatDuration(ms: number): string {
	if (!Number.isFinite(ms) || ms < 0) return "unknown";
	const totalSeconds = Math.round(ms / 1000);
	if (totalSeconds < 60) return `${totalSeconds}s`;
	const minutes = Math.floor(totalSeconds / 60);
	return `${minutes}m${String(totalSeconds % 60).padStart(2, "0")}s`;
}

/** The completion turn. Leads with the VERDICT and the title, because this
 *  arrives unsolicited in an agent's context and the first line has to answer
 *  "what finished, and did it work?" before anything else. */
export function buildBgCompletionTurn(input: {
	readonly title: string;
	readonly exitCode: number;
	readonly durationMs: number;
	readonly outPath: string;
	readonly output: string;
}): string {
	const verdict = input.exitCode === 0 ? "OK" : `FAILED (exit ${input.exitCode})`;
	const tail = tailOf(input.output);
	const parts = [
		`[pij bg] ${verdict} — ${input.title} (${formatDuration(input.durationMs)})`,
		`full log: ${input.outPath}`,
	];
	if (tail !== "") parts.push(`tail: ${flatten(tail)}`);
	return parts.join(" · ");
}

/** The wrapper the detached child executes.
 *
 *  Every value it needs arrives through the environment (see {@link BG_ENV}), so
 *  the script itself is a CONSTANT — the user's command is never interpolated
 *  into it and therefore cannot alter its structure.
 *
 *  `exec` is deliberate on the notify call: the wrapper's own exit status is
 *  irrelevant once the payload is delivered, and the job's status is carried in
 *  the message, not in the shell. */
export function bgWrapperScript(notifyArgv: readonly string[]): string {
	const notify = notifyArgv.map(shellQuote).join(" ");
	return [
		`sh -c "$${BG_ENV.command}" > "$${BG_ENV.out}" 2>&1`,
		"__pij_bg_code=$?",
		// The notify's own output goes to a sidecar log, never to /dev/null. A
		// delivery that fails here fails INVISIBLY — the job ran, the output is
		// correct, and the caller simply waits forever for a turn that will never
		// arrive. That is the worst failure this feature can have, so it must
		// leave evidence next to the job it belongs to.
		`${notify} --to "$${BG_ENV.to}" --title "$${BG_ENV.title}" --out "$${BG_ENV.out}" --exit "$__pij_bg_code" --job "$${BG_ENV.jobId}" > "$${BG_ENV.out}.notify" 2>&1`,
	].join("\n");
}

/** Render multi-line text as ONE line with visible break markers.
 *
 *  A completion turn is delivered by typing it into a peer's composer
 *  (`send-keys -l`), where a real newline is a SUBMIT — so bodies are flattened
 *  in transit and a multi-line message arrives as an unreadable run-on
 *  ("…(31s)full output: /path/x.logtick 1tick 2…", observed on the first live
 *  run). Formatting for one line is therefore not a style choice: it is the only
 *  shape that survives the transport intact on every delivery path. */
function flatten(text: string): string {
	return text
		.split(/\r?\n/)
		.map((line) => line.trim())
		.filter((line) => line !== "")
		.join(" ⏎ ");
}

/** Single-quote for POSIX sh. Only ever applied to values pij itself controls
 *  (the notify argv), never to user input — user input travels by environment. */
function shellQuote(value: string): string {
	return `'${value.replaceAll("'", `'\\''`)}'`;
}

/** Recover a job's start instant from its id.
 *
 *  `bg-<base36 ms>-<rand>`. Deliberately strict: base-36 will happily parse a
 *  random suffix like `x2` into a small integer, which then renders as a
 *  duration of "29756269m09s" (observed). A stamp that cannot be a plausible
 *  launch time is no stamp at all — return undefined and say "unknown" rather
 *  than print a confident absurdity. */
export function jobStartedAtMs(jobId: string, nowMs: number): number | undefined {
	const encoded = jobId.split("-")[1];
	if (encoded === undefined || !/^[0-9a-z]+$/.test(encoded)) return undefined;
	const parsed = Number.parseInt(encoded, 36);
	if (!Number.isFinite(parsed)) return undefined;
	// Must land inside a sane window: after 2020 and not in the future.
	if (parsed < 1_577_836_800_000 || parsed > nowMs + 60_000) return undefined;
	return parsed;
}

// ─── job records (what makes list/tail/kill possible) ───────────────────────
//
// A queued job used to leave only its log file, so nothing on disk knew its
// title, its pid, or whether it was still running. `list` had nothing to render
// and `kill` had nothing to target. The record is written at launch, BEFORE the
// child starts (persist-before-mutate, P9): a job that exists but is unlisted is
// a job nobody can stop.

/** Where a job's record and log live, derived from the owning seat's data dir. */
export function bgJobPaths(
	dataDir: string,
	jobId: string,
): {
	readonly record: string;
	readonly log: string;
} {
	return { record: `${dataDir}/${jobId}.json`, log: `${dataDir}/${jobId}.log` };
}

/** How a finished job ended. `killed` is deliberately distinct from a non-zero
 *  exit: an operator who stopped a job should never wonder whether it failed. */
export type BgJobOutcome = "ok" | "failed" | "killed";

export interface BgJobRecord {
	readonly schema_version: 1;
	readonly jobId: string;
	readonly title: string;
	readonly command: string;
	readonly owner: SessionId;
	readonly startedAt: string;
	/** The wrapper's process GROUP. `kill` must signal the group, not the pid —
	 *  the wrapper spawns the real command as a child, so signalling only the
	 *  wrapper leaves the actual work running and orphaned. */
	readonly pgid: number;
	readonly logPath: string;
	/** Absent while running. */
	readonly finishedAt?: string;
	readonly outcome?: BgJobOutcome;
	readonly exitCode?: number;
}

export function newBgJobRecord(input: {
	readonly spec: BgJobSpec;
	readonly pgid: number;
	readonly nowIso: string;
}): BgJobRecord {
	return {
		schema_version: 1,
		jobId: input.spec.jobId,
		title: input.spec.title,
		command: input.spec.command,
		owner: input.spec.to,
		startedAt: input.nowIso,
		pgid: input.pgid,
		logPath: input.spec.outPath,
	};
}

/** Is this job still going? Derived from the record plus a liveness probe, never
 *  from the record alone — a job whose process died without ever writing its
 *  completion (machine reboot, SIGKILL) would otherwise read as running forever. */
export function bgJobState(
	record: BgJobRecord,
	isAlive: (pid: number) => boolean,
): "running" | "done" | "lost" {
	if (record.finishedAt !== undefined) return "done";
	return isAlive(record.pgid) ? "running" : "lost";
}

/** One line per job. Deliberately terse and fixed-width-ish: `bg list` is read
 *  at a glance, and a multi-line entry per job defeats the purpose. */
export function renderBgJobLine(
	record: BgJobRecord,
	state: "running" | "done" | "lost",
	nowMs: number,
): string {
	const startedMs = Date.parse(record.startedAt);
	// A RUNNING job's number is elapsed-so-far, measured from now. A FINISHED
	// job's is how long it TOOK, measured from its own completion — reading `now`
	// for that reports how long ago it started, which grows forever and made a
	// 7m22s CI wait render as "ok in 82m05s" an hour later. Same word, two
	// different questions.
	const finishedMs = record.finishedAt === undefined ? undefined : Date.parse(record.finishedAt);
	const endMs = state === "running" || finishedMs === undefined ? nowMs : finishedMs;
	const age =
		Number.isNaN(startedMs) || Number.isNaN(endMs) ? "?" : formatDuration(endMs - startedMs);
	const verdict =
		state === "running"
			? `running ${age}`
			: state === "lost"
				? `lost (no completion recorded)`
				: `${record.outcome ?? "done"}${record.exitCode ? ` (exit ${record.exitCode})` : ""} in ${age}`;
	return `${record.jobId}  ${verdict}  ${record.title}`;
}

/** The turn a KILLED job fires back.
 *
 *  A kill that stays silent re-creates the exact failure `pij bg` exists to
 *  abolish: the caller waits forever for a result that can never arrive. Killing
 *  is an ending, and every ending reports. */
export function buildBgKilledTurn(record: BgJobRecord, outPath: string): string {
	return `[pij bg] KILLED — ${record.title} · full log: ${outPath}`;
}
