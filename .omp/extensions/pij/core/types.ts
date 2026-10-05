// pij-messaging — shared domain types (Pattern P2: pi-free).
//
// Imports NOTHING from @earendil-works/*. Plain data + structural types only.
// These are the vocabulary the whole core speaks; ports (ports.ts) and the
// pure logic modules (seq/events/state/commands/discovery/message/receipts)
// all build on these.

import type { StoredOrchestrationRole } from "./orchestration/role.js";

// ─── identity ────────────────────────────────────────────────────────────
export type SessionId = string;

/** Role a session plays in the parent/worker loop (rides in PIJ_ROLE). */
export type Role = "parent" | "worker";

type Exact<Left, Right> =
	(<Value>() => Value extends Left ? 1 : 2) extends <Value>() => Value extends Right ? 1 : 2
		? true
		: false;
type Assert<Condition extends true> = Condition;

/** Compiled invariant: widening Role silently breaks PIJ_ROLE boot narrowing. */
export type RoleExactnessInvariant = Assert<Exact<Role, "parent" | "worker">>;

/** Which coding-agent harness a session runs under (Plan 019 control plane).
 *  The transport-selection contract (`HarnessKind` → inbox|sendkeys) lives in
 *  `core/harness/types.ts`; the kind is declared here because it is part of the
 *  shared `SessionDescriptor` vocabulary. Absent ⇒ legacy pi session.
 *  `codex` (Plan 022) is the 4th spawnable harness — a discovery-bound,
 *  send-keys-driven, claude-style harness with its own transcript layout. */
export type HarnessKind = "pi" | "claude" | "copilot" | "codex";
/** Delivery ownership for an external harness. Absent preserves legacy behavior:
 * pi self-consumes its inbox; claude/copilot/codex use daemon tmux push. */
export type DeliveryMode = "push" | "pull";

/** Immutable metadata for one saved native-session focus snapshot. */
export interface FocusManifest {
	readonly version: 1;
	readonly name: string;
	readonly harness: HarnessKind;
	readonly harnessSessionId: string;
	readonly model?: string;
	readonly effort?: string;
	readonly originCwd: string;
	readonly sha256: string;
	readonly createdAt: string;
	readonly lineage: {
		readonly sourcePijId: SessionId;
		readonly sourceHarnessSessionId: string;
	};
}

/** Spawn→bind lifecycle of a control-plane session (Plan 019). DISTINCT from
 *  `SessionDescriptor.state` (working/idle of a live turn): this tracks the
 *  pre-bind handoff `pending` → `ready` → `bound`, or `failed` on watchdog
 *  timeout. Absent ⇒ a session that never went through control-plane spawn. */
export type SessionLifecycle = "pending" | "ready" | "bound" | "failed" | "dissolved";

// ─── fail-loud death-reason vocabulary ───────────────────────────────────────
/** Machine-stable reason a spawned session failed to bind or died after binding.
 *  Used in `SessionDescriptor.failureReason` and `classifyDeathReason`. */
export type DeathReason =
	| "model-not-supported" // harness rejected the --model (400/not_found_error)
	| "auth" // authentication failure (401)
	| "quota" // terminal quota: credit/billing/insufficient (429/529/overloaded are transient → unknown)
	| "stalled" // shared whole-life/watchdog verdict: peer stayed silent through its response threshold
	| "pane-input-blocked" // boot line never written: a human was typing in the pane (s069)
	| "bind-timeout" // spawned, pane alive, but never bound inside the bind window (s071 D3)
	| "dead" // pane exited (no specific error signal)
	| "unknown"; // fallback when no pattern matched

/** A pij-owned teardown intent, persisted before the pane is killed or its
 * descriptor is dissolved. It is not an assertion about why a process stopped. */
export interface CloseIntent {
	readonly actor: string;
	readonly kind: "cli-close" | "in-process-close" | "once-close" | "session-close";
	readonly requestedAt: string;
}

/** `unrequested-by-pij` means only an observed absence without a persisted
 * pij close intent. It never names a crash cause or an external actor. */
export type TerminalDisposition = "requested" | "unrequested-by-pij" | "unavailable";
export type TerminalEvidence =
	| "pid-missing"
	| "pane-missing"
	| "expectation-expired"
	| "observation-unavailable";

