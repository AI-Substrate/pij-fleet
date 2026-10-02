// pij-messaging — pure spawn-command builder + ready-body codec.
//
// Pi-free, child_process-free (AC-08). The argv/env surface the rest of
// the system deduces spawn behaviour from. No @earendil-works/* imports.
//
// Design notes:
//   - task rides PIJ_SPAWN_TASK env (never positional) to dodge the
//     announce-vs-initial-prompt race (finding 01 / CF-01).
//   - paneId? is an optional pass-through: Phase 2 resolves whether the
//     child ever reads PIJ_PANE_ID (e.g. for self-close) or if it is
//     spawner-only state. See PIJ_PANE_ID advisory in phase-1 dossier.

import { basename, isAbsolute, resolve } from "node:path";
import { normalizeModelQuery } from "./models/match.js";
import type { ModelEntry } from "./models/registry.js";
import { validateEffort, validateModel } from "./models/validate.js";
import type { RegistryPort } from "./ports.js";
import { persistDaemonWrite } from "./registry-write.js";
import type { HarnessKind, Role, SessionDescriptor, SessionId, SessionLifecycle } from "./types.js";
import { err, ok, type Result } from "./types.js";

// ─── Types ──────────────────────────────────────────────────────────────────

/** Explicit launch identity; an executable override never chooses a harness. */
export type SpawnHarness = HarnessKind | "omp";

export interface PiModelBinding {
	readonly model?: string;
	readonly provider?: string;
	readonly notice?: string;
}

/** Resolve a pi-family model before any pane or expectation mutation.
 *
 * Provider-qualified input is authoritative. A bare exact id is qualified when
 * its provider is unique. If the id exists under github-copilot and other
 * providers, github-copilot is the deliberate machine default and the caller
 * receives a notice. Every other multi-provider collision fails loud instead of
 * delegating an order-dependent choice to Pi's or OMP's fuzzy resolver. Unknown ids
 * preserve the existing warn-and-pass-through contract. */
export function resolvePiModelBinding(
	model: string | undefined,
	known: readonly ModelEntry[],
): Result<PiModelBinding> {
	if (model === undefined) return ok({});
	const providers = [...new Set(known.map((entry) => entry.provider))];
	const slash = model.indexOf("/");
	if (slash > 0) {
		const requestedProvider = model.slice(0, slash);
		const provider = providers.find(
			(candidate) => candidate.toLowerCase() === requestedProvider.toLowerCase(),
		);
		if (provider !== undefined) {
			return ok({ model: `${provider}/${model.slice(slash + 1)}`, provider });
		}
	}

	const normalized = normalizeModelQuery(model);
	const matchingProviders = [
		...new Set(
			known
				.filter((entry) => normalizeModelQuery(entry.id) === normalized)
				.map((entry) => entry.provider),
		),
	];
	if (matchingProviders.length === 0) return ok({ model });
	if (matchingProviders.includes("github-copilot")) {
		const notice =
			matchingProviders.length > 1
				? `notice: model '${model}' is available from ${matchingProviders.join(
						", ",
					)}; defaulting pi-family spawn to github-copilot (qualify --model to override)`
				: undefined;
		return ok({
			model: `github-copilot/${model}`,
			provider: "github-copilot",
			...(notice !== undefined ? { notice } : {}),
		});
	}
	const onlyProvider = matchingProviders[0];
	if (matchingProviders.length === 1 && onlyProvider !== undefined) {
		return ok({ model: `${onlyProvider}/${model}`, provider: onlyProvider });
	}
	return err(
		"E-AMBIGUOUS",
		`model '${model}' exists under multiple providers: ${matchingProviders
			.map((provider) => `${provider}/${model}`)
			.join(", ")}; qualify --model with the intended provider`,
	);
}

/** Input to buildSpawnCommand. */
export interface SpawnInput {
	/** Required runtime identity. No binary or environment default selects it. */
	harness: "pi" | "omp";
	/** Optional executable path; flag translation still follows harness. */
	bin?: string;
	/** Optional model override (passed as --model <model>). */
	model?: string;
	/** Resolved provider persisted separately from the model reference. */
	provider?: string;
	/** Optional thinking/reasoning effort (#3). Pi encodes it as a per-model
	 * `:<level>` suffix; OMP takes `--thinking <level>`. Unset emits neither. */
	effort?: string;
	/** Optional first task; delivered via PIJ_SPAWN_TASK env (finding 01). */
	task?: string;
	/** Explicit opaque plan identifier for the spawned seat. */
	planId?: string;
	/** Correlation token — becomes PIJ_SPAWN_ID. */
	spawnId: string;
	/** The spawner's pij id — becomes PIJ_ANNOUNCE_TO. */
	announceTo: string;
	/** Optional pane id pass-through — becomes PIJ_PANE_ID iff set.
	 *  Phase 2 resolves ownership; do not require it here. */
	paneId?: string;
	/** Absolute working directory for the new session. */
	cwd: string;
	/** Role the new session plays ("worker" for spawned children). */
	role: Role;
	/** Mark the child exempt in its watchdog sidecar on first boot. */
	noWatchdog?: boolean;
}

/** Output of buildSpawnCommand — passed to TmuxPort.newWindow. `cmd` is `"pi"`
 *  for a pi worker; the control-plane builder (buildControlSpawnCommand) widens
 *  it to `"claude"` (copilot later), hence the `string` type. */
export interface SpawnCommand {
	readonly cmd: string;
	readonly args: string[];
	readonly env: Record<string, string>;
}

/** The payload a spawned child sends back to the parent once ready. */
export interface ReadyPayload {
	readonly spawnId: string;
	readonly model: string;
	readonly effort?: string;
	readonly cwd: string;
}

// ─── Public API ─────────────────────────────────────────────────────────────

