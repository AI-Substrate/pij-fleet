// The write law's own semantics (plan 071 review round 1, §1.2).

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { applyWriteLaw, DESCRIPTOR_FIELD_OWNER } from "./registry-write.js";
import type { SessionDescriptor } from "./types.js";

function descriptor(over: Partial<SessionDescriptor> = {}): SessionDescriptor {
	return {
		id: "pij-node",
		folder: "/repo",
		dataDir: "/home/.pij/pij-node",
		eventsPath: "/home/.pij/pij-node/events.ndjson",
		pid: 100,
		startedAt: "2026-07-25T11:00:00.000Z",
		...over,
	};
}

describe("applyWriteLaw", () => {
	it("a brand-new descriptor is published verbatim — nothing to merge against", () => {
		const fresh = descriptor({ parentId: "pij-boss", prime: true });
		expect(applyWriteLaw(fresh, null)).toEqual(fresh);
	});

	// The bug class: a stale snapshot replaying over a field someone else stamped.
	it("restores a contested field the writer does not own from disk", () => {
		const stale = descriptor({ terminal: undefined });
		const disk = descriptor({
			terminal: { disposition: "requested", observedAt: "x", evidence: "pane-missing" },
		});
		expect(applyWriteLaw(stale, disk, "daemon").terminal).toEqual(disk.terminal);
	});

	// THE INVERSION TRAP: if the law were symmetric, an owner would re-read its own
	// field from disk and drop the value it just computed.
	it("lets the OWNER's value win — the trap the daemon-centric framing would create", () => {
		const proposed = descriptor({ currentAssignment: "asg-new" });
		const disk = descriptor({ currentAssignment: "asg-old" });
		expect(applyWriteLaw(proposed, disk, "cli").currentAssignment).toBe("asg-new");
		// …and a NON-owner writing the same field still yields disk's value.
		expect(applyWriteLaw(proposed, disk, "daemon").currentAssignment).toBe("asg-old");
	});

	it("AC-01: stateNote remains CLI-owned across daemon replay", () => {
		const proposed = descriptor({
			stateNote: {
				text: "waiting for review",
				state: "waiting",
				at: "2026-07-29T01:00:00.000Z",
			},
		});
		const disk = descriptor({
			stateNote: {
				text: "blocked on CI",
				state: "blocked",
				at: "2026-07-29T00:00:00.000Z",
			},
		});
		expect(applyWriteLaw(proposed, disk, "cli").stateNote).toEqual(proposed.stateNote);
		expect(applyWriteLaw(proposed, disk, "daemon").stateNote).toEqual(disk.stateNote);
	});

	it("AC-01: statusPrev remains CLI-owned across daemon replay", () => {
		const proposed = descriptor({ statusPrev: "working" });
		const disk = descriptor({ statusPrev: "idle" });
		expect(applyWriteLaw(proposed, disk, "cli").statusPrev).toBe("working");
		expect(applyWriteLaw(proposed, disk, "daemon").statusPrev).toBe("idle");
	});

	it("AC-01: statusNext remains CLI-owned across daemon replay", () => {
		const proposed = descriptor({ statusNext: "done" });
		const disk = descriptor({ statusNext: "working" });
		expect(applyWriteLaw(proposed, disk, "cli").statusNext).toBe("done");
		expect(applyWriteLaw(proposed, disk, "daemon").statusNext).toBe("working");
	});

	it("AC-01: statusAt remains CLI-owned across daemon replay", () => {
		const proposed = descriptor({ statusAt: "2026-07-29T02:00:00.000Z" });
		const disk = descriptor({ statusAt: "2026-07-29T01:00:00.000Z" });
		expect(applyWriteLaw(proposed, disk, "cli").statusAt).toBe("2026-07-29T02:00:00.000Z");
		expect(applyWriteLaw(proposed, disk, "daemon").statusAt).toBe("2026-07-29T01:00:00.000Z");
	});

	it("AC-01: statusSeq remains CLI-owned across daemon replay", () => {
		const proposed = descriptor({ statusSeq: 8 });
		const disk = descriptor({ statusSeq: 7 });
		expect(applyWriteLaw(proposed, disk, "cli").statusSeq).toBe(8);
		expect(applyWriteLaw(proposed, disk, "daemon").statusSeq).toBe(7);
	});

	it("AC-01: orchestrationRole remains CLI-owned across daemon replay", () => {
		const proposed = descriptor({ orchestrationRole: "worker" });
		const disk = descriptor({ orchestrationRole: "pm" });
		expect(applyWriteLaw(proposed, disk, "cli").orchestrationRole).toBe("worker");
		expect(applyWriteLaw(proposed, disk, "daemon").orchestrationRole).toBe("pm");
	});

	it("an undeclared writer owns nothing and can never clobber a contested field", () => {
		const proposed = descriptor({ prime: false, terminal: undefined });
		const disk = descriptor({
			prime: true,
			terminal: { disposition: "requested", observedAt: "x", evidence: "pane-missing" },
		});
		const merged = applyWriteLaw(proposed, disk);
		expect(merged.prime).toBe(true);
		expect(merged.terminal).toEqual(disk.terminal);
	});

	// The boundary that matters: "disk wins even when absent" would break every
	// two-phase publish (spawn writes, then writes again with its parent link).
	it("a writer may SET a contested field that disk does not have", () => {
		const proposed = descriptor({ parentId: "pij-boss" });
		expect(applyWriteLaw(proposed, descriptor(), "daemon").parentId).toBe("pij-boss");
	});

	it("append-only reportedAt fills a gap but never overwrites a value the writer holds", () => {
		const withValue = descriptor({ reportedAt: "2026-07-25T13:00:00.000Z" });
		const disk = descriptor({ reportedAt: "2026-07-25T12:00:00.000Z" });
		// Writer has one → keeps it (AC-16 idempotence).
		expect(applyWriteLaw(withValue, disk, "daemon").reportedAt).toBe("2026-07-25T13:00:00.000Z");
		// Writer has none → fills from disk.
		expect(applyWriteLaw(descriptor(), disk, "daemon").reportedAt).toBe(disk.reportedAt);
	});

	it("leaves uncontested fields entirely alone", () => {
		const proposed = descriptor({ state: "working", lastEventAt: "2026-07-25T12:00:00.000Z" });
		const disk = descriptor({ state: "idle", lastEventAt: "2026-07-25T09:00:00.000Z" });
		const merged = applyWriteLaw(proposed, disk, "daemon");
		expect(merged.state).toBe("working");
		expect(merged.lastEventAt).toBe("2026-07-25T12:00:00.000Z");
	});

	it("every contested field names exactly one owner", () => {
		for (const [field, owner] of Object.entries(DESCRIPTOR_FIELD_OWNER)) {
			expect(["daemon", "cli", "seat", "close"], `${field} has an unknown owner`).toContain(owner);
		}
		// The incidents this law was built from must stay covered.
		expect(DESCRIPTOR_FIELD_OWNER.terminal).toBe("close");
		expect(DESCRIPTOR_FIELD_OWNER.closeIntent).toBe("close");
		expect(DESCRIPTOR_FIELD_OWNER.currentAssignment).toBe("cli");
	});
});