export interface TerminalObservation {
	readonly disposition: TerminalDisposition;
	readonly observedAt: string;
	readonly evidence: TerminalEvidence;
	readonly lastSeenAt?: string;
	readonly unavailableReason?: string;
}

// ─── liveness + state (vocabulary aligned with agent-workbench) ───────────
/** Self-reported working state of a session. */
export type SessionState = "idle" | "in-progress" | "paused" | "reviewing" | "complete" | "error";

/** Liveness verdict derived from pid probe + latest-event age. */
export type LivenessVerdict = "active" | "stale" | "dead" | "dissolved";

// ─── two-axis node truth (plan 054 Phase 2; WS-6 — HUMAN-RULED vocabulary) ──
// Both arrays are ruled words, byte-exact: never extend, rename, or reorder
// (s055 watchdog consumes `systemState` by these names; core/types.test.ts
// pins the full vocabulary so any drift is a loud red).

/** Semantic axis — declared per-assignment by agents through pij (WS-6). */
export const SEMANTIC_STATES = [
	"blocked",
	"question",
	"hold",
	"waiting",
	"ready",
	"failed",
	"cancelled",
	"done",
] as const;
export type SemanticState = (typeof SEMANTIC_STATES)[number];

/** Mechanical axis — computed ONLY by pij's own probes (WS-5 retained
 *  exception): honest `unknown` over any heuristic guess (AC-04). */
export const SYSTEM_STATES = [
	"starting",
	"working",
	"idle",
	"stalled",
	"stopped",
	"dead",
	"unknown",
] as const;
export type SystemState = (typeof SYSTEM_STATES)[number];

/** One context-window gauge reading (AC-09). `value` is a REAL reading — a
 *  finite token count — or the honest literal `"unknown"` (no source, e.g.
 *  copilot); never an estimate, never NaN/null. `provenance` names where the
 *  reading came from; `asOf` is the ISO-8601 instant it was taken. */
export interface ContextGauge {
	readonly value: number | "unknown";
	readonly asOf: string;
	readonly provenance: string;
}

export function isSemanticState(value: unknown): value is SemanticState {
	return typeof value === "string" && (SEMANTIC_STATES as readonly string[]).includes(value);
}

export function isSystemState(value: unknown): value is SystemState {
	return typeof value === "string" && (SYSTEM_STATES as readonly string[]).includes(value);
}

/** Total guard, own-property law (platform/types.ts precedent — helpers are
 *  duplicated privately here because types.ts imports NOTHING). */
export function isContextGauge(value: unknown): value is ContextGauge {
	try {
		if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
		const record = value as Record<string, unknown>;
		const gaugeValue = record.value;
		return (
			Object.hasOwn(record, "value") &&
			(gaugeValue === "unknown" ||
				(typeof gaugeValue === "number" && Number.isFinite(gaugeValue))) &&
			Object.hasOwn(record, "asOf") &&
			typeof record.asOf === "string" &&
			Object.hasOwn(record, "provenance") &&
			typeof record.provenance === "string"
		);
	} catch {
		return false;
	}
}

