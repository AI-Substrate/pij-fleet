import { describe, expect, it } from "vitest";
import {
	applyTerminalObservation,
	bindSpawnExpectation,
	createSpawnExpectation,
	DEFAULT_SPAWN_EXPECTATION_TTL_MS,
	requestClose,
	spawnExpectationDeadline,
} from "./spawn-expectation.js";

describe("spawn expectation reducer", () => {
	const createdAt = "2026-07-20T00:00:00.000Z";

	it("pins the named five-minute registration boundary and exact deadline", () => {
		expect(DEFAULT_SPAWN_EXPECTATION_TTL_MS).toBe(300_000);
		expect(spawnExpectationDeadline(createdAt)).toBe("2026-07-20T00:05:00.000Z");
	});

	it("keeps a pre-register no-show honest: requested harness is known, runtime harness is unavailable", () => {
		const expectation = createSpawnExpectation({
			spawnId: "s-1",
			creatorId: "pij-parent",
			requestedHarness: "pi",
			requestedAt: createdAt,
		});
		const terminal = applyTerminalObservation(expectation, {
			kind: "absent",
			observedAt: "2026-07-20T00:00:02.000Z",
			evidence: "pane-missing",
		});
		expect(terminal.terminal).toEqual({
			disposition: "unrequested-by-pij",
			observedAt: "2026-07-20T00:00:02.000Z",
			evidence: "pane-missing",
		});
		expect(terminal.runtimeHarness).toBeUndefined();
	});

	it("writes a close intent before terminalizing an observed absence", () => {
		const expectation = createSpawnExpectation({
			spawnId: "s-2",
			creatorId: "pij-parent",
			requestedHarness: "claude",
			requestedAt: createdAt,
		});
		const requested = requestClose(expectation, {
			actor: "pij-parent",
			kind: "cli-close",
			requestedAt: "2026-07-20T00:00:01.000Z",
		});
		const terminal = applyTerminalObservation(requested, {
			kind: "absent",
			observedAt: "2026-07-20T00:00:02.000Z",
			evidence: "pid-missing",
			lastSeenAt: "2026-07-20T00:00:01.500Z",
		});
		expect(terminal.terminal).toMatchObject({
			disposition: "requested",
			lastSeenAt: "2026-07-20T00:00:01.500Z",
		});
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
