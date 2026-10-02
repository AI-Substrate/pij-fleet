// pij-control-plane — THE REGISTRY WRITE LAW (plan 071 review round 1, §1.2).
//
// This lives next to the registry port, NOT in a daemon module, because the law
// is not daemon-specific — and pretending it was is exactly what let it be
// bypassed twice inside the branch that was fixing it.
//
// The incident list, verbatim from where each was learned:
//
//   #1  s054 Finding 04 — the node-truth denorms (`currentAssignment`/
//       `currentTask`/`semanticState`) are CLI-stamped between the daemon's
//       tick-start snapshot and its persist, so a daemon write replayed them away.
//   #2  s070 / #47 — terminal truth is stamped by the CLOSE path (`pij close` and
//       the in-process close), never computed by the daemon, and it is the
//       evidence the death reconciler classifies from. Leaving `closeIntent`/
//       `terminal`/`deathNoticeLatchedAt` unprotected made a pij-REQUESTED close
//       announce itself as `unrequested-by-pij`: close persists intent, kills the
//       pane, stamps `terminal: requested`, dissolves — then an overlapping tick
//       wrote its pre-close snapshot back over all of it, and the next sweep saw a
//       dead PID with no intent and no terminal. A lost update, not a missing check.
//   #3  s071 D7 — a buffered message reported as consumed destroyed the only
//       durable copy of an undelivered message.
//   #4  s071 D3 — `stampSenderActivity`, a CLI-side read-modify-write.
//   #5  s071 D4 — `releaseIdentity`: the widest window yet (two unlinks between
//       read and write) AND a `writeAtomic` that bypassed the dissolved-tombstone
//       guard, resurrecting a tombstone as an unreconcilable `pending` zombie.
//
// #4 and #5 were written by the branch fixing #1-#3, while the law sat documented
// behind a docstring reading "every DAEMON descriptor write". A law that requires
// the writer to know it has now failed twice in twelve commits, so it is no longer
// a law the writer must find: `RegistryPort.write` MERGES BY DEFAULT, inside the
// adapter, at zero extra I/O (that read already happens for the tombstone guard).
// The sixth writer is safe having read none of this.
//
// ── The framing trap (and why these are NOT "externally owned" fields) ─────────
//
// The original lists were daemon-centric: "externally owned" meant "not owned by
// the daemon". The moment `write()` merges for EVERYONE, that framing inverts and
// becomes a lost-update in the opposite direction — the CLI *owns*
// `currentAssignment`/`currentTask`/`semanticState`, so a symmetric merge would
// make the CLI re-read its own field from disk and drop the value it just computed.
//
// So the law is stated as PER-FIELD OWNERSHIP: every contested field has exactly
// ONE writer. On any write, fields owned by someone OTHER than the declared writer
// come from disk; fields the writer owns win. A writer that declares nothing owns
// nothing, which is the safe default — it refreshes its own uncontested fields and
// can never clobber a contested one.

import type { RegistryPort } from "./ports.js";
import type { SessionDescriptor } from "./types.js";

/** Who is performing a write — the AUTHORITY being exercised, not the process.
 *  `"close"` means "I am writing terminal truth", whether that is `pij close`, the
 *  in-process close, or the daemon's death reconciler observing a dead pane. What
 *  s070/#47 actually forbade was a daemon ACTIVITY write clobbering terminal
 *  truth, not the daemon ever writing it.
 *
 *  Declaring your authority lets you keep the fields it owns.
 *
 *  OMITTING IT IS NOT "SAFE" — read this precisely, because the imprecise version
 *  of this sentence already shipped a bug (review round 2). Omitting is safe for
 *  OTHER writers' data: you cannot clobber a contested field. It is SILENTLY LOSSY
 *  FOR YOUR OWN: any contested field you are trying to SET is discarded whenever
 *  disk already holds a value for it, with no error and no log line. If your write
 *  carries a contested field ON PURPOSE, you MUST declare the authority that owns
 *  it. `parentId` on a re-parent is the live example. */
export type DescriptorWriter = "daemon" | "cli" | "seat" | "close";

type ContestedField = keyof typeof DESCRIPTOR_FIELD_OWNER;

/** The single owner of each contested field. "Contested" = more than one process
 *  writes the descriptor while this field matters. Anything not listed here is
 *  uncontested and always takes the writer's value. */
