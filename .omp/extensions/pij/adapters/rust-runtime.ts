import { randomUUID } from "node:crypto";
import { parseDestination, renderDestination } from "../core/address.js";
import { type ControlOutcome, isControlCommand, validateCommand } from "../core/commands.js";
import {
	COLD_WAKE_CODE,
	type FyiClaimRequest,
	isHeldFyi,
	type Registration,
	type RustSeatDescriptor,
} from "../core/daemon-wire.js";
import { memorableIdentitySeed } from "../core/discovery.js";
import { buildCallerContext } from "../core/generation-routing.js";
import { memorablePijIdCandidates } from "../core/memorable-id.js";
import { frame, parseFrame, senderLabel } from "../core/message.js";
import type { ModelEntry } from "../core/models/registry.js";
import type {
	DeliveryPort,
	EventLogPort,
	PiRuntimePort,
	RegistryPort,
	TmuxPort,
} from "../core/ports.js";
import {
	type BootInput,
	PijSession,
	type SessionShutdownReason,
	type SpawnOpts,
} from "../core/session.js";
import {
	type EventQuery,
	err,
	ok,
	type PijEvent,
	type PijMessage,
	type Result,
	type SessionDescriptor,
	type SessionId,
} from "../core/types.js";
import type { PijDaemonClient } from "./daemon-http.js";
import type { ExtensionBuildIdentity } from "./extension-build.js";

interface RustBootInput extends Omit<BootInput, "id">, ExtensionBuildIdentity {
	readonly runtimeBin: "pi" | "omp";
	readonly piSessionId?: string;
	readonly actualModel?: string | null;
	readonly reason: SessionShutdownReason | "startup";
	readonly notice?: (text: string) => void;
}

/** How a send may deviate from a plain waking message. */
export interface RustSendOptions {
	readonly command?: string;
	/** Hold until the recipient's next turn (plan 158). */
	readonly fyi?: boolean;
	/** Wake a cold recipient anyway (plan 157 phase 2); the reason is audited. */
	readonly force?: { readonly reason: string };
}

interface VisibleRuntimePort extends PiRuntimePort {
	setStatus?(text: string | undefined): void;
	/** The `✉N` footer slot, separate from the pij-mail pending count. */
	setFyiStatus?(text: string | undefined): void;
	notify?(text: string, level?: "info" | "warning" | "error"): void;
}

interface InboxClaim {
	readonly jobId: number;
	readonly attempt: number;
	readonly message: PijMessage & { readonly messageId: string; readonly urgent?: boolean };
}

interface PendingConsumption {
	claim: InboxClaim;
	readonly seat: string;
	readonly client: PijDaemonClient;
	readonly lifecycle: number;
	consumed: boolean;
	acknowledging: boolean;
	heartbeating: boolean;
	lastHeartbeatAt: number;
	heartbeatUncertain?: boolean;
	resends: number;
	injectedAt: number;
	lastResentAt?: number;
	boundaryDueAt?: number;
}

const MAIL_STATUS_TICK_MS = 1_000;
const CLAIM_HEARTBEAT_MS = 20_000;
/** Healthy OMP drain p95=139ms (plan 143); max(2*p95, 2s) leaves a conservative floor. */
const BOUNDARY_RESEND_GRACE_MS = 2_000;
const DEFAULT_REDELIVER_IDLE_MS = 10_000;
const DEFAULT_COMPACTION_LATCH_MAX_MS = 120_000;
const MAX_RESENDS = 3;
const PARKED_OUTCOMES = new Set([
	"undelivered:lease-exhausted",
	"undelivered:harness-swallowed",
	"undelivered:operator-released",
	"undelivered:native-receiver-unavailable",
]);

/** How much of an arriving body the announcement shows. */
const ANNOUNCE_BODY_CHARS = 80;

/** A hung daemon must hold neither the human's typed turn (FYI claim) nor shutdown. */
const HOOK_CALL_TIMEOUT_MS = 3_000;

/**
 * Snap-in recipe (composition owner: PM, index.ts unchanged):
 *
 * In the ONE session_start handler, for startup/reload/new/resume/fork:
 *   rustRuntime ??= new RustRuntimeSession(generation.client, new TmuxAdapter(), loadModels());
 *   rustRuntime.setClient(generation.client);
 *   rustRuntime.setPi(new PiRuntimeAdapter(pi, ctx, runtimeBin, () => commandControl));
 *   const boot = await rustRuntime.boot({ ...existingBootInput, reason: event.reason });
 *   session = rustRuntime;
 *
 * boot arms a 1s mail-status ticker; it never reads the composer or delays delivery.
 * session_shutdown cancels the ticker and event stream; setPi binds the fresh
 * context before boot can consume pushes.
 *
 * The daemon owns durable claim order and per-job deferrals. Startup un-defers the
 * backlog oldest-first, retaining that phase across unavailable claims until the inbox empties.
 * Steady-state retries only claim/peek; normal arrivals never declare releases.
 */
export class RustRuntimeSession {
	private readonly registry = new MemoryRegistry();
	private readonly eventLog = new MemoryEventLog();
	private readonly delivery = new BufferedDelivery();
	private readonly pi: MutablePiRuntime;
	private readonly session: PijSession;
	private client: PijDaemonClient;
	private self = "";
	private stopEvents: (() => void) | undefined;
	private flushChain: Promise<void> = Promise.resolve();
	private pushedChain: Promise<void> = Promise.resolve();
	private readonly lastInbound = new Map<string, string>();
	/** Retained until native consumption AND a successful daemon acknowledgement. */
	private readonly pendingConsumption = new Map<string, PendingConsumption>();
	/** Claims are serialized, so only the latest ACK can overtake a claim response. */
	private lastAcknowledgedJobId: number | undefined;
	private idleSince: number | undefined;
	/** Our outbound messages already reported parked; their ids are ours, so local. */
	private readonly parkedNotifications = new Set<string>();
	/** Inbound jobs already reported parked. Keyed by job, the daemon's delivery identity:
	 *  a parked event names only the raw msg_id, which a forwarded message can share. */
	private readonly parkedJobs = new Set<number>();
	private readonly redeliverIdleMs: number;
	private readonly compactionLatchMaxMs: number;
	private readonly boundaryGraceMs: number;
	private mailTicker: ReturnType<typeof setInterval> | undefined;
	private lifecycle = 0;
	/** A pushed or startup-discovered backlog, never composer activity. */
	private inboxPending = false;
	private inboxPollQueued = false;
	private startupRecovery = false;
	private releasedStartupHead: number | undefined;
	private compacting = false;
	private compactionExpiresAt = 0;
	private nativeSession: string | undefined;
	private paneId: string | undefined;
	/** Only the newest count request may paint the footer. */
	private fyiCountTicket = 0;
	/** Activity publications land in call order: a late `working` never follows `idle`. */
	private activityChain: Promise<void> = Promise.resolve();
	/** Set when the daemon has no /v1/activity route; cleared by a new client. */
	private activityUnsupported = false;
	/** The last state the daemon accepted; shutdown repairs a seat left `working`. */
	private activityPublished: "working" | "idle" | undefined;