// ─── registry descriptor (the ~/.pij/<id>.json a peer reads) ──────────────
export interface SessionDescriptor {
	readonly id: SessionId;
	readonly role?: Role;
	/** Honor-system orchestration designation. Absent means legacy/not prime. */
	readonly prime?: boolean;
	/** Retired orchestration designation. Absent means legacy/not old-prime. */
	readonly oldPrime?: boolean;
	/** Structural tree parent. An id is an explicit parent, `null` is an explicit
	 * root, and absence falls back read-only to legacy `spawnedBy`. */
	readonly parentId?: SessionId | null;
	/** Canonical absolute git common directory, shared by linked worktrees. */
	readonly gitCommonDir?: string;
	/** Absolute project folder the session runs in (used by --here filter). */
	readonly folder: string;
	/** Absolute data dir: ~/.pij/<id>/ holding events.ndjson + state.json. */
	readonly dataDir: string;
	/** Absolute path to events.ndjson (pij path <id>). */
	readonly eventsPath: string;
	/** OS process id, for the liveness pid probe. */
	readonly pid: number;
	/** ISO-8601 session start time. */
	readonly startedAt: string;
	/** Coarse working/idle signal (D-A): `working` between turn_start and
	 *  turn_end, else `idle`. Lets `pij state` report working/static without a
	 *  stream parse (AC-9); `working` + a stale `lastEventAt` = a stall (AC-7a). */
	readonly state?: "working" | "idle";
	/** ISO-8601 timestamp of this session's newest captured event (D-A) — the
	 *  cheap age source for liveness/stall, read straight from the descriptor. */
	readonly lastEventAt?: string;
	/** ISO-8601 timestamp of the daemon's latest pass over this control-plane
	 *  session. Absent for legacy descriptors and pi-owned delivery. */
	readonly lastTickAt?: string;
	/** ISO-8601 timestamp of this seat's most recent inbox delivery poll scan,
	 *  persisted at a coarse ~2500ms cadence (in-memory tracking is per-scan).
	 *  Poll-primary delivery's LIVENESS heartbeat (plan 057 thread-1): the daemon's
	 *  `inbox-poll-stalled` anomaly flags a bound seat whose stamp is older than
	 *  the stall threshold — a stalled delivery poll (long synchronous block, or a
	 *  seat that stopped draining) is the poll-primary analogue of a silent
	 *  fs.watch drop, now OBSERVABLE. Absent on seats that don't self-poll (tmux
	 *  seats are drained by the daemon tick). */
	readonly lastInboxScanAt?: string;
	/** ISO-8601 — a compaction was fired at this pane (`pij compact-self`
	 *  best-effort self-mark, or a daemon-injected remote `/compact`). While
	 *  fresh (see isCompacting) the daemon HOLDS inbox drain — the harness eats
	 *  input injected mid-compact (DL-004) — so messages stay durable-unread and
	 *  flush after compaction. Cleared by the daemon once the pane reads ready
	 *  again past a short grace, or unconditionally past the staleness bound. */
	readonly compactingAt?: string;
	/** ISO-8601 timestamp of the latest delivered watchdog turn. Descriptor-owned
	 *  axis truth; absent before the first fire and for legacy descriptors. */
	readonly lastWatchdogFireAt?: string;
	/** Tmux pane id (%N) of this session's window, self-recorded by the child
	 *  from $TMUX_PANE at fresh boot (§H1). Present iff spawned via pij_spawn.
	 *  Used by PijSession.close() to killPane. */
	readonly paneId?: string;
	/** SessionId of the session that spawned this one (= PIJ_ANNOUNCE_TO at
	 *  fresh boot). Present iff this is a spawned worker session. */
	readonly spawnedBy?: SessionId;
	/** Deliberate-silence class (Plan 056): a relay/bridge/control-plane
	 *  infrastructure peer whose inbox forwards to an external sink (e.g. the
	 *  pij-telegram bridge → the operator's phone). Its idleness is correct by
	 *  design, never a stall — so the watchdog must NEVER watch it. A watchdog
	 *  nudge into such a peer becomes a real-world message (the 20-nudge
	 *  incident). Absent ⇒ ordinary watchable peer. */
	readonly relay?: boolean;
	// ─── control plane (Plan 019; all optional ⇒ migration-safe) ──────────
	/** Which harness this session runs (`pi`/`claude`/`copilot`). Absent ⇒
	 *  legacy pi session. Decides the message transport (inbox vs send-keys). */
	readonly harness?: HarnessKind;
	/** Native executable and resume-store discriminator for this TypeScript descriptor.
	 * Rust registration persists it as the distinct "pi" or "omp" harness. */
	readonly runtimeBin?: "pi" | "omp";
	/** Explicit pull peers keep durable mail for `pij inbox`; absent is legacy. */
	readonly deliveryMode?: DeliveryMode;
	/** The harness-native session id bound to this pij-id — for Claude, the
	 *  transcript stem under ~/.claude/projects/<mangled cwd>/<this>.jsonl,
	 *  discovered deterministically by the daemon (AC-03). Drives `pij tail`. */
	readonly harnessSessionId?: string;
	/** Codex only (Plan 022): the ABSOLUTE path to the discovered rollout `*.jsonl`,
	 *  persisted at bind. Codex's transcript dir is date-nested + global, so a bare
	 *  `harnessSessionId` (the trailing UUID) cannot reconstruct the path the way
	 *  claude's stem→`transcriptPathFor` join can (Finding 06) — `pij tail` reads
	 *  this file directly. Absent for claude (stem-derived) and copilot/pi. */
	readonly transcriptPath?: string;
	/** Copilot only: the session UUID pij CHOSE at spawn and passed to
	 *  `copilot --session-id <uuid>`. Copilot lets us set a new session's UUID, so
	 *  binding is deterministic at spawn (no transcript-discovery race like Claude).
	 *  The daemon binds to this once the pane is ready + initialised; it then
	 *  becomes `harnessSessionId`. Absent for claude/pi. */
	readonly plannedHarnessSessionId?: string;
	/** ISO-8601 — set the moment the daemon injects the init exactly once.
	 *  Persisted so a daemon restart never re-injects (init-exactly-once, AC-02/12). */
	readonly initInjectedAt?: string;
	/** Spawn→bind lifecycle (pending/ready/bound/failed) — distinct from
	 *  `state` (working/idle). The daemon dir-watch keys on `pending` (AC-01). */
	readonly lifecycle?: SessionLifecycle;
	/** Transcript `*.jsonl` paths present in the cwd's project dir at the instant
	 *  of spawn — captured BEFORE the pane exists so new-path discovery is truly
	 *  deterministic (AC-03). Without this the daemon's first-tick snapshot races
	 *  Claude's early transcript write (dogfood review H1). Cleared once bound. */
	readonly transcriptsAtSpawn?: readonly string[];
	/** Branch-from-self (Plan 020): the source harness session id this session was
	 *  forked from at spawn (claude `--resume <src> --fork-session`). Observability
	 *  only — binding keys on `plannedHarnessSessionId`. Absent for a normal spawn. */
	readonly branchedFrom?: string;
	// ─── fail-loud model layer (additive — migration-safe) ────────────────
	/** Model pinned by spawn, replaced by the actual harness-footer model once
	 *  captured (which may differ if the harness substituted a fallback).
	 *  Absent for legacy/default-model descriptors. */
	readonly boundModel?: string;
	/** Provider resolved before spawn. Separate from `boundModel` because model ids
	 * can collide across providers. Absent for legacy/default-model descriptors. */
	readonly boundProvider?: string;
	/** Reasoning effort pinned by spawn. Unlike a model footer this cannot be
	 *  inferred reliably after launch, so the registry is authoritative.
	 *  Absent for legacy/default-effort descriptors. */
	readonly effort?: string;
	/** Machine-stable failure reason set by the daemon when lifecycle → failed.
	 *  model-not-supported|auth|quota|stalled|dead|unknown. Absent until failed. */
	readonly failureReason?: DeathReason;
	/** Spawn correlation joining this descriptor to its pre-launch expectation. */
	readonly spawnId?: string;
	/** Persisted before pij-owned teardown. */
	readonly closeIntent?: CloseIntent;
	/** Durable, evidence-based terminal absence classification. */
	readonly terminal?: TerminalObservation;
	/** A dissolved native session has been relaunched but still awaits golden
	 * recall proof. Also selects the non-continuing revive init reframe. */
	readonly revivePendingAt?: string;
	/** Once-latch for the creator terminal notice across daemon recreation. */
	readonly deathNoticeLatchedAt?: string;
	// ─── agent-pack peer layer (Plan 029 Phase 3; additive — migration-safe) ──
	/** The agent pack slug this peer runs (`pij agent spawn <slug>`), or `"inline"`
	 *  for a `--prompt` spawn. Absent ⇒ not an agent-pack peer (a plain colleague). */
	readonly agentPack?: string;
	/** The resolved pack directory captured at spawn — the discovered pack's dir, or
	 *  `~/.pij/<id>/pack/` for an inline (`--prompt`) spawn. Lets `pij agent report`
	 *  and the daemon locate the pack's schema without re-running discovery. */
	readonly agentPackDir?: string;
	/** `--once`/pack `lifecycle: once` — the daemon closes this peer's pane after
	 *  its first report push is durable (T008). Absent/false ⇒ resident (default). */
	readonly agentOnce?: boolean;
	/** ISO-8601 — stamped by `pij agent report` the moment a VALID report is pushed
	 *  to the spawner. Drives the `--once` auto-close latch (`agentOnce && reportedAt`,
	 *  T005 planOnceClose). Re-stamped on each subsequent report. */
	readonly reportedAt?: string;
	// ─── node truth (plan 054 Phase 2; all optional ⇒ migration-safe) ─────
	/** Assignment id this node is currently pointed at (`pij task set`). The
	 *  full record lives in the assignment store; this is a UI denorm. */
	readonly currentAssignment?: string;
	/** Denormalized task text of the current assignment (`pij task set`). */
	readonly currentTask?: string;
	/** Opaque plan identifier explicitly attested by `pij spawn --plan-id` or
	 * `pij attest --plan-id`. Absent means unattested; never inferred from paths. */
	readonly planId?: string;
	/** Denormalized semantic state of the current assignment (`pij report state`)
	 *  — agent-declared truth, externally owned (never daemon-clobbered). */
	readonly semanticState?: SemanticState;
	/** Durable explanation attached to the current semantic state. */
	readonly stateNote?: {
		readonly text: string;
		readonly state: SemanticState;
		readonly at: string;
	};
	/** Previous projected status word. */
	readonly statusPrev?: string;
	/** Next projected status word. */
	readonly statusNext?: string;
	/** ISO-8601 timestamp of the projected status transition. */
	readonly statusAt?: string;
	/** Monotonic sequence of the projected status transition. */
	readonly statusSeq?: number;
	/** WHO WROTE the card, when that is not the seat it is about (plan 078).
	 *
	 *  Absent for a self-authored card, which is the overwhelming case and keeps
	 *  every existing descriptor byte-identical. Set to the writer's id when a PA
	 *  relays its prime's card, so the rail can render "via <pa-id>".
	 *
	 *  The SPINE already distinguished these — a status event carries `actor`
	 *  (who wrote) and `peer` (whose card) as separate fields. The denorm did
	 *  not, so any consumer reading the descriptor instead of replaying the log
	 *  saw a relayed card as self-authored. That is an attribution silence, and
	 *  a PA that can write its prime's card without it is indistinguishable from
	 *  the prime — which is why a capability gate that refuses `close` while
	 *  permitting identity-borrowing is not read-only by construction. */
	readonly statusWrittenBy?: string;
	/** Stored partial orchestration role; prime remains a separate descriptor flag. */
	readonly orchestrationRole?: StoredOrchestrationRole;
	/** Mechanical axis verdict — computed and owned by the DAEMON only (WS-5
	 *  exception): deliberately NOT in MUTABLE_EXTERNALLY_OWNED_FIELDS. */
	readonly systemState?: SystemState;
	/** Tmux window id (@N) holding this node's pane — captured at spawn/adopt,
	 *  daemon-backfilled for legacy live nodes (AC-09 terminal addressability:
	 *  `tmux select-window -t <windowId>`). */
	readonly windowId?: string;
	/** Context-window capacity (tokens) joined from the models registry via
	 *  `boundModel`. Absent when the model is unknown to the registry. */
	readonly contextMax?: number;
	/** Latest context-usage gauge reading — real or honest-unknown (AC-09). */
	readonly contextCurrent?: ContextGauge;
}

