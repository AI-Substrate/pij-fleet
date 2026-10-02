import type { CallerContext } from "./generation-routing.js";

export const PIJ_ENVELOPE_VERSION = 2;
export const PIJ_EVENT_VERSION = 1;

export type DaemonErrorKind =
	| "refused"
	| "not_found"
	| "auth"
	| "skew"
	| "adapter"
	| "cursor_reset";

export interface DaemonEnvelope<T> {
	readonly ok: boolean;
	readonly command: string;
	readonly v: number;
	readonly data?: T;
	readonly meta?: string;
	readonly error?: DaemonErrorKind;
}
export type SuccessfulDaemonEnvelope<T> = DaemonEnvelope<T> & {
	readonly ok: true;
	readonly data: T;
};

export interface HealthPayload {
	readonly status: string;
	readonly build: string;
	readonly offline: boolean;
	readonly machine: string;
	/** Absent on older daemons; absence is not permission to launch. */
	readonly retired_harnesses?: readonly Registration["harness"][];
}

export interface Registration {
	readonly supersedes?: string;
	readonly id: string;
	readonly harness: "pi" | "omp" | "claude" | "copilot" | "codex";
	readonly harness_session?: string;
	readonly extension_build?: string;
	readonly extension_path?: string;
	readonly folder: string;
	readonly pane?: string;
	readonly pid?: number;
	readonly proc_start?: number;
	readonly spawn_id?: string;
	readonly model?: string;
	readonly actual_model?: string;
	readonly actual_model_observed?: boolean;
	readonly provider?: string;
	readonly effort?: string;
	readonly parent?: string;
	readonly relay: boolean;
}

export interface RustSeatDescriptor {
	readonly id: string;
	readonly harness: string;
	readonly extension_build?: string | null;
	readonly extension_path?: string | null;
	readonly pane?: string;
	readonly proc?: { readonly pid: number; readonly proc_start: number };
	readonly proc_source?: "harness" | "pane";
	readonly folder: string;
	readonly state: string;
	readonly parent?: string;
	readonly spawn_id?: string;
	/** Register-response-only addition; not persisted into the local registry. */
	readonly typing_grace_ms?: number;
	readonly binding?: "created" | "rebound" | "same";
}

export interface SendRequest {
	readonly from: string;
	readonly to: { readonly seat: string; readonly machine?: string };
	readonly body: string;
	readonly msg_id: string;
	readonly in_reply_to?: string;
	readonly command?: string;
	readonly caller?: CallerContext;
	/** Plan 158: hold until the recipient's next turn; the daemon refuses it with `command`. */
	readonly fyi?: true;
	/** Plan 157 phase 2: wake a cold recipient anyway; the daemon refuses it without `reason`. */
	readonly force?: true;
	/** Why a forced cold wake is worth its price; recorded on the spine. */
	readonly reason?: string;
}

/** The code opening every cold-wake refusal's meta (crates/core/src/cold_wake.rs). */
export const COLD_WAKE_CODE = "E-RS-COLD-WAKE";

export interface QueueReceipt {
	readonly msg_id: string;
	readonly outcome: unknown;
	readonly at: number;
	/** The cold-wake guard's verdict: `clear`, `busy`, `forced` or `unknown: <why>`.
	 *  Absent when the guard did not run (FYIs, controls, forwarded sends). */
	readonly cold_check?: string;
	/** Plan 159: set only when a held FYI's body contains `?`; the send is still held. */
	readonly warning?: string;
}

/** A held FYI receipt carries `outcome: {outcome: "held", reason: "fyi"}`. */
export function isHeldFyi(receipt: QueueReceipt): boolean {
	const outcome = receipt.outcome;
	if (typeof outcome !== "object" || outcome === null) return false;
	const record = outcome as Record<string, unknown>;
	return record.outcome === "held" && record.reason === "fyi";
}

