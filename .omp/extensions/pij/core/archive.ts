// pij-control-plane — two-tier registry policy (plan 071 D1, T001).
//
// PURE. Decides whether a registry record belongs in the HOT tier (`~/.pij/*.json`,
// which the daemon tick scans in full every 600ms) or is ARCHIVABLE (moved under
// `~/.pij/archive/`, reachable only by keyed lookup).
//
// Why this exists: on 2026-07-25 ~2,000 dead descriptors made each daemon tick
// ~19s — the tick calls `registry.list()` six times, and every call is a readdir
// plus a JSON.parse of EVERY descriptor that has ever existed. Delivery waits for
// a tick, so tick cost is delivery latency. The invariant this module encodes is
// `tick cost = O(live)`: the hot tier holds live seats plus recently-terminal ones,
// and nothing else.
//
// The bias is deliberately conservative — archiving is a MOVE, and a wrongly
// archived live seat is a fleet outage while a wrongly retained dead one is only
// slow. So every "can't prove it" branch answers `hot`.

import type { SessionDescriptor } from "./types.js";

/** A terminal record stays hot this long after its last observed activity, so
 *  post-mortem tooling (`pij state`, `pij tail`, a human reading the wreckage)
 *  finds it exactly where it always was. Jordan-approved: 48h. */
export const ARCHIVE_AFTER_MS = 48 * 60 * 60 * 1000;

/** How long an ARCHIVED record's wreckage is kept before it is pruned.
 *  Jordan-ruled 2026-08-09: 90 days.
 *
 *  IT IS A CEILING, NOT A CLEANUP, and it was chosen knowing that. Measured when
 *  the ruling was made: the entire archive was 1.0 GB across 2,548 session
 *  directories and **nothing in it was older than 22.7 days** (median 19), so a
 *  90-day bound reclaims ZERO BYTES on the day it ships. At the observed ~44
 *  MB/day it binds once the archive reaches steady state, around 4 GB. The point
 *  is that unbounded growth stops being unbounded — not that disk is freed today.
 *
 *  Anchored on the same death instant as {@link ARCHIVE_AFTER_MS} rather than on
 *  "time since the move", so there is ONE clock in this file and not two: a record
 *  enters the archive at death + 48h and is pruned at death + 90d, i.e. after ~88
 *  days of being archived. Reusing the anchor also inherits its
 *  provenance-independence for free. */
export const ARCHIVE_PRUNE_AFTER_MS = 90 * 24 * 60 * 60 * 1000;

/** Which tier a record belongs in. `hot` = scanned by the tick; `archivable` =
 *  eligible for the daemon's end-of-tick move to `~/.pij/archive/`. */
export type RegistryTier = "hot" | "archivable";

/** Lifecycles with no future: the session will never run, send, or bind again.
 *  `bound`/`pending`/`ready` are live-or-becoming-live and NEVER archivable, no
 *  matter how stale — a stale bound seat is a stall to report, not a corpse to
 *  bury (that distinction is the whole point of D3). */
export function isTerminalRecord(descriptor: SessionDescriptor): boolean {
	return descriptor.lifecycle === "dissolved" || descriptor.lifecycle === "failed";
}

/** The record's genuine ACTIVITY axis, newest-wins: when did this seat last *do*
 *  something. `lastEventAt` is the real signal; `startedAt` is the floor (always
 *  present) so a descriptor that died before it ever did anything still has one.
 *
 *  `lastTickAt` IS DELIBERATELY ABSENT, and pij#204 is why. It was moved out of
 *  the descriptor into a heartbeat side file (pij#180), which is overlaid by
 *  `read()`/`list()` and scrubbed on terminal reads — so a descriptor obtained by
 *  raw `readFile` and one obtained by `read()` are the same TYPE and not the same
 *  VALUE. Any function reading it silently changes answer with its caller's read
 *  path. **Dropping it is what makes this function provenance-independent**: every
 *  field it touches is a plain persisted one, so it returns the same answer from
 *  every path. That property, not the arithmetic, is the fix.
 *
 *  Returns null when NOTHING parses. */
