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

/** A contract pass on `version` that recorded a local pass for it (F250). */
const passOn = (version: string) => ({
	status: "pass",
	version,
	verified: "1.0.113-0",
	local: { version, passed_at: "2026-11-21T12:00:00.000Z", checks: 1, contract_commit: null },
	checks: [{ name: "tool declared", ok: true, detail: "ok" }],
});

describe("just update-copilot (D-F208-2)", () => {
	it("updates, runs the contract on the new version, and says the fleet may reload on a local pass", () => {
		const { run, updates, contractArgs } = updateCopilot({
			next: "1.0.115-0",
			report: passOn("1.0.115-0"),
		});
		expect(run.status, run.stdout + run.stderr).toBe(0);
		expect(updates).toBe("updated\n");
		expect(contractArgs).toBe("--json\n");
		expect(run.stdout).toContain("Copilot updated 1.0.114-0 -> 1.0.115-0");
		expect(run.stdout).toContain(
			"the contract passed on Copilot 1.0.115-0 and recorded a local pass for it (2026-11-21T12:00:00.000Z); nothing to commit",
		);
		expect(run.stdout).toContain("The fleet may reload Copilot seats.");
	});

	it("a pass on the committed floor's version may reload too", () => {
		const { local: _local, ...report } = passOn("1.0.115-0");
		const { run } = updateCopilot({
			next: "1.0.115-0",
			report: { ...report, verified: "1.0.115-0" },
		});
		expect(run.status, run.stdout + run.stderr).toBe(0);
		expect(run.stdout).toContain("passed on Copilot 1.0.115-0, the committed floor");
	});

	// R261-001: a pass approves only the version it checked and recorded.
	it.each([
		[
			"checked an older version",
			passOn("1.0.114-0"),
			"passed on Copilot 1.0.114-0, verified 1.0.114-0",
		],
		[
			"names no version",
			{ status: "pass", checks: [{ name: "tool declared", ok: true, detail: "ok" }] },
			"passed on Copilot (no version named), verified (nothing)",
		],
		[
			"recorded a local pass for another version",
			{ ...passOn("1.0.115-0"), local: { ...passOn("1.0.114-0").local } },
			"passed on Copilot 1.0.115-0, verified 1.0.114-0",
		],
	])("never says the fleet may reload when the passing contract %s", (_case, report, named) => {
		const { run } = updateCopilot({ next: "1.0.115-0", report });
		expect(run.status).toBe(1);
		expect(run.stdout).toContain(DO_NOT_RELOAD);
		expect(run.stdout).toContain(`${named}, but Copilot 1.0.115-0 is installed`);
		expect(run.stdout).not.toContain("may reload");
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
		const { run, updates, contractArgs } = updateCopilot({ report: passOn("1.0.114-0") });
		expect(run.status, run.stdout + run.stderr).toBe(0);
		expect(updates).toBe("updated\n");
		expect(run.stdout).toContain(
			"no update available: Copilot is still 1.0.114-0; running the contract anyway",
		);
		expect(contractArgs).toBe("--json\n");
	});

	it("never says the fleet may reload when the contract's report is unreadable", () => {
		const { run } = updateCopilot({ next: "1.0.115-0", report: {} });
		expect(run.status).toBe(1);
		expect(run.stdout).toContain(DO_NOT_RELOAD);
	});
});

describe("the contract's verdict on a Copilot version it has not verified (D-F208-2)", () => {
	const checks = [{ name: "tool declared", ok: true }];

	it("names the fix verbatim, and no record PR, on a version nothing verified", () => {
		const { status, summary } = verdict({ version: "1.0.115-0", verified: "1.0.114-0", checks });
		expect(status).toBe("warn");
		expect(summary).toContain("run just copilot-sdk-contract before reloading seats");
		expect(summary).not.toContain("--record");
	});

	// F250: a local pass clears the warn only for exactly its own version.
	it("a local pass on the installed version clears the warn; one on another version does not", () => {
		const local = (version: string) => ({ version, passed_at: "2026-11-21T12:00:00.000Z" });
		const on3 = verdict({
			version: "1.0.115-3",
			verified: "1.0.115-2",
			local: local("1.0.115-3"),
			checks,
		});
		expect(on3.status).toBe("pass");
		expect(on3.summary).toContain("verified on this machine at 2026-11-21T12:00:00.000Z");
		expect(
			verdict({ version: "1.0.115-3", verified: "1.0.115-2", local: local("1.0.115-2"), checks })
				.status,
		).toBe("warn");
	});

	it("says why a passing run recorded no local pass", () => {
		const { status, summary } = verdict({
			version: "1.0.115-3",
			verified: "1.0.115-2",
			checks,
			unrecorded: "EACCES",
		});
		expect(status).toBe("warn");
		expect(summary).toContain("recorded no local pass: EACCES");
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
