// pij-messaging — port interfaces (hexagonal boundary, Pattern P3 DI).
//
// Pi-free. These are the seams the pure core depends on; adapters/ (fakes for
// tests, real fs/pi adapters in Phase 2/3) implement them. Only
// adapters/pi-runtime.ts (Phase 2) may import @earendil-works/* — never here.

import type { ArchiveIndexEntry } from "./archive.js";
import type { BgJobRecord } from "./bg.js";
import type { DescriptorWriter } from "./registry-write.js";
import type {
	DeliveredMessage,
	EventQuery,
	InboxClaim,
	InboxMark,
	InboxReadMarker,
	PijEvent,
	PijMessage,
	Result,
	SessionDescriptor,
	SessionId,
	SpawnExpectation,
} from "./types.js";

/** Outcome of a daemon-owned tmux text injection.
 *
 *  - `confirmed`  — the payload was typed AND submission was positively observed.
 *  - `unverified` — the payload WAS typed, but submission could not be confirmed.
 *    Replaying could duplicate an already-accepted turn, so the caller consumes.
 *  - `held`       — the pane's composer had live human input, so NOTHING was
 *    typed. Not a delivery failure: retry on a later tick.
 *  - `failed`     — the send threw before submission, so nothing reliably landed
 *    (tmux unavailable, transient error). Distinguished from `unverified` in plan
 *    071 D7: collapsing the two let the caller consume the ONLY durable copy of
 *    a message that was never typed. The caller must retry, never consume.
 *  - `gone`       — tmux reports the target pane DOES NOT EXIST. Split out of
 *    `failed` because it is PERMANENT, not transient: retrying can never succeed,
 *    so requeueing spins forever (a host reboot produced ~200 such messages, one
 *    tmux call each, every tick). Worse than the noise: tmux re-issues pane ids
 *    from `%0` in every new server, so a queued message for a dead `%315` becomes
 *    deliverable again — into whatever LIVE pane inherits that id. The caller must
 *    stop targeting the binding; the durable copy stays unconsumed in the mailbox. */
export type SendOutcome = "confirmed" | "unverified" | "held" | "failed" | "gone";

/** Outcome of one archival attempt (plan 071 D1). `skipped` means the move was
 *  declined for safety (a conflicting half-archive) — never silent data loss. */
export type ArchiveOutcome = "archived" | "already-archived" | "skipped";

/** Reads/writes the ~/.pij/ peer registry (one descriptor per session).
 *
 *  Two-tier since plan 071 D1: `list()` scans ONLY the hot tier (live seats plus
 *  terminal ones inside the 48h window) so tick cost is O(live); keyed lookup
 *  (`read`) falls through to `~/.pij/archive/` transparently, by direct path. */
export interface RegistryPort {
	/** All known session descriptors in the HOT tier. Never reads the archive —
	 *  that is the whole point of the tier split. */
	list(): SessionDescriptor[];
	/** One descriptor by id, or null if absent. Falls back to the archive by
	 *  direct path (filename = id, no glob) so an archived seat stays addressable. */
	read(id: SessionId): SessionDescriptor | null;
	/** Upsert this session's descriptor, APPLYING THE WRITE LAW
	 *  (`core/registry-write.ts`): contested fields you do not own are taken from
	 *  the latest on-disk record, so a stale snapshot can never replay over a field
	 *  a concurrent writer just stamped. Five lost-updates say this must be the
	 *  default rather than something the writer has to know about.
	 *
	 *  `writer` declares which side you are, so the fields you OWN keep the values
	 *  you just computed. Omitting it is safe — you simply own nothing. */
	write(descriptor: SessionDescriptor, writer?: DescriptorWriter): void;
	/** Exact last-write-wins, NO merge. The only correct use is deliberately
	 *  CLEARING a contested field (e.g. stripping a prior incarnation's `terminal`
	 *  when a seat is re-adopted). Every call site must say why in a comment; the
	 *  write-law test enumerates them so a new one has to justify itself. */
	writeExact(descriptor: SessionDescriptor): void;
	/** Replace a dissolved tombstone with a new runtime incarnation. */
	revive(descriptor: SessionDescriptor): Result<void>;
	/** Remove a session's descriptor (on shutdown). */
	remove(id: SessionId): void;
	/** Persist a terminal tombstone so stale queued writes cannot resurrect a
	 *  session after close. Dissolved descriptors are hidden from list(). */
	dissolve(id: SessionId): void;
	/** Move one terminal record (descriptor + session dir) to the archive and
	 *  append its index line. DAEMON-ONLY: the daemon is the single writer for
	 *  archival moves; the CLI may only read the archive. Idempotent, and safe to
	 *  re-run over a crash-interrupted half-move.
	 *
	 *  Optional so in-memory fakes and non-archiving callers stay source-compatible. */
	archive?(id: SessionId, nowMs: number): ArchiveOutcome;
	/** Archive every hot record the 48h terminal policy calls archivable, and
	 *  report the tally. DAEMON-ONLY. Scans the hot tier directly — including
	 *  `dissolved` records, which `list()` hides and which are the bulk of what
	 *  needs clearing. */
	sweepArchivable?(nowMs: number): { readonly archived: number; readonly skipped: number };
	/** Archived records, newest-archived first, read from the append-only index.
	 *  Backs `pij list --archived`. */
	listArchived?(): readonly ArchiveIndexEntry[];
	/** Pull an ARCHIVED record back into the hot tier (descriptor + session dir).
	 *  Returns the restored descriptor, or null when the id is not archived.
	 *
	 *  Deliberately NOT called `revive`: `revive(descriptor)` above is s066's verb
	 *  for relaunching a dissolved session's process. This one only moves storage
	 *  tiers and starts nothing. The two compose — `pij revive` on a long-dead seat
	 *  must unarchive first — but conflating them would be a genuine bug. */
	unarchive?(id: SessionId): SessionDescriptor | null;
}