export function lastActivityAtMs(descriptor: SessionDescriptor): number | null {
	let newest: number | null = null;
	for (const stamp of [descriptor.lastEventAt, descriptor.startedAt]) {
		if (typeof stamp !== "string") continue;
		const parsed = Date.parse(stamp);
		if (!Number.isFinite(parsed)) continue;
		if (newest === null || parsed > newest) newest = parsed;
	}
	return newest;
}

/** The instant a terminal record's post-mortem clock starts: WHEN IT DIED.
 *
 *  {@link ARCHIVE_AFTER_MS} says a terminal record "stays hot this long after its
 *  last observed activity, so post-mortem tooling finds it exactly where it always
 *  was". Measuring that from ACTIVITY does not deliver it, and fails in the
 *  direction that destroys evidence: a seat that ran for five days and died a
 *  minute ago is instantly older than the window and is archived on the next
 *  sweep — the wreckage moved out from under the grace it was promised. Measured
 *  live (pij#204): `pij-straight-araminta`, 124.8h by activity, 0.8h since death.
 *
 *  `terminal.observedAt` is the death reconciler's terminal truth and was present
 *  on 66 of 66 terminal records when this was written. Jordan-ruled 2026-08-09.
 *
 *  FALLBACK, and it is an explicit branch rather than a `??` on purpose: a record
 *  with no death stamp (legacy — the field postdates them) falls back to the
 *  ACTIVITY anchor, never to "not archivable". "No death stamp" and "died at the
 *  epoch" must not share an answer, and nothing may become immortal by lacking a
 *  field. */
export function archiveAgeAnchorMs(descriptor: SessionDescriptor): number | null {
	const observedAt = descriptor.terminal?.observedAt;
	if (typeof observedAt === "string") {
		const parsed = Date.parse(observedAt);
		if (Number.isFinite(parsed)) return parsed;
	}
	return lastActivityAtMs(descriptor);
}

/** Hot vs archivable for one record at `nowMs`.
 *
 *  A record is archivable ONLY when it is terminal AND its newest provable
 *  activity is older than {@link ARCHIVE_AFTER_MS}. Everything else — live
 *  lifecycles, legacy descriptors with no lifecycle at all, unparseable
 *  timestamps, and clock skew that puts the anchor in the future — stays hot. */
/** How long a stamped `revivePendingAt` protects a record from the janitor.
 *  Covers a cold harness boot with a large transcript; the marker stops mattering
 *  the moment the revived seat publishes a live descriptor, so this is a ceiling. */
export const REVIVE_GRACE_MS = 10 * 60_000;

export function classifyRegistryRecord(descriptor: SessionDescriptor, nowMs: number): RegistryTier {
	if (!isTerminalRecord(descriptor)) return "hot";
	// A revive in flight (review round 1 §2.1). `pij revive` relaunches a DISSOLVED
	// seat, and for Pi and OMP s066 deliberately writes no descriptor during boot —
	// so the record stays `dissolved` for the whole window and the 60s janitor would
	// `renameSync` the session dir out from under the booting process. Age alone
	// cannot see this: a revivable seat is >=48h old BY DEFINITION.
	const revivingMs = descriptor.revivePendingAt
		? Date.parse(descriptor.revivePendingAt)
		: Number.NaN;
	if (Number.isFinite(revivingMs)) {
		const sinceMs = nowMs - revivingMs;
		if (sinceMs >= 0 && sinceMs < REVIVE_GRACE_MS) return "hot";
	}
	const anchorMs = archiveAgeAnchorMs(descriptor);
	if (anchorMs === null) return "hot";
	const ageMs = nowMs - anchorMs;
	if (!Number.isFinite(ageMs) || ageMs < ARCHIVE_AFTER_MS) return "hot";
	return "archivable";
}

/** One append-only line in `~/.pij/archive/index.jsonl`, written by the daemon at
 *  the moment of the move and read by `pij list --archived`. Deliberately a
 *  denormalised summary: the point of the index is that listing archived seats
 *  never has to open ~2,000 descriptor files. */
