// pij platform — schema-versioned PUBLIC on-disk contract (plan 054, WS-4).
//
// Imports NOTHING. Project / Assignment / SpineEvent are records external
// tools read and write: every record carries `schema_version: 1` (WS-4 —
// the field is `schema_version`, never "version"); unknown extra fields are
// tolerated (additive / migration-safe, AC-11); absent optionals are
// `undefined` — JSON `null` is never "absent". SpineEvent `kind` is an OPEN
// vocabulary (WS-5); the kinds pij itself emits live here as SPINE_KIND_*.
// Guards are total boolean predicates (Pattern P4, isFocusManifest precedent).

// ─── attribution (WS-5) ────────────────────────────────────────────────────

/** How a spine line's `actor` was established. */
export const ACTOR_PROVENANCES = ["resolved", "asserted"] as const;

export type ActorProvenance = (typeof ACTOR_PROVENANCES)[number];

/** Attribution fields every spine line carries. `SpineEvent` extends this,
 *  so any SpineEvent is assignable wherever an envelope is expected. */
export interface AttributionEnvelope {
	readonly actor: string;
	readonly ts: string;
	readonly refs: readonly string[];
	readonly prev?: string;
	readonly next?: string;
	readonly verifiedBy?: string;
	readonly actorProvenance?: ActorProvenance;
}

// ─── records (WS-4) ────────────────────────────────────────────────────────

/** Orchestrator autonomy policy. Absent means gated for legacy projects. */
export const PROJECT_AUTONOMIES = ["power-through", "gated"] as const;

export type ProjectAutonomy = (typeof PROJECT_AUTONOMIES)[number];

/** One tracked project. */
export interface Project {
	readonly schema_version: 1;
	readonly slug: string;
	readonly description: string;
	readonly repo?: string;
	readonly planPath?: string;
	readonly primeId?: string;
	readonly autonomy?: ProjectAutonomy;
	readonly created: {
		readonly actor: string;
		readonly ts: string;
	};
}

/** Stream-allocation lifecycle. Ordinals remain reserved after close/tombstone. */
export const ALLOCATION_STATES = ["created", "briefed", "closed", "tombstoned"] as const;

export type AllocationState = (typeof ALLOCATION_STATES)[number];

/** One persisted, ordered transaction-journal step for stream stand-up/close. */
export interface AllocationStep {
	readonly name: string;
	readonly ok: boolean;
	readonly evidence: string;
	readonly ts: string;
}

/** One reserved stream allocation. */
export interface Allocation {
	readonly schema_version: 1;
	readonly id: string;
	readonly project: string;
	readonly ordinal: number;
	readonly slug: string;
	readonly worktree: string;
	readonly branch: string;
	readonly baseSha: string;
	readonly state: AllocationState;
	readonly steps: readonly AllocationStep[];
	readonly created: {
		readonly actor: string;
		readonly ts: string;
	};
}

/** Fence classes are descriptive sensors, never enforcement. */
export const FENCE_CLASSES = ["notify-only"] as const;

export type FenceClass = (typeof FENCE_CLASSES)[number];

/** Expected touch-set for one allocation. */
export interface Fence {
	readonly schema_version: 1;
	readonly id: string;
	readonly allocation: string;
	readonly touchSet: readonly string[];
	readonly shared: readonly string[];
	readonly class: FenceClass;
	readonly updated: {
		readonly actor: string;
		readonly ts: string;
	};
}

/** Dispatch lifecycle is separate from the shipped delivery receipt vocabulary. */
export const DISPATCH_STATES = ["undelivered", "delivered-unacked", "acked"] as const;

export type DispatchState = (typeof DISPATCH_STATES)[number];

export const DISPATCH_DELIVERY_STATES = ["queued", "delivered", "unverified"] as const;

export type DispatchDeliveryState = (typeof DISPATCH_DELIVERY_STATES)[number];

export interface DispatchAck {
	readonly schema_version: 1;
	readonly kind: "brief-ack";
	readonly messageId: string;
	readonly packetId: string;
	readonly packetSha256: string;
	readonly declaredRuntime: {
		readonly model: string;
		readonly effort: string;
		readonly source: "self-report";
	};
	readonly seat: string;
	readonly ts: string;
}

