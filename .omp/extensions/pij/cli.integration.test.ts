// The shipped TS executable against private scripted HTTP and real native children.
// These prove the shim boundary, not Rust business handlers or a live fleet daemon.
import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createServer, type Server } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const TSX = createRequire(import.meta.url).resolve("tsx");
const CLI = join(import.meta.dirname, "cli.ts");
const REPO_ROOT = join(import.meta.dirname, "..", "..", "..");
const KEY = "private-cli-fixture-key";
const HEALTH = JSON.stringify({
	ok: true,
	command: "health",
	v: 2,
	data: { status: "ok", build: "fixture", offline: true, machine: "private" },
});
const SUCCESS =
	' { "command": "fixture", "v": 2, "ok": true, "data": { "line": "daemon answer" }, "meta": "kept", "future": {"proof":17} } ';
const REFUSAL =
	' { "v": 2, "command": "fixture", "ok": false, "error": "refused", "meta": "daemon admission refused", "details": {"code":"E-OWN","nested":[1,{"why":"private"}]}, "future": true }\n';

interface RequestRecord {
	path: string;
	method: string;
	authorization: string | undefined;
	body: string;
}
interface Reply {
	status: number;
	body: string;
}
interface CliRun {
	code: number;
	stdout: string;
	stderr: string;
}

let root: string;
let home: string;
let legacyHome: string;
let stateDir: string;
let bin: string;
let addr: string;
let server: Server;
let requests: RequestRecord[];
let reply: (request: RequestRecord) => Reply;
let healthReply: Reply;
const children = new Set<ChildProcessWithoutNullStreams>();

beforeEach(async () => {
	root = realpathSync(mkdtempSync(join(tmpdir(), "pij-cli-cutover-")));
	home = join(root, "home");
	legacyHome = join(home, ".pij");
	stateDir = join(root, "rs state");
	bin = join(root, "bin");
	for (const path of [home, stateDir, bin]) mkdirSync(path, { recursive: true });
	writeFileSync(join(stateDir, "daemon.key"), KEY);
	// Any accidental legacy control must stay within this private stand-in.
	writeFileSync(
		join(bin, "tmux"),
		`#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(join(root, "tmux-called"))}, 'called'); process.exitCode = 91;\n`,
	);
	chmodSync(join(bin, "tmux"), 0o755);
	requests = [];
	healthReply = { status: 200, body: HEALTH };
	reply = () => ({ status: 200, body: SUCCESS });
	server = createServer((request, response) => {
		void (async () => {
			const chunks: Buffer[] = [];
			for await (const chunk of request) chunks.push(Buffer.from(chunk));
			const record = {
				path: request.url ?? "",
				method: request.method ?? "",
				authorization: request.headers.authorization,
				body: Buffer.concat(chunks).toString("utf8"),
			};
			requests.push(record);
			const answer = record.path === "/health" ? healthReply : reply(record);
			response.writeHead(answer.status, { "Content-Type": "application/json" });
			response.end(answer.body);
		})().catch((error: Error) => response.destroy(error));
	});
	await new Promise<void>((resolve, reject) => {
		server.once("error", reject);
		server.listen(0, "127.0.0.1", resolve);
	});
	const address = server.address();
	if (address === null || typeof address === "string")
		throw new Error("fixture did not bind a private TCP port");
	addr = `127.0.0.1:${address.port}`;
});

afterEach(async () => {
	const exits = [...children].map(
		(child) =>
			new Promise<void>((resolve) => {
				child.once("close", () => resolve());
				child.kill("SIGKILL");
			}),
	);
	await Promise.all(exits);
	if (server.listening)
		await new Promise<void>((resolve, reject) => {
			server.close((error) => (error ? reject(error) : resolve()));
			server.closeAllConnections();
		});
	rmSync(root, { recursive: true, force: true });
});