	constructor(
		client: PijDaemonClient,
		tmux: TmuxPort,
		models: readonly ModelEntry[],
		private readonly clock: () => number = () => Date.now(),
	) {
		this.client = client;
		const configuredIdleMs = Number(process.env.PIJ_REDELIVER_IDLE_MS);
		this.redeliverIdleMs =
			Number.isSafeInteger(configuredIdleMs) && configuredIdleMs > 0
				? configuredIdleMs
				: DEFAULT_REDELIVER_IDLE_MS;
		const configuredLatchMs = Number(process.env.PIJ_COMPACTION_LATCH_MAX_MS);
		this.compactionLatchMaxMs =
			Number.isSafeInteger(configuredLatchMs) && configuredLatchMs > 0
				? configuredLatchMs
				: DEFAULT_COMPACTION_LATCH_MAX_MS;
		const configuredBoundaryMs = Number(process.env.PIJ_BOUNDARY_GRACE_MS);
		this.boundaryGraceMs =
			Number.isSafeInteger(configuredBoundaryMs) && configuredBoundaryMs > 0
				? configuredBoundaryMs
				: BOUNDARY_RESEND_GRACE_MS;
		this.pi = new MutablePiRuntime();
		this.session = new PijSession({
			registry: this.registry,
			eventLog: this.eventLog,
			delivery: this.delivery,
			pi: this.pi,
			process: {
				pid: () => process.pid,
				isAlive: (pid) => {
					try {
						process.kill(pid, 0);
						return true;
					} catch {
						return false;
					}
				},
				now: clock,
				env: (key) => process.env[key],
			},
			tmux,
			models,
		});
	}

	setClient(client: PijDaemonClient): void {
		if (client !== this.client) this.activityUnsupported = false;
		this.client = client;
	}

	setPi(pi: VisibleRuntimePort): void {
		this.pi.set(pi);
	}

	async boot(input: RustBootInput): Promise<{ readonly id: string; readonly role?: string }> {
		const lifecycle = ++this.lifecycle;
		clearInterval(this.mailTicker);
		this.mailTicker = undefined;
		this.stopEvents?.();
		const previousSelf = this.self || undefined;
		const replacing =
			previousSelf !== undefined && (input.reason === "new" || input.reason === "fork");
		const procStart = this.client.processStart(process.pid);
		const seats = await this.client.seats();
		const id = this.chooseIdentity(input, procStart, seats, replacing);
		const registration = registrationFor(
			id,
			procStart,
			input,
			replacing ? previousSelf : undefined,
		);
		const accepted = await this.client.register(registration);
		if (accepted.proc_source === "pane") {
			input.notice?.(
				`pij: harness discovery fell back to the pane; registered pid ${accepted.proc?.pid ?? "unbound"}.`,
			);
		}
		const acceptedId = accepted.id;
		// A replacement runtime has no claim on its predecessor's volatile queue.
		this.pendingConsumption.clear();
		this.lastAcknowledgedJobId = undefined;
		this.idleSince = this.pi.isIdle() ? this.clock() : undefined;
		this.parkedNotifications.clear();
		this.parkedJobs.clear();
		this.inboxPending = false;
		this.inboxPollQueued = false;
		this.compacting = false;
		this.startupRecovery = true;
		this.releasedStartupHead = undefined;
		this.pi.setStatus(undefined);
		this.pi.setFyiStatus(undefined);
		this.nativeSession = input.piSessionId;
		this.paneId = input.paneId;
		const continuityRegistration = registrationFor(acceptedId, procStart, input);
		for (const seat of seats) this.registry.seed(fromRustDescriptor(seat));
		if (replacing && previousSelf) this.registry.dissolve(previousSelf);
		/** Snap-in: keep roster seeding before boot; the daemon owns freshness.
		 * Missing binding passes undefined so PijSession retains
		 * `input.fresh ?? (liveDescriptor === null || wasDissolved)`.
		 * Both created and rebound announce; same never does.
		 */
		const boot = this.session.boot({
			...input,
			id: acceptedId,
			fresh: accepted.binding === undefined ? undefined : accepted.binding !== "same",
		});
		this.registry.seed(fromRustDescriptor(accepted));
		this.self = acceptedId;
		process.env.PIJ_SESSION_ID = acceptedId;
		await this.flush();
		await this.recoverInbox(acceptedId, this.client, lifecycle);
		this.stopEvents?.();
		this.stopEvents = await this.client.watchEvents(
			(frame) => {
				if (this.lifecycle !== lifecycle) return;
				const seat = this.self;
				const client = this.client;
				// FYIs never ride the pushed path; recipient-scoped fyi.* and tombstone
				// events only repaint the pending count.
				if (
					frame.event.seat === seat &&
					(frame.event.kind.startsWith("fyi.") || frame.event.kind === "seat.tombstone")
				)
					this.refreshFyiCount(seat, client, lifecycle);
				if (frame.event.kind === "delivery.parked") {
					const payload = JSON.parse(frame.event.payload) as Record<string, unknown>;
					if (
						typeof payload.recipient === "string" &&
						typeof payload.messageId === "string" &&
						typeof payload.outcome === "string" &&
						PARKED_OUTCOMES.has(payload.outcome)
					) {
						if (payload.recipient === seat) {
							if (typeof payload.jobId === "number")
								this.noteParked(payload.jobId, payload.messageId, payload.outcome);
						} else if (
							frame.event.seat === seat &&
							!this.parkedNotifications.has(payload.messageId)
						) {
							this.parkedNotifications.add(payload.messageId);
							this.pi.notify(
								`pij message ${payload.messageId} to ${payload.recipient}: ${payload.outcome}`,
								"warning",
							);
						}
					}
					this.session.capture("daemon_event", frame);
					return;
				}
				if (frame.event.kind !== "message.pushed" || frame.event.seat !== seat) {
					this.session.capture("daemon_event", frame);
					return;
				}
				if (this.compacting) {
					this.inboxPending = true;
					return;
				}
				const consumed = this.pushedChain.then(() =>
					this.consumePushed(frame.event.payload, seat, client, lifecycle),
				);
				this.pushedChain = consumed.catch(() => undefined);
				return consumed;
			},
			(error) => this.session.capture("daemon_event_error", { message: error.message }),
			(notice) => {
				input.notice?.(notice);
				if (!notice.startsWith("pij: re-attached")) return;
				const seat = this.self;
				const client = this.client;
				this.pushedChain = this.pushedChain
					.then(() => this.ensureRegistration(seat, client, continuityRegistration))
					.catch((error: unknown) => {
						this.session.capture("daemon_event_error", {
							message: error instanceof Error ? error.message : String(error),
							stage: "reattach-registration",
						});
					});
			},
		);
		this.mailTicker = setInterval(() => {
			if (this.compacting && this.clock() >= this.compactionExpiresAt) {
				this.repollAfterCompaction("ceiling");
			}
			this.recoverUnconsumed();
			for (const pending of this.pendingConsumption.values()) {
				if (pending.consumed) void this.ackConsumed(pending);
				else void this.heartbeatPending(pending);
			}
			if (this.compacting || this.client.isReconnecting()) return;
			// Lease expiry is claim-driven; an unconsumed head needs polling even
			// without another message.pushed event.
			if ((!this.inboxPending && this.pendingConsumption.size === 0) || this.inboxPollQueued)
				return;
			this.inboxPollQueued = true;
			const seat = this.self;
			const client = this.client;
			this.pushedChain = this.pushedChain
				.then(() => this.recoverInbox(seat, client, lifecycle))
				.catch((error: unknown) => {
					this.session.capture("daemon_event_error", {
						message: error instanceof Error ? error.message : String(error),
						stage: "inbox-recovery",
					});
				})
				.finally(() => {
					if (this.lifecycle === lifecycle) this.inboxPollQueued = false;
				});
		}, MAIL_STATUS_TICK_MS);
		this.mailTicker.unref?.();
		this.refreshFyiCount(acceptedId, this.client, lifecycle);
		return { id: boot.id, role: boot.role };
	}

