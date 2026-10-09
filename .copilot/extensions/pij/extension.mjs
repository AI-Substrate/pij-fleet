import { execFile } from "node:child_process";
import { readlink } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { promisify } from "node:util";
import { joinSession } from "@github/copilot-sdk/extension";
import {
	chooseRegistration,
	createNativeReporter,
	DaemonClient,
	extensionBuildIdentity,
	FileJournal,
	HOLD_ESCALATED_RETRY_MS,
	HOLD_ESCALATION_MS,
	INITIAL_RETRY_MS,
	MAX_RETRY_MS,
	NativeBridge,
	NativeError,
	normalizeExecutable,
	parseProcessStart,
	resolveNativeHost,
	resolveRegistration,
} from "./store.mjs";

const execute = promisify(execFile);

async function inspectProcess(pid) {
	const options = { encoding: "utf8", env: { ...process.env, LC_ALL: "C" }, timeout: 3000 };
	const [identity, started] = await Promise.all([
		execute("ps", ["-p", String(pid), "-o", "pid=,ppid=,comm="], options),
		execute("ps", ["-p", String(pid), "-o", "lstart="], options),
	]);
	const row = /^\s*(\d+)\s+(\d+)\s+(.+?)\s*$/.exec(identity.stdout);
	if (!row) throw new NativeError("Cannot inspect Copilot ancestor process");
	const executable =
		process.platform === "linux"
			? normalizeExecutable(await readlink(`/proc/${pid}/exe`))
			: { command: row[3], replaced: false };
	return {
		pid: Number(row[1]),
		ppid: Number(row[2]),
		// Linux comm is a mutable task name (Copilot reports MainThread), not its executable.
		...executable,
		proc_start: parseProcessStart(started.stdout),
	};
}

function report(event) {
	// stdout belongs to the SDK parent-process JSON-RPC transport. Never log message bodies or keys.
	console.error(`[pij-native] ${JSON.stringify({ at: new Date().toISOString(), ...event })}`);
}

