import { describe, expect, it } from "vitest";

import { guardInvariantNineModal, INVARIANT_NINE_MODAL_REASON } from "./invariant-guard.js";
import type { SessionDescriptor } from "./types.js";

function descriptor(overrides: Partial<SessionDescriptor> = {}): SessionDescriptor {
	return {
		id: "pij-test",
		folder: "/project",
		dataDir: "/pij/pij-test",
		eventsPath: "/pij/pij-test/events.ndjson",
		pid: 1,
		startedAt: "2026-07-20T00:00:00.000Z",
		...overrides,
	};
}

describe("guardInvariantNineModal", () => {
	it("blocks the exact modal tool for every structurally managed Pi peer", () => {
		for (const managed of [
			descriptor({ harness: "pi", parentId: null }),
			descriptor({ harness: "pi", spawnedBy: "pij-parent" }),
			descriptor({ harness: "pi", prime: true }),
			descriptor({ harness: "pi", oldPrime: true }),
		]) {
			expect(guardInvariantNineModal(managed, "ask_user_question")).toEqual({
				block: true,
				reason: INVARIANT_NINE_MODAL_REASON,
			});
		}
	});

	it("does not block generic or non-Pi sessions, or a different tool", () => {
		expect(guardInvariantNineModal(undefined, "ask_user_question")).toBeUndefined();
		expect(guardInvariantNineModal(null, "ask_user_question")).toBeUndefined();
		expect(
			guardInvariantNineModal(descriptor({ harness: "pi" }), "ask_user_question"),
		).toBeUndefined();
		expect(
			guardInvariantNineModal(
				descriptor({ harness: "claude", parentId: "pij-parent" }),
				"ask_user_question",
			),
		).toBeUndefined();
		expect(
			guardInvariantNineModal(descriptor({ harness: "pi", prime: true }), "pij_send"),
		).toBeUndefined();
	});
});
