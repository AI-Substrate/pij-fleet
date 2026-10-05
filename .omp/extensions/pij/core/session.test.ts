// pij-messaging — PijSession coordinator specs (Pattern P8: target the pure
// coordinator vs fakes, never the pi wiring). Covers findings 01,04,07,08 and
// spec AC-2/3/4/5/6/13.

import { describe, expect, it } from "vitest";
import {
	FakeDelivery,
	FakeEventLog,
	FakePiRuntime,
	FakeProcess,
	FakeRegistry,
	FakeTmux,
} from "../adapters/fakes.js";
import { frame } from "./message.js";
import type { ModelEntry } from "./models/registry.js";
import type { SpawnExpectationStore } from "./ports.js";
import type { BootInput, PijPorts } from "./session.js";
import { PijSession } from "./session.js";
import { DEFAULT_SPAWN_EXPECTATION_TTL_MS } from "./spawn-expectation.js";
import { err, type PijEvent, type SessionDescriptor, type SpawnExpectation } from "./types.js";

const T0 = Date.parse("2026-06-16T00:00:00.000Z");

function bootInput(over: Partial<BootInput> = {}): BootInput {
	return {
		id: "alice",
		role: "worker",
		folder: "/repo",
		dataDir: "/home/.pij/alice",
		eventsPath: "/home/.pij/alice/events.ndjson",
		harness: "pi",
		harnessSessionId: "pi-native-alice",
		...over,
	};
}

function harness(
	opts: {
		idle?: boolean;
		registry?: readonly SessionDescriptor[];
		events?: readonly PijEvent[];
		now?: number;
		/** PIJ_* env vars forwarded to FakeProcess (for spawn-child boot tests). */
		vars?: Record<string, string>;
		/** Override default FakeTmux (e.g. to test E-NOTMUX with sessionName: null). */
		tmux?: FakeTmux;
		expectations?: SpawnExpectationStore;
		models?: readonly ModelEntry[];
	} = {},
) {
	const registry = new FakeRegistry(opts.registry ?? []);
	const eventLog = new FakeEventLog(opts.events ?? []);
	const delivery = new FakeDelivery();
	const pi = new FakePiRuntime(opts.idle ?? true);
	const process = new FakeProcess(4242, opts.now ?? T0, opts.vars ?? {});
	const tmux = opts.tmux ?? new FakeTmux();
	const ports: PijPorts = {
		registry,
		eventLog,
		delivery,
		pi,
		process,
		tmux,
		...(opts.models ? { models: opts.models } : {}),
		...(opts.expectations ? { expectations: opts.expectations } : {}),
	};
	return { ports, registry, eventLog, delivery, pi, process, tmux, session: new PijSession(ports) };
}

function memoryExpectationStore(): SpawnExpectationStore & { writes: SpawnExpectation[] } {
	const map = new Map<string, SpawnExpectation>();
	const writes: SpawnExpectation[] = [];
	return {
		writes,
		list: () => [...map.values()],
		read: (spawnId) => map.get(spawnId) ?? null,
		write: (expectation) => {
			writes.push(expectation);
			map.set(expectation.spawnId, expectation);
		},
		remove: (spawnId) => {
			map.delete(spawnId);
		},
	};
}

