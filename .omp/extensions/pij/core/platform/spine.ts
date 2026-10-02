// pij platform — pure spine-event construction + filtering (plan 054, WS-5).
//
// buildSpineEvent is FALLIBLE on the clock (review 001 F7): the injected
// nowMs (Pattern P3) is checked through isoTimestamp, so NaN or an
// out-of-TimeClip clock is E-ARG, never an escaping RangeError. It stamps
// schema_version 1 and ts = ISO(nowMs); refs defaults to []; absent
// optionals stay ABSENT (never undefined-valued keys) so records are
// JSON-round-trip stable. It returns a SpineEventDraft — NO seq: sequence
// numbers are allocated inside SpineLogPort.append/appendOnce (review 001 F1),
// never by callers. filterSpineEvents mirrors core/events.ts
// filterEvents: sequential narrowing since (seq > since, EXCLUSIVE) →
// peer → project, exact string equality (AC-02), input untouched.

import { ok, type Result } from "../types.js";
import { isoTimestamp } from "./time.js";
import type { ActorProvenance, SpineEvent, SpineEventDraft } from "./types.js";

/** Constructor input. NO seq — the log port allocates it atomically on
 *  append (review 001 F1); `nowMs` is the injected clock (Pattern P3). */
export interface BuildSpineEventInput {
	readonly nowMs: number;
	readonly actor: string;
	readonly kind: string;
	readonly refs?: readonly string[];
	readonly peer?: string;
	readonly project?: string;
	readonly repo?: string;
	readonly prev?: string;
	readonly next?: string;
	readonly verifiedBy?: string;
	readonly actorProvenance?: ActorProvenance;
}

/** Build one spine line draft; E-ARG on an invalid clock (review 001 F7).
 *  The log port stamps `seq` when it appends. */
export function buildSpineEvent(input: BuildSpineEventInput): Result<SpineEventDraft> {
	const ts = isoTimestamp(input.nowMs);
	if (!ts.ok) return ts;
	return ok({
		schema_version: 1,
		ts: ts.value,
		actor: input.actor,
		kind: input.kind,
		// Copy, never alias: a caller-held refs array must not rewrite a built event.
		refs: input.refs === undefined ? [] : [...input.refs],
		...(input.peer === undefined ? {} : { peer: input.peer }),
		...(input.project === undefined ? {} : { project: input.project }),
		...(input.repo === undefined ? {} : { repo: input.repo }),
		...(input.prev === undefined ? {} : { prev: input.prev }),
		...(input.next === undefined ? {} : { next: input.next }),
		...(input.verifiedBy === undefined ? {} : { verifiedBy: input.verifiedBy }),
		...(input.actorProvenance === undefined ? {} : { actorProvenance: input.actorProvenance }),
	});
}

/** Filters for incremental spine follow. */
export interface SpineEventQuery {
	/** Only events with seq > since (EXCLUSIVE). */
	readonly since?: number;
	/** Only events whose peer equals this exactly (AC-02 — never prefix/substring). */
	readonly peer?: string;
	/** Only events whose project equals this exactly (AC-02). */
	readonly project?: string;
}

/** Apply since/peer/project filters. Order: since → peer → project.
 *  Fresh array out; input and order untouched. */
export function filterSpineEvents(
	events: readonly SpineEvent[],
	query: SpineEventQuery = {},
): SpineEvent[] {
	let out = events.slice();
	if (query.since !== undefined) {
		const since = query.since;
		out = out.filter((e) => e.seq > since);
	}
	if (query.peer !== undefined) {
		const peer = query.peer;
		out = out.filter((e) => e.peer === peer);
	}
	if (query.project !== undefined) {
		const project = query.project;
		out = out.filter((e) => e.project === project);
	}
	return out;
}
