import { once } from "node:events";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PijDaemonClient } from "./daemon-http.js";
import { FakeTmux } from "./fakes.js";
import { describeSpawnedWorker, RustRuntimeSession } from "./rust-runtime.js";

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
	expect(result.value.registration.waitedMs).toBeGreaterThanOrEqual(50);
});

it.each([
	[
		"an older spawn id",
		{ spawn_id: "s-old", proc: { pid: 7, proc_start: stamp(Date.now() + 60_000) } },
	],
	[
		"no spawn id and a process older than the launch",
		{ proc: { pid: 7, proc_start: 20200101000000 } },
	],
])("never confirms a recycled pane held by a seat with %s (#44 B1)", async (_label, row) => {
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

it("confirms a spawn-id-less seat on the new pane whose process started after the launch", async () => {
	const runtime = new RustRuntimeSession(
		registeringDaemon(() => [
			{
				id: "pij-kid",
				harness: "omp",
				folder: "/repo",
				state: "idle",
				pane: "%900",
				proc: { pid: 7, proc_start: stamp(Date.now() + 1_000) },
			},
		]),
		new FakeTmux(),
		[],
	);
	await expect(
		runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" }),
	).resolves.toMatchObject({
		ok: true,
		value: { registration: { registered: true, id: "pij-kid" } },
	});
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