export const CANARY_MODEL_CHECKS = ["matched", "unpinned-default"] as const;

export type CanaryModelCheck = (typeof CANARY_MODEL_CHECKS)[number];

/** Mechanical canary legs (a)+(b), attached only to a fully acknowledged
 * dispatch after nonce/sha, identity, and runtime checks all pass. */
export interface CanaryRecord {
	readonly schema_version: 1;
	readonly kind: "canary";
	readonly dispatchId: string;
	readonly nonce: string;
	readonly target: string;
	readonly expectedModel?: string;
	readonly declaredRuntime: DispatchAck["declaredRuntime"];
	readonly modelCheck: CanaryModelCheck;
	readonly contextWindow?: {
		readonly expected: number;
		readonly expectedLabel: string;
		/** The tier the pane actually showed, or `"unverified"` when the harness
		 *  publishes no context marker at all (plan 071 D6). */
		readonly observedLabel: string;
		/** `"unobservable"` when nothing could be read — the check did not run, as
		 *  opposed to running and passing. */
		readonly source: "pane-footer" | "unobservable";
		/** `matched` = observed and agreed. `unverified` = the harness exposes no
		 *  tier, so there is nothing to contradict. There is no passing `mismatched`
		 *  — a contradiction refuses. */
		readonly check: "matched" | "unverified";
	};
	readonly identity: {
		readonly paneId: string;
		readonly pid: number;
		readonly harnessSessionId: string;
	};
	readonly passed: {
		readonly actor: string;
		readonly ts: string;
	};
}

/** One dispatch packet and its delivery/engagement evidence. */
export interface Dispatch {
	readonly schema_version: 1;
	readonly id: string;
	readonly packetPath: string;
	readonly packetSha256: string;
	readonly from: string;
	readonly to: string;
	readonly messageId?: string;
	readonly deliveryState?: DispatchDeliveryState;
	readonly state: DispatchState;
	readonly ack?: DispatchAck;
	readonly canary?: CanaryRecord;
	readonly created: {
		readonly actor: string;
		readonly ts: string;
	};
	readonly updated: {
		readonly actor: string;
		readonly ts: string;
	};
}

/** Why an assignment closed (closed union — unknown reasons are rejected). */
export const ASSIGNMENT_CLOSE_REASONS = ["done", "cancelled", "failed", "superseded"] as const;

export type AssignmentCloseReason = (typeof ASSIGNMENT_CLOSE_REASONS)[number];

/** One unit of work bound to a node. `states` are spine seq refs. */
export interface Assignment {
	readonly schema_version: 1;
	readonly id: string;
	readonly nodeId: string;
	readonly projectSlug?: string;
	readonly task: string;
	readonly states: readonly number[];
	readonly opened: {
		readonly actor: string;
		readonly ts: string;
	};
	readonly closed?: {
		readonly actor: string;
		readonly ts: string;
		readonly reason: AssignmentCloseReason;
	};
}

// ─── spine events (WS-5) ───────────────────────────────────────────────────

/** Kinds pij itself emits. `kind` stays an open string for external writers;
 *  the guard does not enforce this list. */
export const SPINE_KIND_PROJECT_CREATED = "project-created";
export const SPINE_KIND_PROJECT_SET = "project-set";
export const SPINE_KIND_ALLOCATION = "allocation";
export const SPINE_KIND_FENCE = "fence";
export const SPINE_KIND_DISPATCH = "dispatch";
// Assignment coupled-write kinds (plan 054 P2 T005). prev/next on these carry
// canonicalAssignmentJson (states[] excluded — a log-derived index); the
// semantic transition rides in structured refs (`state:<word>`).
export const SPINE_KIND_TASK_SET = "task-set";
/** Discharge of an assignment (plan 075). The far end of the lifecycle
 *  `task-set` opens — absent until now, which is why 91 of 91 assignments on
 *  the first measured box were open and none had ever been closed.
 *  prev/next carry canonicalAssignmentJson like its sibling; the reason rides
 *  in a structured ref (`reason:<word>`) so the discharge is auditable without
 *  parsing the record. */
