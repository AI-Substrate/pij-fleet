import { type ChildProcess, spawn } from "node:child_process";
import { constants } from "node:fs";
import { access, lstat, mkdir, mkdtemp, realpath, rm, symlink, unlink } from "node:fs/promises";
import { createRequire } from "node:module";
import { createConnection, createServer, type Server } from "node:net";
import { homedir } from "node:os";
import { basename, delimiter, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";

const COMMAND = 10_000;
const STARTUP = 30_000;
const STOP = 3_000;
const BUILD = 180_000;
const POLL = 25;
const require = createRequire(import.meta.url);

export interface CommandResult {
	readonly status: number | null;
	readonly stdout: string;
	readonly stderr: string;
}

export interface OwnedProcess {
	readonly child: ChildProcess;
	readonly result: Promise<CommandResult>;
	output(): { stdout: string; stderr: string };
	stop(): Promise<void>;
}

/** Only this unreplaced child handle can select a signal target. */
class ManagedProcess implements OwnedProcess {
	readonly child: ChildProcess;
	readonly result: Promise<CommandResult>;
	private stdout = "";
	private stderr = "";
	private closed = false;
	private readonly completion: Promise<void>;
	private stopping?: Promise<void>;

	constructor(program: string, args: readonly string[], env: NodeJS.ProcessEnv, cwd: string) {
		this.child = spawn(program, [...args], { cwd, env, stdio: ["pipe", "pipe", "pipe"] });
		this.child.stdout?.setEncoding("utf8").on("data", (chunk: string) => {
			this.stdout += chunk;
		});
		this.child.stderr?.setEncoding("utf8").on("data", (chunk: string) => {
			this.stderr += chunk;
		});
		let failure: Error | undefined;
		this.child.on("error", (error: Error) => {
			failure = error;
		});
		this.result = new Promise((fulfill, reject) => {
			this.child.once("close", (status) => {
				this.closed = true;
				if (failure) reject(new Error(`spawn ${program}: ${failure.message}`, { cause: failure }));
				else fulfill({ status, ...this.output() });
			});
		});
		// A long-lived process may fail before its owner reaches an await. Keep
		// that rejection observed, without changing the public result promise.
		this.completion = this.result.then(
			() => undefined,
			() => undefined,
		);
	}

	output(): { stdout: string; stderr: string } {
		return { stdout: this.stdout, stderr: this.stderr };
	}

	assertRunning(label: string): void {
		if (
			this.closed ||
			this.child.exitCode !== null ||
			this.child.signalCode !== null ||
			!this.child.pid
		) {
			throw new Error(`${label} exited before readiness: ${JSON.stringify(this.output())}`);
		}
	}

	async wait(bound: number): Promise<boolean> {
		if (this.closed) return true;
		let timer: ReturnType<typeof setTimeout> | undefined;
		try {
			return await Promise.race([
				this.completion.then(() => true),
				new Promise<boolean>((fulfill) => {
					timer = setTimeout(() => fulfill(false), bound);
				}),
			]);
		} finally {
			clearTimeout(timer);
		}
	}

	stop(): Promise<void> {
		this.stopping ??= this.stopOwned();
		return this.stopping;
	}

	private async stopOwned(): Promise<void> {
		if (!this.closed && this.child.exitCode === null && this.child.signalCode === null) {
			this.child.kill("SIGINT");
		}
		if (!(await this.wait(STOP))) {
			if (this.child.exitCode === null && this.child.signalCode === null)
				this.child.kill("SIGKILL");
			if (!(await this.wait(STOP))) {
				throw new Error(
					`owned child ${this.child.pid} did not reap after SIGKILL: ${JSON.stringify(this.output())}`,
				);
			}
		}
		await this.result;
	}

	get reaped(): boolean {
		return this.closed;
	}
}

function successful(result: CommandResult, label: string): CommandResult {
	if (result.status !== 0)
		throw new Error(`${label} exited ${result.status}: ${JSON.stringify(result)}`);
	return result;
}

async function command(
	process: ManagedProcess,
	bound: number,
	label: string,
): Promise<CommandResult> {
	process.child.stdin?.end();
	if (await process.wait(bound)) return process.result;
	const timeout = new Error(`${label} exceeded ${bound}ms: ${JSON.stringify(process.output())}`);
	try {
		await process.stop();
	} catch (cleanup) {
		throw new AggregateError([timeout, cleanup], `${label} timed out and could not be reaped`);
	}
	throw timeout;
}

function hasCode(error: unknown, code: string): boolean {
	return error instanceof Error && "code" in error && error.code === code;
}

async function absent(path: string): Promise<boolean> {
	try {
		await lstat(path);
		return false;
	} catch (error) {
		if (hasCode(error, "ENOENT")) return true;
		throw error;
	}
}

function object(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** A refusal proves no listener; timeouts and other errors do not. */
async function accepts(port: number): Promise<boolean> {
	return new Promise((fulfill, reject) => {
		const socket = createConnection({ host: "127.0.0.1", port });
		let answer: boolean | Error;
		socket.once("connect", () => {
			answer = true;
			socket.destroy();
		});
		socket.once("error", (error: Error) => {
			answer = hasCode(error, "ECONNREFUSED") ? false : error;
		});
		socket.setTimeout(500, () => {
			answer = new Error(`private port ${port} probe timed out`);
			socket.destroy();
		});
		socket.once("close", () => {
			if (typeof answer === "boolean") fulfill(answer);
			else reject(answer ?? new Error(`private port ${port} closed without a result`));
		});
	});
}

/** One isolated native daemon and exactly two inert, unadopted tmux panes. */
export class PrivateGovernanceFixture {
	readonly repo: string;
	readonly state: string;
	readonly binary: string;
	readonly env: NodeJS.ProcessEnv;
	private readonly label: string;
	private readonly clients = new Set<ManagedProcess>();
	private reservation?: Server;
	private port = 0;
	private daemon?: ManagedProcess;
	private tmux?: ManagedProcess;
	private socket?: string;
	private parent = "";
	private worker = "";
	private closing?: Promise<void>;

	private constructor(
		readonly root: string,
		private readonly projectRoot: string,
	) {
		this.repo = join(root, "repo");
		this.state = join(root, "state");
		this.binary = join(projectRoot, "target/debug/pij-rs");
		this.label = basename(root);
		const home = join(root, "home");
		// Deliberately no ambient identities, credentials, proxies, NODE_OPTIONS,
		// or global Git configuration. PATH is only for installed executable tools.
		this.env = {
			PATH: `${join(root, "bin")}${delimiter}${process.env.PATH ?? "/usr/bin:/bin"}`,
			HOME: home,
			CLAUDE_CONFIG_DIR: join(home, ".claude"),
			XDG_CONFIG_HOME: join(home, ".config"),
			XDG_DATA_HOME: join(home, ".local/share"),
			XDG_STATE_HOME: join(home, ".local/state"),
			XDG_CACHE_HOME: join(home, ".cache"),
			COPILOT_HOME: join(home, ".copilot"),
			COPILOT_CONFIG_DIR: join(home, ".copilot"),
			CODEX_HOME: join(home, ".codex"),
			PI_CONFIG_DIR: join(home, ".omp"),
			PI_CODING_AGENT_DIR: join(home, ".omp/agent"),
			OMP_CONFIG_DIR: join(home, ".omp"),
			PIJ_HOME: join(home, ".pij"),
			PIJ_RS_STATE_DIR: this.state,
			PIJ_DAEMON_GENERATION: "rs",
			TMPDIR: join(root, "tmp"),
			TMUX_TMPDIR: root,
			SHELL: "/bin/sh",
			TERM: "xterm-256color",
			LC_ALL: "C",
			GIT_CONFIG_NOSYSTEM: "1",
			GIT_CONFIG_SYSTEM: "/dev/null",
			GIT_CONFIG_GLOBAL: "/dev/null",
		};
	}

	get address(): string {
		return `127.0.0.1:${this.port}`;
	}
	get parentPane(): string {
		return this.parent;
	}
	get workerPane(): string {
		return this.worker;
	}

	static async open(projectRoot: string): Promise<PrivateGovernanceFixture> {
		const fixture = new PrivateGovernanceFixture(await mkdtemp("/tmp/p139-"), resolve(projectRoot));
		try {
			await fixture.initialize();
			return fixture;
		} catch (primary) {
			try {
				await fixture.close();
			} catch (cleanup) {
				throw new AggregateError(
					[primary, cleanup],
					"private fixture initialization and cleanup failed",
				);
			}
			throw primary;
		}
	}

	start(program: string, args: readonly string[], env = this.env, cwd = this.repo): OwnedProcess {
		if (this.closing) throw new Error("private fixture is closing");
		return this.track(program, args, env, cwd);
	}

	private track(
		program: string,
		args: readonly string[],
		env = this.env,
		cwd = this.repo,
	): ManagedProcess {
		const child = new ManagedProcess(program, args, env, cwd);
		this.clients.add(child);
		return child;
	}

	async run(
		program: string,
		args: readonly string[],
		env = this.env,
		cwd = this.repo,
	): Promise<CommandResult> {
		if (this.closing) throw new Error("private fixture is closing");
		return command(this.track(program, args, env, cwd), COMMAND, program);
	}

	native(args: readonly string[], env = this.env): Promise<CommandResult> {
		return this.run(
			this.binary,
			["--json", "--state-dir", this.state, "--addr", this.address, ...args],
			env,
		);
	}

	shim(args: readonly string[], env = this.env): Promise<CommandResult> {
		return this.run(
			process.execPath,
			[
				"--import",
				pathToFileURL(require.resolve("tsx")).href,
				join(this.projectRoot, ".omp/extensions/pij/cli.ts"),
				...args,
				"--json",
			],
			env,
		);
	}

	private async initialize(): Promise<void> {
		const directories = [this.repo, this.state, join(this.root, "bin"), join(this.root, "tmp")];
		for (const key of [
			"HOME",
			"CLAUDE_CONFIG_DIR",
			"XDG_CONFIG_HOME",
			"XDG_DATA_HOME",
			"XDG_STATE_HOME",
			"XDG_CACHE_HOME",
			"COPILOT_HOME",
			"CODEX_HOME",
			"PI_CODING_AGENT_DIR",
			"PIJ_HOME",
		]) {
			const path = this.env[key];
			if (path) directories.push(path);
		}
		for (const path of directories) await mkdir(path, { recursive: true });
		const target = join(await realpath(this.projectRoot), "target");
		const assertLocalArtifact = async (): Promise<void> => {
			for (const path of [join(this.projectRoot, "target"), dirname(this.binary), this.binary]) {
				if (await absent(path)) continue;
				const actual = await realpath(path);
				const within = relative(target, actual);
				if (within.startsWith("..") || isAbsolute(within)) {
					throw new Error(
						`native fixture authority escaped worktree target: ${path} -> ${actual}; build a local target/debug/pij-rs`,
					);
				}
			}
		};
		await assertLocalArtifact();
		try {
			await access(this.binary, constants.X_OK);
		} catch (error) {
			if (!hasCode(error, "ENOENT") && !hasCode(error, "EACCES")) throw error;
			// Only the offline build may read installed Rust caches. Runtime children
			// never inherit these homes. Cargo output is forced into this worktree.
			const buildEnv = {
				...this.env,
				CARGO_HOME: process.env.CARGO_HOME ?? join(homedir(), ".cargo"),
				RUSTUP_HOME: process.env.RUSTUP_HOME ?? join(homedir(), ".rustup"),
				CARGO_TARGET_DIR: join(this.projectRoot, "target"),
			};
			try {
				successful(
					await command(
						this.track(
							"cargo",
							["build", "--locked", "--offline", "-p", "pij-cli", "--bin", "pij-rs"],
							buildEnv,
							this.projectRoot,
						),
						BUILD,
						"offline native build",
					),
					"offline native build",
				);
				await access(this.binary, constants.X_OK);
			} catch (buildError) {
				throw new Error(
					`missing executable ${this.binary}; bounded cargo build --locked --offline -p pij-cli --bin pij-rs failed (CARGO_TARGET_DIR=${buildEnv.CARGO_TARGET_DIR}); provision the installed Rust toolchain and offline caches`,
					{ cause: buildError },
				);
			}
		}
		await assertLocalArtifact();
		if (!(await lstat(await realpath(this.binary))).isFile())
			throw new Error(`native fixture artifact is not a file: ${this.binary}`);
		await symlink(this.binary, join(this.root, "bin/pij-rs"));

		this.reservation = createServer();
		await new Promise<void>((fulfill, reject) => {
			this.reservation?.once("error", reject);
			this.reservation?.listen(0, "127.0.0.1", () => fulfill());
		});
		const address = this.reservation.address();
		if (!address || typeof address === "string")
			throw new Error("private port reservation has no TCP address");
		this.port = address.port;
		if (this.port === 7461)
			throw new Error("private reservation selected forbidden production port 7461");
		this.env.PIJ_RS_ADDR = this.address;
		this.env.PIJ_RS_BIND = this.address;

		this.tmux = new ManagedProcess(
			"tmux",
			["-L", this.label, "-f", "/dev/null", "-D"],
			this.env,
			this.repo,
		);
		const readyBy = Date.now() + STARTUP;
		for (;;) {
			this.tmux.assertRunning("private tmux server");
			// Unlike new-session, show-options cannot create a replacement server.
			if (
				(await this.run("tmux", ["-L", this.label, "show-options", "-s", "exit-empty"])).status ===
				0
			)
				break;
			if (Date.now() >= readyBy)
				throw new Error(`private tmux readiness timed out: ${JSON.stringify(this.tmux.output())}`);
			await delay(POLL);
		}
		this.parent = await this.pane("new-session", ["-s", "governance-smoke", "-n", "parent"]);
		this.env.TMUX_PANE = this.parent;
		this.worker = await this.pane("new-window", ["-t", "governance-smoke:", "-n", "worker"]);
		const panes = successful(
			await this.run("tmux", ["list-panes", "-a", "-F", "#{pane_id}"]),
			"inherited private TMUX",
		)
			.stdout.trim()
			.split("\n")
			.sort();
		if (
			JSON.stringify(panes) !== JSON.stringify([this.parent, this.worker].sort()) ||
			this.parent === this.worker
		) {
			throw new Error(
				`inherited TMUX does not select exactly the two owned panes: ${JSON.stringify(panes)}`,
			);
		}

		await this.releaseReservation();
		this.daemon = new ManagedProcess(
			this.binary,
			["--state-dir", this.state, "daemon", "--bind", this.address],
			this.env,
			this.repo,
		);
		const listeningBy = Date.now() + STARTUP;
		for (;;) {
			this.daemon.assertRunning("private native daemon");
			const { stdout, stderr } = this.daemon.output();
			const log = stdout + stderr;
			if (
				log.includes(`pij-rs daemon: listening on ${this.address}`) &&
				log.includes("offline=false")
			)
				break;
			if (Date.now() >= listeningBy) throw new Error(`private daemon readiness timed out: ${log}`);
			await delay(POLL);
		}
		if (!(await accepts(this.port)))
			throw new Error("announced private daemon does not accept TCP");
		const ping: unknown = JSON.parse(
			successful(await this.native(["ping"]), "private native ping").stdout,
		);
		if (
			!object(ping) ||
			ping.ok !== true ||
			!object(ping.data) ||
			ping.data.status !== "healthy" ||
			ping.data.offline !== false
		) {
			throw new Error(`private native daemon is not healthy and all-real: ${JSON.stringify(ping)}`);
		}
		for (const file of ["pij.sqlite", "daemon.key"]) {
			const stats = await lstat(join(this.state, file));
			if (!stats.isFile() || stats.size === 0)
				throw new Error(`private daemon did not create real ${file}`);
		}
	}

	private async pane(verb: string, args: readonly string[]): Promise<string> {
		this.tmux?.assertRunning("owned foreground tmux server");
		const output = successful(
			await this.run("tmux", [
				"-L",
				this.label,
				verb,
				"-d",
				...args,
				"-P",
				"-F",
				"#{socket_path}|#{pid}|#{pane_id}",
				"-c",
				this.repo,
				"/bin/sleep",
				"3600",
			]),
			`private tmux ${verb}`,
		).stdout.trim();
		const [socket, pid, pane, extra] = output.split("|");
		if (
			!socket ||
			!pid ||
			!pane ||
			extra !== undefined ||
			!/^%\d+$/.test(pane) ||
			Number(pid) !== this.tmux?.child.pid
		) {
			throw new Error(`private tmux identity is not the owned foreground child: ${output}`);
		}
		const within = relative(await realpath(this.root), await realpath(dirname(socket)));
		if (within.startsWith("..") || isAbsolute(within))
			throw new Error(`tmux socket escaped private root: ${socket}`);
		if (!(await lstat(socket)).isSocket())
			throw new Error(`observed tmux path is not a socket: ${socket}`);
		if (this.socket && socket !== this.socket)
			throw new Error("private pane moved to a different tmux socket");
		this.socket = socket;
		this.env.TMUX = `${socket},${pid},0`;
		return pane;
	}

	private async releaseReservation(): Promise<void> {
		const reservation = this.reservation;
		if (!reservation) return;
		if (reservation.listening) {
			await new Promise<void>((fulfill, reject) =>
				reservation.close((error) => (error ? reject(error) : fulfill())),
			);
		}
		this.reservation = undefined;
	}

	/** Callers must aggregate a body error with this rejection, not overwrite it. */
	close(): Promise<void> {
		this.closing ??= this.teardown();
		return this.closing;
	}

	private async teardown(): Promise<void> {
		const errors: unknown[] = [];
		const attempt = async (action: () => Promise<void>): Promise<void> => {
			try {
				await action();
			} catch (error) {
				errors.push(error);
			}
		};
		await attempt(() => this.releaseReservation());
		// Event streams and external hosts must close before graceful HTTP drain.
		await Promise.all([...this.clients].map((client) => attempt(() => client.stop())));
		if (this.daemon) {
			const daemon = this.daemon;
			await attempt(async () => {
				await daemon.stop();
				successful(await daemon.result, "private daemon shutdown");
			});
		}
		if (this.tmux) {
			const tmux = this.tmux;
			if (!tmux.reaped) {
				await attempt(async () => {
					const stop = this.track("tmux", ["-L", this.label, "kill-server"]);
					successful(
						await command(stop, COMMAND, "private tmux kill-server"),
						"private tmux kill-server",
					);
					if (!(await tmux.wait(COMMAND)))
						throw new Error("owned foreground tmux did not exit after kill-server");
				});
			}
			await attempt(async () => {
				await tmux.stop();
				successful(await tmux.result, "foreground tmux shutdown");
			});
		}
		// Include a failed teardown command client in the final reap barrier.
		await Promise.all(
			[...this.clients]
				.filter((client) => !client.reaped)
				.map((client) => attempt(() => client.stop())),
		);
		const reaped = [
			...this.clients,
			...(this.daemon ? [this.daemon] : []),
			...(this.tmux ? [this.tmux] : []),
		].every((child) => child.reaped);
		if (!reaped || this.reservation) {
			errors.push(
				new Error(`retaining private root ${this.root}: owned resources remain unreaped`),
			);
		} else {
			if (this.socket) {
				const socket = this.socket;
				await attempt(async () => {
					try {
						await unlink(socket);
					} catch (error) {
						if (!hasCode(error, "ENOENT")) throw error;
					}
					if (!(await absent(socket))) throw new Error(`owned tmux socket remains: ${socket}`);
				});
			}
			await attempt(async () => {
				await rm(this.root, { recursive: true, force: true });
				if (!(await absent(this.root))) throw new Error(`private root remains: ${this.root}`);
				if (this.socket && !(await absent(this.socket)))
					throw new Error(`owned socket remains: ${this.socket}`);
			});
		}
		if (this.port && this.port !== 7461)
			await attempt(async () => {
				if (await accepts(this.port))
					throw new Error(`private port ${this.address} still accepts connections after teardown`);
			});
		if (errors.length)
			throw new AggregateError(errors, "private governance fixture cleanup failed");
	}
}
