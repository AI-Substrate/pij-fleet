import type {
	HarnessVerb,
	VerbContext,
	VerbResult,
} from "@ai-substrate/engineering-harness/contract";

/**
 * A single deterministic sensor — wraps a REAL repo command (P8 "wrap, don't
 * rebuild"). The set below mirrors pij's `just self-check` composite, so
 * `harness checks` IS the signal inventory made runnable. Add a sensor here when
 * the engineering harness adopts a new backpressure/check.
 */
interface Sensor {
	name: string;
	cmd: string;
	args: string[];
	/** Skipped under --quick (slow and/or environment-bound, e.g. tmux smoke). */
	heavy?: boolean;
	/** Report-only sensor: preserve its output without failing the gate. */
	advisory?: boolean;
	/** Extra env for this sensor (merged into process.env for the child). */
	env?: Record<string, string>;
	/** One-line description of what it proves. */
	proves: string;
}

const SENSORS: Sensor[] = [
	{
		name: "local-paths",
		cmd: "just",
		args: ["local-path-check"],
		proves: "operational files contain no user-specific absolute home paths",
	},
	{
		name: "typecheck",
		cmd: "just",
		args: ["typecheck"],
		proves: "the TypeScript surface compiles",
	},
	{
		name: "lockfile",
		cmd: "just",
		args: ["lockfile-allowlist"],
		proves: "every package-lock.json source is an allowed registry host",
	},
	{ name: "lint", cmd: "just", args: ["lint"], proves: "Biome (errors + warnings) is clean" },
	{ name: "test", cmd: "just", args: ["test"], proves: "the vitest suite passes" },
	{
		name: "copilot-native",
		cmd: "just",
		args: ["copilot-native-test"],
		proves: "native Copilot extension node:test contracts pass",
	},
	{
		name: "rust",
		cmd: "just",
		args: ["rust-check"],
		heavy: true,
		proves: "Cargo.lock is current and the Rust workspace passes fmt, clippy and tests",
	},
	{
		name: "smoke",
		cmd: "just",
		args: ["smoke"],
		heavy: true,
		proves: "the tmux-driven end-to-end driver scenarios pass",
	},
	{
		name: "pij-commit-trailers",
		cmd: "just",
		args: ["pij-commit-trailers", "--json"],
		advisory: true,
		proves: "branch-local harness commits carry derived pij attribution (advisory)",
	},
];

interface SensorResult {
	name: string;
	status: "pass" | "fail" | "warn" | "skipped";
	code: number;
	proves: string;
	output?: string;
}

/** Last N non-empty lines of a failing sensor's output. */
function tail(text: string, n = 25): string {
	const lines = text.trimEnd().split("\n");
	return lines.slice(-n).join("\n");
}

const checks: HarnessVerb = {
	name: "checks",
	summary:
		"Run pij's full deterministic gate (the signal inventory) and report a ship/done verdict.",
	description:
		'The single "are we done?" gate. Runs every sensor in the engineering-harness signal inventory ' +
		"(mirrors `just self-check`: local-paths, typecheck, lockfile, lint, test, copilot-native, rust, smoke, pij-commit-trailers) as individual " +
		"stages, reports a per-sensor verdict, and — unlike self-check — runs ALL of them so you see every " +
		"failure in one pass. Run it before ship and before declaring any non-trivial task done. " +
		"`--quick` skips heavy sensors (rust, smoke) for a fast static+unit gate.",
	options: [
		{ flags: "--quick", description: "skip heavy sensors (rust, smoke) — fast static + unit gate" },
	],
	async run(ctx: VerbContext): Promise<VerbResult> {
		const quick = ctx.options.quick === true;
		const results: SensorResult[] = [];
		const failureLogs: Array<{ name: string; output: string }> = [];

		for (const s of SENSORS) {
			if (quick && s.heavy) {
				results.push({ name: s.name, status: "skipped", code: 0, proves: s.proves });
				continue;
			}
			// Apply per-sensor env (spawn inherits process.env), restore afterwards.
			const saved: Array<[string, string | undefined]> = [];
			if (s.env) {
				for (const [k, v] of Object.entries(s.env)) {
					saved.push([k, process.env[k]]);
					process.env[k] = v;
				}
			}
			let res: { ok: boolean; code: number; stdout: string; stderr: string };
			try {
				res = await ctx.exec(s.cmd, s.args);
			} finally {
				for (const [k, v] of saved) {
					if (v === undefined) delete process.env[k];
					else process.env[k] = v;
				}
			}
			let advisoryStatus: "pass" | "warn" = "warn";
			if (s.advisory && res.ok) {
				try {
					const report = JSON.parse(res.stdout) as { status?: unknown };
					if (report.status === "pass") advisoryStatus = "pass";
				} catch {
					// An unavailable advisory must remain visible, never block delivery.
				}
			}
			results.push({
				name: s.name,
				status: s.advisory ? advisoryStatus : res.ok ? "pass" : "fail",
				code: res.code,
				proves: s.proves,
				...(s.advisory ? { output: res.stdout || res.stderr } : {}),
			});
			if (!res.ok && !s.advisory) failureLogs.push({ name: s.name, output: tail(res.stderr || res.stdout) });
		}

		const failed = results.filter((r) => r.status === "fail");
		const skipped = results.filter((r) => r.status === "skipped").map((r) => r.name);
		const advisoryGuidance = results.some((r) => r.status === "warn")
			? " Review advisory attribution findings with `just pij-commit-trailers`; JSON details are in results[].output."
			: "";

		if (failed.length > 0) {
			const names = failed.map((r) => r.name).join(", ");
			return ctx.error("checks-failed", `${failed.length} check(s) failed: ${names}.`, {
				details: { results, failures: failureLogs, skipped },
				next_action: `Fix the failing check(s) above (${names}), then re-run \`harness checks\`. Not ship/done-ready until this is green.${advisoryGuidance}`,
			});
		}

		const ran = results.filter((r) => r.status !== "skipped").map((r) => r.name);
		return ctx.ok(
			{ ok: true, ran, skipped, results },
			{
				next_action: (quick
					? "Quick checks green. Run the full `harness checks` (incl. smoke) before ship / declaring done."
					: advisoryGuidance
						? "Required checks green."
						: "All checks green — safe to ship / declare this task done.") + advisoryGuidance,
			},
		);
	},
};

export default checks;
