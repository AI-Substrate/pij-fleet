import { describe, expect, it } from "vitest";

import { SeqCounter } from "./seq.js";

describe("SeqCounter", () => {
	it("starts at 0 and increments strictly", () => {
		const s = new SeqCounter();
		expect(s.peek()).toBe(0);
		expect(s.next()).toBe(1);
		expect(s.next()).toBe(2);
		expect(s.peek()).toBe(2);
	});

	it("recovers from a persisted lastSeq (crash-safe after /reload)", () => {
		const s = new SeqCounter(7);
		expect(s.peek()).toBe(7);
		expect(s.next()).toBe(8);
	});

	it("never repeats a seq across recovery", () => {
		const first = new SeqCounter();
		first.next();
		first.next();
		const recovered = new SeqCounter(first.peek());
		expect(recovered.next()).toBe(3);
	});
});
