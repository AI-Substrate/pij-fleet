import { describe, expect, it } from "vitest";
import { parseTimeScale, scaled } from "./time-scale.js";

describe("PIJ_TEST_TIME_SCALE", () => {
	it("is 1 unset or blank, stretches by a number of at least 1, and refuses anything else", () => {
		expect(parseTimeScale(undefined)).toBe(1);
		expect(parseTimeScale(" ")).toBe(1);
		expect(scaled(3_000, { PIJ_TEST_TIME_SCALE: "2.5" })).toBe(7_500);
		expect(scaled(3_000, {})).toBe(3_000);
		for (const bad of ["0.5", "0", "-2", "fast", "Infinity"])
			expect(() => parseTimeScale(bad)).toThrow(/PIJ_TEST_TIME_SCALE must be a number >= 1/);
	});
});