async function startExtension() {
	const controller = new AbortController();
	const signal = controller.signal;
	let bridge;
	let session;
	let unsubscribe;
	const nativeReport = createNativeReporter({
		log: (message, options) => session.log(message, options),
		capture: report,
	});
	const stop = (cause) => {
		if (signal.aborted) return;
		controller.abort();
		// A seat left working publishes idle (bounded, never rejects) before exit.
		const settled = bridge?.stop(cause);
		unsubscribe?.();
		// A disconnected parent may never answer detach; only this extension process is stopped.
		void Promise.all([
			settled,
			Promise.race([session?.disconnect().catch(() => undefined), sleep(1000)]),
		]).finally(() => process.exit(0));
	};
	for (const event of ["SIGTERM", "SIGINT", "SIGHUP", "disconnect"])
		process.once(event, () => stop(event));
	process.stdin.once("end", () => stop("stdin.end"));
	process.stdin.once("close", () => stop("stdin.close"));
	process.once("exit", (code) => bridge?.stop(`process.exit:${code}`));

	try {
		session = await joinSession({
			tools: [
				{
					name: "pij_send",
					description:
						"Send a message to a Pij peer from this native Copilot seat. A receipt proves queue/native acceptance, not completed model work. Call explicitly; replies are never automatically forwarded. `fyi: true` holds a message and rides along on the recipient's next turn instead of opening one. Use `fyi: true` only when the recipient's next action doesn't depend on it. If they'd be stuck, wrong or waiting without it, it's a normal send. If they'd be fine not reading it until their next turn, it's `fyi: true`. If they'd be fine never reading it, don't send it. Always normal sends: work done or a phase complete, review verdicts, hand-offs, blockers, questions, decisions needed. FYI examples: \"merged #452, nothing needed from you\"; \"heads-up: main moved, rebase when you next touch it\"; progress notes nobody is waiting on. A cold recipient (large context, idle past its prompt-cache TTL, not working) refuses a waking message with E-RS-COLD-WAKE naming the price; resend with fyi if their next action doesn't depend on it, or with force and a reason only when the wake is worth that price (audited).",
					parameters: {
						type: "object",
						properties: {
							to: {
								type: "string",
								description:
									"Recipient Pij seat ID, or seat@machine for a paired machine (copy a `[pij from …]` sender verbatim)",
							},
							message: { type: "string", description: "Message body" },
							fyi: {
								type: "boolean",
								description:
									"Hold until the recipient's next turn instead of opening one. Only when their next action doesn't depend on it: if they'd be stuck, wrong or waiting without it, send normally; if they'd be fine never reading it, don't send it. Never for work done, review verdicts, hand-offs, blockers, questions or decisions needed. Never with force",
							},
							force: {
								type: "boolean",
								description:
									"Wake a cold recipient anyway (otherwise refused with E-RS-COLD-WAKE and the price). Needs reason; never with fyi",
							},
							reason: {
								type: "string",
								pattern: "\\S",
								description:
									"Why this forced cold wake is worth its price; recorded in the daemon's audit. Required with force",
							},
						},
						required: ["to", "message"],
						additionalProperties: false,
					},
					handler: async (input, invocation) => {
						if (!session || invocation.sessionId !== session.sessionId || !bridge || signal.aborted)
							return { ok: false, error: "Pij native identity is not ready for this session" };
						return bridge.send(input);
					},
				},
			],
			hooks: {
				// Plan 158: a typed prompt carries this seat's held FYIs as context.
				onUserPromptSubmitted: async (input, invocation) => {
					if (
						!session ||
						!bridge ||
						signal.aborted ||
						invocation?.sessionId !== session.sessionId ||
						(input?.sessionId ?? session.sessionId) !== session.sessionId
					)
						return undefined;
					return bridge.claimFyis();
				},
			},
		});
		signal.throwIfAborted();
		if (!session.sessionId || session.sessionId !== process.env.SESSION_ID)
			throw new NativeError("Joined native session does not match extension parent context");
		unsubscribe = session.on("session.shutdown", () => stop("session.shutdown"));

		const pane = process.env.TMUX_PANE || undefined;
		let paneProcess;
		if (pane) {
			if (!/^%\d+$/.test(pane)) throw new NativeError("Malformed native TMUX_PANE");
			const result = await execute("tmux", ["display-message", "-p", "-t", pane, "#{pane_pid}"], {
				encoding: "utf8",
				timeout: 3000,
			});
			paneProcess = Number(result.stdout.trim());
		}
		const host = await resolveNativeHost({
			parentPid: process.ppid,
			inspectProcess,
			pane,
			paneProcess,
		});
		const stateDir = process.env.PIJ_RS_STATE_DIR ?? join(homedir(), ".pij-rs");
		if (!stateDir.trim()) throw new NativeError("PIJ_RS_STATE_DIR cannot be empty");
		const client = new DaemonClient({
			addr: process.env.PIJ_RS_ADDR ?? "127.0.0.1:7461",
			stateDir,
		});
		let retry = INITIAL_RETRY_MS;
		// Identity evidence only: an unreadable build never blocks registration.
		const extension = await extensionBuildIdentity(import.meta.dirname).catch(() => undefined);
		let failureEpisode = false;
		let holdStarted;
		while (!signal.aborted) {
			try {
				const roster = await client.request("/v1/seats?scope=local", undefined, signal);
				const registration = await resolveRegistration(
					client,
					chooseRegistration({
						sessionId: session.sessionId,
						host,
						folder: process.cwd(),
						seats: roster?.seats,
						env: process.env,
						extension,
					}),
					signal,
				);
				signal.throwIfAborted();
				if (holdStarted !== undefined) nativeReport({ kind: "connection-ready" });
				bridge = new NativeBridge({
					registration,
					native: session,
					client,
					journal: new FileJournal(stateDir, registration),
					report: nativeReport,
				});
				// One immutable native-session consumer. Held receiving leaves the explicit outgoing tool alive.
				await bridge.run();
				return;
			} catch (error) {
				if (signal.aborted) return;
				if (!error.retryable) throw error;
				if (error instanceof NativeError && error.holdKind === "native-session") {
					const now = performance.now();
					holdStarted ??= now;
					const elapsedMs = now - holdStarted;
					if (elapsedMs >= HOLD_ESCALATION_MS) retry = HOLD_ESCALATED_RETRY_MS;
					failureEpisode = false;
					nativeReport({
						kind: "registration-wait",
						holdKind: error.holdKind,
						elapsedMs,
						retryMs: retry,
					});
				} else {
					if (holdStarted !== undefined) retry = INITIAL_RETRY_MS;
					holdStarted = undefined;
					if (!failureEpisode)
						nativeReport({ kind: "registration-wait", diagnostic: error.message, retryMs: retry });
					failureEpisode = true;
				}
				await sleep(retry, undefined, { signal });
				retry = Math.min(retry * 2, MAX_RETRY_MS);
			}
		}
	} catch (error) {
		if (signal.aborted) return;
		if (session)
			await nativeReport({
				kind: "extension-unavailable",
				safeDiagnostic: error instanceof NativeError ? error.safeDiagnostic : undefined,
			});
		else
			console.error(
				"[pij native] unavailable: native-join; check Copilot native extension support. Ordinary Copilot remains usable; no fallback transport is enabled.",
			);
	}
}

void startExtension().catch(() =>
	report({
		kind: "extension-unavailable",
		action: "Native extension initialization failed; no fallback transport is enabled.",
	}),
);
