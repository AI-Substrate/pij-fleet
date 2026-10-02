import type { SessionDescriptor } from "./types.js";

export const INVARIANT_NINE_MODAL_REASON =
	"The ask_user_question tool is forbidden by pij invariant #9. Ask inline through the active delivery channel using pij_send, persist the pending decision, and block only dependent work.";

export interface InvariantNineModalBlock {
	readonly block: true;
	readonly reason: typeof INVARIANT_NINE_MODAL_REASON;
}

function isManagedPiPeer(descriptor: SessionDescriptor): boolean {
	return (
		descriptor.harness === "pi" &&
		(descriptor.parentId !== undefined ||
			descriptor.spawnedBy !== undefined ||
			descriptor.prime === true ||
			descriptor.oldPrime === true)
	);
}

/** Blocks modal questions only in a pij-managed Pi peer. */
export function guardInvariantNineModal(
	descriptor: SessionDescriptor | null | undefined,
	toolName: string,
): InvariantNineModalBlock | undefined {
	if (toolName !== "ask_user_question" || descriptor == null || !isManagedPiPeer(descriptor)) {
		return undefined;
	}
	return { block: true, reason: INVARIANT_NINE_MODAL_REASON };
}