	async send(
		to: string,
		body: string,
		options: RustSendOptions = {},
	): Promise<{
		readonly msgId: string;
		readonly held: boolean;
		readonly coldCheck?: string;
		readonly warning?: string;
	}> {
		const { command, fyi = false, force } = options;
		const destination = parseDestination(to);
		if (!destination.ok) throw new Error(`${destination.code}: ${destination.message}`);
		if (fyi && command !== undefined) {
			throw new Error("fyi cannot be combined with a control command");
		}
		if (force !== undefined) {
			// The daemon refuses these too; refusing here sends nothing at all.
			if (fyi || command !== undefined)
				throw new Error(`${COLD_WAKE_CODE}: force wakes a message; never with fyi or a command`);
			if (force.reason.trim() === "")
				throw new Error(
					`${COLD_WAKE_CODE}: force needs a non-empty reason saying why the wake is worth it`,
				);
		}
		if (command !== undefined) {
			const validated = validateCommand(command);
			if (!validated.ok) throw new Error(validated.message);
			if ((parseFrame(body)?.body ?? body).trim() !== "") {
				throw new Error("a control command cannot include a message body");
			}
			body = "";
		}
		const msgId = randomUUID();
		// Keyed like `senderLabel`, so a reply to the shown sender threads onto its message.
		const replyKey = renderDestination(destination.value);
		const inReplyTo = this.lastInbound.get(replyKey);
		const receipt = await this.client.send({
			from: this.self,
			to: destination.value,
			body,
			msg_id: msgId,
			...(command === undefined
				? {}
				: {
						command,
						caller: buildCallerContext(process.env, {
							cwd: process.cwd(),
							pid: process.pid,
							procStart: this.client.processStart(process.pid),
						}),
					}),
			...(fyi ? { fyi: true } : {}),
			...(force === undefined ? {} : { force: true, reason: force.reason }),
			...(inReplyTo === undefined ? {} : { in_reply_to: inReplyTo }),
		});
		if (inReplyTo !== undefined) this.lastInbound.delete(replyKey);
		return {
			msgId,
			held: isHeldFyi(receipt),
			...(receipt.cold_check === undefined ? {} : { coldCheck: receipt.cold_check }),
			...(receipt.warning === undefined ? {} : { warning: receipt.warning }),
		};
	}

	/**
	 * Claim this seat's held FYIs for a typed turn. Returns the daemon's block
	 * verbatim, or undefined when none are pending or the claim fails: a claim
	 * failure is logged and never blocks the human's turn.
	 */
	async claimFyi(via: FyiClaimRequest["via"]): Promise<string | undefined> {
		const seat = this.self;
		const client = this.client;
		const lifecycle = this.lifecycle;
		if (seat === "") return undefined;
		const evidence = this.bindingEvidence("fyi-claim");
		if (evidence === undefined) return undefined;
		try {
			const claim = await client.claimFyi(
				{ seat, ...evidence, via },
				AbortSignal.timeout(HOOK_CALL_TIMEOUT_MS),
			);
			this.refreshFyiCount(seat, client, lifecycle);
			return claim.count > 0 ? claim.block : undefined;
		} catch (error) {
			this.session.capture("daemon_event_error", {
				message: error instanceof Error ? error.message : String(error),
				stage: "fyi-claim",
			});
			return undefined;
		}
	}

	private refreshFyiCount(seat: string, client: PijDaemonClient, lifecycle: number): void {
		const ticket = ++this.fyiCountTicket;
		client.pendingFyis(seat).then(
			(count) => {
				if (this.lifecycle !== lifecycle || this.self !== seat || ticket !== this.fyiCountTicket)
					return;
				this.pi.setFyiStatus(count > 0 ? `✉${count}` : undefined);
			},
			(error: unknown) => {
				this.session.capture("daemon_event_error", {
					message: error instanceof Error ? error.message : String(error),
					stage: "fyi-count",
				});
			},
		);
	}

	/** Binding evidence for seat-scoped calls: the harness session, else the pane. */
	private bindingEvidence(
		stage: string,
	): { readonly native_session: string } | { readonly pane: string } | undefined {
		if (this.nativeSession !== undefined) return { native_session: this.nativeSession };
		if (this.paneId !== undefined) return { pane: this.paneId };
		this.session.capture("daemon_event_error", {
			message: "no native session or pane to bind this seat",
			stage,
		});
		return undefined;
	}

	/**
	 * Queue a turn-state publication; failures are logged and never reach the turn.
	 * A `shutdown` signal marks the shutdown repair: it bounds the request and
	 * publishes only when the daemon last accepted `working`.
	 */
	private publishActivity(state: "working" | "idle", shutdown?: AbortSignal): void {
		const seat = this.self;
		const client = this.client;
		if (seat === "" || this.activityUnsupported) return;
		const evidence = this.bindingEvidence("activity");
		if (evidence === undefined) return;
		this.activityChain = this.activityChain.then(async () => {
			if (this.activityUnsupported) return;
			if (shutdown !== undefined && this.activityPublished !== "working") return;
			try {
				if (await client.publishActivity({ seat, ...evidence, state }, shutdown))
					this.activityPublished = state;
				else this.activityUnsupported = true;
			} catch (error) {
				this.session.capture("daemon_event_error", {
					message: error instanceof Error ? error.message : String(error),
					stage: "activity",
				});
			}
		});
	}

