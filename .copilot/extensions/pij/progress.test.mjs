import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { registerHooks } from "node:module";
import test from "node:test";
import { setImmediate as flush } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import * as store from "./store.mjs";

// Fallbacks keep RED probes executable against the frozen, pre-feature module.
const DEADLINE = store.NATIVE_RPC_DEADLINE_MS ?? 15000;
const EMPTY_LIMIT = store.RECEIVER_EMPTY_READ_LIMIT ?? 3;
const INTERVAL = store.RECEIVER_PROBE_INTERVAL_MS ?? 1000;
const registration = {
	id: "pij-progress",
	harness: "copilot",
	harness_session: "session-progress",
	folder: "/work",
	pid: 152,
	proc_start: 20260916120000,
	native_extension_delivery: true,
};
const claim = {
	job_id: 152,
	message: {
		msg_id: "message-progress",
		from: "pij-peer",
		to: registration.id,
		body: "PRIVATE_BODY",
	},
	native_consumer: {
		native_session: registration.harness_session,
		pid: registration.pid,
		proc_start: registration.proc_start,
	},
};
const user = {
	type: "user.message",
	id: "owned-user",
	data: { messageId: "native-progress", interactionId: "owned-turn" },
};
const terminal = [
	{
		type: "assistant.message",
		id: "owned-final",
		parentId: user.id,
		data: { toolRequests: [], content: "PRIVATE_RESPONSE" },
	},
	{ type: "assistant.turn_end", id: "owned-end", parentId: "owned-final", data: {} },
];
function deferred() {
	let resolve;
	const promise = new Promise((done) => {
		resolve = done;
	});
	return { promise, resolve };
}
function gate(waits, ms, signal) {
	const tick = deferred();
	const abort = () => tick.resolve();
	signal.addEventListener("abort", abort, { once: true });
	waits.push({ ...tick, ms });
	return tick.promise
		.then(() => signal.throwIfAborted())
		.finally(() => signal.removeEventListener("abort", abort));
}
function fixture(t, { consume = true, claims = true, boundary = false } = {}) {
	const reports = [],
		calls = [],
		acks = [],
		sends = [],
		heartbeats = [],
		reads = [];
	const waits = [],
		heartbeatWaits = [],
		timers = new Map(),
		records = new Map();
	const state = {
		now: 100000,
		generation: "before-model-picker",
		history: [],
		lease: { state: "live", lease_ms: 60000, renew_after_ms: 20000 },
		claims: 0,
		// Real SDK tail cursors count ephemeral events; includeEphemeral:false reads do not.
		ephemeral: 0,
	};
	let observer;
	const cursor = (index) => `opaque-forward:${state.generation}:${index}`;
	const cursors = new Map();
	const forward = (index) => {
		const value = cursor(index);
		cursors.set(value, { generation: state.generation, index });
		return value;
	};
	const native = {
		on(fn) {
			observer = fn;
			return () => {};
		},
		async send(input) {
			calls.push("native.send");
			sends.push(input);
			if (consume) state.history.push(user);
			if (boundary) observer({ type: "session.idle", id: "foreground-boundary", data: {} });
			return "native-progress";
		},
		rpc: {
			eventLog: {
				async tail() {
					calls.push("eventLog.tail");
					const durable = forward(state.history.length);
					if (!state.ephemeral) return { cursor: durable };
					const value = `${durable}+ephemeral:${state.ephemeral}`;
					cursors.set(value, cursors.get(durable));
					return { cursor: value };
				},
				async read(input) {
					calls.push("eventLog.read");
					reads.push(input);
					if (input.direction === "backward" && input.max === 1)
						return {
							events: state.history.slice(-1),
							cursor: "opaque-backward:anchor",
							hasMore: state.history.length > 1,
							cursorStatus: "ok",
						};
					assert.equal(input.max, 128);
					assert.equal(input.includeEphemeral, false);
					if (input.direction === "backward")
						return {
							events: state.window ?? state.history.slice(-128),
							cursor: "opaque-backward:older-only",
							hasMore: state.history.length > 128,
							cursorStatus: state.windowStatus ?? "ok",
						};
					assert.notEqual(
						input.cursor,
						"opaque-backward:older-only",
						"backward cursor cannot be used as forward continuation",
					);
					const point =
						input.cursor === undefined
							? { generation: state.generation, index: 0 }
							: cursors.get(input.cursor);
					assert.ok(point, "forward cursor was issued by the SDK");
					if (point.generation !== state.generation)
						return { events: [], cursor: input.cursor, hasMore: false, cursorStatus: "ok" };
					const events = state.history.slice(point.index, point.index + input.max);
					return {
						events,
						cursor: forward(point.index + events.length),
						hasMore: point.index + events.length < state.history.length,
						cursorStatus: "ok",
					};
				},
			},
			queue: {
				async pendingItems() {
					calls.push("queue.pendingItems");
					return { items: [], steeringMessages: [], inFlightSteeringCount: 0 };
				},
			},
			metadata: {
				async isProcessing() {
					calls.push("metadata.isProcessing");
					return { processing: false };
				},
			},
		},
	};
	const journal = {
		async load(id) {
			return records.get(id);
		},
		async begin(message) {
			if (records.has(message.msg_id)) return false;
			records.set(message.msg_id, { state: "pending", message });
			return true;
		},
		async accept(message, nativeId) {
			records.set(message.msg_id, { state: "accepted", message, nativeId });
		},
		async rearm() {
			calls.push("journal.rearm");
			return true;
		},
	};
	const client = {
		async request(path, body) {
			if (path === "/v1/register")
				return {
					id: registration.id,
					harness: "copilot",
					session: registration.harness_session,
					folder: registration.folder,
					proc: { pid: registration.pid, proc_start: registration.proc_start },
					native_extension_delivery: true,
				};
			if (path === "/v1/inbox/heartbeat") {
				heartbeats.push(body);
				return state.lease;
			}
			if (path === "/v1/inbox/ack") {
				acks.push(body);
				return body.job_id;
			}
			if (path === "/v1/send") return { msg_id: body.msg_id };
			assert.fail(`Unexpected endpoint ${path}`);
		},
		async claimInbox(_tuple, signal) {
			if (state.claims++ === 0 && claims) return { claims: [claim], hold: null };
			await gate([], 0, signal);
			return { claims: [], hold: null };
		},
		async nativeSnapshot() {
			return { native_consumer: claim.native_consumer };
		},
	};
	const bridge = new store.NativeBridge({
		registration,
		native,
		journal,
		client,
		report: (event) => reports.push(event),
		delay: (ms, signal) => gate(waits, ms, signal),
		heartbeatDelay: (ms, signal) => gate(heartbeatWaits, ms, signal),
		now: () => state.now,
		setTimer: (fn, ms) => {
			const id = {};
			timers.set(id, { fn, at: state.now + ms });
			return id;
		},
		clearTimer: (id) => timers.delete(id),
	});
	t.after(() => bridge.stop("test-cleanup"));
	return {
		bridge,
		native,
		journal,
		client,
		state,
		reports,
		calls,
		acks,
		sends,
		heartbeats,
		reads,
		waits,
		heartbeatWaits,
		timers,
		records,
		event(event) {
			observer(event);
		},
		async advance(ms) {
			state.now += ms;
			for (const [id, timer] of timers)
				if (timer.at <= state.now) {
					timers.delete(id);
					timer.fn();
				}
			await flush();
		},
		async tick(bucket = waits) {
			state.now += INTERVAL;
			bucket.shift()?.resolve();
			await flush();
		},
		rebase(events) {
			state.generation = "after-model-picker";
			state.history = events;
		},
	};
}
async function pump(f, bucket = f.waits) {
	for (let n = 0; n < EMPTY_LIMIT + 2; n++) await f.tick(bucket);
}
const held = (f) => f.reports.filter((event) => event.kind === "receive-held");

