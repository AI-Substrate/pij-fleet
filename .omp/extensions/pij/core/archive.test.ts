import { describe, expect, it } from "vitest";
import {
	ARCHIVE_AFTER_MS,
	ARCHIVE_PRUNE_AFTER_MS,
	archiveAgeAnchorMs,
	buildArchiveIndexEntry,
	classifyRegistryRecord,
	isPrunableArchiveRecord,
	isTerminalRecord,
	lastActivityAtMs,
	parseArchiveIndexLine,
	REVIVE_GRACE_MS,
} from "./archive.js";
import type { SessionDescriptor } from "./types.js";

const NOW = Date.parse("2026-07-25T12:00:00.000Z");
const LONG_AGO = new Date(NOW - ARCHIVE_AFTER_MS - 60_000).toISOString();
const RECENTLY = new Date(NOW - 60_000).toISOString();

function descriptor(over: Partial<SessionDescriptor> = {}): SessionDescriptor {
	return {
		id: "pij-test-peer",
		folder: "/repo",
		dataDir: "/home/.pij/pij-test-peer",
		eventsPath: "/home/.pij/pij-test-peer/events.ndjson",
		pid: 4242,
		startedAt: LONG_AGO,
		...over,
	};
}

describe("isTerminalRecord", () => {
	it("counts dissolved and failed as terminal", () => {
		expect(isTerminalRecord(descriptor({ lifecycle: "dissolved" }))).toBe(true);
		expect(isTerminalRecord(descriptor({ lifecycle: "failed" }))).toBe(true);
	});

	it("counts every live lifecycle — and a legacy absent one — as non-terminal", () => {
		for (const lifecycle of ["pending", "ready", "bound"] as const) {
			expect(isTerminalRecord(descriptor({ lifecycle }))).toBe(false);
		}
		expect(isTerminalRecord(descriptor())).toBe(false);
	});
});

const DIED = new Date(NOW - 60 * 60_000).toISOString(); // one hour ago
const terminalAt = (observedAt: string): Partial<SessionDescriptor> => ({
	lifecycle: "dissolved",
	terminal: { disposition: "requested", observedAt, evidence: "pane-missing" },
});

describe("lastActivityAtMs — the ACTIVITY axis", () => {
	it("takes the NEWEST parseable stamp, not the first present one", () => {
		expect(lastActivityAtMs(descriptor({ startedAt: LONG_AGO, lastEventAt: RECENTLY }))).toBe(
			Date.parse(RECENTLY),
		);
	});

	it("falls back past unparseable stamps to one that parses", () => {
		expect(lastActivityAtMs(descriptor({ lastEventAt: "not-a-date", startedAt: LONG_AGO }))).toBe(
			Date.parse(LONG_AGO),
		);
	});

	it("returns null when nothing parses", () => {
		expect(lastActivityAtMs(descriptor({ startedAt: "garbage" }))).toBeNull();
	});

	// pij#204. `lastTickAt` is overlaid by `read()`/`list()` and scrubbed on
	// terminal reads, so a function that consults it answers differently depending
	// on how its caller obtained the descriptor — same type, different value. This
	// asserts the field is INERT, which is the property that makes this function
	// provenance-independent.
	//
	// It also replaces a fixture that had quietly stopped testing anything: the old
	// case passed `lastTickAt: LONG_AGO` beside `lastEventAt: RECENTLY` and asserted
	// RECENTLY won — which stayed true after the field was dropped, so the test
	// went on passing while no longer exercising the stamp it named.
	it("IGNORES lastTickAt entirely, even when it is the newest stamp", () => {
		const withTick = descriptor({ startedAt: LONG_AGO, lastTickAt: RECENTLY });
		expect(lastActivityAtMs(withTick)).toBe(Date.parse(LONG_AGO));
		// …and the same descriptor without it answers identically. THAT equality is
		// the provenance guarantee: a raw read and an overlaid read cannot diverge.
		expect(lastActivityAtMs(descriptor({ startedAt: LONG_AGO }))).toBe(lastActivityAtMs(withTick));
	});
});