describe("PijSession.boot", () => {
	it("fresh boot writes a descriptor, announces once, returns fresh=true", () => {
		const h = harness();
		const r = h.session.boot(bootInput({ planId: "073-pij-first-class-ui" }));
		expect(r).toMatchObject({ id: "alice", role: "worker", fresh: true });
		const d = h.registry.read("alice");
		expect(d).toMatchObject({
			id: "alice",
			role: "worker",
			folder: "/repo",
			pid: 4242,
			startedAt: new Date(T0).toISOString(),
			planId: "073-pij-first-class-ui",
		});
		// announce injected immediately, exactly once, stamped with self id
		expect(h.pi.injects).toHaveLength(1);
		expect(h.pi.injects[0]).toMatchObject({ mode: "immediate" });
		expect(h.pi.injects[0]?.text).toContain("alice");
	});

	it("persists the selected Pi-family runtime on first boot", () => {
		const h = harness();
		h.session.boot(bootInput({ runtimeBin: "omp" }));
		expect(h.registry.read("alice")?.runtimeBin).toBe("omp");
	});

	it("reload reuses the descriptor: no re-announce, startedAt preserved, fresh=false", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
		};
		const h = harness({ registry: [existing], now: T0 });
		const r = h.session.boot(bootInput());
		expect(r.fresh).toBe(false);
		expect(h.pi.injects).toHaveLength(0); // no replay of the announce
		expect(h.registry.read("alice")?.startedAt).toBe("2026-06-15T00:00:00.000Z");
		expect(h.registry.read("alice")?.pid).toBe(4242); // pid refreshed
	});

	it("reload preserves an existing planId over a stale spawn environment", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			planId: "073-pij-first-class-ui",
		};
		const h = harness({ registry: [existing], now: T0 });
		h.session.boot(bootInput({ planId: "071-stale-spawn-env" }));
		expect(h.registry.read("alice")?.planId).toBe("073-pij-first-class-ui");
	});

	it("resume preserves durable fields while replacing stale runtime attachment", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/old-repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			lifecycle: "failed",
			failureReason: "dead",
			paneId: "%1",
			spawnedBy: "pij-parent",
			lastEventAt: "2026-06-15T01:00:00.000Z",
			boundModel: "model-before-restart",
			prime: true,
		};
		const h = harness({ registry: [existing], now: T0 });
		h.session.boot(bootInput({ folder: "/new-repo", paneId: "%9", resetRuntimeState: true }));
		expect(h.registry.read("alice")).toMatchObject({
			id: "alice",
			folder: "/new-repo",
			pid: 4242,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			lifecycle: undefined,
			failureReason: undefined,
			paneId: "%9",
			spawnedBy: "pij-parent",
			lastEventAt: "2026-06-15T01:00:00.000Z",
			boundModel: "model-before-restart",
			prime: true,
		});
	});

	it("revives a dissolved Pi-family descriptor and clears terminal incarnation state", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/old-repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			runtimeBin: "omp",
			lifecycle: "dissolved",
			paneId: "%old",
			closeIntent: {
				actor: "pij-parent",
				kind: "cli-close",
				requestedAt: "2026-06-15T01:00:00.000Z",
			},
			terminal: {
				disposition: "requested",
				observedAt: "2026-06-15T01:00:01.000Z",
				evidence: "pane-missing",
			},
		};
		const h = harness({ registry: [existing], now: T0 });
		const result = h.session.boot(
			bootInput({ paneId: "%new", resetRuntimeState: true, runtimeBin: "omp" }),
		);
		expect(result.fresh).toBe(true);
		expect(h.registry.read("alice")).toMatchObject({
			id: "alice",
			pid: 4242,
			paneId: "%new",
			runtimeBin: "omp",
		});
		expect(h.registry.read("alice")).not.toHaveProperty("closeIntent");
		expect(h.registry.read("alice")).not.toHaveProperty("terminal");
		expect(h.registry.read("alice")?.lifecycle).not.toBe("dissolved");
	});

	it("injects a Pi-family revive reframe without an announcement target", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			lifecycle: "dissolved",
			spawnedBy: "pij-dead-original-parent",
		};
		const h = harness({
			registry: [existing],
			vars: { PIJ_SPAWN_TASK: "REVIVED: wait for new instructions" },
		});
		h.session.boot(bootInput({ paneId: "%new", resetRuntimeState: true }));
		expect(h.pi.injects).toEqual([
			{ text: "REVIVED: wait for new instructions", mode: "immediate" },
		]);
		expect(h.registry.read("alice")?.spawnedBy).toBeUndefined();
	});

	it("hydrates durable metadata when the live descriptor was removed", () => {
		const durable: SessionDescriptor = {
			id: "alice",
			role: "parent",
			folder: "/old-repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			spawnedBy: "pij-creator",
			boundModel: "model-before-restart",
			prime: false,
			paneId: "%dead",
			lifecycle: "failed",
			failureReason: "dead",
		};
		const h = harness({ now: T0 });
		const result = h.session.boot(
			bootInput({
				role: undefined,
				paneId: "%new",
				durableDescriptor: durable,
				resetRuntimeState: true,
			}),
		);
		expect(result).toMatchObject({ fresh: true, role: "parent" });
		expect(h.registry.read("alice")).toMatchObject({
			id: "alice",
			role: "parent",
			startedAt: "2026-06-15T00:00:00.000Z",
			spawnedBy: "pij-creator",
			boundModel: "model-before-restart",
			prime: false,
			paneId: "%new",
			state: "idle",
			lifecycle: undefined,
			failureReason: undefined,
		});
	});

	it("refreshes structural and repository metadata while preserving close ownership", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			parentId: "pij-old-parent",
			gitCommonDir: "/old/.git",
			folder: "/old-repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			spawnedBy: "pij-close-owner",
		};
		const refreshed = harness({ registry: [existing], now: T0 });
		refreshed.session.boot(
			bootInput({
				parentId: null,
				gitCommonDir: "/new/.git",
				resetRuntimeState: true,
			}),
		);
		expect(refreshed.registry.read("alice")).toMatchObject({
			parentId: null,
			gitCommonDir: "/new/.git",
			spawnedBy: "pij-close-owner",
		});

		const preserved = harness({ registry: [existing], now: T0 });
		preserved.session.boot(bootInput({ resetRuntimeState: true }));
		expect(preserved.registry.read("alice")).toMatchObject({
			parentId: "pij-old-parent",
			gitCommonDir: "/old/.git",
			spawnedBy: "pij-close-owner",
		});
	});

	it("hydrates durable structural metadata and applies a freshly resolved repository identity", () => {
		const durable: SessionDescriptor = {
			id: "alice",
			parentId: null,
			gitCommonDir: "/old/.git",
			folder: "/old-repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			harness: "pi",
			harnessSessionId: "pi-native-alice",
			spawnedBy: "pij-close-owner",
		};
		const h = harness({ now: T0 });
		h.session.boot(
			bootInput({
				durableDescriptor: durable,
				gitCommonDir: "/new/.git",
				resetRuntimeState: true,
			}),
		);
		expect(h.registry.read("alice")).toMatchObject({
			parentId: null,
			gitCommonDir: "/new/.git",
			spawnedBy: "pij-close-owner",
		});
	});

	it("reseeds the seq counter from lastSeq() (crash-safe, finding 04)", () => {
		const events: PijEvent[] = [
			{ seq: 7, timestamp: new Date(T0).toISOString(), type: "tool_call" },
		];
		const h = harness({ events, now: T0 });
		h.session.boot(bootInput());
		h.session.capture("tool_result");
		expect(h.eventLog.read({ since: 7 })[0]?.seq).toBe(8);
	});
});