// ─── durable spawn expectation ──────────────────────────────────────────────

/** Spawn intent persisted before every launch attempt, keyed by PIJ_SPAWN_ID. */
export interface SpawnExpectation {
	readonly spawnId: string;
	readonly creatorId?: SessionId;
	readonly requestedHarness: HarnessKind | "omp";
	readonly requestedAt: string;
	/** Bounded registration window: expiry is a no-show observation, not a harness cause. */
	readonly deadlineAt?: string;
	readonly paneId?: string;
	readonly sessionId?: SessionId;
	readonly runtimeHarness?: HarnessKind;
	readonly boundAt?: string;
	readonly closeIntent?: CloseIntent;
	readonly terminal?: TerminalObservation;
	readonly deathNoticeLatchedAt?: string;
}

// ─── session-tree projection ────────────────────────────────────────────────

/** Orchestration-facing activity axis used by list/tree filters. */
export type TreeActivity = "working" | "idle" | "done";

/** A descriptor paired with the live values computed by the caller. */
export interface TreeSession {
	readonly descriptor: SessionDescriptor;
	readonly activity: TreeActivity;
	readonly liveness: LivenessVerdict;
}

/** Tree filters compose as OR within one axis and AND across axes. */
export interface TreeFilters {
	readonly activity?: readonly TreeActivity[];
	readonly liveness?: readonly LivenessVerdict[];
	readonly lifecycle?: readonly SessionLifecycle[];
	/** Include dead/dissolved history when no explicit history filter is present. */
	readonly all?: boolean;
}