describe("archiveAgeAnchorMs — the DEATH axis (pij#204, Jordan-ruled 2026-08-09)", () => {
	// ARCHIVE_AFTER_MS promises a terminal record stays hot 48h so a human can read
	// the wreckage. Anchored on ACTIVITY it did not deliver that, and failed in the
	// direction that destroys evidence: measured live, `pij-straight-araminta` was
	// 124.8h old by activity and had died 0.8h earlier — already past the window it
	// was promised, archived within the hour of dying.
	it("anchors a terminal record on WHEN IT DIED, not when it last worked", () => {
		const d = descriptor({ ...terminalAt(DIED), startedAt: LONG_AGO, lastEventAt: LONG_AGO });
		expect(archiveAgeAnchorMs(d)).toBe(Date.parse(DIED));
	});

	it("keeps a long-lived seat that JUST died hot for its full window", () => {
		// The measured case above, as a classification rather than a timestamp.
		const d = descriptor({ ...terminalAt(DIED), startedAt: LONG_AGO, lastEventAt: LONG_AGO });
		expect(classifyRegistryRecord(d, NOW)).toBe("hot");
	});

	it("still archives it once the window has elapsed since death", () => {
		const d = descriptor({ ...terminalAt(LONG_AGO), startedAt: LONG_AGO, lastEventAt: LONG_AGO });
		expect(classifyRegistryRecord(d, NOW)).toBe("archivable");
	});

	// The fallback is an explicit branch, not a `??`: "no death stamp" and "died at
	// the epoch" must not share an answer, and nothing may become IMMORTAL by
	// lacking a field.
	it("falls back to ACTIVITY for a legacy record with no death stamp", () => {
		const d = descriptor({ lifecycle: "dissolved", startedAt: LONG_AGO, lastEventAt: LONG_AGO });
		expect(archiveAgeAnchorMs(d)).toBe(Date.parse(LONG_AGO));
		expect(classifyRegistryRecord(d, NOW)).toBe("archivable");
	});

	it("falls back to ACTIVITY when the death stamp is unparseable", () => {
		const d = descriptor({ ...terminalAt("not-a-date"), startedAt: LONG_AGO });
		expect(archiveAgeAnchorMs(d)).toBe(Date.parse(LONG_AGO));
	});

	it("returns null when nothing parses at all", () => {
		expect(archiveAgeAnchorMs(descriptor({ startedAt: "garbage" }))).toBeNull();
	});

	it("IGNORES lastTickAt, so the decision cannot vary with the caller's read path", () => {
		const d = descriptor({ ...terminalAt(DIED), startedAt: LONG_AGO, lastTickAt: RECENTLY });
		expect(archiveAgeAnchorMs(d)).toBe(Date.parse(DIED));
	});
});

describe("classifyRegistryRecord", () => {
	it("archives a terminal record older than the 48h window", () => {
		expect(classifyRegistryRecord(descriptor({ lifecycle: "dissolved" }), NOW)).toBe("archivable");
		expect(classifyRegistryRecord(descriptor({ lifecycle: "failed" }), NOW)).toBe("archivable");
	});

	it("keeps a terminal record hot inside the 48h window", () => {
		const fresh = descriptor({ lifecycle: "dissolved", lastEventAt: RECENTLY });
		expect(classifyRegistryRecord(fresh, NOW)).toBe("hot");
	});

	it("keeps a terminal record hot at EXACTLY the boundary (strictly-older archives)", () => {
		const at = descriptor({
			lifecycle: "failed",
			startedAt: new Date(NOW - ARCHIVE_AFTER_MS).toISOString(),
		});
		expect(classifyRegistryRecord(at, NOW)).toBe("archivable");
		const justInside = descriptor({
			lifecycle: "failed",
			startedAt: new Date(NOW - ARCHIVE_AFTER_MS + 1).toISOString(),
		});
		expect(classifyRegistryRecord(justInside, NOW)).toBe("hot");
	});

	// The load-bearing safety property: never move a seat that could still work.
	it("NEVER archives a live lifecycle, however stale", () => {
		const ancient = new Date(NOW - ARCHIVE_AFTER_MS * 100).toISOString();
		for (const lifecycle of ["pending", "ready", "bound"] as const) {
			const stale = descriptor({ lifecycle, startedAt: ancient, lastEventAt: ancient });
			expect(classifyRegistryRecord(stale, NOW)).toBe("hot");
		}
	});

	it("NEVER archives a legacy descriptor that carries no lifecycle at all", () => {
		expect(classifyRegistryRecord(descriptor({ lifecycle: undefined }), NOW)).toBe("hot");
	});

	it("keeps an unprovable-age terminal record hot rather than guessing", () => {
		const unparseable = descriptor({ lifecycle: "dissolved", startedAt: "¯\\_(ツ)_/¯" });
		expect(classifyRegistryRecord(unparseable, NOW)).toBe("hot");
	});

	it("keeps a future-dated terminal record hot (clock skew is not age)", () => {
		const future = descriptor({
			lifecycle: "dissolved",
			startedAt: new Date(NOW + ARCHIVE_AFTER_MS).toISOString(),
		});
		expect(classifyRegistryRecord(future, NOW)).toBe("hot");
	});
});

