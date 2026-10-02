#!/usr/bin/env tsx
// Real client witness, never a replacement transport. Tmux keys simulate draft/one-shot consent only.
import { strict as assert } from "node:assert";
import { type ChildProcess, execFileSync, spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	realpathSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import {
	object,
	sendPrompt,
	smokeEnvironment,
	startFixtureProvider,
} from "./copilot-native-fixture.js";
import { manageCopilotExtension } from "./link-global.js";

interface Options {
	mode: "manual" | "spawned" | "both";
	provider: "local" | "real";
	workspaceTrust: "seeded" | "prompt";
	model?: string;
	pijBin: string;
	copilotBin: string;
	output?: string;
	timeoutMs: number;
}
interface Seat {
	id: string;
	harness: string;
	session: string;
	pane: string;
	proc: { pid: number; proc_start: number };
	spawn_id?: string;
	native_extension_delivery?: boolean;
}
interface NativeEvent {
	id: string;
	type: string;
	data: Record<string, unknown>;
}

interface TypingSample {
	jobs: Record<string, unknown>[];
	acknowledgements: Record<string, unknown>[];
	nativeEvents: NativeEvent[];
	deliveryEvents: Record<string, unknown>[];
}

const ROOT = resolve(import.meta.dirname, "../..");
const HUMAN_KEYS = {
	clearDraft: "C-u",
	approveOnce: "Enter",
	submitCommand: "Enter",
	newSession: "/new",
	exit: "/exit",
} as const;

export function parseSmokeArgs(args: string[]): Options {
	const options: Options = {
		mode: "both",
		provider: "local",
		workspaceTrust: "seeded",
		pijBin: join(ROOT, "target/debug/pij-rs"),
		copilotBin: "copilot",
		timeoutMs: 90_000,
	};
	for (let index = 0; index < args.length; index += 2) {
		const value = args[index + 1];
		if (!value) throw new Error(`missing value for ${args[index]}`);
		switch (args[index]) {
			case "--mode":
				assert(["manual", "spawned", "both"].includes(value), "--mode manual|spawned|both");
				options.mode = value as Options["mode"];
				break;
			case "--workspace-trust":
				assert(["seeded", "prompt"].includes(value), "--workspace-trust seeded|prompt");
				options.workspaceTrust = value as Options["workspaceTrust"];
				break;
			case "--provider":
				assert(["local", "real"].includes(value), "--provider local|real");
				options.provider = value as Options["provider"];
				break;
			case "--model":
				options.model = value;
				break;
			case "--pij-bin":
				options.pijBin = resolve(value);
				break;
			case "--copilot-bin":
				options.copilotBin = value;
				break;
			case "--output":
				options.output = resolve(value);
				break;
			case "--timeout-seconds":
				options.timeoutMs = Number(value) * 1000;
				assert(
					Number.isFinite(options.timeoutMs) &&
						options.timeoutMs > 0 &&
						options.timeoutMs <= 600_000,
					"timeout must be 1..600 seconds",
				);
				break;
			default:
				throw new Error(`unknown option ${args[index]}`);
		}
	}
	return options;
}

export function readNativeEvents(path: string): NativeEvent[] {
	if (!existsSync(path)) return [];
	const text = readFileSync(path, "utf8");
	// Only complete lines: the live CLI may currently be appending its final event.
	return text
		.slice(0, text.lastIndexOf("\n") + 1)
		.split("\n")
		.filter(Boolean)
		.map((line) => {
			const event = object(JSON.parse(line));
			assert.equal(typeof event.id, "string", "native event must have its real event id");
			assert.equal(typeof event.type, "string");
			return { id: event.id as string, type: event.type as string, data: object(event.data ?? {}) };
		});
}

export function nativeMessages(events: NativeEvent[], nonce: string): NativeEvent[] {
	return events.filter(
		(event) =>
			event.type === "user.message" &&
			typeof event.data.content === "string" &&
			event.data.content.includes(nonce),
	);
}

export function verifyOrdinaryUsability(
	events: NativeEvent[],
	nonce: string,
	terminal: string,
): void {
	const users = nativeMessages(events, nonce);
	assert.equal(users.length, 1, "ordinary native user nonce must occur once");
	const user = users[0];
	assert(
		user && typeof user.data.messageId === "string" && typeof user.data.interactionId === "string",
		"ordinary user needs actual native message/interaction ids",
	);
	assert(
		events.some(
			(event) =>
				event.type === "assistant.message" &&
				event.data.interactionId === user.data.interactionId &&
				typeof event.data.messageId === "string" &&
				event.data.content === "PIJ_NATIVE_OBSERVED",
		),
		"ordinary assistant must answer the same native interaction",
	);
	assert.equal(
		terminal.split("[pij native] unavailable:").length - 1,
		1,
		"one diagnostic per outage episode, not per retry",
	);
}

/** Compare full-driver startup runs, never two sessions sharing remembered trust. */
export function verifyWorkspaceTrustDiscriminator(
	seeded: Record<string, unknown>,
	prompt: Record<string, unknown>,
): void {
	const seededIsolation = object(seeded.isolation);
	const promptIsolation = object(prompt.isolation);
	assert(
		typeof seededIsolation.home === "string" &&
			typeof promptIsolation.home === "string" &&
			seededIsolation.home !== promptIsolation.home &&
			seeded.run !== prompt.run,
		"trust discriminator requires distinct private HOMEs and runs",
	);
	assert.equal(
		typeof seededIsolation.trust_config_before,
		"string",
		"seeded native config must be captured before launch",
	);
	assert.deepEqual(
		object(JSON.parse(seededIsolation.trust_config_before as string)).trustedFolders,
		[seededIsolation.workspace],
		"native config must trust only the isolated workspace",
	);
	assert.equal(
		promptIsolation.trust_config_before,
		null,
		"unseeded run must start without native config",
	);
	assert(Array.isArray(seeded.witnesses), "seeded run requires an ordinary-use witness");
	const ordinary = seeded.witnesses
		.map(object)
		.find((row) => row.kind === "daemon-key-absent-ordinary-use");
	assert(ordinary && Array.isArray(ordinary.events), "seeded run requires an ordinary-use witness");
	assert.equal(ordinary.daemon_started, false);
	assert.equal(ordinary.key_present, false);
	verifyOrdinaryUsability(
		ordinary.events as NativeEvent[],
		String(ordinary.nonce),
		String(ordinary.terminal),
	);
	assert.deepEqual(
		prompt.witnesses,
		[],
		"unseeded run must remain before ordinary use and daemon startup",
	);
	assert.deepEqual(prompt.fixture_requests, [], "unseeded run must have no provider activity");
	assert(Array.isArray(prompt.final_pane_captures), "unseeded run needs terminal evidence");
	const workspace = promptIsolation.workspace;
	assert(
		typeof workspace === "string" &&
			prompt.final_pane_captures
				.map(object)
				.some(
					(row) =>
						typeof row.terminal === "string" &&
						row.terminal.includes("Confirm folder trust") &&
						row.terminal.split("\n").some((line) => line.replaceAll("│", "").trim() === workspace),
				),
		"trust prompt must name the exact isolated workspace",
	);
}

function command(
	bin: string,
	args: string[],
	env?: NodeJS.ProcessEnv,
	preserveOutput = false,
): string {
	const output = execFileSync(bin, args, {
		encoding: "utf8",
		env,
		timeout: 15_000,
		stdio: ["ignore", "pipe", "pipe"],
	});
	return preserveOutput ? output : output.trim();
}
function quote(value: string): string {
	return `'${value.replaceAll("'", "'\\''")}'`;
}
function sha(path: string): string {
	return createHash("sha256").update(readFileSync(path)).digest("hex");
}
async function freePort(): Promise<number> {
	const server = createServer();
	await new Promise<void>((ok, fail) => {
		server.once("error", fail);
		server.listen(0, "127.0.0.1", ok);
	});
	const addr = server.address();
	assert(addr && typeof addr !== "string");
	await new Promise<void>((ok, fail) => server.close((error) => (error ? fail(error) : ok())));
	return addr.port;
}
async function until<T>(
	label: string,
	timeout: number,
	probe: () => T | undefined | Promise<T | undefined>,
): Promise<T> {
	const deadline = Date.now() + timeout;
	do {
		const result = await probe();
		if (result !== undefined) return result;
		await delay(250);
	} while (Date.now() < deadline);
	throw new Error(`PIJ_NATIVE_TIMEOUT: ${label}`);
}
async function stopOwned(child: ChildProcess): Promise<void> {
	if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
	const exited = new Promise<void>((ok) => child.once("exit", () => ok()));
	child.kill("SIGINT");
	const hardKill = setTimeout(() => child.kill("SIGKILL"), 5000);
	try {
		await exited;
	} finally {
		clearTimeout(hardKill);
	}
}

/** One tmux batch pairs real coordinates with an unjoined viewport, not joined scrollback. */
function captureSmokePaneViewport(
	tmux: (args: string[], preserveOutput?: boolean) => string,
	pane: string,
) {
	const captured_at_ms = Date.now();
	let terminal: string | undefined;
	let metadata_raw: string | undefined;
	try {
		const output = tmux(
			[
				"display-message",
				"-p",
				"-t",
				pane,
				"#{cursor_x}\t#{cursor_y}\t#{pane_width}\t#{pane_height}",
				";",
				"capture-pane",
				"-p",
				"-t",
				pane,
			],
			true,
		);
		const newline = output.indexOf("\n");
		metadata_raw = newline < 0 ? output : output.slice(0, newline);
		terminal = newline < 0 ? undefined : output.slice(newline + 1);
		assert(
			newline >= 0 && /^\d+\t\d+\t[1-9]\d*\t[1-9]\d*$/.test(metadata_raw),
			"PIJ_NATIVE_CAPTURE_CURSOR: missing or malformed tmux coordinates",
		);
		const coordinates = metadata_raw.split("\t").map(Number);
		assert(
			coordinates.every(Number.isSafeInteger),
			"PIJ_NATIVE_CAPTURE_CURSOR: unsafe tmux coordinates",
		);
		const [cursor_x, cursor_y, pane_width, pane_height] = coordinates;
		return {
			pane,
			captured_at_ms,
			terminal,
			metadata_raw,
			cursor_x,
			cursor_y,
			pane_width,
			pane_height,
		};
	} catch (error) {
		return { pane, captured_at_ms, terminal, metadata_raw, error: String(error) };
	}
}

/** Snapshot every owned pane before teardown; an exited pane must not hide the others. */
export function captureSmokePanes(tmux: (args: string[], preserveOutput?: boolean) => string) {
	const panes = tmux(["list-panes", "-a", "-F", "#{pane_id} #{pane_pid} #{pane_current_command}"]);
	const captures = panes
		.split("\n")
		.filter(Boolean)
		.map((row) => {
			const pane = row.split(" ")[0] as string;
			const pane_capture = captureSmokePaneViewport(tmux, pane);
			try {
				return {
					pane,
					terminal: tmux(["capture-pane", "-p", "-J", "-S", "-2000", "-t", pane]),
					pane_capture,
				};
			} catch (error) {
				return { pane, pane_capture, error: String(error) };
			}
		});
	return { panes, captures };
}

/** Later read-only diagnostics; neither success nor failure changes the witness outcome. */
export async function captureSmokeFinalTyping(
	io: {
		tmux: (args: string[], preserveOutput?: boolean) => string;
		seats: (signal: AbortSignal) => Promise<Seat[]>;
		get: (path: string, signal: AbortSignal) => Promise<unknown>;
	},
	timeoutMs = 5000,
) {
	const result = {
		started_at_ms: Date.now(),
		timeout_ms: timeoutMs,
		timing: "nearby-not-atomic",
		roster_request: "/v1/seats",
		observations: [] as Record<string, unknown>[],
	};
	try {
		// One HTTP budget covers the fresh roster and all exact-tuple requests.
		const signal = AbortSignal.timeout(timeoutMs);
		const panes = new Set(io.tmux(["list-panes", "-a", "-F", "#{pane_id}"]).split("\n"));
		const roster = await io.seats(signal);
		result.observations = await Promise.all(
			roster
				.filter(
					(seat) =>
						seat.harness === "copilot" &&
						seat.native_extension_delivery === true &&
						panes.has(seat.pane),
				)
				.map(async (seat) => {
					const sample: Record<string, unknown> = { seat, requested_at_ms: Date.now() };
					try {
						assert(
							typeof seat.id === "string" &&
								seat.id &&
								typeof seat.session === "string" &&
								seat.session &&
								Number.isSafeInteger(seat.proc?.pid) &&
								seat.proc.pid > 0 &&
								Number.isSafeInteger(seat.proc.proc_start) &&
								seat.proc.proc_start > 0,
							"invalid current native tuple",
						);
						const query = new URLSearchParams({
							seat: seat.id,
							native_session: seat.session,
							pid: String(seat.proc.pid),
							proc_start: String(seat.proc.proc_start),
						});
						const path = `/v1/inbox/typing?${query}`;
						sample.sensor_request = path;
						sample.sensor = await io.get(path, signal);
					} catch (error) {
						sample.error = `PIJ_NATIVE_FINAL_TYPING: ${String(error)}`;
					}
					sample.completed_at_ms = Date.now();
					// This is a subsequent physical viewport, not the daemon's earlier sensor frame.
					sample.pane_capture = captureSmokePaneViewport(io.tmux, seat.pane);
					return sample;
				}),
		);
		return { ...result, completed_at_ms: Date.now() };
	} catch (error) {
		return {
			...result,
			completed_at_ms: Date.now(),
			error: `PIJ_NATIVE_FINAL_TYPING: ${String(error)}`,
		};
	}
}

/** Capture before destroying the owned socket; only cleanup failure affects its result. */
export async function finishSmokePanes(
	tmux: (args: string[], preserveOutput?: boolean) => string,
	receipt: Record<string, unknown>,
	captureTyping?: () => Promise<unknown>,
): Promise<boolean> {
	try {
		if (captureTyping) receipt.final_typing_capture = await captureTyping();
		else receipt.final_typing_capture_skipped = "daemon-api-roster-not-initialized";
	} catch (error) {
		receipt.final_typing_capture_error = `PIJ_NATIVE_FINAL_TYPING: ${String(error)}`;
	}
	try {
		const final = captureSmokePanes(tmux);
		receipt.final_panes = final.panes;
		receipt.final_pane_captures = final.captures;
	} catch (error) {
		receipt.final_capture_error = String(error);
	}
	try {
		tmux(["kill-server"]);
		return true;
	} catch (error) {
		receipt.tmux_cleanup_error = String(error);
		return false;
	}
}

// Known Copilot welcome layout may finish painting before the composer below it.
const COPILOT_WELCOME_ROWS = [
	/^│\s*│$/,
	/^│\s*╭─╮╭─╮\s*│\s*Getting started\s*│$/,
	/^│\s*╰─╯╰─╯\s+Copilot v\d+\.\d+\.\d+(?:-\d+)? uses AI\.\s*│\s*Use the tabs above to explore your sessions and pull requests\s*│$/,
	/^│\s*█ ▘▝ █\s+\/experimental\s*│\s*\/init — Initialize Copilot instructions for this repository\s*│$/,
	/^│\s*▔▔▔▔\s*│\s*\/model — Switch models across providers, or use Auto\s*│$/,
	/^│\s*│$/,
];

/** False: no active box. Undefined: incomplete repaint. Array: complete bottom box body. */
function activeBottomModal(lines: string[]): string[] | false | undefined {
	let top = -1;
	for (let index = lines.length - 1; index >= 0; index--) {
		if (/^╭─+╮$/.test(lines[index] ?? "")) {
			top = index;
			break;
		}
	}
	const last = lines.at(-1) ?? "";
	if (top < 0) return /^[│╭╰]/.test(last) ? undefined : false;
	const bottom = lines.findIndex((line, index) => index > top && /^╰─+╯$/.test(line));
	if (bottom < 0) return undefined;
	// Welcome/transcript boxes above later UI are not the active control surface.
	if (bottom !== lines.length - 1) return /^[│╭╰]/.test(last) ? undefined : false;
	const body = lines.slice(top + 1, bottom);
	if (
		body.length === COPILOT_WELCOME_ROWS.length &&
		COPILOT_WELCOME_ROWS.every((row, index) => row.test(body[index] ?? ""))
	)
		return undefined;
	return body;
}

/** Match only the active bottom modal, never arguments echoed in earlier conversation. */
export function verifyToolApproval(
	terminal: string,
	expected: { to: string; message: string },
): boolean | undefined {
	const lines = terminal
		.trim()
		.split("\n")
		.map((line) => line.trim());
	const body = activeBottomModal(lines);
	if (body === false || body === undefined) return body;
	const fail = "PIJ_NATIVE_UNEXPECTED_APPROVAL";
	const modal = body.map((line) => {
		assert(/^│.*│$/.test(line), `${fail}: ambiguous modal boundary`);
		return line.slice(1, -1).trim();
	});
	assert.equal(modal[0], 'Run extension tool "pij_send"', `${fail}: unknown dialog title`);
	const start = modal.indexOf("{");
	const end = modal.indexOf("}", start + 1);
	assert(start > 0 && end > start, `${fail}: missing complete dialog arguments`);
	assert.equal(modal.lastIndexOf("{"), start, `${fail}: ambiguous dialog arguments`);
	assert.equal(modal.lastIndexOf("}"), end, `${fail}: ambiguous dialog arguments`);
	let args: unknown;
	try {
		args = JSON.parse(modal.slice(start, end + 1).join("\n"));
	} catch {
		throw new Error(`${fail}: malformed dialog arguments`);
	}
	assert.deepEqual(args, expected, `${fail}: dialog recipient/body mismatch`);
	assert.deepEqual(
		modal.filter((line) => /^(?:❯ )?\d+\. /.test(line)),
		[
			"❯ 1. Yes",
			'2. Yes, and approve "pij_send" for the rest of the session',
			"3. No",
			"4. No, and tell Copilot why... (Esc to stop)",
		],
		`${fail}: one-time Yes must be the sole selected choice`,
	);
	assert.equal(
		modal.at(-1),
		"↑/↓ to navigate · enter to select · esc to cancel",
		`${fail}: unknown controls`,
	);
	return true;
}

/** Observe existing preauthorization or confirm one exact dialog; never infer consent from absence. */
export async function confirmSmokeToolInvocation(
	tmux: (args: string[]) => string,
	expected: { pane: string; to: string; message: string },
	timeout: number,
	witness: Record<string, unknown>,
	preauthorized?: {
		launch: { seat: Seat; process: string };
		incomingEventId: string;
		observe: () => { nativeEvents: NativeEvent[]; jobs: Record<string, unknown>[] };
	},
): Promise<void> {
	const capture = () => tmux(["capture-pane", "-p", "-J", "-t", expected.pane]);
	const args = { to: expected.to, message: expected.message };
	Object.assign(witness, { kind: "native-tool-human-approval", expected, status: "waiting" });
	try {
		if (preauthorized) {
			const fail = "PIJ_NATIVE_PREAUTHORIZED_TOOL_PROOF";
			const { launch, incomingEventId } = preauthorized;
			Object.assign(witness, {
				kind: "native-tool-preauthorized-execution",
				launch_policy: launch,
				incoming_event_id: incomingEventId,
				decision: "observe-existing-preauthorization",
				keys: [],
			});
			assert.equal(launch.seat.pane, expected.pane, `${fail}: launch pane mismatch`);
			assert.equal(launch.seat.harness, "copilot", `${fail}: native Copilot launch required`);
			assert.equal(launch.seat.native_extension_delivery, true, `${fail}: native launch required`);
			assert.equal(
				Number(launch.process.trim().split(/\s+/, 1)[0]),
				launch.seat.proc.pid,
				`${fail}: launch PID mismatch`,
			);
			assert(
				/(?:^|\s)--yolo(?:\s|$)/.test(launch.process),
				`${fail}: existing launch policy unavailable`,
			);
			const proof = await until("exact preauthorized native pij_send execution", timeout, () => {
				const terminal = capture();
				witness.before = terminal;
				const approval = verifyToolApproval(terminal, args);
				assert(approval !== true, `${fail}: unexpected approval dialog under preauthorization`);
				if (approval !== false) return undefined;
				const sample = preauthorized.observe();
				witness.last_observation = sample;
				const session = sample.nativeEvents.find((event) => event.type === "session.start");
				assert.equal(
					session?.data.sessionId,
					launch.seat.session,
					`${fail}: native session mismatch`,
				);
				const incoming = sample.nativeEvents.findIndex(
					(event) => event.id === incomingEventId && event.type === "user.message",
				);
				if (incoming < 0) return undefined;
				const subsequent = sample.nativeEvents.slice(incoming + 1);
				const starts = subsequent.filter((event) => event.type === "tool.execution_start");
				assert(starts.length <= 1, `${fail}: duplicate tool execution`);
				const start = starts[0];
				if (!start) return undefined;
				let permission: NativeEvent | undefined;
				for (const event of sample.nativeEvents) {
					// A later grant cannot authorize an invocation that has already started.
					if (event === start) break;
					if (event.type === "session.permissions_changed") permission = event;
				}
				if (!permission) return undefined;
				assert.equal(
					permission.data.allowAllPermissions,
					true,
					`${fail}: native preauthorization disabled`,
				);
				assert.equal(
					permission.data.allowAllPermissionMode,
					"on",
					`${fail}: native preauthorization mode`,
				);
				assert(
					start.id && typeof start.data.toolCallId === "string" && start.data.toolCallId,
					`${fail}: native invocation ID required`,
				);
				assert.equal(start.data.toolName, "pij_send", `${fail}: wrong tool`);
				assert.deepEqual(start.data.arguments, args, `${fail}: tool recipient/body mismatch`);
				const completions = subsequent.filter((event) => event.type === "tool.execution_complete");
				assert(completions.length <= 1, `${fail}: duplicate tool completion`);
				const completed = completions[0];
				if (!completed) return undefined;
				assert(
					completed.id &&
						completed.id !== start.id &&
						subsequent.indexOf(completed) > subsequent.indexOf(start),
					`${fail}: native completion order/ID`,
				);
				assert.equal(
					completed.data.toolCallId,
					start.data.toolCallId,
					`${fail}: tool call mismatch`,
				);
				assert.equal(completed.data.success, true, `${fail}: native tool failed`);
				const content = object(completed.data.result).content;
				assert(typeof content === "string", `${fail}: missing tool result`);
				let result: Record<string, unknown>;
				try {
					result = object(JSON.parse(content));
				} catch {
					throw new Error(`${fail}: malformed tool result`);
				}
				assert.equal(result.ok, true, `${fail}: outgoing tool result failed`);
				const msgId = object(result.receipt).msg_id;
				assert(typeof msgId === "string" && msgId, `${fail}: outgoing receipt ID required`);
				const matches = sample.jobs.filter((row) => row.dedupe_key === msgId);
				assert(matches.length <= 1, `${fail}: duplicate durable outgoing`);
				const outbound = matches[0];
				if (!outbound) return undefined;
				assert(
					Number.isSafeInteger(outbound.id) && Number(outbound.id) > 0,
					`${fail}: durable job ID required`,
				);
				assert.equal(outbound.serial_key, expected.to, `${fail}: durable recipient mismatch`);
				assert.equal(
					outbound.kind,
					`delivery:${expected.to}`,
					`${fail}: durable job kind mismatch`,
				);
				const message = object(JSON.parse(String(outbound.payload)));
				assert.equal(message.from, launch.seat.id, `${fail}: durable sender mismatch`);
				assert.equal(message.to, expected.to, `${fail}: durable recipient mismatch`);
				assert.equal(message.body, expected.message, `${fail}: durable body mismatch`);
				assert.equal(message.msg_id, msgId, `${fail}: durable receipt mismatch`);
				const rechecked = capture();
				witness.before_complete = rechecked;
				const currentApproval = verifyToolApproval(rechecked, args);
				assert(currentApproval !== true, `${fail}: approval dialog appeared before completion`);
				return currentApproval === false ? { start, completed, outbound, permission } : undefined;
			});
			Object.assign(witness, {
				proof,
				status: "preauthorized-executed",
				grade: "native-tool-executed-not-human-approved-not-model-complete",
			});
			return;
		}
		await until("exact native pij_send approval dialog", timeout, () => {
			const terminal = capture();
			witness.before = terminal;
			if (verifyToolApproval(terminal, args) !== true) {
				witness.waiting_before = terminal;
				return undefined;
			}
			const rechecked = capture();
			witness.before_confirm = rechecked;
			return verifyToolApproval(rechecked, args) === true ? true : undefined;
		});
		Object.assign(witness, { decision: "approve-once", keys: [HUMAN_KEYS.approveOnce] });
		tmux(["send-keys", "-t", expected.pane, HUMAN_KEYS.approveOnce]);
		witness.status = "confirmation-sent";
		witness.confirmation_sent = true;
		const afterInput = capture();
		witness.after_input = afterInput;
		witness.after = afterInput;
		// Absence proves dismissal; an incomplete repaint still needs a bounded wait.
		if (verifyToolApproval(afterInput, args) !== false) {
			await until("one-time native tool approval dismissed", timeout, () => {
				const terminal = capture();
				witness.after = terminal;
				return verifyToolApproval(terminal, args) === false ? true : undefined;
			});
		}
		witness.status = "dismissed";
	} catch (error) {
		if (!witness.decision) witness.decision = "withhold";
		witness.status = "failed";
		witness.error = String(error);
		throw error;
	}
}

/** Read only the actual bottom composer; quoted transcript text is never a control surface. */
export function nativeComposer(terminal: string): string | undefined {
	const lines = terminal
		.trim()
		.split("\n")
		.map((line) => line.trim());
	const modal = activeBottomModal(lines);
	if (modal === undefined) return undefined;
	assert(modal === false, "PIJ_NATIVE_UNEXPECTED_LIFECYCLE_DIALOG");
	let top = -1;
	for (let index = lines.length - 1; index >= 0; index--) {
		if (/^╻▄+$/.test(lines[index] ?? "")) {
			top = index;
			break;
		}
	}
	if (top < 0) return undefined;
	const bottom = lines.findIndex((line, index) => index > top && /^╹▀+$/.test(line));
	if (bottom < 0) return undefined;
	const text = lines.slice(top + 1, bottom).map((line) => {
		assert(line.startsWith("┃"), "PIJ_NATIVE_AMBIGUOUS_COMPOSER");
		return line.slice(1).trim();
	});
	return text.join("\n");
}

/** Stage only a documented lifecycle command; an optional hook proves a queue hold before submit. */
export async function driveSmokeLifecycleCommand(
	tmux: (args: string[], preserveOutput?: boolean) => string,
	expected: { pane: string; action: "newSession" | "exit" },
	timeout: number,
	witness: Record<string, unknown>,
	whileStaged?: () => Promise<void>,
): Promise<void> {
	const capture = () => tmux(["capture-pane", "-p", "-J", "-t", expected.pane]);
	const command = HUMAN_KEYS[expected.action];
	const stagedCommand = (field: "staged" | "before_submit") => {
		const frame = captureSmokePaneViewport(tmux, expected.pane);
		witness[field] = frame.terminal;
		witness[`${field}_pane_capture`] = frame;
		if ("error" in frame) throw new Error(frame.error);
		const { terminal, cursor_x, cursor_y, pane_width, pane_height } = frame;
		assert(
			terminal !== undefined &&
				cursor_x !== undefined &&
				cursor_y !== undefined &&
				pane_width !== undefined &&
				pane_height !== undefined,
			"PIJ_NATIVE_LIFECYCLE_COMMAND_CHANGED: missing measured viewport",
		);
		const composer = nativeComposer(terminal);
		const ghostSuffix = command === "/new" ? " [prompt]" : " [print]";
		const ghost = composer === command + ghostSuffix;
		if (composer !== command && !ghost) return false;
		// Bind the existing parser's command to its single physical row, not transcript text.
		const lines = terminal.split("\n");
		let top = -1;
		for (let index = lines.length - 1; index >= 0; index--) {
			if (/^╻▄+$/.test(lines[index]?.trim() ?? "")) {
				top = index;
				break;
			}
		}
		const prefix = `┃ ${command}`;
		if (
			cursor_y !== top + 1 ||
			cursor_y >= pane_height ||
			cursor_x >= pane_width ||
			cursor_x !== prefix.length ||
			!/^╹▀+$/.test(lines[cursor_y + 1]?.trim() ?? "") ||
			lines[cursor_y]?.slice(0, cursor_x) !== prefix ||
			lines[cursor_y]?.slice(cursor_x).trimEnd() !== (ghost ? ghostSuffix : "")
		)
			return false;
		// Only the adjacent command menu can authorize a ghost; older transcript selections cannot.
		let end = top - 1;
		// The captured five-slot menu leaves four blank rows for the two /exit choices.
		while (end > top - 5 && lines[end]?.trim() === "") end--;
		let start = end;
		// A blank row (possibly with the transcript scrollbar) is a boundary; malformed text is not.
		while (start >= 0 && !/^\s*┃?\s*$/.test(lines[start] ?? "")) start--;
		const menu = lines.slice(start + 1, end + 1);
		const selected = menu.filter((line) => line.trimStart().startsWith("❯"));
		if (ghost || selected.length > 0 || menu.some((line) => /^ {2,}\//.test(line))) {
			return (
				menu.every(
					(line) =>
						/^ {2}(?:❯ | {2})\/[a-z][a-z-]* {2,}\S.*$/.test(line) ||
						(command === "/exit" && /^ {2}(?:❯ | {2})\/exit(?: print {2,}\S.*)?$/.test(line)),
				) &&
				selected.length === 1 &&
				/^ {2}❯ (\/[a-z][a-z-]*)(?: {2,}\S.*)?$/.exec(selected[0] ?? "")?.[1] === command
			);
		}
		return true;
	};
	Object.assign(witness, {
		kind: "native-lifecycle-human-control",
		...expected,
		command,
		status: "waiting",
	});
	try {
		await until("empty native lifecycle composer", timeout, () => {
			const terminal = capture();
			witness.before = terminal;
			const composer = nativeComposer(terminal);
			if (composer === undefined) return undefined;
			assert.equal(composer, "", "PIJ_NATIVE_HUMAN_DRAFT: refusing to overwrite composer");
			return true;
		});
		tmux(["send-keys", "-t", expected.pane, "-l", "--", command]);
		await until("exact native lifecycle command staged", timeout, () =>
			stagedCommand("staged") ? true : undefined,
		);
		if (whileStaged) await whileStaged();
		assert.equal(stagedCommand("before_submit"), true, "PIJ_NATIVE_LIFECYCLE_COMMAND_CHANGED");
		witness.submit_keys = [HUMAN_KEYS.submitCommand];
		tmux(["send-keys", "-t", expected.pane, HUMAN_KEYS.submitCommand]);
		witness.command_submitted = true;
		// /exit can remove the owned pane before capture; actual host death is proved separately.
		try {
			witness.after = capture();
		} catch (error) {
			witness.after_capture_error = String(error);
		}
		if (typeof witness.after === "string") nativeComposer(witness.after);
		witness.status = "submitted-not-yet-lifecycle-proven";
	} catch (error) {
		witness.status = "failed";
		witness.error = String(error);
		throw error;
	}
}

/** /exit can close only a conversation. Retire the owned pane, then prove real host death. */
export async function stopSmokeNativeHost(
	io: {
		tmux: (args: string[], preserveOutput?: boolean) => string;
		probe: (pid: number, signal: 0) => unknown;
	},
	expected: { previous: Seat; launch: Seat; socket: string },
	timeout: number,
	witness: Record<string, unknown>,
) {
	const { previous, launch, socket } = expected;
	const proc = { ...previous.proc };
	Object.assign(witness, {
		kind: "fixture-owned-native-host-teardown",
		ownership: { socket, launch },
		previous,
		status: "waiting",
	});
	const panes = (field: string) => {
		const raw = io.tmux(["list-panes", "-a", "-F", "#{pane_id}"]);
		witness[field] = raw;
		const ids = raw.split("\n").filter(Boolean);
		assert(
			ids.every((id) => /^%\d+$/.test(id)),
			"PIJ_NATIVE_HOST_OWNERSHIP: invalid pane list",
		);
		return ids;
	};
	const observe = () => {
		const observation = { ...proc, observed_at: new Date().toISOString() };
		try {
			io.probe(proc.pid, 0);
			witness.last_liveness = { ...observation, state: "alive" };
			return undefined;
		} catch (error) {
			witness.last_liveness = { ...observation, error: String(error), code: object(error).code };
			if (object(error).code !== "ESRCH") throw error;
			return { ...observation, evidence: "kill(pid, 0): ESRCH" };
		}
	};
	try {
		// These checks are brakes: removing one could only broaden destructive teardown.
		assert.equal(previous.harness, "copilot", "PIJ_NATIVE_HOST_OWNERSHIP: native host required");
		assert.equal(
			launch.native_extension_delivery,
			true,
			"PIJ_NATIVE_HOST_OWNERSHIP: launch required",
		);
		assert.equal(launch.harness, "copilot", "PIJ_NATIVE_HOST_OWNERSHIP: native launch required");
		assert(/^%\d+$/.test(previous.pane), "PIJ_NATIVE_HOST_OWNERSHIP: exact pane required");
		assert.equal(previous.pane, launch.pane, "PIJ_NATIVE_HOST_OWNERSHIP: pane differs from launch");
		assert.deepEqual(proc, launch.proc, "PIJ_NATIVE_HOST_OWNERSHIP: process differs from launch");
		assert(
			Number.isSafeInteger(proc.pid) &&
				proc.pid > 0 &&
				Number.isSafeInteger(proc.proc_start) &&
				proc.proc_start > 0,
			"PIJ_NATIVE_HOST_OWNERSHIP: real process tuple required",
		);
		const observedSocket = io.tmux(["display-message", "-p", "#{socket_path}"]);
		witness.observed_socket = observedSocket;
		assert(socket.length > 0, "PIJ_NATIVE_HOST_OWNERSHIP: private socket required");
		assert.equal(observedSocket, socket, "PIJ_NATIVE_HOST_OWNERSHIP: wrong socket");
		observe();
		witness.after_native_exit = witness.last_liveness;
		let teardownFailure: { error: unknown } | undefined;
		if (panes("panes_before").includes(previous.pane)) {
			witness.before_teardown_pane_capture = captureSmokePaneViewport(io.tmux, previous.pane);
			const args = ["kill-pane", "-t", previous.pane];
			witness.teardown_command = { socket, args };
			try {
				io.tmux(args);
				witness.decision = "closed-owned-pane";
			} catch (error) {
				witness.teardown_error = String(error);
				let remaining: string[];
				try {
					remaining = panes("panes_after_failed_teardown");
				} catch (captureError) {
					witness.teardown_recheck_error = String(captureError);
					throw error;
				}
				if (remaining.includes(previous.pane)) throw error;
				teardownFailure = { error };
				witness.decision = "pane-disappeared-during-teardown";
			}
		} else {
			witness.decision = "owned-pane-already-absent";
		}
		const death = await until("actual native host exits before resume", timeout, observe).catch(
			(error: unknown) => {
				witness.death_proof_error = String(error);
				throw teardownFailure ? teardownFailure.error : error;
			},
		);
		witness.host_death = death;
		witness.status = "host-death-proven-not-resume-proven";
		return death;
	} catch (error) {
		witness.status = "failed";
		witness.error = String(error);
		throw error;
	}
}

export function verifyNativeLifecycleIdentity(
	previous: Seat,
	current: Seat,
	action: "new" | "resume",
): void {
	assert.equal(current.harness, "copilot");
	assert.equal(current.native_extension_delivery, true);
	assert(/^[0-9a-f-]{36}$/i.test(current.session), "native lifecycle requires a real session UUID");
	assert(
		current.proc.pid > 0 && current.proc.proc_start > 0,
		"native lifecycle requires a real host tuple",
	);
	if (action === "new") {
		assert.notEqual(current.session, previous.session, "/new must change native conversation");
		assert.notEqual(current.id, previous.id, "/new must not retain old-context Pij address");
		assert.equal(current.pane, previous.pane, "/new remains in the actual existing pane");
		assert.deepEqual(current.proc, previous.proc, "/new remains in the actual existing host");
	} else {
		assert.equal(current.session, previous.session, "--resume must preserve native conversation");
		assert.equal(
			current.id,
			previous.id,
			"--resume must preserve Pij address after proven owner death",
		);
		assert.notDeepEqual(
			current.proc,
			previous.proc,
			"--resume must use a fresh real host incarnation",
		);
	}
}

export function verifyNativeContextHeld(
	expected: { seat: string; session: string; nonce: string; msgId: string },
	snapshot: {
		jobs: Record<string, unknown>[];
		acknowledgements: Record<string, unknown>[];
		predecessorEvents: NativeEvent[];
		successorEvents: NativeEvent[];
		successorJobs: Record<string, unknown>[];
	},
): void {
	assert.equal(snapshot.jobs.length, 1, "old-context job must remain durable");
	const job = snapshot.jobs[0];
	assert(
		job && ["pending", "running"].includes(String(job.state)),
		"old-context job must remain live",
	);
	const payload = object(JSON.parse(String(job.payload)));
	assert.equal(payload.to, expected.seat, "old-context recipient must not migrate");
	assert.equal(
		payload.native_target_session,
		expected.session,
		"old-context native stamp must survive",
	);
	assert.equal(payload.msg_id, expected.msgId);
	assert.equal(payload.body, expected.nonce);
	assert.equal(snapshot.acknowledgements.length, 0, "old-context job must not be acknowledged");
	assert.equal(
		nativeMessages(snapshot.predecessorEvents, expected.nonce).length,
		0,
		"old consumer must not drain held work",
	);
	assert.equal(
		nativeMessages(snapshot.successorEvents, expected.nonce).length,
		0,
		"old context must not enter successor conversation",
	);
	assert.equal(
		snapshot.successorJobs.length,
		0,
		"old-context work must not migrate to successor queue",
	);
}

/** 136/137: native delivery bypasses typing, not semantic Hold, consent or native-target isolation. */
export async function driveSmokeTypingWitness(
	io: {
		tmux: (args: string[], preserveOutput?: boolean) => string;
		observe: () => Promise<TypingSample>;
		send: () => Promise<unknown>;
	},
	expected: { seat: Seat; draft: string; nonce: string; msgId: string },
	timeout: number,
	witness: Record<string, unknown>,
): Promise<void> {
	const { seat, draft, nonce, msgId } = expected;
	const capture = () => io.tmux(["capture-pane", "-p", "-J", "-t", seat.pane]);
	function check(condition: unknown, message: string): asserts condition {
		assert(condition, `PIJ_NATIVE_TYPING_PROOF: ${message}`);
	}
	const keys: string[][] = [];
	Object.assign(witness, { seat, draft, nonce, msgId, status: "running", keys });
	const type = (...input: string[]) => {
		const command = ["send-keys", "-t", seat.pane, ...input];
		keys.push(command);
		io.tmux(command);
	};
	const visible = (text: string) =>
		until("exact owned draft visible", timeout, () => {
			const terminal = capture();
			witness.staged = terminal;
			return nativeComposer(terminal) === text ? terminal : undefined;
		});
	const sample = async () => {
		const value = await io.observe();
		const pane_capture = captureSmokePaneViewport(io.tmux, seat.pane);
		witness.last_observation = { ...value, pane_capture };
		const terminal = capture();
		witness.after = terminal;
		check(nativeComposer(terminal) === draft, "same nonempty composer must remain intact");
		check(
			nativeMessages(value.nativeEvents, draft).length === 0,
			"human draft must not be submitted",
		);
		const native = nativeMessages(value.nativeEvents, nonce);
		check(
			native.length <= 1 && value.acknowledgements.length <= 1,
			"duplicate native acceptance or ACK",
		);
		const holds = value.deliveryEvents.filter((row) => {
			if (row.kind !== "delivery.held" || row.seat !== seat.id) return false;
			const payload = object(JSON.parse(String(row.payload)));
			return payload.msg_id === msgId && payload.reason === "human-typing";
		});
		check(holds.length === 0, "native delivery must not hold for human typing");
		return { ...value, pane_capture, terminal, native };
	};
	try {
		check(
			/^HUMAN_DRAFT_[a-zA-Z0-9-]+$/.test(draft),
			"only the fixture human draft is a typing control",
		);
		witness.before = capture();
		check(nativeComposer(String(witness.before)) === "", "refusing to overwrite existing composer");
		type("-l", "--", draft);
		witness.draft_before = await visible(draft);
		const initial = await sample();
		witness.typing_observed = initial;
		check(
			initial.native.length === 0 &&
				initial.acknowledgements.length === 0 &&
				initial.jobs.length === 0,
			"message identity must be unused before sending",
		);
		const started = Date.now();
		const deliveryBudget = Math.min(timeout, 5000);
		witness.send = await io.send();
		const accepted = await until(
			"native acceptance while actively editing an intact draft",
			deliveryBudget,
			async () => {
				// Real composer edits keep the witness active; no daemon typing sensor or release control.
				check(nativeComposer(capture()) === draft, "refusing to edit changed composer");
				type("-l", "--", "x");
				await visible(`${draft}x`);
				type("BSpace");
				await visible(draft);
				const value = await sample();
				const elapsed = Date.now() - started;
				witness.delivered_within_ms = elapsed;
				check(elapsed <= deliveryBudget, "native delivery exceeded one sweep (5 seconds)");
				if (!value.native.length || !value.acknowledgements.length) return undefined;
				const native = value.native[0];
				check(
					typeof native?.id === "string" &&
						native.id.length > 0 &&
						typeof native.data.messageId === "string" &&
						native.data.messageId.length > 0,
					"real native event and message ids required",
				);
				const ack = value.acknowledgements[0];
				check(
					ack?.recipient === seat.id && ack.msg_id === msgId && ack.origin === "reader-read",
					"wrong native ACK identity or origin",
				);
				check(
					value.jobs.length === 1 && value.jobs[0]?.state === "done",
					"one completed delivery job required",
				);
				const job = value.jobs[0] as Record<string, unknown>;
				check(Number.isSafeInteger(job.id) && Number(job.id) > 0, "real durable job id required");
				const payload = object(JSON.parse(String(job.payload)));
				check(
					payload.to === seat.id &&
						payload.msg_id === msgId &&
						payload.body === nonce &&
						payload.native_target_session === seat.session,
					"job identity or native context changed",
				);
				const ackEvents = value.deliveryEvents
					.filter((row) => row.kind === "delivery.inbox-ack" && row.seat === seat.id)
					.map((row) => ({ ...row, payload: object(JSON.parse(String(row.payload))) }))
					.filter((row) => row.payload.job_id === job.id && row.payload.outcome === "reader-read");
				if (!ackEvents.length) return undefined;
				check(ackEvents.length === 1, "duplicate original job ACK");
				return { ...value, ack_events: ackEvents, native };
			},
		);
		Object.assign(witness, {
			accepted,
			draft_intact:
				nativeComposer(String(witness.draft_before)) === nativeComposer(String(witness.after)),
			draft_submitted: false,
			status: "passed",
			grade: "native-accepted-not-model-complete",
		});
	} catch (error) {
		Object.assign(witness, { status: "failed", error: String(error) });
		throw error;
	}
}

/** Canonical opt-in and remembered workspace trust for one fixture-owned home. */
function prepareSmokeHome(home: string, trustedFolders: string[]) {
	const copilotHome = join(home, ".copilot");
	mkdirSync(copilotHome, { recursive: true, mode: 0o700 });
	if (trustedFolders.length) {
		writeFileSync(join(copilotHome, "config.json"), `${JSON.stringify({ trustedFolders })}\n`, {
			flag: "wx",
			mode: 0o600,
		});
	}
	const installLog: string[] = [];
	for (const args of [[], ["--doctor-copilot"]]) {
		assert.equal(
			manageCopilotExtension({
				pijRoot: ROOT,
				home,
				copilotHome,
				args,
				stdout: (line) => installLog.push(line),
				stderr: (line) => installLog.push(line),
			}).skipped,
			0,
			installLog.join("\n"),
		);
	}
	return { home, copilotHome, installLog };
}

/** Read and retain, never repair a primary config changed by another scenario. */
export function verifySmokePrimarySettings(
	copilotHome: string,
	witness: Record<string, unknown>,
	field: string,
) {
	const raw = readFileSync(join(copilotHome, "settings.json"), "utf8");
	witness[field] = raw;
	assert.equal(
		object(JSON.parse(raw)).experimental,
		true,
		"PIJ_NATIVE_PRIMARY_SETTINGS: experimental must remain true",
	);
}

/** Only negative CLI configuration varies; transport and workspace remain the same. */
export async function launchSmokeNegativeControl(
	tmux: (args: string[]) => string,
	options: {
		run: string;
		primaryCopilotHome: string;
		cwd: string;
		trustedFolders: string[];
		copilotBin: string;
		common: string[];
	},
	timeout: number,
	witness: Record<string, unknown>,
): Promise<string> {
	witness.status = "starting";
	witness.primary_copilot_home = options.primaryCopilotHome;
	try {
		verifySmokePrimarySettings(options.primaryCopilotHome, witness, "primary_settings_before");
		const home = mkdtempSync(join(options.run, "negative-home-"));
		const { copilotHome, installLog } = prepareSmokeHome(home, options.trustedFolders);
		const environment = {
			HOME: home,
			COPILOT_HOME: copilotHome,
			XDG_CONFIG_HOME: join(home, ".config"),
		};
		witness.environment_overrides = environment;
		witness.install = installLog;
		witness.trusted_folders = options.trustedFolders;
		witness.home = home;
		witness.copilot_home = copilotHome;
		witness.negative_settings_before = readFileSync(join(copilotHome, "settings.json"), "utf8");
		const args = [
			"new-window",
			"-d",
			"-P",
			"-F",
			"#{pane_id}",
			"-t",
			"native-smoke",
			"-c",
			options.cwd,
			...Object.entries(environment).flatMap(([name, value]) => ["-e", `${name}=${value}`]),
			options.copilotBin,
			...options.common,
			"--no-experimental",
		];
		witness.launch_command = args;
		witness.launch_args = [...options.common, "--no-experimental"];
		let pane = "";
		let failure: { error: unknown } | undefined;
		try {
			pane = tmux(args);
			witness.pane = pane;
			assert(/^%\d+$/.test(pane), "actual tmux pane id required");
			await until("disabled CLI composer ready", timeout, () =>
				nativeComposer(tmux(["capture-pane", "-p", "-t", pane])) === "" ? true : undefined,
			);
		} catch (error) {
			failure = { error };
		}
		try {
			witness.negative_settings_after = readFileSync(join(copilotHome, "settings.json"), "utf8");
			verifySmokePrimarySettings(options.primaryCopilotHome, witness, "primary_settings_after");
		} catch (error) {
			witness.settings_error = String(error);
			failure ??= { error };
		}
		if (failure) throw failure.error;
		witness.status = "ready-not-absence-proven";
		return pane;
	} catch (error) {
		witness.status = "failed";
		witness.error = String(error);
		throw error;
	}
}

export async function runSmoke(options: Options): Promise<{ code: number; output: string }> {
	if (options.output)
		assert(!existsSync(options.output), `refusing to overwrite receipt ${options.output}`);
	const run = mkdtempSync(join(tmpdir(), "pij-copilot-native-"));
	const output = options.output ?? join(run, "receipt.json");
	assert(!existsSync(output), `refusing to overwrite receipt ${output}`);
	const secrets = new Set<string>();
	for (const key of ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"]) {
		if (process.env[key]) secrets.add(process.env[key] as string);
	}
	const receipt: Record<string, unknown> = {
		status: "running",
		provider:
			options.provider === "local"
				? "deterministic-local-fixture-not-real-inference"
				: "real-model",
		mode: options.mode,
		run,
		started: new Date().toISOString(),
		witnesses: [],
	};
	const witnesses = receipt.witnesses as Record<string, unknown>[];
	let daemon: ChildProcess | undefined;
	let fixture: Awaited<ReturnType<typeof startFixtureProvider>> | undefined;
	let env: NodeJS.ProcessEnv | undefined;
	let socketCreated = false;
	let code = 1;
	let captureFinalTyping: (() => ReturnType<typeof captureSmokeFinalTyping>) | undefined;
	const socket = join(run, "tmux.sock");
	const tmux = (args: string[], preserveOutput = false) =>
		command("tmux", ["-S", socket, ...args], env, preserveOutput);
	let daemonLog = "";
	try {
		if (!existsSync(options.pijBin))
			throw new Error(
				"PIJ_NATIVE_PREREQUISITE_BINARY: build the composed tree with CARGO_TARGET_DIR=$PWD/target cargo build -p pij-cli --bin pij-rs",
			);
		for (const module of ["extension.mjs", "store.mjs"]) {
			if (!existsSync(join(ROOT, ".copilot/extensions/pij", module)))
				throw new Error(
					`PIJ_NATIVE_PREREQUISITE_SOURCE: composed .copilot/extensions/pij/${module} missing; no shipping fixture substitutes`,
				);
		}
		if (options.provider === "real" && (!options.model || secrets.size === 0)) {
			throw new Error(
				"PIJ_NATIVE_PREREQUISITE_AUTH: --provider real requires --model and an inherited COPILOT_GITHUB_TOKEN, GH_TOKEN or GITHUB_TOKEN; never copy global auth files",
			);
		}
		const copilotBin = isAbsolute(options.copilotBin)
			? realpathSync(options.copilotBin)
			: realpathSync(command("which", [options.copilotBin]));
		const home = join(run, "home");
		const copilotHome = join(home, ".copilot");
		const state = join(run, "daemon");
		const cwd = join(run, "workspace");
		for (const path of [home, state, cwd]) mkdirSync(path, { recursive: true, mode: 0o700 });
		const addr = `127.0.0.1:${await freePort()}`;
		env = smokeEnvironment(process.env, home, copilotHome, state, addr, options.provider);
		if (options.provider === "local") {
			fixture = await startFixtureProvider();
			Object.assign(env, {
				COPILOT_PROVIDER_BASE_URL: fixture.url,
				COPILOT_PROVIDER_TYPE: "openai",
				COPILOT_PROVIDER_WIRE_API: "completions",
				COPILOT_MODEL: "pij-native-fixture",
			});
		} else env.COPILOT_MODEL = options.model;
		// Native remembered trust lives in config.json, not user settings.json.
		const trustedFolders = options.workspaceTrust === "seeded" ? [realpathSync(cwd)] : [];
		const { installLog } = prepareSmokeHome(home, trustedFolders);
		receipt.identity = {
			source_commit: command("git", ["-C", ROOT, "rev-parse", "HEAD"]),
			node: process.version,
			copilot: command(copilotBin, ["--version"], env),
			copilot_binary: copilotBin,
			copilot_sha256: sha(copilotBin),
			pij_binary: realpathSync(options.pijBin),
			pij_sha256: sha(options.pijBin),
			pij_version: command(options.pijBin, ["--version"], env),
			tmux: command("tmux", ["-V"]),
			source_modules: Object.fromEntries(
				["extension.mjs", "store.mjs"].map((name) => [
					name,
					sha(join(ROOT, ".copilot/extensions/pij", name)),
				]),
			),
			model: env.COPILOT_MODEL,
		};
		receipt.install = installLog;
		receipt.isolation = {
			home,
			copilotHome,
			state,
			addr,
			socket,
			workspace: realpathSync(cwd),
			workspace_trust: options.workspaceTrust,
			trusted_folders: trustedFolders,
			trust_config_before: existsSync(join(copilotHome, "config.json"))
				? readFileSync(join(copilotHome, "config.json"), "utf8")
				: null,
			manual_identity_env_keys: Object.keys(env).filter(
				(key) => key.startsWith("PIJ_") && !["PIJ_RS_ADDR", "PIJ_RS_STATE_DIR"].includes(key),
			),
			auth: options.provider === "real" ? "inherited-in-memory-only" : "none",
		};
		// A new socket and an empty tmux config prevent use of, or changes to, the live fleet.
		tmux([
			"-f",
			"/dev/null",
			"new-session",
			"-d",
			"-s",
			"native-smoke",
			"-x",
			"160",
			"-y",
			"48",
			"-c",
			cwd,
			"/bin/sleep",
			"600",
		]);
		socketCreated = true;
		env.TMUX = tmux(["display-message", "-p", "#{socket_path},#{pid},0"]);
		const common = [
			"--no-custom-instructions",
			"--disable-builtin-mcps",
			"--no-remote",
			"--no-auto-update",
			"--allow-tool=pij_send",
		];
		const key = () => {
			const value = readFileSync(join(state, "daemon.key"), "utf8").trim();
			assert(value, "isolated daemon key must not be empty");
			secrets.add(value);
			return value;
		};
		const api = async (path: string, body?: unknown, signal?: AbortSignal): Promise<unknown> => {
			const response = await fetch(`http://${addr}${path}`, {
				method: body === undefined ? "GET" : "POST",
				headers: { Authorization: `Bearer ${key()}`, "Content-Type": "application/json" },
				body: body === undefined ? undefined : JSON.stringify(body),
				signal: signal ?? AbortSignal.timeout(10_000),
			});
			const raw = await response.text();
			let envelope: Record<string, unknown>;
			try {
				envelope = object(JSON.parse(raw));
			} catch {
				throw new Error(`HTTP ${response.status} ${path}: ${raw.slice(0, 2048)}`);
			}
			if (!response.ok || envelope.ok !== true)
				throw new Error(`HTTP ${response.status} ${path}: ${raw.slice(0, 2048)}`);
			return envelope.data;
		};
		const bootDaemon = async () => {
			daemon = spawn(options.pijBin, ["--state-dir", state, "daemon", "--bind", addr], {
				env,
				cwd,
				stdio: ["ignore", "pipe", "pipe"],
			});
			daemon.stdout?.on("data", (chunk: Buffer) => {
				daemonLog += chunk.toString();
			});
			daemon.stderr?.on("data", (chunk: Buffer) => {
				daemonLog += chunk.toString();
			});
			let launchError: Error | undefined;
			daemon.once("error", (error) => {
				launchError = error;
			});
			const health = await until("isolated Rust daemon readiness", options.timeoutMs, async () => {
				if (launchError) throw launchError;
				if (daemon?.exitCode !== null)
					throw new Error(`isolated daemon exited: ${daemonLog.slice(-2048)}`);
				if (!existsSync(join(state, "daemon.key"))) return undefined;
				try {
					return await api("/health");
				} catch {
					return undefined;
				}
			});
			assert.equal(
				object(health).offline,
				false,
				"native smoke requires real Rust adapters, never offline fakes",
			);
			witnesses.push({ kind: "isolated-daemon-health", pid: daemon.pid, health });
		};
		// Before daemon/key creation, prove Pij failure does not break ordinary native use.
		const ordinaryNonce = `PIJ_ORDINARY_${randomUUID()}`;
		const ordinaryPane = tmux([
			"new-window",
			"-d",
			"-P",
			"-F",
			"#{pane_id}",
			"-t",
			"native-smoke",
			"-c",
			cwd,
			copilotBin,
			...common,
			"-i",
			`Ordinary Copilot smoke ${ordinaryNonce}. Reply exactly PIJ_NATIVE_OBSERVED. Do not call tools.`,
		]);
		assert.equal(
			existsSync(join(state, "daemon.key")),
			false,
			"daemon key must actually be absent",
		);
		const ordinaryEvents = await until(
			"ordinary native user/assistant while daemon absent",
			options.timeoutMs,
			() => {
				const stateRoot = join(copilotHome, "session-state");
				if (!existsSync(stateRoot)) return undefined;
				const matching = readdirSync(stateRoot, { withFileTypes: true })
					.filter((entry) => entry.isDirectory())
					.map((entry) => ({
						session: entry.name,
						events: readNativeEvents(join(stateRoot, entry.name, "events.jsonl")),
					}))
					.filter((row) => nativeMessages(row.events, ordinaryNonce).length === 1);
				assert(
					matching.length <= 1,
					"ordinary session identity must be unique by nonce, never newest-by-mtime",
				);
				return matching.find((row) =>
					row.events.some(
						(event) =>
							event.type === "assistant.message" && event.data.content === "PIJ_NATIVE_OBSERVED",
					),
				);
			},
		);
		const unavailablePrefix = "[pij native] unavailable:";
		await until("one actionable native unavailable diagnostic", options.timeoutMs, () =>
			tmux(["capture-pane", "-p", "-J", "-S", "-2000", "-t", ordinaryPane]).includes(
				unavailablePrefix,
			)
				? true
				: undefined,
		);
		await delay(2000);
		const ordinaryTerminal = tmux(["capture-pane", "-p", "-J", "-S", "-2000", "-t", ordinaryPane]);
		verifyOrdinaryUsability(ordinaryEvents.events, ordinaryNonce, ordinaryTerminal);
		assert.equal(existsSync(join(state, "daemon.key")), false);
		witnesses.push({
			kind: "daemon-key-absent-ordinary-use",
			nonce: ordinaryNonce,
			...ordinaryEvents,
			terminal: ordinaryTerminal,
			diagnostic_count: 1,
			daemon_started: false,
			key_present: false,
		});
		tmux(["kill-pane", "-t", ordinaryPane]);
		await bootDaemon();
		const rows = (sql: string, ...params: string[]): Record<string, unknown>[] => {
			const db = new DatabaseSync(join(state, "pij.sqlite"), { readOnly: true });
			try {
				return db.prepare(sql).all(...params);
			} finally {
				db.close();
			}
		};
		const delivered = (seat: Seat, msgId: string) =>
			rows(
				"SELECT recipient, msg_id, origin, delivered_at FROM delivered_messages WHERE recipient = ? AND msg_id = ?",
				seat.id,
				msgId,
			);
		const jobs = (seat: Seat, msgId: string) =>
			rows(
				"SELECT id, state, payload FROM jobs WHERE serial_key = ? AND dedupe_key = ?",
				seat.id,
				msgId,
			);
		const typingEvents = (seat: Seat, msgId: string) =>
			rows(
				`SELECT s.seq, s.at, s.kind, s.seat, s.payload FROM spine_events s
			 JOIN jobs j ON j.serial_key = s.seat AND
			 (json_extract(s.payload, '$.msg_id') = j.dedupe_key OR json_extract(s.payload, '$.job_id') = j.id)
			 WHERE j.serial_key = ? AND j.dedupe_key = ?
			 AND s.kind IN ('delivery.held', 'delivery.released', 'delivery.inbox-ack') ORDER BY s.seq`,
				seat.id,
				msgId,
			);
		const seats = async (signal?: AbortSignal): Promise<Seat[]> => {
			const data = await api("/v1/seats", undefined, signal);
			const list = Array.isArray(data) ? data : object(data).seats;
			assert(Array.isArray(list), "roster must be array or seats envelope");
			return list as Seat[];
		};
		captureFinalTyping = () =>
			captureSmokeFinalTyping({ tmux, seats, get: (path, signal) => api(path, undefined, signal) });
		const events = (seat: Seat) =>
			readNativeEvents(join(copilotHome, "session-state", seat.session, "events.jsonl"));
		const capture = (seat: Seat) => tmux(["capture-pane", "-p", "-J", "-t", seat.pane]);
		// Spawn still uses the real daemon launch/prebind path; exec preserves actual host identity.
		const wrapper = join(run, "copilot-smoke");
		writeFileSync(
			wrapper,
			`#!/bin/sh\nexec ${[copilotBin, ...common].map(quote).join(" ")} "$@"\n`,
			{ mode: 0o700 },
		);
		const prebindSends = new Map<string, unknown>();
		const launchProofs = new Map<string, { seat: Seat; process: string }>();
		const launch = async (
			mode: "manual" | "spawned",
			disabled = false,
			initial?: { from: Seat; body: string; msgId: string },
			resume?: Seat,
		): Promise<Seat> => {
			if (resume)
				assert(mode === "manual" && !disabled, "resume proof must be a native manual launch");
			let pane: string;
			let spawnReceipt: Record<string, unknown> | undefined;
			if (disabled) {
				assert.equal(mode, "manual", "negative control is a direct native launch");
				const negative: Record<string, unknown> = { kind: "missing-consumer-home-isolation" };
				witnesses.push(negative);
				pane = await launchSmokeNegativeControl(
					tmux,
					{ run, primaryCopilotHome: copilotHome, cwd, trustedFolders, copilotBin, common },
					options.timeoutMs,
					negative,
				);
			} else if (mode === "manual") {
				pane = tmux([
					"new-window",
					"-d",
					"-P",
					"-F",
					"#{pane_id}",
					"-t",
					"native-smoke",
					"-c",
					cwd,
					copilotBin,
					...common,
					...(resume ? [`--resume=${resume.session}`] : []),
				]);
			} else {
				spawnReceipt = object(
					JSON.parse(
						command(
							options.pijBin,
							[
								"--json",
								"--addr",
								addr,
								"--state-dir",
								state,
								"spawn",
								"--harness",
								"copilot",
								"--bin",
								wrapper,
								"--cwd",
								cwd,
								"--session",
								"native-smoke",
								"--no-wait",
							],
							env,
						),
					),
				);
				spawnReceipt = object(spawnReceipt.data);
				assert.equal(spawnReceipt.dispatched, true);
				pane = String(spawnReceipt.pane);
				if (initial) {
					const first = await api("/v1/send", {
						from: initial.from.id,
						to: { seat: spawnReceipt.id },
						body: initial.body,
						msg_id: initial.msgId,
					});
					prebindSends.set(String(spawnReceipt.id), first);
					witnesses.push({
						kind: "spawned-initial-task-before-bind-wait",
						prebind: spawnReceipt,
						msgId: initial.msgId,
						receipt: first,
					});
				}
			}
			assert(/^%\d+$/.test(pane), "actual tmux pane id required");
			if (disabled) {
				command(
					options.pijBin,
					["--json", "--addr", addr, "--state-dir", state, "adopt", pane, "--harness", "copilot"],
					env,
				);
			}
			const seat = await until("truthful native registration", options.timeoutMs, async () =>
				(await seats()).find(
					(row) =>
						row.pane === pane &&
						row.harness === "copilot" &&
						row.proc?.pid &&
						(disabled || row.native_extension_delivery === true),
				),
			);
			if (spawnReceipt)
				assert.equal(
					seat.id,
					spawnReceipt.id,
					"spawn preallocation must bind exactly the same seat",
				);
			assert.equal(
				(await seats()).filter((row) => row.pane === pane && row.proc?.pid === seat.proc.pid)
					.length,
				1,
				"one registered seat per host/pane",
			);
			if (!disabled) assert(/^[0-9a-f-]{36}$/i.test(seat.session), "native session UUID required");
			const processProof = command("ps", [
				"-p",
				String(seat.proc.pid),
				"-o",
				"pid=,ppid=,lstart=,command=",
			]);
			assert(!/--ui-server|--port(?:\s|=)/.test(processProof), "obsolete RPC args forbidden");
			launchProofs.set(seat.id, { seat, process: processProof });
			witnesses.push({
				kind: disabled ? "missing-consumer-start" : `${mode}-registration`,
				seat,
				process: processProof,
				spawn: spawnReceipt,
				pane: capture(seat),
				resume_of: resume,
			});
			return seat;
		};
		const send = (from: Seat, to: Seat, body: string, msgId: string) =>
			api("/v1/send", { from: from.id, to: { seat: to.id }, body, msg_id: msgId });
		const waitDelivery = async (seat: Seat, nonce: string, msgId: string) => {
			const native = await until(
				"native user message with exact nonce",
				options.timeoutMs,
				() => nativeMessages(events(seat), nonce)[0],
			);
			assert.equal(
				typeof native.data.messageId,
				"string",
				"native messageId is distinct from native event id",
			);
			const ack = await until("daemon reader-read acknowledgement", options.timeoutMs, () =>
				delivered(seat, msgId).find((row) => row.origin === "reader-read"),
			);
			assert.equal(nativeMessages(events(seat), nonce).length, 1, "native duplicate injection");
			return {
				native,
				acknowledgement: ack,
				jobs: jobs(seat, msgId),
				grade: "native-accepted-not-model-complete",
			};
		};
		const waitTurnEnd = (seat: Seat, nonce: string) =>
			until("native turn ended and lifecycle composer ready", options.timeoutMs, () => {
				const native = events(seat);
				const message = nativeMessages(native, nonce)[0];
				const index = native.findIndex((event) => event.id === message?.id);
				if (index < 0) return undefined;
				const ended = native.find(
					(event, offset) => offset > index && event.type === "assistant.turn_end",
				);
				return ended && nativeComposer(capture(seat)) === "" ? ended : undefined;
			});
		for (const mode of options.mode === "both"
			? (["manual", "spawned"] as const)
			: [options.mode]) {
			const sender = await launch(mode);
			const nonce = `PIJ_NATIVE_${randomUUID()}`;
			const msgId = randomUUID();
			const body = sendPrompt(sender.id, nonce);
			const target = await launch(mode, false, { from: sender, body, msgId });
			const initial =
				mode === "spawned" ? prebindSends.get(target.id) : await send(sender, target, body, msgId);
			const accepted = await waitDelivery(target, nonce, msgId);
			const approval: Record<string, unknown> = { mode };
			witnesses.push(approval);
			const launchProof = launchProofs.get(target.id);
			assert(launchProof, "actual launch proof required");
			await confirmSmokeToolInvocation(
				tmux,
				{ pane: target.pane, to: sender.id, message: nonce },
				options.timeoutMs,
				approval,
				mode === "spawned"
					? {
							launch: launchProof,
							incomingEventId: accepted.native.id,
							observe: () => ({
								nativeEvents: events(target),
								jobs: rows(
									"SELECT id, kind, state, serial_key, dedupe_key, payload FROM jobs WHERE serial_key = ?",
									sender.id,
								),
							}),
						}
					: undefined,
			);
			const reply = await until(
				"native pij_send reply arrives at original sender",
				options.timeoutMs,
				() => nativeMessages(events(sender), nonce)[0],
			);
			const tool = await until("actual native outgoing tool event", options.timeoutMs, () =>
				events(target).find(
					(event) =>
						JSON.stringify(event.data).includes("pij_send") &&
						["assistant.message", "tool.execution_start"].includes(event.type),
				),
			);
			const outbound = await until(
				"durable outgoing Pij job with exact source identity",
				options.timeoutMs,
				() =>
					rows("SELECT id, state, payload FROM jobs WHERE serial_key = ?", sender.id).find(
						(row) => {
							const message = object(JSON.parse(String(row.payload)));
							return message.from === target.id && message.body === nonce;
						},
					),
			);
			const completion = await until("separate model completion canary", options.timeoutMs, () =>
				events(target).find(
					(event) =>
						event.type === "assistant.message" &&
						typeof event.data.content === "string" &&
						event.data.content.includes("PIJ_NATIVE_DONE"),
				),
			);
			const duplicate = await send(sender, target, body, msgId);
			await delay(1500);
			assert.equal(
				nativeMessages(events(target), nonce).length,
				1,
				"accepted duplicate must not reinject",
			);
			assert.equal(
				nativeMessages(events(sender), nonce).length,
				1,
				"outgoing tool reply must not repeat",
			);
			witnesses.push({
				kind: `${mode}-roundtrip`,
				nonce,
				msgId,
				initial,
				accepted,
				reply,
				tool,
				outbound,
				completion,
				duplicate,
				target_events: events(target),
				sender_events: events(sender),
				note: "native IDs and reader-read are acceptance; assistant completion/idle events are separate",
			});
			await waitTurnEnd(target, nonce);
			const draft = `HUMAN_DRAFT_${randomUUID()}`;
			const typingNonce = `PIJ_TYPING_${randomUUID()}`;
			const typingId = randomUUID();
			const typing: Record<string, unknown> = {
				kind: `${mode}-native-delivery-while-typing`,
			};
			witnesses.push(typing);
			await driveSmokeTypingWitness(
				{
					tmux,
					observe: async () => ({
						nativeEvents: events(target),
						acknowledgements: delivered(target, typingId),
						deliveryEvents: typingEvents(target, typingId),
						jobs: jobs(target, typingId),
					}),
					send: () => send(sender, target, typingNonce, typingId),
				},
				{ seat: target, draft, nonce: typingNonce, msgId: typingId },
				options.timeoutMs,
				typing,
			);
			// Only AFTER native acceptance with the unchanged draft: restore the fixture for later scenarios.
			const cleanup: Record<string, unknown> = {
				before: capture(target),
				key: HUMAN_KEYS.clearDraft,
			};
			typing.post_proof_cleanup = cleanup;
			assert.equal(
				nativeComposer(String(cleanup.before)),
				draft,
				"refusing to clear changed composer",
			);
			tmux(["send-keys", "-t", target.pane, HUMAN_KEYS.clearDraft]);
			await until("post-proof fixture draft cleanup", options.timeoutMs, () => {
				const terminal = capture(target);
				cleanup.after = terminal;
				return nativeComposer(terminal) === "" ? true : undefined;
			});
			const disabled = await launch("manual", true);
			const absentNonce = `PIJ_ABSENT_${randomUUID()}`;
			const absentId = randomUUID();
			const refused = await send(sender, disabled, absentNonce, absentId);
			await delay(2000);
			assert.equal(
				delivered(disabled, absentId).length,
				0,
				"disabled extension must not be delivered by fallback",
			);
			assert(
				!capture(disabled).includes(absentNonce),
				"disabled extension must never receive tmux text",
			);
			const outcome = object(object(refused).outcome).outcome;
			assert(
				["queued", "held", "refused"].includes(String(outcome)),
				"absence requires explicit queued/held/refused outcome",
			);
			witnesses.push({
				kind: `${mode}-missing-consumer`,
				nonce: absentNonce,
				msgId: absentId,
				receipt: refused,
				jobs: jobs(disabled, absentId),
				pane: capture(disabled),
			});
			assert(daemon);
			await stopOwned(daemon);
			key(); // Remember old credential solely for output redaction.
			writeFileSync(join(state, "daemon.key"), `${randomUUID()}${randomUUID()}\n`, { mode: 0o600 });
			await bootDaemon();
			await until(
				"same native session reattested after isolated key rotation",
				options.timeoutMs,
				async () =>
					(await seats()).find(
						(row) =>
							row.id === target.id &&
							row.session === target.session &&
							row.proc?.pid === target.proc.pid &&
							row.native_extension_delivery === true,
					),
			);
			const reconnectNonce = `PIJ_RECONNECT_${randomUUID()}`;
			const reconnectId = randomUUID();
			await send(sender, target, reconnectNonce, reconnectId);
			witnesses.push({
				kind: `${mode}-reconnect-key-rotation`,
				nonce: reconnectNonce,
				msgId: reconnectId,
				...(await waitDelivery(target, reconnectNonce, reconnectId)),
			});
			const oldNonce = `PIJ_OLD_CONTEXT_${randomUUID()}`;
			const oldMsgId = randomUUID();
			const oldExpected = {
				seat: target.id,
				session: target.session,
				nonce: oldNonce,
				msgId: oldMsgId,
			};
			const rollover: Record<string, unknown> = {
				kind: `${mode}-native-rollover`,
				before: target,
				old_expected: oldExpected,
			};
			witnesses.push(rollover);
			rollover.previous_turn_end = await waitTurnEnd(target, reconnectNonce);
			// A staged /new draft no longer gates native delivery. Hold the owned
			// fixture explicitly so the rollover still exercises outstanding work.
			rollover.predecessor_hold = await api("/v1/report", {
				seat: target.id,
				argv: ["report", "state", "hold"],
			});
			assert.equal(object(rollover.predecessor_hold).state, "hold");
			const oldContextSnapshot = (successor?: Seat) => ({
				jobs: jobs(target, oldMsgId),
				acknowledgements: rows(
					"SELECT recipient, msg_id, origin, delivered_at FROM delivered_messages WHERE msg_id = ?",
					oldMsgId,
				),
				predecessorEvents: events(target),
				successorEvents: successor ? events(successor) : [],
				successorJobs: successor ? jobs(successor, oldMsgId) : [],
			});
			const newControl: Record<string, unknown> = {};
			rollover.human_control = newControl;
			await driveSmokeLifecycleCommand(
				tmux,
				{ pane: target.pane, action: "newSession" },
				options.timeoutMs,
				newControl,
				async () => {
					rollover.old_send = await send(sender, target, oldNonce, oldMsgId);
					await delay(2000);
					const held = oldContextSnapshot();
					rollover.held_before_new = held;
					verifyNativeContextHeld(oldExpected, held);
				},
			);
			const successor = await until(
				"truthful native /new registration",
				options.timeoutMs,
				async () => {
					const terminal = capture(target);
					newControl.after = terminal;
					nativeComposer(terminal); // An unexpected modal is not a new consent policy.
					const roster = await seats();
					rollover.roster_after = roster;
					return roster.find(
						(row) =>
							row.pane === target.pane &&
							row.session !== target.session &&
							row.native_extension_delivery === true,
					);
				},
			);
			rollover.after = successor;
			verifyNativeLifecycleIdentity(target, successor, "new");
			assert.notEqual(
				object(successor).semantic_state,
				"hold",
				"predecessor Hold must not mask successor native-target isolation",
			);
			if (mode === "spawned") {
				assert.equal(typeof target.spawn_id, "string", "spawned source correlation required");
				assert.equal(
					successor.spawn_id,
					target.spawn_id,
					"spawned /new must preserve verified spawn correlation",
				);
			}
			const retired = (await seats()).find((row) => row.id === target.id);
			rollover.retired_predecessor = retired;
			assert(retired, "superseded native predecessor must remain observable");
			assert.equal(retired.native_extension_delivery, false);
			assert(Number(object(retired).tombstoned_at) > 0);
			assert(String(object(retired).tombstone_reason).includes(successor.id));
			const oldQuery = new URLSearchParams({
				seat: target.id,
				wait: "false",
				native_session: target.session,
				pid: String(target.proc.pid),
				proc_start: String(target.proc.proc_start),
			});
			const staleClaim = await fetch(`http://${addr}/v1/inbox?${oldQuery}`, {
				headers: { Authorization: `Bearer ${key()}` },
				signal: AbortSignal.timeout(10_000),
			});
			const staleBody = object(await staleClaim.json());
			rollover.old_consumer_refusal = {
				status: staleClaim.status,
				body: staleBody,
				tuple: target.proc,
				session: target.session,
			};
			assert.equal(staleClaim.status, 400, "retired native consumer must be refused");
			assert.equal(staleBody.ok, false);
			assert.equal(staleBody.error, "refused");
			assert(
				String(staleBody.meta).includes(
					"native-extension-unavailable: Copilot requires current native registration",
				),
			);
			const freshNonce = `PIJ_NEW_CONTEXT_${randomUUID()}`;
			const freshId = randomUUID();
			rollover.fresh_send = await send(sender, successor, freshNonce, freshId);
			rollover.fresh_delivery = await waitDelivery(successor, freshNonce, freshId);
			rollover.fresh_turn_end = await waitTurnEnd(successor, freshNonce);
			assert.equal(nativeMessages(events(successor), freshNonce).length, 1);
			assert.equal(delivered(successor, freshId).length, 1);
			const isolated = oldContextSnapshot(successor);
			rollover.held_after_new = isolated;
			verifyNativeContextHeld(oldExpected, isolated);
			rollover.grade =
				"native-accepted; old binding revoked and claims refused; private abort signal not observed";
			let activeTarget = successor;
			if (mode === "manual") {
				const resume: Record<string, unknown> = {
					kind: "manual-native-dead-host-resume",
					before: successor,
				};
				witnesses.push(resume);
				resume.process_before_exit = command("ps", [
					"-p",
					String(successor.proc.pid),
					"-o",
					"pid=,ppid=,lstart=,command=",
				]);
				const exitControl: Record<string, unknown> = {};
				resume.human_control = exitControl;
				await driveSmokeLifecycleCommand(
					tmux,
					{ pane: successor.pane, action: "exit" },
					options.timeoutMs,
					exitControl,
				);
				const hostTeardown: Record<string, unknown> = {};
				resume.host_teardown = hostTeardown;
				const ownedLaunch = launchProofs.get(target.id);
				assert(ownedLaunch, "actual launch ownership required before host teardown");
				resume.host_death = await stopSmokeNativeHost(
					{ tmux, probe: (pid, signal) => process.kill(pid, signal) },
					{ previous: successor, launch: ownedLaunch.seat, socket },
					options.timeoutMs,
					hostTeardown,
				);
				const identityKeys: string[] = Object.keys(env).filter(
					(name) => name.startsWith("PIJ_") && !["PIJ_RS_ADDR", "PIJ_RS_STATE_DIR"].includes(name),
				);
				resume.identity_environment_keys = identityKeys;
				assert.deepEqual(identityKeys, [], "manual resume must not inherit Pij preallocation");
				resume.launch_args = [...common, `--resume=${successor.session}`];
				verifySmokePrimarySettings(copilotHome, resume, "primary_settings_before_resume");
				activeTarget = await launch("manual", false, undefined, successor);
				resume.after = activeTarget;
				verifyNativeLifecycleIdentity(successor, activeTarget, "resume");
				const resumedNonce = `PIJ_RESUMED_${randomUUID()}`;
				const resumedId = randomUUID();
				resume.send = await send(sender, activeTarget, resumedNonce, resumedId);
				resume.delivery = await waitDelivery(activeTarget, resumedNonce, resumedId);
				resume.turn_end = await waitTurnEnd(activeTarget, resumedNonce);
				resume.native_events = events(activeTarget);
				resume.roster = await seats();
				resume.terminal = capture(activeTarget);
				assert.equal(nativeMessages(events(activeTarget), resumedNonce).length, 1);
				assert.equal(delivered(activeTarget, resumedId).length, 1);
				const stillIsolated = oldContextSnapshot(activeTarget);
				resume.old_context_exclusion = stillIsolated;
				verifyNativeContextHeld(oldExpected, stillIsolated);
				resume.grade =
					"native-accepted-not-model-complete; real host death observed before fresh manual resume";
			}
			// Successful modes also retire panes before the outer finally block.
			witnesses.push({ kind: `${mode}-final-typing-capture`, ...(await captureFinalTyping()) });
			for (const seat of [sender, activeTarget, disabled]) tmux(["kill-pane", "-t", seat.pane]);
		}
		receipt.status = "passed";
		code = 0;
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		receipt.status = message.includes("PIJ_NATIVE_PREREQUISITE_")
			? "prerequisite-missing"
			: "failed";
		receipt.error = message;
		code = receipt.status === "prerequisite-missing" ? 2 : 1;
	} finally {
		if (socketCreated && !(await finishSmokePanes(tmux, receipt, captureFinalTyping))) code = 1;
		if (daemon) await stopOwned(daemon);
		if (fixture) {
			receipt.fixture_requests = fixture.requests;
			await fixture.close();
		}
		receipt.daemon_log = daemonLog;
		if (code === 1) receipt.status = "failed";
		receipt.finished = new Date().toISOString();
		let serialized = `${JSON.stringify(receipt, null, 2)}\n`;
		for (const secret of secrets) serialized = serialized.replaceAll(secret, "[redacted]");
		mkdirSync(dirname(output), { recursive: true });
		writeFileSync(output, serialized, { flag: "wx", mode: 0o600 });
	}
	return { code, output };
}

const entry = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (entry === import.meta.url) {
	if (process.argv.includes("--help")) {
		console.log(
			"copilot-native-smoke --mode manual|spawned|both --provider local|real [--workspace-trust seeded|prompt] [--model MODEL] [--pij-bin PATH] [--copilot-bin PATH] [--output RECEIPT.json] [--timeout-seconds 90]\nUses isolated HOME/COPILOT_HOME, dedicated tmux socket and owned real Rust daemon. Real provider requires inherited token; never copies auth. Exit 0 passed, 1 failed, 2 prerequisite missing. No global setup or restart. Workspace trust defaults to seeded; prompt is a negative control expected to time out before provider activity.",
		);
	} else {
		try {
			const result = await runSmoke(parseSmokeArgs(process.argv.slice(2)));
			console.log(JSON.stringify(result));
			process.exitCode = result.code;
		} catch (error) {
			console.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 2;
		}
	}
}
