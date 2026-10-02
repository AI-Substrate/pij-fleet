// pij platform — store ports (plan 054, T006/T008/T010 seam).
// Pure boundary types: fs adapters and in-memory fakes both implement these;
// CliDeps consumes them. Reads are guard-validated null-on-fail (focus-store
// precedent); Project.create is first-writer-wins (publishNoReplace
// semantics); the spine log is append-only with durable key-dedupe (AC-03).

import type { Result } from "../types.js";
import type { SpineEventQuery } from "./spine.js";
import type {
	Allocation,
	Assignment,
	Dispatch,
	Fence,
	Project,
	SpineEvent,
	SpineEventDraft,
} from "./types.js";

export interface ProjectStorePort {
	/** First-writer-wins claim of `projects/<slug>/project.json`. */
	create(project: Project): Result<"claimed" | "exists">;
	/** Atomic replace of an EXISTING project; E-NOREG when it never existed. */
	update(project: Project): Result<void>;
	read(slug: string): Project | null;
	/** Valid records only, sorted by slug; corrupt/foreign entries skipped. */
	list(): Project[];
}

export interface AssignmentStorePort {
	/** Atomic create-or-replace of `assignments/<id>.json`. Participates in
	 *  the coupled-write corroboration matrix (plan 054 P2 T005): recovery
	 *  adjudicates task-set/state-set/state-verified ops by comparing this
	 *  store's record against the draft's prev/next via
	 *  `canonicalAssignmentJson` — which EXCLUDES `states` (a log-derived
	 *  index appended inside the pend window; the spine is truth). */
	write(assignment: Assignment): Result<void>;
	read(id: string): Assignment | null;
	/** Valid records only, sorted by id; corrupt/foreign entries skipped. */
	list(): Assignment[];
	listByNode(nodeId: string): Assignment[];
}

export interface AllocationStorePort {
	/** Atomic create-or-replace of `allocations/<id>.json`. */
	write(allocation: Allocation): Result<void>;
	read(id: string): Allocation | null;
	/** Valid records only, ordinal-then-id sorted. */
	list(): Allocation[];
}

export interface FenceStorePort {
	/** Atomic create-or-replace of `fences/<id>.json`. */
	write(fence: Fence): Result<void>;
	read(id: string): Fence | null;
	/** Valid records only, id-sorted. */
	list(): Fence[];
}

export interface DispatchStorePort {
	/** Atomic create-or-replace of `dispatches/<id>.json`. */
	write(dispatch: Dispatch): Result<void>;
	read(id: string): Dispatch | null;
	/** Valid records only, id-sorted. */
	list(): Dispatch[];
}

/** Result of a keyed idempotent append: the event as it exists in the log
 *  (freshly stamped, or the one a previous call already stamped). */
export interface SpineAppendOnceOutcome {
	readonly outcome: "appended" | "existing";
	readonly event: SpineEvent;
}

/** Journal-entry lifecycle (review 002 G2): `intent` = recorded before the
 *  state write — NOT yet replayable, the state may never land; `committed` =
 *  the state write completed. A committed marker is a CLAIM the recovery gate
 *  corroborates before replaying (review 003 H1): for a coupled op ONLY
 *  persisted state equal to the draft's `next` corroborates it (review 004
 *  J1 — the once-record proves the event survived, never the state); an
 *  uncoupled draft is corroborated by its once-record alone. The flip and
 *  the state publish are separate durability domains, so the marker can
 *  outlive a lost state write. */
export type PendingOpPhase = "intent" | "committed";

/** One journaled coupled-write op. `order` is the DURABLE causal position the
 *  journal allocated at record time (review 002 G3: random-UUID filenames
 *  sorted lexically are not an order); entries sharing an order were recorded
 *  by concurrent writers and are mutually unordered — opId breaks the tie
 *  deterministically. */
export interface PendingOp {
	readonly opId: string;
	readonly order: number;
	readonly phase: PendingOpPhase;
	readonly draft: SpineEventDraft;
}

/** Write-ahead journal for the journal-FIRST coupled write (review p1-review-001
 *  HIGH-2 + review 002 G2/G3): the draft spine event is journaled durably as an
 *  INTENT before any project state commits, durably marked COMMITTED after the
 *  state write, appended via `SpineLogPort.appendOnce(opId, draft)`, and cleared
 *  on success — so committed state without its audit event never survives a
 *  single append failure, and a crash before the state write leaves a
 *  discardable intent instead of a phantom event. Surviving ops are recovered
 *  at the start of every platform WRITE verb, in order; a verb must NOT
 *  proceed while a predecessor remains unresolvable. */