	async spawn(opts: SpawnOpts): Promise<ReturnType<PijSession["spawn"]>> {
		const health = await this.client.health();
		if (!Array.isArray(health.retired_harnesses))
			return err(
				"E-NOREG",
				"daemon does not report retired harness policy; update and restart pij-rs before spawning",
			);
		if (health.retired_harnesses.includes(opts.harness))
			return err(
				"E-ARG",
				`harness ${opts.harness} is retired on this machine; ${opts.harness === "omp" ? "use another harness" : "use omp"} (or pass --allow-retired)`,
			);
		if (opts.harness !== "pi" && opts.harness !== "omp") {
			if (opts.layout === "split")
				return err("E-ARG", `${opts.harness} uses daemon windows; pass layout:'window'`);
			const child = await this.client.spawn({
				harness: opts.harness,
				model: opts.model,
				effort: opts.effort,
				cwd: opts.cwd,
				caller_pane: process.env.TMUX_PANE,
				parent: this.self,
				no_wait: true,
				accept_inbound: opts.harness === "claude",
				...(opts.role === undefined
					? {}
					: {
							role: opts.role,
							caller: {
								PIJ_SESSION_ID: this.self,
								...(process.env.TMUX_PANE ? { TMUX_PANE: process.env.TMUX_PANE } : {}),
							},
						}),
			});
			if (!child.spawn_id || !child.pane)
				return err("E-NOREG", "daemon spawn returned no spawn identity or pane");
			if (opts.task !== undefined) await this.send(child.id, opts.task);
			return ok({ spawnId: child.spawn_id, paneId: child.pane });
		}
		if (opts.role !== undefined)
			return err(
				"E-ARG",
				`role on an ${opts.harness} spawn is not supported yet (pij-fleet#25); after the child's ready-ping run \`pij link <child> --role ${opts.role}\``,
			);
		const result = this.session.spawn(opts);
		void this.flush();
		return result;
	}

	close(id: SessionId): ReturnType<PijSession["close"]> {
		return this.session.close(id);
	}

	capture(type: string, data?: unknown): void {
		this.session.capture(type, data);
	}

	onTurnStart(iso: string): void {
		this.idleSince = undefined;
		this.repollAfterCompaction("turn_start");
		this.session.onTurnStart(iso);
		this.publishActivity("working");
		void this.flush();
	}

	onTurnEnd(): void {
		this.session.onTurnEnd();
		this.publishActivity("idle");
		if (this.pi.isIdle()) this.idleSince ??= this.clock();
		this.queueBoundaryResend();
	}

	onToolResult(): void {
		this.queueBoundaryResend();
	}

	onBeforeCompact(): void {
		this.compacting = true;
		this.compactionExpiresAt = this.clock() + this.compactionLatchMaxMs;
	}

	onCompact(): void {
		this.repollAfterCompaction("session_compact");
	}

	private repollAfterCompaction(
		boundary: "session_compact" | "turn_start" | "agent_end" | "message_start" | "ceiling",
	): void {
		if (!this.compacting && boundary !== "session_compact") return;
		this.compacting = false;
		this.inboxPending = true;
		this.session.capture("delivery.compaction-repoll", { boundary });
	}

	private queueBoundaryResend(): void {
		for (const pending of this.pendingConsumption.values()) {
			if (!pending.consumed && pending.resends === 0) {
				pending.boundaryDueAt ??= this.clock() + this.boundaryGraceMs;
			}
		}
	}

	/** message_start is correlated per envelope; neither turn_start nor idle proves consumption. */
	async onMessageStart(message: {
		role: string;
		customType?: string;
		details?: unknown;
		content?: unknown;
	}): Promise<void> {
		this.repollAfterCompaction("message_start");
		const id = consumedMessageId(message);
		if (id === undefined) return;
		const pending = this.pendingConsumption.get(id);
		if (!pending || pending.lifecycle !== this.lifecycle || pending.seat !== this.self) return;
		if (!pending.consumed) {
			pending.consumed = true;
			// Keyed by the shown sender: a local reply to `w3` never threads onto `w3@laptop`.
			const { from, fromMachine, messageId } = pending.claim.message;
			this.lastInbound.set(senderLabel(from, fromMachine), messageId);
		}
		this.showMailStatus();
		await this.ackConsumed(pending);
	}

	/** Boundaries grant one grace-delayed resend; later retries still require idle. */
	onAgentEnd(willContinue = false): void {
		if (!willContinue && this.pi.isIdle()) this.idleSince ??= this.clock();
		// Idempotent idle repair for a turn_end that never published.
		if (!willContinue) this.publishActivity("idle");
		this.repollAfterCompaction("agent_end");
		this.queueBoundaryResend();
	}

	/**
	 * Tears down synchronously. The returned promise settles once queued activity
	 * has landed, including `idle` for a seat the daemon last saw `working`,
	 * bounded by HOOK_CALL_TIMEOUT_MS. It never rejects.
	 */
	shutdown(reason: SessionShutdownReason): Promise<void> {
		const bound = AbortSignal.timeout(HOOK_CALL_TIMEOUT_MS);
		this.publishActivity("idle", bound);
		// Executor form: the compile target's lib predates Promise.withResolvers.
		const expired = new Promise<void>((resolve) =>
			bound.addEventListener("abort", () => resolve(), { once: true }),
		);
		const settled = Promise.race([this.activityChain, expired]);
		++this.lifecycle;
		clearInterval(this.mailTicker);
		this.mailTicker = undefined;
		this.inboxPending = false;
		this.inboxPollQueued = false;
		this.compacting = false;
		this.startupRecovery = false;
		this.releasedStartupHead = undefined;
		this.idleSince = undefined;
		this.parkedNotifications.clear();
		this.parkedJobs.clear();
		this.pendingConsumption.clear();
		this.lastAcknowledgedJobId = undefined;
		this.pi.setStatus(undefined);
		this.pi.setFyiStatus(undefined);
		this.stopEvents?.();
		this.stopEvents = undefined;
		this.session.shutdown(reason);
		return settled;
	}

	applyPendingControl(): ReturnType<PijSession["applyPendingControl"]> {
		return this.session.applyPendingControl();
	}

	readSelf(): SessionDescriptor | null {
		return this.registry.read(this.self);
	}

	peerCount(): number {
		return this.registry.list().filter((seat) => seat.id !== this.self).length;
	}

	eventCount(type?: string): number {
		return type === undefined ? this.eventLog.count() : this.eventLog.read({ type }).length;
	}

	private async ensureRegistration(
		seat: string,
		client: PijDaemonClient,
		registration: Registration,
	): Promise<void> {
		if (this.self !== seat) return;
		const seats = await client.seats();
		if (this.self !== seat || seats.some((candidate) => candidate.id === seat)) return;
		const accepted = await client.register(registration);
		if (this.self === seat) this.registry.writeExact(fromRustDescriptor(accepted));
	}

