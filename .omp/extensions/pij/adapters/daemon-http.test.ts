import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PijNoDaemonError } from "../core/daemon-wire.js";
import {
	type DaemonHttpDeps,
	daemonGenerationPreference,
	daemonLocation,
	detectDaemonGeneration,
	PijDaemonClient,
} from "./daemon-http.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const WIRE = resolve(HERE, "../../../../crates/testkit/fixtures/wire");
const fixture = (name: string): string => readFileSync(resolve(WIRE, name), "utf8");
const envelopeFixture = (name: string): string => {
	const envelope = JSON.parse(fixture(name)) as Record<string, unknown>;
	return JSON.stringify({ ...envelope, v: 2 });
};
const ERROR_ENVELOPES = JSON.parse(
	readFileSync(
		resolve(HERE, "../../../../crates/testkit/fixtures/golden/api/error-envelopes.json"),
		"utf8",
	),
) as Array<{ command: string; error: string; meta: string }>;
const roots: string[] = [];

function root(): string {
	const path = mkdtempSync(join(tmpdir(), "pij-rs-extension-"));
	roots.push(path);
	return path;
}

function deps(fetchImpl: typeof fetch): DaemonHttpDeps {
	return { fetch: fetchImpl, readFile, processStart: () => 20260829103052 };
}

function eventStream(frames: readonly Record<string, unknown>[], keepOpen = false): Response {
	const encoder = new TextEncoder();
	return new Response(
		new ReadableStream<Uint8Array>({
			start(controller) {
				controller.enqueue(
					encoder.encode(
						`${[
							JSON.stringify({ hello: true, v: 1, build: "pij-rs test" }),
							...frames.map((frame) => JSON.stringify(frame)),
						].join("\n")}\n`,
					),
				);
				if (!keepOpen) controller.close();
			},
		}),
	);
}

function frame(machine: string, cursor: number): Record<string, unknown> {
	return {
		machine,
		cursor,
		event: { v: 1, at: cursor, kind: "future.kind", payload: "{}" },
	};
}

function refusal(kind: string, message: string, status: number): Response {
	return new Response(
		JSON.stringify({ ok: false, command: "pij events", v: 2, error: kind, meta: message }),
		{ status },
	);
}

function refused(runtime: "node" | "bun"): Error {
	if (runtime === "bun") {
		return Object.assign(new Error("Unable to connect. Is the computer able to access the url?"), {
			code: "ConnectionRefused",
		});
	}
	return Object.assign(new TypeError("fetch failed"), { cause: { code: "ECONNREFUSED" } });
}

afterEach(() => {
	for (const path of roots.splice(0)) rmSync(path, { recursive: true, force: true });
});

