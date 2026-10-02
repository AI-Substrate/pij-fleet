// pij-messaging — PijSession coordinator (Pattern P2: Pi-free; P3: ports by DI;
// P4: tagged-union returns; P8: the testable backbone).
//
// Holds ALL of the extension's wiring logic — boot/announce, event capture,
// the idle/steer delivery injector, command execution, delivery receipts, and
// shutdown — against the 5 frozen ports + plain data. The pi extension
// (index.ts) is a thin pi-event -> coordinator translator that owns no logic,
// so the behaviour here is proven against fakes before any live-pi smoke.

import { type ControlCommand, isControlCommand, validateCommand } from "./commands.js";
import { buildEvent } from "./events.js";
import { announceText, frame, receiptBody } from "./message.js";
import type { ModelEntry } from "./models/registry.js";
import type {
	DeliveryPort,
	EventLogPort,
	PiRuntimePort,
	ProcessPort,
	RegistryPort,
	SpawnExpectationStore,
	TmuxPort,
} from "./ports.js";
import {
	classifyOnInject,
	correlateDeliveredAt,
	initialReceipt,
	markDelivered,
} from "./receipts.js";
import { buildSeatLabel } from "./seat-label.js";
import { SeqCounter } from "./seq.js";
import {
	buildSpawnCommand,
	readyBody,
	resolvePiModelBinding,
	type SpawnHarness,
	STACK_COLUMN_PERCENT,
} from "./spawn.js";
import {
	applyTerminalObservation,
	bindSpawnExpectation,
	createSpawnExpectation,
	requestClose,
	spawnExpectationDeadline,
} from "./spawn-expectation.js";
import {
	err,
	type MessageReceipt,
	ok,
	type PijErrorCode,
	type PijMessage,
	type ReceiptState,
	type Result,
	type Role,
	type SessionDescriptor,
	type SessionId,
	type WatchdogSidecar,
} from "./types.js";
import { applyCompactPause, applyWatchdogExemption, applyWorkingTransition } from "./watchdog.js";

/** Exact installed Pi lifecycle contract: replacement reasons carry a successor,
 * whereas quit is left for the daemon's PID/pane observation. */
export type SessionShutdownReason = "quit" | "reload" | "new" | "resume" | "fork";

export function shutdownDecision(
	reason: SessionShutdownReason,
): { readonly kind: "dissolve-replacement" } | { readonly kind: "await-terminal-observation" } {
	return reason === "reload" || reason === "new" || reason === "resume" || reason === "fork"
		? { kind: "dissolve-replacement" }
		: { kind: "await-terminal-observation" };
}

export interface WatchdogSessionStore {
	read(id: SessionId): WatchdogSidecar | undefined;
	write(id: SessionId, sidecar: WatchdogSidecar): void;
}

/** The coordinator seams (constructor-injected). */
export interface PijPorts {
	readonly registry: RegistryPort;
	readonly eventLog: EventLogPort;
	readonly delivery: DeliveryPort;
	readonly pi: PiRuntimePort;
	readonly process: ProcessPort;
	/** Tmux seam: open/kill windows, check session. Wired in index.ts only
	 *  (P2: core stays tmux-free); `FakeTmux` used in session.test.ts (P8). */
	readonly tmux: TmuxPort;
	/** Optional during migration; runtime wiring supplies the sidecar adapter. */
	readonly watchdog?: WatchdogSessionStore;
	/** Durable pre-launch intent, independent from child self-registration. */
	readonly expectations?: SpawnExpectationStore;
	/** Runtime model catalog used to resolve provider ambiguity before spawning. */
	readonly models?: readonly ModelEntry[];
}

/** Input to PijSession.spawn(). The session generates spawnId + announceTo
 *  internally; callers supply task/model/cwd. */
