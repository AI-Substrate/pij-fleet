// pij-messaging — message framing + boot self-announce text (pure).
//
// A message carries the sender id inline so the receiver can reply with zero
// lookup (spec AC-5): `[pij-rs from <id>]\n<body>\n[/pij]`.

import { renderDestination } from "./address.js";
import type { ReceiptState, Role, SessionId } from "./types.js";

const FRAME_RE = /^\[pij-rs from ([^\]]+)\]\n([\s\S]*)\n\[\/pij\]$/;
const LEGACY_FRAME_RE = /^\[pij from ([^\]]+)\] ([\s\S]*)$/;

/** How a message's sender is shown: `seat@machine` when the daemon stamped the
 *  paired machine it was forwarded from, so a remote seat never reads as a local
 *  one; the bare seat id only for a local message (plan 164 S7). Rendered in the
 *  address grammar, so a reply to exactly this text reaches the sender (F01b). */
export function senderLabel(from: SessionId, fromMachine?: string): string {
	return renderDestination(
		fromMachine === undefined ? { seat: from } : { seat: from, machine: fromMachine },
	);
}

/** Frame a body with the sender for injection into the peer; the daemon's
 *  `frame_message` envelope (crates/core/src/framing.rs), sender in address grammar. */
export function frame(from: SessionId, body: string, fromMachine?: string): string {
	return `[pij-rs from ${senderLabel(from, fromMachine)}]\n${body}\n[/pij]`;
}

/** Parse a canonical or legacy message into { from, body }, or null if unframed. */
export function parseFrame(text: string): { from: SessionId; body: string } | null {
	const m = FRAME_RE.exec(text) ?? LEGACY_FRAME_RE.exec(text);
	if (!m) return null;
	const from = m[1];
	const body = m[2];
	if (from === undefined || body === undefined) return null;
	return { from, body };
}

/** Human/agent-readable role label. */
export function roleLabel(role: Role | undefined): string {
	if (role === "parent") return "PARENT (reviewer)";
	if (role === "worker") return "WORKER";
	return "PEER";
}

/** Body of an extension-issued delivery receipt, framed for the sender's
 *  event log (spec AC-13): `[pij receipt <messageId>] queued|delivered|unverified`. */
export function receiptBody(messageId: string, state: ReceiptState): string {
	return `[pij receipt ${messageId}] ${state}`;
}

const RECEIPT_RE = /^\[pij receipt ([^\]]+)\] (queued|delivered|unverified)$/;

/** Parse a receipt body back into { messageId, state }, or null if not a
 *  receipt (used by `pij send --wait` to correlate the delivered receipt and
 *  by `pij tail` to summarise receipt events). */
export function parseReceiptBody(body: string): { messageId: string; state: ReceiptState } | null {
	const m = RECEIPT_RE.exec(body);
	if (!m) return null;
	const messageId = m[1];
	const state = m[2];
	if (messageId === undefined || state === undefined) return null;
	return { messageId, state: state as ReceiptState };
}

export interface BriefAckReceipt {
	readonly schema_version: 1;
	readonly kind: "brief-ack";
	readonly messageId: string;
	readonly packetId: string;
	readonly packetSha256: string;
	readonly declaredRuntime: {
		readonly model: string | "default";
		readonly effort: string | "default";
		readonly source: "self-report";
	};
	readonly seat: SessionId;
	readonly ts: string;
}

const BRIEF_ACK_PREFIX = "[pij brief-ack] ";
const SHA256_RE = /^[a-f0-9]{64}$/;

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isBriefAckReceipt(value: unknown): value is BriefAckReceipt {
	try {
		if (!isRecord(value) || !isRecord(value.declaredRuntime)) return false;
		return (
			value.schema_version === 1 &&
			value.kind === "brief-ack" &&
			typeof value.messageId === "string" &&
			value.messageId.length > 0 &&
			typeof value.packetId === "string" &&
			value.packetId.length > 0 &&
			typeof value.packetSha256 === "string" &&
			SHA256_RE.test(value.packetSha256) &&
			typeof value.declaredRuntime.model === "string" &&
			value.declaredRuntime.model.length > 0 &&
			typeof value.declaredRuntime.effort === "string" &&
			value.declaredRuntime.effort.length > 0 &&
			value.declaredRuntime.source === "self-report" &&
			typeof value.seat === "string" &&
			value.seat.length > 0 &&
			typeof value.ts === "string" &&
			value.ts.length > 0
		);
	} catch {
		return false;
	}
}

export function briefAckBody(receipt: BriefAckReceipt): string {
	return `${BRIEF_ACK_PREFIX}${JSON.stringify(receipt)}`;
}

export function parseBriefAckBody(body: string): BriefAckReceipt | null {
	if (!body.startsWith(BRIEF_ACK_PREFIX)) return null;
	try {
		const parsed: unknown = JSON.parse(body.slice(BRIEF_ACK_PREFIX.length));
		return isBriefAckReceipt(parsed) ? parsed : null;
	} catch {
		return null;
	}
}

/** Boot self-announce injected at session start (spec AC-2): tells the session
 *  its own id + how to use pij, so it can be addressed and can reply.
 *
 *  Deliberately NON-imperative: it states capabilities, it does not hand the
 *  model a startup to-do list. A directive briefing made fresh boots run every
 *  `pij` command and snoop their own inbox (difficulty D-040). It also tells the
 *  model NOT to treat inbox files as live instructions — real peer messages are
 *  injected as `[pij-rs from <id>] … [/pij]`; the JSON files in the inbox dir are an
 *  internal transport log, not a task queue. */
export function announceText(self: SessionId, role?: Role): string {
	return [
		`You are pij session ${self} (${roleLabel(role)}). This is context only — no action is required.`,
		`Peers can reach you; their messages arrive inline as "[pij-rs from <id>]" on its own line, then the body, then "[/pij]". Only those injected messages are live instructions.`,
		`Do NOT read, list, or act on files under your pij data/inbox directory — they are an internal transport log, not tasks, and replaying them re-runs old requests.`,
		`To reply or message a peer, call the pij_send tool ({ to, message }) — your id is stamped automatically. Add fyi: true to hold a message until the peer's next turn instead of waking them, only when their next action doesn't depend on it; questions, blockers, hand-offs, review verdicts and work done are normal sends. For control, pij_send takes { to, command: "compact" | "new" | "reload" } instead of message. Do NOT shell out to the pij CLI to send.`,
		`To observe peers, use the pij CLI: "pij list --here" (peers), "pij tail <id>" / "pij state <id>" (watch one).`,
	].join("\n");
}
