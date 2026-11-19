import { execFileSync, spawnSync } from "node:child_process";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

const script = join(import.meta.dirname, "copilot-sdk-contract.mjs");
const lateStderr = pathToFileURL(
	join(import.meta.dirname, "fixtures", "copilot-sdk-contract-late-stderr.mjs"),
).href;

// The contract drives the INSTALLED Copilot CLI's SDK, so it runs only where
// one is installed (never in CI); the script itself skips there too.
const installed = (() => {
	try {
		execFileSync(process.env.COPILOT_BIN || "copilot", ["--version"], {
			env: { ...process.env, COPILOT_OFFLINE: "true" },
			stdio: "ignore",
			timeout: 10_000,
		});
		return true;
	} catch {
		return false;
	}
})();

describe("copilot-sdk-contract", () => {
	it.skipIf(!installed)(
		"still sees every hook and tool call when the child's stderr arrives late and cut mid-line (R259-001)",
		() => {
			const run = spawnSync(process.execPath, ["--import", lateStderr, script, "--json"], {
				encoding: "utf8",
				timeout: 60_000,
			});
			let report: { checks?: Array<{ ok: boolean }> } | undefined;
			try {
				report = JSON.parse(run.stdout.trim().split("\n").at(-1) ?? "");
			} catch {
				report = undefined;
			}
			expect(report, `no report; stdout:\n${run.stdout}\nstderr:\n${run.stderr}`).toBeDefined();
			expect(
				report.checks?.filter((check: { ok: boolean }) => !check.ok),
				run.stdout,
			).toEqual([]);
			expect(report.checks).toHaveLength(7);
			expect(run.status).toBe(0);
		},
	);
});