/** Resolves the canonical git common directory for a checkout/worktree path. */
/** Durable pre-launch expectations, keyed by the spawn correlation token.
 * The store is intentionally independent from registry descriptors: a child can
 * disappear before it ever self-registers. */
export interface SpawnExpectationStore {
	list(): SpawnExpectation[];
	read(spawnId: string): SpawnExpectation | null;
	write(expectation: SpawnExpectation): void;
	remove(spawnId: string): void;
}

export interface RepositoryIdentityPort {
	gitCommonDir(folder: string): string | null;
}

/** Appends/reads a single session's events.ndjson. */
export interface EventLogPort {
	/** Append one event line (persist before mutate — Pattern P9). */
	append(event: PijEvent): void;
	/** Atomically publish one event per idempotence key. Optional so existing
	 * structural consumers remain source-compatible until they opt in. */
	appendOnce?(key: string, event: PijEvent): "appended" | "existing";
	/** Read events, optionally filtered (since/type/last). */
	read(query?: EventQuery): PijEvent[];
	/** Highest seq written so far, or 0 if empty. Drives crash-safe seq
	 *  recovery after /reload (finding 04). */
	lastSeq(): number;
	/** Total number of events. */
	count(): number;
}

/** Writes a framed message to a target peer's delivery channel. */
export interface DeliveryPort {
	/** Deliver a message to the peer named by message.to. */
	deliver(message: PijMessage): Result<{ messageId: string }>;
}

/** Reads and durably claims immutable inbox envelopes. */
export interface InboxPort {
	/** List currently unread messages in lexical message-id order. */
	listUnread(id: SessionId): Result<readonly DeliveredMessage[]>;
	/** Exclusively claim a message by atomically publishing its read marker. */
	claimUnread(id: SessionId, messageId: string, marker?: InboxReadMarker): Result<InboxClaim>;
	/** Idempotently mark a known message read without returning its envelope. */
	markRead(id: SessionId, messageId: string, marker?: InboxReadMarker): Result<InboxMark>;
}

/** The ONLY port whose real adapter imports pi (adapters/pi-runtime.ts,
 *  Phase 3). The core depends on this interface, never the pi SDK. */
export interface PiRuntimePort {
	/** Is the session currently idle (not streaming a turn)? */
	isIdle(): boolean;
	/** messageId correlates native consumption; resendAttempt forces the prompt recovery path. */
	inject(
		text: string,
		mode: "immediate" | "steer",
		messageId?: string,
		resendAttempt?: number,
	): void;
	/** Trigger a context compaction (remote command: compact). Always available
	 *  — compact() lives on the long-lived ExtensionContext. */
	compact(): void | Promise<void>;
	/** Returns false synchronously without an armed context; otherwise completes
	 *  after the runtime operation (rejections are execution refusals). */
	control(command: "new" | "reload"): boolean | Promise<boolean>;
}

// ─── Tmux seam ──────────────────────────────────────────────────────────────

