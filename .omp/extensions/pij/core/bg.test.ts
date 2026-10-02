// pij-control-plane — background job planning + completion turns (pure).

import { describe, expect, it } from "vitest";
import type { BgJobRecord } from "./bg.js";
import {
	BG_ACTOR,
	BG_ENV,
	BG_TAIL_LIMIT,
	bgJobPaths,
	bgJobState,
	bgWrapperScript,
	buildBgCompletionTurn,
	buildBgKilledTurn,
	formatDuration,
	jobStartedAtMs,
	planBgJob,
	renderBgJobLine,
	tailOf,
} from "./bg.js";

const base = {
	title: "harness checks",
	command: "harness checks",
	to: "pij-seat",
	jobId: "bg-abc",
	outDir: "/home/.pij/pij-seat",
};

describe("planBgJob", () => {
	it("shapes a job and puts its log under the caller's data dir", () => {
		const planned = planBgJob(base);
		expect(planned.ok && planned.value.outPath).toBe("/home/.pij/pij-seat/bg-abc.log");
	});

	it("refuses a job nobody could attribute or run", () => {
		for (const bad of [
			{ ...base, title: "   " },
			{ ...base, command: "  " },
			{ ...base, title: "a\nb" },
			{ ...base, title: "x".repeat(200) },
		]) {
			expect(planBgJob(bad).ok).toBe(false);
		}
	});
});

describe("bgWrapperScript", () => {
	it("never interpolates the user's command — it is read from the environment", () => {
		// The injection guard: a command full of quotes and metacharacters must not
		// be able to alter the wrapper, because the wrapper never contains it.
		const script = bgWrapperScript(["node", "/path/cli.ts", "bg-deliver"]);
		expect(script).toContain(`"$${BG_ENV.command}"`);
		expect(script).not.toContain("rm -rf");
		const evil = planBgJob({ ...base, command: `x"; rm -rf /; echo "` });
		expect(evil.ok).toBe(true);
		expect(script).not.toContain("rm -rf /");
	});

	it("records the notify's own output so a failed delivery leaves evidence", () => {
		// A silent delivery failure is the worst outcome: the job succeeded and the
		// caller waits forever. Proven by asserting the sidecar redirect exists.
		expect(bgWrapperScript(["pij", "bg-deliver"])).toContain(`"$${BG_ENV.out}.notify"`);
	});

	it("quotes the notify argv it controls, so a spaced path survives", () => {
		expect(bgWrapperScript(["/my path/node", "cli.ts", "bg-deliver"])).toContain("'/my path/node'");
	});
});

describe("tailOf", () => {
	it("keeps short output whole", () => {
		expect(tailOf("done")).toBe("done");
	});

	it("keeps the END of long output and says so", () => {
		// The tail, not the head: a failure's cause is at the bottom of a log.
		const out = `${"a".repeat(BG_TAIL_LIMIT * 2)}THE-ERROR`;
		const tail = tailOf(out);
		expect(tail).toContain("THE-ERROR");
		expect(tail).toContain("truncated");
		expect(tail.length).toBeLessThan(out.length);
	});
});

describe("formatDuration", () => {
	it("reads as seconds under a minute and m/s above", () => {
		expect(formatDuration(30_000)).toBe("30s");
		expect(formatDuration(95_000)).toBe("1m35s");
		expect(formatDuration(Number.NaN)).toBe("unknown");
	});
});

describe("buildBgCompletionTurn", () => {
	const turn = (exitCode: number, output = "all good") =>
		buildBgCompletionTurn({
			title: "harness checks",
			exitCode,
			durationMs: 300_000,
			outPath: "/home/.pij/pij-seat/bg-abc.log",
			output,
		});

	it("leads with the verdict and the title", () => {
		// This arrives UNSOLICITED in an agent's context; the opening has to answer
		// "what finished, and did it work?" before anything else.
		expect(turn(0).startsWith("[pij bg] OK — harness checks (5m00s)")).toBe(true);
		expect(turn(1)).toContain("FAILED (exit 1)");
	});

	it("always points at the full log, and carries a preview", () => {
		expect(turn(0)).toContain("full log: /home/.pij/pij-seat/bg-abc.log");
		expect(turn(0)).toContain("all good");
	});

	it("stays readable when the job produced nothing", () => {
		expect(turn(0, "")).toBe(
			"[pij bg] OK — harness checks (5m00s) · full log: /home/.pij/pij-seat/bg-abc.log",
		);
	});

	it("emits ONE line — delivery types bodies into a composer, where \\n submits", () => {
		// Observed on the first live run: a multi-line body arrived as
		// "…(31s)full output: /path/x.logtick 1tick 2…". Line breaks do not survive
		// the transport, so the message has to be built for one line.
		const multi = turn(0, "tick 1\ntick 2\ncounted to 30");
		expect(multi).not.toContain("\n");
		expect(multi).toContain("tick 1 ⏎ tick 2 ⏎ counted to 30");
	});
});

describe("BG_ACTOR", () => {
	it("is a fixed literal, never caller-supplied", () => {
		// Delivering as the queueing seat would trip E-SELF and would also be a
		// lie; a caller-settable actor would be a spoofing vector.
		expect(BG_ACTOR).toBe("pij-bg");
	});
});