const storeSource = `
		export * from ${JSON.stringify(new URL("./store.mjs", import.meta.url).href)};
		const registration = ${JSON.stringify(registration)};
		export const chooseRegistration=()=>registration;
		export const resolveRegistration=async()=>registration;
		export const resolveNativeHost=async()=>({});
		export class FileJournal {
			async load() { return this.record; }
			async begin(message) { this.record={state:'pending',message}; return true; }
			async accept(message,nativeId) { this.record={state:'accepted',message,nativeId}; }
		}
		export class DaemonClient {
			async request(path,body) {
				if(path.startsWith('/v1/seats')) return {seats:[]};
				if(path==='/v1/register') return {id:registration.id,harness:'copilot',session:registration.harness_session,folder:registration.folder,proc:{pid:registration.pid,proc_start:registration.proc_start},native_extension_delivery:true};
				if(path==='/v1/inbox/heartbeat') return {state:'live',lease_ms:60000,renew_after_ms:20000};
				if(path==='/v1/inbox/ack') return body.job_id;
				throw new Error('Unexpected '+path);
			}
			async nativeSnapshot() { return {native_consumer:${JSON.stringify(claim.native_consumer)}}; }
			async claimInbox(_tuple,signal) {
				if(!this.claimed) {this.claimed=true;return {claims:[${JSON.stringify(claim)}],hold:null};}
				await new Promise((resolve)=>signal.addEventListener('abort',resolve,{once:true}));
				return {claims:[],hold:null};
			}
		}
	`;
