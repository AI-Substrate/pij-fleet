import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { PijDaemonClient } from "./daemon-http.js";
import { FakeTmux } from "./fakes.js";
import { RustRuntimeSession } from "./rust-runtime.js";

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

it("reports a spawned pane whose child never registers as spawned, not registered (#44)", async () => {
	const tmux = new (class extends FakeTmux {
		override capturePane(paneId: string): string | null {
			return paneId === "%900"
				? "omp v18\npij: boot registration failed; this session has no seat.\n\n"
				: null;
		}
	})();
	const runtime = new RustRuntimeSession(
		registeringDaemon(() => []),
		tmux,
		[],
	);
	const result = await runtime.spawn({ harness: "omp", cwd: "/repo", layout: "window" });
	expect(result).toMatchObject({
		ok: true,
		value: {
			paneId: "%900",
			registration: {
				registered: false,
				waitedMs: expect.any(Number),
				paneTail: "omp v18\npij: boot registration failed; this session has no seat.",
			},
		},
	});
	if (!result.ok || result.value.registration.registered) throw new Error("expected unregistered");
	expect(result.value.registration.waitedMs).toBeGreaterThanOrEqual(50);
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