export const SPINE_KIND_TASK_CLOSE = "task-close";
export const SPINE_KIND_STATE_SET = "state-set";
/** First-person now/next report consumed by the rail and PM watchdog. */
export const SPINE_KIND_STATUS = "status";
/** Removes the current assignment's declaration; this is a transition event,
 * never a member of the closed semantic-state vocabulary. */
export const SPINE_KIND_STATE_CLEARED = "state-cleared";
export const SPINE_KIND_STATE_VERIFIED = "state-verified";
/** Mechanical-axis transition, appended by the DAEMON with `actor: daemon`
 *  (plan 054 P2 T008, V-05). prev/next carry WS-6 SystemState words; s055
 *  consumes this kind by exact name. Uncoupled (telemetry — the axis TRUTH
 *  is the descriptor), so it appends plain, never journaled. */
export const SPINE_KIND_SYSTEM_STATE = "system-state";
/** Re-parent audit event (plan 054 P3 T004, AC-08): `pij link` appends one
 *  per hop with prev=old effective parent, next=new parent, peer=child.
 *  A `--root` link OMITS `next` (the envelope's `next?` is string-typed —
 *  never null/sentinel) and refs `[node:<child>]` only. Uncoupled (V-05:
 *  descriptor is tree truth, the event is history), so it appends plain,
 *  never journaled — under the write lock + recovery gate like any
 *  platform append. */
export const SPINE_KIND_NODE_LINKED = "node-linked";
/** Stored orchestration-role designation history; prev/next carry role words. */
export const SPINE_KIND_ROLE_SET = "role-set";
/** Current/retired prime designation history; prev/next carry prime words. */
export const SPINE_KIND_PRIME_SET = "prime-set";

/** One spine log line. */
export interface SpineEvent extends AttributionEnvelope {
	readonly schema_version: 1;
	readonly seq: number;
	readonly kind: string;
	readonly peer?: string;
	readonly project?: string;
	readonly repo?: string;
}

/** A spine event BEFORE the log stamps its `seq`. Sequence numbers are
 *  allocated inside `SpineLogPort.append`/`appendOnce` (cross-process atomic
 *  — review 001 F1): no caller may mint one from `lastSeq() + 1`. */
export type SpineEventDraft = Omit<SpineEvent, "seq">;

// ─── guards (total, never throw) ───────────────────────────────────────────
// Each exported guard is wrapped in try/catch: throwing accessors / revoked
// proxies (never produced by JSON.parse) return false instead of escaping.

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

// Own-property law (review 001 F6): these guards are trust boundaries for
// non-JSON callers too, so a required field must be an OWN property —
// `Object.create(validRecord)` has none — and a known optional supplied only
// by the prototype chain is a forgery, not an absence. JSON.parse output
// (own data properties throughout, even a literal "__proto__" key) is
// untouched by either rule.

/** Required-field law: own property AND passing the value check. */
function ownField(
	record: Record<string, unknown>,
	key: string,
	isValue: (value: unknown) => boolean,
): boolean {
	return Object.hasOwn(record, key) && isValue(record[key]);
}

/** Known-optional law: absent everywhere, or own and passing the value
 *  check — present on the prototype chain but not own is rejected. */
function ownOptional(
	record: Record<string, unknown>,
	key: string,
	isValue: (value: unknown) => boolean,
): boolean {
	if (!(key in record)) return true;
	return Object.hasOwn(record, key) && isValue(record[key]);
}

/** Optional field law: absent means `undefined`; JSON `null` is rejected. */
function isOptionalString(value: unknown): boolean {
	return value === undefined || typeof value === "string";
}

const isString = (value: unknown): boolean => typeof value === "string";

function isActorStamp(value: unknown): boolean {
	if (!isRecord(value)) return false;
	return ownField(value, "actor", isString) && ownField(value, "ts", isString);
}

/** Spine seq refs are integer-minded log positions, but the V-01 contract
 *  types them as plain `number` — only finiteness is enforced here (NaN and
 *  Infinity are corrupted refs; the tests pin NaN). */