/** POST /v1/fyi/claim; `pane` or `native_session` is the binding evidence. */
export interface FyiClaimRequest {
	readonly seat?: string;
	readonly pane?: string;
	readonly native_session?: string;
	readonly via: "hook:claude" | "hook:copilot" | "hook:omp" | "hook:pi";
}

/** `block` is the daemon's golden FYI block, passed through verbatim; "" when count is 0. */
export interface FyiClaim {
	readonly seat: string;
	readonly count: number;
	readonly block: string;
	readonly ids: readonly string[];
}

/** POST /v1/activity: the seat's turn state, bound like an fyi claim. */
export interface ActivityRequest {
	readonly seat: string;
	readonly native_session?: string;
	readonly pane?: string;
	readonly state: "working" | "idle";
}

export interface HoldRequest {
	readonly seat: string;
	readonly job_id: number;
	readonly msg_id: string;
	readonly reason: "human-typing";
	readonly since_ms: number;
}

export interface HoldResponse {
	readonly msg_id: string;
	readonly held: boolean;
}

export interface ReleaseRequest {
	readonly seat: string;
	readonly job_id: number;
	readonly msg_id: string;
	readonly at_ms: number;
}

export interface ReleaseResponse {
	readonly msg_id: string;
	readonly released: boolean;
}

export interface WireEvent {
	readonly v: number;
	readonly at: number;
	readonly kind: string;
	readonly seat?: string;
	readonly payload: string;
}

export interface EventFrame {
	readonly machine: string;
	readonly cursor: number;
	readonly event: WireEvent;
}

export interface HelloLine {
	readonly hello: true;
	readonly v: number;
	readonly build: string;
}

export class PijNoDaemonError extends Error {
	readonly code = "PIJ_NO_DAEMON";

	constructor(
		readonly addr: string,
		readonly stateDir: string,
	) {
		super(
			`No pij daemon answered at ${addr}. Start pij-rs there (\`just bounce-rs\`) or set PIJ_RS_ADDR. ` +
				`The state directory came from PIJ_RS_STATE_DIR, then HOME/.pij-rs (${stateDir}).`,
		);
		this.name = "PijNoDaemonError";
	}
}

export class PijWireSkewError extends Error {
	readonly code = "PIJ_WIRE_SKEW";

	constructor(
		readonly found: number,
		readonly supported: number,
	) {
		super(
			`pij daemon wire v${found} is unsupported by this extension (v${supported}); upgrade pij.`,
		);
		this.name = "PijWireSkewError";
	}
}

export class DaemonRefusalError extends Error {
	constructor(
		readonly command: string,
		readonly kind: DaemonErrorKind | undefined,
		message: string,
		readonly rawEnvelope?: string,
	) {
		super(message);
		this.name = "DaemonRefusalError";
	}
}

function object(value: unknown, label: string): Record<string, unknown> {
	if (typeof value !== "object" || value === null || Array.isArray(value)) {
		throw new Error(`${label} must be an object`);
	}
	return value as Record<string, unknown>;
}

function string(value: unknown, label: string): string {
	if (typeof value !== "string") throw new Error(`${label} must be a string`);
	return value;
}

function finiteInteger(value: unknown, label: string): number {
	if (typeof value !== "number" || !Number.isSafeInteger(value) || value < 0) {
		throw new Error(`${label} must be a non-negative safe integer`);
	}
	return value;
}

