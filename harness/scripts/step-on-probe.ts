#!/usr/bin/env tsx
// Opt-in real clients, real daemon, real model. Never adopt a synthetic seat.
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
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import { nativeComposer } from "./copilot-native-smoke.js";
import { manageCopilotExtension } from "./link-global.js";

type Harness = "omp" | "claude" | "copilot";
type ProbeState = "idle" | "bash" | "subagents";
interface Seat {
	id: string;
	harness: string;
	pane: string;
	session: string;
	native_extension_delivery?: boolean;
}
interface Options {
	harness: Harness;
	state?: ProbeState;
	sourceRoot: string;
	pijBin: string;
	model?: string;
	thinking?: string;
	observeMs: number;
	timeoutMs: number;
	output?: string;
	sendKeysOnly: boolean;
}
const ROOT = resolve(import.meta.dirname, "../..");
const DEFAULT_PROBE_KEYBINDINGS = { submit: "Enter", nextChoice: "Down" } as const;
const sha = (text: string) => createHash("sha256").update(text).digest("hex");
const command = (bin: string, args: string[], env?: NodeJS.ProcessEnv) =>
	execFileSync(bin, args, {
		env,
		encoding: "utf8",
		timeout: 15_000,
		stdio: ["ignore", "pipe", "pipe"],
	});
const object = (value: unknown): Record<string, unknown> => {
	assert(value && typeof value === "object" && !Array.isArray(value));
	return value as Record<string, unknown>;
};

export function parseProbeArgs(args: string[]): Options {
	const options: Options = {
		harness: "omp",
		state: "idle",
		sourceRoot: ROOT,
		pijBin: join(ROOT, "target/debug/pij-rs"),
		observeMs: 10_000,
		timeoutMs: 90_000,
		sendKeysOnly: false,
	};
	for (let i = 0; i < args.length; i++) {
		const arg = args[i];
		const value = args[++i];
		assert(value, `missing value for ${arg}`);
		if (arg === "--harness") {
			assert(["omp", "claude", "copilot"].includes(value));
			options.harness = value as Harness;
		} else if (arg === "--state") {
			assert(["idle", "bash", "subagents"].includes(value), `invalid probe state ${value}`);
			options.state = value as ProbeState;
		} else if (arg === "--source-root") options.sourceRoot = resolve(value);
		else if (arg === "--pij-bin") options.pijBin = resolve(value);
		else if (arg === "--model") options.model = value;
		else if (arg === "--thinking") options.thinking = value;
		else if (arg === "--output") options.output = resolve(value);
		else if (arg === "--observe-ms") options.observeMs = Number(value);
		else if (arg === "--timeout-ms") options.timeoutMs = Number(value);
		else if (arg === "--send-keys-only") {
			assert(value === "true" || value === "false");
			options.sendKeysOnly = value === "true";
		} else throw new Error(`unknown option ${arg}`);
	}
	assert(Number.isFinite(options.observeMs) && options.observeMs >= 5000);
	assert(Number.isFinite(options.timeoutMs) && options.timeoutMs > 0);
	assert(
		!options.sendKeysOnly || options.harness === "claude",
		"send-keys control uses a real Claude pane with daemon socket discovery isolated",
	);
	assert(
		options.state === "idle" || options.harness === "omp",
		"busy probe states require --harness omp",
	);
	assert(
		options.thinking === undefined || options.harness === "omp",
		"--thinking requires --harness omp",
	);
	return options;
}