/**
 * Build the argv + env needed to launch a new pij worker via tmux.
 *
 * - `--model <model>` is emitted iff `input.model` is given.
 * - `PIJ_SPAWN_MODEL` is emitted in env iff `input.model` is given, so the
 *   child's `boot()` can include it in the ready-ping body (§H2).
 * - `task` is passed via `PIJ_SPAWN_TASK` env (not positional arg) to avoid
 *   the announce-vs-initial-prompt race (finding 01 / CF-01).
 * - `paneId` is an optional pass-through (Phase 2 resolves ownership).
 */
export function buildSpawnCommand(input: SpawnInput): SpawnCommand {
	const args: string[] = [];
	// Pi encodes effort in the model selector. OMP exposes a separate --thinking
	// flag and rejects a suffixed selector as an unknown model.
	const modelArg =
		input.model !== undefined && input.effort !== undefined && input.harness === "pi"
			? `${input.model}:${input.effort}`
			: input.model;
	// OMP has no human at the daemon-driven pane to answer approval prompts, so
	// mirror the other headless harnesses and skip approvals up front.
	if (input.harness === "omp") {
		args.push("--auto-approve");
	}
	if (modelArg !== undefined) {
		args.push("--model", modelArg);
	}
	if (input.harness === "omp" && input.effort !== undefined) {
		args.push("--thinking", input.effort);
	}

	const env: Record<string, string> = {
		PIJ_ANNOUNCE_TO: input.announceTo,
		// The spawner's pij-id, exposed under a stable identity name so the child
		// (and anything it shells) can read "who spawned me" without resolution
		// gymnastics. For pi this equals PIJ_ANNOUNCE_TO; PIJ_SESSION_ID (the
		// child's OWN id) is exported by the child at boot. Empty ⇒ caller unresolved.
		PIJ_PARENT_ID: input.announceTo,
		PIJ_SPAWN_ID: input.spawnId,
		PIJ_ROLE: input.role,
	};

	// Thread the effective model selector through the ready ping. OMP keeps the
	// model id unsuffixed and reports effort separately.
	if (modelArg !== undefined) {
		env.PIJ_SPAWN_MODEL = modelArg;
	}
	if (input.provider !== undefined) {
		env.PIJ_SPAWN_PROVIDER = input.provider;
	}
	if (input.effort !== undefined) {
		env.PIJ_SPAWN_EFFORT = input.effort;
	}

	if (input.task !== undefined) {
		env.PIJ_SPAWN_TASK = input.task;
	}
	if (input.planId !== undefined) {
		env.HARNESS_PLAN_ID = input.planId;
		env.PIJ_PLAN_ID = input.planId;
	}

	if (input.paneId !== undefined) {
		env.PIJ_PANE_ID = input.paneId;
	}
	if (input.noWatchdog === true) {
		env.PIJ_NO_WATCHDOG = "1";
	}

	return { cmd: input.bin ?? input.harness, args, env };
}

export interface PiFocusSpawnInput extends SpawnInput {
	readonly snapshotPath: string;
	readonly sessionDir: string;
	readonly forkSessionId: string;
}

/** Build a focus fork on pi's self-registering spawn path. The child owns its
 * pij id; only the native fork id and isolated transcript destination are pinned. */
export function buildPiFocusSpawnCommand(input: PiFocusSpawnInput): SpawnCommand {
	const base = buildSpawnCommand(input);
	return {
		...base,
		args: [
			"--fork",
			input.snapshotPath,
			"--session-dir",
			input.sessionDir,
			"--session-id",
			input.forkSessionId,
			...base.args,
		],
	};
}

/**
 * Build the ready-ping body string that a spawned child sends to the
 * parent once it is initialised. Encodes spawnId, model, and cwd.
 */
export function readyBody(spawnId: string, model: string, cwd: string, effort?: string): string {
	return JSON.stringify({
		spawnId,
		model,
		...(effort !== undefined ? { effort } : {}),
		cwd,
	});
}

/**
 * Parse a ready-ping body produced by readyBody(). Returns null if the
 * payload is malformed or missing required fields.
 */
export function parseReadyBody(body: string): ReadyPayload | null {
	try {
		const parsed = JSON.parse(body) as unknown;
		if (
			typeof parsed === "object" &&
			parsed !== null &&
			typeof (parsed as Record<string, unknown>).spawnId === "string" &&
			typeof (parsed as Record<string, unknown>).model === "string" &&
			typeof (parsed as Record<string, unknown>).cwd === "string"
		) {
			const p = parsed as Record<string, unknown>;
			return {
				spawnId: p.spawnId as string,
				model: p.model as string,
				...(typeof p.effort === "string" ? { effort: p.effort } : {}),
				cwd: p.cwd as string,
			};
		}
		return null;
	} catch {
		return null;
	}
}

// ─── Control plane (Plan 019) ────────────────────────────────────────────────
//
// A non-pi harness (Claude Code today) cannot derive its own pij-id at boot the
// way a child pi session does (it knows nothing about pij). So the daemon
// PRE-ALLOCATES the id before the pane exists (AC-01), threads it into the child
// env (PIJ_SESSION_ID), and writes a `pending` descriptor the daemon then drives
// to `ready` → `bound`. These builders are PURE — the impure split/write/inject
// orchestration lives in the daemon (Groups E/F).