	/** Startup migrates deferred rows in order; normal retries never change eligibility. */
	private async recoverInbox(
		seat: string,
		client: PijDaemonClient,
		lifecycle: number,
	): Promise<void> {
		while (this.self === seat && this.lifecycle === lifecycle && !this.compacting) {
			if (client.isReconnecting()) return;
			if (this.startupRecovery) {
				const pending = await client.peekInbox(seat);
				if (this.self !== seat || this.lifecycle !== lifecycle) return;
				const oldest = this.readInbox(pending, seat);
				this.inboxPending = oldest !== undefined;
				if (!oldest) {
					this.startupRecovery = false;
					this.releasedStartupHead = undefined;
					return;
				}
				if (oldest.message.command === undefined && this.releasedStartupHead !== oldest.jobId) {
					await client.release({
						seat,
						job_id: oldest.jobId,
						msg_id: oldest.message.messageId,
						at_ms: this.clock(),
					});
					if (this.self !== seat || this.lifecycle !== lifecycle) return;
					this.releasedStartupHead = oldest.jobId;
				}
			}
			if (this.compacting) return; // Startup peek/release can cross compaction start.
			const response = await client.claimInbox(seat);
			if (this.self !== seat || this.lifecycle !== lifecycle) return;
			const claim = this.readInbox(response, seat);
			if (!claim) {
				if (this.startupRecovery) return; // Preserve oldest-first recovery across unavailable claims.
				const pending = await client.peekInbox(seat);
				if (this.self === seat && this.lifecycle === lifecycle) {
					this.inboxPending = this.readInbox(pending, seat) !== undefined;
				}
				return;
			}
			await this.routeClaim(claim, seat, client, lifecycle);
		}
	}

	private async consumePushed(
		payload: string,
		seat: string,
		client: PijDaemonClient,
		lifecycle: number,
	): Promise<void> {
		const announced = pushedMessage(payload, seat);
		if (this.self !== seat || this.lifecycle !== lifecycle) {
			this.session.capture("daemon_event_claim_stale", {
				currentSeat: this.self,
				messageId: announced.messageId,
				seat,
				stage: "before-claim",
			});
			return;
		}
		if (this.compacting || client.isReconnecting()) {
			this.inboxPending = true;
			return;
		}
		if (this.startupRecovery) {
			await this.recoverInbox(seat, client, lifecycle);
			return;
		}
		const response = await client.claimInbox(seat);
		if (this.self !== seat || this.lifecycle !== lifecycle) {
			this.session.capture("daemon_event_claim_stale", {
				currentSeat: this.self,
				messageId: announced.messageId,
				seat,
				stage: "after-claim",
			});
			return;
		}
		const claim = this.readInbox(response, seat);
		if (claim === undefined) {
			this.inboxPending = true;
			this.session.capture("daemon_event_claim_unavailable", {
				messageId: announced.messageId,
			});
			return;
		}
		if (messageKey(claim.message) !== messageKey(announced)) {
			this.inboxPending = true;
			this.session.capture("daemon_event_claim_mismatch", {
				announcedMessageId: announced.messageId,
				claimedMessageId: claim.message.messageId,
			});
		}
		await this.routeClaim(claim, seat, client, lifecycle);
	}

	private async routeClaim(
		claim: InboxClaim,
		seat: string,
		client: PijDaemonClient,
		lifecycle: number,
	): Promise<void> {
		if (this.self !== seat || this.lifecycle !== lifecycle) return;
		// An ACK or parking can overtake an in-flight claim response.
		if (this.lastAcknowledgedJobId === claim.jobId || this.parkedJobs.has(claim.jobId)) return;
		// Announce before a busy runtime queues the message at its next boundary.
		if (!this.pendingConsumption.has(messageKey(claim.message))) this.announceArrival(claim);
		await this.injectAndAck(claim, seat, client, lifecycle);
	}

	private async injectAndAck(
		claim: InboxClaim,
		seat: string,
		client: PijDaemonClient,
		lifecycle: number,
	): Promise<void> {
		if (this.self !== seat || this.lifecycle !== lifecycle) return;
		if (claim.message.command !== undefined) {
			const outcome = await this.executeControl(claim.message.command);
			this.session.capture("receipt", {
				messageId: claim.message.messageId,
				command: claim.message.command,
				control_outcome: outcome,
			});
			// new/reload may replace this lifecycle. Settle the original claim on
			// its original client/seat, not the replacement's identity.
			await this.ackClaim(claim, seat, client, outcome);
			return;
		}
		const id = messageKey(claim.message);
		const existing = this.pendingConsumption.get(id);
		if (existing) {
			if (claim.attempt < existing.claim.attempt) return;
			if (claim.attempt > existing.claim.attempt) existing.lastHeartbeatAt = this.clock();
			existing.claim = claim;
			if (existing.consumed) await this.ackConsumed(existing);
			return;
		}
		const pending: PendingConsumption = {
			claim,
			seat,
			client,
			lifecycle,
			consumed: false,
			acknowledging: false,
			heartbeating: false,
			lastHeartbeatAt: this.clock(),
			resends: 0,
			injectedAt: this.clock(),
		};
		this.pendingConsumption.set(id, pending);
		// Reclaimed bodies may already be queued; await a boundary or idle proof.
		this.showMailStatus();
		if (claim.attempt > 0) return;
		if (this.compacting) {
			// A claim response can cross compaction start. Keep it, but do not inject.
			pending.boundaryDueAt = this.clock() + this.boundaryGraceMs;
			return;
		}
		try {
			this.session.onInbound(claim.message, claim.message.messageId, id);
		} catch (error) {
			this.pendingConsumption.delete(id);
			this.showMailStatus();
			throw error;
		}
		if (this.self === seat) await this.flush();
	}

	private async executeControl(command: string): Promise<ControlOutcome> {
		const validated = validateCommand(command);
		if (!validated.ok) return { outcome: "refused", reason: validated.message };
		try {
			if (isControlCommand(validated.value)) {
				if (!(await this.pi.control(validated.value))) {
					return {
						outcome: "refused",
						reason:
							"Control context is unarmed; ask the human to run /pij, then resend the command",
					};
				}
			} else {
				await this.pi.compact();
			}
			return { outcome: "executed" };
		} catch (error) {
			return { outcome: "refused", reason: error instanceof Error ? error.message : String(error) };
		}
	}

	private async ackClaim(
		claim: InboxClaim,
		seat: string,
		client: PijDaemonClient,
		outcome?: ControlOutcome,
		deliveryOutcome?: "undelivered:harness-swallowed",
	): Promise<boolean> {
		try {
			await client.ackInbox(seat, claim.jobId, outcome, deliveryOutcome);
			return true;
		} catch (error) {
			this.session.capture("daemon_event_ack_error", {
				jobId: claim.jobId,
				messageId: claim.message.messageId,
				message: error instanceof Error ? error.message : String(error),
			});
			return false;
		}
	}

	private announceArrival(claim: InboxClaim): void {
		const body = claim.message.body ?? "";
		const shown =
			body.length > ANNOUNCE_BODY_CHARS ? `${body.slice(0, ANNOUNCE_BODY_CHARS)}…` : body;
		const sender = senderLabel(claim.message.from, claim.message.fromMachine);
		this.pi.notify?.(`📨 pij from ${sender}: ${shown}`, "info");
	}

	private showMailStatus(): void {
		let count = 0;
		const now = this.clock();
		for (const pending of this.pendingConsumption.values()) {
			if (
				!pending.consumed &&
				(pending.lastResentAt === undefined || now - pending.lastResentAt >= this.redeliverIdleMs)
			)
				count++;
		}
		this.pi.setStatus(count > 0 ? `📨 ${count} pending` : undefined);
	}

