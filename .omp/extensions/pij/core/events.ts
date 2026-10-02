// pij-messaging — event construction, filtering, and age (pure).
//
// Every event carries a strictly-monotonic `seq` and an ISO-8601 `timestamp`
// so a reader computes each event's age from the stream alone (spec AC-7/7a).

import type { EventQuery, PijEvent } from "./types.js";

/** Build one event line. `seq` comes from SeqCounter; `nowMs` is injected for
 *  deterministic tests (Pattern P3). */
export function buildEvent(seq: number, type: string, nowMs: number, data?: unknown): PijEvent {
	return {
		seq,
		timestamp: new Date(nowMs).toISOString(),
		type,
		...(data === undefined ? {} : { data }),
	};
}

/** Apply since/type/last filters (spec AC-8). Order: since → type → last.
 *  `last` keeps the final N after the other filters (present-minus-N). */
export function filterEvents(events: readonly PijEvent[], query: EventQuery = {}): PijEvent[] {
	let out = events.slice();
	if (query.since !== undefined) {
		const since = query.since;
		out = out.filter((e) => e.seq > since);
	}
	if (query.type !== undefined) {
		const type = query.type;
		out = out.filter((e) => e.type === type);
	}
	if (query.last !== undefined) {
		out = out.slice(Math.max(0, out.length - query.last));
	}
	return out;
}

/** Age in ms of an event relative to now (now − event.timestamp). */
export function eventAgeMs(event: PijEvent, nowMs: number): number {
	return nowMs - Date.parse(event.timestamp);
}

/** Age in ms of the newest event, or null when the stream is empty. Used for
 *  stall detection (spec AC-7a) and the state report (AC-9). */
export function latestEventAgeMs(events: readonly PijEvent[], nowMs: number): number | null {
	const first = events[0];
	if (first === undefined) return null;
	let newest = first;
	for (const e of events) {
		if (e.seq > newest.seq) newest = e;
	}
	return eventAgeMs(newest, nowMs);
}
