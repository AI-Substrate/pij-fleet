#!/usr/bin/env tsx
// Actual Copilot abort/model-picker receiver witness; inference alone is a local fixture.
import { strict as assert } from "node:assert";
import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
	cpSync,
	existsSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { NATIVE_RPC_DEADLINE_MS } from "../../.copilot/extensions/pij/store.mjs";
import { object } from "./copilot-native-fixture.js";
import {
	eventTail,
	type NativeSmokeContext,
	runMemorySmoke,
	seedHistory,
} from "./copilot-native-memory-smoke.js";
import {
	readNativeEvents,
	stopSmokeNativeHost,
	verifyNativeLifecycleIdentity,
} from "./copilot-native-smoke.js";

const ROOT = resolve(import.meta.dirname, "../..");
const EVIDENCE = join(ROOT, "scratch/evidence/receiver-progress");
const DEFAULT_PROGRESS_KEYS = { abort: "Escape", cancel: "C-c", submit: "Enter", next: "Down" };
const TIMEOUT_MS = 90_000;
const args = process.argv.slice(2);
const productionShape = args.includes("--production-shape");
const injectEmptyCursor = args.includes("--inject-empty-cursor");
const largeHistory = args.includes("--large-history");
const models = productionShape
	? ["gpt-6-astra", "gpt-5.6-sol-fast"]
	: ["pij-native-fixture", "pij-progress-alternate"];
const value = (flag: string) => {
	const index = args.indexOf(flag);
	return index < 0 ? undefined : args[index + 1];
};
const baselineRoot = value("--baseline-root");
const output = resolve(value("--output-dir") ?? join(EVIDENCE, `progress-${Date.now()}`));
assert(output.startsWith(`${EVIDENCE}/`), "evidence must stay under scratch/evidence");
const pijBin = resolve(value("--pij-bin") ?? join(ROOT, "target/debug/pij-rs"));
let releaseReply: (() => void) | undefined;
let fixtureBlocked = false;
const blockedReply = new Promise<void>((resolve) => {
	releaseReply = resolve;
});