/** Options for opening a new tmux window running a pij worker. */
export interface NewWindowOpts {
	/** Command to run (always "pi"). */
	cmd: string;
	/** Argv to pass to the command. */
	args: string[];
	/** Environment variables for the new window. */
	env: Record<string, string>;
	/** Tmux window name, e.g. "pi:<spawnId>". */
	name: string;
	/** Human-readable pane title (`select-pane -T`); naming failure aborts spawn. */
	title: string;
	/** Working directory for the new window. */
	cwd?: string;
	/** Pass tmux `-d`: create the window in the BACKGROUND without switching the
	 *  client's focus to it (e.g. the auto-started daemon window — it must not
	 *  steal the operator's view). */
	detached?: boolean;
}

/** Options for splitting an existing pane (layout:"split" mode). */
export interface SplitWindowOpts {
	/** Command to run (always "pi"). */
	cmd: string;
	/** Argv to pass to the command. */
	args: string[];
	/** Environment variables for the new pane. */
	env: Record<string, string>;
	/** Human-readable pane title (`select-pane -T`); naming failure aborts spawn. */
	title: string;
	/** Working directory for the new pane. */
	cwd?: string;
	/** Pane to split (%N): orchestrator pane for the right column; worker-1's
	 *  pane to stack the second worker below it. */
	target: string;
	/** "h" = LEFT/RIGHT (side-by-side column); "v" = UP/DOWN (stacked).
	 *  NB: bare `split-window` defaults to -v, so -h must be explicit. */
	direction: "h" | "v";
	/** Size % for the NEW pane (e.g. 33 → right column ~1/3 width). */
	percent?: number;
	/** Pass tmux -d: keep focus on the current pane (don't follow into the new one). */
	detached?: boolean;
	/** After the split, even out the new pane's stack (`select-layout -E -t <new>`)
	 *  — evens the heights of that vertical run. Best-effort cosmetic step;
	 *  failures are swallowed. */
	evenOut?: boolean;
	/** After evening, pin the stack column back to this % of the window width
	 *  (`resize-pane -x N%` on the new pane) — `-E` can re-spread the root h-split
	 *  too (seen live on tmux 3.6a). Best-effort; failures are swallowed. */
	columnPercent?: number;
}

/** Seam for creating/destroying tmux windows + panes (Pattern P2: impurity
 *  confined to adapters/tmux.ts; this interface is pi-free). */
export interface TmuxPort {
	/** Create a new tmux window running the given command. Returns the
	 *  captured %N pane id plus the @M window id holding it (plan 054 P2
	 *  T006, AC-09 terminal addressability). `windowId` is optional: a
	 *  malformed capture degrades to paneId-only — the daemon backfill
	 *  retries it, and addressability is never worth failing a spawn. */
	newWindow(opts: NewWindowOpts): Result<{ paneId: string; windowId?: string }>;
	/** Split an existing pane (layout:"split"); returns the new %N pane id
	 *  (+ @M window id, same contract as newWindow). */
	splitWindow(opts: SplitWindowOpts): Result<{ paneId: string; windowId?: string }>;
	/** Kill a window by its pane id. Swallows "already gone" (idempotent). */
	killWindow(paneId: string): Result<void>;
	/** Kill a single pane by its id (split-safe: siblings survive; a window's
	 *  last pane dying closes the window). Swallows "already gone". */
	killPane(paneId: string): Result<void>;
	/** Returns the current tmux session name if inside tmux, else null. */
	currentSession(): string | null;
	/** The orchestrator's own pane id ($TMUX_PANE), or null if not in tmux. */
	currentPane(): string | null;
	/** Pane ids in the orchestrator's current window (for the split cap count). */
	currentWindowPanes(): string[];
}

/** OS-level seams: pid, liveness probe, clock, env. */
export interface ProcessPort {
	/** This process's pid. */
	pid(): number;
	/** Is a pid currently alive? */
	isAlive(pid: number): boolean;
	/** Current time (ms epoch) — injected for deterministic tests. */
	now(): number;
	/** Read an environment variable (e.g. PIJ_SESSION_ID). */
	env(key: string): string | undefined;
}

/** Launches a detached background job. The child outlives this process — the
 *  whole point of `pij bg` is that the caller's turn ends immediately. */
export interface BackgroundLauncherPort {
	/** Start `script` under `sh`, detached, with `env` overlaid on the ambient
	 *  environment. Returns the child's pid. */
	launch(input: {
		readonly script: string;
		readonly env: Readonly<Record<string, string>>;
		readonly cwd: string;
	}): Result<{ readonly pid: number }>;
}

/** Durable per-job records for `pij bg`. Without these a queued job leaves only
 *  a log file, so `list` has nothing to render and `kill` nothing to target. */
export interface BgJobStorePort {
	write(record: BgJobRecord): void;
	read(jobId: string): BgJobRecord | undefined;
	/** Newest first. */
	list(): readonly BgJobRecord[];
}
