import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import type * as DriverModule from "../driver/index.js";
import { loadScenario, runScenario } from "../driver/index.js";
import {
	findProjectExtensionEntries,
	parseSmokeEnvelope,
	resolveSmokeCommand,
	runWatchdogSmoke,
} from "./smoke.js";

vi.mock("../driver/index.js", async (importOriginal) => {
	const actual = await importOriginal<typeof DriverModule>();
	return {
		...actual,
		loadScenario: vi.fn(),
		runScenario: vi.fn(),
	};
});

const cleanupPaths: string[] = [];

afterEach(() => {
	for (const path of cleanupPaths.splice(0)) {
		rmSync(path, { recursive: true, force: true });
	}
	vi.unstubAllEnvs();
});

describe("runWatchdogSmoke", () => {
	it("refuses real watchdog mutation under both routing modes without changing private authority", async () => {
		const result = await runWatchdogSmoke();
		expect(result, result.reason).toEqual({ verdict: "PASS" });
	}, 600_000);
});

describe("parseSmokeEnvelope", () => {
	it("preserves the complete successful wire envelope and added fields", () => {
		const envelope = {
			ok: true,
			command: "pij task",
			v: 2,
			data: { seq: 41, task: { id: "assignment" } },
			meta: "native receipt",
			extension: { value: "kept" },
		};
		expect(parseSmokeEnvelope({ status: 0, stdout: JSON.stringify(envelope), stderr: "" })).toEqual(
			envelope,
		);
	});

	it("preserves a named refusal instead of rebuilding it from its code", () => {
		const envelope = {
			ok: false,
			command: "pij watchdog",
			v: 2,
			error: "refused",
			details: { code: "E-RS-UNPORTED", verb: "watchdog", ledger_item: "native-only" },
			meta: "no fallback",
		};
		expect(
			parseSmokeEnvelope(
				{ status: 4, stdout: JSON.stringify(envelope), stderr: "" },
				false,
				"E-RS-UNPORTED",
			),
		).toEqual(envelope);
	});

	it("rejects flattened data, wrong versions and mismatched process status", () => {
		for (const envelope of [
			{ id: "flattened" },
			{ ok: true, command: "pij task", v: 1, data: {} },
			{ ok: false, command: "pij task", v: 2, data: {} },
		]) {
			expect(() =>
				parseSmokeEnvelope({ status: 0, stdout: JSON.stringify(envelope), stderr: "" }),
			).toThrow();
		}
		expect(() =>
			parseSmokeEnvelope({
				status: 2,
				stdout: JSON.stringify({ ok: true, command: "pij task", v: 2, data: {} }),
				stderr: "failed",
			}),
		).toThrow();
	});

	it("does not accept any failure as the requested named refusal", () => {
		const stdout = JSON.stringify({
			ok: false,
			command: "pij watchdog",
			v: 2,
			details: { code: "E-RS-AUTH" },
		});
		expect(() =>
			parseSmokeEnvelope({ status: 4, stdout, stderr: "" }, false, "E-RS-UNPORTED"),
		).toThrow();
		expect(() =>
			parseSmokeEnvelope({ status: 0, stdout, stderr: "" }, false, "E-RS-AUTH"),
		).toThrow();
		expect(() =>
			parseSmokeEnvelope({ status: null, stdout, stderr: "signal" }, false, "E-RS-AUTH"),
		).toThrow();
	});
});

describe("resolveSmokeCommand", () => {
	it("sorts and safely quotes every supplied project-local extension", () => {
		expect(
			resolveSmokeCommand({}, [
				"/workspace/z-last/index.ts",
				"/workspace/author's/index.ts",
				"/workspace/a first/index.ts",
			]),
		).toBe(
			`omp --auto-approve --no-extensions --extension '/workspace/a first/index.ts' --extension '/workspace/author'"'"'s/index.ts' --extension '/workspace/z-last/index.ts'`,
		);
	});

	it("preserves an explicit scenario command byte-for-byte", () => {
		const command = "custom-pi  --flag='two words'  ";

		expect(resolveSmokeCommand({ cmd: command }, ["/ignored/index.ts"])).toBe(command);
	});
});

describe("findProjectExtensionEntries", () => {
	it("returns the complete sorted top-level index inventory without a machine-specific root", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-smoke-inventory-"));
		cleanupPaths.push(root);
		const extensionsRoot = join(root, ".omp", "extensions");
		const expected = [
			join(extensionsRoot, "a first", "index.ts"),
			join(extensionsRoot, "author's", "index.ts"),
			join(extensionsRoot, "z-last", "index.ts"),
		].sort();

		for (const path of expected) {
			mkdirSync(join(path, ".."), { recursive: true });
			writeFileSync(path, "export default () => {};\n");
		}
		mkdirSync(join(extensionsRoot, "missing-index"), { recursive: true });
		mkdirSync(join(extensionsRoot, "nested", "child"), { recursive: true });
		writeFileSync(
			join(extensionsRoot, "nested", "child", "index.ts"),
			"export default () => {};\n",
		);

		expect(findProjectExtensionEntries(extensionsRoot)).toEqual(expected);
	});
});

describe("smoke runner imports", () => {
	it("does not execute scenarios when the resolver is imported", () => {
		expect(loadScenario).not.toHaveBeenCalled();
		expect(runScenario).not.toHaveBeenCalled();
	});
});