describe("archive index entries", () => {
	it("denormalises the fields a listing needs without reopening the descriptor", () => {
		const entry = buildArchiveIndexEntry(
			descriptor({
				lifecycle: "failed",
				failureReason: "bind-timeout",
				harness: "claude",
				folder: "/repo/worktree",
				lastEventAt: LONG_AGO,
			}),
			NOW,
		);
		expect(entry).toMatchObject({
			id: "pij-test-peer",
			archivedAt: new Date(NOW).toISOString(),
			lifecycle: "failed",
			failureReason: "bind-timeout",
			harness: "claude",
			folder: "/repo/worktree",
			lastActivityAt: LONG_AGO,
		});
	});

	it("omits a lastActivityAt it cannot compute", () => {
		const entry = buildArchiveIndexEntry(
			descriptor({ lifecycle: "dissolved", startedAt: "nope" }),
			NOW,
		);
		expect(entry.lastActivityAt).toBeUndefined();
	});

	it("round-trips through the jsonl line format", () => {
		const entry = buildArchiveIndexEntry(descriptor({ lifecycle: "dissolved" }), NOW);
		expect(parseArchiveIndexLine(JSON.stringify(entry))).toEqual(entry);
	});

	it("returns null for blank, corrupt, and shape-invalid lines (a torn tail loses one row, not the listing)", () => {
		expect(parseArchiveIndexLine("")).toBeNull();
		expect(parseArchiveIndexLine("   ")).toBeNull();
		expect(parseArchiveIndexLine('{"id":"half-writ')).toBeNull();
		expect(parseArchiveIndexLine('{"archivedAt":"2026-07-25T12:00:00.000Z"}')).toBeNull();
		expect(parseArchiveIndexLine('{"id":"x"}')).toBeNull();
		expect(parseArchiveIndexLine("[1,2,3]")).toBeNull();
	});
});

// Review round 1 §2.1 — a revive in flight must be invisible to the janitor.
// The seats worth reviving are >=48h old by definition, so age alone can never
// distinguish "dead for two days" from "dead for two days and booting right now".
describe("revive-in-flight exemption", () => {
	it("keeps a dissolved record HOT while its revive marker is fresh", () => {
		const reviving = descriptor({
			lifecycle: "dissolved",
			revivePendingAt: new Date(NOW - 5_000).toISOString(),
		});
		expect(classifyRegistryRecord(reviving, NOW)).toBe("hot");
	});

	// CONTROL: byte-identical record WITHOUT the marker is archivable, so the
	// exemption is doing the work and not some unrelated age quirk.
	it("control — the same record without the marker is archivable", () => {
		expect(classifyRegistryRecord(descriptor({ lifecycle: "dissolved" }), NOW)).toBe("archivable");
	});

	it("stops protecting once the grace window passes — the marker cannot pin a record forever", () => {
		const stale = descriptor({
			lifecycle: "dissolved",
			revivePendingAt: new Date(NOW - REVIVE_GRACE_MS - 1).toISOString(),
		});
		expect(classifyRegistryRecord(stale, NOW)).toBe("archivable");
	});

	it("ignores an unparseable or future-dated marker rather than trusting it", () => {
		expect(
			classifyRegistryRecord(descriptor({ lifecycle: "dissolved", revivePendingAt: "soon" }), NOW),
		).toBe("archivable");
		expect(
			classifyRegistryRecord(
				descriptor({
					lifecycle: "dissolved",
					revivePendingAt: new Date(NOW + 60_000).toISOString(),
				}),
				NOW,
			),
		).toBe("archivable");
	});
});