function launch(
	argv: readonly string[],
	options: {
		env?: NodeJS.ProcessEnv;
		brokenStdout?: boolean;
		delayedReader?: boolean;
		signalAfterOutput?: "SIGTERM" | "SIGINT";
	} = {},
): Promise<CliRun> {
	return new Promise((resolve, reject) => {
		// The loader runs cli.ts in this exact PID; the tsx executable adds a
		// supervising process whose own signal handling would mask this boundary.
		const child = spawn(process.execPath, ["--import", TSX, CLI, ...argv], {
			cwd: root,
			env: {
				PATH: `${bin}${delimiter}${process.env.PATH ?? ""}`,
				HOME: home,
				USERPROFILE: home,
				TMPDIR: root,
				NODE_NO_WARNINGS: "1",
				PIJ_HOME: legacyHome,
				PIJ_RS_ADDR: addr,
				PIJ_RS_STATE_DIR: stateDir,
				...options.env,
			},
			stdio: "pipe",
			timeout: 20_000,
			killSignal: "SIGKILL",
		});
		children.add(child);
		const stdout: Buffer[] = [];
		const stderr: Buffer[] = [];
		let signalSent = false;
		child.stdout.on("data", (chunk: Buffer) => {
			stdout.push(chunk);
			if (options.signalAfterOutput !== undefined && !signalSent) {
				signalSent = true;
				child.kill(options.signalAfterOutput);
			}
		});
		child.stderr.on("data", (chunk: Buffer) => stderr.push(chunk));
		if (options.brokenStdout) child.stdout.destroy();
		let timer: NodeJS.Timeout | undefined;
		if (options.delayedReader) {
			child.stdout.pause();
			// Start the delay only once output exists, so TS startup speed cannot
			// accidentally turn the backpressure proof into an eager-reader test.
			child.stdout.once("readable", () => {
				timer = setTimeout(() => child.stdout.resume(), 250);
			});
		}
		child.once("error", reject);
		child.once("close", (code, signal) => {
			children.delete(child);
			if (timer !== undefined) clearTimeout(timer);
			if (signal !== null) {
				reject(new Error(`CLI terminated by ${signal}`));
				return;
			}
			resolve({
				code: code ?? 1,
				stdout: Buffer.concat(stdout).toString("utf8"),
				stderr: Buffer.concat(stderr).toString("utf8"),
			});
		});
		child.stdin.end();
	});
}

function seedLegacy(): string {
	mkdirSync(join(legacyHome, "pij-old", "inbox"), { recursive: true });
	const descriptor = {
		id: "pij-old",
		harness: "copilot",
		harnessSessionId: "legacy-native",
		paneId: "%137",
		folder: root,
		dataDir: join(legacyHome, "pij-old"),
		eventsPath: join(legacyHome, "pij-old", "events.ndjson"),
		pid: process.pid,
		state: "idle",
		lifecycle: "bound",
	};
	writeFileSync(join(legacyHome, "pij-old.json"), JSON.stringify(descriptor));
	writeFileSync(join(legacyHome, "pij-old", "events.ndjson"), "legacy sentinel\n");
	return legacySnapshot();
}

function legacySnapshot(): string {
	if (!existsSync(legacyHome)) return "absent";
	return JSON.stringify(
		readdirSync(legacyHome, { recursive: true, withFileTypes: true })
			.map((entry) => ({
				path: join(entry.parentPath, entry.name),
				contents: entry.isFile()
					? readFileSync(join(entry.parentPath, entry.name)).toString("base64")
					: null,
			}))
			.sort((a, b) => a.path.localeCompare(b.path)),
	);
}

function operationRequests(): RequestRecord[] {
	return requests.filter((request) => request.path !== "/health");
}