describe("jobStartedAtMs", () => {
	const NOW = Date.parse("2026-07-30T02:00:00.000Z");

	it("recovers the launch instant from a real job id", () => {
		const started = NOW - 31_000;
		expect(jobStartedAtMs(`bg-${started.toString(36)}-abc123`, NOW)).toBe(started);
	});

	it("returns undefined rather than a confident absurdity", () => {
		// base36 happily parses a random suffix into a small integer, which then
		// rendered as "29756269m09s" on a live probe. Out-of-window means unknown.
		for (const bad of ["bg-x2-y", "bg--z", "bg", "bg-ZZZZZZZZZZ-q"]) {
			expect(jobStartedAtMs(bad, NOW)).toBeUndefined();
		}
		expect(formatDuration(Number.NaN)).toBe("unknown");
	});
});

describe("bg job records — what makes list/tail/kill possible", () => {
	const NOW = Date.parse("2026-07-30T02:00:00.000Z");
	const record = (over: Partial<BgJobRecord> = {}): BgJobRecord => ({
		schema_version: 1,
		jobId: "bg-abc-def",
		title: "harness checks",
		command: "harness checks",
		owner: "pij-seat",
		startedAt: new Date(NOW - 90_000).toISOString(),
		pgid: 4242,
		logPath: "/home/.pij/pij-seat/bg-abc-def.log",
		...over,
	});

	it("puts a job's record beside its log", () => {
		const paths = bgJobPaths("/home/.pij/pij-seat", "bg-abc-def");
		expect(paths.record).toBe("/home/.pij/pij-seat/bg-abc-def.json");
		expect(paths.log).toBe("/home/.pij/pij-seat/bg-abc-def.log");
	});

	it("carries the process GROUP, not just a pid", () => {
		// The wrapper spawns the real command as a CHILD. Signalling the wrapper
		// alone leaves the actual work running and orphaned, with nothing left to
		// report its own completion.
		expect(record().pgid).toBe(4242);
	});

	describe("bgJobState", () => {
		const alive = () => true;
		const dead = () => false;

		it("is running while its group lives, done once it has finished", () => {
			expect(bgJobState(record(), alive)).toBe("running");
			expect(bgJobState(record({ finishedAt: "2026-07-30T01:59:00.000Z" }), alive)).toBe("done");
		});

		it("is LOST when the process is gone but no completion was ever recorded", () => {
			// A reboot or SIGKILL kills the wrapper before it can deliver. Trusting
			// the record alone would show that job as running forever; probing
			// liveness is what makes the difference visible.
			expect(bgJobState(record(), dead)).toBe("lost");
		});
	});

	describe("renderBgJobLine", () => {
		it("is ONE line per job — bg list is read at a glance", () => {
			const line = renderBgJobLine(record(), "running", NOW);
			expect(line).not.toContain("\n");
			expect(line).toContain("bg-abc-def");
			expect(line).toContain("running 1m30s");
			expect(line).toContain("harness checks");
		});

		it("reports how long a finished job TOOK, not how long ago it started", () => {
			// Observed live: a 7m22s CI wait rendered as "ok in 82m05s" an hour after
			// it finished, because the elapsed number was measured from `now` for
			// running AND finished jobs alike. A finished job's duration must stop
			// growing the moment it stops.
			const done = record({
				startedAt: new Date(NOW - 90 * 60_000).toISOString(),
				finishedAt: new Date(NOW - 83 * 60_000).toISOString(),
				outcome: "ok",
			});
			// NOW is an hour and a half after it started, seven minutes after it ran.
			expect(renderBgJobLine(done, "done", NOW)).toContain("in 7m00s");
			expect(renderBgJobLine(done, "done", NOW)).not.toContain("90m");
			// …and a job still running is still measured against now.
			expect(
				renderBgJobLine(
					record({ startedAt: new Date(NOW - 90_000).toISOString() }),
					"running",
					NOW,
				),
			).toContain("running 1m30s");
		});

		it("distinguishes killed from failed", () => {
			// An operator who stopped a job must never wonder whether it broke.
			const killed = renderBgJobLine(
				record({ finishedAt: new Date(NOW).toISOString(), outcome: "killed" }),
				"done",
				NOW,
			);
			expect(killed).toContain("killed");
			expect(killed).not.toContain("failed");
		});

		it("shows a failed job's exit code", () => {
			expect(
				renderBgJobLine(
					record({ finishedAt: new Date(NOW).toISOString(), outcome: "failed", exitCode: 127 }),
					"done",
					NOW,
				),
			).toContain("exit 127");
		});
	});

	it("a KILLED job still fires a turn back", () => {
		// The load-bearing one: a silent kill re-creates the exact failure bg
		// exists to abolish — the caller waits forever for a result that can now
		// never arrive. Killing is an ending, and every ending reports.
		const turn = buildBgKilledTurn(record(), record().logPath);
		expect(turn).toContain("KILLED");
		expect(turn).toContain("harness checks");
		expect(turn).toContain(record().logPath);
		expect(turn).not.toContain("\n");
	});
});