/** Input to {@link buildControlSpawnCommand}. */
export interface ControlSpawnInput {
	/** Daemon-bound harness to launch. Pi uses {@link buildPiFocusSpawnCommand}. */
	readonly harness: Exclude<HarnessKind, "pi">;
	/** The registry-claimed pre-bind pij-id — rides PIJ_SESSION_ID so
	 *  the agent's `pij phonehome` self-resolves to the same id. */
	readonly pijId: SessionId;
	/** Absolute working directory for the new session. */
	readonly cwd: string;
	/** Optional model override (`claude --model <model>`). */
	readonly model?: string;
	/** Optional thinking/reasoning effort level (#3). Translated per harness:
	 *  claude/copilot → `--effort <level>`; codex → `-c model_reasoning_effort=<level>`
	 *  (codex has no `--effort` flag). Unset ⇒ nothing emitted (boot default). */
	readonly effort?: string;
	/** Optional first task — delivered later via the init inject, not argv. */
	readonly task?: string;
	/** Explicit opaque plan identifier for the spawned seat. */
	readonly planId?: string;
	/** The spawner's pij id — becomes PIJ_PARENT_ID so the child knows who
	 *  spawned it (uniform with the pi path's PIJ_PARENT_ID). Omitted from env
	 *  when the caller can't be resolved. */
	readonly parentId?: string;
	/** Copilot only: the pij-chosen session UUID, passed as `--session-id <uuid>`
	 *  so binding is deterministic at spawn (Copilot sets a new session's UUID). */
	readonly copilotSessionId?: string;
	/** Branch-from-self (claude, Plan 020): the SOURCE harness session id to fork —
	 *  emits `--resume <branchFrom> --fork-session`. Paired with {@link forkSessionId}.
	 *  Absent for a normal (non-branch) spawn. */
	readonly branchFrom?: string;
	/** Branch-from-self (claude): the pij-chosen NEW session id for the fork —
	 *  emits `--session-id <forkSessionId>` so the daemon binds deterministically
	 *  (the planned-id path, no transcript discovery). Set with `branchFrom`. */
	readonly forkSessionId?: string;
}

/** Input to {@link buildPendingDescriptor}. */
export interface PendingDescriptorInput {
	readonly pijId: SessionId;
	/** %N captured from `split-window -P` at spawn (finding 04). */
	readonly paneId: string;
	/** @M window id captured beside the pane (plan 054 P2 T006, AC-09) —
	 *  `tmux select-window -t <windowId>` opens the node's terminal. Absent
	 *  when the capture didn't yield one; the daemon backfills. */
	readonly windowId?: string;
	readonly cwd: string;
	readonly harness: HarnessKind;
	readonly dataDir: string;
	readonly eventsPath: string;
	readonly pid: number;
	/** ISO-8601 — the daemon's clock at spawn. */
	readonly startedAtIso: string;
	/** The spawner's pij-id (= PIJ_PARENT_ID). Records ownership so `pij close`
	 *  can distinguish a worker you spawned from one you don't own (a pi worker
	 *  self-registers this; claude/copilot get it written here at spawn). */
	readonly spawnedBy?: SessionId;
	/** Structural tree parent, independent from close ownership. */
	readonly parentId?: SessionId | null;
	/** Canonical git common directory resolved for the spawn cwd. */
	readonly gitCommonDir?: string;
	/** Transcript paths present at spawn (BEFORE the pane exists) — seeds
	 *  deterministic new-path discovery (AC-03 / review H1). Claude only. */
	readonly transcriptsAtSpawn?: readonly string[];
	/** Copilot only: the pij-chosen session UUID (`--session-id`) the daemon binds
	 *  to deterministically — no discovery. */
	readonly plannedHarnessSessionId?: string;
	/** Branch-from-self (Plan 020): the source harness session id this pane was
	 *  forked from. Observability only — the bind keys on `plannedHarnessSessionId`. */
	readonly branchedFrom?: string;
	/** Durable join to the pre-launch expectation keyed by PIJ_SPAWN_ID. */
	readonly spawnId?: string;
	/** Model pinned on the spawn command, persisted before first inference. */
	readonly model?: string;
	/** Resolved model provider; Copilot defaults to github-copilot. */
	readonly provider?: string;
	/** Reasoning effort pinned on the spawn command, persisted as registry truth. */
	readonly effort?: string;
	/** Explicit opaque plan identifier attested at spawn. */
	readonly planId?: string;
}

/** Stable input for the registry-owned pre-bind memorable-id reservation. */
export function spawnIdentitySeed(spawnToken: string, pid: number): string {
	return `spawn\0${spawnToken}\0${pid}`;
}

/**
 * Build argv + env to launch a non-pi harness in a tmux pane. The pre-allocated
 * pij-id rides PIJ_SESSION_ID (so `pij phonehome` from inside binds to it) and
 * the harness kind rides PIJ_HARNESS. argv stays an array (AC-09: no shell).
 *
 * - `copilot`: `copilot --yolo --session-id <uuid> [--model <model>]`
 *   and, when a model is pinned, `--context long_context` so the requested
 *   catalog window is selected rather than Copilot's smaller default tier.
 *
 * The blanket-permission flag (`--dangerously-skip-permissions` for claude,
 * `--yolo` for copilot — the latter = --allow-all-tools/paths/urls,
 * `--dangerously-bypass-approvals-and-sandbox` for codex) is REQUIRED
 * for a daemon-driven pane: there is no human at the pane to answer the harness's
 * permission/auto-mode prompts, so without it the agent hangs the instant it runs
 * a tool (e.g. the confirmatory `pij phonehome` bash call gets blocked). The pane
 * is a controlled peer we spawned, not an untrusted surface, so this is the right
 * trust posture. `--session-id <uuid>` SETS the copilot session UUID so the daemon
 * binds deterministically to it (no transcript-discovery race like claude).
 */
