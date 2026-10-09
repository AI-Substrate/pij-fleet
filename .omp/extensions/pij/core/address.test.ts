import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parseDestination, renderDestination } from "./address.js";
import { senderLabel } from "./message.js";

/** Shared with crates/core/src/address.rs so the TS and Rust grammars cannot drift. */
type Case =
	| { readonly input: string; readonly seat: string; readonly machine: string | null }
	| { readonly input: string; readonly error: true };

const cases = JSON.parse(
	readFileSync(
		new URL("../../../../crates/testkit/fixtures/golden/address/cases.json", import.meta.url),
		"utf8",
	),
) as readonly Case[];
const valid = cases.flatMap((c) => ("error" in c ? [] : [c]));
const invalid = cases.flatMap((c) => ("error" in c ? [c] : []));

describe("parseDestination (golden address cases)", () => {
	it.each(valid)("parses $input as seat $seat, machine $machine", ({ input, seat, machine }) => {
		const parsed = parseDestination(input);
		expect(parsed).toEqual({ ok: true, value: machine === null ? { seat } : { seat, machine } });
	});

	it.each(invalid)("refuses $input", ({ input }) => {
		expect(parseDestination(input)).toMatchObject({ ok: false, code: "E-ARG" });
	});

	it.each(valid)("renders $input back to the same destination", ({ input }) => {
		const parsed = parseDestination(input);
		if (!parsed.ok) throw new Error(parsed.message);
		expect(parseDestination(renderDestination(parsed.value))).toEqual(parsed);
	});

	it.each(valid)("a sender shown as senderLabel parses back to its seat and machine", ({
		seat,
		machine,
	}) => {
		const label = senderLabel(seat, machine ?? undefined);
		expect(parseDestination(label)).toEqual({
			ok: true,
			value: machine === null ? { seat } : { seat, machine },
		});
	});
});