export interface SpawnOpts {
	/** Required harness; external runtimes use the native daemon launch path. */
	readonly harness: SpawnHarness;
	/** Optional model override for the worker (passed via --model + PIJ_SPAWN_MODEL). */
	readonly model?: string;
	/** Optional reasoning effort pinned for the worker. */
	readonly effort?: string;
	/** Optional first task (delivered via PIJ_SPAWN_TASK env; finding 01 / CF-01). */
	readonly task?: string;
	/** Absolute working directory for the new pi session. */
	readonly cwd: string;
	/** "split" (the DEFAULT — unset behaves the same) stacks the worker in a
	 *  ~1/3-width column on the caller's right (uncapped; the stack evens itself);
	 *  "window" opts out into a new background tmux window. */
	readonly layout?: "window" | "split";
}

/** What index.ts hands `boot` once it has minted/derived the session id from
 *  pi's own session identity (NOT resolveSelf — that is the CLI's resolver). */
export interface BootInput {
	readonly id: SessionId;
	readonly role?: Role;
	readonly folder: string;
	readonly dataDir: string;
	readonly eventsPath: string;
	/** Exact native identity persisted by Pi so hash collisions can be detected. */
	readonly harness?: "pi";
	readonly harnessSessionId?: string;
	/** Actual Pi-family executable hosting this session. */
	readonly runtimeBin?: "pi" | "omp";
	readonly paneId?: string;
	/** Structural parent from PIJ_PARENT_ID; null is an explicit root. */
	readonly parentId?: SessionId | null;
	/** Fresh canonical git common directory for this registration. */
	readonly gitCommonDir?: string;
	/** Explicit plan attestation from the spawn environment. */
	readonly planId?: string;
	/** Presence-independent metadata restored when shutdown removed the descriptor. */
	readonly durableDescriptor?: SessionDescriptor;
	/** Startup/resume/new/fork replace a prior process incarnation; reload does not. */
	readonly resetRuntimeState?: boolean;
	readonly fresh?: boolean;
}

/** Boot outcome — the (id, role) the wiring exports to PIJ_SESSION_ID/PIJ_ROLE,
 *  plus whether this was a first boot (announce fired) or a reload (it didn't). */
export interface BootResult {
	readonly id: SessionId;
	readonly role?: Role;
	readonly fresh: boolean;
}

export type InboundResult =
	| { readonly kind: "delivered"; readonly state: ReceiptState }
	| { readonly kind: "command-executed"; readonly command: string }
	| { readonly kind: "command-started"; readonly command: string }
	| { readonly kind: "command-deferred"; readonly command: string }
	| { readonly kind: "command-rejected"; readonly code: PijErrorCode }
	| { readonly kind: "receipt-recorded" };

interface PendingReceipt {
	readonly injectIso: string;
	readonly receipt: MessageReceipt;
}

function stripPriorRuntimeTermination(
	descriptor: SessionDescriptor | null,
): SessionDescriptor | null {
	if (!descriptor) return null;
	const {
		closeIntent: _closeIntent,
		terminal: _terminal,
		deathNoticeLatchedAt: _deathNoticeLatchedAt,
		failureReason: _failureReason,
		initInjectedAt: _initInjectedAt,
		lastTickAt: _lastTickAt,
		lastInboxScanAt: _lastInboxScanAt,
		compactingAt: _compactingAt,
		lastWatchdogFireAt: _lastWatchdogFireAt,
		...durable
	} = descriptor;
	return durable;
}