function isSeqRef(value: unknown): boolean {
	return typeof value === "number" && Number.isFinite(value);
}

/** Dense real-Array check. `Array.isArray` gates out TypedArrays; the index
 *  loop (never `.every`, which skips holes) surfaces sparse-array holes as
 *  `undefined` so they fail the item check. */
function isDenseArrayOf(value: unknown, isItem: (item: unknown) => boolean): boolean {
	if (!Array.isArray(value)) return false;
	for (let i = 0; i < value.length; i++) {
		if (!isItem(value[i])) return false;
	}
	return true;
}

function isActorProvenance(value: unknown): value is ActorProvenance {
	return typeof value === "string" && (ACTOR_PROVENANCES as readonly string[]).includes(value);
}

function isAssignmentClosed(value: unknown): boolean {
	if (!isRecord(value)) return false;
	return (
		ownField(value, "actor", isString) &&
		ownField(value, "ts", isString) &&
		ownField(
			value,
			"reason",
			(reason) =>
				typeof reason === "string" &&
				(ASSIGNMENT_CLOSE_REASONS as readonly string[]).includes(reason),
		)
	);
}

function isAllocationState(value: unknown): value is AllocationState {
	return typeof value === "string" && (ALLOCATION_STATES as readonly string[]).includes(value);
}

function isAllocationStep(value: unknown): boolean {
	if (!isRecord(value)) return false;
	return (
		ownField(value, "name", isString) &&
		ownField(value, "ok", (v) => typeof v === "boolean") &&
		ownField(value, "evidence", isString) &&
		ownField(value, "ts", isString)
	);
}

function isFenceClass(value: unknown): value is FenceClass {
	return typeof value === "string" && (FENCE_CLASSES as readonly string[]).includes(value);
}

function isDispatchState(value: unknown): value is DispatchState {
	return typeof value === "string" && (DISPATCH_STATES as readonly string[]).includes(value);
}

function isDispatchDeliveryState(value: unknown): value is DispatchDeliveryState {
	return (
		typeof value === "string" && (DISPATCH_DELIVERY_STATES as readonly string[]).includes(value)
	);
}

function isDispatchAck(value: unknown): value is DispatchAck {
	if (!isRecord(value) || !isRecord(value.declaredRuntime)) return false;
	return (
		ownField(value, "schema_version", (v) => v === 1) &&
		ownField(value, "kind", (v) => v === "brief-ack") &&
		ownField(value, "messageId", isString) &&
		ownField(value, "packetId", isString) &&
		ownField(value, "packetSha256", (v) => typeof v === "string" && /^[a-f0-9]{64}$/.test(v)) &&
		ownField(
			value,
			"declaredRuntime",
			(v) =>
				isRecord(v) &&
				ownField(v, "model", isString) &&
				ownField(v, "effort", isString) &&
				ownField(v, "source", (source) => source === "self-report"),
		) &&
		ownField(value, "seat", isString) &&
		ownField(value, "ts", isString)
	);
}

function isCanaryModelCheck(value: unknown): value is CanaryModelCheck {
	return typeof value === "string" && (CANARY_MODEL_CHECKS as readonly string[]).includes(value);
}

function isCanaryRecord(value: unknown): value is CanaryRecord {
	if (!isRecord(value) || !isRecord(value.declaredRuntime) || !isRecord(value.identity)) {
		return false;
	}
	return (
		ownField(value, "schema_version", (v) => v === 1) &&
		ownField(value, "kind", (v) => v === "canary") &&
		ownField(value, "dispatchId", isString) &&
		ownField(value, "nonce", isString) &&
		ownField(value, "target", isString) &&
		ownOptional(value, "expectedModel", isOptionalString) &&
		ownField(
			value,
			"declaredRuntime",
			(v) =>
				isRecord(v) &&
				ownField(v, "model", isString) &&
				ownField(v, "effort", isString) &&
				ownField(v, "source", (source) => source === "self-report"),
		) &&
		ownField(value, "modelCheck", isCanaryModelCheck) &&
		ownOptional(
			value,
			"contextWindow",
			(v) =>
				v === undefined ||
				(isRecord(v) &&
					ownField(v, "expected", (n) => typeof n === "number" && Number.isFinite(n) && n > 0) &&
					ownField(v, "expectedLabel", isString) &&
					ownField(v, "observedLabel", isString) &&
					// plan 071 D6: the RUNTIME validator is a second gate the type change
					// alone does not reach — it rejected every unverified record with a
					// bare E-ARG. Both variants are valid persisted shapes.
					ownField(
						v,
						"source",
						(source) => source === "pane-footer" || source === "unobservable",
					) &&
					ownField(v, "check", (check) => check === "matched" || check === "unverified")),
		) &&
		ownField(
			value,
			"identity",
			(v) =>
				isRecord(v) &&
				ownField(v, "paneId", isString) &&
				ownField(
					v,
					"pid",
					(pid) => typeof pid === "number" && Number.isSafeInteger(pid) && pid > 0,
				) &&
				ownField(v, "harnessSessionId", isString),
		) &&
		ownField(value, "passed", isActorStamp)
	);
}

