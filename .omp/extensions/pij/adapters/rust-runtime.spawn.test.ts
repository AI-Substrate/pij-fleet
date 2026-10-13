import { once } from "node:events";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PijDaemonClient } from "./daemon-http.js";
import { FakeTmux } from "./fakes.js";
import { describeSpawnedWorker, RustRuntimeSession } from "./rust-runtime.js";
import { TmuxAdapter } from "./tmux.js";

// The registration wait is read at construction; keep it short in every test.
beforeEach(() => vi.stubEnv("PIJ_SPAWN_REGISTER_TIMEOUT_MS", "50"));
afterEach(() => vi.unstubAllEnvs());

function registeringDaemon(seats: () => readonly Record<string, unknown>[]) {
	return new PijDaemonClient({ addr: "127.0.0.1:1", stateDir: "/unused" }, "key", {
		fetch: async (url) => {
			const path = new URL(String(url)).pathname;
			const data =
				path === "/health" ? { retired_harnesses: [] } : { seats: seats(), unavailable: [] };
			return new Response(JSON.stringify({ ok: true, command: path, v: 2, data }));
		},
		readFile: async () => "key",
		processStart: () => 1,
	});
}

/** A tmux whose new pane (%900) shows `text`. */
function paneShowing(text: string): FakeTmux {
	return new (class extends FakeTmux {
		override capturePane(paneId: string): string | null {
			return paneId === "%900" ? text : null;
		}
	})();
}

/** `ps -o lstart` packed as YYYYMMDDhhmmss, local time: the proc_start shape. */
function stamp(ms: number): number {
	const at = new Date(ms);
	const two = (value: number) => String(value).padStart(2, "0");
	return Number(
		`${at.getFullYear()}${two(at.getMonth() + 1)}${two(at.getDate())}${two(at.getHours())}${two(at.getMinutes())}${two(at.getSeconds())}`,
	);
}

const SECRET = "review45_SYNTHETIC_SECRET";

it("never forwards pane text; the child's own boot-failure marker classifies it (#44 B2)", async () => {
	const tmux = paneShowing(
		`startup failed\nTOKEN=${SECRET}\npij: boot registration failed; this session has no seat. seat x is retired\n`,
	);
	const runtime = new RustRuntimeSession(
		registeringDaemon(() => []),
		tmux,
		[],
	);
	const result = await runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" });
	expect(result).toMatchObject({
		ok: true,
		value: { paneId: "%900", registration: { registered: false, diagnosis: "boot-failed" } },
	});
	if (!result.ok) throw new Error("expected a launch");
	expect(JSON.stringify(result)).not.toContain(SECRET);
	const text = describeSpawnedWorker(result.value);
	expect(text).toMatch(/^spawned, NOT registered \(boot-failed\)/);
	expect(text).toContain("%900");
	expect(text).not.toContain(SECRET);
});

it("classifies a pane without the marker as no-registration, still without its text", async () => {
	const runtime = new RustRuntimeSession(
		registeringDaemon(() => []),
		paneShowing(`TOKEN=${SECRET}\n`),
		[],
	);
	const result = await runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" });
	expect(result).toMatchObject({
		ok: true,
		value: { registration: { registered: false, diagnosis: "no-registration" } },
	});
	expect(JSON.stringify(result)).not.toContain(SECRET);
	if (!result.ok || result.value.registration.registered) throw new Error("expected unregistered");
	// It polled for the whole budget less the quarter kept for the pane check.
	expect(result.value.registration.waitedMs).toBeGreaterThanOrEqual(50 - Math.floor(50 / 4) - 1);
});