describe("PijSession.capture", () => {
	it("appends events with strictly monotonic seq + ISO timestamp", () => {
		const h = harness({ now: T0 });
		h.session.boot(bootInput());
		h.process.advance(1000);
		h.session.capture("tool_call", { name: "ctx_read" });
		h.session.capture("tool_result");
		const evs = h.eventLog.read();
		expect(evs.map((e) => e.seq)).toEqual([1, 2]);
		expect(evs[0]?.timestamp).toBe(new Date(T0 + 1000).toISOString());
		expect(evs[0]?.data).toEqual({ name: "ctx_read" });
	});
});

describe("PijSession.onInbound — free text", () => {
	it("keeps legacy delivery on its existing submission path unless native consumption tracking is enabled", () => {
		const h = harness({ idle: false, now: T0 });
		h.session.boot(bootInput());
		const identities: Array<string | undefined> = [];
		h.ports.pi.inject = (_text, _mode, nativeId) => {
			identities.push(nativeId);
		};
		h.session.onInbound({ from: "bob", to: "alice", body: "legacy" }, "legacy-id");
		expect(identities).toEqual([undefined]);
		h.session.onInbound({ from: "bob", to: "alice", body: "native" }, "native-id", true);
		expect(identities).toEqual([undefined, "native-id"]);
	});
	it("idle peer: immediate inject, framed sender id, single delivered receipt", () => {
		const h = harness({ idle: true, now: T0 });
		h.session.boot(bootInput());
		h.pi.injects.length = 0; // drop the announce
		const res = h.session.onInbound({ from: "bob", to: "alice", body: "hi" }, "m1");
		expect(res).toMatchObject({ kind: "delivered", state: "delivered" });
		expect(h.pi.injects[0]).toMatchObject({
			text: frame("bob", "hi"),
			mode: "immediate",
		});
		// receipt goes back to the sender as a kind:receipt message...
		expect(h.delivery.outbox).toHaveLength(1);
		expect(h.delivery.outbox[0]?.message).toMatchObject({
			from: "alice",
			to: "bob",
			kind: "receipt",
		});
		expect(h.delivery.outbox[0]?.message.body).toBe("[pij receipt m1] delivered");
		// ...and is recorded as an event (AC-13 visible in tail/state)
		expect(h.eventLog.read({ type: "receipt" })).toHaveLength(1);
	});

	it("busy peer: steer inject, queued receipt, then delivered at next turn_start", () => {
		const h = harness({ idle: false, now: T0 });
		h.session.boot(bootInput());
		h.pi.injects.length = 0;
		const res = h.session.onInbound({ from: "bob", to: "alice", body: "yo" }, "m2");
		expect(res).toMatchObject({ kind: "delivered", state: "queued" });
		expect(h.pi.injects[0]).toMatchObject({ mode: "steer" });
		expect(h.delivery.outbox[0]?.message.body).toBe("[pij receipt m2] queued");
		// a turn_start strictly after the inject resolves queued -> delivered
		const later = new Date(T0 + 5000).toISOString();
		h.session.onTurnStart(later);
		expect(h.delivery.outbox).toHaveLength(2);
		expect(h.delivery.outbox[1]?.message.body).toBe("[pij receipt m2] delivered");
		// a second turn_start does not re-deliver
		h.session.onTurnStart(new Date(T0 + 9000).toISOString());
		expect(h.delivery.outbox).toHaveLength(2);
	});

	// Plan 164 S7: a forwarded sender is shown machine-qualified everywhere it is named.
	it.each([
		{ fromMachine: "laptop", sender: "bob@laptop" },
		{ fromMachine: undefined, sender: "bob" },
	])("names the sender as $sender in the frame and the receipt event", ({
		fromMachine,
		sender,
	}) => {
		const h = harness({ idle: true, now: T0 });
		h.session.boot(bootInput());
		h.pi.injects.length = 0;
		h.session.onInbound(
			{
				from: "bob",
				...(fromMachine === undefined ? {} : { fromMachine }),
				to: "alice",
				body: "hi",
			},
			"m1",
		);
		expect(h.pi.injects[0]?.text).toBe(`[pij-rs from ${sender}]\nhi\n[/pij]`);
		expect(h.eventLog.read({ type: "receipt" })[0]?.data).toMatchObject({ to: sender });
	});
});

