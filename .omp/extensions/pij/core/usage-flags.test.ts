import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { RS_ROUTE_TABLE } from "./generation-routing.js";

const CLI = join(import.meta.dirname, "..", "cli.ts");
const TSX_CLI = createRequire(import.meta.url).resolve("tsx/cli");
let sandbox: string;
let usage: string;

// The executable no longer runs core/cli.ts's legacy ALLOWED_FLAGS parser.
// Its help must describe the real rs inventory, not recreate the removed USAGE
// string or claim that legacy-only flags remain accepted by the new executable.
beforeAll(() => {
	sandbox = mkdtempSync(join(tmpdir(), "pij-rs-usage-"));
	const result = spawnSync(process.execPath, [TSX_CLI, CLI, "--help"], {
		cwd: sandbox,
		env: {
			...process.env,
			HOME: sandbox,
			PIJ_HOME: join(sandbox, "pij"),
			PIJ_RS_STATE_DIR: join(sandbox, "rs"),
			PIJ_RS_ADDR: "127.0.0.1:1",
		},
		encoding: "utf8",
		timeout: 10_000,
	});
	expect(result.error).toBeUndefined();
	expect(result.status, result.stderr).toBe(0);
	usage = result.stdout;
}, 15_000);

afterAll(() => {
	if (sandbox !== undefined) rmSync(sandbox, { recursive: true, force: true });
});

describe("executable help describes the actual rs command surface", () => {
	it("lists every explicit route exactly once with no daemon credentials", () => {
		expect(RS_ROUTE_TABLE.length).toBeGreaterThan(0);
		expect(usage).toContain("Usage: pij");
		const lines = usage.split("\n");
		for (const row of RS_ROUTE_TABLE) {
			const label = `pij ${row.verb}${row.leaf === undefined ? "" : ` ${row.leaf}`}:`;
			expect(
				lines.filter((line) => line.startsWith(label)),
				label,
			).toHaveLength(1);
		}
	});

	it("names unported verbs and points to the permanent refusal ledger", () => {
		const refused = RS_ROUTE_TABLE.filter((row) => "unported" in row);
		expect(refused.length).toBeGreaterThan(0);
		for (const row of refused) {
			const label = `pij ${row.verb}${row.leaf === undefined ? "" : ` ${row.leaf}`}:`;
			expect(usage).toContain(`${label} E-RS-UNPORTED`);
		}
		expect(usage).toContain("docs/how/pij-rs-api.md#unsupported-status");
	});

	it("distinguishes HTTP JSON, native trailers and collection filter support", () => {
		expect(usage).toContain("HTTP --json preserves the complete validated v2 response envelope");
		expect(usage).toContain("commit-trailers preserves native output even with --json");
		const listHelp = usage.split("\n").find((line) => line.startsWith("pij list [")) ?? "";
		for (const flag of ["--harness", "--folder", "--parent", "--scope local"]) {
			expect(listHelp).toContain(flag);
		}
		expect(usage).toContain(
			"sessions accepts only bare invocation or --json; all filters are refused",
		);
	});
});