/** Optional selection applied before filters and projection. */
export interface TreeProjectionOptions {
	/** Restrict projection to this preselected set (for repository views). */
	readonly selectedIds?: readonly SessionId[];
	/** Restrict projection to one node and its descendants. */
	readonly rootId?: SessionId;
	readonly filters?: TreeFilters;
}

export type TreeProblem = "orphan" | "filtered-parent" | "cycle";

/** Stable JSON tree node: raw descriptor fields stay top-level and additive. */
export interface SessionTreeNode extends Omit<SessionDescriptor, "planId"> {
	readonly planId: string | null;
	readonly effectiveParentId: SessionId | null;
	readonly activity: TreeActivity;
	readonly liveness: LivenessVerdict;
	/** ADOPTION axis (plan 054 P3, WS-1 — carp's split): a non-prime with NO
	 *  effective parent has nobody to escalate to. Present-when-true, like
	 *  `problem`. Distinct from `TreeProblem` (structural: an orphan HAS a
	 *  parent pointer whose target vanished) and from the runtime axis
	 *  (`systemState`) — three independently assertable axes. */
	readonly unadopted?: true;
	readonly problem?: TreeProblem;
	readonly cycleTo?: SessionId;
	readonly children: readonly SessionTreeNode[];
}

export interface SessionForest {
	readonly roots: readonly SessionTreeNode[];
}