describe("PijSession.onInbound — commands (AC-6, finding 05)", () => {
	it("compact is executed and recorded; no inject", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.pi.injects.length = 0;
		const res = h.session.onInbound(
			{ from: "bob", to: "alice", body: "", command: "compact" },
			"c1",
		);
		expect(res).toMatchObject({ kind: "command-executed", command: "compact" });
		expect(h.pi.compactCount).toBe(1);
		expect(h.pi.injects).toHaveLength(0);
	});

	it("unknown command is rejected with E-CMD and never reaches pi", () => {
		const h = harness();
		h.session.boot(bootInput());
		const before = h.pi.compactCount;
		const res = h.session.onInbound({ from: "bob", to: "alice", body: "", command: "rm-rf" }, "c2");
		expect(res).toMatchObject({ kind: "command-rejected", code: "E-CMD" });
		expect(h.pi.compactCount).toBe(before);
	});

	it("new fires via the captured command context when armed; no compact, no inject", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.pi.injects.length = 0;
		const res = h.session.onInbound({ from: "bob", to: "alice", body: "", command: "new" }, "c3");
		expect(res).toMatchObject({ kind: "command-executed", command: "new" });
		expect(h.pi.controlCalls).toEqual(["new"]);
		expect(h.pi.compactCount).toBe(0);
		expect(h.pi.injects).toHaveLength(0);
	});

	it("reload is deferred when un-armed: queued + wakes, then drained on /pij", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.pi.setArmed(false);
		h.pi.injects.length = 0;
		const res = h.session.onInbound(
			{ from: "bob", to: "alice", body: "", command: "reload" },
			"c4",
		);
		expect(res).toMatchObject({ kind: "command-deferred", command: "reload" });
		expect(h.pi.controlCalls).toHaveLength(0);
		expect(h.pi.injects[0]?.text).toContain("/reload");
		expect(h.pi.injects[0]?.text).toContain("human operator");
		// a `/pij` invocation arms the channel and drains the queue exactly once
		h.pi.setArmed(true);
		expect(h.session.applyPendingControl()).toEqual(["reload"]);
		expect(h.pi.controlCalls).toEqual(["reload"]);
		expect(h.session.applyPendingControl()).toEqual([]);
	});

	it.each([
		{ fromMachine: "laptop", sender: "bob@laptop" },
		{ fromMachine: undefined, sender: "bob" },
	])("a deferred control names its requester as $sender", ({ fromMachine, sender }) => {
		const h = harness();
		h.session.boot(bootInput());
		h.pi.setArmed(false);
		h.pi.injects.length = 0;
		h.session.onInbound(
			{
				from: "bob",
				...(fromMachine === undefined ? {} : { fromMachine }),
				to: "alice",
				body: "",
				command: "reload",
			},
			"c5",
		);
		expect(h.pi.injects[0]?.text).toMatch(new RegExp(`^\\[pij\\] Peer ${sender} asked `));
	});
});

describe("PijSession.onInbound — receipts never wake the peer", () => {
	it("a kind:receipt message is recorded as an event, never injected", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.pi.injects.length = 0;
		const res = h.session.onInbound(
			{ from: "bob", to: "alice", body: "[pij receipt x] delivered", kind: "receipt" },
			"r1",
		);
		expect(res).toMatchObject({ kind: "receipt-recorded" });
		expect(h.pi.injects).toHaveLength(0); // the parent is NOT woken
		expect(h.delivery.outbox).toHaveLength(0); // no receipt-of-a-receipt
		expect(h.eventLog.read({ type: "receipt" })).toHaveLength(1);
	});
});

describe("PijSession descriptor state (D-A / AC-9, AC-7a)", () => {
	it("boots idle, goes working on turn_start, idle again on turn_end", () => {
		const h = harness({ now: T0 });
		h.session.boot(bootInput());
		expect(h.registry.read("alice")?.state).toBe("idle");
		h.session.onTurnStart(new Date(T0).toISOString());
		expect(h.registry.read("alice")?.state).toBe("working");
		h.session.onTurnEnd();
		expect(h.registry.read("alice")?.state).toBe("idle");
	});

	it("capture refreshes lastEventAt to the event's ISO timestamp", () => {
		const h = harness({ now: T0 });
		h.session.boot(bootInput());
		expect(h.registry.read("alice")?.lastEventAt).toBeUndefined();
		h.process.advance(2000);
		h.session.capture("tool_call");
		expect(h.registry.read("alice")?.lastEventAt).toBe(new Date(T0 + 2000).toISOString());
	});

	it("reload preserves state + lastEventAt", () => {
		const existing: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
			state: "working",
			lastEventAt: "2026-06-15T01:00:00.000Z",
		};
		const h = harness({ registry: [existing], now: T0 });
		h.session.boot(bootInput());
		expect(h.registry.read("alice")?.state).toBe("working");
		expect(h.registry.read("alice")?.lastEventAt).toBe("2026-06-15T01:00:00.000Z");
	});
});

describe("PijSession.shutdown", () => {
	it("dissolves a replacement descriptor without leaving it in the live registry list", () => {
		const h = harness();
		h.session.boot(
			bootInput({
				parentId: null,
				gitCommonDir: "/repo/.git",
			}),
		);
		expect(h.registry.read("alice")).not.toBeNull();
		h.session.shutdown("reload");
		expect(h.registry.read("alice")).toMatchObject({
			lifecycle: "dissolved",
			parentId: null,
			gitCommonDir: "/repo/.git",
		});
		expect(h.registry.list()).toEqual([]);
	});

	it("leaves quit available for daemon terminal observation", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.session.shutdown("quit");
		expect(h.registry.read("alice")?.lifecycle).not.toBe("dissolved");
	});
});

// ─── T202: PijSession.spawn ───────────────────────────────────────────────────