export class PijSession {
	private self: SessionId = "";
	private role: Role | undefined;
	private seq = new SeqCounter(0);
	/** Monotonic counter for deterministic spawnId generation (§M4). */
	private spawnCounter = 0;
	/** Pane ids this session has split into the current window (the default side
	 *  stack), in spawn order. Tracked parent-side because the child's registry
	 *  descriptor is written only when it boots — a registry-only count would race
	 *  back-to-back fire-and-forget split spawns (all see 0 kids → all -h, no
	 *  stack). Pruned to live panes on each use. */
	private splitPanes: string[] = [];
	private pending: PendingReceipt[] = [];
	/** new|reload requests that arrived before a command context was armed; the
	 *  `/pij` handler drains these via applyPendingControl() (finding: command
	 *  context only exists inside a registered command handler). */
	private pendingControl: ControlCommand[] = [];
	private descriptor: SessionDescriptor | undefined;
	/** Persist cadence for the poll-primary delivery liveness heartbeat (plan 057
	 *  thread-1): the delivery poll runs every 500ms, but lastInboxScanAt is
	 *  written only this often to bound descriptor writes. The daemon's
	 *  inbox-poll-stalled anomaly reads the coarse persisted copy; the accurate
	 *  per-scan time flows through noteInboxScan's argument, so no in-memory field
	 *  is stored. */
	private static readonly INBOX_SCAN_STAMP_CADENCE_MS = 2_500;
	private lastPersistedInboxScanMs = 0;

	constructor(private readonly ports: PijPorts) {}