export interface OpJournalPort {
	/** Durably journal an intended spine append BEFORE its state write, in
	 *  phase `intent`, allocating its durable causal `order`; returns the
	 *  generated opId. Journal-write failure is a Result error (never a
	 *  throw) and aborts the coupled write before any state commit. */
	record(draft: SpineEventDraft): Result<string>;
	/** Durably flip an entry to phase `committed` AFTER its state write
	 *  landed. E-NOREG when the entry is missing/unwritable — callers may
	 *  proceed (recovery resolves the intent by comparing persisted state
	 *  against the draft's prev/next), but must not throw. */
	markCommitted(opId: string): Result<void>;
	/** Resolve the entry after its coupled write completes. Ok ONLY with
	 *  DURABLE evidence of the resolution (review 003 M3, hardened by review
	 *  005 K1): entry removal alone is not that evidence — its durability
	 *  rides on directory fsync, which is fail-soft or unsupported on some
	 *  platforms, and a power-loss-resurrected entry is byte-identical to a
	 *  live crash record, so recovery would forge an aborted intent past a
	 *  genuine winner or false-block a legitimately moved-on committed op.
	 *  After a successful clear the op must NEVER again be presented by
	 *  pending() as live, even if its directory entry resurrects. E-NOREG
	 *  when that evidence cannot be established; clearing an already-absent
	 *  opId is ok. Never throws. */
	clear(opId: string): Result<void>;
	/** Surviving ops from crashed/failed coupled writes, order-ascending
	 *  (opId as tiebreak). An unreadable, invalid, or newer-schema op-shaped
	 *  entry is E-NOREG naming its path, NEVER silently skipped (review 003
	 *  H2): a damaged safety record is not equivalent to absence — recovery
	 *  must fail before mutation rather than write past an unaudited
	 *  predecessor. Non-op filenames in an exclusive journal directory may
	 *  still be ignored. An entry carrying durable resolution evidence — a
	 *  cleared op resurrected by power loss — is NOT a surviving op (review
	 *  005 K1): it is never presented, and is swept opportunistically, its
	 *  evidence discarded only once the entry's absence is itself durable.
	 *
	 *  DOCUMENTED RESIDUAL (plan 054 P2 T011, cycle-6 carry-in): where
	 *  directory fsync is unsupported (Windows cannot open directories for
	 *  fsync), the sweep can never prove an op entry's absence durable, so
	 *  resolution tombstones are retained indefinitely — growth is one small
	 *  file per resolved op in an exclusive directory, and the pending()
	 *  scan is O(entries). Discarding them on any weaker signal (age, count)
	 *  would reopen K1's forge/false-block, so pij deliberately does not.
	 *  On fsync-capable platforms every pending() pass empties the sweepable
	 *  set (growth-bound pinned). Manual remedy on Windows, under
	 *  process-crash durability scope: with ALL platform writers stopped and
	 *  after a clean OS flush, delete `*.resolved` files whose
	 *  `<opId>.json` twin is absent. */
	pending(): Result<readonly PendingOp[]>;
}

/** Machine-wide mutual exclusion for platform WRITE verbs (review 002 G2/G3).
 *  Holding it across the whole coupled write (recover → journal → state →
 *  append → clear) is what makes intent-phase recovery SOUND: any intent
 *  entry observed under the lock belongs to a crashed or aborted writer,
 *  never to a live one mid-window, so discarding an abandoned intent can
 *  never strip a live writer of its crash protection. Never stolen (G1
 *  policy): acquisition failure is an honest E-NOREG naming the lock. */
export interface PlatformWriteLockPort {
	withPlatformWriteLock<T>(operation: () => T): Result<T>;
}

export interface SpineLogPort {
	/** Append one line, allocating `seq` ATOMICALLY inside the operation
	 *  (review 001 F1: caller-side `lastSeq() + 1` let two processes mint the
	 *  same seq and lose an event behind an advanced `read({since})` cursor).
	 *  Allocation and durability are serialized cross-process, so a reader can
	 *  never observe seq n before every seq < n is on disk. Newline-guards a
	 *  torn tail so a crashed writer never swallows the next event. Returns
	 *  the stamped event; store faults (incl. lock timeout) are Result errors,
	 *  never throws. */
	append(draft: SpineEventDraft): Result<SpineEvent>;
	/** Durable idempotence by key, same atomic allocation: replay returns the
	 *  ORIGINALLY stamped event ("existing") and gains nothing. */
	appendOnce(key: string, draft: SpineEventDraft): Result<SpineAppendOnceOutcome>;
	/** Whether a keyed appendOnce ALREADY landed durably (its once-record
	 *  exists), without appending anything. Recovery's corroboration probe for
	 *  UNCOUPLED drafts only (review 003 H1, narrowed by review 004 J1): the
	 *  once-record proves the EVENT survived — never that a coupled op's
	 *  project publish did. Spine link and project rename are separate
	 *  directory entries under best-effort fsync, so the once-file can outlive
	 *  a lost publish; a coupled op's persisted-state mismatch therefore
	 *  always blocks, once-record or not. */
	hasOnce(key: string): boolean;
	/** Max seq on disk (0 when empty); corrupt/partial lines skipped. */
	lastSeq(): number;
	/** Guard-filtered, seq-ascending; exact peer/project, exclusive since. */
	read(query?: SpineEventQuery): SpineEvent[];
}