function isProjectAutonomy(value: unknown): value is ProjectAutonomy {
	return typeof value === "string" && (PROJECT_AUTONOMIES as readonly string[]).includes(value);
}

export function isProject(value: unknown): value is Project {
	try {
		if (!isRecord(value)) return false;
		return (
			ownField(value, "schema_version", (v) => v === 1) &&
			ownField(value, "slug", isString) &&
			ownField(value, "description", isString) &&
			ownOptional(value, "repo", isOptionalString) &&
			ownOptional(value, "planPath", isOptionalString) &&
			ownOptional(value, "primeId", isOptionalString) &&
			ownOptional(value, "autonomy", (v) => v === undefined || isProjectAutonomy(v)) &&
			ownField(value, "created", isActorStamp)
		);
	} catch {
		return false;
	}
}

export function isAllocation(value: unknown): value is Allocation {
	try {
		if (!isRecord(value)) return false;
		return (
			ownField(value, "schema_version", (v) => v === 1) &&
			ownField(value, "id", isString) &&
			ownField(value, "project", isString) &&
			ownField(
				value,
				"ordinal",
				(v) => typeof v === "number" && Number.isSafeInteger(v) && v >= 1,
			) &&
			ownField(value, "slug", isString) &&
			ownField(value, "worktree", isString) &&
			ownField(value, "branch", isString) &&
			ownField(value, "baseSha", isString) &&
			ownField(value, "state", isAllocationState) &&
			ownField(value, "steps", (v) => isDenseArrayOf(v, isAllocationStep)) &&
			ownField(value, "created", isActorStamp)
		);
	} catch {
		return false;
	}
}

export function isFence(value: unknown): value is Fence {
	try {
		if (!isRecord(value)) return false;
		return (
			ownField(value, "schema_version", (v) => v === 1) &&
			ownField(value, "id", isString) &&
			ownField(value, "allocation", isString) &&
			ownField(value, "touchSet", (v) => isDenseArrayOf(v, isString)) &&
			ownField(value, "shared", (v) => isDenseArrayOf(v, isString)) &&
			ownField(value, "class", isFenceClass) &&
			ownField(value, "updated", isActorStamp)
		);
	} catch {
		return false;
	}
}

