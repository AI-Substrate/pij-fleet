// pij-messaging — delivery receipt model + steered-delivery correlation (pure).
//
// Mechanism proven in scratch/receipt_test/ (finding 08):
//   - input.streamingBehavior null  → idle  → delivered immediately
//   - input.streamingBehavior steer → busy  → queued, then delivered at the
//     NEXT turn_start after the input(steer) event (no before_agent_start fires).
// Runtime emission is Phase 3; this module is the pure model + correlation.

import type { MessageReceipt, ReceiptState, SessionId } from "./types.js";

/** A daemon ticks every 600ms, but a full field day (T5, 2026-07-18) showed
 *  5-19s tick ages on virtually EVERY send under normal load — at 5s the
 *  stale warning was the ordinary receipt and the fleet learned to ignore it
 *  (alarm fatigue is a loosened gate). 30s sits above every observed-normal
 *  reading and far below wedge-scale (a dead daemon reads minutes). */
export const DAEMON_TICK_STALE_AFTER_MS = 30_000;

export interface DaemonTickStatus {
	readonly daemonLastTickAt: string | null;
	readonly daemonTickAgeMs: number | null;
	readonly daemonTickStale: boolean;
}

/** Derive daemon heartbeat health without changing the existing ReceiptState
 *  vocabulary (`queued|delivered|unverified`). */
export function daemonTickStatus(
	lastTickAt: string | undefined,
	nowMs: number,
	staleAfterMs: number = DAEMON_TICK_STALE_AFTER_MS,
): DaemonTickStatus {
	if (!lastTickAt) {
		return { daemonLastTickAt: null, daemonTickAgeMs: null, daemonTickStale: true };
	}
	const tickMs = Date.parse(lastTickAt);
	if (Number.isNaN(tickMs)) {
		return { daemonLastTickAt: null, daemonTickAgeMs: null, daemonTickStale: true };
	}
	const ageMs = Math.max(0, nowMs - tickMs);
	return {
		daemonLastTickAt: lastTickAt,
		daemonTickAgeMs: ageMs,
		daemonTickStale: ageMs > staleAfterMs,
	};
}

/** Classify a message at inject time from the peer's idle state. */
export function classifyOnInject(idle: boolean): ReceiptState {
	return idle ? "delivered" : "queued";
}

/** Build the first receipt for a just-injected message. Idle peer → a single
 *  `delivered`; busy peer → `queued` (a later `delivered` follows). */
export function initialReceipt(
	messageId: string,
	from: SessionId,
	to: SessionId,
	idle: boolean,
	atIso: string,
	daemonLastTickAt?: string,
): MessageReceipt {
	if (idle) {
		return { messageId, from, to, state: "delivered", deliveredAt: atIso };
	}
	return {
		messageId,
		from,
		to,
		state: "queued",
		queuedAt: atIso,
		...(daemonLastTickAt !== undefined
			? daemonTickStatus(daemonLastTickAt, Date.parse(atIso))
			: {}),
	};
}

/** Transition a queued receipt to delivered. */
export function markDelivered(receipt: MessageReceipt, atIso: string): MessageReceipt {
	return { ...receipt, state: "delivered", deliveredAt: atIso };
}

/** Correlate the delivery time of a steered message to the runtime stream.
 *  - idle (steer=false) → delivered immediately (returns injectIso);
 *  - steer (steer=true) → the first turn_start timestamp strictly after the
 *    inject, or null if none has occurred yet (still queued).
 *  `turnStartIsos` are turn_start timestamps observed after the inject, in
 *  arrival order. */
export function correlateDeliveredAt(
	injectIso: string,
	steer: boolean,
	turnStartIsos: readonly string[],
): string | null {
	if (!steer) return injectIso;
	const injectMs = Date.parse(injectIso);
	for (const ts of turnStartIsos) {
		if (Date.parse(ts) > injectMs) return ts;
	}
	return null;
}