export function composerText(
	harness: Harness,
	viewport: string,
	cursor?: { x: number; y: number },
): string | undefined {
	if (harness === "copilot" && !cursor) return nativeComposer(viewport);
	const lines = viewport.split("\n");
	const borders = lines.flatMap((line, i) =>
		(harness === "copilot" ? /^(?:╻▄+|╹▀+)/u : /^[─━]{10}/u).test(line.trim()) ? [i] : [],
	);
	if (borders.length < 2) return undefined;
	const bottom = borders.at(-1) as number;
	const top = borders.at(-2) as number;
	const body = lines.slice(top + 1, bottom);
	const prefix = harness === "copilot" ? /^┃ /u : /^[❯>›][ \u00a0]?/u;
	const first = body[0] ?? "";
	const prompt = prefix.exec(first)?.[0];
	if (!prompt) return undefined;
	if (cursor) {
		// The probe types one ASCII line and never moves its cursor. Cursor bounds
		// distinguish trailing draft spaces from the terminal's blank right padding.
		if (
			body.length !== 1 ||
			cursor.y !== top + 1 ||
			cursor.x < prompt.length ||
			cursor.x > first.length ||
			!/^ *$/.test(first.slice(cursor.x))
		)
			return undefined;
		return first.slice(prompt.length, cursor.x);
	}
	return body
		.map((line, i) => (i === 0 ? line.replace(/^[❯>›][ \u00a0]/u, "") : line).trimEnd())
		.join("\n");
}

async function until<T>(
	label: string,
	ms: number,
	sample: () => Promise<T | undefined> | T | undefined,
): Promise<T> {
	const end = Date.now() + ms;
	while (Date.now() < end) {
		const result = await sample();
		if (result !== undefined) return result;
		await delay(100);
	}
	throw new Error(`step-on probe timeout: ${label}`);
}
async function freePort(): Promise<number> {
	const server = createServer();
	await new Promise<void>((done, reject) => {
		server.once("error", reject);
		server.listen(0, "127.0.0.1", done);
	});
	const address = server.address();
	assert(address && typeof address !== "string");
	await new Promise<void>((done, reject) =>
		server.close((error) => (error ? reject(error) : done())),
	);
	return address.port;
}
function jsonlFiles(root: string): string[] {
	if (!existsSync(root)) return [];
	return readdirSync(root, { withFileTypes: true }).flatMap((entry) =>
		entry.isDirectory()
			? jsonlFiles(join(root, entry.name))
			: entry.name.endsWith(".jsonl")
				? [join(root, entry.name)]
				: [],
	);
}
function transcriptEntries(paths: string[]): Record<string, unknown>[] {
	return paths.flatMap((path) =>
		readFileSync(path, "utf8")
			.split("\n")
			.filter(Boolean)
			.flatMap((line) => {
				try {
					return [object(JSON.parse(line))];
				} catch {
					return [];
				}
			}),
	);
}

export function parentTranscript(paths: string[], session: string): string | undefined {
	return paths.find((path) =>
		transcriptEntries([path]).some((entry) => entry.type === "session" && entry.id === session),
	);
}

export function transcriptToolCalls(path: string): Record<string, unknown>[] {
	return transcriptEntries([path]).flatMap((entry) => {
		const message = entry.message;
		if (!message || typeof message !== "object") return [];
		const record = object(message);
		if (record.role !== "assistant" || !Array.isArray(record.content)) return [];
		return record.content.flatMap((part) => {
			if (!part || typeof part !== "object" || object(part).type !== "toolCall") return [];
			return [{ ...object(part), timestamp: entry.timestamp }];
		});
	});
}

/** Native pij context must enter directly after the original blocking tool,
 * not through a child-result notice or a follow-up drain after a terminal reply. */
export function parentSteeringContext(path: string, nonce: string, command: string): boolean {
	let previousAssistant: Record<string, unknown> | undefined;
	for (const entry of transcriptEntries([path])) {
		const message = entry.message;
		if (message && typeof message === "object" && object(message).role === "assistant") {
			previousAssistant = object(message);
		}
		if (
			entry.type !== "custom_message" ||
			entry.customType !== "pij" ||
			typeof entry.content !== "string" ||
			!entry.content.startsWith("[pij-rs from pij-step-on-probe]\n") ||
			!entry.content.includes(nonce)
		)
			continue;
		const content = previousAssistant?.content;
		return (
			Array.isArray(content) &&
			content.some((part) => {
				if (!part || typeof part !== "object") return false;
				const call = object(part);
				return (
					call.type === "toolCall" &&
					call.name === "bash" &&
					call.arguments !== null &&
					typeof call.arguments === "object" &&
					object(call.arguments).command === command
				);
			})
		);
	}
	return false;
}