export function decodeEnvelope<T>(text: string): SuccessfulDaemonEnvelope<T> {
	let parsed: unknown;
	try {
		parsed = JSON.parse(text);
	} catch (error) {
		throw new Error(`daemon returned invalid JSON: ${String(error)}`);
	}
	const value = object(parsed, "daemon envelope");
	const version = finiteInteger(value.v, "daemon envelope v");
	if (version !== PIJ_ENVELOPE_VERSION) {
		throw new PijWireSkewError(version, PIJ_ENVELOPE_VERSION);
	}
	if (typeof value.ok !== "boolean") throw new Error("daemon envelope ok must be boolean");
	const command = string(value.command, "daemon envelope command");
	const kind = value.error;
	if (
		kind !== undefined &&
		kind !== "refused" &&
		kind !== "not_found" &&
		kind !== "auth" &&
		kind !== "skew" &&
		kind !== "adapter" &&
		kind !== "cursor_reset"
	) {
		throw new Error("daemon envelope error must be a known ErrorKind");
	}
	const envelope: DaemonEnvelope<T> = {
		ok: value.ok,
		command,
		v: version,
		...(value.data === undefined ? {} : { data: value.data as T }),
		...(typeof value.meta === "string" ? { meta: value.meta } : {}),
		...(kind === undefined ? {} : { error: kind }),
	};
	if (!envelope.ok) {
		throw new DaemonRefusalError(
			command,
			envelope.error,
			envelope.meta ?? `${command} failed`,
			text,
		);
	}
	if (envelope.data === undefined) throw new Error(`${command} succeeded without data`);
	return envelope as SuccessfulDaemonEnvelope<T>;
}

export function decodeStreamLine(text: string): HelloLine | EventFrame {
	let parsed: unknown;
	try {
		parsed = JSON.parse(text);
	} catch (error) {
		throw new Error(`daemon event line is invalid JSON: ${String(error)}`);
	}
	const value = object(parsed, "daemon event line");
	if (value.hello === true) {
		const version = finiteInteger(value.v, "daemon hello v");
		if (version !== PIJ_EVENT_VERSION) throw new PijWireSkewError(version, PIJ_EVENT_VERSION);
		return { hello: true, v: version, build: string(value.build, "daemon hello build") };
	}
	const rawEvent = object(value.event, "daemon event frame event");
	const eventVersion = finiteInteger(rawEvent.v, "daemon event v");
	if (eventVersion !== PIJ_EVENT_VERSION) {
		throw new PijWireSkewError(eventVersion, PIJ_EVENT_VERSION);
	}
	return {
		machine: string(value.machine, "daemon event frame machine"),
		cursor: finiteInteger(value.cursor, "daemon event frame cursor"),
		event: {
			v: eventVersion,
			at: finiteInteger(rawEvent.at, "daemon event at"),
			kind: string(rawEvent.kind, "daemon event kind"),
			...(rawEvent.seat === undefined || rawEvent.seat === null
				? {}
				: { seat: string(rawEvent.seat, "daemon event seat") }),
			payload: string(rawEvent.payload, "daemon event payload"),
		},
	};
}

const MONTHS: Readonly<Record<string, number>> = {
	Jan: 1,
	Feb: 2,
	Mar: 3,
	Apr: 4,
	May: 5,
	Jun: 6,
	Jul: 7,
	Aug: 8,
	Sep: 9,
	Oct: 10,
	Nov: 11,
	Dec: 12,
};

/** Pack C-locale `ps -o lstart=` as YYYYMMDDhhmmss, matching pij-rs ProcLiveness. */
export function parseProcessStart(row: string): number {
	const match =
		/^(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun) ([A-Z][a-z]{2})\s+(\d{1,2}) (\d{2}):(\d{2}):(\d{2}) (\d{4})$/.exec(
			row.trim(),
		);
	if (!match) throw new Error(`could not parse process start: ${row}`);
	const month = MONTHS[match[1] ?? ""];
	const day = Number(match[2]);
	const hour = Number(match[3]);
	const minute = Number(match[4]);
	const second = Number(match[5]);
	const year = Number(match[6]);
	if (
		month === undefined ||
		year < 1970 ||
		day < 1 ||
		day > daysInMonth(year, month) ||
		hour > 23 ||
		minute > 59 ||
		second > 59
	) {
		throw new Error(`could not parse process start: ${row}`);
	}
	return (
		year * 10_000_000_000 +
		month * 100_000_000 +
		day * 1_000_000 +
		hour * 10_000 +
		minute * 100 +
		second
	);
}

function daysInMonth(year: number, month: number): number {
	if (month === 2) {
		return year % 400 === 0 || (year % 4 === 0 && year % 100 !== 0) ? 29 : 28;
	}
	return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}
