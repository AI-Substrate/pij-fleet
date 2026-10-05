import { basename } from "node:path";
import { StringEnum } from "@earendil-works/pi-ai";
import type {
	ExtensionAPI,
	ExtensionCommandContext,
	ExtensionContext,
} from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { type DaemonGeneration, detectDaemonGeneration } from "./adapters/daemon-http.js";
import { computeExtensionBuildIdentity } from "./adapters/extension-build.js";
import { GitRepositoryAdapter } from "./adapters/git-repository.js";
import type { CommandControl } from "./adapters/pi-runtime.js";
import {
	PiRuntimeAdapter,
	publishReloadCompleted,
	registerOmpReloadCompletion,
	reloadWithCompletion,
} from "./adapters/pi-runtime.js";
import { consumedMessageId, RustRuntimeSession } from "./adapters/rust-runtime.js";
import { TmuxAdapter } from "./adapters/tmux.js";
import { ALLOWED_COMMANDS } from "./core/commands.js";
import { detectPiRuntime, isSubagentChild } from "./core/discovery.js";
import { guardInvariantNineModal } from "./core/invariant-guard.js";
import { loadModels } from "./core/models/registry.js";
import {
	PIJ_STATUS_KEY,
	publishSeatName,
	type SessionNamePublisherDeps,
} from "./core/session-name.js";
import type { Role } from "./core/types.js";

// Capture the loaded source once; later HEAD/disk changes must not relabel this runtime.
const extensionIdentity = computeExtensionBuildIdentity(import.meta.dirname);

// pij — peer session messaging + observability.
//
// Thin pi-event -> coordinator translator (Patterns P2/P8/P10): owns NO logic.

// All boot/announce/capture/inject/command/receipt/shutdown behaviour lives in
// RustRuntimeSession (./adapters/rust-runtime.ts), which talks to the pij-rs
// daemon; this file only adapts runtime events into its calls. The single
// pi-API-importing seams are here + adapters/pi-runtime.ts; core/ stays free of it.