	/** session_start (all reasons, P10). Mints/persists the descriptor on first
	 *  boot, reuses it on reload (no duplicate, no replay), seeds the seq counter
	 *  from the log, and announces once on a fresh session. */
	boot(input: BootInput): BootResult {
		const liveDescriptor = this.ports.registry.read(input.id);
		const existing = liveDescriptor ?? input.durableDescriptor ?? null;
		const wasDissolved = liveDescriptor?.lifecycle === "dissolved";
		const fresh = input.fresh ?? (liveDescriptor === null || wasDissolved);
		const persisted =
			input.resetRuntimeState || wasDissolved ? stripPriorRuntimeTermination(existing) : existing;
		const descriptor: SessionDescriptor = {
			// Durable identity/history metadata survives a process or machine restart.
			// Runtime attachment fields below deliberately replace the prior incarnation.
			...persisted,
			id: input.id,
			role: input.role ?? existing?.role,
			folder: input.folder,
			dataDir: input.dataDir,
			eventsPath: input.eventsPath,
			pid: this.ports.process.pid(),
			startedAt: existing?.startedAt ?? this.nowIso(),
			state: input.resetRuntimeState ? "idle" : (existing?.state ?? "idle"),
			lastEventAt: existing?.lastEventAt,
			harness: input.harness ?? existing?.harness,
			harnessSessionId: input.harnessSessionId ?? existing?.harnessSessionId,
			runtimeBin: input.runtimeBin ?? existing?.runtimeBin,
			lifecycle: input.resetRuntimeState || wasDissolved ? undefined : existing?.lifecycle,
			failureReason: input.resetRuntimeState ? undefined : existing?.failureReason,
			// Runtime pane comes from this incarnation; creator relation is durable.
			paneId: input.resetRuntimeState ? input.paneId : (input.paneId ?? existing?.paneId),
			spawnedBy:
				wasDissolved && input.resetRuntimeState
					? this.ports.process.env("PIJ_ANNOUNCE_TO") || undefined
					: existing?.spawnedBy,
			...(input.parentId !== undefined ? { parentId: input.parentId } : {}),
			...(input.gitCommonDir !== undefined ? { gitCommonDir: input.gitCommonDir } : {}),
			...(existing?.planId !== undefined
				? { planId: existing.planId }
				: input.planId !== undefined
					? { planId: input.planId }
					: {}),
		};
		if (wasDissolved) {
			const revived = this.ports.registry.revive(descriptor);
			if (!revived.ok) throw new Error(`pij revive error: ${revived.message}`);
		} else {
			// writeExact, deliberately: boot REPLACES the prior incarnation. When a seat
			// is re-adopted (or was dissolved), `stripPriorRuntimeTermination` clears
			// `terminal`/`closeIntent`/`failureReason`, and the merging write would
			// restore them from disk — leaving a live seat wearing a dead one's
			// tombstone. Clearing a contested field is exactly what writeExact is for.
			this.ports.registry.writeExact(descriptor);
		}
		this.descriptor = descriptor;
		this.self = input.id;
		if (fresh && this.ports.process.env("PIJ_NO_WATCHDOG") === "1") {
			const current = this.ports.watchdog?.read(this.self);
			this.ports.watchdog?.write(
				this.self,
				applyWatchdogExemption(current, this.ports.process.now()),
			);
		}
		this.role = descriptor.role;
		this.seq = new SeqCounter(this.ports.eventLog.lastSeq());
		if (fresh) {
			const announceTo = this.ports.process.env("PIJ_ANNOUNCE_TO");
			if (announceTo) {
				// Spawned child boot path (P10: additive, fresh-guarded).
				// §H1: read own pane id from $TMUX_PANE — tmux sets it natively in
				// every new pane; no PIJ_PANE_ID needed in the child env.
				const tmuxPane = this.ports.process.env("TMUX_PANE");
				const spawnId = this.ports.process.env("PIJ_SPAWN_ID") ?? "";
				// §H2: model via PIJ_SPAWN_MODEL (set by spawner); "" = default model
				const spawnModel = this.ports.process.env("PIJ_SPAWN_MODEL") ?? "";
				const provider = this.ports.process.env("PIJ_SPAWN_PROVIDER");
				const effort = this.ports.process.env("PIJ_SPAWN_EFFORT");
				const effortSuffix = effort ? `:${effort}` : "";
				const model =
					effortSuffix && spawnModel.endsWith(effortSuffix)
						? spawnModel.slice(0, -effortSuffix.length)
						: spawnModel;
				// P9: persist paneId/spawnedBy BEFORE the ready-ping
				this.persist({
					paneId: tmuxPane,
					spawnedBy: announceTo,
					...(model ? { boundModel: model } : {}),
					...(provider ? { boundProvider: provider } : {}),
					...(effort ? { effort } : {}),
					...(spawnId ? { spawnId } : {}),
				});
				if (spawnId) {
					const expectation = this.ports.expectations?.read(spawnId);
					if (expectation) {
						this.ports.expectations?.write(
							bindSpawnExpectation(expectation, {
								sessionId: this.self,
								paneId: tmuxPane,
								runtimeHarness: "pi",
								boundAt: this.nowIso(),
							}),
						);
					}
				}
				// Deliver the ready-ping via channel (event — never an inject)
				this.ports.delivery.deliver({
					from: this.self,
					to: announceTo,
					body: readyBody(spawnId, model, input.folder, effort),
				});
				// Finding 07: exactly ONE inject at boot — suppress announceText
				// when a task is present (the CF-01 mitigation; finding 01).
				const task = this.ports.process.env("PIJ_SPAWN_TASK");
				if (task) {
					this.ports.pi.inject(task, "immediate");
				} else {
					this.ports.pi.inject(announceText(input.id, descriptor.role), "immediate");
				}
			} else {
				// A revive task is safety-critical even when invoked from a root/non-peer
				// shell: do not let a missing announcement target suppress the reframe.
				const task = this.ports.process.env("PIJ_SPAWN_TASK");
				this.ports.pi.inject(task ?? announceText(input.id, descriptor.role), "immediate");
			}
		}
		return { id: input.id, role: descriptor.role, fresh };
	}