export function busyProbePlan(state: "bash" | "subagents", run: string) {
	const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
	const work = (name: string, seconds: number) => {
		const started = join(run, `${name}.started`);
		const done = join(run, `${name}.done`);
		const script = join(run, `${name}.sh`);
		return {
			started,
			done,
			script,
			scriptContent: `set -e\nprintf started > ${shellQuote(started)}\n/bin/sleep ${seconds}\nprintf done > ${shellQuote(done)}\n`,
			command: `/bin/sh ${shellQuote(script)}`,
		};
	};
	const first = work("parent-first", state === "bash" ? 30 : 45);
	const remaining = work("parent-remaining", 10);
	const children = state === "subagents" ? [work("child-one", 30), work("child-two", 30)] : [];
	const doneMarker = `WORK_DONE_${randomUUID()}`;
	const childTasks = children.map((child, index) => ({
		name: `ProbeChild${index + 1}`,
		task: `# Target\nOwned sleep fixture ${child.script}. No file edits.\n# Change\nUse bash to run exactly ${child.command} with timeout 90. Do not delegate or run validation.\n# Acceptance\nAfter the command completes, return DONE.`,
	}));
	const prompt = [
		"This is an authorized isolated delivery probe. The harness created these sleep/marker fixtures in its own scratch directory. Invoke each as one external binary via bash; no inline shell scripts or manual file inspection are needed.",
		...(children.length
			? [
					`First launch both children together with task using these arguments: ${JSON.stringify({ context: "# Goal\nRun two owned sleep fixtures concurrently.\n# Constraints\nNo file edits, no validation, no further delegation.\n# Contract\nEach child runs its given external command and returns DONE.", tasks: childTasks })}. Then run your own bash command immediately, without waiting for the children.`,
				]
			: []),
		`Run bash with timeout 90 and this exact command: ${first.command}`,
		`After that bash call finishes, make a SECOND separate bash call with timeout 90 and command: ${remaining.command}. Do not batch the two bash calls.`,
		`After all requested commands, including the children, finish, reply exactly ${doneMarker}. Do not cancel, restart, combine, or background the shell commands.`,
	].join(" ");
	return { first, remaining, children, doneMarker, prompt };
}

export function transcriptTexts(paths: string[], roles: string[]): string[] {
	return transcriptEntries(paths).flatMap((entry) => {
		const nativeRole =
			entry.type === "user.message"
				? "user"
				: entry.type === "assistant.message"
					? "assistant"
					: entry.type === "custom_message"
						? "custom"
						: undefined;
		const message =
			nativeRole === "custom"
				? entry
				: nativeRole
					? object(entry.data)
					: entry.message && typeof entry.message === "object"
						? object(entry.message)
						: undefined;
		if (!message || !roles.includes(String(nativeRole ?? message.role))) return [];
		if (typeof message.content === "string") return [message.content];
		if (!Array.isArray(message.content)) return [];
		return [
			message.content
				.map((part) => (typeof object(part).text === "string" ? object(part).text : ""))
				.join("\n"),
		];
	});
}