export default function (pi: ExtensionAPI): void {
	// A pi process spawned as a subagent child (pi-subagents: `pi --mode json -p`)
	// must NOT activate pij: the session_start announce triggers a model turn that
	// collides with the child's `-p` task prompt ("Agent is already processing"),
	// and a throwaway child should never register as a peer. Skip ALL wiring.
	if (isSubagentChild(process.env)) return;
	const runtimeBin = detectPiRuntime({
		ompCode: process.env.OMPCODE,
		bun: "bun" in process.versions,
		executableNames: [process.execPath, process.argv[0], process.argv[1]]
			.filter((value): value is string => value !== undefined)
			.map((value) => basename(value)),
	});

	const isEmbeddedOmpChild = (ctx: ExtensionContext & { readonly mode?: string }): boolean =>
		runtimeBin === "omp" &&
		ctx.mode === "print" &&
		isSubagentChild(
			{},
			{
				runtimeBin,
				mode: ctx.mode,
				entries: ctx.sessionManager.getEntries(),
			},
		);
	const requirePeerContext = (ctx: ExtensionContext): void => {
		if (isEmbeddedOmpChild(ctx)) {
			throw new Error(
				"E-NOREG: Pij is disabled in an OMP in-process subagent; return results through the parent task instead of using its peer identity.",
			);
		}
	};

	const repositories = new GitRepositoryAdapter();

	// Native send tool (the model-facing comms seam). Agents call this instead of
	// shelling out to the `pij` CLI — it sends through the booted Rust runtime, so
	// there is no second send logic. Registered at factory level so it is callable
	// before/independent of a turn.
	pi.registerTool({
		name: "pij_send",
		label: "pij send",
		description:
			"Send a message — or run an allow-listed control command (compact/new/reload) — to another pij peer session in this project. Prefer this over shelling out to the `pij` CLI: it resolves your id, delivers, and reports the receipt. Reply to a `[pij-rs from <id>] … [/pij]` message by passing that <id> as `to`. `fyi: true` holds a message until the recipient's next turn instead of waking them (receipt: held (fyi)); use it only when their next action doesn't depend on it (see the `fyi` parameter). A cold recipient (large context, idle past its prompt-cache TTL, not working) refuses a waking message with E-RS-COLD-WAKE naming the price; resend with `fyi: true` if their next action doesn't depend on it, or with `force: true` and a `reason` only when the wake is worth that price.",
		promptSnippet: "Message or control a peer pij session (reply to [pij-rs from <id>] … [/pij])",
		promptGuidelines: [
			"Use pij_send to reply to a `[pij-rs from <id>] … [/pij]` message or to message/control a peer — do not shell out to the `pij` CLI to send.",
			"Use `fyi: true` only when the recipient's next action doesn't depend on it. If they'd be stuck, wrong or waiting without it, it's a normal send. If they'd be fine not reading it until their next turn, it's `fyi: true`. If they'd be fine never reading it, don't send it. Always normal sends: work done or a phase complete, review verdicts, hand-offs, blockers, questions, decisions needed. FYI examples: \"merged #452, nothing needed from you\"; \"heads-up: main moved, rebase when you next touch it\"; progress notes nobody is waiting on.",
			"A cold seat refuses a waking pij_send with E-RS-COLD-WAKE and the price of waking it. Use `fyi: true` instead if their next action doesn't depend on it, or `force: true` with a `reason` only when the wake is worth that price; every forced wake is audited.",
		],
		parameters: Type.Object(
			{
				to: Type.String({
					description:
						"Target peer session id, e.g. pij-1gzyr0p, or seat@machine for a paired machine (the <id> from a `[pij-rs from <id>] … [/pij]` message, verbatim, or from `pij list --here`).",
				}),
				message: Type.Optional(
					Type.String({
						description:
							"Message text to deliver (appears to the peer as user input). Provide message OR command, not both.",
					}),
				),
				command: Type.Optional(
					StringEnum(ALLOWED_COMMANDS, {
						description:
							"Run an allow-listed control command on the peer instead of text: compact | new | reload. Provide message OR command, not both.",
					}),
				),
				fyi: Type.Optional(
					Type.Boolean({
						description:
							"true = hold this message until the recipient's next turn instead of waking them. Only when their next action doesn't depend on it: if they'd be stuck, wrong or waiting without it, send normally; if they'd be fine never reading it, don't send it. Never for work done, review verdicts, hand-offs, blockers, questions or decisions needed. Message only; never with `command` or `force`.",
					}),
				),
				force: Type.Optional(
					Type.Boolean({
						description:
							"true = wake a cold recipient anyway (the daemon otherwise refuses with E-RS-COLD-WAKE and the price). Needs `reason`. Message only; never with `fyi` or `command`.",
					}),
				),
				reason: Type.Optional(
					Type.String({
						pattern: "\\S",
						description:
							"Why this forced cold wake is worth its price; recorded in the daemon's audit. Required with `force`.",
					}),
				),
			},
			{
				// s099 / pij#166. The XOR was previously expressed ONLY in execute(),
				// so `{to}` and `{to, message, command}` were schema-VALID and the
				// model was permitted to emit them and told afterwards. Encoding it
				// here makes the invalid state unrepresentable rather than rejected.
				//
				// `oneOf` on the object (not a top-level union): the root must stay
				// `type: "object"` for the tool-calling wire format. See
				// docs/plans/099-send-tool-xor/assets/union-spike.md.
				oneOf: [
					{
						required: ["message"],
						not: {
							anyOf: [
								{ required: ["command"] },
								{
									required: ["fyi", "force"],
									properties: { fyi: { const: true }, force: { const: true } },
								},
							],
						},
						// Plan 157 phase 2: force carries a non-blank reason.
						anyOf: [
							{ not: { required: ["force"], properties: { force: { const: true } } } },
							{ required: ["reason"] },
						],
					},
					{
						required: ["command"],
						not: {
							anyOf: [
								{ required: ["message"] },
								{ required: ["fyi"], properties: { fyi: { const: true } } },
								{ required: ["force"], properties: { force: { const: true } } },
							],
						},
					},
				],
			},
		),
		async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
			requirePeerContext(ctx);
			const message = typeof params.message === "string" ? params.message.trim() : "";
			const command = typeof params.command === "string" ? params.command : undefined;
			if (message.length > 0 === (command !== undefined)) {
				throw new Error("pij_send needs exactly one of `message` or `command`.");
			}
			const fyi = params.fyi === true;
			if (fyi && command !== undefined) {
				throw new Error("pij_send `fyi` cannot be combined with a control `command`.");
			}
			const force = params.force === true;
			if (rustRuntime) {
				const receipt = await rustRuntime.send(params.to, message, {
					command,
					fyi,
					...(force ? { force: { reason: params.reason ?? "" } } : {}),
				});
				const coldCheck =
					receipt.coldCheck === undefined ? "" : ` (cold-check: ${receipt.coldCheck})`;
				const warning = receipt.warning === undefined ? "" : `\n${receipt.warning}`;
				return {
					content: [
						{
							type: "text",
							text: `${receipt.held ? "held (fyi)" : "queued"} ${receipt.msgId} -> ${params.to}${coldCheck}${warning}`,
						},
					],
					details: {},
				};
			}
			throw new Error(
				"pij_send: not registered with the pij-rs daemon; start it (`pij-rs daemon`) and /reload.",
			);
		},
	});

	// Per-session handles, (re)assigned on every session_start (all reasons).
	let session: RustRuntimeSession | undefined;
	let rustRuntime: RustRuntimeSession | undefined;
	let self = "";
	let role: Role | undefined;
	// Captured from pi's ExtensionCommandContext on each `/pij` run (the only
	// instant pi exposes newSession/reload). The receive watcher re-routes remote
	// new|reload onto this; undefined until armed / after a consuming op.
	let commandControl: CommandControl | undefined;
	let ompSessionNameGeneration = 0;
	let ompSessionNamePublication:
		| {
				deps: SessionNamePublisherDeps;
				self: string;
				daemonGeneration: "rs" | "legacy";
				state: {
					reasserted: boolean;
					took: boolean;
					noticeShown: boolean;
				};
				notifyRecovered(): void;
		  }
		| undefined;

	const startOmpSessionNamePublication = (
		ctx: ExtensionContext,
		name: string,
		daemonGeneration: "rs" | "legacy",
		publicationGeneration: number,
	): void => {
		if (publicationGeneration !== ompSessionNameGeneration) return;
		const state = { reasserted: false, took: false, noticeShown: false };
		const deps: SessionNamePublisherDeps = {
			setSessionName: async (sessionName) => {
				if (publicationGeneration !== ompSessionNameGeneration) return;
				pi.setSessionName(sessionName);
			},
			getSessionName: () =>
				publicationGeneration === ompSessionNameGeneration ? pi.getSessionName() : undefined,
			setStatus: (key, text) => {
				if (publicationGeneration === ompSessionNameGeneration) ctx.ui.setStatus(key, text);
			},
			notice: (text) => {
				if (publicationGeneration !== ompSessionNameGeneration || state.noticeShown) return;
				ctx.ui.notify(text, "warning");
				state.noticeShown = true;
			},
			sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
		};
		const publication = {
			deps,
			self: name,
			daemonGeneration,
			state,
			notifyRecovered: () => {
				if (publicationGeneration !== ompSessionNameGeneration) return;
				try {
					ctx.ui.notify(`pij: omp session name set after retry — id is ${name}`, "info");
				} catch {
					// Recovery remains successful when the transient notice surface is unavailable.
				}
			},
		};
		ompSessionNamePublication = publication;
		void publishSeatName(
			deps,
			name,
			daemonGeneration,
			undefined,
			extensionIdentity.extension_build,
		).then((result) => {
			if (ompSessionNamePublication === publication && result.took) state.took = true;
		});
	};

	// pij_spawn — open a new tmux window running a pij worker (T206).
	// Harness is explicit; cwd comes from the invoking runtime context.
	pi.registerTool({
		name: "pij_spawn",
		label: "pij spawn",
		description:
			"Spawn a pij worker in the explicitly selected harness: omp, pi, claude, copilot or codex. OMP and Pi workers use the side stack by default; layout:'window' opens a background tmux window. Other harnesses use native daemon windows. Requires tmux; retired harnesses are refused by machine policy.",
		promptSnippet: "Spawn a pij worker into the side stack (default) or a new tmux window",
		promptGuidelines: [
			"Always name harness explicitly; omp and pi are distinct runtimes, never binary switches or inferred defaults. For OMP use a provider-qualified model such as github-copilot/gpt-6-astra. OMP and Pi default to the right-hand side stack; layout:'window' opens a background window. Other harnesses use daemon windows. A ready-ping proves child boot; pane creation alone does not.",
		],
		parameters: Type.Object({
			harness: Type.Union(
				[
					Type.Literal("omp"),
					Type.Literal("pi"),
					Type.Literal("claude"),
					Type.Literal("copilot"),
					Type.Literal("codex"),
				],
				{ description: "Required harness identity; no default or inheritance from the caller." },
			),
			task: Type.Optional(
				Type.String({
					description:
						"Initial task injected into the child session as its first prompt (via PIJ_SPAWN_TASK env; avoids the announce-race).",
				}),
			),
			model: Type.Optional(
				Type.String({
					description: "Model override for the child session (passed as --model).",
				}),
			),
			effort: Type.Optional(
				Type.String({
					description: "Reasoning effort override for the child session.",
				}),
			),
			layout: Type.Optional(
				Type.Union([Type.Literal("window"), Type.Literal("split")], {
					description:
						"Where to place the worker: 'split' (the DEFAULT — omitting behaves the same) stacks it in a ~1/3-width column on the caller's right (uncapped; the stack evens itself); 'window' opens a new background tmux window instead.",
				}),
			),
		}),
		async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
			requirePeerContext(ctx);
			if (!session) throw new Error("pij_spawn: session not booted yet");
			if (!rustRuntime)
				throw new Error(
					"pij_spawn requires the Rust daemon; legacy spawn cannot enforce machine harness policy",
				);
			const res = await rustRuntime.spawn({
				harness: params.harness,
				task: typeof params.task === "string" ? params.task : undefined,
				model: typeof params.model === "string" ? params.model : undefined,
				effort: typeof params.effort === "string" ? params.effort : undefined,
				layout: params.layout === "window" || params.layout === "split" ? params.layout : undefined,
				cwd: ctx.cwd, // §M6: cwd from tool execute context
			});
			if (!res.ok) {
				throw new Error(`pij_spawn failed (${res.code}): ${res.message}`);
			}
			return {
				content: [
					{
						type: "text",
						text: `spawned pij worker — spawnId=${res.value.spawnId} paneId=${res.value.paneId}${res.value.notice ? `\n${res.value.notice}` : ""}`,
					},
				],
				details: {},
			};
		},
	});

	// pij_close — kill a spawned worker's tmux window + remove its descriptor (T206).
	pi.registerTool({
		name: "pij_close",
		label: "pij close",
		description:
			"Close a pij worker session: kills its tmux window and removes it from the peer registry.",
		promptSnippet: "Close a pij worker session",
		promptGuidelines: [
			// FT-005: pij_spawn returns spawnId+paneId, NOT the child SessionId.
			// The child id arrives as [pij-rs from <child-id>] … [/pij] or via pij list.
			"Use pij_close to terminate a spawned worker session. Pass the child session id from its ready-ping ([pij-rs from <child-id>] … [/pij]) or from pij list; do not pass the spawnId.",
		],
		parameters: Type.Object({
			to: Type.String({
				description: "Session id of the worker to close (e.g. pij-1abc2de).",
			}),
		}),
		async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
			requirePeerContext(ctx);
			if (!session) throw new Error("pij_close: session not booted yet");
			const res = session.close(params.to);
			if (!res.ok) {
				throw new Error(`pij_close failed (${res.code}): ${res.message}`);
			}
			// FT-002: surface AC-06 non-owner warning to the caller.
			const text = res.value.warning
				? `closed pij worker: ${params.to}\n⚠️ ${res.value.warning}`
				: `closed pij worker: ${params.to}`;
			return {
				content: [{ type: "text", text }],
				details: {},
			};
		},
	});

	// Pattern P10: ONE session_start handler for every reason
	// (startup/reload/new/resume/fork). Boot is idempotent — reload reuses the
	// descriptor (no duplicate, no replay) and refreshes the live ctx.
	pi.on("session_start", async (event, ctx: ExtensionContext) => {
		// Native OMP task sessions share process.env and TMUX_PANE with Main.
		// Exclude them before discovery, registration, announcements or inbox ownership.
		if (isEmbeddedOmpChild(ctx)) return;
		// Derive a stable self-id from pi's OWN session identity (changes on /new
		// and /fork, stable across /reload and /resume) so a /new session becomes a
		// new peer instead of reusing this process's id (D-041). Falls back to the
		// OS pid when pi does not surface a session id (SDK/test).
		const sessionNameGeneration = ++ompSessionNameGeneration;
		ompSessionNamePublication = undefined;
		let piSessionId: string | undefined;
		try {
			piSessionId = ctx.sessionManager?.getSessionId();
		} catch {
			piSessionId = undefined; // stale/unavailable session manager
		}
		const envRole = process.env.PIJ_ROLE;
		role = envRole === "parent" || envRole === "worker" ? envRole : undefined;
		const folder = process.cwd();
		const gitCommonDir = repositories.gitCommonDir(folder);
		let generation: DaemonGeneration;
		try {
			generation = await detectDaemonGeneration();
		} catch (error) {
			ctx.ui.setStatus(PIJ_STATUS_KEY, "daemon error");
			ctx.ui.notify(`pij: ${error instanceof Error ? error.message : String(error)}`, "error");
			throw error;
		}
		if (generation.kind !== "rust") {
			// Only an explicit PIJ_DAEMON_GENERATION=legacy reaches here; that generation is gone.
			const message =
				"pij: the legacy daemon generation was removed; unset PIJ_DAEMON_GENERATION and run pij-rs";
			ctx.ui.setStatus(PIJ_STATUS_KEY, "daemon error");
			ctx.ui.notify(message, "error");
			throw new Error(message);
		}
		if (!rustRuntime) {
			rustRuntime = new RustRuntimeSession(generation.client, new TmuxAdapter(), loadModels());
		} else {
			rustRuntime.setClient(generation.client);
		}
		rustRuntime.setPi(new PiRuntimeAdapter(pi, ctx, runtimeBin, () => commandControl));
		const boot = await rustRuntime.boot({
			...extensionIdentity,
			role,
			folder,
			dataDir: "",
			eventsPath: "",
			harness: "pi",
			harnessSessionId: piSessionId,
			piSessionId,
			actualModel: ctx.model === undefined ? null : `${ctx.model.provider}/${ctx.model.id}`,
			runtimeBin,
			paneId: process.env.TMUX_PANE,
			...(process.env.PIJ_PARENT_ID !== undefined ? { parentId: process.env.PIJ_PARENT_ID } : {}),
			...(gitCommonDir !== null ? { gitCommonDir } : {}),
			...(process.env.PIJ_PLAN_ID !== undefined ? { planId: process.env.PIJ_PLAN_ID } : {}),
			resetRuntimeState: event.reason !== "reload",
			notice: (text) => ctx.ui.notify(text, "info"),
			reason: event.reason,
		});
		session = rustRuntime;
		self = boot.id;
		role = boot.role === "parent" || boot.role === "worker" ? boot.role : undefined;
		if (runtimeBin === "omp")
			startOmpSessionNamePublication(ctx, self, "rs", sessionNameGeneration);
		else
			ctx.ui.setStatus(
				PIJ_STATUS_KEY,
				`${self} · rs-v1 · ext ${extensionIdentity.extension_build}`,
			);
		ctx.ui.notify(
			`pij: detected rs daemon at ${generation.location.addr}; registered ${self}`,
			"info",
		);
		if (event.reason === "reload") publishReloadCompleted(pi);
	});
	if (runtimeBin === "omp") registerOmpReloadCompletion(pi);

	// Event capture (registered once, top-level — reload-safe). Each pi event maps
	// to exactly one coordinator capture. There is no pi `usage` event.
	pi.on("tool_call", (event) => {
		session?.capture("tool_call", event);
		return guardInvariantNineModal(rustRuntime?.readSelf() ?? null, event.toolName);
	});
	pi.on("tool_result", (event) => {
		session?.capture("tool_result", event);
		rustRuntime?.onToolResult();
	});
	pi.on("session_before_compact", () => rustRuntime?.onBeforeCompact());
	pi.on("session_compact", () => rustRuntime?.onCompact());
	pi.on("message_end", (event) => session?.capture("message", event));
	// Forward the full envelope: prompt recovery carries its id in user content.
	pi.on("message_start", async (event, ctx) => {
		try {
			await rustRuntime?.onMessageStart(event.message);
		} finally {
			if (
				runtimeBin === "omp" &&
				ctx.hasUI &&
				event.message.role === "user" &&
				consumedMessageId(event.message) !== undefined
			) {
				// OMP 18.1.14 awaits extension handlers before EventController clears
				// external user prompts. Snapshot AFTER the ACK wait (not at enqueue),
				// then restore after that clear, never over intervening human input.
				const draft = ctx.ui.getEditorText();
				if (draft) {
					let humanInput = false;
					const unsubscribe = ctx.ui.onTerminalInput(() => {
						humanInput = true;
						return undefined;
					});
					setTimeout(() => {
						unsubscribe();
						if (!humanInput && ctx.ui.getEditorText() === "") {
							// OMP setEditorText delegates to setText, placing the cursor at EOF.
							ctx.ui.setEditorText(draft);
						}
					}, 0);
				}
			}
		}
	});
	pi.on("agent_end", (event) => {
		const willContinue = "willContinue" in event && event.willContinue === true;
		rustRuntime?.onAgentEnd(willContinue);
	});

	// Plan 158: a typed turn carries this seat's held FYIs. The daemon owns the
	// block format; it is passed through verbatim. A claim failure resolves to
	// undefined inside claimFyi, so it never blocks the human's turn.
	pi.on("before_agent_start", async (_event, ctx) => {
		if (isEmbeddedOmpChild(ctx)) return undefined;
		const block = await rustRuntime?.claimFyi(runtimeBin === "omp" ? "hook:omp" : "hook:pi");
		return block === undefined
			? undefined
			: { message: { customType: "pij-fyi", content: block, display: true } };
	});

	// Turn activity resets the Rust idle-loss detector, never its consumption proof.
	// Legacy queued receipts retain their turn-start handling.
	pi.on("turn_start", (event) => {
		const publication = ompSessionNamePublication;
		if (publication && !publication.state.took && !publication.state.reasserted) {
			publication.state.reasserted = true;
			void publishSeatName(
				{
					...publication.deps,
					// The primary retry window owns the single final-failure notice.
					notice: () => {},
				},
				publication.self,
				publication.daemonGeneration,
				[0],
				extensionIdentity.extension_build,
			).then((result) => {
				if (ompSessionNamePublication !== publication || !result.took) return;
				publication.state.took = true;
				if (publication.state.noticeShown) publication.notifyRecovered();
			});
		}
		session?.onTurnStart(new Date(event.timestamp).toISOString());
	});

	// turn_end flips the descriptor back to idle (D-A) so `pij state` reads
	// working/idle without parsing the stream.
	pi.on("turn_end", () => session?.onTurnEnd());

	pi.on("session_shutdown", async (event, ctx: ExtensionContext) => {
		if (isEmbeddedOmpChild(ctx)) return;
		commandControl = undefined;
		ompSessionNameGeneration += 1;
		ompSessionNamePublication = undefined;
		// Pi guarantees this closed union (extensions.md, session_shutdown): only
		// replacement reasons dissolve a predecessor; quit remains observable.
		// The Rust runtime settles a seat left working to idle, bounded, first.
		await session?.shutdown(event.reason);
		ctx.ui.setStatus(PIJ_STATUS_KEY, undefined);
	});

	pi.registerCommand("pij", {
		description: "pij peer messaging — self id, role, live peers, captured events",
		handler: async (_args: string, ctx: ExtensionCommandContext): Promise<void> => {
			if (isEmbeddedOmpChild(ctx)) {
				ctx.ui.notify(
					"pij: disabled in OMP in-process subagents; use the parent task channel",
					"info",
				);
				return;
			}
			if (!self || !rustRuntime) {
				ctx.ui.notify("pij: not booted yet", "info");
				return;
			}
			// Arm the command-control channel: this ctx is an ExtensionCommandContext,
			// the one place pi hands out newSession/reload. Capture it so the receive
			// watcher can fire remote new|reload, then drain anything queued while
			// un-armed.
			commandControl = {
				newSession: () => ctx.newSession(),
				reload: () => reloadWithCompletion(pi, () => ctx.reload()),
			};
			const applied = session?.applyPendingControl() ?? [];
			const peers = rustRuntime.peerCount();
			const appliedNote = applied.length > 0 ? ` · applied ${applied.join("+")}` : "";
			ctx.ui.notify(
				`pij: ${self} · role=${role ?? "peer"} · peers ${peers} · events ${rustRuntime.eventCount()}${appliedNote}`,
				"info",
			);
		},
	});
}