export interface ArchiveIndexEntry {
	readonly id: string;
	/** ISO-8601 — when the daemon moved it. */
	readonly archivedAt: string;
	readonly lifecycle?: SessionDescriptor["lifecycle"];
	readonly failureReason?: SessionDescriptor["failureReason"];
	readonly harness?: SessionDescriptor["harness"];
	readonly folder?: string;
	/** ISO-8601 — the seat's last GENUINE activity (`lastEventAt`, else
	 *  `startedAt`). Means what it says and nothing else. */
	readonly lastActivityAt?: string;
	/** ISO-8601 — when the seat DIED, and the anchor the archival decision was
	 *  made on. Absent for a legacy record with no `terminal.observedAt`, in which
	 *  case the decision fell back to `lastActivityAt`.
	 *
	 *  SEPARATE FROM `lastActivityAt` BY RULING (Jordan, 2026-08-09, pij#204).
	 *  One field was doing both jobs and doing neither honestly: it was labelled
	 *  "last activity" while carrying the decision's anchor, and it was written
	 *  from a raw descriptor whose `lastTickAt` varied with read path — so the
	 *  index was falsified as it was written, and pij#183's retention work would
	 *  have read it. Two fields, each true, neither inheriting the other's defect. */
	readonly diedAt?: string;
}

/** Build the index line for a record being archived at `nowMs`. */
export function buildArchiveIndexEntry(
	descriptor: SessionDescriptor,
	nowMs: number,
): ArchiveIndexEntry {
	const activityMs = lastActivityAtMs(descriptor);
	const diedAtRaw = descriptor.terminal?.observedAt;
	const diedMs = typeof diedAtRaw === "string" ? Date.parse(diedAtRaw) : Number.NaN;
	return {
		id: descriptor.id,
		archivedAt: new Date(nowMs).toISOString(),
		...(descriptor.lifecycle ? { lifecycle: descriptor.lifecycle } : {}),
		...(descriptor.failureReason ? { failureReason: descriptor.failureReason } : {}),
		...(descriptor.harness ? { harness: descriptor.harness } : {}),
		...(descriptor.folder ? { folder: descriptor.folder } : {}),
		...(activityMs === null ? {} : { lastActivityAt: new Date(activityMs).toISOString() }),
		...(Number.isFinite(diedMs) ? { diedAt: new Date(diedMs).toISOString() } : {}),
	};
}

/** Parse one `index.jsonl` line. Returns null for blank/corrupt lines so a torn
 *  tail (the daemon crashing mid-append) degrades to "one missing row" instead of
 *  failing the whole listing. */
export function parseArchiveIndexLine(line: string): ArchiveIndexEntry | null {
	const trimmed = line.trim();
	if (trimmed === "") return null;
	try {
		const parsed: unknown = JSON.parse(trimmed);
		if (typeof parsed !== "object" || parsed === null) return null;
		const entry = parsed as Record<string, unknown>;
		if (typeof entry.id !== "string" || typeof entry.archivedAt !== "string") return null;
		return entry as unknown as ArchiveIndexEntry;
	} catch {
		return null;
	}
}

/** Is this archived record old enough to have its wreckage deleted?
 *
 *  WHAT IS DELETED AND WHAT SURVIVES — the asymmetry is the ruling, not an
 *  implementation detail. The record file and the session directory are the bulk
 *  (94 MB for the largest single seat when this was written); the
 *  `archive/index.jsonl` row is one line. So the WRECKAGE is deleted and the
 *  TOMBSTONE IS KEPT FOREVER: "did seat X ever exist, when did it die, how did it
 *  end" stays answerable for near-zero disk, while the transcripts and event logs
 *  that make up the gigabyte do not.
 *
 *  Only TERMINAL records are prunable, for the same reason only terminal records
 *  are archivable — and an unparseable anchor keeps the record, because a record
 *  that cannot be proven old must never be deleted on suspicion. */
export function isPrunableArchiveRecord(descriptor: SessionDescriptor, nowMs: number): boolean {
	if (!isTerminalRecord(descriptor)) return false;
	const anchorMs = archiveAgeAnchorMs(descriptor);
	if (anchorMs === null) return false;
	const ageMs = nowMs - anchorMs;
	// NaN-safe and skew-safe: a future anchor fails this and the record is KEPT.
	return Number.isFinite(ageMs) && ageMs >= ARCHIVE_PRUNE_AFTER_MS;
}