describe("closed rs-only CLI", () => {
	it("prints the real inventory without opening the daemon or legacy registry", async () => {
		const result = await launch(["--help"]);
		expect(result).toMatchObject({ code: 0, stderr: "" });
		expect(result.stdout).toContain("pij — rs-only command shim");
		expect(result.stdout).toContain("pij list: GET /v1/seats");
		expect(result.stdout).toContain("pij sessions: GET /v1/shim/sessions");
		expect(result.stdout).toContain("pij spawn: E-RS-UNPORTED");
		expect(result.stdout).toContain("pij commit-trailers: native pij-rs");
		expect(result.stdout).not.toContain("[--prime]");
		expect(result.stdout).not.toContain("served by legacy");
		expect(requests).toEqual([]);
		expect(legacySnapshot()).toBe("absent");
	});

	it("keeps version local and reads the actual package version", async () => {
		const version = JSON.parse(readFileSync(join(REPO_ROOT, "package.json"), "utf8")).version;
		expect(await launch(["--version"])).toEqual({
			code: 0,
			stdout: `pij ${version}\n`,
			stderr: "",
		});
		expect(requests).toEqual([]);
	});

	it.each([
		["spawn", "--harness", "claude"],
		["revive", "pij-old"],
		["tail", "pij-old"],
		["inbox", "unknown"],
		["daemon", "start"],
		["watch", "src/**"],
		["watchdog", "--help"],
		["identity", "release", "pij-old"],
		["unknown-command"],
	])("refuses %j without HTTP, tmux, or legacy writes", async (...argv) => {
		const before = seedLegacy();
		const result = await launch([...argv, "--json"]);
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout)).toMatchObject({
			ok: false,
			v: 2,
			error: "refused",
			details: { code: "E-RS-UNPORTED" },
		});
		expect(requests).toEqual([]);
		expect(legacySnapshot()).toBe(before);
		expect(existsSync(join(root, "tmux-called"))).toBe(false);
	});

	it("refuses external registration without native evidence before touching legacy state", async () => {
		const before = seedLegacy();
		const result = await launch(["inbox", "register", "--json"]);
		expect(result.code).toBe(4);
		expect(JSON.parse(result.stdout)).toMatchObject({
			ok: false,
			v: 2,
			error: "refused",
			details: { code: "E-AMBIG" },
		});
		expect(requests).toEqual([
			{ path: "/health", method: "GET", authorization: `Bearer ${KEY}`, body: "" },
		]);
		expect(legacySnapshot()).toBe(before);
		expect(existsSync(join(root, "tmux-called"))).toBe(false);
	});

	it("refuses a missing daemon without legacy registration or delivery", async () => {
		const before = seedLegacy();
		await new Promise<void>((resolve, reject) =>
			server.close((error) => (error ? reject(error) : resolve())),
		);
		const result = await launch(["send", "pij-old", "must not deliver", "--json"]);
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout)).toMatchObject({
			ok: false,
			details: { code: "E-RS-UNPORTED" },
		});
		expect(legacySnapshot()).toBe(before);
		expect(existsSync(join(root, "tmux-called"))).toBe(false);
	});

	it("refuses explicit legacy forcing before any request", async () => {
		const before = seedLegacy();
		const result = await launch(["send", "pij-old", "not delivered", "--json"], {
			env: { PIJ_DAEMON_GENERATION: "legacy" },
		});
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout).meta).toContain("PIJ_DAEMON_GENERATION=legacy is retired");
		expect(requests).toEqual([]);
		expect(legacySnapshot()).toBe(before);
	});

	it("returns a v2 local refusal for malformed routing configuration", async () => {
		const result = await launch(["whoami", "--json"], { env: { PIJ_RS_ADDR: "bad-address" } });
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout)).toMatchObject({
			ok: false,
			error: "refused",
			details: { code: "E-RS-REQUEST" },
		});
		expect(requests).toEqual([]);
	});

	it.each([
		"list",
		"sessions",
	])("routes bare %s and --json as GET without a legacy union", async (verb) => {
		const before = seedLegacy();
		for (const args of [[verb], [verb, "--json"]]) {
			const result = await launch(args);
			expect(result.code).toBe(0);
			expect(result.stdout).not.toContain("pij-old");
			if (args.includes("--json")) expect(result.stdout).toBe(`${SUCCESS}\n`);
		}
		expect(operationRequests()).toEqual(
			Array.from({ length: 2 }, () => ({
				// Plan 160: the shim always asks for each seat's size.
				path: verb === "list" ? "/v1/seats?sizes=true" : "/v1/shim/sessions",
				method: "GET",
				authorization: `Bearer ${KEY}`,
				body: "",
			})),
		);
		expect(legacySnapshot()).toBe(before);
	});

	it("forwards declared list filters as encoded GET query values without a legacy union", async () => {
		const before = seedLegacy();
		const folder = join(root, "folder with spaces & λ");
		const parent = "pij-parent/one?x=1&y=2";
		const args = [
			"list",
			"--harness",
			"claude",
			"--folder",
			folder,
			"--parent",
			parent,
			"--scope",
			"local",
			"--json",
		];
		expect(await launch(args)).toEqual({ code: 0, stdout: `${SUCCESS}\n`, stderr: "" });
		expect(operationRequests()).toHaveLength(1);
		const request = operationRequests()[0] as RequestRecord;
		expect(request).toMatchObject({ method: "GET", authorization: `Bearer ${KEY}`, body: "" });
		const url = new URL(request.path, `http://${addr}`);
		expect(url.pathname).toBe("/v1/seats");
		expect([...url.searchParams.entries()].sort()).toEqual(
			[
				["harness", "claude"],
				["folder", folder],
				["parent", parent],
				["scope", "local"],
				["sizes", "true"],
			].sort(),
		);
		expect(legacySnapshot()).toBe(before);
	});

	it.each([
		["list", "--role", "worker"],
		["sessions", "--here"],
		["sessions", "--harness", "claude"],
		["sessions", "--folder", "some-folder"],
		["sessions", "--parent", "pij-parent"],
		["sessions", "--scope", "local"],
	])("refuses unsupported query arguments instead of dropping them: %j", async (...args) => {
		const result = await launch([...args, "--json"]);
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout).details.code).toBe("E-RS-UNPORTED");
		expect(requests).toEqual([]);
	});

	it.each([
		["report", "now", "did", "next", "--state", "question", "--note", "--help"],
		["send", "pij-target", "--help"],
		["report", "now", "--help"],
	])("forwards --help in argument/value slots to the daemon unchanged: %j", async (...args) => {
		const argv = [...args, "--json"];
		expect(await launch(argv)).toEqual({ code: 0, stdout: `${SUCCESS}\n`, stderr: "" });
		expect(operationRequests()).toHaveLength(1);
		const request = operationRequests()[0] as RequestRecord;
		expect(request).toMatchObject({
			path: args[0] === "send" ? "/v1/shim/send" : "/v1/report",
			method: "POST",
		});
		expect(JSON.parse(request.body).argv).toEqual(argv);
	});

	it.each([
		false,
		true,
	])("keeps unambiguous verb --help local with trailing --json=%s", async (json) => {
		const result = await launch(["report", "--help", ...(json ? ["--json"] : [])]);
		expect(result).toMatchObject({ code: 0, stderr: "" });
		expect(result.stdout).toContain("pij report: POST /v1/report");
		expect(requests).toEqual([]);
	});

	it.each([
		"anomalies",
		"decisions",
	])("forwards %s argv and caller to the existing POST", async (verb) => {
		const args = [verb, "--json"];
		const result = await launch(args, {
			env: { TMUX_PANE: "%137", PIJ_SESSION_ID: "caller-evidence" },
		});
		expect(result).toEqual({ code: 0, stdout: `${SUCCESS}\n`, stderr: "" });
		expect(operationRequests()).toHaveLength(1);
		const request = operationRequests()[0] as RequestRecord;
		expect(request).toMatchObject({
			path: `/v1/${verb}`,
			method: "POST",
			authorization: `Bearer ${KEY}`,
		});
		expect(JSON.parse(request.body)).toMatchObject({
			argv: args,
			caller: {
				cwd: root,
				pid: expect.any(Number),
				tmuxPane: "%137",
				pijSessionId: "caller-evidence",
			},
		});
	});

	it.each([
		["adopt", "%137", "--harness", "claude"],
		["send", "pij-old", "--command", "compact"],
		["compact-self", "--pane", "%137"],
	])("lets daemon admission decide %j despite conflicting legacy identity", async (...argv) => {
		const before = seedLegacy();
		reply = () => ({ status: 403, body: REFUSAL });
		const result = await launch([...argv, "--json"]);
		expect(result).toEqual({ code: 4, stdout: REFUSAL, stderr: "" });
		expect(operationRequests()).toHaveLength(1);
		expect(legacySnapshot()).toBe(before);
		expect(existsSync(join(root, "tmux-called"))).toBe(false);
	});

	it("reports generation as a read-only rs inventory without operation calls", async () => {
		const result = await launch(["generation", "send", "--json"]);
		expect(result.code).toBe(0);
		expect(JSON.parse(result.stdout)).toMatchObject([{ verb: "send", generation: "rs", addr }]);
		expect(operationRequests()).toEqual([]);
		expect(legacySnapshot()).toBe("absent");
	});

	it("gives unported diagnostics a nonzero exit and no false legacy destination", async () => {
		const result = await launch(["generation", "spawn", "--json"]);
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout)).toMatchObject([{ verb: "spawn", generation: "rs-failed" }]);
		expect(requests).toEqual([]);
	});
});

