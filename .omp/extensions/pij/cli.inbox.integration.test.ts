// Real private Rust daemon + public TS CLI descendants of active package-shaped hosts.
// These are deterministic process/protocol tests, not model-backed harness canaries.
import { type ChildProcess, spawn } from "node:child_process";
import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { observedProcessStart } from "./adapters/daemon-http.js";

const nodeRequire = createRequire(import.meta.url);
const TSX = nodeRequire.resolve("tsx");
const CLI = join(import.meta.dirname, "cli.ts");
const REPO_ROOT = resolve(import.meta.dirname, "../../..");
// Build before this suite: cargo build -p pij-cli --bin pij-rs (from this worktree).
// Never build during a test or silently select an installed production daemon.
const RS_BINARY = join(
	resolve(REPO_ROOT, process.env.CARGO_TARGET_DIR ?? "target"),
	"debug",
	process.platform === "win32" ? "pij-rs.exe" : "pij-rs",
);

type Harness = "claude" | "copilot" | "codex";
interface CliRun {
	readonly code: number;
	readonly stdout: string;
	readonly stderr: string;
}
interface Envelope<T> {
	v: number;
	command: string;
	ok: boolean;
	data: T;
	meta?: string;
	error?: string;
}
interface Descriptor {
	id: string;
	harness: string;
	session: string;
	pane: string | null;
	proc: { pid: number; proc_start: number };
	folder: string;
	relay: boolean;
	native_extension_delivery: boolean;
	binding?: "created" | "same" | "rebound";
}
interface Receipt {
	msg_id: string;
	outcome: { outcome: string; origin?: string };
	at: number;
	cold_check?: string;
}
interface Claim {
	job_id: number;
	message: { from: string; to: string; body: string; msg_id: string; in_reply_to?: string };
	native_consumer: { native_session: string; pid: number; proc_start: number };
}
interface OwnedProcess {
	child: ChildProcess;
	completed: Promise<CliRun>;
	output: () => { stdout: string; stderr: string };
	closed: () => boolean;
}
interface Host {
	process: OwnedProcess;
	harness: Harness;
	session: string;
	launch: (
		argv: readonly string[],
		env?: Record<string, string>,
	) => Promise<{
		pid: number;
		completed: Promise<CliRun>;
	}>;
	run: (argv: readonly string[], env?: Record<string, string>) => Promise<CliRun>;
}

async function until<T>(label: string, observe: () => T | undefined, timeout = 10_000): Promise<T> {
	const deadline = Date.now() + timeout;
	while (Date.now() < deadline) {
		const value = observe();
		if (value !== undefined) return value;
		await new Promise<void>((resolve) => setTimeout(resolve, 20));
	}
	throw new Error(`timed out observing ${label}`);
}

function success<T>(run: CliRun, command: string): Envelope<T> {
	expect(run, `${command}: ${run.stderr || run.stdout}`).toMatchObject({ code: 0, stderr: "" });
	const envelope = JSON.parse(run.stdout) as Envelope<T>;
	expect(envelope).toMatchObject({ v: 2, command, ok: true });
	expect(envelope).toHaveProperty("data");
	expect(envelope).not.toHaveProperty("error");
	return envelope;
}

function refusal(run: CliRun, reason: RegExp): void {
	expect(run).toMatchObject({ code: 4, stderr: "" });
	const envelope = JSON.parse(run.stdout) as Envelope<never>;
	expect(envelope).toMatchObject({ v: 2, ok: false, error: expect.any(String) });
	expect(envelope.command).toEqual(expect.any(String));
	expect(envelope).not.toHaveProperty("data");
	expect(envelope.meta).toMatch(reason);
}

