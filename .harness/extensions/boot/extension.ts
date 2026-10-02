import type {
	HarnessVerb,
	VerbContext,
	VerbResult,
} from "@ai-substrate/engineering-harness/contract";

/** Tail the last N lines of a command's output for a compact failure report. */
function tail(text: string, n = 30): string {
	const lines = text.trimEnd().split("\n");
	return lines.slice(-n).join("\n");
}

/**
 * Both streams, in order, so a stage's diagnostics survive whichever stream the
 * tool chose. `tsc` prints its errors on STDOUT while `just`/`npm` put a warning
 * on STDERR; `stderr || stdout` showed the warning and dropped the errors
 * (req-0051, DL-002).
 */
function combined(result: { stdout: string; stderr: string }): string {
	return [result.stdout, result.stderr].filter((s) => s.trim().length > 0).join("\n");
}

/** Hard per-stage deadlines. The exec port SIGKILLs and resolves code 124. */
const TYPECHECK_TIMEOUT_MS = 60_000;
const SMOKE_TIMEOUT_MS = 90_000;
const FULL_TEST_TIMEOUT_MS = 600_000;
const TIMEOUT_EXIT_CODE = 124;

const ORIENTATION =
	"pij is a pi-extension workshop. The agent harness (the-flow, minih packs, retros) " +
	"sits on top of the engineering harness (just recipes, harness/ driver SDK, smoke). " +
	"Canonical gate before declaring done: `just self-check`. New extension: `just new <name>`.";

interface Stage {
	name: string;
	cmd: string;
	ok: boolean;
	code: number;
	elapsed_ms: number;
	timed_out: boolean;
}

const boot: HarnessVerb = {
	name: "boot",
	summary: "Prove pij is ready: typecheck + smoke (or --full vitest), then re-orient the agent.",
	description:
		"Readiness proof for pij. Runs `just typecheck` then `just smoke` (no shell, sequenced, " +
		"each stage under a hard deadline), returns a ready/error verdict the calling agent can " +
		"branch on, and prints orientation. `--full` runs `just test` (the whole vitest suite) " +
		"instead of smoke; that is the merge gate's job, not boot's, and takes minutes.",
	async run(ctx: VerbContext): Promise<VerbResult> {
		const full = Boolean(ctx.options.full);
		const stages: Stage[] = [];

		const runStage = async (
			name: string,
			args: string[],
			timeoutMs: number,
		): Promise<{ stage: Stage; output: string }> => {
			const started = Date.now();
			const result = await ctx.exec("just", args, { timeoutMs });
			const stage: Stage = {
				name,
				cmd: `just ${args.join(" ")}`,
				ok: result.ok,
				code: result.code,
				elapsed_ms: Date.now() - started,
				timed_out: result.code === TIMEOUT_EXIT_CODE,
			};
			stages.push(stage);
			return { stage, output: tail(combined(result)) };
		};

		const fail = (stage: Stage, output: string, what: string, fix: string): VerbResult =>
			ctx.error(
				stage.timed_out ? `boot-${stage.name}-timeout` : `boot-${stage.name}-failed`,
				stage.timed_out
					? `\`${stage.cmd}\` exceeded its ${what} deadline after ${stage.elapsed_ms} ms and was killed.`
					: `\`${stage.cmd}\` failed (exit ${stage.code}) — ${what}.`,
				{
					details: { stages, output },
					next_action: stage.timed_out
						? `The stage hung or is too slow for boot; run \`${stage.cmd}\` by hand to see where, then re-run \`harness boot\`.`
						: fix,
				},
			);

		const deps = await runStage("worktree-deps", ["worktree-deps", "--check"], 10_000);
		if (!deps.stage.ok) {
			return ctx.ok(
				{ ready: false, advisory: true, stages, output: deps.output, orientation: ORIENTATION },
				{
					next_action:
						"worktree-deps: NOT-READY. Run `just worktree-deps` before TypeScript/vitest work, then re-run `harness boot`. Rust-only work can continue.",
				},
			);
		}

		// Stage 1 — typecheck (the whole TS surface compiles).
		const tc = await runStage("typecheck", ["typecheck"], TYPECHECK_TIMEOUT_MS);
		if (!tc.stage.ok) {
			return fail(
				tc.stage,
				tc.output,
				"the TypeScript surface does not compile",
				"Fix the type errors in details.output, then re-run `harness boot`.",
			);
		}

		// Stage 2 — smoke by default (bounded, ~45 s); --full runs the vitest suite.
		const test = full
			? await runStage("test", ["test"], FULL_TEST_TIMEOUT_MS)
			: await runStage("smoke", ["smoke"], SMOKE_TIMEOUT_MS);
		if (!test.stage.ok) {
			return fail(
				test.stage,
				test.output,
				full ? "the vitest suite is red" : "the smoke proof is red",
				"Fix the failing tests in details.output, then re-run `harness boot`.",
			);
		}

		return ctx.ok(
			{ ready: true, mode: full ? "full" : "smoke", stages, orientation: ORIENTATION },
			{
				next_action:
					`pij is ready (typecheck + ${full ? "vitest" : "smoke"} green). Proceed with the-flow / your task. ` +
					"Before declaring done, run `just self-check` (the full gate; boot is readiness only).",
			},
		);
	},
};

export default boot;
