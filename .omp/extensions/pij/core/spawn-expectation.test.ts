import { describe, expect, it } from "vitest";
import {
	bindSpawnExpectation,
	createSpawnExpectation,
	DEFAULT_SPAWN_EXPECTATION_TTL_MS,
	spawnExpectationDeadline,
} from "./spawn-expectation.js";

describe("spawn expectation reducer", () => {
	const createdAt = "2026-07-20T00:00:00.000Z";

	it("pins the named five-minute registration boundary and exact deadline", () => {
		expect(DEFAULT_SPAWN_EXPECTATION_TTL_MS).toBe(300_000);
		expect(spawnExpectationDeadline(createdAt)).toBe("2026-07-20T00:05:00.000Z");
	});

	it("binds a successor without terminalizing the predecessor", () => {
		const expectation = createSpawnExpectation({
			spawnId: "s-3",
			creatorId: "pij-parent",
			requestedHarness: "pi",
			requestedAt: createdAt,
		});
		const bound = bindSpawnExpectation(expectation, {
			sessionId: "pij-child",
			paneId: "%4",
			runtimeHarness: "pi",
			boundAt: "2026-07-20T00:00:01.000Z",
		});
		expect(bound).toMatchObject({ sessionId: "pij-child", runtimeHarness: "pi" });
		expect(bound.terminal).toBeUndefined();
	});
});