export const DESCRIPTOR_FIELD_OWNER = {
	// Honour-system orchestration designation + graph shape — CLI verbs only.
	// `prime:false`/`oldPrime:false` are meaningful state, so truthiness or
	// fill-only merging is wrong; ownership is the correct frame.
	prime: "cli",
	oldPrime: "cli",
	parentId: "cli",
	gitCommonDir: "cli",
	// Incident #1 (lines 9-11 above) — CLI-stamped node truth must survive a
	// daemon write replaying its tick-start snapshot.
	currentAssignment: "cli",
	currentTask: "cli",
	planId: "cli",
	semanticState: "cli",
	stateNote: "cli",
	statusPrev: "cli",
	statusNext: "cli",
	statusAt: "cli",
	statusSeq: "cli",
	statusWrittenBy: "cli",
	orchestrationRole: "cli",
	// Stamped by spawn/adopt; the daemon's backfill only ever writes it where absent.
	windowId: "cli",
	// #2 — terminal truth belongs to the close path, never to a computed tick.
	closeIntent: "close",
	terminal: "close",
	deathNoticeLatchedAt: "close",
	// Stamped by `pij agent report`, in the peer's own process.
	reportedAt: "seat",
} as const satisfies Partial<Record<keyof SessionDescriptor, DescriptorWriter>>;

/** APPEND-ONLY contested fields: a non-owner fills the gap from disk when it has
 *  no value of its own, but a value it DOES carry still stands.
 *
 *  `reportedAt` is the case: it is stamped once by the peer and then only ever
 *  carried forward, so "disk always wins" would be wrong — the daemon legitimately
 *  propagates a value it already holds, and a strict owner-wins rule silently drops
 *  it (AC-16). Everything else is owner-wins. */
const APPEND_ONLY_FIELDS = new Set<ContestedField>(["reportedAt"]);

const CONTESTED_FIELDS = Object.keys(DESCRIPTOR_FIELD_OWNER) as ContestedField[];

/** Apply the ownership law: take what the writer proposes, then restore every
 *  contested field it does not own from the latest on-disk record.
 *
 *  `latest` absent (a brand-new descriptor) ⇒ nothing to merge; the proposal stands.
 *
 *  `undefined` on disk is respected as "absent", so a field the owner has not yet
 *  stamped never resurrects. A non-owner cannot CLEAR a contested field either —
 *  clearing one is always a deliberate act, which is what `writeExact` is for. */
export function applyWriteLaw(
	proposed: SessionDescriptor,
	latest: SessionDescriptor | null,
	writer?: DescriptorWriter,
): SessionDescriptor {
	if (!latest) return proposed;
	let merged = proposed;
	for (const field of CONTESTED_FIELDS) {
		if (DESCRIPTOR_FIELD_OWNER[field] === writer) continue; // the owner's value wins
		const disk = latest[field];
		// Disk wins only where disk HAS a value. This is the proven rule from the
		// original law, and the boundary matters: "disk wins even when absent" would
		// stop any non-owner from ever SETTING a contested field, which breaks every
		// two-phase publish (spawn writes a descriptor, then writes it again with its
		// parent link). The bug class this law exists to stop is a stale writer
		// OVERWRITING a value someone else just stamped — not a writer filling a gap.
		if (disk === undefined || disk === merged[field]) continue;
		// Append-only: only ever fills a gap; never overwrites a value the writer holds.
		if (APPEND_ONLY_FIELDS.has(field) && merged[field] !== undefined) continue;
		merged = { ...merged, [field]: disk };
	}
	return merged;
}

/** Persist a daemon-computed descriptor and return REGISTRY TRUTH.
 *
 *  This is no longer a merge — the ownership law lives in exactly one place now,
 *  inside the adapter's `write()` (plan 071 review §1.2, dove's ruling). All this
 *  does is declare the writer as `"daemon"` and re-read, because the daemon's
 *  callers depend on the persisted value (which may be a concurrent dissolved
 *  tombstone when the registry rejects a stale write). */
export function persistDaemonWrite(
	registry: RegistryPort,
	computed: SessionDescriptor,
): SessionDescriptor {
	registry.write(computed, "daemon");
	const persisted = registry.read(computed.id);
	if (!persisted) throw new Error(`daemon write for ${computed.id} did not persist`);
	return persisted;
}