export function buildControlSpawnCommand(input: ControlSpawnInput): SpawnCommand {
	const args: string[] = [];
	if (input.harness === "claude") {
		args.push("--dangerously-skip-permissions");
		// Branch-from-self: fork the caller's own session into this pane. `--session-id`
		// pins the fork's id so the daemon binds deterministically (no discovery race).
		if (input.branchFrom !== undefined && input.forkSessionId !== undefined) {
			args.push(
				"--resume",
				input.branchFrom,
				"--fork-session",
				"--session-id",
				input.forkSessionId,
			);
		} else if (input.forkSessionId !== undefined) {
			// s071 D4 — a PLAIN claude spawn now pins its id too. `claude --session-id
			// <uuid>` is supported standalone (not just with --fork-session), so there
			// is no reason to leave the non-branched path on transcript discovery.
			//
			// Discovery was the root of the never-bind wedge: it identifies a session
			// by "the transcript path that wasn't there before", so two claude peers
			// booting into ONE folder produce two new paths and discovery is
			// permanently ambiguous. Pinning the id removes the race rather than
			// recovering from it.
			args.push("--session-id", input.forkSessionId);
		}
	} else if (input.harness === "copilot") {
		args.push("--yolo");
		if (input.copilotSessionId !== undefined) {
			args.push("--session-id", input.copilotSessionId);
		}
	} else if (input.harness === "codex") {
		// Codex's blanket-permission flag (Finding 05): skip all approval prompts +
		// run without the sandbox, so the daemon-driven pane never blocks on a tool.
		// Codex auto-generates its session UUID (F-01) — no `--session-id` to set, so
		// binding is transcript DISCOVERY (the claude path), not a deterministic id.
		args.push("--dangerously-bypass-approvals-and-sandbox");
	}
	if (input.model !== undefined) {
		args.push("--model", input.model);
	}
	if (input.harness === "copilot" && input.model !== undefined) {
		args.push("--context", "long_context");
	}
	// Effort translation (#3) — pinned per harness (flag drift is the key risk):
	//  • claude / copilot accept `--effort <level>` (verified in their --help).
	//  • codex has NO CLI effort flag — it takes a config override instead, so we
	//    emit `-c model_reasoning_effort=<level>` (discrete argv, AC-09: no shell).
	// Unset ⇒ emit nothing, so the harness uses its own boot default.
	if (input.effort !== undefined) {
		if (input.harness === "codex") {
			args.push("-c", `model_reasoning_effort=${input.effort}`);
		} else {
			args.push("--effort", input.effort);
		}
	}
	const env: Record<string, string> = {
		PIJ_SESSION_ID: input.pijId,
		PIJ_HARNESS: input.harness,
	};
	// The spawner's pij-id (when the caller resolved) — the control-plane analogue
	// of the pi path's PIJ_PARENT_ID. Self id is PIJ_SESSION_ID above; this is the
	// parent. Omitted (not set to "") when the caller couldn't be resolved.
	if (input.parentId) {
		env.PIJ_PARENT_ID = input.parentId;
	}
	if (input.task !== undefined) {
		env.PIJ_SPAWN_TASK = input.task;
	}
	if (input.planId !== undefined) {
		env.HARNESS_PLAN_ID = input.planId;
		env.PIJ_PLAN_ID = input.planId;
	}
	const cmd = input.harness === "claude" ? "claude" : input.harness;
	return { cmd, args, env };
}

/**
 * Build the `pending` descriptor written atomically at spawn, before any inject
 * (F2 / AC-01). The daemon's dir-watch keys on `lifecycle: "pending"` to start
 * readiness; `paneId` (from split `-P`) is how it addresses the pane.
 */
export function buildPendingDescriptor(input: PendingDescriptorInput): SessionDescriptor {
	return {
		id: input.pijId,
		folder: input.cwd,
		dataDir: input.dataDir,
		eventsPath: input.eventsPath,
		pid: input.pid,
		startedAt: input.startedAtIso,
		harness: input.harness,
		paneId: input.paneId,
		...(input.windowId !== undefined ? { windowId: input.windowId } : {}),
		lifecycle: "pending",
		// AC-04: the mechanical axis is `starting` from birth and HOLDS until
		// the first bind/readiness verdict (the daemon owns it from here).
		systemState: "starting",
		...(input.model !== undefined ? { boundModel: input.model } : {}),
		...(input.provider !== undefined
			? { boundProvider: input.provider }
			: input.harness === "copilot"
				? { boundProvider: "github-copilot" }
				: {}),
		...(input.effort !== undefined ? { effort: input.effort } : {}),
		...(input.planId !== undefined ? { planId: input.planId } : {}),
		...(input.spawnedBy ? { spawnedBy: input.spawnedBy } : {}),
		...(input.parentId !== undefined ? { parentId: input.parentId } : {}),
		...(input.gitCommonDir !== undefined ? { gitCommonDir: input.gitCommonDir } : {}),
		...(input.transcriptsAtSpawn ? { transcriptsAtSpawn: input.transcriptsAtSpawn } : {}),
		...(input.plannedHarnessSessionId
			? { plannedHarnessSessionId: input.plannedHarnessSessionId }
			: {}),
		...(input.branchedFrom ? { branchedFrom: input.branchedFrom } : {}),
		...(input.spawnId !== undefined ? { spawnId: input.spawnId } : {}),
	};
}

export interface SpawnOutputInput {
	readonly id?: SessionId;
	readonly paneId: string;
	readonly harness: HarnessKind;
	readonly lifecycle?: SessionLifecycle;
	readonly model?: string;
	readonly effort?: string;
	readonly planId?: string;
	readonly warnings?: readonly string[];
	readonly branchedFrom?: string;
	readonly note?: string;
}

/** Stable machine-readable spawn result shared by every harness. */
export function buildSpawnOutput(input: SpawnOutputInput): {
	readonly id: SessionId | null;
	readonly paneId: string;
	readonly harness: HarnessKind;
	readonly lifecycle: SessionLifecycle | null;
	readonly model: string | null;
	readonly effort: string | null;
	readonly planId: string | null;
	readonly warnings: readonly string[];
	readonly branchedFrom?: string;
	readonly note?: string;
} {
	return {
		id: input.id ?? null,
		paneId: input.paneId,
		harness: input.harness,
		lifecycle: input.lifecycle ?? null,
		model: input.model ?? null,
		effort: input.effort ?? null,
		planId: input.planId ?? null,
		warnings: input.warnings ?? [],
		...(input.branchedFrom !== undefined ? { branchedFrom: input.branchedFrom } : {}),
		...(input.note !== undefined ? { note: input.note } : {}),
	};
}