	/** Open a new tmux window running a pij worker and return immediately
	 *  (fire-and-forget: the child's ready-ping arrives via the delivery channel
	 *  once it has booted). */
	spawn(opts: SpawnOpts): Result<{ spawnId: string; paneId: string; notice?: string }> {
		if (opts.harness !== "pi" && opts.harness !== "omp")
			return err(
				"E-ARG",
				"local spawn requires --harness omp or pi; other harnesses require the Rust daemon",
			);
		const binding = resolvePiModelBinding(opts.model, this.ports.models ?? []);
		if (!binding.ok) return binding;
		// §M5: E-NOTMUX lives here (P8-testable against FakeTmux)
		if (this.ports.tmux.currentSession() === null) {
			return err("E-NOTMUX", "not inside a tmux session — cannot spawn a pij worker window");
		}
		// §M4: deterministic spawnId via clock + per-session counter (not crypto.randomUUID)
		const spawnId = `s${this.ports.process.now()}-${this.spawnCounter++}`;
		const requestedAt = this.nowIso();
		const expectation = createSpawnExpectation({
			spawnId,
			creatorId: this.self,
			requestedHarness: opts.harness,
			requestedAt,
			deadlineAt: spawnExpectationDeadline(requestedAt),
		});
		// Persist intent before opening a pane: a vanished child may never produce a descriptor.
		this.ports.expectations?.write(expectation);
		const spawnCmd = buildSpawnCommand({
			harness: opts.harness,
			model: binding.value.model,
			provider: binding.value.provider,
			effort: opts.effort,
			task: opts.task,
			spawnId,
			announceTo: this.self,
			// paneId NOT passed (§H1): child reads its own $TMUX_PANE at boot
			cwd: opts.cwd,
			role: "worker",
		});
		// §H2: PIJ_SPAWN_MODEL is now emitted by buildSpawnCommand itself when
		// input.model is set — no post-process needed here.
		const seatLabel = buildSeatLabel({
			cwd: opts.cwd,
			job: "worker",
			peerId: `pending:${spawnId}`,
			model: binding.value.model,
		});

		// Side stack — the DEFAULT (layout unset or "split"): place the worker in
		// the CURRENT window. #1 splits the orchestrator pane LEFT/RIGHT (-h → a
		// ~1/3-width right column); every later worker splits the NEWEST peer pane
		// UP/DOWN (-v → appended to the stack) and the column is evened out.
		// UNCAPPED — panes just get shorter. layout:"window" opts out (below).
		if (opts.layout !== "window") {
			const here = new Set(this.ports.tmux.currentWindowPanes());
			// Live split panes in THIS window = parent-recorded panes (known the
			// instant splitWindow returns — the registry lags the fire-and-forget
			// child boot) UNION any registry kids, ordered, pruned to panes still
			// alive in the window (a closed pane drops out via currentWindowPanes()).
			this.splitPanes = this.splitPanes.filter((p) => here.has(p));
			const ordered: string[] = [...this.splitPanes];
			for (const d of this.ports.registry.list()) {
				if (
					d.spawnedBy === this.self &&
					d.paneId &&
					here.has(d.paneId) &&
					!ordered.includes(d.paneId)
				) {
					ordered.push(d.paneId);
				}
			}
			const newest = ordered[ordered.length - 1];
			const target = newest ?? this.ports.tmux.currentPane();
			if (!target) {
				return err("E-NOTMUX", "cannot resolve the current tmux pane to split");
			}
			const first = newest === undefined;
			const splitResult = this.ports.tmux.splitWindow({
				cmd: spawnCmd.cmd,
				args: spawnCmd.args,
				env: spawnCmd.env,
				title: seatLabel.paneTitle,
				cwd: opts.cwd,
				target,
				direction: first ? "h" : "v",
				percent: first ? STACK_COLUMN_PERCENT : undefined,
				evenOut: !first,
				columnPercent: first ? undefined : STACK_COLUMN_PERCENT,
				detached: true,
			});
			if (!splitResult.ok) {
				this.ports.expectations?.remove(spawnId);
				return err(splitResult.code, splitResult.message);
			}
			this.ports.expectations?.write({ ...expectation, paneId: splitResult.value.paneId });
			// Record parent-side so the NEXT spawn sees this pane before the child
			// has booted its descriptor (fixes the fire-and-forget stack-target race).
			this.splitPanes.push(splitResult.value.paneId);
			return ok({
				spawnId,
				paneId: splitResult.value.paneId,
				...(binding.value.notice ? { notice: binding.value.notice } : {}),
			});
		}

		const winResult = this.ports.tmux.newWindow({
			cmd: spawnCmd.cmd,
			args: spawnCmd.args,
			env: spawnCmd.env,
			name: seatLabel.windowName,
			title: seatLabel.paneTitle,
			cwd: opts.cwd,
		});
		if (!winResult.ok) {
			this.ports.expectations?.remove(spawnId);
			return err(winResult.code, winResult.message);
		}
		this.ports.expectations?.write({ ...expectation, paneId: winResult.value.paneId });
		return ok({
			spawnId,
			paneId: winResult.value.paneId,
			...(binding.value.notice ? { notice: binding.value.notice } : {}),
		});
	}

