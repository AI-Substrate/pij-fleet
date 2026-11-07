#!/usr/bin/env node
// Plan 214 AC4 (ruling C31): the live mid-turn steering canary.
//
// A real Copilot CLI host, with this tree's native pij extension, against a
// private pij-rs daemon built from this tree, on its own tmux socket, with
// private HOME and COPILOT_HOME. Nothing touches the live fleet. The model is a
// deterministic local provider (no auth, no inference): each reply waits
// REPLY_DELAY_MS and, while a turn is under ROUNDS tool calls, calls a harmless
// tool again, so a turn a pij message starts runs about a minute.
//
// Run 1 (steering on, the default): a pij message starts the long turn and
// five more normal sends arrive during it. Each must be claimed and reach the
// session (claimed_at, acked_at) before the turn ends, the provider must see
// each body inside that same turn, the turn must end with the final reply, and
// the seat must go idle.
// Run 2: `pij steer off` for the seat, a second long turn, one send during it:
// it must wait for that turn to end.
//
// Usage: node steer-canary.mjs --pij-bin <path> [--copilot-bin <path>] [--out <dir>]
// Writes receipt.json (and the daemon log, provider log, pane captures) to --out.

import { execFileSync, spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, realpathSync, symlinkSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { createServer as netServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";

const ROOT = resolve(import.meta.dirname, "../../../../..");
const args = process.argv.slice(2);
const flag = (name, fallback) => {
	const at = args.indexOf(name);
	return at < 0 ? fallback : args[at + 1];
};
const pijBin = resolve(flag("--pij-bin", join(ROOT, "target/debug/pij-rs")));
const copilotBin = realpathSync(
	flag("--copilot-bin", execFileSync("which", ["copilot"], { encoding: "utf8" }).trim()),
);
const run = resolve(flag("--out", join(tmpdir(), `pij-steer-canary-${Date.now()}`)));
const ROUNDS = 8;
const REPLY_DELAY_MS = 10_000;
const MARKER = "CANARY_LONG_TURN";
const SENDER = "pij-canary-sender";

const now = () => new Date().toISOString();
const log = (line) => process.stdout.write(`${now()} ${line}\n`);
const receipt = { started: now(), run, rounds: ROUNDS, reply_delay_ms: REPLY_DELAY_MS, steps: [] };
const step = (kind, data = {}) => {
	receipt.steps.push({ at: now(), kind, ...data });
	log(`${kind} ${JSON.stringify(data)}`);
};

async function freePort() {
	return await new Promise((done) => {
		const server = netServer().listen(0, "127.0.0.1", () => {
			const { port } = server.address();
			server.close(() => done(port));
		});
	});
}

// --- the deterministic provider ------------------------------------------
const providerLog = [];
let finalSentAt = 0;
const text = (message) =>
	typeof message?.content === "string"
		? message.content
		: Array.isArray(message?.content)
			? message.content.map((part) => part?.text ?? "").join("\n")
			: "";
function reply(body) {
	const messages = Array.isArray(body.messages) ? body.messages : [];
	let start = -1;
	for (let i = messages.length - 1; i >= 0; i--) {
		if (messages[i].role === "user" && text(messages[i]).includes(MARKER)) {
			start = i;
			break;
		}
	}
	const after = start < 0 ? [] : messages.slice(start + 1);
	const tools = after.filter((message) => message.role === "tool").length;
	const finished = after.some(
		(message) => message.role === "assistant" && text(message).includes("CANARY_TURN_DONE"),
	);
	const steered = after
		.filter((message) => message.role === "user")
		.map((message) => /msg_id="(canary-[^"]+)"/.exec(text(message))?.[1])
		.filter(Boolean);
	providerLog.push({ at: now(), turn_start_index: start, tools, finished, steered });
	if (start >= 0 && !finished && tools < ROUNDS) {
		const names = (body.tools ?? []).map((tool) => tool?.function?.name).filter(Boolean);
		const name =
			names.find((n) => /^(bash|shell)$/i.test(n)) ??
			names.find((n) => /bash|shell/i.test(n)) ??
			names[0];
		return {
			role: "assistant",
			content: null,
			tool_calls: [
				{
					id: `call_${randomUUID()}`,
					type: "function",
					function: {
						name,
						arguments: JSON.stringify({
							command: "true",
							description: "canary round",
							mode: "sync",
							initial_wait: 30,
						}),
					},
				},
			],
		};
	}
	const final = start >= 0 && !finished;
	if (final) providerLog.push({ at: now(), final: true });
	return { role: "assistant", content: final ? "CANARY_TURN_DONE" : "CANARY_ACK" };
}
async function startProvider() {
	const server = createServer(async (request, response) => {
		try {
			if (request.method === "GET" && request.url?.endsWith("/models")) {
				response.setHeader("Content-Type", "application/json");
				response.end(
					JSON.stringify({ object: "list", data: [{ id: "pij-canary", object: "model" }] }),
				);
				return;
			}
			const chunks = [];
			for await (const chunk of request) chunks.push(chunk);
			const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
			const message = reply(body);
			await delay(REPLY_DELAY_MS);
			if (message.content === "CANARY_TURN_DONE") finalSentAt = Math.floor(Date.now() / 1000);
			const completion = {
				id: `chatcmpl-${randomUUID()}`,
				object: "chat.completion",
				created: Math.floor(Date.now() / 1000),
				model: "pij-canary",
				choices: [
					{ index: 0, message, finish_reason: message.tool_calls ? "tool_calls" : "stop" },
				],
				usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
			};
			if (body.stream) {
				response.setHeader("Content-Type", "text/event-stream");
				const delta = message.tool_calls
					? {
							role: "assistant",
							tool_calls: message.tool_calls.map((call, index) => ({ index, ...call })),
						}
					: { role: "assistant", content: message.content };
				response.write(
					`data: ${JSON.stringify({ ...completion, object: "chat.completion.chunk", choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`,
				);
				response.write(
					`data: ${JSON.stringify({ ...completion, object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: completion.choices[0].finish_reason }] })}\n\n`,
				);
				response.end("data: [DONE]\n\n");
			} else {
				response.setHeader("Content-Type", "application/json");
				response.end(JSON.stringify(completion));
			}
		} catch (error) {
			response.writeHead(500).end(String(error));
		}
	});
	const port = await freePort();
	await new Promise((done) => server.listen(port, "127.0.0.1", done));
	return { url: `http://127.0.0.1:${port}/v1`, close: () => server.close() };
}

// --- the private world ------------------------------------------------------
const home = join(run, "home");
const copilotHome = join(home, ".copilot");
const state = join(run, "daemon");
const cwd = join(run, "workspace");
for (const path of [home, copilotHome, state, cwd, join(copilotHome, "extensions")])
	mkdirSync(path, { recursive: true, mode: 0o700 });
const workspace = realpathSync(cwd);
symlinkSync(join(ROOT, ".copilot/extensions/pij"), join(copilotHome, "extensions/pij"));
writeFileSync(join(copilotHome, "settings.json"), `${JSON.stringify({ experimental: true })}\n`);
writeFileSync(
	join(copilotHome, "config.json"),
	`${JSON.stringify({ trustedFolders: [workspace] })}\n`,
);
writeFileSync(
	join(copilotHome, "permissions-config.json"),
	`${JSON.stringify({
		locations: {
			[workspace]: {
				tool_approvals: [{ kind: "extension-permission-access", extensionName: "user:pij" }],
			},
		},
	})}\n`,
);

const addr = `127.0.0.1:${await freePort()}`;
const provider = await startProvider();
const env = {
	PATH: process.env.PATH,
	LANG: process.env.LANG ?? "en_US.UTF-8",
	TERM: "xterm-256color",
	TMPDIR: process.env.TMPDIR,
	SHELL: "/bin/zsh",
	USER: process.env.USER,
	HOME: home,
	XDG_CONFIG_HOME: join(home, ".config"),
	COPILOT_HOME: copilotHome,
	PIJ_RS_STATE_DIR: state,
	PIJ_RS_ADDR: addr,
	COPILOT_AUTO_UPDATE: "false",
	COPILOT_OFFLINE: "true",
	NO_PROXY: "127.0.0.1,localhost",
	COPILOT_PROVIDER_BASE_URL: provider.url,
	COPILOT_PROVIDER_TYPE: "openai",
	COPILOT_PROVIDER_WIRE_API: "completions",
	COPILOT_MODEL: "pij-canary",
};
const socket = join(run, "tmux.sock");
const tmux = (argv) =>
	execFileSync("tmux", ["-S", socket, ...argv], { env, encoding: "utf8" }).trim();

let daemonLog = "";
let daemon;
function startDaemon() {
daemon = spawn(pijBin, ["--state-dir", state, "daemon", "--bind", addr], {
	env,
	cwd,
	stdio: ["ignore", "pipe", "pipe"],
});
daemon.stdout.on("data", (chunk) => {
	daemonLog += chunk;
});
daemon.stderr.on("data", (chunk) => {
	daemonLog += chunk;
});
}

const key = () => readFileSync(join(state, "daemon.key"), "utf8").trim();
async function api(path, body) {
	const response = await fetch(`http://${addr}${path}`, {
		method: body === undefined ? "GET" : "POST",
		headers: { Authorization: `Bearer ${key()}`, "Content-Type": "application/json" },
		body: body === undefined ? undefined : JSON.stringify(body),
		signal: AbortSignal.timeout(15_000),
	});
	const envelope = await response.json();
	if (!envelope.ok) throw new Error(`${path}: ${JSON.stringify(envelope)}`);
	return envelope.data;
}
async function seats() {
	const data = await api("/v1/seats");
	return Array.isArray(data) ? data : data.seats;
}
async function until(what, ms, probe) {
	const deadline = Date.now() + ms;
	for (;;) {
		try {
			const value = await probe();
			if (value) return value;
		} catch {}
		if (Date.now() > deadline) throw new Error(`timed out: ${what}`);
		await delay(500);
	}
}
const store = () => new DatabaseSync(join(state, "pij.sqlite"), { readOnly: true });
const rows = (seat, ids) => {
	const db = store();
	try {
		return ids.map((id) =>
			db
				.prepare(
					"SELECT dedupe_key AS msg_id, state, enqueued_at, claimed_at, acked_at FROM jobs WHERE kind = ? AND dedupe_key = ? ORDER BY id DESC LIMIT 1",
				)
				.get(`delivery:${seat}`, id),
		);
	} finally {
		db.close();
	}
};
const send = (to, body, msgId) =>
	api("/v1/send", { from: SENDER, to: { seat: to, machine: null }, body, msg_id: msgId });
const capture = (pane) => tmux(["capture-pane", "-p", "-J", "-S", "-200", "-t", pane]);

let code = 1;
let pane;
try {
	// The private tmux server exists before the daemon, and the daemon is
	// pointed at it, so it never sees the live fleet's panes.
	tmux(["-f", "/dev/null", "new-session", "-d", "-s", "canary", "-x", "160", "-y", "48", "-c", cwd, "/bin/sleep", "3600"]);
	tmux(["set-option", "-g", "remain-on-exit", "on"]);
	env.TMUX = tmux(["display-message", "-p", "#{socket_path},#{pid},0"]);
	startDaemon();
	await until("daemon key", 60_000, async () => key());
	await until("daemon answers", 60_000, async () => Array.isArray(await seats()));
	pane = tmux([
		"new-window", "-d", "-P", "-F", "#{pane_id}", "-t", "canary", "-c", cwd,
		copilotBin, "--no-custom-instructions", "--disable-builtin-mcps",
		"--no-auto-update", "--allow-all-tools",
	]);
	receipt.identity = {
		source_commit: execFileSync("git", ["-C", ROOT, "rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
		pij_bin: pijBin,
		copilot: execFileSync(copilotBin, ["--version"], { env, encoding: "utf8" }).trim(),
		node: process.version,
		addr,
		pane,
		provider: "deterministic-local-canary-not-real-inference",
	};
	const seat = await until("native registration", 90_000, async () =>
		(await seats()).find(
			(row) => row.pane === pane && row.harness === "copilot" && row.native_extension_delivery,
		),
	);
	step("registered", { seat: seat.id, pid: seat.proc?.pid, session: seat.session });

	// Run 1: steering on (default).
	const start1 = `canary-start-1-${randomUUID()}`;
	step("send", { msg_id: start1, receipt: await send(seat.id, `${MARKER} 1: keep working.`, start1) });
	await until("turn 1 under way", 60_000, async () => providerLog.some((row) => row.turn_start_index >= 0));
	const steered = [];
	for (let n = 1; n <= 5; n++) {
		await delay(6_000);
		const id = `canary-steer-${n}-${randomUUID()}`;
		steered.push(id);
		step("send", { msg_id: id, receipt: await send(seat.id, `steer ${n}: noted mid-turn.`, id) });
	}
	finalSentAt = 0;
	await until("turn 1 final reply sent", 240_000, async () => finalSentAt > 0);
	const turnEnd1 = finalSentAt;
	const idle1 = await until("seat idle after turn 1", 90_000, async () => {
		const row = (await seats()).find((s) => s.id === seat.id);
		return row?.state === "idle" ? row : undefined;
	});
	const run1 = rows(seat.id, [start1, ...steered]);
	const insideTurn1 = providerLog
		.filter((row) => row.turn_start_index >= 0 && !row.finished && !row.final)
		.flatMap((row) => row.steered ?? []);
	step("run1", {
		rows: run1,
		turn_end_observed_at_s: turnEnd1,
		steered_seen_by_model_inside_turn: [...new Set(insideTurn1)],
		idle_state: idle1.state,
		provider_log: [...providerLog],
	});
	const pass1 =
		run1.every((row) => row?.state === "done" && row.acked_at <= turnEnd1) &&
		steered.every((id) => insideTurn1.some((seen) => seen === id));

	// Run 2: steering off.
	const off = await api("/v1/steer", { argv: ["steer", "off", seat.id], caller: { TMUX_PANE: pane, cwd } });
	step("steer off", { receipt: off });
	providerLog.length = 0;
	const start2 = `canary-start-2-${randomUUID()}`;
	step("send", { msg_id: start2, receipt: await send(seat.id, `${MARKER} 2: keep working.`, start2) });
	await until("turn 2 under way", 60_000, async () => providerLog.some((row) => row.turn_start_index >= 0));
	await delay(6_000);
	const waiting = `canary-wait-${randomUUID()}`;
	step("send", { msg_id: waiting, receipt: await send(seat.id, "after steer off: should wait.", waiting) });
	finalSentAt = 0;
	await until("turn 2 final reply sent", 240_000, async () => finalSentAt > 0);
	const turnEnd2 = finalSentAt;
	const waited = await until("the held send reaches the session", 120_000, async () => {
		const [row] = rows(seat.id, [waiting]);
		return row?.state === "done" ? row : undefined;
	});
	const insideTurn2 = providerLog
		.filter((row) => row.turn_start_index >= 0 && !row.finished && !row.final)
		.flatMap((row) => row.steered ?? []);
	step("run2", { row: waited, turn_end_observed_at_s: turnEnd2, steered_inside_turn: insideTurn2, provider_log: [...providerLog] });
	const pass2 = waited.claimed_at >= turnEnd2 - 1 && !insideTurn2.includes(waiting);

	receipt.pass = { run1_steered_inside_turn_and_idle: pass1, run2_waited_for_turn_end: pass2 };
	code = pass1 && pass2 ? 0 : 1;
} catch (error) {
	receipt.error = String(error?.stack ?? error);
	log(receipt.error);
} finally {
	try {
		if (pane) writeFileSync(join(run, "pane.txt"), capture(pane));
	} catch {}
	receipt.provider_log = providerLog;
	receipt.finished = now();
	receipt.code = code;
	writeFileSync(join(run, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
	writeFileSync(join(run, "daemon.log"), daemonLog);
	try {
		tmux(["kill-server"]);
	} catch {}
	daemon?.kill();
	provider.close();
	log(`receipt: ${join(run, "receipt.json")} code=${code}`);
	process.exit(code);
}
