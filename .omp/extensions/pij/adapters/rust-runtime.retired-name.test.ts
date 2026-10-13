import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Registration, RustSeatDescriptor } from "../core/daemon-wire.js";
import { memorableIdentitySeed } from "../core/discovery.js";
import { memorablePijIdCandidates } from "../core/memorable-id.js";
import { PijDaemonClient } from "./daemon-http.js";
import { FakePiRuntime, FakeTmux } from "./fakes.js";
import { RustRuntimeSession } from "./rust-runtime.js";

// #44: `/v1/seats` omits retired seats, so a self-chosen name can be a retired
// seat's id. Registration refuses it with a typed code; only a self-chosen
// startup name may advance past that refusal.

const SESSION = "omp-session-44";
const CANDIDATES = [...memorablePijIdCandidates(memorableIdentitySeed("pi", SESSION))].slice(0, 12);
const [FIRST, SECOND, THIRD] = CANDIDATES as [string, string, string];

class NotifyingPi extends FakePiRuntime {
	readonly notices: Array<{ text: string; level?: string }> = [];
	notify(text: string, level?: "info" | "warning" | "error"): void {
		this.notices.push({ text, level });
	}
}

function ok(command: string, data: unknown): Response {
	return new Response(JSON.stringify({ ok: true, command, v: 2, data }));
}

function daemon(options: {
	readonly live?: readonly string[];
	/** Full live rows, for same-pane and same-process matches. */
	readonly rows?: readonly RustSeatDescriptor[];
	readonly retired?: ReadonlySet<string>;
	readonly typed?: boolean;
	/** The seat the typed refusal names, when it is not the claimed id. */
	readonly refusedSeat?: string;
}) {
	const claims: string[] = [];
	const registrations: Registration[] = [];
	const sends: Array<Record<string, unknown>> = [];
	const live: RustSeatDescriptor[] = [
		...(options.live ?? []).map((id) => ({
			id,
			harness: "omp",
			folder: "/abs/tree",
			state: "idle",
		})),
		...(options.rows ?? []),
	];
	const fetchMock = async (url: string | URL | Request, init?: RequestInit) => {
		const path = new URL(String(url)).pathname;
		if (path === "/v1/seats") return ok("pij seats", { seats: live, unavailable: [] });
		if (path === "/v1/register") {
			const claim = JSON.parse(String(init?.body)) as Registration;
			claims.push(claim.id);
			registrations.push(claim);
			if (options.retired?.has(claim.id)) {
				return new Response(
					JSON.stringify({
						v: 2,
						ok: false,
						command: "pij register",
						error: "refused",
						meta: `seat ${claim.id} is retired (plan 128 cleanup, 1); a different session may not take it; harness mismatch: row pi, claim omp`,
						...(options.typed === false
							? {}
							: { details: { code: "E-RS-SEAT-RETIRED", seat: options.refusedSeat ?? claim.id } }),
					}),
					{ status: 400 },
				);
			}
			return ok("pij register", {
				id: claim.id,
				harness: claim.harness,
				pane: claim.pane,
				proc: { pid: claim.pid, proc_start: claim.proc_start },
				folder: claim.folder,
				state: "idle",
				binding: "created",
			});
		}
		if (path === "/v1/events")
			return new Response(`${JSON.stringify({ hello: true, v: 1, build: "test" })}\n`);
		if (path === "/v1/inbox") return ok("pij inbox", []);
		if (path === "/v1/state") return ok("pij state", { pendingFyis: 0 });
		if (path === "/v1/activity") return ok("pij activity", { changed: false });
		if (path === "/v1/send") {
			const sent = JSON.parse(String(init?.body)) as Record<string, unknown>;
			sends.push(sent);
			return ok("pij send", { msg_id: sent.msg_id, outcome: "accepted", at: 1 });
		}
		throw new Error(`unexpected path ${path}`);
	};
	const client = new PijDaemonClient({ addr: "127.0.0.1:7461", stateDir: "/tmp/state" }, "key", {
		fetch: fetchMock as typeof fetch,
		readFile: async () => "key",
		processStart: () => PROC_START,
	});
	const runtime = new RustRuntimeSession(client, new FakeTmux(), []);
	const pi = new NotifyingPi();
	runtime.setPi(pi);
	const boot = (reason: "startup" | "reload" | "new" | "fork" = "startup") =>
		runtime.boot({
			extension_build: "0123456789",
			extension_path: "/abs/extension",
			folder: "/abs/tree",
			dataDir: "",
			eventsPath: "",
			harness: "pi",
			harnessSessionId: SESSION,
			piSessionId: SESSION,
			runtimeBin: "omp",
			paneId: "%4400",
			resetRuntimeState: reason !== "reload",
			reason,
		});
	return { runtime, pi, claims, registrations, sends, boot };
}

/** The daemon's process-start fixture, so a row can match this very process. */
const PROC_START = 20260916000000;

const runtimes: RustRuntimeSession[] = [];
beforeEach(() => {
	vi.stubEnv("PIJ_SESSION_ID", undefined);
	vi.stubEnv("PIJ_ANNOUNCE_TO", "");
	vi.stubEnv("PIJ_PARENT_ID", undefined);
	vi.stubEnv("PIJ_SPAWN_ID", "");
	vi.stubEnv("PIJ_SPAWN_TASK", undefined);
});
afterEach(() => {
	for (const runtime of runtimes.splice(0)) runtime.shutdown("quit");
	vi.unstubAllEnvs();
});