describe("complete HTTP output", () => {
	it.each([
		"",
		"\n",
		"\r\n",
		"\n\n",
	])("preserves success envelope bytes with %j termination", async (ending) => {
		reply = () => ({ status: 200, body: SUCCESS + ending });
		expect(await launch(["--json", "project", "list"])).toEqual({
			code: 0,
			stdout: SUCCESS + (ending || "\n"),
			stderr: "",
		});
	});

	it.each([
		200, 401, 403, 404, 409, 500,
	])("preserves every daemon refusal field at HTTP %i and exits nonzero", async (status) => {
		reply = () => ({ status, body: REFUSAL });
		expect(await launch(["whoami", "--json"])).toEqual({ code: 4, stdout: REFUSAL, stderr: "" });
	});

	it("preserves the original health-auth refusal before any operation", async () => {
		const raw =
			' { "v":2, "ok":false, "command":"health", "error":"auth", "meta":"private key rejected", "details":{"challenge":"keep this"}, "future": [3,2,1] }\n';
		healthReply = { status: 401, body: raw };
		const before = seedLegacy();
		expect(await launch(["whoami", "--json"])).toEqual({ code: 4, stdout: raw, stderr: "" });
		expect(requests.map((request) => request.path)).toEqual(["/health"]);
		expect(legacySnapshot()).toBe(before);
	});

	it("keeps human refusal output on stderr", async () => {
		reply = () => ({ status: 409, body: REFUSAL });
		const result = await launch(["whoami"]);
		expect(result).toMatchObject({ code: 4, stdout: "" });
		expect(result.stderr).toContain("daemon admission refused");
	});

	it.each([
		"not json",
		'{"v":1,"ok":true,"command":"old","data":{}}',
		'{"v":2,"ok":true,"command":"missing-data"}',
	])("does not pass invalid envelopes through --json: %s", async (body) => {
		reply = () => ({ status: 200, body });
		const result = await launch(["whoami", "--json"]);
		expect(result.code).not.toBe(0);
		expect(JSON.parse(result.stdout)).toMatchObject({
			ok: false,
			v: 2,
			details: { code: "E-RS-WIRE" },
		});
		expect(result.stdout).not.toBe(`${body}\n`);
	});

	it("drains a multi-megabyte envelope through a real pipe with a delayed reader", async () => {
		const raw = JSON.stringify({
			ok: true,
			command: "project list",
			v: 2,
			data: { literal: "λ".repeat(1_500_000) },
			meta: "complete final field",
		});
		reply = () => ({ status: 200, body: raw });
		const result = await launch(["project", "list", "--json"], { delayedReader: true });
		expect(result).toEqual({ code: 0, stdout: `${raw}\n`, stderr: "" });
	});

	it("acknowledges inbox jobs only after successfully writing their complete output", async () => {
		const raw = JSON.stringify({
			ok: true,
			command: "inbox",
			v: 2,
			data: [
				{ job_id: "job-1", message: { from: "peer", body: "literal body" } },
				{ job_id: 2, message: { body: "second" } },
			],
			meta: "keep claims",
		});
		reply = (request) => ({ status: 200, body: request.path.endsWith("/ack") ? SUCCESS : raw });
		const result = await launch(["inbox", "check", "--json"]);
		expect(result).toEqual({ code: 0, stdout: `${raw}\n`, stderr: "" });
		expect(operationRequests().map((request) => request.path)).toEqual([
			"/v1/shim/inbox",
			"/v1/shim/inbox/ack",
			"/v1/shim/inbox/ack",
		]);
		expect(
			operationRequests()
				.slice(1)
				.map((request) => JSON.parse(request.body).job_id),
		).toEqual(["job-1", 2]);
	});

	it("does not acknowledge an inbox claim when the real stdout pipe is broken", async () => {
		reply = () => ({
			status: 200,
			body: JSON.stringify({
				ok: true,
				command: "inbox",
				v: 2,
				data: [{ job_id: "must-remain-claimable", message: { body: "not delivered" } }],
			}),
		});
		const result = await launch(["inbox", "--json"], { brokenStdout: true });
		expect(result.code).not.toBe(0);
		expect(result.stderr).toMatch(/EPIPE|broken pipe/i);
		expect(operationRequests().map((request) => request.path)).toEqual(["/v1/shim/inbox"]);
	});

	it("does not acknowledge refused claims", async () => {
		reply = () => ({ status: 409, body: REFUSAL });
		expect((await launch(["inbox", "--json"])).code).not.toBe(0);
		expect(operationRequests().map((request) => request.path)).toEqual(["/v1/shim/inbox"]);
	});

	it("keeps delivered output successful when acknowledgment warns", async () => {
		const raw = JSON.stringify({
			ok: true,
			command: "inbox",
			v: 2,
			data: [{ job_id: 1, message: { body: "delivered" } }],
		});
		reply = (request) =>
			request.path.endsWith("/ack") ? { status: 500, body: REFUSAL } : { status: 200, body: raw };
		const result = await launch(["inbox", "--json"]);
		expect(result).toMatchObject({ code: 0, stdout: `${raw}\n` });
		expect(result.stderr).toContain("acknowledgement failed");
	});
});