// A missing daemon binary is a build-environment fact, not a product defect.
// CI builds it and sets PIJ_RS_BIN_REQUIRED=1 so absence fails loud there;
// elsewhere (Windows compat job, a worktree without a cargo build) the suite
// skips with the reason on the record instead of throwing.
const RS_BINARY_PRESENT = existsSync(RS_BINARY);
if (!RS_BINARY_PRESENT && process.env.PIJ_RS_BIN_REQUIRED === "1") {
	throw new Error(
		`PIJ_RS_BIN_REQUIRED=1 but ${RS_BINARY} is missing; build with cargo build -p pij-cli --bin pij-rs`,
	);
}
const describeWithDaemon = RS_BINARY_PRESENT ? describe : describe.skip;
describeWithDaemon("public pij inbox lifecycle against a private Rust daemon", () => {
	let root: string;
	let pijHome: string;
	let folder: string;
	let agentHome: string;
	let stateDir: string;
	let addr: string;
	let daemon: OwnedProcess | undefined;
	let tmuxServer: OwnedProcess | undefined;
	let hostSequence: number;
	const children = new Set<OwnedProcess>();
	const hosts: Host[] = [];
	const streamClosers: Array<() => Promise<void>> = [];

	beforeEach(() => {
		root = realpathSync(mkdtempSync(join(tmpdir(), "pij-native-inbox-")));
		pijHome = join(root, "legacy-home");
		folder = join(root, "cwd");
		agentHome = join(root, "home");
		stateDir = join(root, "rs-state");
		for (const path of [pijHome, folder, agentHome, stateDir, join(root, "tmux")]) {
			mkdirSync(path, { recursive: true });
		}
		addr = "127.0.0.1:0";
		daemon = undefined;
		tmuxServer = undefined;
		hostSequence = 0;
	});

	function own(child: ChildProcess): OwnedProcess {
		let stdout = "";
		let stderr = "";
		let closed = false;
		let failure: Error | undefined;
		child.stdout?.setEncoding("utf8").on("data", (chunk: string) => {
			stdout += chunk;
		});
		child.stderr?.setEncoding("utf8").on("data", (chunk: string) => {
			stderr += chunk;
		});
		child.once("error", (error) => {
			failure = error;
		});
		const completed = new Promise<CliRun>((resolve, reject) => {
			child.once("close", (code, signal) => {
				closed = true;
				if (failure) reject(failure);
				else resolve({ code: code ?? (signal === null ? 1 : 128), stdout, stderr });
			});
		});
		// A process may fail before its readiness/result is awaited; retain the rejection.
		void completed.catch(() => undefined);
		const owned = { child, completed, output: () => ({ stdout, stderr }), closed: () => closed };
		children.add(owned);
		return owned;
	}

	async function stop(owned: OwnedProcess, signal: NodeJS.Signals): Promise<CliRun> {
		if (!owned.closed()) owned.child.kill(signal);
		const kill = setTimeout(() => {
			if (!owned.closed()) owned.child.kill("SIGKILL");
		}, 5000);
		try {
			return await owned.completed;
		} finally {
			clearTimeout(kill);
		}
	}

	afterEach(async () => {
		// One failed assertion/stream must not bypass any host's descendant drain.
		const cleanup = await Promise.allSettled([
			...streamClosers.splice(0).map(async (close) => close()),
			...hosts.splice(0).map(async (host) => {
				const exit = await stop(host.process, "SIGTERM");
				expect(exit, exit.stderr).toMatchObject({ code: 0, stderr: "" });
			}),
			(async () => {
				if (!daemon) return;
				const exit = await stop(daemon, "SIGINT");
				expect(exit.stdout).toContain("pij-rs daemon: shutting down");
			})(),
		]);
		if (tmuxServer) {
			const server = tmuxServer;
			cleanup.push(
				...(await Promise.allSettled([
					(async () => {
						const stopped = await runTmux(["kill-server"]);
						expect(stopped, stopped.stderr).toMatchObject({ code: 0 });
						expect(await server.completed).toMatchObject({ code: 0 });
					})(),
				])),
			);
		}
		cleanup.push(
			...(await Promise.allSettled(
				[...children].map(async (child) => {
					await stop(child, child === tmuxServer ? "SIGTERM" : "SIGKILL");
				}),
			)),
		);
		children.clear();
		rmSync(root, { recursive: true, force: true });
		for (const result of cleanup) if (result.status === "rejected") throw result.reason;
	});

	function cliEnv(overrides: Record<string, string> = {}): NodeJS.ProcessEnv {
		return {
			PATH: process.env.PATH,
			SystemRoot: process.env.SystemRoot,
			HOME: agentHome,
			USERPROFILE: agentHome,
			CLAUDE_CONFIG_DIR: join(agentHome, ".claude"),
			XDG_CONFIG_HOME: join(agentHome, ".config"),
			TMPDIR: root,
			TMUX_TMPDIR: join(root, "tmux"),
			NODE_NO_WARNINGS: "1",
			PIJ_HOME: pijHome,
			PIJ_RS_STATE_DIR: stateDir,
			PIJ_RS_ADDR: addr,
			...overrides,
		};
	}

	function runTmux(argv: readonly string[]): Promise<CliRun> {
		return own(
			spawn("tmux", ["-S", join(root, "tmux.sock"), ...argv], {
				cwd: folder,
				env: cliEnv(),
				stdio: "pipe",
				timeout: 10_000,
				killSignal: "SIGKILL",
			}),
		).completed;
	}

	async function privateTmux(): Promise<void> {
		if (tmuxServer) return;
		tmuxServer = own(
			spawn("tmux", ["-S", join(root, "tmux.sock"), "-f", "/dev/null", "-D"], {
				cwd: folder,
				env: cliEnv(),
				stdio: "pipe",
			}),
		);
		const server = tmuxServer;
		await until("private tmux server socket", () => {
			if (server.closed()) throw new Error(`tmux exited: ${JSON.stringify(server.output())}`);
			return existsSync(join(root, "tmux.sock")) ? true : undefined;
		});
		const ready = await runTmux(["show-options", "-s", "exit-empty"]);
		expect(ready, ready.stderr).toMatchObject({ code: 0 });
		// The server must have a session for real list-panes to succeed. This
		// resource is not the claimed harness host and proves no model/pane binding.
		const session = await runTmux([
			"new-session",
			"-d",
			"-s",
			"pij-inbox-fixture",
			"-c",
			folder,
			"exec /bin/sleep 300",
		]);
		expect(session, session.stderr).toMatchObject({ code: 0 });
	}

	async function bindPaned(harness: "claude" | "pi" | "omp"): Promise<Descriptor> {
		await startDaemon();
		const created = await runTmux([
			"new-window",
			"-d",
			"-t",
			"pij-inbox-fixture",
			"-P",
			"-F",
			"#{pane_id}",
			"-c",
			folder,
			"exec /bin/sleep 300",
		]);
		expect(created, created.stderr).toMatchObject({ code: 0 });
		const pane = created.stdout.trim();
		expect(pane).toMatch(/^%[0-9]+$/);
		// Real owned pane/process admission; inert hosts prove no model session.
		const envelope = await http<Descriptor>("/v1/adopt", {
			argv: ["adopt", pane, "--harness", harness],
			caller: { TMUX_PANE: pane, cwd: folder },
		});
		expect(envelope).toMatchObject({
			v: 2,
			command: "pij adopt",
			ok: true,
			data: { id: expect.stringMatching(/^pij-/), harness, pane, session: null },
		});
		expect(envelope.data.proc.proc_start).toBe(observedProcessStart(envelope.data.proc.pid));
		return envelope.data;
	}

	async function http<T>(path: string, body?: unknown): Promise<Envelope<T>> {
		const response = await fetch(`http://${addr}${path}`, {
			method: body === undefined ? "GET" : "POST",
			headers: {
				Authorization: `Bearer ${readFileSync(join(stateDir, "daemon.key"), "utf8").trim()}`,
				"Content-Type": "application/json",
			},
			body: body === undefined ? undefined : JSON.stringify(body),
			signal: AbortSignal.timeout(10_000),
		});
		const text = await response.text();
		expect(response.status, text).toBe(200);
		return success<T>({ code: 0, stdout: text, stderr: "" }, JSON.parse(text).command);
	}

	async function startDaemon(): Promise<void> {
		if (daemon) return;
		if (!existsSync(RS_BINARY)) {
			throw new Error(
				`Missing worktree daemon ${RS_BINARY}; build with cargo build -p pij-cli --bin pij-rs in ${REPO_ROOT} (honoring CARGO_TARGET_DIR) before running this suite`,
			);
		}
		await privateTmux();
		const reservation = createServer();
		await new Promise<void>((resolve, reject) => {
			reservation.once("error", reject);
			reservation.listen(0, "127.0.0.1", resolve);
		});
		const binding = reservation.address();
		await new Promise<void>((resolve, reject) => {
			reservation.close((error) => (error ? reject(error) : resolve()));
		});
		if (!binding || typeof binding === "string" || binding.port === 7461) {
			throw new Error("private non-production loopback port was not allocated");
		}
		addr = `127.0.0.1:${binding.port}`;
		daemon = own(
			spawn(RS_BINARY, ["--state-dir", stateDir, "--addr", addr, "daemon", "--bind", addr], {
				cwd: folder,
				env: cliEnv({ TMUX: `${join(root, "tmux.sock")},0,0` }),
				stdio: "pipe",
			}),
		);
		const process = daemon;
		addr = await until(
			"private daemon listening banner",
			() => {
				const output = process.output();
				const match = `${output.stdout}\n${output.stderr}`.match(
					/pij-rs daemon: listening on (127\.0\.0\.1:\d+)/,
				);
				if (match) return match[1];
				if (process.closed())
					throw new Error(`daemon exited before readiness: ${output.stdout}\n${output.stderr}`);
				return undefined;
			},
			20_000,
		);
		expect(addr).not.toBe("127.0.0.1:0");
		expect(addr).not.toBe("127.0.0.1:7461");
		const health = await http<{ status: string }>("/health");
		expect(health.data.status).toBe("healthy");
	}

	function runPij(argv: readonly string[], env: Record<string, string> = {}): Promise<CliRun> {
		return own(
			spawn(process.execPath, ["--import", TSX, CLI, ...argv], {
				cwd: folder,
				env: cliEnv(env),
				stdio: "pipe",
				timeout: 20_000,
				killSignal: "SIGKILL",
			}),
		).completed;
	}

	function ambientFixture(harness: Harness, session: string): Record<string, string> {
		if (harness === "claude") return { CLAUDE_CODE_SESSION_ID: session };
		if (harness === "copilot") {
			mkdirSync(join(agentHome, ".copilot", "session-state", session), { recursive: true });
			return { COPILOT_AGENT_SESSION_ID: session };
		}
		const rolloutDir = join(agentHome, ".codex", "sessions", "2026", "07", "12");
		mkdirSync(rolloutDir, { recursive: true });
		writeFileSync(
			join(rolloutDir, `rollout-2026-07-12T00-00-00-${session}.jsonl`),
			`${JSON.stringify({ type: "session_meta", payload: { id: session, cwd: folder } })}\n`,
		);
		return { CODEX_THREAD_ID: session };
	}

	async function startHost(harness: Harness, session: string): Promise<Host> {
		await startDaemon();
		const entry = {
			claude: "@anthropic-ai/claude-code/cli.js",
			copilot: "@github/copilot/index.js",
			codex: "@openai/codex/bin/codex.js",
		}[harness];
		const hostPath = join(root, `host-${++hostSequence}`, "node_modules", entry);
		mkdirSync(dirname(hostPath), { recursive: true });
		// This live IPC host actually launches/owns the public CLI, rather than
		// spoofing argv0 or keeping an unrelated idle process alive as a fixture.
		writeFileSync(
			hostPath,
			`
const { spawn } = require("node:child_process");
const children = new Set();
let stopping = false;
const send = (message) => { if (process.connected) process.send(message); };
const finish = () => {
  if (stopping && children.size === 0) {
    if (process.connected) process.disconnect();
    process.exitCode = 0;
  }
};
const stop = () => {
  stopping = true;
  for (const child of children) child.kill("SIGKILL");
  finish();
};
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
process.on("disconnect", stop);
process.on("message", ({ id, argv, env }) => {
  if (stopping) return;
  const child = spawn(process.execPath, ["--import", ${JSON.stringify(TSX)}, ${JSON.stringify(CLI)}, ...argv], {
    cwd: ${JSON.stringify(folder)}, env: { ...process.env, ...env },
    stdio: ["ignore", "pipe", "pipe"], timeout: 20000, killSignal: "SIGKILL",
  });
  children.add(child);
  let stdout = "", stderr = "", error;
  child.stdout.setEncoding("utf8").on("data", (chunk) => { stdout += chunk; });
  child.stderr.setEncoding("utf8").on("data", (chunk) => { stderr += chunk; });
  child.once("spawn", () => send({ type: "started", id, pid: child.pid }));
  child.once("error", (value) => { error = value.message; });
  child.once("close", (code) => {
    children.delete(child);
    send({ type: "completed", id, result: { code: code ?? 1, stdout, stderr }, error });
    finish();
  });
});
send({ type: "ready", pid: process.pid });
`,
		);
		const sessionArgs = harness === "codex" ? ["resume", session] : ["--session-id", session];
		const owned = own(
			spawn(process.execPath, [hostPath, ...sessionArgs], {
				cwd: folder,
				env: cliEnv(ambientFixture(harness, session)),
				stdio: ["ignore", "pipe", "pipe", "ipc"],
			}),
		);
		let ready = false;
		let next = 0;
		const started = new Map<number, number>();
		const results = new Map<number, { result: CliRun; error?: string }>();
		owned.child.on(
			"message",
			(message: { type: string; id: number; pid: number; result: CliRun; error?: string }) => {
				if (message.type === "ready") ready = message.pid === owned.child.pid;
				if (message.type === "started") started.set(message.id, message.pid);
				if (message.type === "completed") results.set(message.id, message);
			},
		);
		const check = () => {
			if (owned.closed()) throw new Error(`host exited: ${JSON.stringify(owned.output())}`);
		};
		const launch: Host["launch"] = async (argv, env = {}) => {
			check();
			const id = ++next;
			owned.child.send({ id, argv, env });
			const pid = await until("host-owned CLI spawn", () => {
				check();
				if (results.get(id)?.error) throw new Error(results.get(id)?.error);
				return started.get(id);
			});
			const completed = until(
				"host-owned CLI completion",
				() => {
					check();
					const answer = results.get(id);
					if (answer?.error) throw new Error(answer.error);
					return answer?.result;
				},
				25_000,
			);
			void completed.catch(() => undefined);
			return { pid, completed };
		};
		const host: Host = {
			process: owned,
			harness,
			session,
			launch,
			run: async (argv, env) => (await launch(argv, env)).completed,
		};
		hosts.push(host);
		await until("active host IPC readiness", () => {
			check();
			return ready ? true : undefined;
		});
		return host;
	}

	async function register(host: Host, env: Record<string, string> = {}): Promise<Descriptor> {
		const run = await host.launch(["inbox", "register", "--json"], env);
		const envelope = success<Descriptor>(await run.completed, "pij register");
		const hostPid = host.process.child.pid;
		if (hostPid === undefined) throw new Error("registered fixture host has no process id");
		expect(envelope.data).toMatchObject({
			id: expect.stringMatching(/^pij-/),
			harness: host.harness,
			session: host.session,
			pane: null,
			folder,
			relay: false,
			native_extension_delivery: false,
			proc: {
				pid: hostPid,
				proc_start: observedProcessStart(hostPid),
			},
		});
		expect(envelope.data.proc.pid).not.toBe(run.pid);
		expect(envelope.data).not.toHaveProperty("descriptor");
		expect(envelope.data).not.toHaveProperty("existing");
		return envelope.data;
	}

	function expectClaims(run: CliRun, reader: Descriptor, messages: Claim["message"][]): Claim[] {
		const envelope = success<Claim[]>(run, "pij inbox");
		expect(envelope).toEqual({ v: 2, command: "pij inbox", ok: true, data: envelope.data });
		expect(envelope.data).toHaveLength(messages.length);
		for (const [index, message] of messages.entries()) {
			expect(envelope.data[index]).toMatchObject({
				job_id: expect.any(Number),
				message,
				native_consumer: { native_session: reader.session, ...reader.proc },
			});
		}
		return envelope.data;
	}

	async function send(host: Host, to: string, body: string, inReplyTo?: string): Promise<Receipt> {
		const argv = ["send", to, body, "--json"];
		if (inReplyTo) argv.push("--in-reply-to", inReplyTo);
		const envelope = success<Receipt>(await host.run(argv), "pij send");
		expect(envelope).toEqual({
			v: 2,
			command: "pij send",
			ok: true,
			data: {
				msg_id: expect.any(String),
				outcome: expect.objectContaining({ outcome: "queued" }),
				at: expect.any(Number),
				// Plan 157: every guarded send says what the cold-wake guard saw.
				// These recipients have no readable session, so it allows as unknown.
				cold_check: expect.stringMatching(/^unknown: /),
			},
		});
		return envelope.data;
	}

	async function watchOutcomes(): Promise<{
		outcomes: Array<{
			seat: string;
			msg_id: string;
			outcome: Receipt["outcome"];
			transport: string;
		}>;
		waitFor: (msgId: string) => Promise<void>;
	}> {
		const controller = new AbortController();
		const response = await fetch(`http://${addr}/v1/events?scope=local`, {
			headers: {
				Authorization: `Bearer ${readFileSync(join(stateDir, "daemon.key"), "utf8").trim()}`,
			},
			signal: controller.signal,
		});
		expect(response.status).toBe(200);
		if (!response.body) throw new Error("event stream has no body");
		const outcomes: Array<{
			seat: string;
			msg_id: string;
			outcome: Receipt["outcome"];
			transport: string;
		}> = [];
		let failure: unknown;
		const reader = response.body.getReader();
		const completed = (async () => {
			const decoder = new TextDecoder();
			let pending = "";
			try {
				for (;;) {
					const { value, done } = await reader.read();
					if (done) break;
					pending += decoder.decode(value, { stream: true });
					for (;;) {
						const end = pending.indexOf("\n");
						if (end < 0) break;
						const frame = JSON.parse(pending.slice(0, end));
						pending = pending.slice(end + 1);
						if (frame.type === "event" && frame.event.kind === "delivery.outcome") {
							outcomes.push({ seat: frame.event.seat, ...JSON.parse(frame.event.payload) });
						}
					}
				}
			} catch (error) {
				if (!controller.signal.aborted) failure = error;
			} finally {
				reader.releaseLock();
			}
		})();
		streamClosers.push(async () => {
			controller.abort();
			await completed;
			if (failure) throw failure;
		});
		return {
			outcomes,
			waitFor: async (msgId) => {
				await until("daemon delivery outcome event", () => {
					if (failure) throw failure;
					return outcomes.some((event) => event.msg_id === msgId) ? true : undefined;
				});
			},
		};
	}

	it("runs whoami from a live external host without PIJ_SESSION_ID or tmux", {
		timeout: 30_000,
	}, async () => {
		const host = await startHost("claude", "claude-portable-whoami");
		const descriptor = await register(host);
		const {
			binding: _binding,
			typing_grace_ms: _grace,
			proc_source: _source,
			...persisted
		} = descriptor as Descriptor & {
			typing_grace_ms?: number;
			proc_source?: string;
		};
		const answer = success<Descriptor>(await host.run(["whoami", "--json"]), "pij whoami");
		// whoami adds the held-FYI count the status lines read (plan 158).
		expect(answer).toEqual({
			v: 2,
			command: "pij whoami",
			ok: true,
			data: { ...persisted, pending_fyis: 0 },
		});
		expect(answer.data.proc.pid).toBe(host.process.child.pid);
	});

	it("delivers literal raw text and correlated replies through public CLI inbox reads", {
		timeout: 40_000,
	}, async () => {
		const sender = await startHost("claude", "claude-portable-raw");
		const receiver = await startHost("copilot", "11111111-2222-4333-8444-555555555555");
		const from = await register(sender);
		const to = await register(receiver);
		const body = "raw `backticks` $(not-a-command) 'quotes' \\\nsecond line";
		const sent = await send(sender, to.id, body);
		expectClaims(await receiver.run(["inbox", "check", "--json"]), to, [
			{ from: from.id, to: to.id, body, msg_id: sent.msg_id },
		]);
		const reply = await send(receiver, from.id, "literal reply", sent.msg_id);
		expectClaims(await sender.run(["inbox", "--json"]), from, [
			{
				from: to.id,
				to: from.id,
				body: "literal reply",
				msg_id: reply.msg_id,
				in_reply_to: sent.msg_id,
			},
		]);
		expectClaims(await receiver.run(["inbox", "--json"]), to, []);
		expectClaims(await sender.run(["inbox", "check", "--json"]), from, []);
		refusal(await sender.run(["send", from.id, "must not self-send", "--json"]), /self|itself/i);
	});

	it.each([
		{ label: "Claude", harness: "claude" as const, session: "claude-portable-current" },
		{
			label: "Copilot",
			harness: "copilot" as const,
			session: "11111111-2222-4333-8444-555555555555",
		},
		{ label: "Codex", harness: "codex" as const, session: "aaaaaaaa-1111-4222-8333-bbbbbbbbbbbb" },
	])("$label ambient registration binds the host idempotently and finite wait returns v2 JSON", {
		timeout: 40_000,
	}, async ({ harness, session }) => {
		const host = await startHost(harness, session);
		const first = await register(host);
		expect(first.binding).toBe("created");
		expect(await register(host)).toMatchObject({ id: first.id, binding: "same", proc: first.proc });
		expect(
			success<Descriptor>(await host.run(["whoami", "--json"]), "pij whoami").data,
		).toMatchObject({ id: first.id, session, proc: first.proc, native_extension_delivery: false });
		const start = performance.now();
		expectClaims(await host.run(["inbox", "check", "--wait", "100", "--json"]), first, []);
		expect(performance.now() - start).toBeGreaterThanOrEqual(100);
		const roster = await http<{ seats: Descriptor[] }>("/v1/seats?scope=local");
		expect(roster.data.seats.map((seat) => seat.id)).toEqual([first.id]);
	});

	it("keeps bare finite inbox waits available to external pull seats", {
		timeout: 30_000,
	}, async () => {
		const host = await startHost("claude", "claude-pull-wait");
		const descriptor = await register(host);
		expectClaims(await host.run(["inbox", "--wait", "10", "--json"]), descriptor, []);
	});

	it("refuses an infinite inbox wait for a paned pushed-delivery compatibility seat", {
		timeout: 30_000,
	}, async () => {
		const seat = await bindPaned("claude");
		if (typeof seat.pane !== "string") throw new Error("paned fixture has no pane");
		refusal(
			await runPij(["inbox", "--wait", "--json"], { TMUX_PANE: seat.pane }),
			/push|paneless|pull/i,
		);
	});

	it("inbox register returns pure whoami for an already-bound Claude pane without ambient identity", {
		timeout: 30_000,
	}, async () => {
		const seat = await bindPaned("claude");
		if (typeof seat.pane !== "string") throw new Error("paned fixture has no pane");
		const env = { TMUX_PANE: seat.pane };
		const before = success<Descriptor>(await runPij(["whoami", "--json"], env), "pij whoami");
		const registered = success<Descriptor>(
			await runPij(["inbox", "register", "--json"], env),
			"pij whoami",
		);
		expect(registered).toEqual(before);
		expect(registered.data).toMatchObject({
			id: seat.id,
			harness: "claude",
			pane: seat.pane,
			session: seat.session,
		});
		expect(registered.data).not.toHaveProperty("binding");
		expect(registered.data).not.toHaveProperty("existing");
		expect(
			(await http<{ seats: Descriptor[] }>("/v1/seats?scope=local")).data.seats.map(
				(row) => row.id,
			),
		).toEqual([seat.id]);
	});

	it.each(["pi", "omp"] as const)("inbox register preserves self-registered %s pane identity", {
		timeout: 30_000,
	}, async (harness) => {
		const seat = await bindPaned(harness);
		if (typeof seat.pane !== "string") throw new Error("paned fixture has no pane");
		const env = { TMUX_PANE: seat.pane };
		const before = success<Descriptor>(await runPij(["whoami", "--json"], env), "pij whoami");
		const registered = success<Descriptor>(
			await runPij(["inbox", "register", "--json"], env),
			"pij whoami",
		);
		expect(registered).toEqual(before);
		expect(registered.data).toMatchObject({
			id: seat.id,
			harness,
			pane: seat.pane,
			session: seat.session,
		});
		expect(registered.data).not.toHaveProperty("binding");
		const taught = await runPij(["inbox", "register"], env);
		expect(taught.code).toBe(0);
		// Plan 153 R1: the guidance names THIS harness and never collapses the two.
		expect(taught.stdout).toContain(harness === "omp" ? "OMP" : "Pi");
		expect(taught.stdout).not.toMatch(/pi\/omp/i);
		expect(
			(await http<{ seats: Descriptor[] }>("/v1/seats?scope=local")).data.seats.map(
				(row) => row.id,
			),
		).toEqual([seat.id]);
	});

	it("repairs stale PIJ_SESSION_ID without replacing authoritative native identity", {
		timeout: 40_000,
	}, async () => {
		const host = await startHost("claude", "claude-stale-session-repair");
		const first = await register(host, { PIJ_SESSION_ID: "pij-stale-missing" });
		expect(first.binding).toBe("created");
		expect(await register(host, { PIJ_SESSION_ID: "pij-stale-missing" })).toMatchObject({
			id: first.id,
			binding: "same",
			proc: first.proc,
		});
		expect(success<Descriptor>(await host.run(["whoami", "--json"]), "pij whoami").data.id).toBe(
			first.id,
		);
	});

	it("refuses conflicting native session evidence observed in the host argv", {
		timeout: 30_000,
	}, async () => {
		const host = await startHost("claude", "claude-host-original");
		const first = await register(host);
		refusal(
			await host.run(["inbox", "register", "--json"], {
				CLAUDE_CODE_SESSION_ID: "claude-child-conflict",
			}),
			/session.*differ|conflict/i,
		);
		expect(success<Descriptor>(await host.run(["whoami", "--json"]), "pij whoami").data.id).toBe(
			first.id,
		);
		const roster = await http<{ seats: Descriptor[] }>("/v1/seats?scope=local");
		expect(roster.data.seats.map((seat) => seat.id)).toEqual([first.id]);
	});

	it("keeps register fail-loud with neither ambient identity nor a pane", {
		timeout: 30_000,
	}, async () => {
		await startDaemon();
		refusal(await runPij(["inbox", "register", "--json"]), /E-AMBIG|cannot detect|identity/i);
		expect((await http<{ seats: Descriptor[] }>("/v1/seats?scope=local")).data.seats).toEqual([]);
	});

	it("refuses ambiguous ambient identities rather than selecting a harness", {
		timeout: 30_000,
	}, async () => {
		const host = await startHost("claude", "claude-ambiguous");
		refusal(
			await host.run(
				["inbox", "register", "--json"],
				ambientFixture("copilot", "22222222-2222-4333-8444-555555555555"),
			),
			/E-AMBIG|ambig|multiple/i,
		);
		expect((await http<{ seats: Descriptor[] }>("/v1/seats?scope=local")).data.seats).toEqual([]);
	});

	it("refuses ambient-only registration without a matching real host ancestor", {
		timeout: 30_000,
	}, async () => {
		await startDaemon();
		refusal(
			await runPij(
				["inbox", "register", "--json"],
				ambientFixture("claude", "claude-no-matching-host"),
			),
			/ancestor|host|session/i,
		);
		expect((await http<{ seats: Descriptor[] }>("/v1/seats?scope=local")).data.seats).toEqual([]);
	});

	it("round-trips concurrent wait, send, output acknowledgement and a correlated reply exactly once", {
		timeout: 60_000,
	}, async () => {
		const receiver = await startHost("copilot", "33333333-2222-4333-8444-555555555555");
		const sender = await startHost("codex", "bbbbbbbb-1111-4222-8333-bbbbbbbbbbbb");
		const to = await register(receiver);
		const from = await register(sender);
		const audit = await watchOutcomes();
		const waiting = await receiver.launch(["inbox", "--wait", "--json"]);
		const competing = await sender.launch(["inbox", "check", "--wait", "1000", "--json"]);
		expect(waiting.pid).not.toBe(to.proc.pid);
		expect(competing.pid).not.toBe(from.proc.pid);
		const sent = await send(sender, to.id, "hello from portable sender");
		const received = expectClaims(await waiting.completed, to, [
			{ from: from.id, to: to.id, body: "hello from portable sender", msg_id: sent.msg_id },
		]);
		expect(received[0]?.job_id).toBeGreaterThan(0);
		expectClaims(await competing.completed, from, []);
		const replyWait = await sender.launch(["inbox", "--wait", "--json"]);
		const reply = await send(receiver, from.id, "reply from reader", sent.msg_id);
		expectClaims(await replyWait.completed, from, [
			{
				from: to.id,
				to: from.id,
				body: "reply from reader",
				msg_id: reply.msg_id,
				in_reply_to: sent.msg_id,
			},
		]);
		expectClaims(await receiver.run(["inbox", "check", "--json"]), to, []);
		expectClaims(await sender.run(["inbox", "check", "--json"]), from, []);
		// A later committed outcome is an ordered stream barrier, not a sleep
		// guessed to be long enough for asynchronous receipt publication.
		const barrier = await send(sender, to.id, "audit barrier");
		await audit.waitFor(barrier.msg_id);
		for (const [msgId, seat] of [
			[sent.msg_id, to.id],
			[reply.msg_id, from.id],
		]) {
			expect(
				audit.outcomes.filter(
					(event) => event.msg_id === msgId && event.outcome.outcome === "delivered",
				),
			).toEqual([
				{
					seat,
					msg_id: msgId,
					outcome: { outcome: "delivered", origin: "reader-read" },
					transport: "inbox",
				},
			]);
		}
		expectClaims(await receiver.run(["inbox", "--json"]), to, [
			{ from: from.id, to: to.id, body: "audit barrier", msg_id: barrier.msg_id },
		]);
	});
});