describe("self-chosen seat name vs retired seats (#44)", () => {
	it("registers as the next candidate when the first is a retired seat the roster omits", async () => {
		const fake = daemon({ retired: new Set([FIRST]) });
		runtimes.push(fake.runtime);
		const booted = await fake.boot();
		expect(booted.id).toBe(SECOND);
		expect(fake.claims).toEqual([FIRST, SECOND]);
	});

	it("still skips a live seat's name without a refusal", async () => {
		const fake = daemon({ live: [FIRST] });
		runtimes.push(fake.runtime);
		expect((await fake.boot()).id).toBe(SECOND);
		expect(fake.claims).toEqual([SECOND]);
	});

	it("skips live names and advances past retired ones in one boot", async () => {
		const fake = daemon({ live: [FIRST], retired: new Set([SECOND]) });
		runtimes.push(fake.runtime);
		expect((await fake.boot()).id).toBe(THIRD);
		expect(fake.claims).toEqual([SECOND, THIRD]);
	});

	it("advances on the typed code only: the same prose without it stays a refusal", async () => {
		const fake = daemon({ retired: new Set([FIRST]), typed: false });
		runtimes.push(fake.runtime);
		await expect(fake.boot()).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual([FIRST]);
	});

	it("never advances past an assigned PIJ_SESSION_ID", async () => {
		vi.stubEnv("PIJ_SESSION_ID", FIRST);
		const fake = daemon({ retired: new Set([FIRST]) });
		runtimes.push(fake.runtime);
		await expect(fake.boot()).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual([FIRST]);
	});

	it("gives up after a bounded number of claims", async () => {
		const fake = daemon({ retired: new Set(CANDIDATES) });
		runtimes.push(fake.runtime);
		await expect(fake.boot()).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual(CANDIDATES.slice(0, 8));
	});

	it("never advances when the typed refusal names a different seat", async () => {
		const fake = daemon({ retired: new Set([FIRST]), refusedSeat: SECOND });
		runtimes.push(fake.runtime);
		await expect(fake.boot()).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual([FIRST]);
	});
});

// R2: every assigned identity keeps a typed retired refusal terminal. Each case
// goes red if its branch is given a candidate sequence to advance through.
describe("assigned identities never advance (#44 R2)", () => {
	const OWNED = "pij-owned-row";
	it.each([
		[
			"the same pane",
			{ id: OWNED, harness: "omp", folder: "/abs/tree", state: "idle", pane: "%4400" },
		],
		[
			"the same process",
			{
				id: OWNED,
				harness: "pi",
				folder: "/abs/tree",
				state: "idle",
				proc: { pid: process.pid, proc_start: PROC_START },
			},
		],
	] as const)("keeps %s's seat refusal terminal", async (_label, row) => {
		const fake = daemon({ rows: [row], retired: new Set([OWNED]) });
		runtimes.push(fake.runtime);
		await expect(fake.boot()).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual([OWNED]);
	});

	it("keeps a reload's own id refusal terminal", async () => {
		const retired = new Set<string>();
		const fake = daemon({ retired });
		runtimes.push(fake.runtime);
		expect((await fake.boot()).id).toBe(FIRST);
		retired.add(FIRST);
		await expect(fake.boot("reload")).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual([FIRST, FIRST]);
	});

	it.each([
		"new",
		"fork",
	] as const)("keeps a /%s replacement's refusal terminal and its predecessor", async (reason) => {
		const retired = new Set<string>();
		const fake = daemon({ retired });
		runtimes.push(fake.runtime);
		expect((await fake.boot()).id).toBe(FIRST);
		retired.add(SECOND);
		await expect(fake.boot(reason)).rejects.toThrow(/is retired/);
		expect(fake.claims).toEqual([FIRST, SECOND]);
		expect(fake.registrations[1]?.supersedes).toBe(FIRST);
	});
});

describe("boot failure is loud (#44 R4)", () => {
	it("notifies at error level and tells the spawner, naming the spawn and pane", async () => {
		vi.stubEnv("PIJ_SESSION_ID", FIRST);
		vi.stubEnv("PIJ_ANNOUNCE_TO", "pij-parent");
		vi.stubEnv("PIJ_SPAWN_ID", "s44-0");
		vi.stubEnv("TMUX_PANE", "%4400");
		const fake = daemon({ retired: new Set([FIRST]) });
		runtimes.push(fake.runtime);
		const error = await fake.boot().catch((caught: unknown) => caught);
		await fake.runtime.reportBootFailure(error);
		expect(fake.pi.notices).toEqual([
			{ text: expect.stringContaining(`seat ${FIRST} is retired`), level: "error" },
		]);
		expect(fake.sends).toHaveLength(1);
		expect(fake.sends[0]).toMatchObject({
			to: { seat: "pij-parent" },
			body: expect.stringMatching(
				/^pij boot failed \(spawn s44-0, pane %4400\): seat .* is retired/,
			),
		});
	});

	it("only notifies when no spawner is known", async () => {
		const fake = daemon({});
		runtimes.push(fake.runtime);
		await fake.runtime.reportBootFailure(new Error("daemon refused"));
		expect(fake.pi.notices.map((notice) => notice.level)).toEqual(["error"]);
		expect(fake.sends).toEqual([]);
	});
});
