// Durable, harness-neutral spawn intent and terminal-observation reducer.
// This module is Pi-free: all callers share the same no-show classification.

import type {
	CloseIntent,
	HarnessKind,
	SpawnExpectation,
	TerminalDisposition,
	TerminalEvidence,
} from "./types.js";

/** Registration is bounded so a missing self-registration becomes a durable
 * no-show observation rather than an immortal pending intent. */
export const DEFAULT_SPAWN_EXPECTATION_TTL_MS = 5 * 60 * 1000;

export interface CreateSpawnExpectationInput {
	readonly spawnId: string;
	readonly creatorId?: string;
	readonly requestedHarness: HarnessKind | "omp";
	readonly requestedAt: string;
	/** Producer-stamped deadline; callers use {@link spawnExpectationDeadline}.
	 * Optional only for legacy fixture compatibility; production callers pass it. */
	readonly deadlineAt?: string;
	readonly paneId?: string;
}

export function spawnExpectationDeadline(
	requestedAt: string,
	ttlMs = DEFAULT_SPAWN_EXPECTATION_TTL_MS,
): string {
	return new Date(Date.parse(requestedAt) + ttlMs).toISOString();
}

export interface BindSpawnExpectationInput {
	readonly sessionId: string;
	readonly paneId?: string;
	readonly runtimeHarness: HarnessKind;
	readonly boundAt: string;
}

export type TerminalObservation =
	| {
			readonly kind: "absent";
			readonly observedAt: string;
			readonly evidence: TerminalEvidence;
			readonly lastSeenAt?: string;
	  }
	| {
			readonly kind: "unavailable";
			readonly observedAt: string;
			readonly evidence: TerminalEvidence;
			readonly reason: string;
			readonly lastSeenAt?: string;
	  };

export function createSpawnExpectation(input: CreateSpawnExpectationInput): SpawnExpectation {
	return {
		spawnId: input.spawnId,
		requestedHarness: input.requestedHarness,
		requestedAt: input.requestedAt,
		deadlineAt: input.deadlineAt ?? spawnExpectationDeadline(input.requestedAt),
		...(input.creatorId !== undefined ? { creatorId: input.creatorId } : {}),
		...(input.paneId !== undefined ? { paneId: input.paneId } : {}),
	};
}

export function bindSpawnExpectation(
	expectation: SpawnExpectation,
	input: BindSpawnExpectationInput,
): SpawnExpectation {
	return {
		...expectation,
		sessionId: input.sessionId,
		runtimeHarness: input.runtimeHarness,
		boundAt: input.boundAt,
		...(input.paneId !== undefined ? { paneId: input.paneId } : {}),
	};
}

/** Persist this result before a pij-owned pane kill or descriptor dissolve. */
export function requestClose(
	expectation: SpawnExpectation,
	closeIntent: CloseIntent,
): SpawnExpectation {
	return { ...expectation, closeIntent };
}

export function applyTerminalObservation(
	expectation: SpawnExpectation,
	observation: TerminalObservation,
): SpawnExpectation {
	if (expectation.terminal !== undefined) return expectation;
	const disposition: TerminalDisposition =
		observation.kind === "unavailable"
			? "unavailable"
			: expectation.closeIntent === undefined
				? "unrequested-by-pij"
				: "requested";
	return {
		...expectation,
		terminal: {
			disposition,
			observedAt: observation.observedAt,
			evidence: observation.evidence,
			...(observation.lastSeenAt !== undefined ? { lastSeenAt: observation.lastSeenAt } : {}),
			...(observation.kind === "unavailable" ? { unavailableReason: observation.reason } : {}),
		},
	};
}

export function latchTerminalNotice(expectation: SpawnExpectation, at: string): SpawnExpectation {
	return expectation.deathNoticeLatchedAt === undefined
		? { ...expectation, deathNoticeLatchedAt: at }
		: expectation;
}