export function renderSpawnReceipt(
	output: ReturnType<typeof buildSpawnOutput>,
	humanLine: string,
	json: boolean,
): string {
	return json ? JSON.stringify(output) : [humanLine, ...output.warnings].join("\n");
}

/** A parsed `pij compact-self [--pane %N] [--delay-ms N] [instruction…]` request. */
export interface CompactSelfRequest {
	/** Target pane (`--pane`, else the caller's `$TMUX_PANE` default). */
	readonly pane?: string;
	/** Optional follow-up to TYPE after `/compact` — queued by the harness while it
	 *  compacts, so it runs as the first turn of the fresh context. */
	readonly instruction?: string;
	/** ms to wait after firing `/compact` before typing the instruction, so the
	 *  harness has entered compaction and queues the follow-up (not run pre-compact). */
	readonly delayMs: number;
}

/** Parse `compact-self` args (pure). Non-flag tokens join into the instruction
 *  (so an unquoted multi-word follow-up still works); `--pane`/`--delay-ms` take
 *  values in space or `=` form. Default delay 1500ms (~1–2s). */
export function parseCompactSelfArgs(
	argv: readonly string[],
	envPane?: string,
): CompactSelfRequest {
	let pane = envPane;
	let delayMs = 1500;
	const rest: string[] = [];
	for (let i = 0; i < argv.length; i++) {
		const tok = argv[i];
		if (tok === "--pane") pane = argv[++i];
		else if (tok?.startsWith("--pane=")) pane = tok.slice("--pane=".length);
		else if (tok === "--delay-ms") {
			const v = Number(argv[++i]);
			if (Number.isFinite(v) && v >= 0) delayMs = v;
		} else if (tok?.startsWith("--delay-ms=")) {
			const v = Number(tok.slice("--delay-ms=".length));
			if (Number.isFinite(v) && v >= 0) delayMs = v;
		} else if (tok !== undefined) {
			rest.push(tok);
		}
	}
	const instruction = rest.join(" ").trim();
	return { pane, delayMs, ...(instruction ? { instruction } : {}) };
}

/** Best-effort compact-self window mark (DL-004): stamp `compactingAt` on the
 *  resolved caller descriptor so the daemon HOLDS inbox drain while this pane
 *  compacts (a mid-compact injection is eaten by the harness's fresh-context
 *  reset). No id / unknown id → null and nothing written — an unregistered
 *  pane still compacts exactly as before. Persists via the daemon merge law
 *  (persistDaemonWrite) so a concurrent daemon tick write is never clobbered. */
export function markCompactingSelf(
	registry: RegistryPort,
	selfId: SessionId | undefined,
	nowIso: string,
): SessionDescriptor | null {
	if (selfId === undefined) return null;
	const descriptor = registry.read(selfId);
	if (!descriptor) return null;
	return persistDaemonWrite(registry, { ...descriptor, compactingAt: nowIso });
}

/** The side stack's column width as a % of the window (~1/3 — the orchestrator
 *  keeps ~2/3 on the left). One constant, shared by the first split (`-p`) and
 *  the post-append width restore (`resize-pane -x N%`). */
export const STACK_COLUMN_PERCENT = 33;

/** Where a control-plane spawn should split, mirroring pi's `layout:"split"`. */
export type ControlSplitPlan =
	| {
			readonly ok: true;
			readonly target: string;
			readonly direction: "h" | "v";
			readonly percent?: number;
			/** After the split, even out the pane's stack (`select-layout -E` on the
			 *  new pane — evens the heights of that vertical run). */
			readonly evenOut?: boolean;
			/** After evening, pin the stack column back to this % of the window width
			 *  (`resize-pane -x N%`) — `-E` can re-spread the root h-split too
			 *  (observed live on tmux 3.6a with a 2-pane column: 67/33 → 50/50). */
			readonly columnPercent?: number;
	  }
	| { readonly ok: false; readonly code: "E-FULL"; readonly message: string };

/** Decide where `pij spawn --harness …` splits — the DEFAULT side-stack layout,
 *  shared with pi's `pij_spawn` (core/session.ts): the first worker splits the
 *  orchestrator pane LEFT/RIGHT (`-h` → a right column ~1/3 wide); every later
 *  worker splits the newest peer pane UP/DOWN (`-v` → appended to the stack)
 *  and the column is then evened out (`evenOut` → `select-layout -E`, verified
 *  on tmux 3.6a to leave the main pane's width alone). UNCAPPED — the stack
 *  just gets shorter panes (the old main+2 cap survives only for the explicit
 *  `--layout right|below` one-shot placements). `peerPanes` is the set of LIVE
 *  peer panes already in the orchestrator's window, in registry order. */
export function planControlSplit(ownPane: string, peerPanes: readonly string[]): ControlSplitPlan {
	const last = peerPanes[peerPanes.length - 1];
	if (last === undefined) {
		return { ok: true, target: ownPane, direction: "h", percent: STACK_COLUMN_PERCENT };
	}
	return {
		ok: true,
		target: last,
		direction: "v",
		evenOut: true,
		columnPercent: STACK_COLUMN_PERCENT,
	};
}

/** The live peer panes sharing the orchestrator's window — the input to
 *  {@link planControlSplit}'s main+2 cap. A descriptor counts iff it has a `paneId`,
 *  it isn't the caller's own pane, and that pane is still ALIVE (present in
 *  `windowPanes`, which lists only live panes — a closed peer frees its slot).
 *  **Harness-agnostic (Plan 021)**: every spawned colleague — pi, claude, or copilot
 *  — counts toward the SAME cap, so a mixed fleet is consistent (a pi pane and a claude
 *  pane are both just panes). Deduped; insertion order preserved. Pure, so both the pi
 *  and the daemon-bound spawn paths share one definition (and one test). */