export function isDispatch(value: unknown): value is Dispatch {
	try {
		if (!isRecord(value)) return false;
		if (
			!(
				ownField(value, "schema_version", (v) => v === 1) &&
				ownField(value, "id", isString) &&
				ownField(value, "packetPath", isString) &&
				ownField(value, "packetSha256", (v) => typeof v === "string" && /^[a-f0-9]{64}$/.test(v)) &&
				ownField(value, "from", isString) &&
				ownField(value, "to", isString) &&
				ownOptional(value, "messageId", isOptionalString) &&
				ownOptional(value, "deliveryState", (v) => v === undefined || isDispatchDeliveryState(v)) &&
				ownField(value, "state", isDispatchState) &&
				ownOptional(value, "ack", (v) => v === undefined || isDispatchAck(v)) &&
				ownOptional(value, "canary", (v) => v === undefined || isCanaryRecord(v)) &&
				ownField(value, "created", isActorStamp) &&
				ownField(value, "updated", isActorStamp)
			)
		) {
			return false;
		}
		const state = value.state as DispatchState;
		const messageId = value.messageId;
		const deliveryState = value.deliveryState;
		const ack = value.ack;
		const canary = value.canary;
		if (state === "undelivered") {
			return (
				messageId === undefined &&
				deliveryState === undefined &&
				ack === undefined &&
				canary === undefined
			);
		}
		if (
			typeof messageId !== "string" ||
			messageId.length === 0 ||
			!isDispatchDeliveryState(deliveryState)
		) {
			return false;
		}
		if (state === "delivered-unacked") return ack === undefined && canary === undefined;
		if (!isDispatchAck(ack)) return false;
		if (
			!(
				ack.messageId === messageId &&
				ack.packetId === value.id &&
				ack.packetSha256 === value.packetSha256 &&
				ack.seat === value.to
			)
		) {
			return false;
		}
		if (canary === undefined) return true;
		if (!isCanaryRecord(canary)) return false;
		return (
			canary.dispatchId === value.id &&
			canary.target === value.to &&
			canary.declaredRuntime.model === ack.declaredRuntime.model &&
			canary.declaredRuntime.effort === ack.declaredRuntime.effort &&
			canary.declaredRuntime.source === ack.declaredRuntime.source
		);
	} catch {
		return false;
	}
}

export function isAssignment(value: unknown): value is Assignment {
	try {
		if (!isRecord(value)) return false;
		return (
			ownField(value, "schema_version", (v) => v === 1) &&
			ownField(value, "id", isString) &&
			ownField(value, "nodeId", isString) &&
			ownOptional(value, "projectSlug", isOptionalString) &&
			ownField(value, "task", isString) &&
			ownField(value, "states", (v) => isDenseArrayOf(v, isSeqRef)) &&
			ownField(value, "opened", isActorStamp) &&
			ownOptional(value, "closed", (v) => v === undefined || isAssignmentClosed(v))
		);
	} catch {
		return false;
	}
}

export function isSpineEvent(value: unknown): value is SpineEvent {
	try {
		if (!isRecord(value)) return false;
		return (
			ownField(value, "schema_version", (v) => v === 1) &&
			ownField(value, "seq", isSeqRef) &&
			ownField(value, "ts", isString) &&
			ownField(value, "actor", isString) &&
			ownField(value, "kind", isString) &&
			ownField(value, "refs", (v) => isDenseArrayOf(v, (item) => typeof item === "string")) &&
			ownOptional(value, "peer", isOptionalString) &&
			ownOptional(value, "project", isOptionalString) &&
			ownOptional(value, "repo", isOptionalString) &&
			ownOptional(value, "prev", isOptionalString) &&
			ownOptional(value, "next", isOptionalString) &&
			ownOptional(value, "verifiedBy", isOptionalString) &&
			ownOptional(value, "actorProvenance", (v) => v === undefined || isActorProvenance(v))
		);
	} catch {
		return false;
	}
}

// ─── id + slug helpers (AC-01) ─────────────────────────────────────────────

/** The one general assignment every node holds: `asg-general-<nodeId>`. */
export function generalAssignmentId(nodeId: string): string {
	return `asg-general-${nodeId}`;
}

/** Lowercase kebab slug: alnum runs joined by single '-', edges trimmed. */
export function kebabSlug(input: string): string {
	return input
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "-")
		.replace(/^-+|-+$/g, "");
}

/** Cap for a project slug (s057 dogfood): auto-derived slugs are truncated to
 *  this length; an explicit `--slug` must fit it outright. Contract constant —
 *  raising it later is additive, lowering is not. */
export const PROJECT_SLUG_MAX_LENGTH = 48;

/** Whether `slug` is a legal EXPLICIT project slug: strict kebab shape
 *  (lowercase alnum runs joined by single '-', no edge hyphens) within
 *  PROJECT_SLUG_MAX_LENGTH. Never throws. */
export function isValidProjectSlug(slug: string): boolean {
	return slug.length <= PROJECT_SLUG_MAX_LENGTH && /^[a-z0-9]+(-[a-z0-9]+)*$/.test(slug);
}

