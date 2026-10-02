import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
	ALLOWED_COMMANDS,
	CONTROL_COMMANDS,
	COPILOT_CONTROL_REFUSAL,
	isControlCommand,
	validateCommand,
} from "./commands.js";

describe("validateCommand", () => {
	it("reads the Rust-owned command and refusal fixture", () => {
		const contract = JSON.parse(
			readFileSync(
				new URL("../../../../crates/core/tests/fixtures/control-commands.json", import.meta.url),
				"utf8",
			),
		);
		expect(ALLOWED_COMMANDS).toEqual(contract.commands);
		expect(COPILOT_CONTROL_REFUSAL).toBe(contract.copilot_refusal);
	});
	it("accepts compact", () => {
		const r = validateCommand("compact");
		expect(r.ok).toBe(true);
		if (r.ok) expect(r.value).toBe("compact");
	});

	it("accepts new and reload", () => {
		for (const c of ["new", "reload"]) {
			const r = validateCommand(c);
			expect(r.ok).toBe(true);
			if (r.ok) expect(r.value).toBe(c);
		}
	});

	it("rejects unknown commands with E-CMD", () => {
		const r = validateCommand("rm-rf");
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.code).toBe("E-CMD");
	});

	it("exposes the allow-list", () => {
		expect(ALLOWED_COMMANDS).toContain("compact");
		expect(ALLOWED_COMMANDS).toContain("new");
		expect(ALLOWED_COMMANDS).toContain("reload");
	});

	it("classifies new/reload as control, compact as not", () => {
		expect(CONTROL_COMMANDS).toEqual(["new", "reload"]);
		expect(isControlCommand("new")).toBe(true);
		expect(isControlCommand("reload")).toBe(true);
		expect(isControlCommand("compact")).toBe(false);
	});
});