describe("daemon generation controls", () => {
	it("defaults to rs and accepts only the two explicit generation values", () => {
		expect(daemonGenerationPreference({})).toBe("rs");
		expect(daemonGenerationPreference({ PIJ_DAEMON_GENERATION: "rs" })).toBe("rs");
		expect(daemonGenerationPreference({ PIJ_DAEMON_GENERATION: "legacy" })).toBe("legacy");
		for (const value of ["", " rust", "ts"]) {
			expect(() => daemonGenerationPreference({ PIJ_DAEMON_GENERATION: value })).toThrow(
				`PIJ_DAEMON_GENERATION has malformed value ${JSON.stringify(value)}`,
			);
		}
	});

	it("resolves strict extension address and state controls", () => {
		expect(daemonLocation({}, "/home/test")).toEqual({
			addr: "127.0.0.1:7461",
			stateDir: "/home/test/.pij-rs",
		});
		expect(
			daemonLocation({ PIJ_RS_ADDR: "localhost:8123", PIJ_RS_STATE_DIR: "/tmp/pij-rs" }),
		).toEqual({ addr: "localhost:8123", stateDir: "/tmp/pij-rs" });
		for (const value of ["", "localhost", "localhost:0", "localhost:65536"]) {
			expect(() => daemonLocation({ PIJ_RS_ADDR: value })).toThrow(
				`PIJ_RS_ADDR has malformed value ${JSON.stringify(value)}`,
			);
		}
		for (const value of ["", "  "]) {
			expect(() => daemonLocation({ PIJ_RS_STATE_DIR: value })).toThrow(
				`PIJ_RS_STATE_DIR has malformed value ${JSON.stringify(value)}`,
			);
		}
	});

	it("uses the authenticated Rust daemon when rs is selected", async () => {
		const base = root();
		const stateDir = join(base, "rust");
		mkdirSync(stateDir);
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const fetchMock = vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
			expect(init?.headers).toEqual({ Authorization: "Bearer secret" });
			return new Response(envelopeFixture("health-v1.json"));
		});

		const generation = await detectDaemonGeneration(
			{ addr: "127.0.0.1:7461", stateDir },
			"rs",
			deps(fetchMock as typeof fetch),
		);

		expect(generation.kind).toBe("rust");
	});

	it("preserves validated health auth refusal bytes when the daemon key is absent", async () => {
		const auth = ERROR_ENVELOPES.find((envelope) => envelope.error === "auth");
		expect(auth).toBeDefined();
		const rawEnvelope = ` \n${JSON.stringify(auth, null, "\t")}\n`;
		const fetchMock = vi.fn(async () => new Response(rawEnvelope, { status: 401 }));
		await expect(
			detectDaemonGeneration(
				{ addr: "127.0.0.1:7999", stateDir: root() },
				"rs",
				deps(fetchMock as typeof fetch),
			),
		).rejects.toMatchObject({ kind: "auth", rawEnvelope });
	});

	it("selects legacy explicitly without probing Rust", async () => {
		const fetchMock = vi.fn(async () => {
			throw new Error("must not probe");
		});
		await expect(
			detectDaemonGeneration(
				{ addr: "127.0.0.1:7461", stateDir: "/unused" },
				"legacy",
				deps(fetchMock as typeof fetch),
			),
		).resolves.toMatchObject({ kind: "legacy" });
		expect(fetchMock).not.toHaveBeenCalled();
	});

	it.each([
		"node",
		"bun",
	] as const)("fails loudly instead of falling back when selected Rust is unavailable in %s", async (runtime) => {
		const base = root();
		const fetchMock = vi.fn(async () => {
			throw refused(runtime);
		});
		try {
			await detectDaemonGeneration(
				{ addr: "127.0.0.1:7999", stateDir: join(base, "missing-rust") },
				"rs",
				deps(fetchMock as typeof fetch),
			);
			throw new Error("missing selected Rust daemon was accepted");
		} catch (error) {
			expect(error).toBeInstanceOf(PijNoDaemonError);
			expect(error).toMatchObject({
				code: "PIJ_NO_DAEMON",
				addr: "127.0.0.1:7999",
				stateDir: join(base, "missing-rust"),
			});
			expect(String(error)).toContain("Start pij-rs");
			expect(String(error)).toContain("just bounce-rs");
			expect(String(error)).not.toContain("legacy");
		}
	});
});

