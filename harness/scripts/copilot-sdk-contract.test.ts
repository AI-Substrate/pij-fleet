import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
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
			// F250: a full pass records a local pass; keep it out of the real state dir.
			const state = mkdtempSync(join(tmpdir(), "pij-copilot-contract-state-"));
			try {
				const run = spawnSync(process.execPath, ["--import", lateStderr, script, "--json"], {
					encoding: "utf8",
					env: { ...process.env, PIJ_RS_STATE_DIR: state },
					timeout: 60_000,
				});
				let report:
					| {
							status?: string;
							version?: string;
							local?: { version: string; checks: number };
							checks?: Array<{ ok: boolean }>;
					  }
					| undefined;
				try {
					report = JSON.parse(run.stdout.trim().split("\n").at(-1) ?? "");
				} catch {
					report = undefined;
				}
				expect(report, `no report; stdout:\n${run.stdout}\nstderr:\n${run.stderr}`).toBeDefined();
				expect(
					report?.checks?.filter((check: { ok: boolean }) => !check.ok),
					run.stdout,
				).toEqual([]);
				expect(report?.checks).toHaveLength(7);
				expect(run.status).toBe(0);
				expect(report?.status).toBe("pass");
				expect(report?.local).toMatchObject({ version: report?.version, checks: 7 });
				expect(readdirSync(join(state, "copilot-sdk-pass"))).toEqual([`${report?.version}.json`]);
			} finally {
				rmSync(state, { recursive: true, force: true });
			}
		},
	);

	// F250: a pass on an SDK or version named by flag checked something other
	// than the installed CLI, so it records no local pass.
	it.skipIf(!installed)("records no local pass for an overridden version", () => {
		const state = mkdtempSync(join(tmpdir(), "pij-copilot-contract-state-"));
		try {
			const version = /^GitHub Copilot CLI (\S+)\.$/m.exec(
				execFileSync(process.env.COPILOT_BIN || "copilot", ["--version"], {
					env: { ...process.env, COPILOT_OFFLINE: "true" },
					encoding: "utf8",
				}),
			)?.[1];
			const run = spawnSync(
				process.execPath,
				[script, "--json", "--copilot-version", `${version}`],
				{
					encoding: "utf8",
					env: { ...process.env, PIJ_RS_STATE_DIR: state },
					timeout: 60_000,
				},
			);
			const report = JSON.parse(run.stdout.trim().split("\n").at(-1) ?? "");
			expect(
				report.checks.every((check: { ok: boolean }) => check.ok),
				run.stdout,
			).toBe(true);
			expect(report.local).toBeUndefined();
			expect(readdirSync(state)).toEqual([]);
		} finally {
			rmSync(state, { recursive: true, force: true });
		}
	});
});