describe("PijSession.spawn", () => {
	it("layout:window opens exactly one window; returns spawnId + paneId (AC-01)", () => {
		const h = harness();
		h.session.boot(bootInput());
		const r = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "window" });
		expect(r.ok).toBe(true);
		if (!r.ok) return;
		expect(typeof r.value.spawnId).toBe("string");
		expect(r.value.paneId).toBe("%900"); // FakeTmux starts at 900
		expect(h.tmux.windows).toHaveLength(1);
		expect(h.tmux.splits).toHaveLength(0);
	});

	it("persists the exact deadline before launch, then correlates pane and child registration", () => {
		const trace: string[] = [];
		const expectations = memoryExpectationStore();
		const originalWrite = expectations.write;
		expectations.write = (expectation) => {
			trace.push(expectation.paneId === undefined ? "expectation-write" : "pane-update");
			originalWrite(expectation);
		};
		const parent = harness({ expectations });
		const originalNewWindow = parent.tmux.newWindow.bind(parent.tmux);
		parent.tmux.newWindow = (opts) => {
			trace.push("tmux-launch");
			return originalNewWindow(opts);
		};
		parent.session.boot(bootInput());
		const spawned = parent.session.spawn({ harness: "pi", cwd: "/repo", layout: "window" });
		expect(spawned.ok).toBe(true);
		if (!spawned.ok) return;
		expect(expectations.writes[0]).toMatchObject({
			spawnId: spawned.value.spawnId,
			requestedHarness: "pi",
			requestedAt: new Date(T0).toISOString(),
			deadlineAt: new Date(T0 + DEFAULT_SPAWN_EXPECTATION_TTL_MS).toISOString(),
		});
		expect(expectations.writes[0]).not.toHaveProperty("paneId");
		expect(expectations.writes[1]).toMatchObject({ paneId: spawned.value.paneId });
		expect(trace).toEqual(["expectation-write", "tmux-launch", "pane-update"]);

		const child = harness({
			expectations,
			vars: {
				PIJ_ANNOUNCE_TO: "alice",
				PIJ_SPAWN_ID: spawned.value.spawnId,
				TMUX_PANE: spawned.value.paneId,
			},
		});
		child.session.boot(bootInput({ id: "bob", harnessSessionId: "pi-native-bob" }));
		expect(expectations.read(spawned.value.spawnId)).toMatchObject({
			sessionId: "bob",
			runtimeHarness: "pi",
			paneId: spawned.value.paneId,
		});
	});

	it("env carries PIJ_ANNOUNCE_TO=self, PIJ_SPAWN_ID, PIJ_ROLE=worker (AC-02)", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.session.spawn({ harness: "pi", cwd: "/repo", layout: "window" });
		const w = h.tmux.windows[0];
		expect(w?.opts.env.PIJ_ANNOUNCE_TO).toBe("alice");
		expect(w?.opts.env.PIJ_ROLE).toBe("worker");
		expect(typeof w?.opts.env.PIJ_SPAWN_ID).toBe("string");
		expect(w?.opts.env.PIJ_SPAWN_ID).toBeTruthy();
	});

	it("threads model via --model argv AND PIJ_SPAWN_MODEL env (§H2 / F003)", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.session.spawn({
			harness: "pi",
			cwd: "/repo",
			model: "test-model",
			effort: "xhigh",
			layout: "window",
		});
		const w = h.tmux.windows[0];
		// F003: PIJ_SPAWN_MODEL is now emitted by buildSpawnCommand (not post-processed)
		expect(w?.opts.args).toContain("--model");
		expect(w?.opts.args).toContain("test-model:xhigh");
		expect(w?.opts.env.PIJ_SPAWN_MODEL).toBe("test-model:xhigh");
		expect(w?.opts.env.PIJ_SPAWN_EFFORT).toBe("xhigh");
		// No post-processing remnant: env is spawnCmd.env directly
		expect(Object.keys(w?.opts.env ?? {}).filter((k) => k === "PIJ_SPAWN_MODEL")).toHaveLength(1);
	});

	it("model-facing spawn uses the same deterministic provider resolver", () => {
		const model = (provider: string): ModelEntry => ({
			id: "gpt-5.6-sol",
			name: "GPT-5.6 Sol",
			provider,
			verified: true,
		});
		const h = harness({
			models: [model("github-copilot"), model("copilot"), model("codex")],
		});
		h.session.boot(bootInput());
		const result = h.session.spawn({
			harness: "pi",
			cwd: "/repo",
			model: "gpt-5.6-sol",
			layout: "window",
		});
		expect(result).toMatchObject({ ok: true });
		if (!result.ok) return;
		expect(result.value.notice).toContain("defaulting");
		const window = h.tmux.windows[0];
		expect(window?.opts.args).toContain("github-copilot/gpt-5.6-sol");
		expect(window?.opts.env.PIJ_SPAWN_PROVIDER).toBe("github-copilot");
	});

	it("does NOT set PIJ_PANE_ID in child env (§H1: child reads $TMUX_PANE)", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.session.spawn({ harness: "pi", cwd: "/repo", layout: "window" });
		const w = h.tmux.windows[0];
		expect(w?.opts.env.PIJ_PANE_ID).toBeUndefined();
	});

	it("passes task via PIJ_SPAWN_TASK env (finding 01 / CF-01)", () => {
		const h = harness();
		h.session.boot(bootInput());
		h.session.spawn({ harness: "pi", cwd: "/repo", task: "do the thing", layout: "window" });
		const w = h.tmux.windows[0];
		expect(w?.opts.env.PIJ_SPAWN_TASK).toBe("do the thing");
	});

	it("returns E-NOTMUX when not inside a tmux session (§M5 / F004 / AC-07)", () => {
		const h = harness({ tmux: new FakeTmux({ sessionName: null }) });
		h.session.boot(bootInput());
		const r = h.session.spawn({ harness: "pi", cwd: "/repo" });
		expect(r.ok).toBe(false);
		if (r.ok) return;
		expect(r.code).toBe("E-NOTMUX");
		expect(h.tmux.windows).toHaveLength(0); // no window opened
	});

	it("synchronous tmux launch failure removes only its owned expectation", () => {
		const expectations = memoryExpectationStore();
		expectations.write({
			spawnId: "sentinel",
			requestedHarness: "pi",
			requestedAt: new Date(T0).toISOString(),
		});
		const tmux = new FakeTmux();
		tmux.newWindow = () => err("E-NOTMUX", "injected launch failure");
		const h = harness({ expectations, tmux });
		h.session.boot(bootInput());
		const result = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "window" });
		expect(result).toMatchObject({ ok: false, code: "E-NOTMUX" });
		expect(expectations.list().map((item) => item.spawnId)).toEqual(["sentinel"]);
	});

	// ─── split-pane layout ───────────────────────────────────────────────────
	const mkKid = (id: string, paneId: string): SessionDescriptor => ({
		id,
		role: "worker",
		folder: "/repo",
		dataDir: `/home/.pij/${id}`,
		eventsPath: `/home/.pij/${id}/events.ndjson`,
		pid: 9999,
		startedAt: new Date(T0).toISOString(),
		paneId,
		spawnedBy: "alice",
	});

	it("DEFAULT (no layout) → split -h (~1/3 right column) on the current pane, no window", () => {
		const h = harness();
		h.session.boot(bootInput());
		const r = h.session.spawn({ harness: "pi", cwd: "/repo" });
		expect(r.ok).toBe(true);
		expect(h.tmux.windows).toHaveLength(0); // stack is the default, not a window
		expect(h.tmux.splits).toHaveLength(1);
		const s = h.tmux.splits[0];
		expect(s?.opts.direction).toBe("h");
		expect(s?.opts.target).toBe("%500"); // FakeTmux default current pane
		expect(s?.opts.percent).toBe(33);
		expect(s?.opts.evenOut).toBe(false); // first pane IS the column — nothing to even
		expect(s?.opts.detached).toBe(true);
	});

	it("layout:split with 1 worker in-window → split -v (stack) on that pane + even out", () => {
		const h = harness({
			registry: [mkKid("kid1", "%901")],
			tmux: new FakeTmux({ currentPane: "%500", windowPanes: ["%901"] }),
		});
		h.session.boot(bootInput());
		const r = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "split" });
		expect(r.ok).toBe(true);
		const s = h.tmux.splits[0];
		expect(s?.opts.direction).toBe("v");
		expect(s?.opts.target).toBe("%901");
		expect(s?.opts.evenOut).toBe(true);
		expect(s?.opts.columnPercent).toBe(33);
	});

	it("a 3rd worker appends below the NEWEST pane (uncapped stack)", () => {
		const h = harness({
			registry: [mkKid("k1", "%901"), mkKid("k2", "%902")],
			tmux: new FakeTmux({ currentPane: "%500", windowPanes: ["%901", "%902"] }),
		});
		h.session.boot(bootInput());
		const r = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "split" });
		expect(r.ok).toBe(true);
		const s = h.tmux.splits[0];
		expect(s?.opts.direction).toBe("v");
		expect(s?.opts.target).toBe("%902"); // newest kid, bottom of the stack
		expect(s?.opts.evenOut).toBe(true);
	});

	it("layout:split counts only CURRENT-window pij panes (window-mode kids ignored)", () => {
		const h = harness({
			registry: [mkKid("winkid", "%950")], // pane lives in another window
			tmux: new FakeTmux({ currentPane: "%500", windowPanes: [] }),
		});
		h.session.boot(bootInput());
		const r = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "split" });
		expect(r.ok).toBe(true);
		expect(h.tmux.splits[0]?.opts.direction).toBe("h"); // still the first column
	});

	it("layout:split tracks panes parent-side across back-to-back spawns (registry lags boot)", () => {
		// Fire-and-forget: children write their descriptors only on boot, so the
		// registry is empty here. The parent must still stack #2 on #1 and #3 on #2.
		const h = harness();
		h.session.boot(bootInput());
		const r1 = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "split" });
		const r2 = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "split" });
		expect(r1.ok && r2.ok).toBe(true);
		if (!r1.ok || !r2.ok) return;
		expect(h.tmux.splits[0]?.opts.direction).toBe("h"); // #1 = right column
		expect(h.tmux.splits[1]?.opts.direction).toBe("v"); // #2 = stacked on #1
		expect(h.tmux.splits[1]?.opts.target).toBe(r1.value.paneId);
		const r3 = h.session.spawn({ harness: "pi", cwd: "/repo", layout: "split" });
		expect(r3.ok).toBe(true); // uncapped — #3 stacks below #2
		if (!r3.ok) return;
		expect(h.tmux.splits[2]?.opts.direction).toBe("v");
		expect(h.tmux.splits[2]?.opts.target).toBe(r2.value.paneId);
		expect(h.tmux.splits[2]?.opts.evenOut).toBe(true);
	});
});