if (process.env.PIJ_PROGRESS_EXIT_CHILD === "1") {
	// Only OS/SDK/daemon seams are replaced. The extension lifecycle and bridge are real.
	const sdkSource = `
		let listener; const typed = new Map();
		export async function joinSession() {
			process.stdin.on('data', (bytes) => {
				if (String(bytes).trim() === 'exit') process.exit(0);
				else { typed.get('session.shutdown')?.(); listener?.({type:'session.shutdown',data:{}}); }
			});
			return {
				sessionId: process.env.SESSION_ID,
				on(type, fn) { if (typeof type === 'function') listener=type; else typed.set(type,fn); return () => {}; },
				async send() { listener({type:'user.message',id:'owned-user',data:{messageId:'native-progress'}}); return 'native-progress'; },
				rpc: { eventLog: { async tail() { return {cursor:'empty'}; }, async read() { return {events:[],cursor:'empty',hasMore:false,cursorStatus:'ok'}; } } },
				async disconnect() {}, async log() {}
			};
		}
	`;
	registerHooks({
		resolve(specifier, context, next) {
			const source =
				specifier === "@github/copilot-sdk/extension"
					? sdkSource
					: specifier === "./store.mjs" && context.parentURL?.endsWith("/extension.mjs")
						? storeSource
						: undefined;
			return source === undefined
				? next(specifier, context)
				: { url: `data:text/javascript,${encodeURIComponent(source)}`, shortCircuit: true };
		},
	});
} else {
	test("shutdown fixture exports every real store symbol", () => {
		const child = spawnSync(
			process.execPath,
			[
				"--input-type=module",
				"--eval",
				`import * as wrapped from ${JSON.stringify(`data:text/javascript,${encodeURIComponent(storeSource)}`)}; console.log(JSON.stringify(Object.keys(wrapped)));`,
			],
			{ encoding: "utf8", timeout: 5000 },
		);
		const exports = JSON.parse(child.stdout);
		assert.ok(
			Object.keys(store).every((name) => exports.includes(name)),
			"shutdown wrapper must preserve the complete store API",
		);
	});

	for (const call of [
		"eventLog.tail",
		"eventLog.read",
		"queue.pendingItems",
		"metadata.isProcessing",
		"native.send",
	]) {
		test(`AC1: ${call} hangs hold at the SDK deadline without ACK or reinjection`, async (t) => {
			const f = fixture(t, {
				consume: !call.startsWith("queue") && !call.startsWith("metadata"),
				boundary: call.startsWith("queue") || call.startsWith("metadata"),
			});
			const late = deferred();
			const [api, method] = call.split(".");
			const owner = api === "native" ? f.native : f.native.rpc[api];
			owner[method] = () => {
				f.calls.push(call);
				return late.promise;
			};
			const run = f.bridge.run();
			await flush();
			assert.ok(f.calls.includes(call), "the hung native operation was actually invoked");
			await f.advance(DEADLINE - 1);
			assert.equal(held(f).length, 0, "no early hold");
			await f.advance(1);
			assert.equal(held(f).length, 1, "the native hang must not renew silently forever");
			assert.ok(held(f)[0].safeDiagnostic.includes(call));
			assert.match(held(f)[0].safeDiagnostic, /deadline/);
			assert.deepEqual(f.acks, []);
			assert.equal(f.calls.includes("journal.rearm"), false);
			late.resolve(
				call === "native.send"
					? "late-native-id"
					: { events: [user], cursor: "late", hasMore: false, cursorStatus: "ok" },
			);
			await run;
			await flush();
			assert.deepEqual(f.acks, [], "late native proof cannot escape the deadline hold");
			assert.equal(
				f.records.get(claim.message.msg_id)?.state,
				call.startsWith("eventLog.") ? undefined : call === "native.send" ? "pending" : "accepted",
			);
			assert.equal(f.timers.size, 0, "settled and cancelled deadlines are released");
		});
	}

	test("AC1: stop cancels a hung native read before deadline and ignores late consumption", async (t) => {
		const f = fixture(t);
		const late = deferred();
		f.native.rpc.eventLog.read = () => late.promise;
		const run = f.bridge.run();
		await flush();
		f.bridge.stop("SIGTERM");
		await run;
		late.resolve({ events: [user], cursor: "late", hasMore: false, cursorStatus: "ok" });
		await f.advance(DEADLINE);
		assert.deepEqual(f.acks, []);
		assert.deepEqual(held(f), []);
		assert.equal(f.timers.size, 0);
	});

	test("AC1: rejected SDK reads name the call without projecting private native errors", async (t) => {
		const f = fixture(t);
		f.native.rpc.eventLog.read = async () => {
			throw new Error("PRIVATE_BODY SECRET_TOKEN");
		};
		await f.bridge.run();
		assert.match(held(f)[0].safeDiagnostic, /eventLog\.read/);
		assert.doesNotMatch(JSON.stringify(f.reports), /SECRET_TOKEN/);
		assert.deepEqual(f.acks, []);
		assert.equal(
			(await f.bridge.send({ to: "pij-peer", message: "outgoing survives startup read failure" }))
				.ok,
			true,
		);
	});

	test("AC2 H1: a read hung after abort and model change holds the completion barrier", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		assert.equal(f.acks.length, 1);
		f.native.rpc.eventLog.read = () => new Promise(() => {});
		f.rebase([
			user,
			{ type: "abort", id: "abort", data: {} },
			{ type: "session.model_change", id: "model-change", data: {} },
			...terminal,
		]);
		await f.tick();
		await f.advance(DEADLINE);
		assert.equal(held(f).length, 1);
		assert.match(held(f)[0].safeDiagnostic, /eventLog\.read.*deadline/);
		assert.equal(f.state.claims, 1);
		assert.equal(
			f.reports.some((event) => event.kind === "native-completed"),
			false,
		);
		await run;
	});

	test("AC2 H2: empty stale cursor recovers correlated terminal from a bounded backward window", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		f.rebase([user, { type: "session.model_change", id: "model-change", data: {} }, ...terminal]);
		await pump(f);
		assert.equal(f.reports.filter((event) => event.kind === "receiver-rebaselined").length, 1);
		assert.equal(
			f.reports.find((event) => event.kind === "native-completed")?.eventId,
			"owned-end",
		);
		assert.equal(f.state.claims, 2, "terminal evidence unlocks the next daemon claim");
		assert.equal(f.acks.length, 1);
		assert.equal(f.sends.length, 1);
		assert.deepEqual(held(f), []);
		assert.deepEqual(
			f.reads.find((read) => read.direction === "backward"),
			{ direction: "backward", max: 128, includeEphemeral: false },
		);
		assert.doesNotMatch(JSON.stringify(f.reports), /PRIVATE_BODY|PRIVATE_RESPONSE/);
		f.bridge.stop();
		await run;
	});

	test("AC2 H3: mid-turn steering with only ephemeral tail progress delivers after the turn instead of holding", async (t) => {
		// Observed on Copilot ab7ead89: an immediate send while busy is persisted as
		// user.message only at the next model call, while ephemeral events (e.g.
		// pending_messages.modified) advance the SDK tail cursor at once.
		const f = fixture(t, { consume: false });
		const prior = [
			{ type: "assistant.turn_start", id: "busy-turn", data: { interactionId: "busy" } },
			{ type: "tool.execution_start", id: "busy-tool", parentId: "busy-turn", data: {} },
		];
		f.state.history.push(...prior);
		const run = f.bridge.run();
		await flush();
		assert.equal(f.sends.length, 1);
		f.state.ephemeral++;
		await pump(f);
		f.state.ephemeral++;
		await pump(f);
		assert.deepEqual(held(f), [], "ephemeral-only tail progress is not a durable gap");
		assert.equal(f.acks.length, 0, "consumption is not yet proven");
		f.state.history.push(user, ...terminal);
		await pump(f);
		assert.deepEqual(held(f), []);
		assert.equal(f.acks.length, 1, "the held message is acknowledged once consumed");
		assert.equal(f.sends.length, 1, "no reinjection");
		assert.equal(
			f.reports.find((event) => event.kind === "native-completed")?.eventId,
			"owned-end",
		);
		assert.equal(f.state.claims, 2, "terminal evidence unlocks the next daemon claim");
		f.bridge.stop();
		await run;
	});

	test("AC2: observed abort completes only its consumed native turn before picker resubmission", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		f.rebase([
			user,
			{ type: "assistant.turn_end", id: "aborted-turn-end", parentId: user.id, data: {} },
			{
				type: "abort",
				id: "owned-abort",
				parentId: "aborted-turn-end",
				data: { reason: "user_abort" },
			},
			{
				type: "session.model_change",
				id: "picker",
				parentId: "owned-abort",
				data: { source: "model_picker" },
			},
			{
				type: "user.message",
				id: "resubmitted-user",
				parentId: "picker",
				data: { messageId: "different-native-id", interactionId: "different-interaction" },
			},
		]);
		await pump(f);
		assert.equal(
			f.reports.find((event) => event.kind === "native-completed")?.eventId,
			"owned-abort",
		);
		assert.equal(
			f.reports.find((event) => event.kind === "native-completed")?.nativeMessageId,
			"native-progress",
		);
		assert.equal(f.acks.length, 1, "resubmission does not authorize another Pij acknowledgement");
		assert.equal(f.sends.length, 1, "the aborted Pij message is not reinjected");
		assert.equal(f.state.claims, 2, "observed abort releases the completion barrier");
		assert.deepEqual(held(f), []);
		f.bridge.stop();
		await run;
	});

	test("AC2: retained matched ancestry can recover terminal even when the user fell outside the window", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		f.rebase(terminal);
		await pump(f);
		assert.equal(
			f.reports.find((event) => event.kind === "native-completed")?.eventId,
			"owned-end",
		);
		assert.equal(
			f.reports.find((event) => event.kind === "receiver-rebaselined")?.gap.contiguous,
			false,
		);
		assert.equal(f.state.claims, 2);
		f.bridge.stop();
		await run;
	});

	// "empty"/"expired" fail the backward page's own validity check (malformed
	// cursor/empty page), which is unconditionally non-retryable and unaffected
	// by the receiver-gap retry window. The other three reach a valid-but-
	// uncorrelated anchor (anchor < 0, no terminal) and now retry in place for
	// up to HOLD_ESCALATION_MS before escalating to the same final hold.
	const retriedBeforeHold = new Set(["unrelated-terminal", "subagent-spoof", "consumption-only"]);
	for (const gap of [
		"empty",
		"expired",
		"unrelated-terminal",
		"subagent-spoof",
		"consumption-only",
	]) {
		test(`AC2: inaccessible ${gap} gap holds without spoofed completion, ACK or resend`, async (t) => {
			const f = fixture(t, { consume: false });
			const run = f.bridge.run();
			await flush();
			const stranger = { ...user, id: "stranger", data: { messageId: "another-native-id" } };
			const spoof = { ...user, agentId: "subagent" };
			const window =
				gap === "empty"
					? []
					: gap === "consumption-only"
						? [user]
						: gap === "subagent-spoof"
							? [spoof, ...terminal]
							: [stranger, ...terminal];
			f.rebase(window);
			if (gap === "expired") f.state.windowStatus = "expired";
			await pump(f);
			if (retriedBeforeHold.has(gap)) {
				assert.equal(
					held(f).length,
					0,
					"a receiver-gap condition retries in place before escalating to a hold",
				);
				assert.ok(
					f.reports.some(
						(event) => event.kind === "reconnecting" && event.holdKind === "receiver-gap",
					),
					"a retryable receiver-gap episode narrates as reconnecting, not held",
				);
				f.state.now += store.HOLD_ESCALATION_MS;
				await f.tick();
			}
			assert.equal(
				held(f).length,
				1,
				"an inaccessible gap must fail closed rather than wait silently",
			);
			assert.equal(
				f.reports.some((event) => event.kind === "native-completed"),
				false,
			);
			assert.deepEqual(f.acks, []);
			assert.equal(f.sends.length, 1);
			assert.equal(f.calls.includes("journal.rearm"), false);
			await run;
		});
	}

	test("AC2: empty queue and fresh idle cannot authorize resend through a stale cursor", async (t) => {
		const f = fixture(t, { consume: false });
		const run = f.bridge.run();
		await flush();
		f.rebase([{ type: "session.model_change", id: "lost-consumption-gap", data: {} }]);
		f.event({ type: "session.idle", id: "fresh-but-unrelated-boundary", data: {} });
		await f.tick();
		// This anchor-gap condition now retries in place (AC-01) before discard
		// recovery's own uncorrelated-gap check can even run; drive it past the
		// escalation ceiling to reach the same final hold.
		assert.equal(held(f).length, 0, "a receiver-gap condition retries before escalating");
		f.state.now += store.HOLD_ESCALATION_MS;
		await f.tick();
		assert.equal(held(f).length, 1, "discard recovery checks progress before its first retry");
		assert.deepEqual(f.acks, []);
		assert.equal(f.sends.length, 1);
		assert.equal(f.calls.includes("journal.rearm"), false);
		await run;
	});

	test("AC2: a receiver-gap retry resolves via a live terminal event before the escalation ceiling", async (t) => {
		const f = fixture(t, { consume: false });
		const run = f.bridge.run();
		await flush();
		f.rebase([{ type: "session.model_change", id: "unrelated-window-event", data: {} }]);
		await pump(f);
		assert.equal(held(f).length, 0, "a receiver-gap condition retries in place before escalating");
		assert.ok(
			f.reports.some((event) => event.kind === "reconnecting" && event.holdKind === "receiver-gap"),
			"the retry narrates as reconnecting with a distinguishable holdKind, not held",
		);
		// The live push channel (not the lagging queryable eventLog window) delivers
		// the correlated turn while the retry is still in its bounded window.
		f.event(user);
		for (const event of terminal) f.event(event);
		await f.tick();
		assert.equal(held(f).length, 0, "a live terminal event resolves the gap without ever holding");
		assert.equal(
			f.reports.find((event) => event.kind === "native-completed")?.eventId,
			"owned-end",
		);
		f.bridge.stop();
		await run;
	});

	test("AC1: a receiver-gap retry does not force a redundant /v1/register round-trip on every backoff cycle", async (t) => {
		const f = fixture(t, { consume: false });
		let registrations = 0;
		const request = f.client.request;
		f.client.request = async (path, body, signal) => {
			if (path === "/v1/register") registrations++;
			return request(path, body, signal);
		};
		const run = f.bridge.run();
		await flush();
		assert.equal(registrations, 1, "the first attempt already registered");
		f.rebase([{ type: "session.model_change", id: "unrelated-window-event", data: {} }]);
		await pump(f);
		assert.equal(held(f).length, 0, "a receiver-gap condition retries in place before escalating");
		assert.equal(
			registrations,
			1,
			"a receiver-gap retry is not a registration problem and must not force re-registration " +
				"on every backoff cycle while simply waiting for the event log to catch up",
		);
		f.bridge.stop();
		await run;
	});

	test("AC1: gapFirstSeenAt resets when the anchor gap resolves via the caught-up branch, not only the fall-through path", async (t) => {
		const f = fixture(t, { consume: false });
		const run = f.bridge.run();
		await flush();
		const eventA = { type: "session.model_change", id: "stale-first-gap-event", data: {} };
		f.rebase([eventA]);
		await pump(f);
		assert.equal(held(f).length, 0, "the first gap retries in place");
		const completion = f.bridge.completion;
		assert.ok(completion.gapFirstSeenAt !== undefined, "the first gap stamped gapFirstSeenAt");
		// Simulate the normal forward-read path (readEvents) independently catching
		// completion.lastEventId up to the window's newest event -- the real "no
		// gap" steady state, reached via the anchor === last-index branch, not the
		// gap branch's own fall-through reset at the bottom of probeReceiver.
		completion.lastEventId = eventA.id;
		await f.tick();
		assert.equal(
			completion.gapFirstSeenAt,
			undefined,
			"catching up through the caught-up branch must clear gapFirstSeenAt too, not only the " +
				"gap branch's own fall-through",
		);
		// Advance well past the escalation ceiling before a brand-new, unrelated gap
		// appears. If gapFirstSeenAt had leaked from the first (already-resolved)
		// gap, this new, independent gap would see itself as having already
		// exceeded the retry window and escalate straight to a non-retryable hold
		// on its very first occurrence. Growing the window (rather than replacing
		// it 1-for-1) also forces a fresh forward cursor, so this probe isn't
		// short-circuited by the unchanged-cursor early return.
		f.state.now += store.HOLD_ESCALATION_MS;
		f.rebase([
			"placeholder-unrelated",
			{ type: "session.model_change", id: "fresh-second-gap-event", data: {} },
		]);
		await f.tick();
		assert.equal(
			held(f).length,
			0,
			"a brand-new, independent gap must get its own fresh retry window, not inherit a stale timestamp",
		);
		assert.ok(
			f.reports.some((event) => event.kind === "reconnecting" && event.holdKind === "receiver-gap"),
			"the fresh gap narrates as a retry, not an immediate escalate",
		);
		f.bridge.stop();
		await run;
	});

	test("AC5: heartbeat lease renewal keeps running through a receiver-gap retry window", async (t) => {
		const f = fixture(t, { consume: false });
		const run = f.bridge.run();
		await flush();
		const heartbeatsBefore = f.heartbeats.length;
		f.rebase([{ type: "session.model_change", id: "unrelated-window-event", data: {} }]);
		await pump(f);
		assert.equal(held(f).length, 0, "a receiver-gap condition retries in place before escalating");
		await f.tick(f.heartbeatWaits);
		assert.ok(
			f.heartbeats.length > heartbeatsBefore,
			"heartbeat lease renewal keeps running while the receiver retries an unresolved gap, " +
				"rather than cascading into a separate native-receiver-stale hold",
		);
		f.bridge.stop();
		await run;
	});

	test("AC5: holdReceiving aborts only the receiver, not the heartbeat controller", () => {
		const f = fixture({ after() {} });
		assert.equal(f.bridge.receiverController.signal.aborted, false);
		assert.equal(f.bridge.heartbeatController.signal.aborted, false);
		f.bridge.holdReceiving(new store.NativeError("direct unit check"));
		assert.equal(
			f.bridge.receiverController.signal.aborted,
			true,
			"holdReceiving still stops the receiver",
		);
		assert.equal(
			f.bridge.heartbeatController.signal.aborted,
			false,
			"holdReceiving no longer unconditionally kills the heartbeat",
		);
	});

	test("AC2: empty idle receiver leaves SDK observation to the next delivery", async (t) => {
		const f = fixture(t, { claims: false });
		const run = f.bridge.run();
		await flush();
		const reads = f.reads.length;
		f.rebase([{ type: "session.model_change", id: "idle-model", data: {} }]);
		await pump(f);
		await f.tick(f.heartbeatWaits);
		assert.equal(f.reads.length, reads, "no SDK reads while the inbox is empty");
		assert.equal(f.waits.length, 0, "no observation poll timer");
		assert.equal(f.heartbeats.at(-1).observed_seq, 0);
		assert.deepEqual(f.sends, []);
		f.bridge.stop();
		await run;
	});

	test("AC3: idle heartbeats do not manufacture observation progress", async (t) => {
		const f = fixture(t, { claims: false });
		const run = f.bridge.run();
		await flush();
		await pump(f);
		await f.tick(f.heartbeatWaits);
		assert.deepEqual(
			f.heartbeats.map(({ observed_at, observed_seq }) => ({ observed_at, observed_seq })),
			[
				{ observed_at: 0, observed_seq: 0 },
				{ observed_at: 0, observed_seq: 0 },
			],
		);
		assert.deepEqual(held(f), []);
		assert.equal(
			f.reports.some((event) => event.kind === "receiver-rebaselined"),
			false,
		);
		f.bridge.stop();
		await run;
	});

	test("AC3: callback/history overlap counts once and observation time advances despite clock rollback", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		await f.tick(f.heartbeatWaits);
		const before = f.heartbeats.at(-1);
		assert.equal(before.observed_seq, 1);
		f.event(user);
		f.state.now = 1;
		f.event({ type: "session.model_change", id: "new-observation", data: {} });
		await f.tick(f.heartbeatWaits);
		const after = f.heartbeats.at(-1);
		assert.equal(after.observed_seq, 2);
		assert.ok(after.observed_at > before.observed_at);
		assert.ok(Number.isSafeInteger(after.observed_at));
		f.bridge.stop();
		await run;
	});

	test("AC3: fast replay cannot manufacture a future lease watermark", async (t) => {
		const f = fixture(t, { claims: false });
		const run = f.bridge.run();
		await flush();
		for (let index = 0; index < 100_000; index++)
			f.event({ type: "session.model_change", id: `bulk-${index}`, data: {} });
		await f.tick(f.heartbeatWaits);
		const progress = f.heartbeats.at(-1);
		assert.equal(progress.observed_seq, 100_000);
		assert.ok(
			progress.observed_at <= f.state.now,
			`replay watermark ${progress.observed_at} exceeds actual time ${f.state.now}`,
		);
		f.bridge.stop();
		await run;
	});

	test("AC3: healthy replacement observes SDK history before its first stale-lease renewal", async (t) => {
		const f = fixture(t, { claims: false });
		f.state.history.push({ type: "session.model_change", id: "current-session-tail", data: {} });
		const request = f.client.request;
		f.client.request = async (path, body) => {
			if (path === "/v1/inbox/heartbeat")
				f.state.lease =
					body.observed_at > 99999 && body.observed_seq > 0
						? { state: "live", lease_ms: 60000, renew_after_ms: 20000 }
						: {
								state: "stale",
								reason: "native-receiver-stale",
								lease_ms: 60000,
								renew_after_ms: 20000,
							};
			return request(path, body);
		};
		const run = f.bridge.run();
		await flush();
		assert.deepEqual(
			held(f),
			[],
			"replacement must not hold before proving its responsive SDK view",
		);
		assert.equal(f.heartbeats[0].observed_seq, 1);
		assert.equal(f.state.claims, 1, "replacement reaches ordinary inbox admission");
		assert.deepEqual(f.acks, []);
		assert.deepEqual(f.sends, []);
		f.bridge.stop();
		await run;
	});

	test("AC3: daemon stale lease is receive-held once, stops renewals and preserves outgoing tools", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		f.state.lease = {
			state: "stale",
			reason: "native-receiver-stale",
			lease_ms: 60000,
			renew_after_ms: 20000,
		};
		await f.tick(f.heartbeatWaits);
		assert.equal(held(f).length, 1);
		assert.equal(held(f)[0].safeDiagnostic, "native-receiver-stale");
		assert.equal(
			f.reports.some((event) => event.kind === "receiver-lease-unavailable"),
			false,
		);
		await pump(f, f.heartbeatWaits);
		assert.equal(f.heartbeats.length, 2);
		assert.equal(f.state.claims, 1);
		assert.equal((await f.bridge.send({ to: "pij-peer", message: "still available" })).ok, true);
		// Self-healing now keeps run() retrying (reconnect()'s backoff wait) rather
		// than resolving quietly; an explicit stop still ends it, same as any other
		// in-flight attempt.
		f.bridge.stop();
		await run;
	});

	test("AC3: stale lease self-heals by re-registering and reclaiming without extensions_reload", async (t) => {
		const f = fixture(t, { claims: false });
		const request = f.client.request;
		let registrations = 0;
		f.client.request = async (path, body, signal) => {
			if (path === "/v1/register") registrations++;
			return request(path, body, signal);
		};
		const run = f.bridge.run();
		await flush();
		assert.equal(registrations, 1);
		const claimsBefore = f.state.claims;
		assert.ok(claimsBefore >= 1, "the first attempt already reached claimInbox");
		f.state.lease = {
			state: "stale",
			reason: "native-receiver-stale",
			lease_ms: 60000,
			renew_after_ms: 20000,
		};
		await f.tick(f.heartbeatWaits);
		assert.equal(held(f).length, 1);
		assert.equal(held(f)[0].safeDiagnostic, "native-receiver-stale");
		assert.equal(
			f.reports.some((event) => event.kind === "reconnecting"),
			true,
			"a retryable stale hold announces the upcoming self-heal attempt",
		);
		const heartbeatsBefore = f.heartbeats.length;
		// Simulate the daemon side of the contract: a fresh attempt's observation
		// baseline advances past what the daemon last saw, clearing staleness
		// (crates/daemon/src/delivery/mod.rs's `advanced` check) on its own.
		f.state.lease = { state: "live", lease_ms: 60000, renew_after_ms: 20000 };
		// The reconnect backoff wait lives on the generic delay queue, not the
		// heartbeat queue; resolving it lets reconnect() start a fresh attempt.
		await f.tick(f.waits);
		assert.equal(
			registrations,
			2,
			"reconnect re-registered instead of requiring extensions_reload",
		);
		assert.ok(
			f.state.claims > claimsBefore,
			"reconnect reached claimInbox again after re-registering, without extensions_reload",
		);
		assert.ok(
			f.heartbeats.length > heartbeatsBefore,
			"heartbeat renewal resumed on the fresh attempt",
		);
		assert.equal(held(f).length, 1, "the fresh attempt settles live, not a second hold");
		f.bridge.stop();
		await run;
	});

	test("AC3: lease-expiry-then-recovery (heartbeat transport failures then success) self-reclaims", async (t) => {
		const f = fixture(t, { claims: false });
		const request = f.client.request;
		let failHeartbeats = 0;
		let registrations = 0;
		f.client.request = async (path, body, signal) => {
			if (path === "/v1/register") registrations++;
			if (path === "/v1/inbox/heartbeat" && failHeartbeats > 0) {
				failHeartbeats--;
				throw new store.NativeError("heartbeat transport failed", true);
			}
			return request(path, body, signal);
		};
		const run = f.bridge.run();
		await flush();
		assert.equal(registrations, 1);
		failHeartbeats = 3;
		// Transient transport failures (e.g. the daemon briefly down) must not hold
		// receiving at all; keepReceiverAlive's own capped backoff already covers
		// this without any new machinery.
		await f.tick(f.heartbeatWaits);
		await f.tick(f.heartbeatWaits);
		await f.tick(f.heartbeatWaits);
		assert.equal(
			held(f).length,
			0,
			"transient heartbeat transport failures must not hold receiving",
		);
		assert.equal(registrations, 1, "transient heartbeat failures do not re-register");
		const claimsBefore = f.state.claims;
		// Once connectivity is back, a genuine lease expiry can still surface; the
		// bridge must then self-reclaim on its own rather than staying wedged.
		f.state.lease = {
			state: "stale",
			reason: "native-receiver-stale",
			lease_ms: 60000,
			renew_after_ms: 20000,
		};
		await f.tick(f.heartbeatWaits);
		assert.equal(held(f).length, 1);
		f.state.lease = { state: "live", lease_ms: 60000, renew_after_ms: 20000 };
		await f.tick(f.waits);
		assert.equal(registrations, 2, "self re-registers after the lease recovers, unaided");
		assert.ok(f.state.claims > claimsBefore, "self re-claims after the lease recovers, unaided");
		assert.equal(held(f).length, 1, "the recovered attempt settles live, not a second hold");
		f.bridge.stop();
		await run;
	});

	test("AC4: completed native work is not reported as outstanding on stop", async (t) => {
		const f = fixture(t);
		const run = f.bridge.run();
		await flush();
		for (const event of terminal) f.event(event);
		await flush();
		assert.equal(f.reports.filter((event) => event.kind === "native-completed").length, 1);
		f.bridge.stop("process.exit:0");
		await run;
		assert.equal(f.reports.filter((event) => event.kind === "receiver-stopped").length, 0);
	});

	for (const accepted of [false, true]) {
		test(`AC4: shutdown logs one outstanding job identity ${accepted ? "after" : "before"} consumption`, async (t) => {
			const f = fixture(t, { consume: accepted });
			const run = f.bridge.run();
			await flush();
			f.event({ type: "session.shutdown", data: {} });
			f.bridge.stop("process.exit:0");
			await run;
			assert.deepEqual(
				f.reports
					.filter((event) => event.kind === "receiver-stopped")
					.map(({ cause, msgId, jobId, nativeMessageId }) => ({
						cause,
						msgId,
						jobId,
						nativeMessageId,
					})),
				[
					{
						cause: "session.shutdown",
						msgId: claim.message.msg_id,
						jobId: claim.job_id,
						nativeMessageId: "native-progress",
					},
				],
			);
			assert.equal(f.acks.length, accepted ? 1 : 0);
		});
	}

	for (const cause of ["process.exit:0", "session.shutdown", "SIGTERM"]) {
		test(`AC4: actual extension ${cause} logs outstanding completion exactly once`, {
			timeout: 5000,
		}, async (t) => {
			const env = {
				...process.env,
				PIJ_PROGRESS_EXIT_CHILD: "1",
				SESSION_ID: registration.harness_session,
			};
			delete env.NODE_TEST_CONTEXT;
			delete env.TMUX_PANE;
			const child = spawn(
				process.execPath,
				[
					"--import",
					fileURLToPath(import.meta.url),
					fileURLToPath(new URL("./extension.mjs", import.meta.url)),
				],
				{ env, stdio: ["pipe", "pipe", "pipe"] },
			);
			t.after(() => {
				if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
			});
			let stderr = "";
			const ready = deferred();
			child.stderr.on("data", (bytes) => {
				stderr += bytes;
				if (stderr.includes('"kind":"completion-wait"')) ready.resolve();
			});
			const exited = new Promise((resolve, reject) => {
				child.once("error", reject);
				child.once("exit", (code, signal) => {
					ready.resolve();
					resolve({ code, signal });
				});
			});
			await ready.promise;
			assert.ok(stderr.includes('"kind":"completion-wait"'), stderr);
			if (cause === "SIGTERM") child.kill("SIGTERM");
			else child.stdin.write(cause === "process.exit:0" ? "exit\n" : "shutdown\n");
			const exit = await exited;
			assert.deepEqual(exit, { code: 0, signal: null }, stderr);
			const diagnostics = stderr
				.split("\n")
				.filter((line) => line.startsWith("[pij-native] "))
				.map((line) => JSON.parse(line.slice("[pij-native] ".length)));
			assert.deepEqual(
				diagnostics
					.filter((event) => event.kind === "receiver-stopped")
					.map(({ cause, msgId, jobId }) => ({ cause, msgId, jobId })),
				[{ cause, msgId: claim.message.msg_id, jobId: claim.job_id }],
			);
		});
	}
}
