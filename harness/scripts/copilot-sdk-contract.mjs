#!/usr/bin/env node
// D-F208-1: does the INSTALLED Copilot SDK register pij's tools and hooks?
//
// F208: Copilot CLI 1.0.113 moved the session config to `tools.definitions`
// and `hooks.handlers` and silently ignored the old flat shape. pij seats lost
// pij_send, FYI claims and turn-end blocks for two days while every test passed,
// because the tests drove a hand-written fake SDK. This check drives the real
// one: it runs the real `.copilot/extensions/pij/extension.mjs` in a child
// process, with `@microsoft/copilot-sdk/extension` resolved to the installed
// CLI's own SDK (copilot-sdk-contract-hook.mjs), and plays the CLI's side of
// the SDK's JSON-RPC wire on its stdio. It then asks, through that wire, what
// a live CLI asks: which tools did the session declare, and does each hook and
// tool call reach pij's handler.
//
// No daemon, pane or Copilot process is touched: the child's daemon address is
// unreachable, its state dir is empty and it has no TMUX_PANE, so no handler
// can do anything but answer "not ready".
//
// Exit 0: passed, or skipped (no Copilot CLI installed, as in CI). Exit 1: the
// installed SDK would not register pij's tools or hooks. `--json` prints one
// report; `--record` writes the version it passed on to
// harness/copilot-sdk-verified.json, which a later run compares against (a
// different installed version is a `warn` until recorded).
import { execFile, spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { promisify } from "node:util";

const repo = join(import.meta.dirname, "..", "..");
const EXTENSION = join(repo, ".copilot", "extensions", "pij", "extension.mjs");
const HOOK = join(import.meta.dirname, "copilot-sdk-contract-hook.mjs");
const RECORD = join(repo, "harness", "copilot-sdk-verified.json");
const STEP_MS = 10_000;

const argv = process.argv.slice(2);
const json = argv.includes("--json");
const record = argv.includes("--record");
const flag = (name) => {
	const at = argv.indexOf(name);
	return at >= 0 ? argv[at + 1] : undefined;
};

function finish(report) {
	if (json) console.log(JSON.stringify(report));
	else {
		for (const check of report.checks ?? [])
			console.log(`${check.ok ? "ok  " : "FAIL"} ${check.name}: ${check.detail}`);
		if (report.diagnostic) console.log(`extension stderr:\n${report.diagnostic}`);
		console.log(`copilot-sdk-contract: ${report.status}: ${report.summary}`);
	}
	process.exit(report.status === "fail" ? 1 : 0);
}

/** The CLI version `copilot` runs now, or undefined when it is not installed. */
async function installedVersion() {
	const binary = process.env.COPILOT_BIN || "copilot";
	try {
		const { stdout } = await promisify(execFile)(binary, ["--version"], {
			env: { ...process.env, COPILOT_OFFLINE: "true" },
			encoding: "utf8",
			timeout: 10_000,
		});
		const version = /^GitHub Copilot CLI (\S+)\.$/m.exec(stdout)?.[1];
		if (!version) throw new Error(`${binary} --version printed no version`);
		return version;
	} catch (error) {
		if (error.code === "ENOENT") return undefined;
		throw error;
	}
}

/** Content-Length framed JSON-RPC, as vscode-jsonrpc speaks it over stdio. */
function wire(child, onMessage) {
	let buffer = Buffer.alloc(0);
	child.stdout.on("data", (chunk) => {
		buffer = Buffer.concat([buffer, chunk]);
		for (;;) {
			const end = buffer.indexOf("\r\n\r\n");
			if (end < 0) return;
			const length = Number(/Content-Length: (\d+)/i.exec(buffer.subarray(0, end).toString())?.[1]);
			if (!Number.isFinite(length) || buffer.length < end + 4 + length) return;
			const body = buffer.subarray(end + 4, end + 4 + length).toString();
			buffer = buffer.subarray(end + 4 + length);
			onMessage(JSON.parse(body));
		}
	});
	return (message) => {
		const body = JSON.stringify({ jsonrpc: "2.0", ...message });
		child.stdin.write(`Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`);
	};
}

function within(promise, what) {
	let timer;
	return Promise.race([
		promise,
		new Promise((_, reject) => {
			timer = setTimeout(() => reject(new Error(`timed out waiting for ${what}`)), STEP_MS);
		}),
	]).finally(() => clearTimeout(timer));
}

async function contract(sdkPath) {
	const source = await readFile(sdkPath, "utf8");
	// The CLI of this version answers with the SDK's own protocol version.
	const protocol = Number(/\bSDK_PROTOCOL_VERSION = (\d+);/.exec(source)?.[1]);
	if (!Number.isFinite(protocol)) throw new Error(`${sdkPath} declares no SDK_PROTOCOL_VERSION`);

	const home = await mkdtemp(join(tmpdir(), "pij-sdk-contract-"));
	const sessionId = "00000000-0000-4000-8000-00000000f208";
	const env = {
		...process.env,
		SESSION_ID: sessionId,
		PIJ_SDK_CONTRACT_SDK: sdkPath,
		PIJ_RS_STATE_DIR: home,
		PIJ_RS_ADDR: "127.0.0.1:9",
	};
	for (const name of [
		"TMUX_PANE",
		"TMUX",
		"PIJ_SEAT_ID",
		"PIJ_SPAWN_ID",
		"COPILOT_EXTENSION_INITIALIZATION",
	])
		delete env[name];
	const child = spawn(process.execPath, ["--import", pathToFileURL(HOOK).href, EXTENSION], {
		cwd: home,
		env,
		stdio: ["pipe", "pipe", "pipe"],
	});
	const facts = [];
	const factSeen = new Set();
	// stderr is read by line: a chunk boundary is not a record boundary (R259-001).
	const stderrLines = [];
	createInterface({ input: child.stderr, crlfDelay: Number.POSITIVE_INFINITY }).on(
		"line",
		(line) => {
			if (!line.startsWith("SDK_CONTRACT ")) {
				stderrLines.push(line);
				return;
			}
			const fact = JSON.parse(line.slice("SDK_CONTRACT ".length));
			facts.push(fact);
			for (const listener of factSeen) listener(fact);
		},
	);
	const exited = new Promise((resolve) =>
		child.once("exit", (code, signal) => resolve({ code, signal })),
	);

	let nextId = 1;
	const pending = new Map();
	const requestSeen = new Set();
	const send = wire(child, (message) => {
		if (message.method === undefined) {
			const waiter = pending.get(message.id);
			pending.delete(message.id);
			waiter?.(message);
			return;
		}
		for (const listener of requestSeen) listener(message);
		if (message.id === undefined) return;
		let result = {};
		if (message.method === "connect" || message.method === "ping")
			result = { protocolVersion: protocol };
		if (message.method === "session.resume")
			result = { sessionId: message.params.sessionId, workspacePath: home, capabilities: {} };
		send({ id: message.id, result });
	});
	const request = (method, params) =>
		within(
			new Promise((resolve) => {
				const id = `contract-${nextId++}`;
				pending.set(id, resolve);
				send({ id, method, params });
			}),
			`the SDK to answer ${method}`,
		);
	const awaitRequest = (method, match = () => true) =>
		within(
			new Promise((resolve) => {
				const listener = (message) => {
					if (message.method !== method || !match(message.params)) return;
					requestSeen.delete(listener);
					resolve(message.params);
				};
				requestSeen.add(listener);
			}),
			`the SDK to send ${method}`,
		);
	const awaitFact = (match, what) =>
		within(
			new Promise((resolve) => {
				const listener = (fact) => {
					if (!match(fact)) return;
					factSeen.delete(listener);
					resolve(fact);
				};
				factSeen.add(listener);
				const seen = facts.find(match);
				if (seen) listener(seen);
			}),
			what,
		);
	// Facts ride stderr; answers ride stdout. A stdout answer does not prove the
	// facts written before it have arrived, so after each answer the parent sends a
	// barrier event and waits for the shim's barrier fact on stderr: every fact
	// written before it has arrived by then. A wait on the pipe's own order, not a
	// sleep, and an absent fact costs nothing extra (R259-001).
	let barriers = 0;
	const barrier = async () => {
		const id = `contract-barrier-${++barriers}`;
		send({
			method: "session.event",
			params: {
				sessionId,
				event: {
					id,
					timestamp: new Date().toISOString(),
					parentId: null,
					type: "contract.barrier",
					data: {},
				},
			},
		});
		await awaitFact((fact) => fact.barrier === id, "the child's stderr to catch up");
	};
	// Which of pij's functions ran, by name, wherever the config nests it: the
	// config path is the extension's choice and the SDK's to read, not this check's.
	const calledNamed = (name) =>
		facts.find((fact) => fact.called?.split(".").includes(name))?.called;
	const heard = (type) => facts.some((fact) => fact.event === type);

	const checks = [];
	const check = (name, ok, detail) => checks.push({ name, ok, detail });
	try {
		const resume = await within(
			Promise.race([
				awaitRequest("session.resume"),
				exited.then(({ code, signal }) => {
					throw new Error(
						`the extension exited (${code ?? signal}) before joining: ${stderrLines.slice(-8).join("\n")}`,
					);
				}),
			]),
			"the extension to join its session",
		);
		// The wire form the CLI reads: `tools.definitions` / `hooks.hasHandlers` since
		// 1.0.113, a bare `tools` array / `hooks` boolean before.
		const declared = resume.tools?.definitions ?? (Array.isArray(resume.tools) ? resume.tools : []);
		const tool = declared.find((definition) => definition.name === "pij_send");
		check(
			"tool declared",
			tool?.hasHandler === true,
			tool
				? `the SDK declared pij_send to the CLI (hasHandler ${tool.hasHandler})`
				: `the SDK declared [${declared.map((definition) => definition.name).join(", ")}] to the CLI, not pij_send`,
		);
		const hooksDeclared =
			typeof resume.hooks === "boolean" ? resume.hooks : resume.hooks?.hasHandlers;
		check(
			"hooks declared",
			hooksDeclared === true,
			`the SDK told the CLI its hooks have handlers: ${hooksDeclared}`,
		);

		await awaitFact((fact) => fact.joined === true, "the extension to finish joining");
		const now = Date.now();
		for (const [hookType, handler, input] of [
			["userPromptSubmitted", "onUserPromptSubmitted", { prompt: "contract" }],
			["agentStop", "onAgentStop", { stopReason: "end_turn", stop_hook_active: false }],
		]) {
			const answer = await request("hooks.invoke", {
				sessionId,
				hookType,
				input: { sessionId, timestamp: now, cwd: home, ...input },
			});
			await barrier();
			const reached = calledNamed(handler);
			check(
				`hook ${hookType}`,
				reached !== undefined && answer.error === undefined,
				reached
					? `hooks.invoke ${hookType} reached pij's ${reached}`
					: `hooks.invoke ${hookType} never reached pij's ${handler}${answer.error ? `: ${answer.error.message}` : ""}`,
			);
		}

		const requestId = "contract-tool-call";
		const toolResult = awaitRequest(
			"session.tools.handlePendingToolCall",
			(params) => params?.requestId === requestId,
		);
		send({
			method: "session.event",
			params: {
				sessionId,
				event: {
					id: "contract-event-tool",
					timestamp: new Date(now).toISOString(),
					parentId: null,
					type: "external_tool.requested",
					data: {
						requestId,
						toolCallId: "contract-call",
						toolName: "pij_send",
						arguments: { to: "pij-contract", message: "contract" },
					},
				},
			},
		});
		const answered = await toolResult.catch(() => undefined);
		await barrier();
		const toolHandler = calledNamed("pij_send");
		check(
			"tool call",
			toolHandler !== undefined && answered?.result !== undefined,
			toolHandler && answered
				? `a pij_send call reached pij's ${toolHandler}, which answered ${JSON.stringify(answered.result).slice(0, 120)}`
				: "a pij_send call never reached pij's handler",
		);

		for (const type of ["assistant.message", "session.idle"]) {
			send({
				method: "session.event",
				params: {
					sessionId,
					event: {
						id: `contract-event-${type}`,
						timestamp: new Date(now).toISOString(),
						parentId: null,
						type,
						data:
							type === "assistant.message"
								? { messageId: "contract-message", content: "contract" }
								: {},
					},
				},
			});
		}
		await within(
			new Promise((resolve) => {
				const done = () => heard("assistant.message") && heard("session.idle") && resolve();
				factSeen.add(done);
				done();
			}),
			"pij's event listeners",
		).catch(() => undefined);
		for (const type of ["assistant.message", "session.idle"])
			check(
				`event ${type}`,
				heard(type),
				heard(type)
					? `the ${type} event reached pij's session.on listeners`
					: `the ${type} event never reached pij's listeners`,
			);
	} catch (error) {
		check("contract run", false, error.message);
	} finally {
		child.stdin.end();
		const timer = setTimeout(() => child.kill("SIGKILL"), 3000);
		await exited;
		clearTimeout(timer);
		await rm(home, { recursive: true, force: true });
	}
	// The extension's own stderr ([pij-native] JSON lines, never bodies or keys).
	const diagnostic = stderrLines
		.filter((line) => line.trim() && !line.startsWith("(node:") && !line.startsWith("(Use `node"))
		.slice(-8)
		.join("\n");
	return { checks, diagnostic };
}

let version;
try {
	version = flag("--copilot-version") ?? (await installedVersion());
} catch (error) {
	finish({
		status: "fail",
		summary: `the installed Copilot CLI could not report its version: ${error.message}`,
	});
}
if (!version)
	finish({
		status: "skipped",
		summary: "no Copilot CLI installed (COPILOT_BIN or `copilot` on PATH); nothing to check",
	});
const copilotHome = process.env.COPILOT_HOME || join(homedir(), ".copilot");
const sdk =
	flag("--sdk") ??
	join(
		copilotHome,
		"pkg",
		`${process.platform}-${process.arch}`,
		version,
		"copilot-sdk",
		"extension.js",
	);
let checks;
let diagnostic;
try {
	({ checks, diagnostic } = await contract(sdk));
} catch (error) {
	finish({
		status: "fail",
		version,
		summary: `could not load Copilot ${version}'s SDK: ${error.message}`,
	});
}
const verified = await readFile(RECORD, "utf8")
	.then((text) => JSON.parse(text).version)
	.catch(() => undefined);
const failed = checks.filter((one) => !one.ok);
if (failed.length > 0)
	finish({
		status: "fail",
		version,
		verified,
		checks,
		diagnostic,
		summary: `Copilot ${version}'s SDK would not register pij's ${failed.map((one) => one.name).join(", ")} (last verified ${verified ?? "never"}); compare its copilot-sdk/extension.js resumeSessionForExtension with ${verified ?? "the last good version"}'s`,
	});
if (record) {
	await writeFile(
		RECORD,
		`${JSON.stringify({ version, checks: checks.map((one) => one.name) }, null, "\t")}\n`,
	);
	finish({
		status: "pass",
		version,
		verified: version,
		checks,
		summary: `passed on Copilot ${version}; recorded`,
	});
}
if (verified !== version)
	finish({
		status: "warn",
		version,
		verified,
		checks,
		summary: `passed on Copilot ${version}, but the last recorded pass is ${verified ?? "none"}: run \`just copilot-sdk-contract --record\` and commit harness/copilot-sdk-verified.json`,
	});
finish({ status: "pass", version, verified, checks, summary: `passed on Copilot ${version}` });