export function livePeerPanes(
	descriptors: readonly SessionDescriptor[],
	windowPanes: readonly string[],
	ownPane: string,
): string[] {
	const alive = new Set(windowPanes);
	return Array.from(
		new Set(
			descriptors
				.filter((d) => d.paneId && d.paneId !== ownPane && alive.has(d.paneId))
				.map((d) => d.paneId as string),
		),
	);
}

/** A parsed `pij spawn --harness <h> [--task …] [--model …] [--json]` request.
 *  Pure parse so the bin only owns the impure split/write (T018). */
export interface SpawnRequest {
	readonly harness: SpawnHarness;
	readonly task?: string;
	readonly model?: string;
	/** Explicit opaque plan identifier; never inferred from cwd or project metadata. */
	readonly planId?: string;
	/** Executable-path override only; never a harness selector. */
	readonly bin?: string;
	/** Optional thinking/reasoning effort level (#3). Translated per harness at
	 *  build time; unset ⇒ no flag (the colleague keeps its boot default). */
	readonly effort?: string;
	/** Branch-from-self (Plan 020): fork the CALLER's own session into the new pane
	 *  (`--branch`). Default false. Gated + resolved by the bin via {@link planBranch}. */
	readonly branch: boolean;
	/** Persist an exempt watchdog sidecar for the spawned peer. */
	readonly noWatchdog?: true;
	/** Explicit placement (FX001-3 / SUGG-001): stack | right | below | window.
	 *  Unset ⇒ `stack` (the default side stack — first peer opens a ~1/3 right
	 *  column, later peers append to it). */
	readonly layout?: SpawnLayout;
	readonly json: boolean;
}

/** Where an explicit `--layout` may place a spawned peer. `stack` names the
 *  default (side stack on the caller's right, uncapped). `headless` is
 *  deliberately absent — a daemon-bound harness needs a pty pane (deferred). */
export type SpawnLayout = "stack" | "right" | "below" | "window";

/** Daemon-bound harnesses: the daemon pre-allocates an id and drives boot→bound
 *  (transcript discovery for claude + codex, deterministic `--session-id` for
 *  copilot). codex (Plan 022) is the second discovery harness. */
const CONTROL_HARNESSES = new Set<HarnessKind>(["claude", "copilot", "codex"]);

/** Both extension runtimes self-register; the other harnesses bind through the daemon. */
const SPAWNABLE_HARNESSES = new Set<SpawnHarness>(["omp", "pi", "claude", "copilot", "codex"]);

/** AC-08 caller-truth parent derivation (plan 054 P3, issue #20): the parent
 *  of a spawned node is the INVOKING SESSION, resolved from identity ONLY —
 *  `PIJ_SESSION_ID` wins outright; else a pane id that matches exactly one
 *  descriptor across the FULL registry (panes are tmux-server-global, so an
 *  adopted peer invoking from a different worktree still resolves). cwd
 *  cohabitation NEVER makes a parent: the old lone-local inference silently
 *  parented children onto whichever single descriptor shared the launch
 *  folder — issue #20's live-reproduced tree corruption. Undefined means the
 *  caller is genuinely unresolved and the child spawns parentless (surfaced
 *  as `unadopted`, never guessed). Distinct from `resolveSelf`, whose job is
 *  "which session am I" for messaging — that contract (s051's zone) keeps
 *  its lone-local convenience; parent derivation must not. */
export function deriveCallerParent(
	envId: string | undefined,
	descriptors: readonly SessionDescriptor[],
	paneId: string | undefined,
): SessionId | undefined {
	if (envId !== undefined && envId.trim() !== "") return envId;
	if (paneId !== undefined && paneId.trim() !== "") {
		const byPane = descriptors.filter((d) => d.paneId === paneId);
		const only = byPane[0];
		if (byPane.length === 1 && only) return only.id;
	}
	return undefined;
}

/** Parse an explicit harness and optional executable path. Unknown flags,
 * missing values, mismatched executables and unsupported forks refuse before launch. */
export function parseSpawnArgs(argv: readonly string[]): Result<SpawnRequest> {
	let harness: SpawnHarness | undefined;
	let task: string | undefined;
	let model: string | undefined;
	let planId: string | undefined;
	let effort: string | undefined;
	let layout: SpawnLayout | undefined;
	let bin: string | undefined;
	let branch = false;
	let noWatchdog = false;
	let json = false;
	for (let i = 0; i < argv.length; i++) {
		const tok = argv[i];
		if (tok === "--json") {
			json = true;
			continue;
		}
		if (tok === "--branch") {
			branch = true;
			continue;
		}
		if (tok === "--no-watchdog") {
			noWatchdog = true;
			continue;
		}
		const eq = tok?.indexOf("=") ?? -1;
		const key = tok?.startsWith("--") ? tok.slice(2, eq === -1 ? undefined : eq) : undefined;
		if (!key) return err("E-ARG", `unexpected argument '${tok}' for spawn`);
		const inlineVal = eq === -1 ? undefined : tok?.slice(eq + 1);
		const value = inlineVal ?? argv[++i];
		if (value === undefined) return err("E-ARG", `--${key} needs a value`);
		if (key === "harness") {
			if (!SPAWNABLE_HARNESSES.has(value as SpawnHarness))
				return err("E-ARG", `--harness must be omp|pi|claude|copilot|codex (got '${value}')`);
			harness = value as SpawnHarness;
		} else if (key === "task") {
			task = value;
		} else if (key === "layout") {
			if (value !== "stack" && value !== "right" && value !== "below" && value !== "window")
				return err("E-ARG", `--layout must be stack|right|below|window (got '${value}')`);
			layout = value;
		} else if (key === "model") {
			model = value;
		} else if (key === "plan-id") {
			if (value.trim() === "") return err("E-ARG", "--plan-id needs a non-empty id");
			planId = value;
		} else if (key === "effort") {
			effort = value;
		} else if (key === "bin") {
			bin = value;
		} else {
			return err("E-ARG", `unknown flag --${key} for spawn`);
		}
	}
	if (!harness)
		return err(
			"E-ARG",
			"usage: pij spawn --harness omp|pi|claude|copilot|codex [--bin /path/to/executable] [--task …] [--model …] [--effort …] [--plan-id <id>]",
		);
	if (bin !== undefined) {
		const name = basename(bin);
		if (SPAWNABLE_HARNESSES.has(name as SpawnHarness) && name !== harness)
			return err("E-ARG", `--bin cannot select a harness; use --harness ${name} instead`);
		if (!isAbsolute(bin))
			return err(
				"E-ARG",
				"--bin requires an absolute executable path; select the runtime with --harness",
			);
	}
	if (harness === "omp" && branch)
		return err(
			"E-ARG",
			"--branch is unsupported for --harness omp: omp has no --session-id to pin the fork — spawn without --branch",
		);
	return ok({
		harness,
		task,
		model,
		planId,
		effort,
		layout,
		...(bin !== undefined ? { bin } : {}),
		branch,
		...(noWatchdog ? { noWatchdog: true as const } : {}),
		json,
	});
}