describe("native commit-trailers child boundary", () => {
	it.each([
		{ json: false, code: 0 },
		{ json: true, code: 0 },
		{ json: false, code: 23 },
		{ json: true, code: 23 },
	])("forwards exact streams and exit without HTTP or JSON wrapping: %j", async ({
		json,
		code,
	}) => {
		const record = join(root, "native-invocation.json");
		const stdout = "Pij-Task: task-id\nPij-Node: node-id";
		const stderr = "native diagnostic without newline";
		const binary = join(bin, "pij-rs");
		writeFileSync(
			binary,
			`#!${process.execPath}\nconst fs = require('node:fs');\nfs.writeFileSync(${JSON.stringify(record)}, JSON.stringify({ argv: process.argv.slice(2), cwd: process.cwd(), home: process.env.HOME }));\nprocess.stdout.write(${JSON.stringify(stdout)});\nprocess.stderr.write(${JSON.stringify(stderr)});\nprocess.exitCode = ${code};\n`,
		);
		chmodSync(binary, 0o755);
		const args = [
			"commit-trailers",
			"--node",
			"literal ; $(not-a-shell)",
			...(json ? ["--json"] : []),
		];
		const before = seedLegacy();
		expect(await launch(args)).toEqual({ code, stdout, stderr });
		expect(JSON.parse(readFileSync(record, "utf8"))).toEqual({
			argv: ["--state-dir", stateDir, "--addr", addr, ...args],
			cwd: root,
			home,
		});
		expect(requests).toEqual([]);
		expect(legacySnapshot()).toBe(before);
	});

	it("forwards native --help to the child instead of rendering shim help", async () => {
		const record = join(root, "native-help.json");
		const binary = join(bin, "pij-rs");
		writeFileSync(
			binary,
			`#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(record)}, JSON.stringify(process.argv.slice(2))); process.stdout.write('native help, no shim wrapper');\n`,
		);
		chmodSync(binary, 0o755);
		expect(await launch(["commit-trailers", "--help", "--json"])).toEqual({
			code: 0,
			stdout: "native help, no shim wrapper",
			stderr: "",
		});
		expect(JSON.parse(readFileSync(record, "utf8"))).toEqual([
			"--state-dir",
			stateDir,
			"--addr",
			addr,
			"commit-trailers",
			"--help",
			"--json",
		]);
		expect(requests).toEqual([]);
	});

	it.each([
		{ signal: "SIGTERM", code: 143 },
		{ signal: "SIGINT", code: 130 },
	] as const)("forwards direct wrapper $signal to the native child and waits for inherited pipes to close", async ({
		signal,
		code,
	}) => {
		const record = join(root, "native-signal.json");
		const binary = join(bin, "pij-rs");
		writeFileSync(
			binary,
			`#!${process.execPath}\nconst fs = require('node:fs');\nprocess.on(${JSON.stringify(signal)}, () => { fs.writeFileSync(${JSON.stringify(record)}, JSON.stringify({ pid: process.pid, signal: ${JSON.stringify(signal)} })); process.removeAllListeners(${JSON.stringify(signal)}); process.kill(process.pid, ${JSON.stringify(signal)}); });\nsetTimeout(() => process.exit(99), 10000);\nprocess.stdout.write('native-ready\\n');\n`,
		);
		chmodSync(binary, 0o755);
		const result = await launch(["commit-trailers", "--json"], { signalAfterOutput: signal });
		expect(result).toEqual({ code, stdout: "native-ready\n", stderr: "" });
		const native = JSON.parse(readFileSync(record, "utf8"));
		expect(native.signal).toBe(signal);
		expect(() => process.kill(native.pid, 0)).toThrow();
		expect(requests).toEqual([]);
	});

	it("forwards an empty native stdout without appending a newline or envelope", async () => {
		const binary = join(bin, "pij-rs");
		writeFileSync(
			binary,
			`#!${process.execPath}\nprocess.stderr.write('native refusal'); process.exitCode = 7;\n`,
		);
		chmodSync(binary, 0o755);
		expect(await launch(["commit-trailers", "--json"])).toEqual({
			code: 7,
			stdout: "",
			stderr: "native refusal",
		});
		expect(requests).toEqual([]);
	});

	it("refuses a missing native executable without HTTP, legacy writes or JSON wrapping", async () => {
		const before = seedLegacy();
		const result = await launch(["commit-trailers", "--json"], { env: { PATH: bin } });
		expect(result).toMatchObject({ code: 127, stdout: "" });
		expect(result.stderr).toContain("pij-rs commit-trailers:");
		expect(result.stderr).toContain("ENOENT");
		expect(requests).toEqual([]);
		expect(legacySnapshot()).toBe(before);
	});
});

describe("independent report guidance", () => {
	it("routes first-person reports and retires completion self-pause", () => {
		const skill = readFileSync(join(REPO_ROOT, "skills/pij/SKILL.md"), "utf8");
		const routing = readFileSync(join(REPO_ROOT, "skills/pij/references/00-routing.md"), "utf8");
		const node = readFileSync(join(REPO_ROOT, "skills/pij/references/routes/node.md"), "utf8");
		expect(skill).toContain("`report` (`now/question/blocked/state/clear/verify`)");
		expect(node).toContain("Everything under `report` is a first-person claim about yourself.");
		expect(node).toContain('pij report question "<what I need from you>"');
		expect(node).toContain('pij report blocked "<what I am waiting on>"');
		expect(node).toMatch(/Actively working has no semantic\s+state\s+word/);
		expect(node).toContain("Inline markdown is supported");
		expect(node).toContain("newlines are refused");
		const liveGuidance = routing;
		expect(liveGuidance).toContain("If done, run `pij report state done`");
		for (const retired of [
			"If done, pause me",
			"pause the watchdog explicitly",
			"Genuinely done → `pij watchdog pause",
			"self-pause (`pij watchdog pause",
		])
			expect(liveGuidance).not.toContain(retired);
	});
});