	private async heartbeatPending(
		pending: PendingConsumption,
		force = false,
	): Promise<"running" | "unknown" | "skip"> {
		const now = this.clock();
		if (
			pending.consumed ||
			pending.acknowledging ||
			pending.heartbeating ||
			this.self !== pending.seat ||
			this.lifecycle !== pending.lifecycle ||
			this.pendingConsumption.get(messageKey(pending.claim.message)) !== pending
		)
			return "skip";
		// Stream reconnect owns daemon outage retries; local idle recovery remains available.
		if (pending.client.isReconnecting()) return "unknown";
		// Neither unknown wire data nor a failed forced request may create a 1 Hz retry loop.
		if (
			(!force || pending.heartbeatUncertain) &&
			now - pending.lastHeartbeatAt < CLAIM_HEARTBEAT_MS
		)
			return pending.heartbeatUncertain ? "unknown" : "skip";
		pending.heartbeating = true;
		pending.lastHeartbeatAt = now;
		try {
			const response = await pending.client.heartbeatInbox(pending.seat, pending.claim.jobId);
			if (
				this.lifecycle !== pending.lifecycle ||
				this.self !== pending.seat ||
				this.pendingConsumption.get(messageKey(pending.claim.message)) !== pending
			)
				return "skip";
			pending.heartbeatUncertain = response === undefined;
			if (response === undefined) {
				this.session.capture("daemon_event_heartbeat_unknown", { jobId: pending.claim.jobId });
				return "unknown";
			}
			if (response.state === "done" || response.state === "failed") {
				this.dropTerminal(pending.claim.jobId, pending.claim.message);
				return "skip";
			}
			return "running";
		} catch (error) {
			pending.heartbeatUncertain = true;
			if (
				!pending.consumed &&
				this.self === pending.seat &&
				this.lifecycle === pending.lifecycle &&
				this.pendingConsumption.get(messageKey(pending.claim.message)) === pending
			) {
				this.session.capture("daemon_event_heartbeat_error", {
					jobId: pending.claim.jobId,
					messageId: pending.claim.message.messageId,
					message: error instanceof Error ? error.message : String(error),
				});
			}
			return "unknown";
		} finally {
			pending.heartbeating = false;
		}
	}

	private async ackConsumed(
		pending: PendingConsumption,
		deliveryOutcome?: "undelivered:harness-swallowed",
	): Promise<void> {
		if (pending.client.isReconnecting()) return;
		if (pending.acknowledging || this.self !== pending.seat || this.lifecycle !== pending.lifecycle)
			return;
		pending.acknowledging = true;
		let acked = await this.ackClaim(
			pending.claim,
			pending.seat,
			pending.client,
			undefined,
			deliveryOutcome,
		);
		if (
			!acked &&
			!pending.client.isReconnecting() &&
			this.self === pending.seat &&
			this.lifecycle === pending.lifecycle
		) {
			try {
				const response = await pending.client.peekInbox(pending.seat);
				if (this.self !== pending.seat || this.lifecycle !== pending.lifecycle) return;
				const oldest = this.readInbox(response, pending.seat);
				// A later live head, or no live head, proves this job is terminal;
				// parked history is not an outstanding acknowledgement.
				acked = oldest === undefined || oldest.jobId > pending.claim.jobId;
			} catch {
				// The ACK error is already recorded; retain consumption proof until reconciliation succeeds.
			}
		}
		pending.acknowledging = false;
		if (this.self !== pending.seat || this.lifecycle !== pending.lifecycle) return;
		this.inboxPending = true; // An expired claim may need reclaiming before ACK can succeed.
		if (!acked) return;
		this.lastAcknowledgedJobId = pending.claim.jobId;
		this.pendingConsumption.delete(messageKey(pending.claim.message));
		if (deliveryOutcome !== undefined) {
			this.noteParked(pending.claim.jobId, pending.claim.message.messageId, deliveryOutcome);
		}
		this.showMailStatus();
	}

	private recoverUnconsumed(): void {
		if (this.compacting) return;
		const now = this.clock();
		if (!this.pi.isIdle()) this.idleSince = undefined;
		else this.idleSince ??= now;
		for (const pending of this.pendingConsumption.values()) {
			if (pending.consumed || pending.acknowledging || pending.heartbeating) continue;
			const boundary =
				pending.resends === 0 &&
				pending.boundaryDueAt !== undefined &&
				now >= pending.boundaryDueAt;
			const idle =
				this.idleSince !== undefined &&
				now - Math.max(this.idleSince, pending.injectedAt) >= this.redeliverIdleMs;
			if (boundary || idle) void this.recoverPending(pending, boundary);
		}
		this.showMailStatus();
	}

	private async recoverPending(pending: PendingConsumption, boundary: boolean): Promise<void> {
		// Only affirmative, validated terminal authority may suppress local idle recovery.
		const state = await this.heartbeatPending(pending, true);
		if (
			state === "skip" ||
			this.compacting ||
			pending.consumed ||
			this.pendingConsumption.get(messageKey(pending.claim.message)) !== pending
		)
			return;
		if (state === "unknown") boundary = false;
		if (
			!boundary &&
			(!this.pi.isIdle() ||
				this.idleSince === undefined ||
				this.clock() - Math.max(this.idleSince, pending.injectedAt) < this.redeliverIdleMs)
		)
			return;
		if (pending.resends >= MAX_RESENDS) {
			await this.ackConsumed(pending, "undelivered:harness-swallowed");
		} else {
			this.resendPending(pending, boundary ? "boundary-unconsumed" : "idle-unconsumed");
		}
	}

	private resendPending(pending: PendingConsumption, reason: string): void {
		if (
			pending.consumed ||
			pending.acknowledging ||
			this.compacting ||
			pending.resends >= MAX_RESENDS ||
			this.self !== pending.seat ||
			this.lifecycle !== pending.lifecycle ||
			this.pendingConsumption.get(messageKey(pending.claim.message)) !== pending
		)
			return;
		const attempt = ++pending.resends;
		pending.injectedAt = this.clock();
		pending.lastResentAt = pending.injectedAt;
		const message = pending.claim.message;
		this.session.capture("delivery.resend", {
			messageId: message.messageId,
			attempt,
			reason,
		});
		this.showMailStatus();
		try {
			this.pi.inject(
				frame(message.from, message.body, message.fromMachine),
				"immediate",
				messageKey(message),
				attempt,
			);
		} catch (error) {
			this.session.capture("daemon_event_error", {
				message: error instanceof Error ? error.message : String(error),
				stage: "delivery-resend",
				messageId: message.messageId,
			});
		}
	}