	/** Kill the tmux window of a spawned peer and remove its descriptor.
	 *  Returns `{ warning }` if the session was spawned by a different peer
	 *  (AC-06 non-owner close — still succeeds; caller should surface the text). */
	close(id: SessionId): Result<{ warning?: string }> {
		const descriptor = this.ports.registry.read(id);
		if (!descriptor) {
			return err("E-NOID", `no such session '${id}'`);
		}
		if (descriptor.lifecycle === "dissolved") return ok({});
		// §H3: descriptor.paneId is string|undefined; guard before killWindow
		if (!descriptor.paneId) {
			return err("E-NOID", `session '${id}' has no paneId — not a spawned window`);
		}
		// Warn when the caller did not spawn this session.
		// Also warn when spawnedBy is absent but paneId exists (spawned via an
		// external path — ownership is unknown — AC-06).
		let warning: string | undefined;
		if (descriptor.spawnedBy !== this.self) {
			warning = `warning: session '${id}' was spawned by ${
				descriptor.spawnedBy ?? "unknown"
			}; you are ${this.self}`;
			this.capture("receipt", {
				kind: "warn-close-not-mine",
				id,
				spawnedBy: descriptor.spawnedBy,
				closedBy: this.self,
			});
		}
		// Kill the pane (TmuxAdapter is idempotent: swallows "already gone").
		// kill-pane is split-safe AND closes the window when it is the last pane,
		// so it is correct for both window-mode and split-mode workers.
		const closeIntent = {
			actor: this.self,
			kind: "in-process-close" as const,
			requestedAt: this.nowIso(),
		};
		// Persist intent before touching tmux so a later absence is not misclassified.
		// "close": the close path is the sole writer of terminal truth (s070/#47).
		this.ports.registry.write({ ...descriptor, closeIntent }, "close");
		if (descriptor.spawnId) {
			const expectation = this.ports.expectations?.read(descriptor.spawnId);
			if (expectation) this.ports.expectations?.write(requestClose(expectation, closeIntent));
		}
		const killResult = this.ports.tmux.killPane(descriptor.paneId);
		if (!killResult.ok) {
			return err(killResult.code, killResult.message);
		}
		// killPane is idempotent: successful return observes the owned pane absent.
		const terminal = {
			disposition: "requested" as const,
			observedAt: this.nowIso(),
			evidence: "pane-missing" as const,
		};
		// "close": this IS the terminal-truth writer (s070/#47).
		this.ports.registry.write(
			{
				...descriptor,
				closeIntent,
				terminal,
				deathNoticeLatchedAt: terminal.observedAt,
			},
			"close",
		);
		if (descriptor.spawnId) {
			const expectation = this.ports.expectations?.read(descriptor.spawnId);
			if (expectation) {
				this.ports.expectations?.write(
					applyTerminalObservation(requestClose(expectation, closeIntent), {
						kind: "absent",
						observedAt: terminal.observedAt,
						evidence: "pane-missing",
					}),
				);
			}
		}
		this.ports.registry.dissolve(id);
		return ok({ warning });
	}

	/** Append one captured pi-activity event (monotonic seq + ISO timestamp). */
	capture(type: string, data?: unknown): void {
		const nowMs = this.ports.process.now();
		this.ports.eventLog.append(buildEvent(this.seq.next(), type, nowMs, data));
		// D-A: refresh the descriptor's age cursor so `pij state`/`list` read a
		// fresh lastEventAt without parsing events.ndjson (AC-9/7a).
		this.persist({ lastEventAt: new Date(nowMs).toISOString() });
	}