// ─── T203: PijSession.close ───────────────────────────────────────────────────

describe("PijSession.close", () => {
	const spawnedDescriptor: SessionDescriptor = {
		id: "bob",
		role: "worker",
		folder: "/repo",
		dataDir: "/home/.pij/bob",
		eventsPath: "/home/.pij/bob/events.ndjson",
		pid: 9999,
		startedAt: new Date(T0).toISOString(),
		paneId: "%901",
		spawnedBy: "alice",
	};

	it("orders intent write → kill → requested terminal write → dissolve exactly", () => {
		const h = harness({ registry: [spawnedDescriptor] });
		const trace: string[] = [];
		const originalWrite = h.registry.write.bind(h.registry);
		// Forwards `writer` — a spy that drops it silently disarms the write law for
		// everything under test (plan 071 review §1.2), and the close path's whole
		// point is that it OWNS terminal truth.
		h.registry.write = (descriptor, writer) => {
			if (descriptor.id === "bob") {
				trace.push(descriptor.terminal ? "terminal-write" : "intent-write");
			}
			originalWrite(descriptor, writer);
		};
		const originalKill = h.tmux.killPane.bind(h.tmux);
		h.tmux.killPane = (paneId) => {
			trace.push("kill");
			return originalKill(paneId);
		};
		const originalDissolve = h.registry.dissolve.bind(h.registry);
		h.registry.dissolve = (id) => {
			trace.push("dissolve");
			originalDissolve(id);
		};
		h.session.boot(bootInput());
		trace.length = 0;
		const r = h.session.close("bob");
		expect(r.ok).toBe(true);
		expect(trace).toEqual(["intent-write", "kill", "terminal-write", "dissolve"]);
		expect(h.registry.read("bob")).toMatchObject({
			lifecycle: "dissolved",
			terminal: { disposition: "requested", evidence: "pane-missing" },
		});
		expect(h.registry.list().map((d) => d.id)).not.toContain("bob");
	});

	it("a failed kill stops after intent without terminalizing or dissolving", () => {
		const h = harness({ registry: [spawnedDescriptor] });
		h.tmux.killPane = () => err("E-NOTMUX", "injected kill failure");
		h.session.boot(bootInput());
		const result = h.session.close("bob");
		expect(result).toMatchObject({ ok: false, code: "E-NOTMUX" });
		expect(h.registry.read("bob")).toMatchObject({
			closeIntent: { kind: "in-process-close" },
		});
		expect(h.registry.read("bob")?.terminal).toBeUndefined();
		expect(h.registry.read("bob")?.lifecycle).not.toBe("dissolved");
	});

	it("is idempotent after dissolve — a second close is a no-op", () => {
		const h = harness({ registry: [spawnedDescriptor] });
		h.session.boot(bootInput());
		expect(h.session.close("bob").ok).toBe(true);
		expect(h.session.close("bob")).toEqual({ ok: true, value: {} });
		expect(h.tmux.killedPanes).toEqual(["%901"]);
		expect(h.registry.read("bob")?.lifecycle).toBe("dissolved");
	});

	it("missing session → E-NOID, no killPane call", () => {
		const h = harness();
		h.session.boot(bootInput());
		const r = h.session.close("nonexistent");
		expect(r.ok).toBe(false);
		if (r.ok) return;
		expect(r.code).toBe("E-NOID");
		expect(h.tmux.killedPanes).toHaveLength(0);
	});

	it("no paneId (not a spawned window) → E-NOID, no killPane call (§H3)", () => {
		const noPaneId: SessionDescriptor = { ...spawnedDescriptor, paneId: undefined };
		const h = harness({ registry: [noPaneId] });
		h.session.boot(bootInput());
		const r = h.session.close("bob");
		expect(r.ok).toBe(false);
		if (r.ok) return;
		expect(r.code).toBe("E-NOID");
		expect(h.tmux.killedPanes).toHaveLength(0);
	});

	it("warn-if-not-mine: captures internal warn event AND returns caller-visible warning (AC-06 / FT-002)", () => {
		const notMine: SessionDescriptor = { ...spawnedDescriptor, spawnedBy: "charlie" };
		const h = harness({ registry: [notMine] });
		h.session.boot(bootInput());
		const r = h.session.close("bob");
		expect(r.ok).toBe(true);
		expect(h.tmux.killedPanes).toEqual(["%901"]);
		expect(h.registry.read("bob")?.lifecycle).toBe("dissolved");
		// Internal event captured
		const warnEvents = h.eventLog.read({ type: "receipt" });
		expect(
			warnEvents.some(
				(e) => (e.data as { kind?: string } | undefined)?.kind === "warn-close-not-mine",
			),
		).toBe(true);
		// Caller-visible warning string (AC-06)
		if (!r.ok) return;
		expect(typeof r.value.warning).toBe("string");
		expect(r.value.warning).toContain("charlie");
		expect(r.value.warning).toContain("alice"); // closedBy = self
	});

	it("warn when spawnedBy is absent but paneId exists (unknown origin — AC-06)", () => {
		const unknownOrigin: SessionDescriptor = { ...spawnedDescriptor, spawnedBy: undefined };
		const h = harness({ registry: [unknownOrigin] });
		h.session.boot(bootInput());
		const r = h.session.close("bob");
		expect(r.ok).toBe(true);
		if (!r.ok) return;
		expect(r.value.warning).toContain("unknown");
	});

	it("no warning when this session spawned the target (spawnedBy === self)", () => {
		const mine: SessionDescriptor = { ...spawnedDescriptor, spawnedBy: "alice" };
		const h = harness({ registry: [mine] });
		h.session.boot(bootInput());
		const r = h.session.close("bob");
		expect(r.ok).toBe(true);
		if (!r.ok) return;
		expect(r.value.warning).toBeUndefined();
	});
});