	private noteParked(jobId: number, messageId: string, outcome: string): void {
		for (const [key, pending] of this.pendingConsumption) {
			if (pending.claim.jobId === jobId) this.pendingConsumption.delete(key);
		}
		this.inboxPending = true;
		this.showMailStatus();
		if (this.parkedJobs.has(jobId)) return;
		this.parkedJobs.add(jobId);
		this.pi.notify(`pij parked message ${messageId}: ${outcome}`, "warning");
	}

	private dropTerminal(jobId: number, message: InboxClaim["message"]): void {
		this.lastAcknowledgedJobId = jobId;
		this.pendingConsumption.delete(messageKey(message));
		this.inboxPending = true;
		this.showMailStatus();
	}

	private readInbox(value: unknown, seat: string): InboxClaim | undefined {
		return pushedInboxClaim(
			value,
			seat,
			(jobId, messageId, outcome) => this.noteParked(jobId, messageId, outcome),
			(jobId, message) => this.dropTerminal(jobId, message),
		);
	}

	private chooseIdentity(
		input: RustBootInput,
		procStart: number,
		seats: readonly RustSeatDescriptor[],
		replacing: boolean,
	): string {
		if (this.self !== "" && !replacing) return this.self;
		if (!replacing) {
			const samePane =
				input.paneId === undefined
					? undefined
					: seats.find((seat) => seat.pane === input.paneId && seat.harness === input.runtimeBin);
			if (samePane) return samePane.id;
			const sameProcess = seats.find(
				(seat) => seat.proc?.pid === process.pid && seat.proc.proc_start === procStart,
			);
			if (sameProcess) return sameProcess.id;
			const spawnedId = process.env.PIJ_SESSION_ID?.trim();
			if (spawnedId) return spawnedId;
		}
		const used = new Set(seats.map((seat) => seat.id));
		if (replacing && this.self !== "") used.add(this.self);
		const seed = memorableIdentitySeed(
			"pi",
			input.piSessionId ?? `pid:${process.pid}:${procStart}`,
		);
		for (const candidate of memorablePijIdCandidates(seed)) {
			if (!used.has(candidate)) return candidate;
		}
		throw new Error("pij memorable id space is exhausted");
	}

	private flush(): Promise<void> {
		const outgoing = this.delivery.take();
		const lifecycle = this.lifecycle;
		// The chain must NEVER be left rejected. Two callers are fire-and-forget
		// (`void this.flush()` from spawn and onTurnStart), so a rejection there is
		// an unhandled rejection — and because the rejected promise is STORED, every
		// later flush inherits it. On quit an in-flight send is aborted, which is how
		// a clean exit turned into `[Unhandled Rejection] AbortError` (observed
		// 2026-09-11). A failed send is captured per item instead: the outbox item is
		// already taken, so silence would lose it without a trace.
		this.flushChain = this.flushChain.then(async () => {
			for (const item of outgoing) {
				if (item.message.kind === "receipt") continue;
				try {
					await this.client.send({
						from: item.message.from,
						to: { seat: item.message.to },
						body: item.message.body,
						msg_id: item.messageId,
						...(item.message.command === undefined
							? {}
							: {
									command: item.message.command,
									caller: buildCallerContext(process.env, {
										cwd: process.cwd(),
										pid: process.pid,
										procStart: this.client.processStart(process.pid),
									}),
								}),
					});
				} catch (error) {
					const message = error instanceof Error ? error.message : String(error);
					this.session.capture("daemon_event_flush_error", {
						messageId: item.messageId,
						to: item.message.to,
						shuttingDown: this.lifecycle !== lifecycle,
						message,
					});
					// Only a live session can act on it; during teardown the abort IS
					// the expected outcome and a notice would be noise on the way out.
					if (this.lifecycle === lifecycle) {
						this.pi.notify?.(`pij: send to ${item.message.to} failed: ${message}`, "error");
					}
				}
			}
		});
		return this.flushChain;
	}
}

function registrationFor(
	id: string,
	procStart: number,
	input: RustBootInput,
	supersedes?: string,
): Registration {
	const effort = process.env.PIJ_SPAWN_EFFORT;
	const requestedModel = process.env.PIJ_SPAWN_MODEL;
	const effortSuffix = effort ? `:${effort}` : "";
	const model =
		effortSuffix && requestedModel?.endsWith(effortSuffix)
			? requestedModel.slice(0, -effortSuffix.length)
			: requestedModel;
	return {
		...(supersedes === undefined ? {} : { supersedes }),
		id,
		harness: input.runtimeBin,
		extension_build: input.extension_build,
		extension_path: input.extension_path,
		...(input.piSessionId === undefined ? {} : { harness_session: input.piSessionId }),
		folder: input.folder,
		...(input.paneId === undefined ? {} : { pane: input.paneId }),
		pid: process.pid,
		proc_start: procStart,
		...(process.env.PIJ_SPAWN_ID ? { spawn_id: process.env.PIJ_SPAWN_ID } : {}),
		...(model ? { model } : {}),
		...(input.actualModel === undefined
			? {}
			: {
					actual_model_observed: true,
					...(input.actualModel === null ? {} : { actual_model: input.actualModel }),
				}),
		...(process.env.PIJ_SPAWN_PROVIDER ? { provider: process.env.PIJ_SPAWN_PROVIDER } : {}),
		...(effort ? { effort } : {}),
		...(input.parentId === undefined || input.parentId === null ? {} : { parent: input.parentId }),
		relay: false,
	};
}

function fromRustDescriptor(seat: RustSeatDescriptor): SessionDescriptor {
	return {
		id: seat.id,
		folder: seat.folder,
		dataDir: "",
		eventsPath: "",
		pid: seat.proc?.pid ?? 0,
		startedAt: new Date().toISOString(),
		state: seat.state === "working" ? "working" : "idle",
		harness:
			seat.harness === "claude" || seat.harness === "copilot" || seat.harness === "codex"
				? seat.harness
				: "pi",
		runtimeBin: seat.harness === "omp" ? "omp" : seat.harness === "pi" ? "pi" : undefined,
		...(seat.pane === undefined || seat.pane === null ? {} : { paneId: seat.pane }),
		...(seat.parent === undefined || seat.parent === null ? {} : { spawnedBy: seat.parent }),
		...(seat.spawn_id === undefined ? {} : { spawnId: seat.spawn_id }),
	};
}

function pushedMessage(payload: string, to: string): InboxClaim["message"] {
	return pushedMessageValue(JSON.parse(payload) as unknown, to, "message.pushed payload");
}