/** A placement plan: a split (ControlSplitPlan) or a fresh tmux window. */
export type PlacementPlan = ControlSplitPlan | { readonly ok: true; readonly window: true };

/** Resolve an explicit `--layout` (FX001-3 / SUGG-001) or fall back to the
 *  DEFAULT side stack ({@link planControlSplit} — `stack`, also the no-flag
 *  behaviour). `right`/`below` split the CALLER's own pane (one-shot placements,
 *  still subject to the main+2 cap); `window` opens a new tmux window in the
 *  caller's session — exempt from the cap. Pure, shared by `pij spawn` and
 *  `pij agent spawn`. */
export function planPlacement(
	layout: SpawnLayout | undefined,
	ownPane: string,
	peerPanes: readonly string[],
): PlacementPlan {
	if (layout === "window") return { ok: true, window: true };
	if (layout === "right" || layout === "below") {
		if (peerPanes.length >= 2) {
			return {
				ok: false,
				code: "E-FULL",
				message:
					"split layout full — 2 workers already in this window; close one or use the default stack (drop --layout) or --layout window",
			};
		}
		return layout === "right"
			? { ok: true, target: ownPane, direction: "h", percent: 40 }
			: { ok: true, target: ownPane, direction: "v" };
	}
	return planControlSplit(ownPane, peerPanes);
}

/**
 * Translate the `pij spawn --agent <slug> …` alias into the argv `pij agent spawn`
 * parses. Returns `null` when `--agent` is absent (a normal colleague spawn, which
 * `parseSpawnArgs` owns). The slug becomes the leading positional; every other
 * token (`-p k=v`, `--once`, `--model`, `--prompt`, …) is forwarded VERBATIM so the
 * two surfaces stay identical (a bad/missing slug then surfaces the same
 * `parseAgentArgs` error). Pure — the bin dispatches the result to the agent path.
 */
export function aliasAgentSpawnArgs(spawnArgs: readonly string[]): string[] | null {
	let hasAgent = false;
	let slug: string | undefined;
	const rest: string[] = [];
	for (let i = 0; i < spawnArgs.length; i++) {
		const tok = spawnArgs[i];
		if (tok === "--agent") {
			hasAgent = true;
			slug = spawnArgs[++i];
			continue;
		}
		if (tok?.startsWith("--agent=")) {
			hasAgent = true;
			slug = tok.slice("--agent=".length);
			continue;
		}
		if (tok !== undefined) rest.push(tok);
	}
	if (!hasAgent) return null;
	return ["spawn", ...(slug !== undefined ? [slug] : []), ...rest];
}

/** Result of planning a branch-from-self spawn (Plan 020). */
export interface BranchPlan {
	/** Source harness session id to fork (`--resume <from>`). */
	readonly from: string;
	/** The pij-chosen new session id for the fork (`--session-id <newSessionId>`). */
	readonly newSessionId: string;
}

/**
 * Decide whether a `--branch` (branch-from-self) spawn is allowed, given the
 * resolved CALLER descriptor (`self`) and a freshly-minted session id. PURE — the
 * impure self-resolution + uuid mint live in the bin. Branch-from-self forks the
 * caller's OWN session, so it requires: the caller resolves, the requested harness
 * supports branching, the caller runs the SAME harness, and the caller is already
 * BOUND (has a `harnessSessionId` to fork). Every reject is a specific, actionable
 * E-BRANCH. `supports` is injected ({@link supportsBranching}) for testability;
 * branching from ANOTHER peer is intentionally out of scope but not precluded —
 * a future caller passes that peer's descriptor as `self`.
 */
export function planBranch(
	reqHarness: HarnessKind,
	self: SessionDescriptor | null,
	supports: (h: HarnessKind) => boolean,
	newSessionId: string,
): Result<BranchPlan> {
	if (!supports(reqHarness)) {
		return err(
			"E-BRANCH",
			`--branch is not supported for ${reqHarness} yet (claude only) — spawn without --branch`,
		);
	}
	if (!self) {
		return err(
			"E-BRANCH",
			"cannot --branch: cannot resolve which session is calling. Set PIJ_SESSION_ID, " +
				'or run `pij adopt "$TMUX_PANE" --harness <h>` so this pane resolves itself',
		);
	}
	const selfHarness = self.harness ?? "pi";
	if (selfHarness !== reqHarness) {
		return err(
			"E-BRANCH",
			`--branch forks your OWN session, so the new harness must match yours: ` +
				`you are ${selfHarness}, spawning ${reqHarness}`,
		);
	}
	if (!self.harnessSessionId || self.harnessSessionId.trim() === "") {
		return err(
			"E-BRANCH",
			"cannot --branch: your session is not bound yet (no harness session id to fork) — " +
				"wait for binding, then retry",
		);
	}
	return ok({ from: self.harnessSessionId, newSessionId });
}