// ─── T204: boot() spawned-child path ─────────────────────────────────────────

describe("PijSession.boot — spawned child path (T204)", () => {
	it("fresh spawned boot: persists paneId from TMUX_PANE + spawnedBy; delivers ready-ping; injects announce (AC-03)", () => {
		const h = harness({
			vars: {
				PIJ_ANNOUNCE_TO: "parent",
				PIJ_SPAWN_ID: "s-test-001",
				TMUX_PANE: "%42",
			},
		});
		h.session.boot(bootInput());
		// P9: descriptor has paneId + spawnedBy
		const d = h.registry.read("alice");
		expect(d?.paneId).toBe("%42");
		expect(d?.spawnedBy).toBe("parent");
		// Ready-ping delivered (not injected) — to the parent
		expect(h.delivery.outbox).toHaveLength(1);
		const ping = h.delivery.outbox[0];
		expect(ping?.message.to).toBe("parent");
		expect(ping?.message.from).toBe("alice");
		const body = JSON.parse(ping?.message.body ?? "") as { spawnId: string; cwd: string };
		expect(body.spawnId).toBe("s-test-001");
		expect(body.cwd).toBe("/repo"); // BootInput.folder
		// Exactly ONE inject: announceText (no task present)
		expect(h.pi.injects).toHaveLength(1);
		expect(h.pi.injects[0]?.text).toContain("alice");
	});

	it("spawned boot with task: suppresses announceText, injects task only (finding 07 / AC-03)", () => {
		const h = harness({
			vars: {
				PIJ_ANNOUNCE_TO: "parent",
				PIJ_SPAWN_ID: "s-test-002",
				PIJ_SPAWN_TASK: "go do the thing",
			},
		});
		h.session.boot(bootInput());
		// Exactly ONE inject: the task (not announceText)
		expect(h.pi.injects).toHaveLength(1);
		expect(h.pi.injects[0]?.text).toBe("go do the thing");
		// Ready-ping still delivered
		expect(h.delivery.outbox).toHaveLength(1);
	});

	it("model + effort become registry truth and thread into the ready-ping body (§H2)", () => {
		const h = harness({
			vars: {
				PIJ_ANNOUNCE_TO: "parent",
				PIJ_SPAWN_ID: "s-test-003",
				PIJ_SPAWN_MODEL: "github-copilot/gpt-5.6-sol:xhigh",
				PIJ_SPAWN_PROVIDER: "github-copilot",
				PIJ_SPAWN_EFFORT: "xhigh",
			},
		});
		h.session.boot(bootInput());
		expect(h.delivery.outbox).toHaveLength(1);
		const body = JSON.parse(h.delivery.outbox[0]?.message.body ?? "") as {
			model: string;
			effort: string;
		};
		expect(body).toMatchObject({ model: "github-copilot/gpt-5.6-sol", effort: "xhigh" });
		expect(h.registry.read("alice")).toMatchObject({
			boundModel: "github-copilot/gpt-5.6-sol",
			effort: "xhigh",
			boundProvider: "github-copilot",
		});
	});

	it("reload (not fresh) does NOT re-ping (finding 04 / AC-04)", () => {
		const existingDescriptor: SessionDescriptor = {
			id: "alice",
			role: "worker",
			folder: "/repo",
			dataDir: "/home/.pij/alice",
			eventsPath: "/home/.pij/alice/events.ndjson",
			pid: 1,
			startedAt: "2026-06-15T00:00:00.000Z",
		};
		const h = harness({
			registry: [existingDescriptor],
			vars: { PIJ_ANNOUNCE_TO: "parent", PIJ_SPAWN_ID: "s-test-004" },
		});
		// boot() sees existing descriptor → fresh=false → no ready-ping, no inject
		h.session.boot(bootInput());
		expect(h.delivery.outbox).toHaveLength(0); // no ping on reload
		expect(h.pi.injects).toHaveLength(0); // no announce on reload
	});
});