// Review round 2 §MED-a — "omitting the authority is always safe" was wrong, and
// the imprecise wording shipped a bug. Omitting is safe for OTHER writers' data
// and SILENTLY LOSSY for your own.
describe("omitting the authority is lossy for your own contested fields", () => {
	it("a second non-owner write cannot replace an existing planId", () => {
		const disk = descriptor({ planId: "073-pij-first-class-ui" });
		const staleSeatWrite = descriptor({ planId: "071-detection-integrity" });

		expect(applyWriteLaw(staleSeatWrite, disk, "seat").planId).toBe("073-pij-first-class-ui");
		expect(
			applyWriteLaw(descriptor({ planId: "074-corrected-attestation" }), disk, "cli").planId,
		).toBe("074-corrected-attestation");
	});

	it("discards a contested field you are trying to SET when disk already has one", () => {
		const reparent = descriptor({ parentId: "pij-new-boss" });
		const disk = descriptor({ parentId: "pij-old-boss" });

		// Undeclared: the re-parent silently pins to the OLD value — no error, no log.
		expect(applyWriteLaw(reparent, disk).parentId).toBe("pij-old-boss");
		// Declared: the verb does what it was asked to do.
		expect(applyWriteLaw(reparent, disk, "cli").parentId).toBe("pij-new-boss");
	});

	it("the loss is invisible on a FRESH record — which is why it hid in spawn", () => {
		// Disk has no parentId, so an undeclared write appears to work perfectly.
		expect(applyWriteLaw(descriptor({ parentId: "pij-boss" }), descriptor()).parentId).toBe(
			"pij-boss",
		);
	});

	it("the law's own documentation must not call omission 'always safe'", () => {
		// The rule failed one level up once already: in its own docstring.
		const law = readFileSync(new URL("./registry-write.ts", import.meta.url), "utf8");
		expect(law).not.toMatch(/omitting it is always\s+\*?\s*safe/);
		expect(law).toContain("SILENTLY LOSSY");
	});
});