/** First free slug: base, then base-2, base-3, … (AC-01). */
export function resolveSlugCollision(base: string, taken: ReadonlySet<string>): string {
	if (!taken.has(base)) return base;
	for (let n = 2; ; n++) {
		const candidate = `${base}-${n}`;
		if (!taken.has(candidate)) return candidate;
	}
}

// ─── agent liveness (plan 095, T-1.1/T-1.2) ────────────────────────────────
//
// A pid is a LEASE, not an identity. `isAlive(pid)` sends signal 0 to a number —
// an EXISTENCE test the OS answers about whatever holds that number NOW, not
// about the agent. And the registry pid is not reliably the agent in the first
// place: measured across 23 live seats, the agent sits AT
// the registry pid for 16 of them and one level below it for 7 (the split tracks
// the spawn path, `--session-id` vs `--resume`, not the harness). So the answer
// has to come from IDENTITY in a bounded subtree, and it has to be allowed to
// say it does not know.

/** Three-valued on purpose. `unknown` is a first-class ANSWER — the probe could
 *  not tell — and is never coerced to `absent`. The whole class of harm this
 *  vocabulary exists to prevent is a missing observation being read as an
 *  observation of absence. */
export type AgentLiveness = "alive" | "absent" | "unknown";

/** Which rung of the identity ladder produced the verdict. Machine-stable so a
 *  consumer can distinguish "we looked and nothing was there" from "we could not
 *  look" without string-matching prose. */
export type AgentLivenessCause =
	/** No usable process table — capture failed, or the legacy probe threw. */
	| "probe-unavailable"
	/** A process in the subtree carries this seat's PARSED session id. Exact. */
	| "session-id-match"
	/** A harness process is present and no id comparison was possible. Corroborating. */
	| "harness-process-present"
	/** Every harness process in the subtree carries a DIFFERENT seat's id. Exact negative. */
	| "foreign-session-id"
	/** A harness process's command line is truncated or unparseable — we cannot tell. */
	| "identity-indeterminate"
	/** Nothing harness-like anywhere in the bounded subtree. The common true death. */
	| "no-harness-process"
	/** Legacy pid-existence probe said the pid exists. Weak by construction. */
	| "pid-present"
	/** Legacy pid-existence probe said the pid is gone. Weak by construction. */
	| "pid-missing";

export interface AgentLivenessProbe {
	readonly liveness: AgentLiveness;
	readonly cause: AgentLivenessCause;
	/** The process the verdict was drawn from, when one was identified. */
	readonly agentPid?: number;
	/** Human-readable. NOT a contract — never parse this. */
	readonly detail: string;
}

/** One row of an OS process table. */
export interface ProcessInfo {
	readonly pid: number;
	readonly ppid: number;
	/** The FULL command line. `ps -ww` is mandatory upstream: a truncated command
	 *  line is MISSING EVIDENCE, not evidence of absence. */
	readonly command: string;
	/** Epoch ms of process start, when the capture could parse one. Corroboration
	 *  only — see `resolveAgentLiveness`: start time may never demote a match. */
	readonly startedAtMs?: number;
	/** The capture could not guarantee this row's full argv (width-limited or
	 *  unparseable). Forces `unknown` rather than a fabricated `absent`. */
	readonly truncated?: boolean;
}

/** An immutable, WHOLE-TABLE capture, taken ONCE PER SWEEP.
 *
 *  Whole-table and once-per-sweep are correctness requirements, not
 *  optimisations: the death sweep runs on the ~600ms daemon tick over ~500
 *  descriptors, so a per-descriptor `ps` is ~500 process-table spawns per tick,
 *  which stalls the tick and therefore message delivery. Passing a VALUE rather
 *  than a callback is what makes the per-descriptor shape unwritable.
 *
 *  Tagged union because a FAILED capture and an EMPTY table are different facts
 *  and must never collapse into each other. */
export type ProcessSnapshot =
	| {
			readonly ok: true;
			readonly capturedAtMs: number;
			readonly processes: readonly ProcessInfo[];
	  }
	| { readonly ok: false; readonly reason: string };