// ─── peer file-watch subscriptions ─────────────────────────────────────────
/** Notice richness for a watch: `notify` = changed-line ranges (default);
 *  `diff` = a unified diff delivered through a pointer file. */
export type WatchMode = "notify" | "diff";

/** One CLI-authored watch subscription in ~/.pij/<id>/watches.json. */
export interface WatchSubscription {
	readonly dir: string;
	readonly patterns: readonly string[];
	readonly recursive?: boolean;
	readonly addedAt: string;
	/** Notice mode; absent ⇒ `notify` (back-compat with pre-034 sidecars). */
	readonly mode?: WatchMode;
	/** Per-subscription collate window in milliseconds; absent ⇒ pij's peer-watch default. */
	readonly debounceMs?: number;
}

/** Sidecar shape owned by the CLI and read-only to the daemon. */
export interface WatchSidecar {
	readonly watches: readonly WatchSubscription[];
}

// ─── peer watchdog sidecar ─────────────────────────────────────────────────
/** Why watchdog firing is suspended. `self` requires an explicit resume;
 *  `compact` resumes when work is next observed; `exempt` is never derived. */
export type WatchdogPauseTier = "self" | "compact" | "exempt";

/** Watcher-specific bounded pane-capture policy. */
export interface WatchdogCapturePolicy {
	readonly mode?: "anomaly" | "always" | "never";
	readonly maxLines?: number;
	readonly maxBytes?: number;
}

/** One peer subscribed to watchdog status for a target session. */
export interface WatchdogWatcher {
	readonly watcherId: SessionId;
	readonly addedAt: string;
	readonly capture?: WatchdogCapturePolicy;
}

/** CLI-owned watchdog configuration and pause state, read-only to the daemon.
 *  Absence of the sidecar is meaningful: watchdog monitoring defaults on. */
export interface WatchdogSidecar {
	readonly enabled?: boolean;
	readonly intervalMs?: number;
	readonly pausedBy?: WatchdogPauseTier;
	readonly pausedAtMs?: number;
	/** Epoch milliseconds at which an `exempt` pause re-arms. Absent legacy
	 * exemptions derive a bounded deadline from pausedAtMs. */
	readonly exemptUntilMs?: number;
	readonly watchers?: readonly WatchdogWatcher[];
}

// ─── event stream ─────────────────────────────────────────────────────────
/** Event categories captured into events.ndjson (open-ended string at the
 *  boundary; these are the known-at-minimum kinds — spec AC-7). */
export type EventType = "tool_call" | "tool_result" | "message" | "usage" | "state" | "receipt";

/** One line of events.ndjson. Every line carries a strictly monotonic `seq`
 *  AND an ISO-8601 `timestamp` so a reader computes age from the stream
 *  alone (spec AC-7 / AC-7a). */
export interface PijEvent {
	readonly seq: number;
	readonly timestamp: string;
	readonly type: string;
	readonly data?: unknown;
}

/** Filters for incremental follow (spec AC-8). */
export interface EventQuery {
	/** Only events with seq > since. */
	readonly since?: number;
	/** Only events of this type. */
	readonly type?: string;
	/** Last N events (present-minus-N). */
	readonly last?: number;
}