describe("pij-rs HTTP client", () => {
	it("authenticates registration and sends the exact structural identity", async () => {
		const calls: Array<{ url: string; init?: RequestInit }> = [];
		const fetchMock = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
			calls.push({ url: String(url), init });
			return new Response(envelopeFixture("register-v1.json"));
		});
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir: "/tmp/state" },
			"secret",
			deps(fetchMock as typeof fetch),
		);
		await client.register({
			id: "pij-test-seat",
			harness: "omp",
			folder: "/abs/tree",
			pane: "%42",
			pid: 4242,
			proc_start: 20260829103052,
			spawn_id: "spawn-1",
			model: "github-copilot/gpt-5.6-sol-fast",
			provider: "github-copilot",
			effort: "high",
			relay: false,
		});

		expect(calls).toHaveLength(1);
		expect(calls[0]?.url).toBe("http://127.0.0.1:7461/v1/register");
		expect(calls[0]?.init?.headers).toMatchObject({ Authorization: "Bearer secret" });
		expect(JSON.parse(String(calls[0]?.init?.body))).toMatchObject({
			pid: 4242,
			proc_start: 20260829103052,
			spawn_id: "spawn-1",
		});
	});

	it("encodes swallowed delivery failure without changing ordinary or control acknowledgements", async () => {
		const requests: unknown[] = [];
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:1", stateDir: "/unused" },
			"secret",
			deps(
				vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
					const request = JSON.parse(String(init?.body)) as { job_id: number };
					requests.push(request);
					return new Response(
						JSON.stringify({ ok: true, v: 2, command: "pij inbox", data: request.job_id }),
					);
				}) as typeof fetch,
			),
		);
		await client.ackInbox("seat", 1);
		await client.ackInbox("seat", 2, { outcome: "executed" });
		await expect(
			client.ackInbox("seat", 3, undefined, "undelivered:harness-swallowed"),
		).resolves.toBe(3);
		expect(requests).toEqual([
			{ seat: "seat", job_id: 1 },
			{ seat: "seat", job_id: 2, control_outcome: { outcome: "executed" } },
			{ seat: "seat", job_id: 3, delivery_outcome: "undelivered:harness-swallowed" },
		]);
	});

	it("busy-seat heartbeat renews the claim through its authenticated dedicated endpoint", async () => {
		const requests: Array<{ path: string; init?: RequestInit }> = [];
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:1", stateDir: "/unused" },
			"secret",
			deps(
				vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
					requests.push({ path: new URL(String(url)).pathname, init });
					return new Response(
						JSON.stringify({
							ok: true,
							v: 2,
							command: "pij inbox",
							data: { job_id: 17, state: "running" },
						}),
					);
				}) as typeof fetch,
			),
		);
		await expect(client.heartbeatInbox("busy seat", 17)).resolves.toEqual({
			job_id: 17,
			state: "running",
		});
		expect(requests).toHaveLength(1);
		expect(requests[0]?.path).toBe("/v1/inbox/heartbeat");
		expect(requests[0]?.init?.method).toBe("POST");
		expect(requests[0]?.init?.headers).toMatchObject({
			Authorization: "Bearer secret",
			"Content-Type": "application/json",
		});
		expect(JSON.parse(String(requests[0]?.init?.body))).toEqual({ seat: "busy seat", job_id: 17 });
	});

	it("forwards unknown frames and keeps reading pushed turns", async () => {
		const fetchMock = vi.fn(async () => new Response(fixture("events-v1.ndjson")));
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir: "/tmp/state" },
			"secret",
			{ ...deps(fetchMock as typeof fetch), readFile: async () => "secret" },
		);
		const frames: string[] = [];
		const errors: Error[] = [];
		const stop = await client.watchEvents(
			(frame) => frames.push(frame.event.kind),
			(error) => errors.push(error),
		);
		try {
			await vi.waitFor(() => expect(frames).toEqual(["future.kind", "message.pushed"]));
			expect(errors).toEqual([]);
		} finally {
			stop();
		}
	});

	it("reconnects with the complete cursor map and refreshes the daemon key", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "old-key\n");
		const urls: URL[] = [];
		const authorizations: string[] = [];
		let attach = 0;
		const fetchMock = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
			urls.push(new URL(String(url)));
			authorizations.push(String((init?.headers as Record<string, string>).Authorization));
			attach += 1;
			if (attach === 1) {
				writeFileSync(join(stateDir, "daemon.key"), "new-key\n");
				return eventStream([frame("alpha", 7), frame("beta", 3)]);
			}
			if (attach === 2) return eventStream([frame("alpha", 8)], true);
			throw new Error(`unexpected attach ${attach}`);
		});
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir },
			"old-key",
			deps(fetchMock as typeof fetch),
		);
		const cursors: number[] = [];
		const notices: string[] = [];
		const stop = await client.watchEvents(
			(received) => cursors.push(received.cursor),
			(error) => {
				throw error;
			},
			(notice) => notices.push(notice),
		);
		try {
			await vi.waitFor(() => expect(cursors).toEqual([7, 3, 8]));
			expect(JSON.parse(urls[1]?.searchParams.get("since") ?? "null")).toEqual({
				alpha: 7,
				beta: 3,
			});
			expect(authorizations).toEqual(["Bearer old-key", "Bearer new-key"]);
			expect(notices).toEqual([
				"pij: daemon stream lost, reconnecting…",
				"pij: re-attached at cursor 7",
			]);
		} finally {
			stop();
		}
	});

	it("falls back to a live-only attach when the daemon refuses replay cursors", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const urls: URL[] = [];
		let attach = 0;
		const fetchMock = vi.fn(async (url: string | URL | Request) => {
			urls.push(new URL(String(url)));
			attach += 1;
			if (attach === 1) return eventStream([frame("alpha", 7)]);
			if (attach === 2) return refusal("cursor_reset", "cursor 7 is beyond spine 2", 409);
			if (attach === 3) return eventStream([], true);
			throw new Error(`unexpected attach ${attach}`);
		});
		const notices: string[] = [];
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir },
			"secret",
			deps(fetchMock as typeof fetch),
		);
		const stop = await client.watchEvents(
			() => {},
			(error) => {
				throw error;
			},
			(notice) => notices.push(notice),
		);
		try {
			await vi.waitFor(() => expect(urls).toHaveLength(3));
			expect(urls[1]?.searchParams.has("since")).toBe(true);
			expect(urls[2]?.searchParams.has("since")).toBe(false);
			expect(notices).toContain("pij: cursor 7 is beyond spine 2; re-attaching live-only");
		} finally {
			stop();
		}
	});

	it("refreshes and retries any request once after an authentication refusal", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "old-key\n");
		const authorizations: string[] = [];
		const fetchMock = vi.fn(async (_url: string | URL | Request, init?: RequestInit) => {
			authorizations.push(String((init?.headers as Record<string, string>).Authorization));
			if (authorizations.length === 1) {
				writeFileSync(join(stateDir, "daemon.key"), "new-key\n");
				return refusal("auth", "missing or wrong bearer token", 401);
			}
			return new Response(
				JSON.stringify({
					ok: true,
					command: "pij send",
					v: 2,
					data: { msg_id: "m1", outcome: { outcome: "queued" }, at: 1 },
				}),
			);
		});
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir },
			"old-key",
			deps(fetchMock as typeof fetch),
		);
		await expect(
			client.send({ from: "pij-a", to: { seat: "pij-b" }, body: "hello", msg_id: "m1" }),
		).resolves.toMatchObject({ msg_id: "m1" });
		expect(authorizations).toEqual(["Bearer old-key", "Bearer new-key"]);

		const alwaysUnauthorized = vi.fn(async () => refusal("auth", "still wrong", 403));
		const failing = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir },
			"old-key",
			deps(alwaysUnauthorized as typeof fetch),
		);
		await expect(failing.seats()).rejects.toThrow("still wrong");
		expect(alwaysUnauthorized).toHaveBeenCalledTimes(2);
	});

	it("caps reconnect backoff after exponential growth", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const sleeps: Array<{ readonly ms: number; readonly release: () => void }> = [];
		const fetchMock = vi.fn(async () => eventStream([]));
		const baseDeps = deps(fetchMock as typeof fetch);
		const client = new PijDaemonClient({ addr: "127.0.0.1:7461", stateDir }, "secret", {
			...baseDeps,
			random: () => 1,
			sleep: (ms) =>
				new Promise<void>((resolveSleep) => sleeps.push({ ms, release: resolveSleep })),
		});
		const stop = await client.watchEvents(
			() => {},
			(error) => {
				throw error;
			},
		);
		try {
			const expected = [300, 600, 1_200, 2_400, 4_800, 5_000, 5_000];
			for (let index = 0; index < expected.length; index += 1) {
				await vi.waitFor(() => expect(sleeps).toHaveLength(index + 1));
				expect(sleeps[index]?.ms).toBe(expected[index]);
				sleeps[index]?.release();
			}
		} finally {
			stop();
		}
	});

	it("reports a stream protocol error while continuing the supervised reconnect", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		let attach = 0;
		const fetchMock = vi.fn(async () => {
			attach += 1;
			if (attach === 1) {
				return new Response(
					`${JSON.stringify({ hello: true, v: 1, build: "pij-rs test" })}\nnot-json\n`,
				);
			}
			return eventStream([], true);
		});
		const errors: Error[] = [];
		const client = new PijDaemonClient({ addr: "127.0.0.1:7461", stateDir }, "secret", {
			...deps(fetchMock as typeof fetch),
			random: () => 0.5,
			sleep: async () => {},
		});
		const stop = await client.watchEvents(
			() => {},
			(error) => errors.push(error),
		);
		try {
			await vi.waitFor(() => expect(attach).toBe(2));
			expect(errors).toHaveLength(1);
			expect(errors[0]?.message).toContain("daemon event line is invalid JSON");
		} finally {
			stop();
		}
	});

	it("does not advance a cursor until asynchronous frame consumption succeeds", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const urls: URL[] = [];
		const fetchMock = vi.fn(async (url: string | URL | Request) => {
			urls.push(new URL(String(url)));
			if (urls.length === 1) return eventStream([frame("alpha", 7)]);
			return eventStream([], true);
		});
		const errors: Error[] = [];
		const client = new PijDaemonClient({ addr: "127.0.0.1:7461", stateDir }, "secret", {
			...deps(fetchMock as typeof fetch),
			random: () => 0.5,
			sleep: async () => {},
		});
		const failedConsumption = Promise.reject(new Error("consume failed"));
		void failedConsumption.catch(() => {});
		const stop = await client.watchEvents(
			() => failedConsumption,
			(error) => errors.push(error),
		);
		try {
			await vi.waitFor(() => expect(urls).toHaveLength(2));
			expect(urls[1]?.searchParams.has("since")).toBe(false);
			expect(errors.map((error) => error.message)).toContain("consume failed");
		} finally {
			stop();
		}
	});

	it("keeps the HTTP status backstop when a failed response has a success envelope", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const fetchMock = vi.fn(
			async () =>
				new Response(JSON.stringify({ ok: true, command: "pij events", v: 2, data: {} }), {
					status: 500,
				}),
		);
		const client = new PijDaemonClient(
			{ addr: "127.0.0.1:7461", stateDir },
			"secret",
			deps(fetchMock as typeof fetch),
		);
		await expect(
			client.watchEvents(
				() => {},
				() => {},
			),
		).rejects.toThrow("pij events failed with HTTP 500");
	});

	it("resets reconnect backoff after a stream consumes a frame", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const sleeps: Array<{ readonly ms: number; readonly release: () => void }> = [];
		let attach = 0;
		const fetchMock = vi.fn(async () => {
			attach += 1;
			return attach === 2 ? eventStream([frame("alpha", 1)]) : eventStream([]);
		});
		const client = new PijDaemonClient({ addr: "127.0.0.1:7461", stateDir }, "secret", {
			...deps(fetchMock as typeof fetch),
			random: () => 0.5,
			sleep: (ms) =>
				new Promise<void>((resolveSleep) => sleeps.push({ ms, release: resolveSleep })),
		});
		const stop = await client.watchEvents(
			() => {},
			(error) => {
				throw error;
			},
		);
		try {
			await vi.waitFor(() => expect(sleeps).toHaveLength(1));
			expect(sleeps[0]?.ms).toBe(250);
			sleeps[0]?.release();
			await vi.waitFor(() => expect(sleeps).toHaveLength(2));
			expect(sleeps[1]?.ms).toBe(250);
		} finally {
			stop();
		}
	});

	it("bounds poison-frame retries, notices once, and continues with the next frame", async () => {
		const stateDir = root();
		writeFileSync(join(stateDir, "daemon.key"), "secret\n");
		const sleeps: Array<{ readonly release: () => void }> = [];
		let attaches = 0;
		const poison = {
			machine: "alpha",
			cursor: 1,
			event: {
				v: 1,
				at: 1,
				kind: "message.pushed",
				seat: "pij-target",
				payload: JSON.stringify({ msg_id: "poison-1", from: "pij-a", body: 42 }),
			},
		};
		const fetchMock = vi.fn(async () => {
			attaches += 1;
			return eventStream([poison, frame("alpha", 2)], attaches >= 5);
		});
		const notices: string[] = [];
		const consumed: number[] = [];
		const client = new PijDaemonClient({ addr: "127.0.0.1:7461", stateDir }, "secret", {
			...deps(fetchMock as typeof fetch),
			random: () => 0.5,
			sleep: (_ms, signal) =>
				new Promise<void>((resolveSleep) => {
					const release = (): void => {
						signal.removeEventListener("abort", release);
						resolveSleep();
					};
					signal.addEventListener("abort", release, { once: true });
					sleeps.push({ release });
				}),
		});
		const stop = await client.watchEvents(
			(received) => {
				if (received.cursor === 1) throw new Error("body must be a string");
				consumed.push(received.cursor);
			},
			() => {},
			(notice) => notices.push(notice),
		);
		try {
			for (let attempt = 1; attempt < 5; attempt += 1) {
				await vi.waitFor(() => expect(sleeps).toHaveLength(attempt));
				sleeps[attempt - 1]?.release();
				await vi.waitFor(() => expect(attaches).toBe(attempt + 1));
			}
			await vi.waitFor(() => expect(consumed).toEqual([2]));
			expect(notices.filter((notice) => notice.includes("poison-1"))).toEqual([
				"pij: skipped poison frame poison-1 after 5 attempts: body must be a string; message remains in pij inbox",
			]);
		} finally {
			stop();
		}
	});
});