/** A parsed `pij adopt <pane> --harness <h> [--id <pij-id>]
 *  [--session-id <native-id>] [--json]` request. */
export interface AdoptRequest {
	readonly pane: string;
	readonly harness: HarnessKind;
	readonly id?: SessionId;
	/** Authoritative harness-native id for restart re-attachment (AC-15). */
	readonly sessionId?: string;
	/** Structural parent to persist independently from close ownership. */
	readonly parentId?: SessionId;
	readonly json: boolean;
}

/** Parse the `adopt` verb: a positional `<pane>` (%N) + `--harness` (required,
 *  a control harness) + optional `--id`, authoritative `--session-id`, + `--json`. */
export function parseAdoptArgs(argv: readonly string[]): Result<AdoptRequest> {
	let pane: string | undefined;
	let harness: HarnessKind | undefined;
	let id: SessionId | undefined;
	let sessionId: string | undefined;
	let parentId: SessionId | undefined;
	let json = false;
	for (let i = 0; i < argv.length; i++) {
		const tok = argv[i];
		if (tok === "--json") {
			json = true;
			continue;
		}
		if (tok && !tok.startsWith("--")) {
			if (pane !== undefined) return err("E-ARG", `unexpected extra argument '${tok}'`);
			pane = tok;
			continue;
		}
		const eq = tok?.indexOf("=") ?? -1;
		const key = tok?.startsWith("--") ? tok.slice(2, eq === -1 ? undefined : eq) : undefined;
		if (!key) return err("E-ARG", `unexpected argument '${tok}' for adopt`);
		const value = eq === -1 ? argv[++i] : tok?.slice(eq + 1);
		if (value === undefined) return err("E-ARG", `--${key} needs a value`);
		if (key === "harness") {
			if (value === "pi") {
				return err(
					"E-ARG",
					"Pi seats self-register at boot; you are already reachable — run `pij whoami` to confirm, never adopt",
				);
			}
			if (!CONTROL_HARNESSES.has(value as HarnessKind))
				return err("E-ARG", `--harness must be claude|copilot|codex (got '${value}')`);
			harness = value as HarnessKind;
		} else if (key === "id") {
			id = value;
		} else if (key === "session-id") {
			if (value.trim() === "" || value.startsWith("--")) {
				return err("E-ARG", "--session-id needs a non-empty native id value");
			}
			sessionId = value;
		} else if (key === "parent") {
			if (value.trim() === "" || value.startsWith("--")) {
				return err("E-ARG", "--parent needs a non-empty pij session id");
			}
			parentId = value;
		} else {
			return err("E-ARG", `unknown flag --${key} for adopt`);
		}
	}
	if (!pane || !/^%\d+$/.test(pane))
		return err(
			"E-ARG",
			"usage: pij adopt <pane:%N> --harness claude [--id <pij-id>] [--session-id <native-id>] [--parent <pij-id>]",
		);
	if (!harness) return err("E-ARG", "adopt needs --harness claude|copilot|codex");
	return ok({
		pane,
		harness,
		id,
		sessionId,
		...(parentId !== undefined ? { parentId } : {}),
		json,
	});
}

// ─── Spawn-time model validation (warn-don't-block, T006) ───────────────────

/**
 * Validate a spawn's --model against the known list. Returns a human-readable
 * warning string if the model is unknown, or null if it is known / not specified
 * / the known list is empty (cannot validate). NEVER blocks spawn — the caller
 * prints the warning and continues. The pij-id is always returned immediately.
 */
export function buildSpawnWarning(
	model: string | undefined,
	known: readonly ModelEntry[],
): string | null {
	if (!model) return null;
	const result = validateModel(model, known);
	if (result.ok) return null;
	// Only warn when the registry can POSITIVELY confirm absence — at least one
	// verified entry must exist. Best-effort alias lists (claude/codex, all
	// verified=false) cannot confirm absence, so stay silent (FIX-C / INS-007).
	if (!known.some((e) => e.verified)) return null;
	const suggestion = result.suggestion ? ` (did you mean '${result.suggestion}'?)` : "";
	return `warning: unknown model '${model}'${suggestion} — spawn continues; confirm the id is correct`;
}

export function buildPlanIdWarning(
	planId: string | undefined,
	cwd: string,
	isDirectory: (path: string) => boolean,
): string | null {
	if (planId === undefined) return null;
	if (
		planId.trim() === "" ||
		planId === "." ||
		planId === ".." ||
		planId.includes("/") ||
		planId.includes("\\")
	) {
		return `warning: plan id '${planId}' was not checked against docs/plans (not a simple path segment) — spawn continues`;
	}
	const planPath = resolve(cwd, "docs", "plans", planId);
	if (isDirectory(planPath)) return null;
	return `warning: plan id '${planId}' does not resolve to '${planPath}' — spawn continues`;
}

/**
 * Validate a spawn's `--effort` against the chosen model's known levels (#3,
 * task 2.6). Returns a human-readable warning when the registry can POSITIVELY
 * say the effort is unsupported for that model (listing the levels it does
 * support), else null (effort unset / model unknown / no level data — cannot
 * validate). NEVER blocks spawn — like {@link buildSpawnWarning}, the caller
 * prints this and continues; the effort is still passed to the harness.
 */
export function buildEffortWarning(
	effort: string | undefined,
	model: string | undefined,
	known: readonly ModelEntry[],
): string | null {
	if (!effort) return null;
	const r = validateEffort(effort, model, known);
	if (r.ok) return null;
	const supported = r.levels.length ? ` — ${model} supports: ${r.levels.join(", ")}` : "";
	return `warning: effort '${effort}' may be unsupported for '${model}'${supported}; spawn continues`;
}