// ─── messaging ─────────────────────────────────────────────────────────────
/** A message as it travels to a peer's delivery channel. */
export interface PijMessage {
	readonly from: SessionId;
	/** The paired machine a forwarded message came from (daemon `from_machine`);
	 *  absent for a local message. Render the sender with `senderLabel`. */
	readonly fromMachine?: string;
	readonly to: SessionId;
	readonly body: string;
	/** Present when this is a remote command rather than free text. */
	readonly command?: string;
	/** "receipt" marks an extension-issued delivery receipt: the receiver
	 *  records it as an event but never injects it, so a receipt can never
	 *  wake (or bill) the peer it acknowledges (finding 08 / Phase-3 fix). */
	readonly kind?: "receipt";
	/** Reference-passing media (Plan 026 Phase 5): each entry is a LOCAL file path
	 *  plus an optional caption — never bytes on the wire. Only the telegram bridge
	 *  acts on these (uploads each via grammY); every other peer ignores the field, so
	 *  it is fully additive and the text `body` stays the contract. Present only when
	 *  `pij send … --file` attached a file; a plain text send has no `attachments` key. */
	readonly attachments?: ReadonlyArray<{ readonly path: string; readonly caption?: string }>;
}

/** A message envelope persisted in a peer inbox. The delivery payload remains
 * immutable; durable read state is recorded in a separate marker. */
export interface DeliveredMessage extends PijMessage {
	readonly messageId: string;
}

/** Optional metadata written into `read-<messageId>.json`. Marker existence,
 * not metadata contents, is authoritative for read state. */
export interface InboxReadMarker {
	readonly messageId: string;
	readonly readAt?: string;
	readonly reader?: SessionId;
}

/** Result of an exclusive attempt to claim one unread message. */
export type InboxClaim =
	| { readonly kind: "claimed"; readonly message: DeliveredMessage }
	| { readonly kind: "already-read"; readonly messageId: string };

/** Idempotent result of marking one message read. */
export type InboxMark =
	| { readonly kind: "marked"; readonly marker: InboxReadMarker }
	| { readonly kind: "already-read"; readonly messageId: string };

// ─── delivery receipts (finding 08; spec AC-13) ───────────────────────────
export type ReceiptState = "queued" | "delivered" | "unverified";

export interface MessageReceipt {
	readonly messageId: string;
	readonly from: SessionId;
	readonly to: SessionId;
	readonly state: ReceiptState;
	/** ISO-8601 — set when a busy peer steered the message. */
	readonly queuedAt?: string;
	/** ISO-8601 — set when the peer consumed it (next turn_start). */
	readonly deliveredAt?: string;
	/** ISO-8601 — set when daemon-owned tmux injection could not positively confirm. */
	readonly unverifiedAt?: string;
	/** Daemon heartbeat observed when this receipt was classified. Additive so
	 *  legacy receipt consumers can continue reading `state` alone. */
	readonly daemonLastTickAt?: string | null;
	readonly daemonTickAgeMs?: number | null;
	readonly daemonTickStale?: boolean;
}

// ─── error codes (workshop 001 CLI surface) ───────────────────────────────
export type PijErrorCode =
	| "E-NOID" // no such session id in registry
	| "E-SELF" // refused: target is self
	| "E-CMD" // unknown remote command (not on allow-list)
	| "E-DEAD" // target session is dead
	| "E-NOREG" // no registry present
	| "E-ARG" // bad CLI arguments
	| "E-AMBIG" // cannot resolve "self" (env unset + multiple local)
	| "E-AMBIGUOUS" // model id resolves to multiple providers; caller must qualify it
	| "E-NOTMUX" // not inside a tmux session (required for pij_spawn)
	| "E-FULL" // explicit --layout right|below full — 2 workers already split (the default stack is uncapped)
	| "E-BRANCH" // branch-from-self refused (unsupported harness / unresolved / mismatch / unbound)
	| "E-EMPTY" // refused: the target would receive no content it can render (plan 093)
	| "E-OWN"; // close refused: caller does not own the target session (re-run with --force)

/** Tagged-union result used across the core (Pattern P4: no throws). */
export type Result<T> =
	| { readonly ok: true; readonly value: T }
	| { readonly ok: false; readonly code: PijErrorCode; readonly message: string };

export function ok<T>(value: T): Result<T> {
	return { ok: true, value };
}

export function err<T>(code: PijErrorCode, message: string): Result<T> {
	return { ok: false, code, message };
}
