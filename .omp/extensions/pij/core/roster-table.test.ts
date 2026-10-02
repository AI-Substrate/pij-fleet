import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { findRoute, renderRsAnswer } from "./generation-routing.js";
import { renderRosterTable } from "./roster-table.js";

const golden = (name: string): string =>
	readFileSync(
		new URL(`../../../../crates/testkit/fixtures/golden/cli/${name}`, import.meta.url),
		"utf8",
	);

describe("pij list table (plan 160)", () => {
	it("renders the same golden as the Rust CLI from the same JSON", () => {
		expect(renderRosterTable(JSON.parse(golden("list-sized.json")))).toBe(golden("list-sized.txt"));
	});

	it("is what the shim prints for a human `pij list`", () => {
		const payload = JSON.parse(golden("list-sized.json"));
		expect(renderRsAnswer(findRoute("list", undefined), payload, ["list"])).toEqual({
			kind: "rendered",
			text: golden("list-sized.txt"),
		});
	});

	it("appends the daemon's size lines to a human `pij state`", () => {
		const payload = {
			id: "pij-cold",
			state: "idle",
			liveness: "alive",
			sizeLines: [
				"context 720k / 1M · last call 2h ago · cache 1h (cold 1h) · 3 compactions",
				"❄ cold-wake guard: a normal send is refused; waking it costs ~$6.45 (--fyi holds it for $0 now)",
			],
		};
		const rendered = renderRsAnswer(findRoute("state", undefined), payload, ["state", "pij-cold"]);
		expect(rendered).toEqual({
			kind: "rendered",
			text: `pij-cold: idle · alive\n${payload.sizeLines.join("\n")}`,
		});
	});
});