async function scenario(context: NativeSmokeContext) {
	const { launch, api, rows, tmux, until, copilotHome, receipt, env, cwd, addr, state } = context;
	const probeDir = join(copilotHome, "extensions/receiver-probe");
	mkdirSync(probeDir, { recursive: true, mode: 0o700 });
	cpSync(
		join(ROOT, "harness/fixtures/copilot-receiver-probe.mjs"),
		join(probeDir, "extension.mjs"),
	);
	writeFileSync(
		join(copilotHome, "receiver-probe-provider.txt"),
		env.COPILOT_PROVIDER_BASE_URL as string,
		{ mode: 0o600 },
	);
	const providerPath = join(copilotHome, "providers.json");
	writeFileSync(
		providerPath,
		JSON.stringify({
			providers: [
				{
					name: "progress-local",
					type: "openai",
					wireApi: "completions",
					baseUrl: env.COPILOT_PROVIDER_BASE_URL,
				},
			],
			models: models.map((id) => ({ id, provider: "progress-local", wireModel: id, name: id })),
		}),
		{ mode: 0o600 },
	);
	tmux(["set-environment", "-g", "COPILOT_PROVIDERS_CONFIG", providerPath]);
	tmux(["set-environment", "-g", "COPILOT_MODEL", `progress-local/${models[0]}`]);
	if (injectEmptyCursor) {
		const subject = join(copilotHome, "extensions/pij");
		unlinkSync(subject); // This run's freshly created, doctor-verified private link only.
		cpSync(join(baselineRoot ?? ROOT, ".copilot/extensions/pij"), subject, { recursive: true });
		const entry = readFileSync(join(subject, "extension.mjs"), "utf8");
		assert.equal(
			entry.split("native: session,").length,
			2,
			"fixture must wrap exactly the native bridge boundary",
		);
		writeFileSync(
			join(subject, "extension.mjs"),
			`import { faultedSession } from "./receiver-empty-cursor.mjs";\n${entry.replace("native: session,", "native: faultedSession(session),")}`,
		);
		cpSync(
			join(ROOT, "harness/fixtures/copilot-receiver-empty-cursor.mjs"),
			join(subject, "receiver-empty-cursor.mjs"),
		);
		receipt.sdk_fault = {
			kind: "controlled H2 empty cursor plus callback loss; NOT a production mechanism reproduction",
			real_tail_and_backward: true,
			store_sha256: createHash("sha256")
				.update(readFileSync(join(subject, "store.mjs")))
				.digest("hex"),
		};
	}
	let target = await launch(
		undefined,
		"PIJ_PROGRESS_BOOT. Reply exactly PIJ_NATIVE_OBSERVED. Do not call tools.",
	);
	const historyPath = join(copilotHome, "session-state", target.seat.session, "events.jsonl");
	let historyEvents = () => readNativeEvents(historyPath);
	if (largeHistory) {
		await until("bootstrap completes before large-history seed", TIMEOUT_MS, () =>
			historyEvents().some((event) => event.type === "assistant.turn_end") ? true : undefined,
		);
		const bootstrap = target;
		const teardown: Record<string, unknown> = {};
		await stopSmokeNativeHost(
			{ tmux, probe: (pid, signal) => process.kill(pid, signal) },
			{
				previous: bootstrap.seat,
				launch: bootstrap.seat,
				socket: env.TMUX?.split(",")[0] as string,
			},
			TIMEOUT_MS,
			teardown,
		);
		const seed = seedHistory(historyPath, 12_500);
		receipt.large_history = seed;
		receipt.bootstrap = { ...bootstrap, teardown };
		historyEvents = eventTail(historyPath, seed.bytes);
		target = await launch(bootstrap.seat);
		verifyNativeLifecycleIdentity(bootstrap.seat, target.seat, "resume");
	}
	receipt.target = target;
	const sender = await launch();
	receipt.sender = sender;
	const probePrefix = join(copilotHome, `receiver-probe-${target.seat.session}`);
	const logDir = join(copilotHome, "logs/extensions");
	const extensionLog = () => {
		const name = readdirSync(logDir).find((name) => name.endsWith(`-${target.extension.pid}.log`));
		return name ? readFileSync(join(logDir, name), "utf8") : "";
	};
	const nativeLog = (allChildren = false) =>
		(allChildren
			? readdirSync(logDir)
					.filter((name) => name.startsWith("user-pij-"))
					.map((name) => readFileSync(join(logDir, name), "utf8"))
					.filter((text) => text.includes(`SESSION_ID=${target.seat.session}`))
					.join("\n")
			: extensionLog()
		)
			.split("\n")
			.flatMap((line) => {
				const start = line.indexOf("[pij-native] ");
				if (start < 0) return [];
				try {
					return [object(JSON.parse(line.slice(start + "[pij-native] ".length)))];
				} catch {
					return [];
				}
			});
	const capture = (label: string) => {
		writeFileSync(join(output, `${label}-extension.log`), extensionLog(), { mode: 0o600 });
		writeFileSync(
			join(output, `${label}-terminal.txt`),
			tmux(["capture-pane", "-p", "-J", "-S", "-100", "-t", target.seat.pane]),
			{ mode: 0o600 },
		);
	};
	const send = (marker: string) => {
		const msgId = randomUUID();
		const bodyFile = join(output, "runtime", `${msgId}.txt`);
		writeFileSync(bodyFile, `${marker}. Reply exactly PIJ_NATIVE_OBSERVED. Do not call tools.`, {
			mode: 0o600,
		});
		const result = spawnSync(
			pijBin,
			[
				"--json",
				"--addr",
				addr,
				"--state-dir",
				state,
				"send",
				"--from",
				sender.seat.id,
				"--to",
				target.seat.id,
				"--body-file",
				bodyFile,
				"--msg-id",
				msgId,
			],
			{
				env: {
					...env,
					PIJ_SESSION_ID: sender.seat.id,
					TMUX_PANE: sender.seat.pane,
					HARNESS_SESSION_ID: sender.seat.session,
				},
				cwd,
				encoding: "utf8",
				timeout: 15_000,
			},
		);
		assert.equal(result.status, 0, result.stderr);
		const envelope = object(JSON.parse(result.stdout));
		assert.equal(envelope.ok, true);
		return { msgId, envelope };
	};
	try {
		if (!largeHistory)
			await until("bootstrap turn completes", TIMEOUT_MS, () =>
				historyEvents().some((event) => event.type === "assistant.turn_end") ? true : undefined,
			);
		await until("real SDK registers alternate model", 15_000, () =>
			existsSync(`${probePrefix}.ready.json`) ? true : undefined,
		);
		receipt.probe = JSON.parse(readFileSync(`${probePrefix}.ready.json`, "utf8"));
		const first = send("PIJ_PROGRESS_ABORT");
		receipt.first = first;
		await until("native completion-wait while model request is blocked", TIMEOUT_MS, () =>
			fixtureBlocked &&
			nativeLog().some((row) => row.kind === "completion-wait" && row.msgId === first.msgId)
				? true
				: undefined,
		);
		if (injectEmptyCursor)
			writeFileSync(join(copilotHome, `receiver-fault-${target.seat.session}.arm`), "H2", {
				mode: 0o600,
			});
		capture("before-abort");
		console.log(
			JSON.stringify({
				phase: "before-abort",
				pane: target.seat.pane,
				socket: env.TMUX?.split(",")[0],
				host: target.host.pid,
				extension: target.extension.pid,
				output,
			}),
		);
		await delay(1000);
		const abortAt = Date.now();
		tmux(["send-keys", "-t", target.seat.pane, DEFAULT_PROGRESS_KEYS.abort]);
		await delay(1000);
		if (!productionShape) {
			if (!historyEvents().some((event) => event.type === "abort")) {
				receipt.abort_control = "Esc produced no abort; Ctrl-C fallback";
				tmux(["send-keys", "-t", target.seat.pane, DEFAULT_PROGRESS_KEYS.cancel]);
			} else receipt.abort_control = "Esc";
			await until("real abort event", TIMEOUT_MS, () =>
				historyEvents().some((event) => event.type === "abort") ? true : undefined,
			);
			releaseReply?.();
			await delay(500);
		} else receipt.abort_control = "Esc then picker during injected turn; no Ctrl-C";
		tmux(["send-keys", "-t", target.seat.pane, "-l", "/model"]);
		tmux(["send-keys", "-t", target.seat.pane, DEFAULT_PROGRESS_KEYS.submit]);
		await until("model picker displays alternate local model", 15_000, () =>
			tmux(["capture-pane", "-p", "-t", target.seat.pane]).includes(models[1] as string)
				? true
				: undefined,
		);
		capture("model-picker");
		tmux(["send-keys", "-t", target.seat.pane, "-l", models[1] as string]);
		tmux(["send-keys", "-t", target.seat.pane, DEFAULT_PROGRESS_KEYS.submit]);
		const changed = await until("actual model_picker model_change event", 15_000, () =>
			historyEvents().find(
				(event) =>
					event.type === "session.model_change" &&
					JSON.stringify(event.data).includes(models[1] as string),
			),
		);
		receipt.abort_model_change = {
			at: new Date(abortAt).toISOString(),
			elapsed_ms: Date.now() - abortAt,
			changed,
		};
		if (productionShape) {
			releaseReply?.();
			await delay(1500);
			receipt.picker_resubmission = historyEvents()
				.filter((event) => ["abort", "user.message", "session.model_change"].includes(event.type))
				.map(({ id, type, data }) => ({
					id,
					type,
					messageId: data.messageId,
					reason: data.reason,
					source: data.source,
				}));
		}
		const next = send("PIJ_PROGRESS_NEXT");
		receipt.next = next;
		let accepted = false;
		try {
			await until("next delivery native-accepted after abort/model change", 45_000, () =>
				nativeLog().find((row) => row.kind === "native-accepted" && row.msgId === next.msgId),
			);
			accepted = true;
		} catch (error) {
			receipt.next_wait_error = error instanceof Error ? error.message : String(error);
		}
		const events = historyEvents();
		receipt.observation = {
			next_accepted: accepted,
			events_after_abort: events
				.slice(events.findIndex((event) => event.type === "abort"))
				.map(({ id, type, data }) => ({ id, type, data })),
			receiver_log: nativeLog(),
			jobs: rows(
				"SELECT id,state,dedupe_key,outcome FROM jobs WHERE serial_key = ?",
				target.seat.id,
			),
			state: await api("/v1/state", { id: target.seat.id }),
		};
		capture("after-model-change");
		console.log(
			JSON.stringify({
				phase: baselineRoot ? "baseline" : "fixed",
				next_accepted: accepted,
				native_events: nativeLog().filter((row) => row.kind === "native-event").length,
				sdk_reads: nativeLog().filter((row) => row.kind === "receiver-read-settled").length,
				output,
			}),
		);
		if (!baselineRoot) assert(accepted, "next queued delivery must land after abort/model change");
		if (!baselineRoot) {
			if (injectEmptyCursor)
				assert(
					nativeLog().some((row) => row.kind === "receiver-rebaselined"),
					"controlled stale cursor must recover through the actual SDK",
				);
			await until("next native turn completes", 20_000, () =>
				nativeLog().find((row) => row.kind === "native-completed" && row.msgId === next.msgId),
			);
			await api("/v1/report", { seat: target.seat.id, argv: ["report", "state", "hold"] });
			const pending = send("PIJ_PROGRESS_MANUAL");
			receipt.pending = pending;
			const disconnectedAt = Date.now();
			writeFileSync(`${probePrefix}.command.json`, JSON.stringify({ op: "disable-pij" }), {
				mode: 0o600,
			});
			await until("host receives actual Pij SDK disable request", 15_000, () =>
				existsSync(`${probePrefix}.requested.json`) ? true : undefined,
			);
			await until("actual disabled native extension exits", 15_000, () => {
				try {
					process.kill(target.extension.pid, 0);
					return undefined;
				} catch (error) {
					if ((error as NodeJS.ErrnoException).code === "ESRCH") return true;
					throw error;
				}
			});
			receipt.sdk_disconnect = {
				...JSON.parse(readFileSync(`${probePrefix}.requested.json`, "utf8")),
				death: "verified owned extension PID ESRCH after host SDK disable",
			};
			const parked = await until(
				"unchanged receiver lease expires and queue parks",
				62_000,
				() =>
					rows(
						"SELECT seq,at,kind,seat,payload FROM spine_events WHERE kind = 'delivery.parked' AND json_extract(payload, '$.messageId') = ?",
						pending.msgId,
					)[0],
			);
			receipt.lease_expiry = { elapsed_ms: Date.now() - disconnectedAt, bound_ms: 60_000, parked };
			assert(
				Number(object(receipt.lease_expiry).elapsed_ms) <= 62_000,
				"expiry must stay within shipped lease plus observation scheduling",
			);
			const stateOutput = spawnSync(
				pijBin,
				["--addr", addr, "--state-dir", state, "state", target.seat.id],
				{ env, cwd, encoding: "utf8", timeout: 15_000 },
			);
			assert.equal(stateOutput.status, 0, stateOutput.stderr);
			assert.match(stateOutput.stdout, /native-extension-unavailable/);
			writeFileSync(join(output, "state.txt"), stateOutput.stdout);
			const shimState = spawnSync(
				join(ROOT, "node_modules/.bin/tsx"),
				[join(ROOT, ".omp/extensions/pij/cli.ts"), "state", target.seat.id],
				{ env, cwd, encoding: "utf8", timeout: 15_000 },
			);
			assert.equal(shimState.status, 0, shimState.stderr);
			assert.match(shimState.stdout, /native-extension-unavailable/);
			writeFileSync(join(output, "state-shim.txt"), shimState.stdout);
			receipt.state_after_disconnect = await api("/v1/state", { id: target.seat.id });
			assert.equal(
				object(receipt.state_after_disconnect).native_receiver_reason,
				"native-extension-unavailable",
			);
			await api("/v1/report", { seat: target.seat.id, argv: ["report", "state", "ready"] });
			const quote = (text: string) => `'${text.replaceAll("'", "'\\''")}'`;
			const pull = join(output, "emergency-inbox.json");
			const status = join(output, "emergency-inbox.status");
			const script = join(output, "runtime/manual-inbox.sh");
			writeFileSync(
				script,
				`#!/bin/sh\n${[pijBin, "--json", "--addr", addr, "--state-dir", state, "inbox"].map(quote).join(" ")} > ${quote(pull)} 2> ${quote(join(output, "emergency-inbox.stderr"))}\nprintf '%s\\n' "$?" > ${quote(status)}\n`,
				{ mode: 0o700 },
			);
			tmux(["send-keys", "-t", target.seat.pane, "-l", `! /bin/sh ${quote(script)}`]);
			tmux(["send-keys", "-t", target.seat.pane, DEFAULT_PROGRESS_KEYS.submit]);
			await until("actual Copilot shell pulls parked message", 20_000, () =>
				existsSync(status) ? true : undefined,
			);
			assert.equal(readFileSync(status, "utf8").trim(), "0");
			const envelope = object(JSON.parse(readFileSync(pull, "utf8")));
			assert(
				Array.isArray(envelope.data) &&
					envelope.data.some((row) => object(row).msg_id === pending.msgId),
			);
			const acknowledgements = rows(
				"SELECT recipient,msg_id,origin FROM delivered_messages WHERE recipient = ? AND msg_id = ?",
				target.seat.id,
				pending.msgId,
			);
			assert.equal(acknowledgements.length, 1, "manual inbox records one acknowledgement");
			receipt.manual_inbox = {
				envelope,
				acknowledgements,
				jobs: rows(
					"SELECT id,state,dedupe_key,outcome FROM jobs WHERE serial_key = ? AND dedupe_key = ?",
					target.seat.id,
					pending.msgId,
				),
			};
			// Model the scheduled child replacement with the real host SDK, not a forged heartbeat.
			assert.equal(
				object(await api("/v1/state", { id: target.seat.id })).native_receiver_reason,
				"native-extension-unavailable",
				"this SDK-enable probe starts with an expired receiver lease, not a healthy receiver",
			);
			const enableStarted = Date.now();
			writeFileSync(`${probePrefix}.command.json`, JSON.stringify({ op: "enable-pij" }), {
				mode: 0o600,
			});
			const registration = await until("replacement registers on the same host", TIMEOUT_MS, () =>
				nativeLog(true).find(
					(event) => event.kind === "registered" && Date.parse(String(event.at)) >= enableStarted,
				),
			);
			const registeredAt = Date.parse(String(registration.at));
			const replacementBudgetMs = NATIVE_RPC_DEADLINE_MS + 30_000;
			const replacement = await until(
				"registered replacement explicitly holds an unresponsive SDK",
				Math.max(0, registeredAt + replacementBudgetMs - Date.now()),
				async () => {
					const card = object(await api("/v1/state", { id: target.seat.id }));
					assert.equal(
						card.pid,
						target.host.pid,
						"receiver replacement must not replace the Copilot host",
					);
					const held = nativeLog(true).find(
						(event) =>
							event.kind === "receive-held" &&
							Date.parse(String(event.at)) >= registeredAt &&
							typeof event.safeDiagnostic === "string" &&
							event.safeDiagnostic ===
								`eventLog.read deadline exceeded (${NATIVE_RPC_DEADLINE_MS}ms)`,
					);
					return held ? { kind: "held" as const, card, held } : undefined;
				},
			);
			assert.equal(replacement.card.native_receiver_reason, "native-extension-unavailable");
			receipt.recycle = {
				kind: replacement.kind,
				registration,
				enable_started: new Date(enableStarted).toISOString(),
				registration_elapsed_ms: registeredAt - enableStarted,
				held_after_registration_ms: Date.parse(String(replacement.held.at)) - registeredAt,
				budget_ms: replacementBudgetMs,
				diagnostic: replacement.held,
				note: "Expired-lease SDK-enable probe: startup read held at its deadline; not a healthy-reload witness.",
			};
			console.log(JSON.stringify({ phase: "sdk-enable-held", ...object(receipt.recycle) }));
			receipt.recycle = {
				...object(receipt.recycle),
				host_pid: target.host.pid,
				jobs: rows(
					"SELECT id,state,dedupe_key,outcome FROM jobs WHERE serial_key = ?",
					target.seat.id,
				),
			};
			writeFileSync(
				join(output, "replacement-extension.log"),
				`${nativeLog(true)
					.map((event) => JSON.stringify(event))
					.join("\n")}\n`,
			);
		}
	} finally {
		releaseReply?.();
		capture("final");
	}
}

const result = await runMemorySmoke({
	pijBin,
	copilotBin: value("--copilot-bin") ?? "copilot",
	copilotRuntimeDir: value("--copilot-runtime-dir"),
	output,
	messages: 30,
	messageIntervalMs: 10_000,
	timeoutMs: TIMEOUT_MS,
	extensionRoot: baselineRoot,
	fixture: {
		beforeReply: async (body) => {
			const messages = Array.isArray(body.messages) ? body.messages.map(object) : [];
			let lastUser = messages.length - 1;
			while (lastUser >= 0 && messages[lastUser]?.role !== "user") lastUser--;
			const user = messages[lastUser];
			if (user && JSON.stringify(user).includes("PIJ_PROGRESS_ABORT") && !fixtureBlocked) {
				fixtureBlocked = true;
				console.log(JSON.stringify({ phase: "fixture-blocked", stream: body.stream }));
				await blockedReply;
			}
		},
	},
	scenario,
});
console.log(JSON.stringify(result));
process.exitCode = result.code;