// ─────────────────────────────────────────────────────────────────────────────
// pij#204 / Jordan-ruled 2026-08-09 — the index carries TWO honest fields.
//
// One field was doing both jobs and doing neither honestly: labelled
// `lastActivityAt` while actually carrying the archival decision's anchor, and
// written from a RAW descriptor whose `lastTickAt` varies with read path — so the
// index was falsified as it was written. pij#183's retention work reads this
// index, so a policy built on it would have inherited the falsification.
describe("buildArchiveIndexEntry — diedAt and lastActivityAt are separate truths", () => {
	it("records BOTH, and they differ when a seat outlived its last event", () => {
		const entry = buildArchiveIndexEntry(
			descriptor({ ...terminalAt(DIED), startedAt: LONG_AGO, lastEventAt: LONG_AGO }),
			NOW,
		);
		expect(entry.lastActivityAt).toBe(LONG_AGO);
		expect(entry.diedAt).toBe(DIED);
		// The two fields must not collapse into each other — that collapse IS the
		// defect this ruling removed.
		expect(entry.lastActivityAt).not.toBe(entry.diedAt);
	});

	it("omits diedAt for a legacy record with no death stamp, and still records activity", () => {
		const entry = buildArchiveIndexEntry(
			descriptor({ lifecycle: "dissolved", startedAt: LONG_AGO, lastEventAt: RECENTLY }),
			NOW,
		);
		expect(entry.diedAt).toBeUndefined();
		expect(entry.lastActivityAt).toBe(RECENTLY);
	});

	it("lastActivityAt IGNORES lastTickAt, so the row cannot vary with read path", () => {
		const raw = buildArchiveIndexEntry(
			descriptor({ ...terminalAt(DIED), startedAt: LONG_AGO, lastTickAt: RECENTLY }),
			NOW,
		);
		const overlaid = buildArchiveIndexEntry(
			descriptor({ ...terminalAt(DIED), startedAt: LONG_AGO }),
			NOW,
		);
		// Byte-identical rows from a descriptor WITH and WITHOUT the overlaid stamp.
		// That equality is the whole fix: the index stops being a function of how
		// the writer happened to read the descriptor.
		expect(raw).toEqual(overlaid);
	});

	it("survives a round trip through the index line parser", () => {
		const entry = buildArchiveIndexEntry(descriptor({ ...terminalAt(DIED) }), NOW);
		const parsed = parseArchiveIndexLine(JSON.stringify(entry));
		expect(parsed?.diedAt).toBe(DIED);
		expect(parsed?.id).toBe("pij-test-peer");
	});
});

// ─────────────────────────────────────────────────────────────────────────────
// pij#183 — the archive retention bound. Jordan-ruled 2026-08-09: 90 days,
// wreckage deleted, index tombstone kept forever.
describe("isPrunableArchiveRecord — the 90-day ceiling", () => {
	const LONG_DEAD = new Date(NOW - ARCHIVE_PRUNE_AFTER_MS - 60_000).toISOString();
	const RECENTLY_DEAD = new Date(NOW - ARCHIVE_PRUNE_AFTER_MS + 60_000).toISOString();

	it("prunes a terminal record past the bound", () => {
		expect(isPrunableArchiveRecord(descriptor(terminalAt(LONG_DEAD)), NOW)).toBe(true);
	});

	it("keeps one a minute short of it — the boundary, from the safe side", () => {
		expect(isPrunableArchiveRecord(descriptor(terminalAt(RECENTLY_DEAD)), NOW)).toBe(false);
	});

	it("NEVER prunes a non-terminal record, however ancient", () => {
		// Same reason only terminal records are archivable: a stale live seat is a
		// stall to report, not a corpse to bury.
		const ancient = new Date(NOW - ARCHIVE_PRUNE_AFTER_MS * 10).toISOString();
		expect(
			isPrunableArchiveRecord(descriptor({ lifecycle: "bound", startedAt: ancient }), NOW),
		).toBe(false);
	});

	it("KEEPS a record whose anchor cannot be parsed — never delete on suspicion", () => {
		const d = descriptor({ lifecycle: "dissolved", startedAt: "garbage" });
		expect(archiveAgeAnchorMs(d)).toBeNull();
		expect(isPrunableArchiveRecord(d, NOW)).toBe(false);
	});

	it("KEEPS a record whose death is in the FUTURE — clock skew is not age", () => {
		const future = new Date(NOW + 60 * 60_000).toISOString();
		expect(isPrunableArchiveRecord(descriptor(terminalAt(future)), NOW)).toBe(false);
	});

	// The two thresholds share ONE anchor, so a record is always archived before it
	// is prunable. If these ever cross, records would be pruned while still hot.
	it("the prune bound is strictly longer than the archive window", () => {
		expect(ARCHIVE_PRUNE_AFTER_MS).toBeGreaterThan(ARCHIVE_AFTER_MS);
		const justArchived = descriptor(terminalAt(new Date(NOW - ARCHIVE_AFTER_MS - 1).toISOString()));
		expect(classifyRegistryRecord(justArchived, NOW)).toBe("archivable");
		expect(isPrunableArchiveRecord(justArchived, NOW)).toBe(false);
	});
});
