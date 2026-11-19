import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { DO_NOT_RELOAD, verdict } from "./copilot-sdk-verdict.mjs";

const script = join(import.meta.dirname, "update-copilot.mjs");
const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

/**
 * `just update-copilot` against stubs only: a `copilot` reporting the version in
 * `version`, an update that writes `next` there (or leaves it), and a contract
 * that logs its arguments and prints `report`. Nothing real is updated.
 */
function updateCopilot({ next, report }: { next?: string; report: object }) {
	const root = mkdtempSync(join(tmpdir(), "pij-update-copilot-"));
	roots.push(root);
	const stub = (name: string, body: string) => {
		writeFileSync(join(root, name), `#!/bin/sh\n${body}\n`, { mode: 0o700 });
		return join(root, name);
	};
	writeFileSync(join(root, "version"), "1.0.114-0");
	const copilot = stub("copilot", `printf 'GitHub Copilot CLI %s.\\n' "$(cat '${root}/version')"`);
	const update = stub(
		"update",
		`echo updated >> '${root}/updates'${next ? `; printf %s '${next}' > '${root}/version'` : ""}`,
	);
	writeFileSync(join(root, "report.json"), JSON.stringify(report));
	const contract = stub(
		"contract",
		`printf '%s\\n' "$*" > '${root}/contract-args'; cat '${root}/report.json'`,
	);
	const run = spawnSync(process.execPath, [script], {
		encoding: "utf8",
		env: {
			...process.env,
			COPILOT_BIN: copilot,
			PIJ_COPILOT_UPDATE_CMD: update,
			PIJ_COPILOT_SDK_CONTRACT_CMD: contract,
		},
		timeout: 30_000,
	});
	const read = (name: string) => {
		try {
			return readFileSync(join(root, name), "utf8");
		} catch {
			return undefined;
		}
	};
	return { run, updates: read("updates"), contractArgs: read("contract-args") };
}

const pass = { status: "pass", checks: [{ name: "tool declared", ok: true, detail: "ok" }] };

describe("just update-copilot (D-F208-2)", () => {
	it("updates, runs the contract with --record on the new version, and says the fleet may reload on a pass", () => {
		const { run, updates, contractArgs } = updateCopilot({ next: "1.0.115-0", report: pass });
		expect(run.status, run.stdout + run.stderr).toBe(0);
		expect(updates).toBe("updated\n");
		expect(contractArgs).toBe("--record --json\n");
		expect(run.stdout).toContain("Copilot updated 1.0.114-0 -> 1.0.115-0");
		expect(run.stdout).toContain("The fleet may reload Copilot seats.");
	});

	it("stops non-zero with 'do not reload' and the failing checks when the contract fails", () => {
		const { run } = updateCopilot({
			next: "1.0.115-0",
			report: {
				status: "fail",
				summary: "Copilot 1.0.115-0's SDK would not register pij's hook agentStop",
				checks: [
					{ name: "tool declared", ok: true, detail: "ok" },
					{
						name: "hook agentStop",
						ok: false,
						detail: "hooks.invoke agentStop never reached pij's onAgentStop",
					},
				],
			},
		});
		expect(run.status).toBe(1);
		expect(run.stdout).toContain(DO_NOT_RELOAD);
		expect(run.stdout).toContain(
			"FAIL hook agentStop: hooks.invoke agentStop never reached pij's onAgentStop",
		);
		expect(run.stdout).not.toContain("may reload");
	});

	it("with no update available says so and still runs the contract", () => {
		const { run, updates, contractArgs } = updateCopilot({ report: pass });
		expect(run.status, run.stdout + run.stderr).toBe(0);
		expect(updates).toBe("updated\n");
		expect(run.stdout).toContain(
			"no update available: Copilot is still 1.0.114-0; running the contract anyway",
		);
		expect(contractArgs).toBe("--record --json\n");
	});

	it("never says the fleet may reload when the contract's report is unreadable", () => {
		const { run } = updateCopilot({ next: "1.0.115-0", report: {} });
		expect(run.status).toBe(1);
		expect(run.stdout).toContain(DO_NOT_RELOAD);
	});
});

describe("the contract's verdict on a Copilot version it has not verified (D-F208-2)", () => {
	const checks = [{ name: "tool declared", ok: true }];

	it("names the fix verbatim, with --record after a pass", () => {
		const { status, summary } = verdict({ version: "1.0.115-0", verified: "1.0.114-0", checks });
		expect(status).toBe("warn");
		expect(summary).toContain("run just copilot-sdk-contract before reloading seats");
		expect(summary).toContain("after a pass, run just copilot-sdk-contract --record");
	});

	it("a failing check is 'do not reload', whatever the record says", () => {
		const { status, summary } = verdict({
			version: "1.0.114-0",
			verified: "1.0.114-0",
			checks: [{ name: "hook agentStop", ok: false }],
			recorded: true,
		});
		expect(status).toBe("fail");
		expect(summary.startsWith("do not reload: the SDK would drop pij_send/hooks")).toBe(true);
	});

	it("a pass on the recorded version is a plain pass", () => {
		expect(verdict({ version: "1.0.114-0", verified: "1.0.114-0", checks }).status).toBe("pass");
	});
});