describe("PijSession.noteInboxScan (plan 057 thread-1 — poll-primary liveness heartbeat)", () => {
	it("persists lastInboxScanAt only at the ~2500ms cadence (per-scan accurate, persisted coarse)", () => {
		const h = harness();
		h.session.boot(bootInput());
		// first scan stamps immediately (bootstraps the heartbeat at boot)
		h.session.noteInboxScan(T0);
		expect(h.registry.read("alice")?.lastInboxScanAt).toBe(new Date(T0).toISOString());
		// a scan within the cadence does NOT persist — the stamp is unchanged
		h.session.noteInboxScan(T0 + 1_000);
		expect(h.registry.read("alice")?.lastInboxScanAt).toBe(new Date(T0).toISOString());
		// a scan past the cadence persists the newer stamp
		h.session.noteInboxScan(T0 + 3_000);
		expect(h.registry.read("alice")?.lastInboxScanAt).toBe(new Date(T0 + 3_000).toISOString());
	});

	it("is a no-op before boot — the marker never advances, so the first post-boot scan still stamps", () => {
		const h = harness();
		h.session.noteInboxScan(T0); // pre-boot: no descriptor
		expect(h.registry.read("alice")).toBeNull();
		h.session.boot(bootInput());
		// only 100ms after the pre-boot call: without the guard the marker would
		// have advanced to T0 and this within-cadence scan would skip — it must NOT.
		h.session.noteInboxScan(T0 + 100);
		expect(h.registry.read("alice")?.lastInboxScanAt).toBe(new Date(T0 + 100).toISOString());
	});
});