function pushedInboxClaim(
	value: unknown,
	to: string,
	onParked: (jobId: number, messageId: string, outcome: string) => void,
	onDone: (jobId: number, message: InboxClaim["message"]) => void,
): InboxClaim | undefined {
	if (!Array.isArray(value)) throw new Error("pij inbox response must be an array");
	let live: InboxClaim | undefined;
	for (const row of value) {
		if (typeof row !== "object" || row === null || Array.isArray(row)) {
			throw new Error("pij inbox claim must be an object");
		}
		const record = row as Record<string, unknown>;
		if (
			typeof record.job_id !== "number" ||
			!Number.isSafeInteger(record.job_id) ||
			record.job_id <= 0
		)
			throw new Error("pij inbox claim job_id must be a positive safe integer");
		const attempt = record.attempt === undefined ? 0 : record.attempt;
		if (typeof attempt !== "number" || !Number.isSafeInteger(attempt) || attempt < 0) {
			throw new Error("pij inbox claim attempt must be a non-negative safe integer");
		}
		const message = pushedMessageValue(record.message, to, "pij inbox claim message");
		if (record.state === "done") {
			onDone(record.job_id, message);
			continue;
		}
		if (record.state === "failed") {
			if (typeof record.outcome !== "string" || !PARKED_OUTCOMES.has(record.outcome)) {
				throw new Error("pij parked inbox claim needs a recognized outcome");
			}
			onParked(record.job_id, message.messageId, record.outcome);
			continue;
		}
		if (record.state !== undefined && record.state !== "pending" && record.state !== "running") {
			throw new Error("pij inbox claim has an unrecognized state");
		}
		if (live !== undefined)
			throw new Error("pij inbox response must contain at most one live claim");
		live = { jobId: record.job_id, attempt, message };
	}
	return live;
}

/** In-memory delivery key for one message identity, (origin machine, msg_id); the wire
 *  msg_id stays raw. A local key is the bare id, as before; a forwarded one appends
 *  `@<alias>`. Injective: the daemon refuses a local msg_id containing `@` and an alias
 *  never contains one, so no local id can name a forwarded message (plan 164 F02). This
 *  is also the consumption marker (`pijMessageId`) a native injection carries. */
function messageKey(message: {
	readonly messageId: string;
	readonly fromMachine?: string;
}): string {
	return message.fromMachine === undefined
		? message.messageId
		: `${message.messageId}@${message.fromMachine}`;
}

export function consumedMessageId(message: {
	role: string;
	customType?: string;
	details?: unknown;
	content?: unknown;
}): string | undefined {
	if (message.role === "custom" && message.customType === "pij") {
		const details = message.details;
		if (details && typeof details === "object" && "pijMessageId" in details) {
			return typeof details.pijMessageId === "string" ? details.pijMessageId : undefined;
		}
	}
	if (message.role !== "user") return undefined;
	const content = message.content;
	const text =
		typeof content === "string"
			? content
			: Array.isArray(content)
				? content.find(
						(part: unknown) =>
							part !== null &&
							typeof part === "object" &&
							"type" in part &&
							part.type === "text" &&
							"text" in part,
					)?.text
				: undefined;
	if (typeof text !== "string") return undefined;
	const marker = /^\[pij resend [1-3]\]\n\[pijMessageId:([^\]\r\n]+)\]\n/.exec(text);
	if (!marker?.[1]) return undefined;
	try {
		return decodeURIComponent(marker[1]);
	} catch {
		return undefined;
	}
}

function pushedMessageValue(value: unknown, to: string, label: string): InboxClaim["message"] {
	if (typeof value !== "object" || value === null || Array.isArray(value)) {
		throw new Error(`${label} must be an object`);
	}
	const record = value as Record<string, unknown>;
	if (
		typeof record.msg_id !== "string" ||
		typeof record.from !== "string" ||
		typeof record.body !== "string"
	) {
		throw new Error(`${label} needs string msg_id, from, and body`);
	}
	return {
		messageId: record.msg_id,
		from: record.from,
		...(typeof record.from_machine === "string" ? { fromMachine: record.from_machine } : {}),
		to,
		body: record.body,
		...(typeof record.command === "string" ? { command: record.command } : {}),
		...(record.urgent === true ? { urgent: true } : {}),
	};
}

class MutablePiRuntime implements PiRuntimePort {
	private current: VisibleRuntimePort | undefined;

	setStatus(text: string | undefined): void {
		this.current?.setStatus?.(text);
	}

	setFyiStatus(text: string | undefined): void {
		this.current?.setFyiStatus?.(text);
	}

	/** Optional on the port: a runtime with no notification surface simply does
	 * not announce, and the status count still reports what it cannot show. */
	notify(text: string, level?: "info" | "warning" | "error"): void {
		this.current?.notify?.(text, level);
	}

	set(pi: VisibleRuntimePort): void {
		this.current = pi;
	}

	isIdle(): boolean {
		return this.need().isIdle();
	}

	inject(
		text: string,
		mode: "immediate" | "steer",
		messageId?: string,
		resendAttempt?: number,
	): void {
		this.need().inject(text, mode, messageId, resendAttempt);
	}

	compact(): void | Promise<void> {
		return this.need().compact();
	}

	control(command: "new" | "reload"): boolean | Promise<boolean> {
		return this.need().control(command);
	}

	private need(): PiRuntimePort {
		if (!this.current) throw new Error("pij Rust runtime has no active Pi context");
		return this.current;
	}
}

class MemoryRegistry implements RegistryPort {
	private readonly seats = new Map<string, SessionDescriptor>();

	seed(descriptor: SessionDescriptor): void {
		if (!this.seats.has(descriptor.id)) this.seats.set(descriptor.id, descriptor);
	}

	list(): SessionDescriptor[] {
		return [...this.seats.values()].filter((seat) => seat.lifecycle !== "dissolved");
	}

	read(id: SessionId): SessionDescriptor | null {
		return this.seats.get(id) ?? null;
	}

	write(descriptor: SessionDescriptor): void {
		this.seats.set(descriptor.id, descriptor);
	}

	writeExact(descriptor: SessionDescriptor): void {
		this.write(descriptor);
	}

	revive(descriptor: SessionDescriptor): Result<void> {
		this.write(descriptor);
		return ok(undefined);
	}

	remove(id: SessionId): void {
		this.seats.delete(id);
	}

	dissolve(id: SessionId): void {
		const seat = this.seats.get(id);
		if (seat) this.seats.set(id, { ...seat, lifecycle: "dissolved" });
	}
}

class MemoryEventLog implements EventLogPort {
	private readonly events: PijEvent[] = [];

	append(event: PijEvent): void {
		this.events.push(event);
	}

	read(query: EventQuery = {}): PijEvent[] {
		let selected = this.events.filter(
			(event) =>
				(query.since === undefined || event.seq > query.since) &&
				(query.type === undefined || event.type === query.type),
		);
		if (query.last !== undefined) selected = selected.slice(-query.last);
		return selected;
	}

	lastSeq(): number {
		return this.events.at(-1)?.seq ?? 0;
	}

	count(): number {
		return this.events.length;
	}
}

class BufferedDelivery implements DeliveryPort {
	private readonly outgoing: Array<{ messageId: string; message: PijMessage }> = [];

	deliver(message: PijMessage): Result<{ messageId: string }> {
		if (!message.to) return err("E-NOID", "pij-rs delivery needs a recipient");
		const messageId = randomUUID();
		this.outgoing.push({ messageId, message });
		return ok({ messageId });
	}

	take(): Array<{ messageId: string; message: PijMessage }> {
		return this.outgoing.splice(0);
	}
}