it.each([
	[
		"an older spawn id",
		{ spawn_id: "s-old", proc: { pid: 7, proc_start: stamp(Date.now() + 60_000) } },
	],
	["no spawn id and an older process", { proc: { pid: 7, proc_start: 20200101000000 } }],
	// Round 2: an old row started 400ms before the launch shares its second.
	[
		"no spawn id and a process from the launch's own second",
		{ proc: { pid: 7, proc_start: stamp(Date.now()) } },
	],
	[
		"no spawn id even with a process newer than the launch",
		{ proc: { pid: 7, proc_start: stamp(Date.now() + 60_000) } },
	],
])("never confirms a seat on the new pane with %s (#44 B1)", async (_label, row) => {
	const runtime = new RustRuntimeSession(
		registeringDaemon(() => [
			{
				id: "pij-old-incarnation",
				harness: "omp",
				folder: "/repo",
				state: "idle",
				pane: "%900",
				...row,
			},
		]),
		new FakeTmux(),
		[],
	);
	await expect(
		runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({
		ok: true,
		value: { paneId: "%900", registration: { registered: false } },
	});
});

it("starts no roster read at the exact polling expiry and gives capture only the budget left (#44 B3b)", async () => {
	vi.stubEnv("PIJ_SPAWN_REGISTER_TIMEOUT_MS", "1000");
	let now = 0;
	let rosterReads = 0;
	const captureBudgets: number[] = [];
	const client = new PijDaemonClient({ addr: "127.0.0.1:1", stateDir: "/unused" }, "key", {
		fetch: async (url) => {
			const path = new URL(String(url)).pathname;
			if (path === "/health")
				return new Response(
					JSON.stringify({ ok: true, command: path, v: 2, data: { retired_harnesses: [] } }),
				);
			rosterReads += 1;
			// The read lands exactly on the polling expiry: 1000ms less the capture slice.
			now += 750;
			return new Response(
				JSON.stringify({ ok: true, command: path, v: 2, data: { seats: [], unavailable: [] } }),
			);
		},
		readFile: async () => "key",
		processStart: () => 1,
	});
	const tmux = new (class extends FakeTmux {
		override capturePane(_paneId: string, timeoutMs: number): string | null {
			captureBudgets.push(timeoutMs);
			now += timeoutMs;
			return null;
		}
	})();
	const runtime = new RustRuntimeSession(client, tmux, [], () => now);
	const result = await runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" });
	expect(rosterReads).toBe(1);
	expect(captureBudgets).toEqual([250]);
	expect(result).toMatchObject({
		ok: true,
		value: { registration: { registered: false, waitedMs: 1000, diagnosis: "no-registration" } },
	});
});

it("skips the capture when no budget is left", async () => {
	vi.stubEnv("PIJ_SPAWN_REGISTER_TIMEOUT_MS", "1000");
	let now = 0;
	const client = new PijDaemonClient({ addr: "127.0.0.1:1", stateDir: "/unused" }, "key", {
		fetch: async (url) => {
			const path = new URL(String(url)).pathname;
			if (path !== "/health") now += 5_000;
			return new Response(
				JSON.stringify({
					ok: true,
					command: path,
					v: 2,
					data: path === "/health" ? { retired_harnesses: [] } : { seats: [], unavailable: [] },
				}),
			);
		},
		readFile: async () => "key",
		processStart: () => 1,
	});
	let captured = false;
	const tmux = new (class extends FakeTmux {
		override capturePane(): string | null {
			captured = true;
			return "pij: boot registration failed";
		}
	})();
	const runtime = new RustRuntimeSession(client, tmux, [], () => now);
	await expect(
		runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({ value: { registration: { diagnosis: "no-registration" } } });
	expect(captured).toBe(false);
});

it("hard-bounds the wait when a roster body never finishes (#44 B3)", async () => {
	const server = createServer((request, response) => {
		if (request.url === "/health") {
			response.end(
				JSON.stringify({ ok: true, command: "health", v: 2, data: { retired_harnesses: [] } }),
			);
			return;
		}
		response.writeHead(200, { "content-type": "application/json" });
		response.write('{"ok":true,');
	});
	server.listen(0, "127.0.0.1");
	await once(server, "listening");
	const { port } = server.address() as AddressInfo;
	try {
		const client = new PijDaemonClient({ addr: `127.0.0.1:${port}`, stateDir: "/unused" }, "key", {
			fetch,
			readFile: async () => "key",
			processStart: () => 1,
		});
		const runtime = new RustRuntimeSession(client, new FakeTmux(), []);
		const started = performance.now();
		const result = await runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" });
		expect(performance.now() - started).toBeLessThan(1_000);
		expect(result).toMatchObject({
			ok: true,
			value: { registration: { registered: false, rosterError: expect.any(String) } },
		});
	} finally {
		server.closeAllConnections();
		server.close();
	}
}, 3_000);

describe("one budget for the whole wait (#44 B3b)", () => {
	let dir: string | undefined;
	afterEach(() => {
		if (dir) rmSync(dir, { recursive: true, force: true });
		dir = undefined;
	});

	it("returns within the configured deadline when the roster hangs AND tmux capture stalls", async () => {
		dir = mkdtempSync(join(tmpdir(), "pij-spawn-stall-"));
		const bin = join(dir, "tmux");
		writeFileSync(
			bin,
			`#!/bin/sh
case "$1" in
new-window) printf '%%900 @1\\n' ;;
display-message) printf 'stub\\n' ;;
capture-pane) exec sleep 10 ;;
esac
exit 0
`,
		);
		chmodSync(bin, 0o755);
		vi.stubEnv("PATH", `${dir}:${process.env.PATH ?? ""}`);
		vi.stubEnv("TMUX_PANE", "%1");
		vi.stubEnv("PIJ_SPAWN_REGISTER_TIMEOUT_MS", "300");
		const server = createServer((request, response) => {
			if (request.url === "/health") {
				response.end(
					JSON.stringify({ ok: true, command: "health", v: 2, data: { retired_harnesses: [] } }),
				);
				return;
			}
			response.writeHead(200, { "content-type": "application/json" });
			response.write('{"ok":true,');
		});
		server.listen(0, "127.0.0.1");
		await once(server, "listening");
		const { port } = server.address() as AddressInfo;
		try {
			const client = new PijDaemonClient(
				{ addr: `127.0.0.1:${port}`, stateDir: "/unused" },
				"key",
				{ fetch, readFile: async () => "key", processStart: () => 1 },
			);
			const runtime = new RustRuntimeSession(client, new TmuxAdapter(), []);
			const result = await runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" });
			if (!result.ok || result.value.registration.registered)
				throw new Error("expected unregistered");
			// Deadline plus a fixed epsilon for process exit and timer slack.
			expect(result.value.registration.waitedMs).toBeLessThan(300 + 150);
			expect(result.value.registration.diagnosis).toBe("no-registration");
		} finally {
			server.closeAllConnections();
			server.close();
		}
	}, 5_000);
});

it("two parents launching in the same millisecond never share a spawn id (#44 B1c)", async () => {
	let now = 1_000;
	let bindSecond = false;
	const first = new FakeTmux({ paneStart: 900 });
	const second = new FakeTmux({ paneStart: 901 });
	const spawnIds = (tmux: FakeTmux) => tmux.windows.map((window) => window.opts.env.PIJ_SPAWN_ID);
	const bound = (id: string, spawnId: string | undefined) => ({
		id,
		harness: "omp",
		folder: "/repo",
		state: "idle",
		spawn_id: spawnId,
		proc: { pid: 7, proc_start: 1 },
	});
	const daemon = () =>
		registeringDaemon(() => {
			// The clock stands still until both parents have launched: one shared instant.
			if (first.windows.length > 0 && second.windows.length > 0) now += 10;
			return [
				bound("pij-first-child", spawnIds(first)[0]),
				...(bindSecond ? [bound("pij-second-child", spawnIds(second).at(-1))] : []),
			];
		});
	const parentA = new RustRuntimeSession(daemon(), first, [], () => now);
	const parentB = new RustRuntimeSession(daemon(), second, [], () => now);
	const [a, b] = await Promise.all([
		parentA.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
		parentB.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	]);
	expect(a).toMatchObject({
		ok: true,
		value: { registration: { registered: true, id: "pij-first-child" } },
	});
	expect(b).toMatchObject({
		ok: true,
		value: { paneId: "%901", registration: { registered: false } },
	});
	expect(spawnIds(first)[0]).toMatch(/^s-[0-9a-f]{32}$/);
	expect(spawnIds(second)[0]).toMatch(/^s-[0-9a-f]{32}$/);
	expect(spawnIds(first)[0]).not.toBe(spawnIds(second)[0]);
	bindSecond = true;
	await expect(
		parentB.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({
		ok: true,
		value: { registration: { registered: true, id: "pij-second-child" } },
	});
});

it("confirms a spawn once a bound seat carries its spawn id; an unbound row is not enough", async () => {
	vi.stubEnv("PIJ_SPAWN_REGISTER_TIMEOUT_MS", "5000");
	const tmux = new FakeTmux();
	let polls = 0;
	const runtime = new RustRuntimeSession(
		registeringDaemon(() => {
			polls += 1;
			const spawnId = tmux.windows[0]?.opts.env.PIJ_SPAWN_ID;
			const unbound = {
				id: "pij-prebind",
				harness: "omp",
				folder: "/repo",
				state: "idle",
				spawn_id: spawnId,
			};
			return polls < 2
				? [unbound]
				: [{ ...unbound, id: "pij-kid", proc: { pid: 7, proc_start: 1 } }];
		}),
		tmux,
		[],
	);
	await expect(
		runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({
		ok: true,
		value: { registration: { registered: true, id: "pij-kid" } },
	});
	expect(polls).toBe(2);
});

it("checks fresh daemon retirement policy before opening local panes", async () => {
	let retired: string[] = ["pi"];
	const client = new PijDaemonClient({ addr: "127.0.0.1:1", stateDir: "/unused" }, "key", {
		fetch: async () =>
			new Response(
				JSON.stringify({
					ok: true,
					command: "health",
					v: 2,
					data: { retired_harnesses: retired },
				}),
			),
		readFile: async () => "key",
		processStart: () => 1,
	});
	const tmux = new FakeTmux();
	const runtime = new RustRuntimeSession(client, tmux, []);
	await expect(
		runtime.spawn({ harness: "pi", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({
		ok: false,
		message: "harness pi is retired on this machine; use omp (or pass --allow-retired)",
	});
	expect(tmux.windows).toEqual([]);
	await expect(
		runtime.spawn({
			harness: "omp",
			model: "github-copilot/gpt-6-astra",
			effort: "high",
			cwd: "/repo",
			layout: "window",
		}),
	).resolves.toMatchObject({ ok: true });
	expect(tmux.windows[0]?.opts).toMatchObject({
		cmd: "omp",
		args: ["--auto-approve", "--model", "github-copilot/gpt-6-astra", "--thinking", "high"],
	});
	retired = ["omp"];
	await expect(
		runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({ ok: false });
	expect(tmux.windows).toHaveLength(1);
});

it("refuses spawning when the daemon cannot report retirement policy", async () => {
	const client = new PijDaemonClient({ addr: "127.0.0.1:1", stateDir: "/unused" }, "key", {
		fetch: async () =>
			new Response(JSON.stringify({ ok: true, command: "health", v: 2, data: {} })),
		readFile: async () => "key",
		processStart: () => 1,
	});
	const tmux = new FakeTmux();
	const runtime = new RustRuntimeSession(client, tmux, []);
	await expect(runtime.spawn({ harness: "omp", cwd: "/repo" })).resolves.toMatchObject({
		ok: false,
		code: "E-NOREG",
	});
	expect(tmux.windows).toEqual([]);
	expect(tmux.splits).toEqual([]);
});

it("forwards a placement role on the daemon spawn route and refuses it on the local omp/pi path", async () => {
	const bodies: Record<string, unknown>[] = [];
	const client = new PijDaemonClient({ addr: "127.0.0.1:1", stateDir: "/unused" }, "key", {
		fetch: async (url, init) => {
			if (String(url).endsWith("/health"))
				return new Response(
					JSON.stringify({ ok: true, command: "health", v: 2, data: { retired_harnesses: [] } }),
				);
			bodies.push(JSON.parse(String(init?.body)));
			return new Response(
				JSON.stringify({
					ok: true,
					command: "pij spawn",
					v: 2,
					data: { id: "pij-kid", spawn_id: "s1", pane: "%9" },
				}),
			);
		},
		readFile: async () => "key",
		processStart: () => 1,
	});
	const tmux = new FakeTmux();
	const runtime = new RustRuntimeSession(client, tmux, []);
	await expect(
		runtime.spawn({ harness: "copilot", cwd: "/repo", layout: "window", role: "pm" }),
	).resolves.toMatchObject({ ok: true });
	expect(bodies[0]).toMatchObject({ role: "pm", caller: { PIJ_SESSION_ID: expect.any(String) } });

	await expect(
		runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window", role: "worker" }),
	).resolves.toMatchObject({ ok: false, code: "E-ARG" });
	expect(tmux.windows).toEqual([]);
	expect(tmux.splits).toEqual([]);
});