	/** Handle one inbound channel message: a receipt (record only), a remote
	 *  command (validate -> compact), or free text (frame + idle/steer inject +
	 *  emit a delivery receipt). Native consumers opt in to per-envelope runtime metadata. */
	onInbound(msg: PijMessage, messageId: string, trackConsumption = false): InboundResult {
		// A receipt acknowledges OUR earlier outbound — record it so the sender
		// sees it via tail/state, but NEVER inject it (don't wake/bill the peer).
		if (msg.kind === "receipt") {
			this.capture("receipt", {
				messageId,
				from: msg.from,
				body: msg.body,
				source: "extension",
			});
			return { kind: "receipt-recorded" };
		}

		// Remote command: only the allow-list reaches pi; unknown is rejected
		// before any pi call (finding 05). `compact` runs autonomously here;
		// `new`/`reload` need a command context, so they route through control()
		// and fall back to a pending queue (drained on the next `/pij`).
		if (msg.command !== undefined) {
			const v = validateCommand(msg.command);
			if (!v.ok) {
				this.capture("receipt", { messageId, command: msg.command, rejected: true, code: v.code });
				return { kind: "command-rejected", code: v.code };
			}
			if (!isControlCommand(v.value)) {
				const sidecar = this.ports.watchdog?.read(this.self);
				const paused = applyCompactPause(sidecar, this.ports.process.now());
				if (paused !== sidecar) this.ports.watchdog?.write(this.self, paused);
				const result = this.ports.pi.compact();
				this.captureControlCompletion(result, { messageId, command: v.value });
				return {
					kind: result instanceof Promise ? "command-started" : "command-executed",
					command: v.value,
				};
			}
			const result = this.ports.pi.control(v.value);
			if (result !== false) {
				this.captureControlCompletion(result, { messageId, command: v.value });
				return {
					kind: result instanceof Promise ? "command-started" : "command-executed",
					command: v.value,
				};
			}
			// Not armed: queue it, and wake the peer. NB the wake lands in the peer's
			// LLM, and an agent cannot run a slash command itself, so
			// the message must ask it to RELAY to its human operator (D-042).
			this.pendingControl.push(v.value);
			this.ports.pi.inject(
				`[pij] Peer ${msg.from} asked this session to /${v.value}, but pij is not armed yet. You cannot run a slash command yourself — please ask your human operator to run /pij once in this session to apply it (that also arms reload/new from peers for the rest of this session).`,
				this.ports.pi.isIdle() ? "immediate" : "steer",
			);
			this.capture("receipt", { messageId, command: v.value, deferred: true });
			return { kind: "command-deferred", command: v.value };
		}

		// Free text: classify from the peer's idle state, frame the sender id so a
		// reply needs no lookup (AC-5), inject, and emit the first receipt.
		const idle = this.ports.pi.isIdle();
		this.ports.pi.inject(
			frame(msg.from, msg.body),
			idle ? "immediate" : "steer",
			trackConsumption ? messageId : undefined,
		);
		const atIso = this.nowIso();
		const state = classifyOnInject(idle);
		const receipt = initialReceipt(messageId, this.self, msg.from, idle, atIso);
		this.emitReceipt(receipt);
		if (!idle) {
			this.pending.push({ injectIso: atIso, receipt });
		}
		return { kind: "delivered", state };
	}

	/** turn_start (ISO). Resolves any queued receipt whose steered message has
	 *  now been consumed by the live turn (finding 08). */
	onTurnStart(iso: string): void {
		const sidecar = this.ports.watchdog?.read(this.self);
		if (sidecar?.pausedBy === "compact") {
			this.ports.watchdog?.write(this.self, applyWorkingTransition(sidecar));
		}
		this.persist({ state: "working" }); // D-A: a live turn => working
		if (this.pending.length === 0) return;
		const still: PendingReceipt[] = [];
		for (const p of this.pending) {
			const at = correlateDeliveredAt(p.injectIso, true, [iso]);
			if (at === null) {
				still.push(p);
			} else {
				this.emitReceipt(markDelivered(p.receipt, at));
			}
		}
		this.pending = still;
	}