export async function runProbe(options: Options): Promise<{ code: number; output: string }> {
	assert.equal(
		process.env.PIJ_STEP_ON_REAL,
		"1",
		"real-resource probe requires PIJ_STEP_ON_REAL=1",
	);
	const probeState = options.state ?? "idle";
	assert(probeState === "idle" || options.harness === "omp", "busy probe states require omp");
	const run = realpathSync(mkdtempSync(join(tmpdir(), "pij-step-on-")));
	const output = options.output ?? join(run, "receipt.json");
	assert(!existsSync(output), `refusing to overwrite ${output}`);
	const state = join(run, "daemon");
	const cwd = join(run, "workspace");
	const home = join(run, "home");
	const bin = join(run, "bin");
	const sessions = join(run, "sessions");
	const socket = join(run, "tmux.sock");
	for (const dir of [state, cwd, home, bin, sessions])
		mkdirSync(dir, { recursive: true, mode: 0o700 });
	symlinkSync(options.pijBin, join(bin, "pij-rs"));
	const addr = `127.0.0.1:${await freePort()}`;
	const env: NodeJS.ProcessEnv = { ...process.env };
	for (const key of Object.keys(env))
		if (
			/^(?:PIJ_|TMUX|CLAUDECODE|CLAUDE_CODE_SESSION_ID|OMP_PROFILE|PI_CODING_AGENT_DIR)/.test(key)
		)
			delete env[key];
	Object.assign(env, {
		PATH: `${bin}:${env.PATH}`,
		PIJ_RS_STATE_DIR: state,
		PIJ_RS_ADDR: addr,
		PIJ_DAEMON_GENERATION: "rs",
		NO_PROXY: "127.0.0.1,localhost",
	});
	const serverEnv = { ...env };
	const tmux = (args: string[]) => command("tmux", ["-S", socket, ...args], env);
	let daemon: ChildProcess | undefined;
	let daemonLog = "";
	let created = false;
	let pane: string | undefined;
	let code = 1;
	const receipt: Record<string, unknown> = {
		harness: options.harness,
		state: probeState,
		run,
		source_root: options.sourceRoot,
		source_commit: command("git", ["-C", options.sourceRoot, "rev-parse", "HEAD"]).trim(),
		binary_sha256: createHash("sha256").update(readFileSync(options.pijBin)).digest("hex"),
		isolation: { state, addr, socket, cwd },
		started: new Date().toISOString(),
	};
	const api = async (path: string, body?: unknown): Promise<unknown> => {
		const response = await fetch(`http://${addr}${path}`, {
			method: body === undefined ? "GET" : "POST",
			headers: {
				Authorization: `Bearer ${readFileSync(join(state, "daemon.key"), "utf8").trim()}`,
				"Content-Type": "application/json",
			},
			body: body === undefined ? undefined : JSON.stringify(body),
			signal: AbortSignal.timeout(5000),
		});
		const envelope = object(await response.json());
		assert(response.ok && envelope.ok === true, JSON.stringify(envelope));
		return envelope.data;
	};
	try {
		tmux([
			"-f",
			"/dev/null",
			"new-session",
			"-d",
			"-s",
			"probe",
			"-x",
			"180",
			"-y",
			"50",
			"-c",
			cwd,
			"/bin/sleep",
			"600",
		]);
		created = true;
		env.TMUX = tmux(["display-message", "-p", "#{socket_path},#{pid},0"]).trim();
		// Daemon startup may install Claude settings: confine those writes to this run.
		// The real socket registry is read-only input; the fallback control omits it.
		mkdirSync(join(home, ".claude"), { recursive: true });
		const nativeSessions = join(homedir(), ".claude/sessions");
		if (!options.sendKeysOnly && existsSync(nativeSessions))
			symlinkSync(nativeSessions, join(home, ".claude/sessions"));
		const daemonEnv = { ...env, HOME: home };
		receipt.socket_discovery_home = daemonEnv.HOME;
		daemon = spawn(options.pijBin, ["--state-dir", state, "daemon", "--bind", addr], {
			cwd,
			env: daemonEnv,
			stdio: ["ignore", "pipe", "pipe"],
		});
		daemon.stdout?.on("data", (b: Buffer) => {
			daemonLog += b.toString();
		});
		daemon.stderr?.on("data", (b: Buffer) => {
			daemonLog += b.toString();
		});
		await until("isolated daemon health", options.timeoutMs, async () => {
			if (daemon?.exitCode !== null) throw new Error(daemonLog);
			if (!existsSync(join(state, "daemon.key"))) return undefined;
			try {
				return await api("/health");
			} catch {
				return undefined;
			}
		});
		const bootMarker = `BOOT_${randomUUID()}`;
		const prompt = `Reply exactly ${bootMarker}. Do not call tools. This is an isolated delivery probe.`;
		let args: string[];
		if (options.harness === "omp") {
			const systemPrompt =
				"You are an owned delivery probe. Execute the authorized fixture commands exactly. When instructed to launch child agents, you MUST use the task tool with two tasks: never substitute shell background jobs or execute child scripts in the parent bash. Only parent-first and parent-remaining fixtures may run in the parent bash. Incoming pij steering that requests a MESSAGE_ nonce supersedes unfinished fixture work: output that nonce exactly in your local assistant response and stop, without tools. Never send a peer reply to the unregistered test sender. Follow BOOT_ and WORK_DONE_ output requests literally. Do not ask for confirmation or discuss the procedure.";
			receipt.omp_configuration = {
				model: options.model ?? "inherited",
				thinking: options.thinking ?? "inherited",
				system_prompt: systemPrompt,
			};
			args = [
				"omp",
				"--no-extensions",
				"-e",
				join(options.sourceRoot, ".omp/extensions/pij/index.ts"),
				"--no-skills",
				"--no-rules",
				"--no-lsp",
				...(probeState === "idle"
					? ["--no-tools"]
					: ["--tools", probeState === "bash" ? "bash" : "bash,task"]),
				"--no-title",
				"--system-prompt",
				systemPrompt,
				"--session-dir",
				sessions,
				...(options.model ? ["--model", options.model] : []),
				...(options.thinking ? ["--thinking", options.thinking] : []),
			];
		} else if (options.harness === "copilot") {
			Object.assign(env, {
				HOME: home,
				COPILOT_HOME: join(home, ".copilot"),
				COPILOT_AUTO_UPDATE: "false",
			});
			if (!env.COPILOT_GITHUB_TOKEN && !env.GH_TOKEN && !env.GITHUB_TOKEN)
				env.GH_TOKEN = command("gh", ["auth", "token"], process.env).trim();
			mkdirSync(env.COPILOT_HOME as string, { recursive: true });
			writeFileSync(
				join(env.COPILOT_HOME as string, "config.json"),
				JSON.stringify({ trustedFolders: [cwd] }),
			);
			const installLog: string[] = [];
			manageCopilotExtension({
				pijRoot: options.sourceRoot,
				home,
				copilotHome: env.COPILOT_HOME,
				args: [],
				stdout: (line) => installLog.push(line),
				stderr: (line) => installLog.push(line),
			});
			receipt.install = installLog;
			args = [
				"copilot",
				"--no-custom-instructions",
				"--disable-builtin-mcps",
				"--no-remote",
				"--no-auto-update",
				...(options.model ? ["--model", options.model] : []),
			];
		} else {
			const settings = join(run, "claude-settings.json");
			writeFileSync(
				settings,
				JSON.stringify({
					crossSessionInbound: "accept",
					hooks: {
						SessionStart: [
							{
								hooks: [
									{
										type: "command",
										command: join(
											options.sourceRoot,
											"harness/scripts/claude-session-start-pij.sh",
										),
									},
								],
							},
						],
					},
				}),
			);
			args = [
				"claude",
				"--setting-sources",
				"",
				"--settings",
				settings,
				"--strict-mcp-config",
				"--tools",
				"",
				...(options.model ? ["--model", options.model] : []),
			];
		}
		// Keep credentials out of the client's argv; update only this owned server.
		for (const [key, value] of Object.entries(env)) {
			if (key !== "TMUX" && value !== undefined && value !== serverEnv[key]) {
				try {
					tmux(["set-environment", "-g", key, value]);
				} catch {
					throw new Error(`could not set owned tmux environment key ${key}`);
				}
			}
		}
		pane = tmux([
			"new-window",
			"-d",
			"-P",
			"-F",
			"#{pane_id}",
			"-t",
			"probe",
			"-c",
			cwd,
			...args,
		]).trim();
		receipt.pane = pane;
		const capture = () => tmux(["capture-pane", "-p", "-t", pane as string]);
		let unreadableComposerFrames = 0;
		const captureComposer = () =>
			until("readable composer snapshot", 1000, () => {
				const raw = tmux([
					"capture-pane",
					"-p",
					"-N",
					"-t",
					pane as string,
					";",
					"display-message",
					"-p",
					"-t",
					pane as string,
					"__PIJ_CURSOR__#{cursor_x},#{cursor_y}",
				]);
				const marker = /__PIJ_CURSOR__(\d+),(\d+)\n?$/.exec(raw);
				assert(marker, "missing paired composer cursor");
				const viewport = raw.slice(0, marker.index);
				const cursor = { x: Number(marker[1]), y: Number(marker[2]) };
				const text = composerText(options.harness, viewport, cursor);
				if (text === undefined) {
					receipt.composer_snapshot_retries = ++unreadableComposerFrames;
					return undefined;
				}
				return { viewport, cursor, text };
			});
		const seat = await until("client self-registration", options.timeoutMs, async () => {
			const terminal = capture();
			if (
				options.harness === "claude" &&
				terminal.includes(cwd) &&
				/❯\s+No, exit/u.test(terminal) &&
				terminal.includes("Yes, I trust this folder")
			) {
				tmux([
					"send-keys",
					"-t",
					pane as string,
					DEFAULT_PROBE_KEYBINDINGS.nextChoice,
					DEFAULT_PROBE_KEYBINDINGS.submit,
				]);
				receipt.workspace_trust = "accepted-owned-scratch-only";
			}
			const data = await api("/v1/seats");
			const list = Array.isArray(data) ? data : object(data).seats;
			assert(Array.isArray(list));
			return (list as Seat[]).find((row) => row.pane === pane);
		});
		receipt.seat = seat;
		if (options.harness === "omp")
			await until("OMP boot turn finished", options.timeoutMs, () =>
				transcriptTexts(jsonlFiles(sessions), ["assistant"]).length > 0 ? true : undefined,
			);
		await until("composer ready", options.timeoutMs, () =>
			composerText(options.harness, capture()) !== undefined ? true : undefined,
		);
		tmux(["send-keys", "-t", pane, "-l", prompt]);
		tmux(["send-keys", "-t", pane, DEFAULT_PROBE_KEYBINDINGS.submit]);
		await until("boot response and idle composer", options.timeoutMs, () => {
			const terminal = capture();
			return terminal.split(bootMarker).length >= 3 &&
				composerText(options.harness, terminal) !== undefined
				? true
				: undefined;
		});
		await delay(options.sendKeysOnly ? 5000 : 1000);
		const busyWork = probeState === "idle" ? undefined : busyProbePlan(probeState, run);
		let parentPath: string | undefined;
		let sessionIntact = true;
		const identity: Record<string, unknown> = { before: seat };
		const currentSeat = async () => {
			const data = await api("/v1/seats");
			const list = Array.isArray(data) ? data : object(data).seats;
			assert(Array.isArray(list));
			return (list as Seat[]).find((row) => row.pane === pane);
		};
		if (busyWork) {
			for (const work of [busyWork.first, busyWork.remaining, ...busyWork.children]) {
				writeFileSync(work.script, work.scriptContent, { mode: 0o600, flag: "wx" });
			}
			assert(seat.session, "OMP root seat has no native session identity");
			receipt.parent_session = identity;
			parentPath = await until("original OMP parent transcript", options.timeoutMs, () =>
				parentTranscript(jsonlFiles(sessions), seat.session),
			);
			receipt.busy_work = { ...busyWork, parent_transcript: parentPath };
			tmux(["send-keys", "-t", pane, "-l", busyWork.prompt]);
			tmux(["send-keys", "-t", pane, DEFAULT_PROBE_KEYBINDINGS.submit]);
			const calls = await until(
				"parent tool calls and running busy work",
				options.timeoutMs,
				() => {
					const work = [busyWork.first, ...busyWork.children];
					assert(
						!work.some((item) => existsSync(item.done)),
						"busy work finished before injection",
					);
					const calls = transcriptToolCalls(parentPath as string);
					const parentStarted = calls.some(
						(call) =>
							call.name === "bash" && object(call.arguments).command === busyWork.first.command,
					);
					const fanoutStarted =
						!busyWork.children.length ||
						calls.some((call) => {
							if (call.name !== "task") return false;
							const tasks = object(call.arguments).tasks;
							return (
								Array.isArray(tasks) &&
								tasks.length === 2 &&
								busyWork.children.every((child) =>
									tasks.some((task) => String(object(task).task).includes(child.command)),
								)
							);
						});
					return parentStarted && fanoutStarted && work.every((item) => existsSync(item.started))
						? calls
						: undefined;
				},
			);
			receipt.busy_started = { at: new Date().toISOString(), tool_calls: calls };
			const afterFanout = await currentSeat();
			identity.after_fanout = afterFanout ?? null;
			sessionIntact = afterFanout?.id === seat.id && afterFanout.session === seat.session;
			assert(sessionIntact, "OMP root seat native session changed during busy fanout");
		}
		const draftNonce = `UNSUBMITTED_${randomUUID()}`;
		const draft = ` ${draftNonce}_keep spaces  exactly `;
		tmux(["send-keys", "-t", pane, "-l", draft]);
		const before = await until("draft visible in composer", 5000, async () => {
			const snapshot = await captureComposer();
			return snapshot.text === draft ? snapshot : undefined;
		});
		receipt.before = { draft, sha256: sha(draft), ...before };
		const nonce = `MESSAGE_${randomUUID()}`;
		const msgId = randomUUID();
		if (busyWork)
			assert(
				![busyWork.first, ...busyWork.children].some((item) => existsSync(item.done)),
				"busy work finished before message send",
			);
		const sentAt = Date.now();
		receipt.send = await api("/v1/send", {
			from: "pij-step-on-probe",
			to: { seat: seat.id },
			body: busyWork
				? `Steering replaces the remaining work: reply locally with exactly ${nonce}. Do not call any tools. The sender pij-step-on-probe is an unregistered test address; do not send it a peer reply.`
				: `Reply exactly ${nonce}. Do not call tools.`,
			msg_id: msgId,
		});
		const transcriptPaths = () =>
			options.harness === "omp"
				? parentPath
					? [parentPath]
					: jsonlFiles(sessions)
				: options.harness === "copilot"
					? [join(home, ".copilot/session-state", seat.session, "events.jsonl")]
					: jsonlFiles(join(homedir(), ".claude/projects", cwd.replace(/[^a-zA-Z0-9]/g, "-")));
		let deliveredWithin: number | null = null;
		let intact = true;
		let submitted = false;
		let reached = false;
		let replied = false;
		let repliedWithin: number | null = null;
		let repliedBeforeRemainingWorkDone = false;
		const observationMs = busyWork
			? Math.max(options.observeMs, options.timeoutMs)
			: options.observeMs;
		const samples: Record<string, unknown>[] = [];
		while (Date.now() - sentAt < observationMs) {
			const snapshot = await captureComposer();
			const composer = snapshot.text;
			const paths = transcriptPaths().filter(existsSync);
			const texts = transcriptTexts(paths, ["user", "custom"]);
			reached ||= busyWork
				? parentSteeringContext(parentPath as string, nonce, busyWork.first.command)
				: texts.some((text) => text.includes(nonce));
			submitted ||= transcriptTexts(paths, ["user"]).some((text) => text.includes(draftNonce));
			const replies = transcriptTexts(paths, ["assistant"]);
			replied ||= replies.some((text) => text.trim() === nonce);
			receipt.model_replies = replies;
			if (reached && deliveredWithin === null) deliveredWithin = Date.now() - sentAt;
			if (replied && repliedWithin === null) repliedWithin = Date.now() - sentAt;
			let busySample: Record<string, unknown> | undefined;
			if (busyWork) {
				const firstFinished = existsSync(busyWork.first.done);
				const remainingFinished = existsSync(busyWork.remaining.done);
				repliedBeforeRemainingWorkDone ||= replied && !remainingFinished;
				const latestSeat = await currentSeat();
				sessionIntact &&= latestSeat?.id === seat.id && latestSeat.session === seat.session;
				identity.after_delivery = latestSeat ?? null;
				busySample = {
					first_work_finished: firstFinished,
					remaining_work_finished: remainingFinished,
					native_session: latestSeat?.session ?? null,
				};
			}
			intact &&= composer === draft;
			samples.push({
				elapsed_ms: Date.now() - sentAt,
				composer,
				cursor: snapshot.cursor,
				sha256: composer === undefined ? null : sha(composer),
				context_accepted: reached,
				model_replied: replied,
				submitted,
				...(busySample ? { busy: busySample } : {}),
			});
			if (busyWork && replied && reached && Date.now() - sentAt >= options.observeMs) break;
			await delay(200);
		}
		const db = new DatabaseSync(join(state, "pij.sqlite"), { readOnly: true });
		try {
			receipt.spine = db
				.prepare(
					"SELECT seq, at, kind, seat, payload FROM spine_events WHERE seat = ? ORDER BY seq",
				)
				.all(seat.id);
			receipt.jobs = db
				.prepare("SELECT id, state, payload FROM jobs WHERE serial_key = ? AND dedupe_key = ?")
				.all(seat.id, msgId);
		} finally {
			db.close();
		}
		receipt.samples = samples;
		const after = await captureComposer();
		intact &&= after.text === draft;
		receipt.after = after;
		receipt.transcripts = transcriptPaths();
		const acknowledged =
			!!busyWork &&
			(receipt.spine as Record<string, unknown>[]).some((row) => {
				if (row.kind !== "delivery.inbox-ack") return false;
				const payload = object(JSON.parse(String(row.payload)));
				if (payload.outcome !== "reader-read") return false;
				return (receipt.jobs as Record<string, unknown>[]).some((job) => job.id === payload.job_id);
			});
		// Transport latency ends at native context acceptance, not provider inference.
		// The independent reply nonce is nevertheless REQUIRED for model-received proof.
		receipt.delivery_timing_basis = busyWork
			? "parent native context acceptance at tool boundary; nonce reply before separate remaining work completes; inbox ACK and stable parent session required"
			: "native transcript context acceptance; model reply independently required";
		const rung = options.sendKeysOnly
			? "typed-body"
			: options.harness === "omp"
				? "extension-stream"
				: options.harness === "copilot"
					? "native"
					: "socket";
		receipt.verdict = {
			harness: options.harness,
			state: probeState,
			transport_rung: rung,
			draft_intact: intact,
			message_reached_model: replied,
			draft_submitted: submitted,
			delivered_within_ms: deliveredWithin,
			...(busyWork
				? {
						model_replied_within_ms: repliedWithin,
						replied_before_remaining_work_done: repliedBeforeRemainingWorkDone,
						parent_session_unchanged: sessionIntact,
						inbox_acknowledged: acknowledged,
					}
				: {}),
		};
		const held = (receipt.spine as Record<string, unknown>[]).some(
			(row) =>
				row.kind === "delivery.held" && /human-typing|composer-busy/.test(String(row.payload)),
		);
		code = options.sendKeysOnly
			? intact && !reached && !replied && !submitted && held
				? 0
				: 1
			: intact &&
					reached &&
					replied &&
					!submitted &&
					deliveredWithin !== null &&
					(busyWork
						? repliedBeforeRemainingWorkDone && sessionIntact && acknowledged
						: deliveredWithin <= 5000)
				? 0
				: 1;
	} catch (error) {
		receipt.error = error instanceof Error ? error.message : String(error);
	} finally {
		if (created) {
			try {
				receipt.final_panes = tmux([
					"list-panes",
					"-a",
					"-F",
					"#{pane_id} #{pane_current_command}",
				]);
				if (pane) receipt.final_viewport = tmux(["capture-pane", "-p", "-t", pane]);
			} catch (error) {
				receipt.cleanup_error = String(error);
				code = 1;
			}
			try {
				tmux(["kill-server"]);
			} catch (error) {
				receipt.teardown_error = String(error);
				code = 1;
			}
		}
		if (daemon && daemon.exitCode === null) {
			daemon.kill("SIGTERM");
			await Promise.race([
				new Promise<void>((done) => daemon?.once("exit", () => done())),
				delay(3000),
			]);
			if (daemon.exitCode === null) daemon.kill("SIGKILL");
		}
		receipt.daemon_log = daemonLog;
		receipt.exit_code = code;
		writeFileSync(output, `${JSON.stringify(receipt, null, 2)}\n`, { mode: 0o600, flag: "wx" });
	}
	return { code, output };
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
	runProbe(parseProbeArgs(process.argv.slice(2)))
		.then((result) => {
			console.log(JSON.stringify(result));
			process.exitCode = result.code;
		})
		.catch((error) => {
			console.error(error);
			process.exitCode = 1;
		});
}
