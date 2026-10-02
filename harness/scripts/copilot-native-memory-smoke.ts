#!/usr/bin/env tsx
// AC6: real Copilot host + worktree extension + Rust daemon; only model inference is a local fixture.
import { strict as assert } from "node:assert";
import { type ChildProcess, execFileSync, spawn, spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
	appendFileSync,
	closeSync,
	cpSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	openSync,
	readFileSync,
	readSync,
	realpathSync,
	rmSync,
	statSync,
	writeFileSync,
	writeSync,
} from "node:fs";
import { createServer } from "node:net";
import { basename, isAbsolute, join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { StringDecoder } from "node:string_decoder";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import { object, smokeEnvironment, startFixtureProvider } from "./copilot-native-fixture.js";
import {
	finishSmokePanes,
	nativeMessages,
	readNativeEvents,
	stopSmokeNativeHost,
	verifyNativeLifecycleIdentity,
} from "./copilot-native-smoke.js";
import { manageCopilotExtension } from "./link-global.js";

const ROOT = resolve(import.meta.dirname, "../..");
const EVIDENCE = join(ROOT, "scratch/evidence/native-extension-memory");
const SAMPLE_MS = 10_000;
// Use the shipped lease, never accelerate the AC5 witness with a test-only duration.
const LEASE_MS = 60_000;
const DEAD_REASON = "native-extension-unavailable";

interface Options {
	pijBin: string;
	copilotBin: string;
	copilotRuntimeDir?: string;
	output: string;
	messages: number;
	messageIntervalMs: number;
	timeoutMs: number;
	extensionRoot?: string;
	fixture?: Parameters<typeof startFixtureProvider>[0];
	scenario?: (context: NativeSmokeContext) => Promise<void>;
}
interface Seat {
	id: string;
	harness: string;
	session: string;
	pane: string;
	proc: { pid: number; proc_start: number };
	native_extension_delivery?: boolean;
}
interface NativeEvent {
	id: string;
	type: string;
	data: Record<string, unknown>;
}
interface ProcessIdentity {
	pid: number;
	ppid: number;
	rss_kib: number;
	started: string;
	command: string;
}

/** Reuse the isolated host lifecycle for receiver failure scenarios, never production seats. */
export interface NativeSmokeContext {
	launch: (
		resume?: Seat,
		prompt?: string,
	) => Promise<{
		seat: Seat;
		host: ProcessIdentity;
		extension: ProcessIdentity;
	}>;
	api: (path: string, body?: unknown) => Promise<unknown>;
	rows: (sql: string, ...params: string[]) => Record<string, unknown>[];
	tmux: (args: string[], preserve?: boolean) => string;
	until: <T>(
		label: string,
		timeout: number,
		probe: () => T | undefined | Promise<T | undefined>,
	) => Promise<T>;
	env: NodeJS.ProcessEnv;
	cwd: string;
	state: string;
	copilotHome: string;
	output: string;
	pijBin: string;
	addr: string;
	receipt: Record<string, unknown>;
}

function optionsFrom(args: string[]): Options {
	const options: Options = {
		pijBin: join(ROOT, "target/debug/pij-rs"),
		copilotBin: "copilot",
		output: join(
			EVIDENCE,
			`ac6-${new Date().toISOString().replaceAll(":", "-")}-${randomUUID().slice(0, 8)}`,
		),
		messages: 30,
		messageIntervalMs: 10_000,
		timeoutMs: 90_000,
	};
	for (let index = 0; index < args.length; index += 2) {
		const value = args[index + 1];
		assert(value, `missing value for ${args[index]}`);
		switch (args[index]) {
			case "--pij-bin":
				options.pijBin = resolve(value);
				break;
			case "--copilot-bin":
				options.copilotBin = value;
				break;
			case "--copilot-runtime-dir":
				options.copilotRuntimeDir = resolve(value);
				break;
			case "--output-dir":
				options.output = resolve(value);
				break;
			case "--messages":
				options.messages = Number(value);
				break;
			case "--message-interval-seconds":
				options.messageIntervalMs = Number(value) * 1000;
				break;
			case "--timeout-seconds":
				options.timeoutMs = Number(value) * 1000;
				break;
			default:
				throw new Error(`unknown argument ${args[index]}`);
		}
	}
	assert(
		Number.isSafeInteger(options.messages) && options.messages >= 30,
		"--messages must be >=30",
	);
	assert(
		Number.isFinite(options.messageIntervalMs) && options.messageIntervalMs >= 1000,
		"message interval must be >=1 second",
	);
	assert(
		Number.isFinite(options.timeoutMs) && options.timeoutMs > 0 && options.timeoutMs <= 600_000,
		"timeout must be 1..600 seconds",
	);
	assert(
		options.output.startsWith(`${EVIDENCE}/`),
		"artifacts must remain in the evidence directory",
	);
	assert(
		options.pijBin.startsWith(`${ROOT}/target/`),
		"use this worktree's newly built Rust binary, never a global binary",
	);
	return options;
}

function command(bin: string, args: string[], env: NodeJS.ProcessEnv, preserve = false): string {
	const text = execFileSync(bin, args, {
		env,
		encoding: "utf8",
		timeout: 15_000,
		stdio: ["ignore", "pipe", "pipe"],
	});
	return preserve ? text : text.trim();
}
async function freePort(): Promise<number> {
	const server = createServer();
	await new Promise<void>((ok, fail) => {
		server.once("error", fail);
		server.listen(0, "127.0.0.1", ok);
	});
	const address = server.address();
	assert(address && typeof address !== "string");
	await new Promise<void>((ok, fail) => server.close((error) => (error ? fail(error) : ok())));
	assert.notEqual(address.port, 7461, "production daemon is never a proof target");
	return address.port;
}
function sha(path: string): string {
	return createHash("sha256").update(readFileSync(path)).digest("hex");
}
async function stopOwned(child: ChildProcess): Promise<void> {
	if (!child.pid || child.exitCode !== null || child.signalCode !== null) return;
	const exited = new Promise<void>((ok) => child.once("exit", () => ok()));
	child.kill("SIGINT");
	const timer = setTimeout(() => child.kill("SIGKILL"), 5000);
	try {
		await exited;
	} finally {
		clearTimeout(timer);
	}
}
function parseProcess(text: string): ProcessIdentity {
	const match =
		/^\s*(\d+)\s+(\d+)\s+(\d+)\s+(\S+\s+\S+\s+\d+\s+\d{2}:\d{2}:\d{2}\s+\d{4})\s+(.+)$/.exec(text);
	assert(match, `unrecognized ps identity: ${text}`);
	return {
		pid: Number(match[1]),
		ppid: Number(match[2]),
		rss_kib: Number(match[3]),
		started: String(match[4]).replace(/\s+/g, " "),
		command: String(match[5]),
	};
}

/** Seed only after the real bootstrap host exits. Compaction changes model context, not event history. */
export function seedHistory(path: string, turns = 10_000) {
	const original = readFileSync(path, "utf8");
	assert(original.endsWith("\n"), "bootstrap history must end at a complete record");
	const bootstrap = original
		.trimEnd()
		.split("\n")
		.map((line) => object(JSON.parse(line)));
	const userTemplate = object(bootstrap.find((event) => event.type === "user.message")?.data);
	const assistantTemplate = object(
		bootstrap.find(
			(event) =>
				event.type === "assistant.message" && typeof object(event.data).content === "string",
		)?.data,
	);
	const hash = createHash("sha256").update(original);
	let parentId = String(bootstrap.at(-1)?.id);
	let count = 0;
	const descriptor = openSync(path, "a");
	const paragraph =
		"Historical source inspection: the parser reads complete UTF-8 records, preserves event identity, checks the cursor boundary, and returns only newly appended work.\nexport function consume(record) { return { id: record.id, kind: record.type, accepted: true }; }\n";
	const body = paragraph.repeat(28);
	const append = (type: string, data: Record<string, unknown>) => {
		const id = randomUUID();
		const line = `${JSON.stringify({ id, timestamp: new Date().toISOString(), parentId, type, data })}\n`;
		writeSync(descriptor, line);
		hash.update(line);
		parentId = id;
		count++;
	};
	try {
		for (let index = 0; index < turns; index++) {
			const interactionId = randomUUID();
			const turnId = `memory-history-${index}`;
			const user = {
				...userTemplate,
				content: `Summarize historical fixture module ${index}.`,
				messageId: randomUUID(),
			};
			const assistant = {
				...assistantTemplate,
				content: `Historical module ${index}:\n${body}`,
				messageId: randomUUID(),
			};
			for (const data of [user, assistant] as Record<string, unknown>[]) {
				if ("interactionId" in data) data.interactionId = interactionId;
				if ("turnId" in data) data.turnId = turnId;
				if ("transformedContent" in data) data.transformedContent = data.content;
			}
			append("user.message", user);
			append("assistant.message", assistant);
		}
		append("session.compaction_start", {});
		append("session.compaction_complete", {
			success: true,
			summaryContent: `Synthetic historical fixture: ${turns} completed source-inspection turns. All work is complete. Respond to new messages normally; do not call tools.`,
			preCompactionMessagesLength: turns * 2,
			messagesRemoved: turns * 2,
		});
	} finally {
		closeSync(descriptor);
	}
	const bytes = statSync(path).size;
	assert(
		count >= 20_000 && bytes >= 60 * 1024 * 1024,
		"AC6 history must contain >=20,000 synthetic events and >=60 MiB",
	);
	return {
		path,
		bootstrap_events: bootstrap.length,
		synthetic_events: count,
		total_events: bootstrap.length + count,
		bytes,
		sha256: hash.digest("hex"),
		last_event_id: parentId,
		format:
			"real bootstrap event templates + chained synthetic user/assistant events + supported successful compaction summary",
		inference: `synthetic history, not a claim of ${turns} real inference turns`,
	};
}

/** The proof monitor must not repeatedly deserialize the same large history either. */
export function eventTail(path: string, offset: number) {
	let cursor = offset;
	let remainder = "";
	const decoder = new StringDecoder("utf8");
	const events: NativeEvent[] = [];
	return () => {
		const size = statSync(path).size;
		assert(size >= cursor, "native host truncated seeded history");
		if (size === cursor) return events;
		const fd = openSync(path, "r");
		try {
			const bytes = Buffer.alloc(64 * 1024);
			while (cursor < size) {
				const length = readSync(fd, bytes, 0, Math.min(bytes.length, size - cursor), cursor);
				assert(length > 0);
				cursor += length;
				remainder += decoder.write(bytes.subarray(0, length));
				let newline = remainder.indexOf("\n");
				while (newline >= 0) {
					const line = remainder.slice(0, newline);
					remainder = remainder.slice(newline + 1);
					if (line) {
						const event = object(JSON.parse(line));
						assert(typeof event.id === "string" && typeof event.type === "string");
						events.push({ id: event.id, type: event.type, data: object(event.data) });
					}
					newline = remainder.indexOf("\n");
				}
			}
		} finally {
			closeSync(fd);
		}
		return events;
	};
}
function prefixSha(path: string, length: number) {
	const hash = createHash("sha256");
	const fd = openSync(path, "r");
	try {
		const bytes = Buffer.alloc(64 * 1024);
		for (let offset = 0; offset < length; ) {
			const count = readSync(fd, bytes, 0, Math.min(bytes.length, length - offset), offset);
			assert(count > 0, "seeded history prefix disappeared");
			hash.update(bytes.subarray(0, count));
			offset += count;
		}
	} finally {
		closeSync(fd);
	}
	return hash.digest("hex");
}

export async function runMemorySmoke(options: Options): Promise<{ code: number; output: string }> {
	assert(!existsSync(options.output), `refusing to overwrite evidence ${options.output}`);
	mkdirSync(options.output, { recursive: true, mode: 0o700 });
	const run = join(options.output, "runtime");
	const home = join(run, "home");
	const copilotHome = join(home, ".copilot");
	const state = join(run, "daemon");
	const cwd = join(run, "workspace");
	// AF_UNIX paths are capped at 104 bytes on macOS; the plan path is too long for a socket.
	const tmuxHome = mkdtempSync("/tmp/pij-memory-");
	for (const path of [
		home,
		copilotHome,
		state,
		cwd,
		join(home, ".claude"),
		join(home, ".config/claude"),
	])
		mkdirSync(path, { recursive: true, mode: 0o700 });
	const socketName = `ac6-${randomUUID().slice(0, 12)}`;
	const receipt: Record<string, unknown> = {
		status: "running",
		started: new Date().toISOString(),
		provider: "deterministic-local-fixture-not-real-inference",
		messages: [],
		rss: [],
		output: options.output,
	};
	const messages = receipt.messages as Record<string, unknown>[];
	const rss = receipt.rss as Record<string, unknown>[];
	let env: NodeJS.ProcessEnv = {};
	let daemon: ChildProcess | undefined;
	let fixture: Awaited<ReturnType<typeof startFixtureProvider>> | undefined;
	let socketCreated = false;
	let socket = "";
	let timer: ReturnType<typeof setInterval> | undefined;
	let samplingError: unknown;
	let interrupted: string | undefined;
	let code = 1;
	const interrupt = (signal: string) => {
		interrupted = signal;
	};
	const onSigint = () => interrupt("SIGINT");
	const onSigterm = () => interrupt("SIGTERM");
	process.on("SIGINT", onSigint);
	process.on("SIGTERM", onSigterm);
	const save = () =>
		writeFileSync(join(options.output, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`, {
			mode: 0o600,
		});
	const log = (kind: string, data: unknown) =>
		appendFileSync(
			join(options.output, "timeline.jsonl"),
			`${JSON.stringify({ at: new Date().toISOString(), kind, data })}\n`,
			{ mode: 0o600 },
		);
	const tmux = (args: string[], preserve = false) =>
		command("tmux", ["-L", socketName, ...args], env, preserve);
	const until = async <T>(
		label: string,
		timeout: number,
		probe: () => T | undefined | Promise<T | undefined>,
	): Promise<T> => {
		const deadline = Date.now() + timeout;
		do {
			if (interrupted) throw new Error(`interrupted: ${interrupted}`);
			if (samplingError) throw samplingError;
			const result = await probe();
			if (result !== undefined) return result;
			await delay(250);
		} while (Date.now() < deadline);
		throw new Error(`PIJ_NATIVE_MEMORY_TIMEOUT: ${label}`);
	};
	try {
		if (!existsSync(options.pijBin))
			throw new Error(
				"PIJ_NATIVE_PREREQUISITE_BINARY: build the composed worktree with CARGO_TARGET_DIR=$PWD/target cargo build -p pij-cli --bin pij-rs",
			);
		assert(
			realpathSync(options.pijBin).startsWith(`${realpathSync(ROOT)}/target/`),
			"Rust binary must resolve inside this worktree target",
		);
		const addr = `127.0.0.1:${await freePort()}`;
		env = smokeEnvironment(process.env, home, copilotHome, state, addr, "local");
		Object.assign(env, {
			LC_ALL: "C",
			CLAUDE_CONFIG_DIR: join(home, ".claude"),
			CLAUDE_HOME: join(home, ".claude"),
			TMUX_TMPDIR: tmuxHome,
		});
		let runtimeVersion: string | undefined;
		if (options.copilotRuntimeDir) {
			const source = realpathSync(options.copilotRuntimeDir);
			const metadata = object(JSON.parse(readFileSync(join(source, "package.json"), "utf8")));
			assert.equal(metadata.name, "@github/copilot", "runtime must be the actual Copilot package");
			assert(
				typeof metadata.version === "string" &&
					/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.+-]+)?$/.test(metadata.version),
				"runtime package must declare its exact version",
			);
			runtimeVersion = metadata.version;
			const platform = `${process.platform}-${process.arch}`;
			const required = [
				"index.js",
				"app.js",
				"copilot-sdk/extension.js",
				`prebuilds/${platform}/runtime.node`,
			];
			for (const file of required) {
				if (!existsSync(join(source, file))) {
					throw new Error(`PIJ_NATIVE_PREREQUISITE_RUNTIME: current native runtime lacks ${file}`);
				}
			}
			const destination = join(copilotHome, "pkg", platform, runtimeVersion);
			mkdirSync(join(copilotHome, "pkg", platform), { recursive: true, mode: 0o700 });
			cpSync(source, destination, {
				recursive: true,
				dereference: true,
				force: false,
				errorOnExist: true,
				filter: (path) => !/^inuse\.\d+\.lock$/.test(basename(path)),
			});
			// The official older SEA loader selects its bundled runtime when auto-update
			// is false. Let it select this private cache; --prefer-version and OFFLINE
			// independently disable the selected current runtime's updater/network.
			Object.assign(env, { XDG_STATE_HOME: home, COPILOT_AUTO_UPDATE: "true" });
			receipt.runtime_cache = {
				source,
				destination,
				version: runtimeVersion,
				copy: "package-code-only; dereferenced private copy; excludes live inuse PID locks",
				updater_disabled_by: ["--prefer-version", "COPILOT_OFFLINE=true"],
				files: Object.fromEntries(required.map((file) => [file, sha(join(destination, file))])),
			};
		}
		const copilotArgs = runtimeVersion
			? ["--prefer-version", runtimeVersion]
			: ["--no-auto-update"];
		let copilotBin: string;
		try {
			copilotBin = realpathSync(
				isAbsolute(options.copilotBin)
					? options.copilotBin
					: command("which", [options.copilotBin], env),
			);
		} catch {
			throw new Error(
				"PIJ_NATIVE_PREREQUISITE_HOST: provide --copilot-bin PATH to a real Copilot CLI with native extensions",
			);
		}
		// Copilot hides some supported flags from --help. Actual native
		// registration below, not help-text substring matches, proves support.
		fixture = await startFixtureProvider(options.fixture);
		Object.assign(env, {
			COPILOT_PROVIDER_BASE_URL: fixture.url,
			COPILOT_PROVIDER_TYPE: "openai",
			COPILOT_PROVIDER_WIRE_API: "completions",
			COPILOT_MODEL: "pij-native-fixture",
		});
		writeFileSync(
			join(copilotHome, "config.json"),
			`${JSON.stringify({ trustedFolders: [realpathSync(cwd)] })}\n`,
			{ flag: "wx", mode: 0o600 },
		);
		const install: string[] = [];
		for (const args of [[], ["--doctor-copilot"]])
			assert.equal(
				manageCopilotExtension({
					pijRoot: options.extensionRoot ?? ROOT,
					home,
					copilotHome,
					args,
					stdout: (line) => install.push(line),
					stderr: (line) => install.push(line),
				}).skipped,
				0,
				install.join("\n"),
			);
		receipt.install = install;
		receipt.identity = {
			node: process.version,
			pij: command(options.pijBin, ["--version"], env),
			pij_binary: options.pijBin,
			pij_sha256: sha(options.pijBin),
			copilot: command(copilotBin, [...copilotArgs, "--version"], env),
			copilot_binary: copilotBin,
			copilot_sha256: sha(copilotBin),
			source_modules: Object.fromEntries(
				["extension.mjs", "store.mjs"].map((name) => [
					name,
					sha(join(options.extensionRoot ?? ROOT, ".copilot/extensions/pij", name)),
				]),
			),
		};
		receipt.isolation = {
			run,
			home,
			copilotHome,
			claudeHome: env.CLAUDE_HOME,
			claudeConfig: env.CLAUDE_CONFIG_DIR,
			state,
			addr,
			workspace: realpathSync(cwd),
			tmux_name: socketName,
			tmux_home: tmuxHome,
			auth: "none",
			lease_ms: LEASE_MS,
			production_contacted: false,
		};
		if (runtimeVersion) {
			assert.equal(
				String(object(receipt.identity).copilot)
					.split("\n")[0]
					?.split(" ")
					.at(-1)
					?.replace(/\.$/, ""),
				runtimeVersion,
				"official launcher must execute the exact copied native runtime version",
			);
		}
		save();
		tmux([
			"-f",
			"/dev/null",
			"new-session",
			"-d",
			"-s",
			"memory-smoke",
			"-x",
			"160",
			"-y",
			"48",
			"-c",
			cwd,
			"/bin/sleep",
			"86400",
		]);
		socketCreated = true;
		socket = tmux(["display-message", "-p", "#{socket_path}"]);
		assert(
			socket.startsWith(`${realpathSync(tmuxHome)}/`),
			"tmux socket must be owned by this evidence run",
		);
		env.TMUX = tmux(["display-message", "-p", "#{socket_path},#{pid},0"]);
		object(receipt.isolation).socket = socket;
		const daemonLog = openSync(join(options.output, "daemon.log"), "wx", 0o600);
		try {
			daemon = spawn(options.pijBin, ["--state-dir", state, "daemon", "--bind", addr], {
				env,
				cwd,
				stdio: ["ignore", daemonLog, daemonLog],
			});
		} finally {
			closeSync(daemonLog);
		}
		let launchError: Error | undefined;
		daemon.once("error", (error) => {
			launchError = error;
		});
		const api = async (path: string, body?: unknown) => {
			const key = readFileSync(join(state, "daemon.key"), "utf8").trim();
			assert(key);
			const response = await fetch(`http://${addr}${path}`, {
				method: body === undefined ? "GET" : "POST",
				headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" },
				body: body === undefined ? undefined : JSON.stringify(body),
				signal: AbortSignal.timeout(10_000),
			});
			const envelope = object(await response.json());
			assert(
				response.ok && envelope.ok === true,
				`HTTP ${response.status} ${path}: ${JSON.stringify(envelope)}`,
			);
			return envelope.data;
		};
		const health = await until("isolated Rust daemon readiness", options.timeoutMs, async () => {
			if (launchError) throw launchError;
			assert(daemon?.exitCode === null && daemon?.signalCode === null, "isolated daemon exited");
			if (!existsSync(join(state, "daemon.key"))) return undefined;
			try {
				return await api("/health");
			} catch {
				return undefined;
			}
		});
		assert.equal(object(health).offline, false, "real Rust adapters required");
		receipt.daemon = { pid: daemon.pid, health };
		const rows = (sql: string, ...params: string[]) => {
			const db = new DatabaseSync(join(state, "pij.sqlite"), { readOnly: true });
			try {
				return db.prepare(sql).all(...params);
			} finally {
				db.close();
			}
		};
		const seats = async (): Promise<Seat[]> => {
			const data = await api("/v1/seats");
			const list = Array.isArray(data) ? data : object(data).seats;
			assert(Array.isArray(list));
			return list as Seat[];
		};
		const inspect = (pid: number) =>
			parseProcess(
				command("ps", ["-p", String(pid), "-o", "pid=,ppid=,rss=,lstart=,command="], env),
			);
		const descendants = (host: ProcessIdentity) => {
			const processes = command("ps", ["-axo", "pid=,ppid="], env)
				.split("\n")
				.map((line) => {
					const match = /^\s*(\d+)\s+(\d+)\s*$/.exec(line);
					assert(match, `unrecognized ps process tree: ${line}`);
					return { pid: Number(match[1]), ppid: Number(match[2]) };
				});
			const isChild = (row: { pid: number; ppid: number }) => {
				let parent = row.ppid;
				const seen = new Set<number>();
				while (parent > 1 && !seen.has(parent)) {
					if (parent === host.pid) return true;
					seen.add(parent);
					parent = processes.find((process) => process.pid === parent)?.ppid ?? 0;
				}
				return false;
			};
			return processes.filter(isChild).flatMap((row) => {
				// A short-lived host helper can exit after the process-tree snapshot.
				const result = spawnSync(
					"ps",
					["-p", String(row.pid), "-o", "pid=,ppid=,rss=,lstart=,command="],
					{ env, encoding: "utf8", timeout: 3000 },
				);
				if (result.error) throw result.error;
				if (result.status === 1 && !result.stdout.trim()) return [];
				assert.equal(result.status, 0, result.stderr);
				const observed = parseProcess(result.stdout.trim());
				if (
					observed.command.includes("/extensions/pij/extension.mjs") &&
					(observed.command.includes(join(ROOT, ".copilot")) ||
						observed.command.includes(copilotHome))
				)
					return [observed];
				// Current Copilot forks a generic bootstrap; the module is in
				// EXTENSION_PATH, not argv. Inspect only this owned descendant.
				if (
					!observed.command.includes(copilotHome) ||
					!observed.command.includes("/preloads/extension_bootstrap.mjs")
				)
					return [];
				const environment = command("ps", ["eww", "-p", String(row.pid), "-o", "command="], env);
				const module = ` EXTENSION_PATH=${join(copilotHome, "extensions/pij/extension.mjs")}`;
				return environment.includes(`${module} `) || environment.endsWith(module) ? [observed] : [];
			});
		};
		const launch = async (resume?: Seat, prompt?: string) => {
			const pane = tmux([
				"new-window",
				"-d",
				"-P",
				"-F",
				"#{pane_id}",
				"-t",
				"memory-smoke",
				"-n",
				resume ? "ac6-target" : prompt ? "ac6-bootstrap" : "ac6-sender",
				"-c",
				cwd,
				copilotBin,
				"--no-custom-instructions",
				"--disable-builtin-mcps",
				"--no-remote",
				...copilotArgs,
				...(resume ? [`--resume=${resume.session}`] : []),
				...(prompt ? ["-i", prompt] : []),
			]);
			const seat = await until(
				"real native extension registration (native-capable host required)",
				options.timeoutMs,
				async () =>
					(await seats()).find(
						(row) =>
							row.pane === pane &&
							row.harness === "copilot" &&
							row.native_extension_delivery === true,
					),
			);
			assert(/^%\d+$/.test(pane) && /^[0-9a-f-]{36}$/i.test(seat.session));
			const host = inspect(seat.proc.pid);
			const extension = await until(
				"unique extension descendant of registered isolated host",
				options.timeoutMs,
				() => {
					const matching = descendants(host);
					assert(matching.length <= 1, "ambiguous extension child; refusing PID selection");
					return matching[0];
				},
			);
			log("native-launch", { seat, host, extension });
			return { seat, host, extension };
		};
		if (options.scenario) {
			await options.scenario({
				launch,
				api,
				rows,
				tmux,
				until,
				env,
				cwd,
				state,
				copilotHome,
				output: options.output,
				pijBin: options.pijBin,
				addr,
				receipt,
			});
		} else {
			const nonce = `PIJ_MEMORY_BOOT_${randomUUID()}`;
			const bootstrap = await launch(
				undefined,
				`${nonce}. Reply exactly PIJ_NATIVE_OBSERVED. Do not call tools.`,
			);
			const historyPath = join(
				copilotHome,
				"session-state",
				bootstrap.seat.session,
				"events.jsonl",
			);
			await until("real bootstrap model completion", options.timeoutMs, () => {
				const events = readNativeEvents(historyPath);
				return nativeMessages(events, nonce).length === 1 &&
					events.some((event) => event.type === "assistant.turn_end")
					? true
					: undefined;
			});
			const teardown: Record<string, unknown> = {};
			receipt.bootstrap = { ...bootstrap, teardown };
			await stopSmokeNativeHost(
				{ tmux, probe: (pid, signal) => process.kill(pid, signal) },
				{ previous: bootstrap.seat, launch: bootstrap.seat, socket },
				options.timeoutMs,
				teardown,
			);
			await until("bootstrap extension exits before history append", options.timeoutMs, () => {
				try {
					process.kill(bootstrap.extension.pid, 0);
					return undefined;
				} catch (error) {
					if ((error as NodeJS.ErrnoException).code === "ESRCH") return true;
					throw error;
				}
			});
			const seed = seedHistory(historyPath);
			receipt.history = seed;
			log("history-seeded", seed);
			const tail = eventTail(historyPath, seed.bytes);
			const target = await launch(bootstrap.seat);
			verifyNativeLifecycleIdentity(bootstrap.seat, target.seat, "resume");
			receipt.target = target;
			assert.equal(
				prefixSha(historyPath, seed.bytes),
				seed.sha256,
				"real resumed host must retain the complete seeded history",
			);
			const sender = await launch();
			receipt.sender = sender;
			const send = (body: string, msgId: string) => {
				const args = [
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
					"--body",
					body,
					"--msg-id",
					msgId,
				];
				const result = spawnSync(options.pijBin, args, {
					env: {
						...env,
						PIJ_SESSION_ID: sender.seat.id,
						TMUX_PANE: sender.seat.pane,
						HARNESS_SESSION_ID: sender.seat.session,
					},
					cwd,
					encoding: "utf8",
					timeout: 15_000,
					stdio: ["ignore", "pipe", "pipe"],
				});
				if (result.error) throw result.error;
				const outcome = {
					at: new Date().toISOString(),
					args,
					status: result.status,
					signal: result.signal,
					stdout: result.stdout,
					stderr: result.stderr,
					envelope: object(JSON.parse(result.stdout)),
				};
				log("pij-send", outcome);
				return outcome;
			};
			const sample = (kind: string) => {
				const observed = inspect(target.extension.pid);
				assert.equal(observed.started, target.extension.started, "extension PID was reused");
				assert.equal(observed.command, target.extension.command, "extension process image changed");
				const row = {
					at: new Date().toISOString(),
					elapsed_ms: Date.now() - sampleStarted,
					kind,
					completed_messages: messages.length,
					pid: observed.pid,
					rss_kib: observed.rss_kib,
					rss_mib: observed.rss_kib / 1024,
				};
				rss.push(row);
				appendFileSync(
					join(options.output, "rss.csv"),
					`${row.at},${row.elapsed_ms},${row.kind},${row.completed_messages},${row.pid},${row.rss_kib},${row.rss_mib}\n`,
					{ mode: 0o600 },
				);
				log("rss", row);
			};
			writeFileSync(
				join(options.output, "rss.csv"),
				"at,elapsed_ms,kind,completed_messages,pid,rss_kib,rss_mib\n",
				{ flag: "wx", mode: 0o600 },
			);
			const sampleStarted = Date.now();
			sample("start");
			timer = setInterval(() => {
				try {
					sample("10s");
				} catch (error) {
					samplingError = error;
				}
			}, SAMPLE_MS);
			for (let index = 0; index < options.messages; index++) {
				const started = Date.now();
				const marker = `PIJ_MEMORY_${index}_${randomUUID()}`;
				const msgId = randomUUID();
				const sent = send(
					`${marker}. Reply exactly PIJ_NATIVE_OBSERVED. Do not call tools.`,
					msgId,
				);
				assert.equal(sent.envelope.ok, true, "ordinary send must succeed");
				const delivered = await until(
					`native completion ${index + 1}/${options.messages}`,
					options.timeoutMs,
					() => {
						const events = tail();
						const matching = nativeMessages(events, marker);
						assert(matching.length <= 1, "native message delivered more than once");
						const message = matching[0];
						if (!message) return undefined;
						const position = events.findIndex((event) => event.id === message.id);
						const after = events.slice(position + 1);
						const assistant = after.find(
							(event) =>
								event.type === "assistant.message" && event.data.content === "PIJ_NATIVE_OBSERVED",
						);
						const ended = after.find((event) => event.type === "assistant.turn_end");
						const ack = rows(
							"SELECT recipient, msg_id, origin, delivered_at FROM delivered_messages WHERE recipient = ? AND msg_id = ? AND origin = 'reader-read'",
							target.seat.id,
							msgId,
						)[0];
						return assistant && ended && ack
							? { native: message, assistant, ended, ack }
							: undefined;
					},
				);
				messages.push({ index: index + 1, msg_id: msgId, marker, sent, ...delivered });
				log("message-complete", messages.at(-1));
				save();
				await until("message pacing", Math.max(options.messageIntervalMs + 1000, 2000), () =>
					Date.now() - started >= options.messageIntervalMs ? true : undefined,
				);
			}
			assert.equal(prefixSha(historyPath, seed.bytes), seed.sha256);
			receipt.history_prefix_preserved_after_messages = true;
			assert(
				rss.filter((row) => row.kind === "10s").length >= 3,
				"RSS series must span at least three 10-second intervals",
			);
			// Leave real durable pending work, not a fabricated row, before killing just the receiver.
			receipt.prekill_hold = await api("/v1/report", {
				seat: target.seat.id,
				argv: ["report", "state", "hold"],
			});
			assert.equal(object(receipt.prekill_hold).state, "hold");
			const pendingId = randomUUID();
			const pending = send(`PIJ_MEMORY_PENDING_${randomUUID()}`, pendingId);
			assert.equal(pending.envelope.ok, true);
			await until("pending native job retained before receiver death", options.timeoutMs, () =>
				rows(
					"SELECT id, state, payload FROM jobs WHERE serial_key = ? AND dedupe_key = ?",
					target.seat.id,
					pendingId,
				).find((row) => row.state === "pending" || row.state === "running"),
			);
			assert.equal(
				rows(
					"SELECT msg_id FROM delivered_messages WHERE recipient = ? AND msg_id = ?",
					target.seat.id,
					pendingId,
				).length,
				0,
			);
			const hostNow = inspect(target.host.pid);
			assert.equal(hostNow.started, target.host.started);
			assert.equal(hostNow.command, target.host.command);
			assert.equal(tmux(["display-message", "-p", "#{socket_path}"]), socket);
			const childrenNow = descendants(hostNow);
			assert.equal(childrenNow.length, 1, "refusing ambiguous or missing isolated extension");
			const killTarget = childrenNow[0];
			assert(
				killTarget &&
					killTarget.pid === target.extension.pid &&
					killTarget.started === target.extension.started &&
					killTarget.command === target.extension.command,
				"kill target must still match verified extension identity and host ancestry",
			);
			sample("before-kill");
			clearInterval(timer);
			timer = undefined;
			const killedAt = Date.now();
			receipt.kill = {
				at: new Date(killedAt).toISOString(),
				signal: "SIGKILL",
				extension: killTarget,
				host: hostNow,
				pending_msg_id: pendingId,
				pending_send: pending,
				socket,
				tmux_name: socketName,
			};
			log("extension-only-kill", receipt.kill);
			save();
			process.kill(killTarget.pid, "SIGKILL");
			await until("verified extension process exits", options.timeoutMs, () => {
				try {
					process.kill(killTarget.pid, 0);
					return undefined;
				} catch (error) {
					if ((error as NodeJS.ErrnoException).code === "ESRCH") return true;
					throw error;
				}
			});
			const parked = await until(
				"dead native receiver parks pending job at its lease deadline",
				LEASE_MS + 2000,
				() =>
					rows(
						"SELECT seq, at, kind, seat, payload FROM spine_events WHERE kind = 'delivery.parked' AND json_extract(payload, '$.recipient') = ? AND json_extract(payload, '$.messageId') = ? ORDER BY seq",
						target.seat.id,
						pendingId,
					).find((row) => {
						const payload = object(JSON.parse(String(row.payload)));
						return payload.reason === DEAD_REASON && payload.messageId === pendingId;
					}),
			);
			const parkObservedMs = Date.now() - killedAt;
			const hostAfter = inspect(target.host.pid);
			assert.equal(
				hostAfter.started,
				target.host.started,
				"Copilot host must remain alive after extension death",
			);
			assert.equal(
				descendants(hostAfter).length,
				0,
				"host restarted receiver; this is not the dead-receiver scenario",
			);
			const nextId = randomUUID();
			const next = send(`PIJ_MEMORY_AFTER_KILL_${randomUUID()}`, nextId);
			const outcome = object(object(next.envelope.data).outcome);
			assert.equal(outcome.outcome, "refused", "next real pij send must refuse a dead receiver");
			assert(
				String(outcome.reason).includes(DEAD_REASON) &&
					String(outcome.reason).includes(target.seat.id),
				"refusal must name dead native receiver and seat",
			);
			receipt.dead_receiver = {
				park_observed_ms: parkObservedMs,
				lease_ms: LEASE_MS,
				parked,
				next_send: next,
				host_after: hostAfter,
				inbox: await api(`/v1/inbox?seat=${encodeURIComponent(target.seat.id)}&peek=true`),
				pending_jobs: rows(
					"SELECT id, state, payload, outcome FROM jobs WHERE serial_key = ? AND dedupe_key = ?",
					target.seat.id,
					pendingId,
				),
				terminal: tmux(["capture-pane", "-p", "-J", "-S", "-2000", "-t", target.seat.pane]),
			};
			log("dead-receiver-visible", receipt.dead_receiver);
			// AC7 uses Copilot's actual user-requested shell surface, not a driver-forged caller tuple.
			receipt.emergency_ready = await api("/v1/report", {
				seat: target.seat.id,
				argv: ["report", "state", "ready"],
			});
			const shellQuote = (value: string) => `'${value.replaceAll("'", "'\\''")}'`;
			const pullOutput = join(options.output, "emergency-inbox.json");
			const pullError = join(options.output, "emergency-inbox.stderr");
			const pullStatus = join(options.output, "emergency-inbox.status");
			const pullIdentity = join(options.output, "emergency-inbox-identity.txt");
			const pullScript = join(options.output, "emergency-inbox.sh");
			const pullArgs = ["--json", "--addr", addr, "--state-dir", state, "inbox"];
			writeFileSync(
				pullScript,
				`#!/bin/sh\nprintf '%s\\n' "$TMUX_PANE" "$COPILOT_AGENT_SESSION_ID" > ${shellQuote(pullIdentity)}\n${[options.pijBin, ...pullArgs].map(shellQuote).join(" ")} > ${shellQuote(pullOutput)} 2> ${shellQuote(pullError)}\nprintf '%s\\n' "$?" > ${shellQuote(pullStatus)}\n`,
				{ flag: "wx", mode: 0o700 },
			);
			const shellCommand = `! /bin/sh ${shellQuote(pullScript)}`;
			tmux(["send-keys", "-t", target.seat.pane, "-l", shellCommand]);
			tmux(["send-keys", "-t", target.seat.pane, "Enter"]);
			await until("actual Copilot shell executes emergency inbox", options.timeoutMs, () =>
				existsSync(pullStatus) && readFileSync(pullStatus, "utf8").trim() ? true : undefined,
			);
			const [callerPane, callerSession] = readFileSync(pullIdentity, "utf8").trimEnd().split("\n");
			assert.equal(
				callerPane,
				target.seat.pane,
				"emergency pull must run inside the actual target pane",
			);
			assert.equal(
				callerSession,
				target.seat.session,
				"native shell must supply matching COPILOT_AGENT_SESSION_ID",
			);
			const pull = object(JSON.parse(readFileSync(pullOutput, "utf8")));
			assert.equal(readFileSync(pullStatus, "utf8").trim(), "0", readFileSync(pullError, "utf8"));
			assert.equal(
				pull.ok,
				true,
				"standard native shell inbox claim must succeed without a process tuple",
			);
			assert(
				Array.isArray(pull.data) && pull.data.some((item) => object(item).msg_id === pendingId),
				"manual pull must return the parked pending message",
			);
			const pullAck = rows(
				"SELECT recipient, msg_id, origin, delivered_at FROM delivered_messages WHERE recipient = ? AND msg_id = ?",
				target.seat.id,
				pendingId,
			);
			assert.equal(pullAck.length, 1, "manual inbox pull must acknowledge exactly once");
			receipt.emergency_inbox = {
				command: shellCommand,
				caller: { pane: callerPane, copilot_session: callerSession },
				envelope: pull,
				acknowledgement: pullAck,
				jobs: rows(
					"SELECT id, state, outcome FROM jobs WHERE serial_key = ? AND dedupe_key = ?",
					target.seat.id,
					pendingId,
				),
				terminal: tmux(["capture-pane", "-p", "-J", "-S", "-2000", "-t", target.seat.pane]),
			};
			log("emergency-inbox-claimed", receipt.emergency_inbox);
			writeFileSync(
				join(options.output, "native-new-events.jsonl"),
				`${tail()
					.map((event) => JSON.stringify(event))
					.join("\n")}\n`,
				{ flag: "wx", mode: 0o600 },
			);
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
		log("failure", message);
	} finally {
		if (timer) clearInterval(timer);
		if (socketCreated && !(await finishSmokePanes(tmux, receipt))) code = 1;
		if (daemon) {
			try {
				await stopOwned(daemon);
			} catch (error) {
				receipt.daemon_cleanup_error = String(error);
				code = 1;
			}
		}
		if (fixture) {
			receipt.fixture_requests = fixture.requests;
			try {
				await fixture.close();
			} catch (error) {
				receipt.fixture_cleanup_error = String(error);
				code = 1;
			}
		}
		if (!socketCreated || !receipt.tmux_cleanup_error) {
			try {
				rmSync(tmuxHome, { recursive: true });
			} catch (error) {
				receipt.tmux_directory_cleanup_error = String(error);
				code = 1;
			}
		}
		process.off("SIGINT", onSigint);
		process.off("SIGTERM", onSigterm);
		if (code === 1) receipt.status = "failed";
		receipt.finished = new Date().toISOString();
		receipt.cleanup = {
			owned_tmux_stopped: socketCreated && !receipt.tmux_cleanup_error,
			owned_daemon_stopped: !daemon || daemon.exitCode !== null || daemon.signalCode !== null,
			retained:
				"Plan-local isolated runtime retained for evidence; daemon key and synthetic history stay inside private runtime directory. No production/global configuration was changed.",
		};
		save();
	}
	return { code, output: options.output };
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
	if (process.argv.includes("--help")) {
		console.log(
			"copilot-native-memory-smoke [--pij-bin WORKTREE/target/debug/pij-rs] [--copilot-bin CURRENT_COPILOT] [--output-dir PLAN/evidence/RUN] [--messages 30] [--message-interval-seconds 10] [--timeout-seconds 90]\nReal native host/extension and Rust daemon, deterministic local model only. Seeds >=60 MiB/>=20,000 events in a stopped fixture session, resumes it, completes >=30 real messages, samples receiver RSS every 10 seconds, kills only its verified extension PID and proves next-send refusal plus pending delivery.parked. Uses private HOME/Copilot/Claude homes, state, port and tmux -L. Finally stops owned services and retains evidence. Exit 0 passed, 1 failed, 2 missing prerequisite. Build the composed Rust tree first; no automatic builds, auth copies or global setup.",
		);
		console.log(
			"--copilot-runtime-dir PATH copies only an installed current platform package into the isolated cache (no auth/config/session copy), retaining the official loader with --prefer-version and offline update suppression.",
		);
	} else {
		try {
			const result = await runMemorySmoke(optionsFrom(process.argv.slice(2)));
			console.log(JSON.stringify(result));
			process.exitCode = result.code;
		} catch (error) {
			console.error(error instanceof Error ? error.message : String(error));
			process.exitCode = 2;
		}
	}
}