	/** turn_end: the session is back to idle (D-A) — a working->idle that the
	 *  CLI reads from the descriptor. Carries no receipt work. */
	onTurnEnd(): void {
		this.persist({ state: "idle" });
	}

	/** session_shutdown only dissolves replacement incarnations. A terminal quit
	 * stays observable until the daemon records evidence; it is never relabelled as
	 * a replacement. */
	shutdown(reason: SessionShutdownReason = "quit"): void {
		if (shutdownDecision(reason).kind === "dissolve-replacement") {
			this.ports.registry.dissolve(this.self);
		}
	}

	/** Drain queued new|reload requests now that a command context is armed
	 *  (called from the `/pij` command handler). Returns the commands applied. */
	applyPendingControl(): ControlCommand[] {
		if (this.pendingControl.length === 0) return [];
		const applied: ControlCommand[] = [];
		const still: ControlCommand[] = [];
		for (const c of this.pendingControl) {
			const result = this.ports.pi.control(c);
			if (result !== false) {
				applied.push(c);
				this.captureControlCompletion(result, { command: c, viaDrain: true });
			} else {
				still.push(c);
			}
		}
		this.pendingControl = still;
		return applied;
	}

	private captureControlCompletion(
		result: void | boolean | Promise<void> | Promise<boolean>,
		data: Record<string, unknown>,
	): void {
		if (result instanceof Promise) {
			void result.then(
				(executed) => this.capture("receipt", { ...data, executed: executed !== false }),
				(error: unknown) =>
					this.capture("receipt", {
						...data,
						rejected: true,
						reason: error instanceof Error ? error.message : String(error),
					}),
			);
		} else {
			this.capture("receipt", { ...data, executed: result !== false });
		}
	}

	/** Poll-primary delivery liveness (plan 057 thread-1). Called on every inbox
	 *  poll scan (drained or empty — an executed scan proves the loop is alive).
	 *  The accurate scan time is `atMs`; lastInboxScanAt is persisted only every
	 *  INBOX_SCAN_STAMP_CADENCE_MS to bound writes. The daemon's inbox-poll-stalled
	 *  anomaly reads the persisted copy. */
	noteInboxScan(atMs: number): void {
		if (!this.descriptor) return; // pre-boot: nothing to stamp, don't advance the marker
		if (atMs - this.lastPersistedInboxScanMs < PijSession.INBOX_SCAN_STAMP_CADENCE_MS) return;
		this.lastPersistedInboxScanMs = atMs;
		this.persist({ lastInboxScanAt: new Date(atMs).toISOString() });
	}

	// ─── internals ──────────────────────────────────────────────────────────
	/** Merge a patch into the live descriptor and persist it (D-A). No-op before
	 *  boot. */
	private persist(
		patch: Partial<
			Pick<
				SessionDescriptor,
				| "state"
				| "lastEventAt"
				| "lastInboxScanAt"
				| "pid"
				| "paneId"
				| "spawnedBy"
				| "boundModel"
				| "boundProvider"
				| "effort"
				| "spawnId"
			>
		>,
	): void {
		if (!this.descriptor) return;
		this.descriptor = { ...this.descriptor, ...patch };
		this.ports.registry.write(this.descriptor);
	}

	/** Record a receipt as an event AND send it back to its target as a
	 *  kind:receipt message (which the target records but never injects). */
	private emitReceipt(r: MessageReceipt): void {
		this.capture("receipt", { messageId: r.messageId, state: r.state, to: r.to });
		this.ports.delivery.deliver({
			from: this.self,
			to: r.to,
			body: receiptBody(r.messageId, r.state),
			kind: "receipt",
		});
	}

	private nowIso(): string {
		return new Date(this.ports.process.now()).toISOString();
	}
}
