import { expect, it } from "vitest";
import { PijDaemonClient } from "./daemon-http.js";
import { FakeTmux } from "./fakes.js";
import { RustRuntimeSession } from "./rust-runtime.js";

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
