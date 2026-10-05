import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { setImmediate as flush } from "node:timers/promises";
import {
	chooseRegistration,
	createNativeReporter,
	DaemonClient,
	FileJournal,
	NativeBridge,
	parseProcessStart,
	resolveRegistration,
} from "./store.mjs";

const registration = Object.freeze({
	id: "pij-test",
	harness: "copilot",
	harness_session: "native-137",
	folder: "/work",
	pid: 137,
	proc_start: 20260905120000,
	pane: "%137",
	relay: false,
	native_extension_delivery: true,
});
const descriptor = {
	id: registration.id,
	harness: "copilot",
	session: registration.harness_session,
	folder: registration.folder,
	pane: registration.pane,
	proc: { pid: registration.pid, proc_start: registration.proc_start },
	native_extension_delivery: true,
	typing_grace_ms: 60000,
};
const claim = {
	job_id: 137,
	message: {
		msg_id: "message-137",
		from: "pij-peer",
		to: registration.id,
		body: "Hello; /new stays data",
	},
	native_consumer: {
		native_session: registration.harness_session,
		pid: registration.pid,
		proc_start: registration.proc_start,
	},
};
const tuple = {
	seat: registration.id,
	native_session: registration.harness_session,
	pid: registration.pid,
	proc_start: registration.proc_start,
};
function observedTyping(overrides = {}, consumer = tuple) {
	return {
		state: "observed",
		native_consumer: {
			native_session: consumer.native_session,
			pid: consumer.pid,
			proc_start: consumer.proc_start,
		},
		typing_grace_ms: 60000,
		observed_at_ms: 100000,
		retry_after_ms: 0,
		source: "pane-observed-edit-recency",
		...overrides,
	};
}
function unavailableTyping(overrides = {}) {
	const { retry_after_ms, source, ...snapshot } = observedTyping();
	return {
		...snapshot,
		state: "unavailable",
		reason: "native-typing-sensor-unavailable",
		...overrides,
	};
}
const unavailableRefusal = {
	ok: false,
	v: 2,
	command: "pij inbox",
	error: "refused",
	meta: "daemon/native-inbox: native-extension-unavailable: Copilot requires current native registration",
};
function deferred() {
	let resolve;
	const promise = new Promise((r) => {
		resolve = r;
	});
	return { promise, resolve };
}
function fixture(overrides = {}) {
	const trace = [];
	const records = new Map();
	const history = [];
	const accepted = deferred();
	let observer;
	let claims = 0;
	const journal = {
		async load(message) {
			return records.get(message.msg_id);
		},
		async begin(message) {
			trace.push("intent");
			if (records.has(message.msg_id)) return false;
			records.set(message.msg_id, { state: "pending", message });
			return true;
		},
		async accept(message, nativeId) {
			trace.push("persist");
			records.set(message.msg_id, { state: "accepted", message, nativeId });
		},
		async rearm(message, nativeId) {
			const record = records.get(message.msg_id);
			if (record?.state !== "accepted" || record.nativeId !== nativeId) return false;
			trace.push("retry-intent");
			records.set(message.msg_id, { state: "pending", message });
			return true;
		},
	};
	const native = {
		on(fn) {
			trace.push("subscribe");
			observer = fn;
			return () => trace.push("unsubscribe");
		},
		async send(input) {
			trace.push("send");
			assert.equal(input.mode, "immediate");
			history.push({
				type: "user.message",
				id: "user-native-message-137",
				data: { messageId: "native-message-137" },
			});
			return "native-message-137";
		},
		async getEvents() {
			return history;
		},
		rpc: {
			eventLog: {
				async tail() {
					const events = await native.getEvents();
					if (!Array.isArray(events)) return undefined;
					return { cursor: String(events.length) };
				},
				async read({ cursor = "0", max, direction }) {
					if (direction === "backward")
						return {
							events: history.slice(-max),
							cursor: "backward-tail",
							hasMore: history.length > max,
							cursorStatus: "ok",
						};
					const snapshot = await native.getEvents();
					const events = snapshot.slice(Number(cursor), Number(cursor) + max);
					const next = Number(cursor) + events.length;
					return {
						events,
						cursor: String(next),
						hasMore: next < snapshot.length,
						cursorStatus: "ok",
					};
				},
			},
		},
	};
	const client = {
		async nativeSnapshot(consumer, signal) {
			assert.equal(signal.aborted, false);
			trace.push("identity");
			return observedTyping({}, consumer);
		},
		async claimInbox(consumer, signal) {
			const query = new URLSearchParams({ ...consumer, wait: "true" });
			return { claims: await this.request(`/v1/inbox?${query}`, undefined, signal), hold: null };
		},
		async request(path, body, signal) {
			if (path === "/v1/inbox/heartbeat")
				return { state: "live", lease_ms: 60000, renew_after_ms: 20000 };
			if (path === "/v1/register") {
				trace.push("register");
				return descriptor;
			}
			if (path.startsWith("/v1/inbox?")) {
				trace.push("claim");
				assert.deepEqual(Object.fromEntries(new URLSearchParams(path.split("?")[1])), {
					...Object.fromEntries(Object.entries(tuple).map(([k, v]) => [k, String(v)])),
					wait: "true",
				});
				if (claims++ === 0) return [claim];
				return new Promise((resolve) =>
					signal.addEventListener("abort", () => resolve([]), { once: true }),
				);
			}
			if (path === "/v1/inbox/ack") {
				trace.push("ack");
				assert.deepEqual(body, { ...tuple, job_id: claim.job_id });
				return claim.job_id;
			}
			if (path === "/v1/send") {
				trace.push("outgoing");
				return { msg_id: body.msg_id, outcome: { outcome: "queued" }, at: 137 };
			}
			throw new Error(`Unexpected path ${path}`);
		},
	};
	const reports = [];
	const bridge = new NativeBridge({
		registration,
		journal,
		native,
		client,
		report: (event) => {
			reports.push(event);
			if (event.kind === "inbox-acknowledged") accepted.resolve();
		},
		...overrides,
	});
	return {
		bridge,
		trace,
		records,
		history,
		accepted,
		journal,
		native,
		client,
		reports,
		event: (event) => observer(event),
	};
}

function discardedQueueFixture() {
	const f = fixture({ delay: () => flush() });
	const state = { processing: true, items: [], sends: 0, queueReads: 0, processingReads: 0 };
	state.steeringMessages = [];
	f.native.rpc = {
		...f.native.rpc,
		queue: {
			pendingItems: async () => {
				state.queueReads++;
				return {
					items: state.items,
					steeringMessages: state.steeringMessages,
					inFlightSteeringCount: state.inFlightSteeringCount,
				};
			},
		},
		metadata: {
			isProcessing: async () => {
				state.processingReads++;
				return { processing: state.processing };
			},
		},
	};
	f.native.send = async () => {
		f.trace.push("send");
		const nativeId = `queued-native-${++state.sends}`;
		if (state.sends > 1)
			f.history.push(
				{ type: "user.message", id: "recovered-user", data: { messageId: nativeId } },
				{
					type: "assistant.message",
					id: "recovered-final",
					parentId: "recovered-user",
					data: { toolRequests: [] },
				},
				{ type: "assistant.turn_end", id: "recovered-end", parentId: "recovered-final", data: {} },
			);
		return nativeId;
	};
	return { ...f, state };
}

test("native discard recovery: discarded enqueue resends once after the foreground idle boundary", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await flush();
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("ack"), false);
	f.state.processing = false;
	f.history.push({
		type: "assistant.turn_end",
		id: "foreground-end",
		parentId: "foreground-answer",
		data: {},
	});
	await flush();
	await flush();
	assert.equal(f.state.sends, 2);
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	assert.equal(f.records.get(claim.message.msg_id).nativeId, "queued-native-2");
	assert.equal(f.reports.filter((event) => event.kind === "native-requeued").length, 1);
	assert.ok(f.state.queueReads > 0);
	assert.ok(f.state.processingReads > 0);
	await flush();
	assert.equal(f.state.sends, 2);
	f.bridge.stop();
	await run;
});

test("native discard recovery: no resend while a potentially consuming turn is still processing", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await flush();
	f.history.push({
		type: "assistant.turn_end",
		id: "tool-turn-end",
		parentId: "tool-result",
		data: {},
	});
	await flush();
	await flush();
	assert.ok(f.state.processingReads > 0);
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.records.get(claim.message.msg_id).state, "accepted");
	assert.equal(
		f.reports.some((event) => event.kind === "native-requeued"),
		false,
	);
	f.bridge.stop();
	await run;
});

for (const identity of ["native ID", "exact display prefix"]) {
	test(`native discard recovery: queued item matched by ${identity} waits for later consumption`, async (t) => {
		const f = discardedQueueFixture();
		t.after(() => f.bridge.stop());
		f.state.processing = false;
		f.state.items = [
			{
				id: identity === "native ID" ? "queued-native-1" : "different-queue-item-id",
				kind: "message",
				displayText:
					identity === "native ID"
						? "opaque preview"
						: `[pij from ${JSON.stringify(claim.message.from)}; msg_id=${JSON.stringify(claim.message.msg_id)}]\n${claim.message.body}`,
				agentMode: "interactive",
			},
		];
		const run = f.bridge.run();
		await flush();
		f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
		await flush();
		await flush();
		assert.ok(f.state.queueReads > 0);
		assert.equal(f.state.sends, 1);
		assert.equal(f.trace.includes("ack"), false);
		f.state.items = [];
		f.state.processing = true;
		f.history.push({
			type: "user.message",
			id: "later-user",
			data: { messageId: "queued-native-1" },
		});
		await f.accepted.promise;
		assert.equal(f.state.sends, 1);
		assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
		f.bridge.stop();
		await run;
	});
}

test("native discard recovery: fresh consumption proof wins over empty queue and ACKs once", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await flush();
	f.state.processing = false;
	f.history.push(
		{ type: "assistant.turn_end", id: "foreground-end", data: {} },
		{ type: "user.message", id: "consumed-user", data: { messageId: "queued-native-1" } },
		{ type: "assistant.message", id: "consumed-answer", parentId: "consumed-user", data: {} },
		{ type: "assistant.turn_end", id: "consumed-end", parentId: "consumed-answer", data: {} },
	);
	await f.accepted.promise;
	await flush();
	await flush();
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 2);
	f.bridge.stop();
	await run;
});

test("native discard recovery: replayed historical boundary cannot authorize resend", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	f.state.processing = false;
	f.history.push({ type: "assistant.turn_end", id: "before-enqueue-end", data: {} });
	const run = f.bridge.run();
	await flush();
	await flush();
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("ack"), false);
	f.history.push({ type: "assistant.turn_end", id: "after-enqueue-end", data: {} });
	await flush();
	await flush();
	assert.equal(f.state.sends, 2);
	f.bridge.stop();
	await run;
});

test("native discard recovery: missing rpc holds visibly at a fresh boundary without ACK or resend", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	delete f.native.rpc.queue;
	delete f.native.rpc.metadata;
	const run = f.bridge.run();
	await flush();
	f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
	await flush();
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("ack"), false);
	assert.ok(
		f.reports.some(
			(event) =>
				event.kind === "receive-held" && /recovery.*queue.*processing/i.test(event.diagnostic),
		),
	);
	f.bridge.stop();
	await run;
});

for (const veto of ["steering prefix", "inflight count"]) {
	test(`native discard recovery: ${veto} vetoes duplicate injection`, async (t) => {
		const f = discardedQueueFixture();
		t.after(() => f.bridge.stop());
		f.state.processing = false;
		if (veto === "steering prefix")
			f.state.steeringMessages = [
				`[pij from ${JSON.stringify(claim.message.from)}; msg_id=${JSON.stringify(claim.message.msg_id)}]\n${claim.message.body}`,
			];
		else f.state.inFlightSteeringCount = 1;
		const run = f.bridge.run();
		await flush();
		f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
		await flush();
		await flush();
		assert.equal(f.state.sends, 1);
		assert.equal(f.trace.includes("ack"), false);
		assert.equal(f.trace.includes("retry-intent"), false);
		assert.equal(
			f.reports.some((event) => event.kind === "native-requeued"),
			false,
		);
		assert.ok(f.state.queueReads > 0);
		f.bridge.stop();
		await run;
	});
}

test("native discard recovery: successful retry notice uses native timeline without message content", async () => {
	const logs = [];
	const captured = [];
	const report = createNativeReporter({
		log: async (message, options) => logs.push({ message, options }),
		capture: (event) => captured.push(event),
	});
	await report({
		kind: "native-requeued",
		msgId: "opaque-message",
		nativeMessageId: "opaque-native",
	});
	assert.deepEqual(logs, [
		{ message: "📨 re-queued 1 pij messages after interrupt", options: { level: "info" } },
	]);
	assert.equal(captured.filter((event) => event.kind === "native-requeued").length, 1);
});

test("terminal registration failure replaces retry advice without exposing raw diagnostics", async () => {
	const logs = [];
	const report = createNativeReporter({
		log: async (message) => logs.push(message),
		capture: () => undefined,
	});
	await report({ kind: "registration-wait", diagnostic: "SECRET_SERVER_BODY" });
	assert.match(logs[0], /retrying when available/);
	await report({
		kind: "extension-unavailable",
		diagnostic: "SECRET_SERVER_BODY",
		safeDiagnostic: "Native host incarnation changed during observation",
	});
	assert.equal(logs.length, 2);
	assert.match(
		logs[1],
		/native registration failed: Native host incarnation changed during observation/,
	);
	assert.match(logs[1], /no retry is scheduled.*restart the Copilot CLI \(\/restart or relaunch\)/);
	assert.match(logs[1], /ordinary Copilot remains usable/);
	assert.doesNotMatch(logs[1], /retrying when available|SECRET_SERVER_BODY/);
});

test("terminal registration failure without safe identity detail never projects an arbitrary error", async () => {
	const logs = [];
	const report = createNativeReporter({
		log: async (message) => logs.push(message),
		capture: () => undefined,
	});
	await report({ kind: "extension-unavailable", diagnostic: "SECRET_KEY ENV_VALUE MESSAGE_BODY" });
	assert.match(logs[0], /native registration failed: native initialization failed/);
	assert.doesNotMatch(logs[0], /SECRET_KEY|ENV_VALUE|MESSAGE_BODY|retrying when available/);
});

test("terminal banner projects only structured host refusal and redacts the daemon key", async () => {
	const logs = [];
	const report = createNativeReporter({
		log: async (message) => logs.push(message),
		capture: () => undefined,
	});
	const client = new DaemonClient({
		addr: "127.0.0.1:1",
		stateDir: "/isolated",
		readKey: async () => "fixture-key",
		fetch: async () =>
			new Response(
				JSON.stringify({
					ok: false,
					v: 2,
					command: "pij register",
					error: "refused",
					meta: 'native Copilot registration refused: claimed pid is not an actual Copilot host executable; observed basename="fixture-key", replaced=true',
					data: { body: "SECRET_MESSAGE_BODY", env: "SECRET_ENV_VALUE" },
				}),
				{ status: 400 },
			),
	});
	await client.request("/v1/register", registration).then(
		() => assert.fail("host refusal must fail registration"),
		(error) =>
			report({
				kind: "extension-unavailable",
				diagnostic: error.message,
				safeDiagnostic: error.safeDiagnostic,
			}),
	);
	assert.match(logs[0], /observed basename="\[redacted\]", replaced=true/);
	assert.doesNotMatch(logs[0], /fixture-key|SECRET_MESSAGE_BODY|SECRET_ENV_VALUE/);
});

test("native discard recovery: a substring in another queued item does not claim ownership", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	f.state.processing = false;
	f.state.items = [
		{
			id: "foreign-id",
			kind: "message",
			displayText: `quoted [pij from ${JSON.stringify(claim.message.from)}; msg_id=${JSON.stringify(claim.message.msg_id)}]\n${claim.message.body}`,
		},
	];
	const run = f.bridge.run();
	await flush();
	f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
	await flush();
	await flush();
	assert.equal(f.state.sends, 2);
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	f.bridge.stop();
	await run;
});

test("native discard recovery: fresh history catches consumption during queue inspection", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	f.state.processing = false;
	const pendingItems = f.native.rpc.queue.pendingItems;
	f.native.rpc.queue.pendingItems = async () => {
		f.history.push({
			type: "user.message",
			id: "racing-user",
			data: { messageId: "queued-native-1" },
		});
		return pendingItems();
	};
	const run = f.bridge.run();
	await flush();
	f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
	await f.accepted.promise;
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("retry-intent"), false);
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	f.bridge.stop();
	await run;
});

test("native discard recovery: stop during queue snapshot cannot rearm or send", async () => {
	const f = discardedQueueFixture();
	const inspecting = deferred();
	const pending = deferred();
	f.native.rpc.queue.pendingItems = () => {
		inspecting.resolve();
		return pending.promise;
	};
	const run = f.bridge.run();
	await flush();
	f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
	await inspecting.promise;
	f.bridge.stop();
	await run;
	pending.resolve({ items: [], steeringMessages: [] });
	await flush();
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("retry-intent"), false);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.records.get(claim.message.msg_id).state, "accepted");
});

test("native discard recovery: consumption during journal rearm restores acceptance without resend", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	f.state.processing = false;
	const rearm = f.journal.rearm;
	f.journal.rearm = async (message, nativeId) => {
		const owned = await rearm(message, nativeId);
		nativeUser(f, nativeId, "racing-consumption");
		return owned;
	};
	const run = f.bridge.run();
	await flush();
	f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
	await f.accepted.promise;
	assert.equal(f.state.sends, 1);
	assert.equal(f.records.get(claim.message.msg_id).state, "accepted");
	assert.equal(f.records.get(claim.message.msg_id).nativeId, "queued-native-1");
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	f.bridge.stop();
	await run;
});

for (const count of [-1, 0.5, "1", null]) {
	test(`native discard recovery: malformed inflight count ${JSON.stringify(count)} holds`, async (t) => {
		const f = discardedQueueFixture();
		t.after(() => f.bridge.stop());
		f.state.processing = false;
		f.state.inFlightSteeringCount = count;
		const run = f.bridge.run();
		await flush();
		f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
		await flush();
		await flush();
		assert.equal(f.state.sends, 1);
		assert.equal(f.trace.includes("ack"), false);
		assert.ok(
			f.reports.some(
				(event) => event.kind === "receive-held" && /malformed queue/i.test(event.diagnostic),
			),
		);
		f.bridge.stop();
		await run;
	});
}

for (const unavailable of [
	"missing queue API",
	"missing processing API",
	"malformed queue",
	"malformed processing",
	"queue error",
	"processing error",
]) {
	test(`native discard recovery: ${unavailable} never authorizes resend`, async (t) => {
		const f = discardedQueueFixture();
		t.after(() => f.bridge.stop());
		f.state.processing = false;
		if (unavailable === "missing queue API") delete f.native.rpc.queue.pendingItems;
		if (unavailable === "missing processing API") delete f.native.rpc.metadata.isProcessing;
		if (unavailable === "malformed queue") f.native.rpc.queue.pendingItems = async () => ({});
		if (unavailable === "malformed processing")
			f.native.rpc.metadata.isProcessing = async () => ({ processing: null });
		if (unavailable === "queue error")
			f.native.rpc.queue.pendingItems = async () => {
				throw new Error("queue unavailable");
			};
		if (unavailable === "processing error")
			f.native.rpc.metadata.isProcessing = async () => {
				throw new Error("processing unavailable");
			};
		const run = f.bridge.run();
		await flush();
		f.history.push({ type: "assistant.turn_end", id: "foreground-end", data: {} });
		await flush();
		await flush();
		assert.equal(f.state.sends, 1);
		assert.equal(f.trace.includes("ack"), false);
		assert.equal(f.records.get(claim.message.msg_id).state, "accepted");
		assert.ok(
			f.reports.some(
				(event) =>
					event.kind === "receive-held" &&
					/discard|recovery|queue|processing/i.test(event.diagnostic),
			),
		);
		f.bridge.stop();
		await run;
	});
}

test("native discard recovery: concurrent journal rearm preserves one exclusive retry intent", async (t) => {
	const home = await mkdtemp(join(tmpdir(), "pij-native-rearm-"));
	t.after(() => rm(home, { recursive: true, force: true }));
	const first = new FileJournal(home, registration);
	const second = new FileJournal(home, registration);
	await first.begin(claim.message);
	await first.accept(claim.message, "discarded-native");
	const winners = await Promise.all([
		first.rearm(claim.message, "discarded-native"),
		second.rearm(claim.message, "discarded-native"),
	]);
	assert.deepEqual(winners.sort(), [false, true]);
	assert.equal((await first.load(claim.message)).state, "pending");
	assert.equal(await second.begin(claim.message), false);
	await first.accept(claim.message, "recovered-native");
	assert.equal(await second.rearm(claim.message, "discarded-native"), false);
	assert.equal((await second.load(claim.message)).nativeId, "recovered-native");
});

test("native consumption: accepted send stays unacknowledged until matching user message", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	const history = [];
	f.native.getEvents = async () => history;
	const run = f.bridge.run();
	await flush();
	assert.equal(f.records.get(claim.message.msg_id).state, "accepted");
	assert.equal(f.trace.includes("ack"), false);
	history.push({
		type: "user.message",
		id: "consumed-user",
		data: { messageId: "native-message-137" },
	});
	await f.accepted.promise;
	assert.equal(f.trace.filter((entry) => entry === "send").length, 1);
	f.bridge.stop();
	await run;
});

test("native consumption: missed callbacks recover consumption and final turn from history", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	const history = [
		{
			type: "user.message",
			id: "history-user",
			data: { messageId: "native-message-137", interactionId: "history-interaction" },
		},
		{
			type: "assistant.message",
			id: "history-answer",
			parentId: "history-user",
			data: { interactionId: "history-interaction", toolRequests: [] },
		},
		{ type: "assistant.turn_end", id: "history-end", parentId: "history-answer", data: {} },
	];
	f.native.getEvents = async () => (f.trace.includes("send") ? history : []);
	const run = f.bridge.run();
	await flush();
	assert.equal(f.trace.includes("ack"), true);
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 2);
	assert.equal(
		f.reports.filter(
			(event) => event.kind === "native-completed" && event.type === "assistant.turn_end",
		).length,
		1,
	);
	f.bridge.stop();
	await run;
});

test("native consumption: unrelated history and idle cannot acknowledge acceptance", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	f.native.getEvents = async () => [
		{ type: "user.message", id: "other-user", data: { messageId: "other-native" } },
		{ type: "session.idle", id: "other-idle", parentId: "other-user", data: {} },
	];
	const run = f.bridge.run();
	await flush();
	f.event({
		type: "session.idle",
		id: "unrelated-idle",
		data: { messageId: "native-message-137" },
	});
	await flush();
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 1);
	f.bridge.stop();
	await run;
});

test("native consumption: accepted duplicate recovers history without native resend", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	f.records.set(claim.message.msg_id, {
		state: "accepted",
		message: claim.message,
		nativeId: "recovered-native",
	});
	let reads = 0;
	f.native.getEvents = async () => {
		reads++;
		return [
			{ type: "user.message", id: "recovered-user", data: { messageId: "recovered-native" } },
			{
				type: "assistant.message",
				id: "recovered-answer",
				parentId: "recovered-user",
				data: { toolRequests: [] },
			},
			{ type: "assistant.turn_end", id: "recovered-end", parentId: "recovered-answer", data: {} },
		];
	};
	const run = f.bridge.run();
	await flush();
	assert.ok(reads > 0);
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), true);
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 2);
	f.bridge.stop();
	await run;
});

test("native consumption: accepted duplicate without matching history stays unacknowledged", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	f.records.set(claim.message.msg_id, {
		state: "accepted",
		message: claim.message,
		nativeId: "missing-native",
	});
	const run = f.bridge.run();
	await flush();
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 1);
	f.bridge.stop();
	await run;
});

test("native consumption: history polling backs off and stops after correlated final completion", async (t) => {
	const waits = [];
	const f = fixture({
		delay: async (ms) => {
			waits.push(ms);
			if (waits.length === 8)
				f.history.push(
					{ type: "user.message", id: "history-user", data: { messageId: "native-message-137" } },
					{ type: "assistant.message", id: "history-answer", parentId: "history-user", data: {} },
					{ type: "assistant.turn_end", id: "history-end", parentId: "history-answer", data: {} },
				);
			await flush();
		},
	});
	t.after(() => f.bridge.stop());
	let reads = 0;
	f.native.getEvents = async () => {
		reads++;
		return waits.length < 8 ? [] : f.history;
	};
	const run = f.bridge.run();
	await f.accepted.promise;
	await flush();
	assert.ok(waits[1] > waits[0], "empty history backs off instead of hot-looping");
	assert.equal(waits.at(-1), waits.at(-2), "backoff reaches a bounded ceiling");
	const completedReads = reads;
	await flush();
	assert.equal(reads, completedReads, "correlated completion stops history polling");
	f.bridge.stop();
	await run;
});

test("native consumption: replayed tool turns and unrelated final turns cannot complete owned work", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	const history = [
		{
			type: "user.message",
			id: "owned-user",
			data: { messageId: "native-message-137", interactionId: "owned" },
		},
		{
			type: "assistant.message",
			id: "owned-tool",
			parentId: "owned-user",
			data: { interactionId: "owned", toolRequests: [{ toolCallId: "tool" }] },
		},
		{ type: "assistant.turn_end", id: "tool-end", parentId: "owned-tool", data: {} },
		{
			type: "assistant.message",
			id: "foreign-final",
			parentId: "foreign-root",
			data: { interactionId: "foreign", toolRequests: [] },
		},
		{ type: "assistant.turn_end", id: "foreign-end", parentId: "foreign-final", data: {} },
		{
			type: "assistant.message",
			id: "child-final",
			parentId: "tool-end",
			agentId: "child",
			data: { toolRequests: [] },
		},
		{
			type: "assistant.turn_end",
			id: "child-end",
			parentId: "child-final",
			agentId: "child",
			data: {},
		},
		{ type: "abort", id: "global-abort", parentId: "foreign-root", data: { reason: "user" } },
	];
	f.native.getEvents = async () => (f.trace.includes("send") ? history : []);
	const run = f.bridge.run();
	await f.accepted.promise;
	await flush();
	await flush();
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 1);
	assert.equal(
		f.reports.some((event) => event.kind === "native-completed"),
		false,
	);
	history.push(
		{
			type: "assistant.message",
			id: "owned-final",
			parentId: "omitted-permission",
			data: { interactionId: "owned", toolRequests: [] },
		},
		{ type: "assistant.turn_end", id: "owned-end", parentId: "owned-final", data: {} },
	);
	await flush();
	await flush();
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 2);
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	assert.equal(
		f.reports.filter((event) => event.kind === "native-completed" && event.eventId === "owned-end")
			.length,
		1,
	);
	f.bridge.stop();
	await run;
});

test("native consumption: stop during history read ignores late proof without ACK or resend", async () => {
	const f = fixture();
	const reading = deferred();
	const history = deferred();
	let reads = 0;
	f.native.getEvents = () => {
		if (reads++ === 0) return Promise.resolve([]);
		reading.resolve();
		return history.promise;
	};
	const run = f.bridge.run();
	await reading.promise;
	f.bridge.stop();
	await run;
	history.resolve([
		{ type: "user.message", id: "late-user", data: { messageId: "native-message-137" } },
	]);
	await flush();
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.trace.filter((entry) => entry === "send").length, 1);
	assert.equal(f.records.get(claim.message.msg_id).state, "accepted");
	assert.equal(f.trace.filter((entry) => entry === "unsubscribe").length, 1);
});

test("native consumption: callbacks can prove consumption during an outstanding history read", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const reading = deferred();
	let reads = 0;
	f.native.getEvents = () => {
		if (reads++ === 0) return Promise.resolve([]);
		reading.resolve();
		return new Promise(() => {});
	};
	const run = f.bridge.run();
	await reading.promise;
	nativeUser(f, "native-message-137");
	await f.accepted.promise;
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	f.bridge.stop();
	await run;
});

for (const unsupported of ["missing", "rejected", "malformed"]) {
	test(`native consumption: ${unsupported} history API holds without ACK`, async () => {
		const f = fixture();
		if (unsupported === "missing") delete f.native.getEvents;
		else
			f.native.getEvents = async () => {
				if (unsupported === "rejected") throw new Error("SDK disconnected");
				return undefined;
			};
		await f.bridge.run();
		assert.equal(f.trace.includes("ack"), false);
		assert.equal(f.trace.filter((entry) => entry === "send").length, 0);
		assert.ok(f.reports.some((event) => event.kind === "receive-held"));
		f.bridge.stop();
	});
}

test("acceptance is persisted before tuple-bound acknowledgement, not model completion", async () => {
	const f = fixture();
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.deepEqual(f.trace.slice(0, 8), [
		"subscribe",
		"register",
		"claim",
		"identity",
		"intent",
		"send",
		"persist",
		"ack",
	]);
	assert.equal(f.records.get(claim.message.msg_id).nativeId, "native-message-137");
	assert.ok(
		f.reports.some(
			(e) => e.kind === "inbox-acknowledged" && e.grade === "native-consumed-not-model-complete",
		),
	);
});

// Plan 164 S7: a sender forwarded from a paired machine is never shown as a local seat.
for (const [machine, sender] of [
	["laptop", "pij-peer@laptop"],
	[undefined, "pij-peer"],
]) {
	test(`native prompt names the sender as ${sender} and journals its machine`, async () => {
		const f = fixture();
		const forwarded = {
			...claim,
			message: { ...claim.message, ...(machine === undefined ? {} : { from_machine: machine }) },
		};
		const request = f.client.request.bind(f.client);
		f.client.request = async (path, body, signal) => {
			const result = await request(path, body, signal);
			return path.startsWith("/v1/inbox?")
				? result.map((c) => (c === claim ? forwarded : c))
				: result;
		};
		const prompts = [];
		const send = f.native.send;
		f.native.send = async (input) => {
			prompts.push(input.prompt);
			return send(input);
		};
		const run = f.bridge.run();
		await f.accepted.promise;
		f.bridge.stop();
		await run;
		assert.deepEqual(prompts, [
			`[pij from ${JSON.stringify(sender)}; msg_id=${JSON.stringify(claim.message.msg_id)}]\n${claim.message.body}`,
		]);
		assert.equal(f.records.get(claim.message.msg_id).message.from_machine, machine);
	});
}

// Plan 164 F02: identity is (origin machine, msg_id); a forwarded `X` is not the local `X`.
test("real journal keeps a local and a forwarded message with the same msg_id apart", async (t) => {
	const home = await mkdtemp(join(tmpdir(), "pij-native-origin-"));
	t.after(() => rm(home, { recursive: true, force: true }));
	const journal = new FileJournal(home, registration);
	const drained = deferred();
	const reports = [];
	const f = fixture({
		journal,
		delay: () => flush(),
		report: (event) => {
			reports.push(event);
			if (event.kind === "receive-held") drained.resolve();
		},
	});
	t.after(() => f.bridge.stop());
	const local = { ...claim, job_id: 1 };
	const forwarded = {
		...claim,
		job_id: 2,
		message: { ...claim.message, from_machine: "laptop", body: "from the laptop" },
	};
	const repeat = { ...forwarded, job_id: 3 };
	const queue = [local, forwarded, repeat];
	f.client.claimInbox = async (_consumer, signal) => {
		const next = queue.shift();
		if (next) return { claims: [next], hold: null };
		drained.resolve();
		return new Promise((resolve) =>
			signal.addEventListener("abort", () => resolve({ claims: [], hold: null }), { once: true }),
		);
	};
	const acks = [];
	const request = f.client.request;
	f.client.request = async (path, body, signal) => {
		if (path !== "/v1/inbox/ack") return request(path, body, signal);
		acks.push(body.job_id);
		return body.job_id;
	};
	const prompts = [];
	f.native.send = async (input) => {
		const n = prompts.push(input.prompt);
		f.history.push(
			{ type: "user.message", id: `user-${n}`, data: { messageId: `native-${n}` } },
			{ type: "assistant.message", id: `answer-${n}`, parentId: `user-${n}`, data: {} },
			{ type: "assistant.turn_end", id: `end-${n}`, parentId: `answer-${n}`, data: {} },
		);
		return `native-${n}`;
	};
	const run = f.bridge.run();
	await drained.promise;
	f.bridge.stop();
	await run;
	assert.deepEqual(
		reports.filter((event) => event.kind === "receive-held").map((event) => event.diagnostic),
		[],
	);
	assert.deepEqual(prompts, [
		`[pij from "pij-peer"; msg_id="message-137"]\n${claim.message.body}`,
		`[pij from "pij-peer@laptop"; msg_id="message-137"]\nfrom the laptop`,
	]);
	assert.deepEqual(acks, [1, 2, 3], "the exact forwarded repeat is the same message");
	assert.equal((await journal.load(local.message)).nativeId, "native-1");
	assert.equal((await journal.load(forwarded.message)).nativeId, "native-2");
	// A local record stays where it was before origin keys existed.
	const legacyName = `${createHash("sha256").update(claim.message.msg_id).digest("hex")}.json`;
	assert.equal(journal.path(local.message), join(journal.directory, legacyName));
	assert.ok(journal.path(forwarded.message).startsWith(join(journal.directory, "peers")));
});

test("busy native turn consumes immediate steering and journals its returned message ID", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	f.history.push(
		{ type: "user.message", id: "human-user", data: { messageId: "human-prompt" } },
		{ type: "assistant.turn_start", id: "human-turn", parentId: "human-user", data: {} },
	);
	f.native.send = async ({ mode }) => {
		const nativeId = "busy-steering-message";
		// Model the native busy-turn contract: enqueue accepts without consuming yet.
		if (mode === "immediate") {
			f.history.push({
				type: "user.message",
				id: "steering-user",
				parentId: "human-turn",
				data: { messageId: nativeId, delivery: "steering" },
			});
		}
		return nativeId;
	};
	const run = f.bridge.run();
	await flush();
	assert.equal(f.history.at(-1).data.delivery, "steering");
	assert.equal(f.records.get(claim.message.msg_id).nativeId, "busy-steering-message");
	assert.ok(
		f.reports.some(
			(event) =>
				event.kind === "inbox-acknowledged" &&
				event.nativeMessageId === "busy-steering-message" &&
				event.grade === "native-consumed-not-model-complete",
		),
	);
	assert.equal(
		f.reports.some((event) => event.kind === "native-completed"),
		false,
	);
	assert.equal(f.trace.filter((step) => step === "claim").length, 1);
	nativeTerminal(f, "human-turn");
	await flush();
	assert.equal(f.trace.filter((step) => step === "claim").length, 1);
	nativeTerminal(f, "steering-user");
	await flush();
	assert.equal(f.trace.filter((step) => step === "claim").length, 2);
	f.bridge.stop();
	await run;
});

test("accepted duplicate after restart only acknowledges, never reinjects", async () => {
	const f = fixture();
	f.client.nativeSnapshot = async () =>
		assert.fail("Accepted duplicates do not authorize a new injection");
	f.records.set(claim.message.msg_id, {
		state: "accepted",
		message: claim.message,
		nativeId: "already-native",
	});
	f.history.push({
		type: "user.message",
		id: "already-user",
		data: { messageId: "already-native" },
	});
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), true);
});

for (const [name, change] of [
	[
		"ambiguous prior send",
		(f) => f.records.set(claim.message.msg_id, { state: "pending", message: claim.message }),
	],
	[
		"recipient mismatch",
		(f) => {
			const request = f.client.request;
			f.client.request = (p, b, s) =>
				p.startsWith("/v1/inbox?")
					? [{ ...claim, message: { ...claim.message, to: "someone-else" } }]
					: request(p, b, s);
		},
	],
	[
		"unsupported command",
		(f) => {
			const request = f.client.request;
			f.client.request = (p, b, s) =>
				p.startsWith("/v1/inbox?")
					? [{ ...claim, message: { ...claim.message, command: "new" } }]
					: request(p, b, s);
		},
	],
	[
		"multiple claims",
		(f) => {
			const request = f.client.request;
			f.client.request = (p, b, s) =>
				p.startsWith("/v1/inbox?") ? [claim, claim] : request(p, b, s);
		},
	],
	[
		"storage failure",
		(f) => {
			f.journal.begin = async () => {
				throw new Error("disk full");
			};
		},
	],
	[
		"duplicate id with changed body",
		(f) =>
			f.records.set(claim.message.msg_id, {
				state: "accepted",
				message: { ...claim.message, body: "other" },
				nativeId: "other-native",
			}),
	],
]) {
	test(`${name} holds without native send or ack; outgoing remains available`, async () => {
		const f = fixture();
		change(f);
		await f.bridge.run();
		assert.equal(f.trace.includes("send"), false);
		assert.equal(f.trace.includes("ack"), false);
		assert.ok(f.reports.some((e) => e.kind === "receive-held"));
		const result = await f.bridge.send({ to: "pij-peer", message: "diagnostic" });
		assert.equal(result.ok, true);
		f.bridge.stop();
	});
}

test("send rejection is ambiguous and never acknowledged or retried", async () => {
	const f = fixture();
	f.native.send = async () => {
		f.trace.push("send");
		throw new Error("connection lost after enqueue");
	};
	await f.bridge.run();
	assert.equal(f.trace.filter((x) => x === "send").length, 1);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.records.get(claim.message.msg_id).state, "pending");
	f.bridge.stop();
});

test("native acceptance followed by failed persistence stays unacknowledged", async () => {
	const f = fixture();
	f.journal.accept = async () => {
		throw new Error("disk full");
	};
	await f.bridge.run();
	assert.equal(f.trace.includes("send"), true);
	assert.equal(f.trace.includes("ack"), false);
	f.bridge.stop();
});

test("late native completion after stop cannot acknowledge another generation", async () => {
	const f = fixture();
	const sending = deferred();
	const completion = deferred();
	f.native.send = () => {
		sending.resolve();
		return completion.promise;
	};
	const run = f.bridge.run();
	await sending.promise;
	f.bridge.stop();
	await run;
	completion.resolve("late-native");
	await Promise.resolve();
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.records.get(claim.message.msg_id).state, "pending");
	assert.equal((await f.bridge.send({ to: "pij-peer", message: "late" })).ok, false);
});

test("two run calls share a single consumer; buffered events never forward replies", async () => {
	const f = fixture();
	const first = f.bridge.run();
	assert.equal(first, f.bridge.run());
	await f.accepted.promise;
	for (const type of ["user.message", "assistant.message", "session.idle", "session.error"])
		f.event({ type, data: { messageId: "buffered", content: "reply" } });
	assert.equal(f.trace.includes("outgoing"), false);
	f.bridge.stop();
	await first;
});

test("outgoing tool ignores spoofed from, mints unique ids and returns queue grade", async () => {
	const f = fixture();
	const run = f.bridge.run();
	await f.accepted.promise;
	const sent = [];
	const request = f.client.request;
	f.client.request = (path, body, signal) => {
		if (path === "/v1/send") sent.push(body);
		return request(path, body, signal);
	};
	for (let i = 0; i < 2; i++)
		assert.equal(
			(await f.bridge.send({ to: "pij-peer", message: "reply", from: "forged" })).ok,
			true,
		);
	assert.equal(sent[0].from, registration.id);
	assert.equal(sent[0].body, "reply");
	assert.notEqual(sent[0].msg_id, sent[1].msg_id);
	f.bridge.stop();
	await run;
});

// Plan 164 F01b: `seat@alias` reaches the paired machine; the grammar is the shared Rust golden.
test("outgoing tool parses every golden address into the daemon destination", async (t) => {
	const cases = JSON.parse(
		await readFile(
			new URL("../../../crates/testkit/fixtures/golden/address/cases.json", import.meta.url),
			"utf8",
		),
	);
	const f = fixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await f.accepted.promise;
	const sent = [];
	const request = f.client.request;
	f.client.request = (path, body, signal) => {
		if (path === "/v1/send") sent.push(body);
		return request(path, body, signal);
	};
	for (const c of cases) {
		sent.length = 0;
		const result = await f.bridge.send({ to: c.input, message: "reply" });
		if (c.error) {
			assert.equal(result.ok, false, c.input);
			assert.deepEqual(sent, [], c.input);
		} else {
			assert.equal(result.ok, true, c.input);
			assert.deepEqual(
				sent.map((body) => body.to),
				[c.machine === null ? { seat: c.seat } : { seat: c.seat, machine: c.machine }],
				c.input,
			);
		}
	}
	f.bridge.stop();
	await run;
});

test("activity: foreground turns publish working then idle in order; failures never break the turn", async () => {
	const f = fixture();
	const run = f.bridge.run();
	await f.accepted.promise;
	const published = [];
	const gates = [];
	const request = f.client.request;
	f.client.request = (path, body, signal) => {
		if (path !== "/v1/activity") return request(path, body, signal);
		assert.deepEqual(Object.keys(body).sort(), ["native_session", "seat", "state"]);
		assert.equal(body.seat, registration.id);
		assert.equal(body.native_session, registration.harness_session);
		published.push(body.state);
		const gate = deferred();
		gates.push(gate);
		return gate.promise.then((outcome) => {
			if (outcome instanceof Error) throw outcome;
			return { seat: body.seat, state: body.state, changed: true };
		});
	};
	const httpError = (status) => Object.assign(new Error(`Pij HTTP ${status}`), { status });

	f.event({ type: "assistant.turn_start", id: "turn-1", data: {} });
	f.event({ type: "assistant.turn_end", id: "sub-end", agentId: "sub-agent-9", data: {} });
	f.event({ type: "assistant.turn_end", id: "turn-1-end", data: {} });
	await flush();
	assert.deepEqual(published, ["working"], "idle waits for the in-flight working publication");
	gates[0].resolve("ok");
	await flush();
	assert.deepEqual(published, ["working", "idle"]);
	f.event({ type: "session.idle", id: "idle-1", data: {} });
	await flush();
	assert.deepEqual(published, ["working", "idle"], "an unchanged state is not republished");

	gates[1].resolve(httpError(500));
	await flush();
	assert.deepEqual(
		f.reports.filter((event) => event.kind === "activity-unpublished").map((e) => e.state),
		["idle"],
	);
	assert.equal((await f.bridge.send({ to: "pij-peer", message: "still usable" })).ok, true);

	// An older daemon without the route stops publication for this runtime.
	f.event({ type: "assistant.turn_start", id: "turn-2", data: {} });
	await flush();
	gates[2].resolve(httpError(404));
	await flush();
	f.event({ type: "assistant.turn_end", id: "turn-2-end", data: {} });
	f.event({ type: "assistant.turn_start", id: "turn-3", data: {} });
	await flush();
	assert.deepEqual(published, ["working", "idle", "working"]);
	f.bridge.stop();
	await run;
});

/** A running bridge whose /v1/activity requests are recorded; stopped after the test. */
async function activityBridge(t, answer = () => ({ changed: true })) {
	const f = fixture();
	const run = f.bridge.run();
	t.after(() => {
		f.bridge.stop();
		return run;
	});
	await f.accepted.promise;
	const published = [];
	const request = f.client.request;
	f.client.request = async (path, body, signal) => {
		if (path !== "/v1/activity") return request(path, body, signal);
		published.push({ state: body.state, signal });
		const outcome = answer(body);
		if (outcome instanceof Error) throw outcome;
		return { seat: body.seat, state: body.state, ...outcome };
	};
	return { f, published, states: () => published.map((entry) => entry.state) };
}

test("activity: sub-agent turns never flip the seat's state", async (t) => {
	const { f, states } = await activityBridge(t);
	f.event({ type: "assistant.turn_start", id: "sub-start", agentId: "sub-agent-9", data: {} });
	f.event({ type: "assistant.turn_end", id: "sub-end", agentId: "sub-agent-9", data: {} });
	f.event({ type: "assistant.turn_start", id: "turn-1", data: {} });
	await flush();
	assert.deepEqual(states(), ["working"]);
});

test("activity: session.idle settles a working seat to idle", async (t) => {
	const { f, states } = await activityBridge(t);
	f.event({ type: "assistant.turn_start", id: "turn-1", data: {} });
	f.event({ type: "session.idle", id: "idle-1", data: {} });
	await flush();
	assert.deepEqual(states(), ["working", "idle"]);
});

test("activity: stop settles a seat left working to idle once, outside the stopped signal", async (t) => {
	const { f, published, states } = await activityBridge(t);
	f.event({ type: "assistant.turn_start", id: "turn-1", data: {} });
	await flush();
	await f.bridge.stop("session.shutdown");
	assert.deepEqual(states(), ["working", "idle"]);
	assert.equal(published[1].signal.aborted, false, "the idle request outlives the stop abort");
	await f.bridge.stop("again");
	assert.equal(published.length, 2);
});

test("activity: stop publishes nothing for a seat already idle", async (t) => {
	const { f, states } = await activityBridge(t);
	f.event({ type: "assistant.turn_start", id: "turn-1", data: {} });
	f.event({ type: "assistant.turn_end", id: "turn-1-end", data: {} });
	await flush();
	await f.bridge.stop();
	assert.deepEqual(states(), ["working", "idle"]);
});

test("activity: a failed stop-time idle is reported and never throws", async (t) => {
	const { f, states } = await activityBridge(t, (body) =>
		body.state === "idle" ? new Error("Pij transport unavailable") : { changed: true },
	);
	f.event({ type: "assistant.turn_start", id: "turn-1", data: {} });
	await flush();
	assert.equal(await f.bridge.stop(), undefined);
	assert.deepEqual(states(), ["working", "idle"]);
	assert.deepEqual(
		f.reports.filter((event) => event.kind === "activity-unpublished").map((e) => e.state),
		["idle"],
	);
});

test("fyi claim: count gates the verbatim block; empty and failed claims add nothing", async () => {
	const golden = await readFile(
		new URL("../../../crates/testkit/fixtures/golden/fyi/block.txt", import.meta.url),
		"utf8",
	);
	const f = fixture();
	const run = f.bridge.run();
	await f.accepted.promise;
	const answers = [
		{ seat: registration.id, count: 2, block: golden, ids: ["fyi-1", "fyi-2"] },
		{ seat: registration.id, count: 0, block: "", ids: [] },
		new Error("Pij transport unavailable"),
	];
	const bodies = [];
	const request = f.client.request;
	f.client.request = async (path, body, signal) => {
		if (path !== "/v1/fyi/claim") return request(path, body, signal);
		bodies.push(body);
		const answer = answers.shift();
		if (answer instanceof Error) throw answer;
		return answer;
	};

	assert.deepEqual(await f.bridge.claimFyis(), { additionalContext: golden });
	assert.equal(await f.bridge.claimFyis(), undefined);
	assert.equal(await f.bridge.claimFyis(), undefined);
	assert.deepEqual(bodies[0], {
		seat: registration.id,
		native_session: registration.harness_session,
		via: "hook:copilot",
	});
	f.bridge.stop();
	await run;
	assert.equal(await f.bridge.claimFyis(), undefined);
	assert.equal(bodies.length, 3, "a stopped bridge claims nothing");
});

for (const patch of [
	{ native_extension_delivery: false },
	{ harness: "omp" },
	{ id: "another" },
	{ session: "another-native" },
	{ proc: { pid: 138, proc_start: registration.proc_start } },
	{ pane: "%138" },
]) {
	test(`registration readback refuses ${JSON.stringify(patch)}`, async () => {
		const f = fixture();
		f.client.request = async () => ({ ...descriptor, ...patch });
		await f.bridge.run();
		assert.equal(f.trace.includes("claim"), false);
		assert.equal((await f.bridge.send({ to: "pij-peer", message: "not bound" })).ok, false);
		assert.ok(f.reports.some((e) => e.kind === "receive-held"));
		f.bridge.stop();
	});
}

test("registration tolerates additive plan135 binding metadata", async () => {
	const f = fixture();
	const request = f.client.request;
	f.client.request = (p, b, s) =>
		p === "/v1/register"
			? { ...descriptor, binding: { unexpected_future: true } }
			: request(p, b, s);
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
});

test("manual identity requests daemon allocation; exact process rollover supersedes predecessor", () => {
	const input = {
		sessionId: registration.harness_session,
		host: { pid: registration.pid, proc_start: registration.proc_start, pane: registration.pane },
		folder: registration.folder,
		env: {},
	};
	const fresh = chooseRegistration({ ...input, seats: [] });
	assert.equal(fresh.id, "");
	assert.equal(
		fresh.id,
		chooseRegistration({
			...input,
			host: { ...input.host, pid: 999, proc_start: input.host.proc_start + 100 },
			seats: [],
		}).id,
	);
	assert.equal(chooseRegistration({ ...input, seats: [descriptor] }).id, registration.id);
	const rolled = chooseRegistration({ ...input, sessionId: "native-next", seats: [descriptor] });
	assert.equal(rolled.supersedes, registration.id);
	assert.notEqual(rolled.id, registration.id);
	assert.throws(
		() =>
			chooseRegistration({
				...input,
				seats: [
					{
						...descriptor,
						session: "different-native-session",
						native_extension_delivery: false,
						proc: { ...descriptor.proc, pid: 999 },
					},
				],
			}),
		/identity|pane|process/i,
	);
	assert.throws(
		() => chooseRegistration({ ...input, seats: [], env: { PIJ_SESSION_ID: "unproven" } }),
		/prebind|identity|corroborat/i,
	);
});

test("spawn binding requires exact process, pane and correlation", () => {
	const input = {
		sessionId: registration.harness_session,
		host: { pid: registration.pid, proc_start: registration.proc_start, pane: registration.pane },
		folder: registration.folder,
		env: { PIJ_SESSION_ID: registration.id, PIJ_SPAWN_ID: "spawn-137" },
	};
	assert.equal(
		chooseRegistration({
			...input,
			seats: [
				{
					...descriptor,
					proc: null,
					session: null,
					spawn_id: "spawn-137",
					native_extension_delivery: false,
				},
			],
		}).id,
		registration.id,
	);
	assert.throws(
		() => chooseRegistration({ ...input, seats: [{ ...descriptor, spawn_id: "another" }] }),
		/spawn|identity|corroborat/i,
	);
});

test("cold native resume nominates a unique attested address without trusting roster state", () => {
	const host = {
		...descriptor.proc,
		pid: 999,
		proc_start: descriptor.proc.proc_start + 100,
		pane: "%999",
	};
	for (const state of ["working", "dead"]) {
		const selected = chooseRegistration({
			sessionId: descriptor.session,
			host,
			folder: "/resumed",
			env: {},
			seats: [{ ...descriptor, state }],
		});
		assert.equal(selected.id, descriptor.id);
		assert.equal(selected.pid, host.pid);
		assert.equal(selected.proc_start, host.proc_start);
		assert.equal(selected.pane, host.pane);
		assert.equal(selected.supersedes, undefined);
	}
});

for (const [label, change] of [
	["tombstoned", { tombstoned_at: 1, state: "tombstoned" }],
	["native delivery withdrawn", { native_extension_delivery: false }],
]) {
	test(`same native session nominates its durable address when ${label}`, () => {
		const selected = chooseRegistration({
			sessionId: descriptor.session,
			host: { pid: 999, proc_start: descriptor.proc.proc_start + 100, pane: descriptor.pane },
			folder: "/resumed",
			env: {},
			seats: [{ ...descriptor, ...change }],
		});
		assert.equal(selected.id, descriptor.id);
		assert.equal(selected.supersedes, undefined);
	});
}

test("two live native addresses remain retryable despite tombstoned history", () => {
	const seats = [
		{ ...descriptor, id: "pij-history", tombstoned_at: 1 },
		descriptor,
		{ ...descriptor, id: "pij-second-live", state: "dead" },
	];
	assert.throws(
		() =>
			chooseRegistration({
				sessionId: descriptor.session,
				host: { ...descriptor.proc, pid: 999 },
				folder: "/work",
				env: {},
				seats,
			}),
		(error) => /multiple candidate/.test(error.message) && error.retryable === true,
	);
});

test("live native address outranks newer tombstones in adversarial alphabetical order", () => {
	const live = { ...descriptor, id: "pij-z-live", parent: "pij-live-parent" };
	const selected = chooseRegistration({
		sessionId: descriptor.session,
		host: { pid: 999, proc_start: descriptor.proc.proc_start + 300, pane: "%new" },
		folder: "/resumed",
		env: {},
		seats: [
			{
				...descriptor,
				id: "pij-a-retired",
				proc: { ...descriptor.proc, proc_start: descriptor.proc.proc_start + 200 },
				tombstoned_at: 2,
			},
			{ ...descriptor, id: "pij-b-retired", tombstoned_at: 1 },
			live,
		],
	});
	assert.equal(selected.id, live.id);
	assert.equal(selected.parent, live.parent);
	assert.equal(selected.supersedes, undefined);
});

test("all-tombstoned native addresses select newest process with missing starts last and id tie break", () => {
	const retired = { ...descriptor, tombstoned_at: 1, native_extension_delivery: false };
	const newest = {
		...retired,
		id: "pij-y-newest",
		proc: { ...descriptor.proc, proc_start: descriptor.proc.proc_start + 100 },
	};
	const seats = [
		{ ...retired, id: "pij-a-missing", proc: null },
		{ ...retired, id: "pij-b-older" },
		{ ...newest, id: "pij-z-tied" },
		newest,
	];
	for (const roster of [seats, [...seats].reverse()]) {
		const selected = chooseRegistration({
			sessionId: descriptor.session,
			host: { pid: 999, proc_start: descriptor.proc.proc_start + 200 },
			folder: "/resumed",
			env: {},
			seats: roster,
		});
		assert.equal(selected.id, newest.id);
		assert.equal(selected.supersedes, undefined);
	}
});

test("same-session nomination leaves missing historical process verification to the daemon", () => {
	const selected = chooseRegistration({
		sessionId: descriptor.session,
		host: { pid: 999, proc_start: descriptor.proc.proc_start + 100 },
		folder: "/resumed",
		env: {},
		seats: [{ ...descriptor, proc: null, tombstoned_at: 1 }],
	});
	assert.equal(selected.id, descriptor.id);
	assert.equal(selected.supersedes, undefined);
});

test("foreign harness cannot nominate a native session or yield its occupied pane", () => {
	const input = {
		sessionId: descriptor.session,
		host: { ...descriptor.proc, pid: 999 },
		folder: "/work",
		env: {},
		seats: [{ ...descriptor, harness: "pi" }],
	};
	assert.equal(chooseRegistration(input).id, "");
	assert.throws(
		() => chooseRegistration({ ...input, host: { ...input.host, pane: descriptor.pane } }),
		/Pane belongs/,
	);
});

test("same-process A to B to A restores the retired native address and accepted journal", async (t) => {
	const home = await mkdtemp(join(tmpdir(), "pij-native-retired-"));
	t.after(() => rm(home, { recursive: true, force: true }));
	const input = {
		sessionId: "native-A",
		host: { ...descriptor.proc, pane: descriptor.pane },
		folder: "/work",
		env: {},
	};
	const allocate = (request, id) =>
		resolveRegistration(
			{
				request: async () => ({
					...descriptor,
					id,
					session: request.harness_session,
					proc: { pid: request.pid, proc_start: request.proc_start },
					pane: request.pane,
				}),
			},
			request,
			new AbortController().signal,
		);
	const first = await allocate(chooseRegistration({ ...input, seats: [] }), "pij-bright-otter");
	const firstSeat = { ...descriptor, id: first.id, session: first.harness_session };
	const message = { ...claim.message, to: first.id };
	const oldJournal = new FileJournal(home, first);
	assert.equal(await oldJournal.begin(message), true);
	await oldJournal.accept(message, "old-acceptance");
	const nextRequest = chooseRegistration({ ...input, sessionId: "native-B", seats: [firstSeat] });
	assert.equal(nextRequest.supersedes, first.id);
	const next = await allocate(nextRequest, "pij-calm-badger");
	const retired = {
		...firstSeat,
		tombstoned_at: 1,
		native_extension_delivery: false,
		state: "tombstoned",
	};
	const current = { ...descriptor, id: next.id, session: next.harness_session };
	const reopenedRequest = chooseRegistration({ ...input, seats: [retired, current] });
	assert.equal(reopenedRequest.id, first.id);
	assert.equal(reopenedRequest.supersedes, undefined);
	const reopened = await resolveRegistration(
		{
			request: async () => assert.fail("Durable native identity must not request a fresh address"),
		},
		reopenedRequest,
		new AbortController().signal,
	);
	assert.notEqual(reopened.id, next.id);
	assert.equal((await new FileJournal(home, reopened).load(message)).nativeId, "old-acceptance");
	assert.equal((await oldJournal.load(message)).nativeId, "old-acceptance");
	const reopenedSeat = { ...firstSeat, id: reopened.id };
	assert.equal(
		chooseRegistration({
			...input,
			host: { ...input.host, pid: 999 },
			seats: [reopenedSeat, current],
		}).id,
		reopened.id,
	);
	const coldReopen = chooseRegistration({
		...input,
		host: { ...input.host, pid: 999 },
		seats: [retired],
	});
	assert.equal(coldReopen.id, retired.id);
	assert.equal(coldReopen.supersedes, undefined);
	assert.throws(
		() =>
			chooseRegistration({
				...input,
				seats: [retired],
				env: { PIJ_SESSION_ID: retired.id, PIJ_SPAWN_ID: "retired-launch" },
			}),
		(error) => error.retryable === false,
	);
	assert.equal(
		chooseRegistration({ ...input, seats: [{ ...retired, native_extension_delivery: true }] }).id,
		retired.id,
	);
});

test("fresh native conversation nominates new address without retiring old pane owner locally", () => {
	const host = {
		...descriptor.proc,
		pid: 999,
		proc_start: descriptor.proc.proc_start + 100,
		pane: descriptor.pane,
	};
	for (const state of ["working", "dead", "tombstoned"]) {
		const old = {
			...descriptor,
			state,
			native_extension_delivery: state !== "tombstoned",
			tombstoned_at: state === "tombstoned" ? 1 : null,
		};
		const selected = chooseRegistration({
			sessionId: "fresh-native",
			host,
			folder: "/work",
			env: {},
			seats: [old],
		});
		assert.notEqual(selected.id, old.id);
		assert.equal(selected.supersedes, undefined);
		assert.equal(selected.harness_session, "fresh-native");
		assert.equal(old.state, state);
		assert.equal(old.session, descriptor.session);
		assert.deepEqual(old.proc, descriptor.proc);
	}
});

test("verified spawn allocation retains its id and parent over a correlated session address", () => {
	const host = { ...descriptor.proc, pid: 999, pane: descriptor.pane };
	const candidate = { ...descriptor, spawn_id: "spawn-137", parent: "pij-parent" };
	const prebind = {
		...candidate,
		id: "pij-preallocated",
		parent: "pij-allocated-parent",
		session: null,
		proc: null,
		native_extension_delivery: false,
	};
	const input = {
		sessionId: descriptor.session,
		host,
		folder: "/work",
		seats: [candidate, prebind],
	};
	const selected = chooseRegistration({
		...input,
		env: { PIJ_SESSION_ID: prebind.id, PIJ_SPAWN_ID: candidate.spawn_id },
	});
	assert.equal(selected.id, prebind.id);
	assert.equal(selected.supersedes, undefined);
	assert.equal(selected.parent, prebind.parent);
});

test("fresh verified spawn keeps its allocated identity and journal separate from the saved session", async (t) => {
	const home = await mkdtemp(join(tmpdir(), "pij-native-prebind-"));
	t.after(() => rm(home, { recursive: true, force: true }));
	const saved = {
		...descriptor,
		pane: "%old",
		spawn_id: "old-spawn",
		parent: "pij-original-parent",
	};
	const prebind = {
		id: "pij-new-prebind",
		harness: "copilot",
		session: null,
		proc: null,
		pane: "%new",
		spawn_id: "new-spawn",
		parent: "pij-new-parent",
		native_extension_delivery: false,
	};
	const savedJournal = new FileJournal(home, { ...registration, id: saved.id });
	const savedMessage = { ...claim.message, to: saved.id };
	assert.equal(await savedJournal.begin(savedMessage), true);
	await savedJournal.accept(savedMessage, "saved-acceptance");
	const selected = chooseRegistration({
		sessionId: saved.session,
		host: { pid: 999, proc_start: 200, pane: prebind.pane },
		folder: "/work",
		seats: [saved, prebind],
		env: { PIJ_SESSION_ID: prebind.id, PIJ_SPAWN_ID: prebind.spawn_id },
	});
	assert.equal(selected.id, prebind.id);
	assert.equal(selected.spawn_id, prebind.spawn_id);
	assert.equal(selected.parent, prebind.parent);
	assert.equal(selected.supersedes, undefined);
	const spawnedJournal = new FileJournal(home, selected);
	assert.equal(await spawnedJournal.load(savedMessage), undefined);
	const spawnedMessage = { ...savedMessage, to: prebind.id };
	assert.equal(await spawnedJournal.begin(spawnedMessage), true);
	await spawnedJournal.accept(spawnedMessage, "spawned-acceptance");
	assert.equal((await savedJournal.load(savedMessage)).nativeId, "saved-acceptance");
	assert.equal((await spawnedJournal.load(spawnedMessage)).nativeId, "spawned-acceptance");
});

test("verified spawn with a compatible saved session retains the allocated parent", () => {
	const saved = { ...descriptor, parent: "pij-saved-parent" };
	const prebind = {
		...saved,
		id: "pij-preallocated",
		proc: null,
		spawn_id: "spawn-137",
		parent: "pij-allocated-parent",
		native_extension_delivery: false,
	};
	const selected = chooseRegistration({
		sessionId: saved.session,
		host: { pid: 999, proc_start: descriptor.proc.proc_start + 100, pane: prebind.pane },
		folder: "/work",
		env: { PIJ_SESSION_ID: prebind.id, PIJ_SPAWN_ID: prebind.spawn_id },
		seats: [saved, prebind],
	});
	assert.equal(selected.id, prebind.id);
	assert.equal(selected.parent, prebind.parent);
	assert.equal(selected.supersedes, undefined);
});

for (const [label, change] of [
	["relay allocation", { relay: true }],
	["different native session", { session: "another-native-session" }],
	["tombstoned allocation", { tombstoned_at: 1 }],
	["different pane", { pane: "%other" }],
	["bound process", { proc: { pid: 500, proc_start: 100 } }],
	["foreign harness", { harness: "pi" }],
]) {
	test(`spawn prebind cannot bypass ownership for ${label}`, () => {
		const saved = { ...descriptor, pane: "%old", spawn_id: "old-spawn" };
		const prebind = {
			id: "pij-preallocated",
			harness: "copilot",
			proc: null,
			session: null,
			pane: "%new",
			spawn_id: "new-spawn",
			...change,
		};
		assert.throws(
			() =>
				chooseRegistration({
					sessionId: saved.session,
					host: { pid: 999, proc_start: descriptor.proc.proc_start + 100, pane: "%new" },
					folder: "/work",
					env: { PIJ_SESSION_ID: prebind.id, PIJ_SPAWN_ID: prebind.spawn_id },
					seats: [saved, prebind],
				}),
			(error) => error.retryable === false,
		);
	});
}

test("same-process native successor reload corroborates immutable launch id by spawn correlation", () => {
	const current = { ...descriptor, spawn_id: "spawn-137", parent: "pij-parent" };
	const selected = chooseRegistration({
		sessionId: current.session,
		host: { ...current.proc, pane: current.pane },
		folder: "/work",
		env: { PIJ_SESSION_ID: "pij-bootstrap-address", PIJ_SPAWN_ID: current.spawn_id },
		seats: [current],
	});
	assert.equal(selected.id, current.id);
	assert.equal(selected.spawn_id, current.spawn_id);
	assert.equal(selected.parent, current.parent);
});

test("saved native session alone cannot corroborate a wrong requested id with matching spawn", () => {
	const saved = { ...descriptor, spawn_id: "spawn-137" };
	assert.throws(
		() =>
			chooseRegistration({
				sessionId: saved.session,
				host: { pid: 999, proc_start: descriptor.proc.proc_start + 100 },
				folder: "/work",
				env: { PIJ_SESSION_ID: "pij-incorrect-request", PIJ_SPAWN_ID: saved.spawn_id },
				seats: [saved],
			}),
		(error) => /not corroborated/.test(error.message) && error.retryable === false,
	);
});

test("native session resolution precedes stale pane metadata and duplicate process candidates", () => {
	const input = {
		sessionId: descriptor.session,
		host: { ...descriptor.proc, pane: "%moved" },
		folder: "/work",
		env: {},
	};
	const selected = chooseRegistration({ ...input, seats: [descriptor] });
	assert.equal(selected.id, descriptor.id);
	assert.equal(selected.pane, input.host.pane);
	assert.throws(
		() => chooseRegistration({ ...input, seats: [descriptor, { ...descriptor, id: "duplicate" }] }),
		(error) => error.retryable === true,
	);
});

test("two live same-session addresses cannot hide behind an exact process or verified prebind", () => {
	const duplicate = {
		...descriptor,
		id: "pij-other-live",
		proc: { pid: 999, proc_start: descriptor.proc.proc_start + 100 },
	};
	const prebind = {
		id: "pij-preallocated",
		harness: "copilot",
		proc: null,
		pane: descriptor.pane,
		spawn_id: "spawn-137",
	};
	assert.throws(
		() =>
			chooseRegistration({
				sessionId: descriptor.session,
				host: { ...descriptor.proc, pane: descriptor.pane },
				folder: "/work",
				env: { PIJ_SESSION_ID: prebind.id, PIJ_SPAWN_ID: prebind.spawn_id },
				seats: [descriptor, duplicate, prebind],
			}),
		(error) => error.retryable === true,
	);
});

test("same-session nomination cannot corroborate an unrelated requested identity", () => {
	assert.throws(
		() =>
			chooseRegistration({
				sessionId: descriptor.session,
				host: { pid: 999, proc_start: descriptor.proc.proc_start + 100 },
				folder: "/work",
				env: { PIJ_SESSION_ID: "pij-unrelated-prebind" },
				seats: [descriptor],
			}),
		(error) => error.retryable === false,
	);
});

test("exact native process cannot corroborate an incorrect requested id", () => {
	assert.throws(
		() =>
			chooseRegistration({
				sessionId: descriptor.session,
				host: { ...descriptor.proc, pane: descriptor.pane },
				folder: "/work",
				env: { PIJ_SESSION_ID: "pij-incorrect-request" },
				seats: [descriptor],
			}),
		(error) => /not corroborated/.test(error.message) && error.retryable === false,
	);
});

test("correct requested id cannot corroborate an incorrect spawn correlation", () => {
	assert.throws(
		() =>
			chooseRegistration({
				sessionId: descriptor.session,
				host: { pid: 999, proc_start: descriptor.proc.proc_start + 100 },
				folder: "/work",
				env: { PIJ_SESSION_ID: descriptor.id, PIJ_SPAWN_ID: "incorrect-spawn" },
				seats: [{ ...descriptor, spawn_id: "allocated-spawn" }],
			}),
		(error) => /Spawn correlation/.test(error.message) && error.retryable === false,
	);
});

test("process start uses daemon local C-locale packed stamp with calendar checks", () => {
	assert.equal(parseProcessStart("Sat Sep  5 12:00:00 2026"), 20260905120000);
	for (const invalid of ["bad", "Mon Feb 30 12:00:00 2026", "Sat Sep 5 25:00:00 2026"])
		assert.throws(() => parseProcessStart(invalid));
});

test("real filesystem journal survives restart and hashes opaque ids with private modes", async (t) => {
	const home = await mkdtemp(join(tmpdir(), "pij-native-journal-"));
	t.after(() => rm(home, { recursive: true, force: true }));
	const journal = new FileJournal(home, registration);
	const message = { ...claim.message, msg_id: "../../opaque/message" };
	const resumed = {
		...registration,
		pid: registration.pid + 1,
		proc_start: registration.proc_start + 100,
	};
	assert.equal(await journal.begin(message), true);
	assert.equal(await new FileJournal(home, resumed).begin(message), false);
	await journal.accept(message, "native-durable");
	const restarted = new FileJournal(home, resumed);
	assert.equal((await restarted.load(message)).nativeId, "native-durable");
	assert.equal((await stat(journal.directory)).mode & 0o777, 0o700);
	assert.equal((await stat(journal.path(message))).mode & 0o777, 0o600);
	assert.ok(journal.path(message).startsWith(journal.directory));
	const next = new FileJournal(home, { ...registration, harness_session: "different" });
	assert.equal(await next.load(message), undefined);
	assert.equal(await next.begin(message), true);
	const otherSeat = new FileJournal(home, { ...resumed, id: "different-seat" });
	assert.equal(await otherSeat.load(message), undefined);
	assert.match(await readFile(journal.path(message), "utf8"), /native-durable/);
});

test("real chooseRegistration and filesystem preserve accepted-before-ack across changed host", {
	timeout: 2000,
}, async (t) => {
	const home = await mkdtemp(join(tmpdir(), "pij-native-restart-"));
	t.after(() => rm(home, { recursive: true, force: true }));
	const input = {
		sessionId: descriptor.session,
		host: { ...descriptor.proc, pane: descriptor.pane },
		folder: "/work",
		env: {},
	};
	const selected = await resolveRegistration(
		{ request: async () => ({ ...descriptor, id: "pij-bright-otter" }) },
		chooseRegistration({ ...input, seats: [] }),
		new AbortController().signal,
	);
	const registered = { ...descriptor, id: selected.id };
	const queued = { ...claim, message: { ...claim.message, to: selected.id } };
	const journal = new FileJournal(home, selected);
	const first = fixture({ registration: selected, journal });
	t.after(() => first.bridge.stop());
	const request = first.client.request;
	let failedAcks = 0;
	first.client.claimInbox = async (consumer) => {
		assert.deepEqual(consumer, { ...tuple, seat: selected.id });
		return { claims: [queued], hold: null };
	};
	first.client.request = async (path, body, signal) => {
		if (path === "/v1/register") {
			assert.deepEqual(body, selected);
			return registered;
		}
		if (path === "/v1/inbox/ack") {
			failedAcks++;
			assert.equal((await journal.load(claim.message)).state, "accepted");
			first.bridge.stop();
			throw new Error("Process stopped after durable acceptance before daemon acknowledgement");
		}
		return request(path, body, signal);
	};
	await first.bridge.run();
	assert.equal(failedAcks, 1);
	assert.equal(first.trace.filter((step) => step === "release").length, 0);
	assert.equal(first.trace.filter((step) => step === "send").length, 1);
	assert.equal(
		first.reports.some((event) => event.kind === "inbox-acknowledged"),
		false,
	);

	const resumed = chooseRegistration({
		...input,
		host: { ...input.host, pid: selected.pid + 1, proc_start: selected.proc_start + 100 },
		seats: [registered],
	});
	assert.equal(resumed.id, selected.id);
	assert.notEqual(resumed.pid, selected.pid);
	assert.notEqual(resumed.proc_start, selected.proc_start);
	const live = {
		native_session: resumed.harness_session,
		pid: resumed.pid,
		proc_start: resumed.proc_start,
	};
	const second = fixture({ registration: resumed, journal: new FileJournal(home, resumed) });
	t.after(() => second.bridge.stop());
	let claims = 0;
	let sends = 0;
	const acks = [];
	second.client.claimInbox = async (consumer) => {
		claims++;
		assert.deepEqual(consumer, { seat: resumed.id, ...live });
		return { claims: [{ ...queued, native_consumer: live }], hold: null };
	};
	const resumedRequest = second.client.request;
	second.client.request = async (path, body, signal) => {
		if (path === "/v1/register") {
			assert.deepEqual(body, resumed);
			return { ...registered, proc: { pid: resumed.pid, proc_start: resumed.proc_start } };
		}
		if (path === "/v1/inbox/ack") {
			acks.push(body);
			return claim.job_id;
		}
		return resumedRequest(path, body, signal);
	};
	second.native.send = async () => {
		sends++;
		throw new Error("Duplicate native send after process restart");
	};
	second.history.push({
		type: "user.message",
		id: "resumed-user",
		data: { messageId: "native-message-137" },
	});
	const secondRun = second.bridge.run();
	await second.accepted.promise;
	await flush();
	assert.equal(claims, 1);
	assert.equal(sends, 0);
	assert.equal(second.trace.filter((step) => step === "release").length, 0);
	assert.deepEqual(acks, [{ seat: resumed.id, ...live, job_id: claim.job_id }]);
	assert.ok(
		second.reports.some(
			(event) =>
				event.kind === "inbox-acknowledged" && event.grade === "native-consumed-not-model-complete",
		),
	);
	assert.ok(second.reports.some((event) => event.kind === "completion-wait"));
	assert.equal(
		(await second.bridge.send({ to: "pij-peer", message: "restart-held outgoing" })).ok,
		true,
	);
	second.bridge.stop();
	await secondRun;
	const fresh = chooseRegistration({
		...input,
		sessionId: "brand-new-native",
		host: { ...input.host, pid: resumed.pid + 1 },
		seats: [registered],
	});
	assert.notEqual(fresh.id, selected.id);
	assert.equal(await new FileJournal(home, fresh).load(queued.message), undefined);
});

test("HTTP client reloads key on every request, binds tuple, keeps bounded non-JSON diagnostic", async () => {
	let key = "first-secret";
	const headers = [];
	const client = new DaemonClient({
		addr: "127.0.0.1:13700",
		stateDir: "/isolated",
		readKey: async () => key,
		fetch: async (_url, init) => {
			headers.push(init.headers.Authorization);
			return new Response(JSON.stringify({ ok: true, v: 2, data: [] }));
		},
	});
	await client.claimInbox(tuple);
	key = "rotated-secret";
	await client.claimInbox(tuple);
	assert.deepEqual(headers, ["Bearer first-secret", "Bearer rotated-secret"]);
	const broken = new DaemonClient({
		addr: "127.0.0.1:13700",
		stateDir: "/isolated",
		readKey: async () => key,
		fetch: async () => new Response(`not-json ${"x".repeat(10000)}`, { status: 503 }),
	});
	await assert.rejects(
		broken.claimInbox(tuple),
		(error) =>
			error.message.includes("503") &&
			error.message.includes("not-json") &&
			error.message.length < 5000 &&
			!error.message.includes(key),
	);
});

test("only typed HTTP 409 registration refusals permit retry", async () => {
	const refusal = {
		ok: false,
		v: 2,
		command: "pij register",
		error: "refused",
		details: { retryable: true },
		meta: "registration temporarily unavailable",
	};
	for (const [label, path, status, envelope, retryable] of [
		["typed refusal", "/v1/register", 409, refusal, true],
		[
			"legacy HTTP 400 text",
			"/v1/register",
			400,
			{
				...refusal,
				details: undefined,
				meta: "seat id belongs to a different process incarnation; retryable",
			},
			false,
		],
		["wrong HTTP status", "/v1/register", 400, refusal, false],
		["wrong envelope version", "/v1/register", 409, { ...refusal, v: 1 }, false],
		["wrong command", "/v1/register", 409, { ...refusal, command: "pij inbox" }, false],
		["wrong error kind", "/v1/register", 409, { ...refusal, error: "retryable" }, false],
		["security refusal", "/v1/register", 409, { ...refusal, details: { retryable: false } }, false],
		["missing retry authorization", "/v1/register", 409, { ...refusal, details: undefined }, false],
		[
			"untyped retry authorization",
			"/v1/register",
			409,
			{ ...refusal, details: { retryable: "true" } },
			false,
		],
		["not a refusal", "/v1/register", 409, { ...refusal, ok: true }, false],
		["different endpoint", "/v1/send", 409, refusal, false],
	]) {
		const client = new DaemonClient({
			addr: "127.0.0.1:13700",
			stateDir: "/isolated",
			readKey: async () => "fixture-key",
			fetch: async () => new Response(JSON.stringify(envelope), { status }),
		});
		await assert.rejects(client.request(path, registration), (error) => {
			assert.equal(error.retryable, retryable, label);
			return true;
		});
	}
});

async function deadlineHttpFixture(t, handler) {
	const requests = [];
	const server = createServer(async (request, response) => {
		let raw = "";
		for await (const chunk of request) raw += chunk;
		const entry = {
			url: new URL(request.url, "http://localhost"),
			method: request.method,
			authorization: request.headers.authorization,
			body: raw ? JSON.parse(raw) : undefined,
		};
		requests.push(entry);
		if (entry.url.pathname === "/v1/inbox/heartbeat") {
			response.end(
				JSON.stringify({
					ok: true,
					v: 2,
					data: { state: "live", lease_ms: 60000, renew_after_ms: 20000 },
				}),
			);
			return;
		}
		handler(entry, response);
	});
	t.after(
		() =>
			new Promise((resolve) => {
				server.closeAllConnections();
				server.close(resolve);
			}),
	);
	await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
	const client = new DaemonClient({
		addr: `127.0.0.1:${server.address().port}`,
		stateDir: "/isolated",
		readKey: async () => "deadline-key",
		requestTimeoutMs: 150,
	});
	return { client, requests };
}

test("own inbox wait deadlines renew with bounded delay without re-registering or outage logs", async (t) => {
	let polls = 0;
	const { client, requests } = await deadlineHttpFixture(t, ({ url, body }, response) => {
		if (url.pathname === "/v1/register")
			response.end(JSON.stringify({ ok: true, v: 2, data: descriptor }));
		else if (url.pathname === "/v1/inbox") {
			assert.equal(url.searchParams.get("wait"), "true");
			if (++polls === 9) response.end(JSON.stringify({ ok: true, v: 2, data: [claim] }));
		} else if (url.pathname === "/v1/inbox/ack") {
			assert.deepEqual(body, { ...tuple, job_id: claim.job_id });
			response.end(JSON.stringify({ ok: true, v: 2, data: claim.job_id }));
		} else if (url.pathname === "/v1/inbox/typing")
			response.end(JSON.stringify({ ok: true, v: 2, data: observedTyping() }));
		else if (url.pathname === "/v1/send")
			response.end(JSON.stringify({ ok: true, v: 2, data: { msg_id: body.msg_id } }));
		else assert.fail(`Unexpected request ${url.pathname}`);
	});
	const waits = [];
	const f = fixture({
		client,
		delay: async (ms) => {
			waits.push(ms);
			assert.equal(f.bridge.registered, true);
			assert.equal(f.trace.includes("send"), false);
			if (waits.length === 4)
				assert.equal(
					(await f.bridge.send({ to: "pij-peer", message: "still available" })).ok,
					true,
				);
		},
	});
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.deepEqual(waits, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
	assert.equal(polls, 9);
	assert.equal(requests.filter(({ url }) => url.pathname === "/v1/register").length, 1);
	assert.equal(requests.filter(({ url }) => url.pathname === "/v1/inbox/ack").length, 1);
	assert.equal(
		requests.every(({ authorization }) => authorization === "Bearer deadline-key"),
		true,
	);
	assert.equal(f.trace.filter((entry) => entry === "send").length, 1);
	assert.equal(
		f.reports.some(({ kind }) =>
			["reconnecting", "connection-ready", "receive-held", "consent-held"].includes(kind),
		),
		false,
	);
	assert.equal(f.trace.at(-1), "unsubscribe");
});

test("inbox wait renewal does not reset an existing outage episode", async (t) => {
	let polls = 0;
	const { client, requests } = await deadlineHttpFixture(t, ({ url }, response) => {
		if (url.pathname === "/v1/register")
			response.end(JSON.stringify({ ok: true, v: 2, data: descriptor }));
		else if (url.pathname === "/v1/inbox") {
			polls++;
			if (polls === 1 || polls === 3) {
				response.writeHead(401);
				response.end(JSON.stringify({ ok: false, v: 2, error: "unauthorized" }));
			} else if (polls === 4) response.end(JSON.stringify({ ok: true, v: 2, data: [claim] }));
		} else if (url.pathname === "/v1/inbox/typing")
			response.end(JSON.stringify({ ok: true, v: 2, data: observedTyping() }));
		else response.end(JSON.stringify({ ok: true, v: 2, data: claim.job_id }));
	});
	const waits = [];
	const f = fixture({
		client,
		delay: async (ms) => {
			waits.push(ms);
			assert.equal(f.reports.filter(({ kind }) => kind === "connection-ready").length, 0);
		},
	});
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.deepEqual(waits, [250, 500, 1000]);
	assert.equal(requests.filter(({ url }) => url.pathname === "/v1/register").length, 3);
	assert.equal(f.reports.filter(({ kind }) => kind === "reconnecting").length, 1);
	assert.equal(f.reports.filter(({ kind }) => kind === "connection-ready").length, 1);
});

test("HTTP non-wait deadlines, body stalls and connection failures remain retryable", async (t) => {
	let mode = "unanswered";
	const { client } = await deadlineHttpFixture(t, (_entry, response) => {
		if (mode === "disconnect") response.destroy();
		else if (mode === "body") {
			response.writeHead(200, { "Content-Type": "application/json" });
			response.write('{"ok":true,');
		}
	});
	for (const [path, body] of [
		["/v1/register", registration],
		["/v1/send", { to: "pij-peer", body: "test" }],
		["/v1/inbox/ack", { ...tuple, job_id: claim.job_id }],
		["/v1/seats?scope=local", undefined],
		["/v1/inbox?wait=false", undefined],
	])
		await assert.rejects(client.request(path, body), { retryable: true }, path);
	mode = "body";
	await assert.rejects(client.claimInbox(tuple), { retryable: true });
	mode = "disconnect";
	await assert.rejects(client.claimInbox(tuple), { retryable: true });
});

test("foreign TimeoutError remains retryable even after the own inbox deadline", async (t) => {
	const { client } = await deadlineHttpFixture(t, () => {});
	client.fetch = async (url, init) => {
		try {
			return await fetch(url, init);
		} catch {
			throw new DOMException("foreign fetch timeout", "TimeoutError");
		}
	};
	await assert.rejects(client.claimInbox(tuple), { retryable: true });
});

test("external inbox abort is cancellation even with a TimeoutError reason", async (t) => {
	const controller = new AbortController();
	const reason = new DOMException("external cancellation", "TimeoutError");
	const { client, requests } = await deadlineHttpFixture(t, () => controller.abort(reason));
	await assert.rejects(client.claimInbox(tuple, controller.signal), (error) => error === reason);
	assert.equal(requests.length, 1);
});

test("shutdown during inbox renewal delay does not claim or re-register", async (t) => {
	const { client, requests } = await deadlineHttpFixture(t, ({ url }, response) => {
		if (url.pathname === "/v1/register")
			response.end(JSON.stringify({ ok: true, v: 2, data: descriptor }));
	});
	const f = fixture({
		client,
		delay: async (_ms, signal) => {
			f.bridge.stop();
			signal.throwIfAborted();
		},
	});
	await f.bridge.run();
	assert.equal(requests.filter(({ url }) => url.pathname === "/v1/register").length, 1);
	assert.equal(requests.filter(({ url }) => url.pathname === "/v1/inbox").length, 1);
	assert.equal(requests.filter(({ url }) => url.pathname === "/v1/inbox/typing").length, 0);
	assert.equal(f.trace.includes("send"), false);
	assert.equal(
		f.reports.some(({ kind }) => ["reconnecting", "receive-held"].includes(kind)),
		false,
	);
	assert.equal(f.trace.at(-1), "unsubscribe");
});

test("accepted-but-unacked reconnect reclaims and acknowledges without second native send", async () => {
	const f = fixture({ delay: async () => undefined });
	const nativeSend = f.native.send;
	f.native.send = async (input) => {
		const nativeId = await nativeSend(input);
		nativeTerminal(f, nativeUser(f, nativeId));
		return nativeId;
	};
	const request = f.client.request;
	let acks = 0;
	let claims = 0;
	f.client.request = async (p, b, s) => {
		if (p.startsWith("/v1/inbox?") && claims++ < 2) return [claim];
		if (p === "/v1/inbox/ack" && acks++ === 0)
			throw Object.assign(new Error("HTTP503 daemon restarted"), { retryable: true });
		return request(p, b, s);
	};
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.equal(f.trace.filter((x) => x === "send").length, 1);
	assert.equal(f.trace.filter((x) => x === "register").length, 2);
	assert.equal(f.trace.filter((x) => x === "identity").length, 1); // Recovered acceptance never authorizes another send.
	assert.ok(f.reports.some((e) => e.kind === "reconnecting"));
});

for (const hold of [null, { kind: "consent", reason: "human-consent" }]) {
	test(
		hold
			? "consent inbox holds back off with a ceiling rather than busy-loop"
			: "empty inbox waits back off with a ceiling rather than busy-loop",
		async () => {
			const waits = [];
			const f = fixture({
				delay: async (ms) => {
					waits.push(ms);
					if (waits.length === 8) f.bridge.stop();
				},
			});
			f.client.claimInbox = async () => ({ claims: [], hold });
			await f.bridge.run();
			assert.deepEqual(waits, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
			assert.equal(f.reports.filter((event) => event.kind === "consent-held").length, hold ? 8 : 0);
			assert.equal(f.trace.includes("send"), false);
			assert.equal(f.trace.includes("ack"), false);
		},
	);
}

test("user-driven native shutdown cancels the blocked inbox consumer", async () => {
	const f = fixture();
	const run = f.bridge.run();
	await f.accepted.promise;
	f.event({ type: "session.shutdown", data: {} });
	await run;
	assert.equal(f.bridge.controller.signal.aborted, true);
	assert.equal(f.trace.filter((x) => x === "unsubscribe").length, 1);
});

test("late stale claim after stop cannot reach native send", async () => {
	const f = fixture();
	const pending = deferred();
	const claimed = deferred();
	const request = f.client.request;
	f.client.request = (p, b, s) => {
		if (p.startsWith("/v1/inbox?")) {
			claimed.resolve();
			return pending.promise;
		}
		return request(p, b, s);
	};
	const run = f.bridge.run();
	await claimed.promise;
	f.bridge.stop();
	pending.resolve([claim]);
	await run;
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), false);
});

test("native success without native message ID is not accepted", async () => {
	const f = fixture();
	f.native.send = async () => undefined;
	await f.bridge.run();
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.records.get(claim.message.msg_id).state, "pending");
	f.bridge.stop();
});

test("daemon stale-incarnation refusal holds without transport fallback", async () => {
	const f = fixture();
	const request = f.client.request;
	f.client.request = (p, b, s) => {
		if (p.startsWith("/v1/inbox?")) throw new Error("HTTP409 stale native incarnation");
		return request(p, b, s);
	};
	await f.bridge.run();
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), false);
	assert.ok(f.reports.some((e) => e.kind === "receive-held" && e.diagnostic.includes("409")));
	f.bridge.stop();
});

test("HTTP skew and refused envelopes are never treated as successful claims", async () => {
	for (const response of [
		{ ok: true, v: 1, data: [] },
		{ ok: false, v: 2, error: { kind: "refused" } },
		{ ok: true, v: 2 },
	]) {
		const client = new DaemonClient({
			addr: "127.0.0.1:13700",
			stateDir: "/isolated",
			readKey: async () => "key",
			fetch: async () => new Response(JSON.stringify(response)),
		});
		await assert.rejects(client.claimInbox(tuple));
	}
});

for (const proof of [
	undefined,
	{ ...claim.native_consumer, native_session: "stale-session" },
	{ ...claim.native_consumer, pid: 999 },
	{ ...claim.native_consumer, proc_start: 1 },
]) {
	test(`native consumer proof rejects ${JSON.stringify(proof)} before injection and ack`, async () => {
		const f = fixture();
		const request = f.client.request;
		let claimed = false;
		f.client.request = (p, b, s) => {
			if (p.startsWith("/v1/inbox?")) {
				if (claimed) {
					f.bridge.stop();
					return [];
				}
				claimed = true;
				return [{ ...claim, native_consumer: proof }];
			}
			return request(p, b, s);
		};
		await f.bridge.run();
		assert.equal(f.trace.includes("send"), false);
		assert.equal(f.trace.includes("ack"), false);
		assert.ok(f.reports.some((e) => e.kind === "receive-held"));
		f.bridge.stop();
	});
}

test("spawned native rollover preserves correlation across extension reload", () => {
	const env = { PIJ_SESSION_ID: registration.id, PIJ_SPAWN_ID: "spawn-137" };
	const host = { ...descriptor.proc, pane: descriptor.pane };
	const predecessor = { ...descriptor, spawn_id: "spawn-137" };
	const next = chooseRegistration({
		sessionId: "next-native",
		host,
		folder: "/work",
		seats: [predecessor],
		env,
	});
	assert.equal(next.supersedes, predecessor.id);
	assert.equal(next.spawn_id, "spawn-137");
	const successor = { ...predecessor, id: next.id, session: "next-native" };
	const reloaded = chooseRegistration({
		sessionId: "next-native",
		host,
		folder: "/work",
		seats: [successor],
		env,
	});
	assert.equal(reloaded.id, successor.id);
	assert.equal(reloaded.supersedes, undefined);
});

test("outage emits one actionable reconnect diagnostic per failure episode", async () => {
	let attempts = 0;
	const f = fixture({
		delay: async () => {
			if (attempts >= 5) f.bridge.stop();
		},
	});
	f.client.request = async () => {
		attempts++;
		throw Object.assign(new Error("daemon.key absent: check PIJ_RS_STATE_DIR"), {
			retryable: true,
		});
	};
	await f.bridge.run();
	assert.equal(attempts, 5);
	assert.equal(f.reports.filter((e) => e.kind === "reconnecting").length, 1);
	assert.equal(f.trace.includes("send"), false);
});

test("consent hold release delivers the queued claim without a second message", async () => {
	let held = true;
	let waits = 0;
	const f = fixture({
		delay: async () => {
			waits++;
			held = false;
		},
	});
	const claimInbox = f.client.claimInbox.bind(f.client);
	f.client.claimInbox = (consumer, signal) =>
		held
			? { claims: [], hold: { kind: "consent", reason: "human-consent" } }
			: claimInbox(consumer, signal);
	const run = f.bridge.run();
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.equal(waits, 1);
	assert.ok(
		f.reports.some((event) => event.kind === "consent-held" && event.reason === "human-consent"),
	);
	assert.equal(f.trace.filter((x) => x === "send").length, 1);
});

function nativeUser(f, nativeId, id = `user-${nativeId}`, interactionId) {
	f.event({
		type: "user.message",
		id,
		parentId: "previous",
		data: { messageId: nativeId, turnId: `turn-${nativeId}`, interactionId },
	});
	return id;
}

function nativeTerminal(f, parentId, type = "session.idle", extra = {}) {
	f.event({ type, id: `terminal-${parentId}`, parentId, data: {}, ...extra });
}

for (const type of ["session.idle", "session.error"]) {
	test(`native-event diagnostics retain pre-observe ancestry for healthy ${type}`, async (t) => {
		const f = fixture();
		const run = f.bridge.run();
		t.after(async () => {
			f.bridge.stop();
			await run;
		});
		await f.accepted.promise;
		const root = nativeUser(f, "native-message-137", "diagnostic-user", "diagnostic-interaction");
		f.event({
			type: "assistant.message",
			id: "diagnostic-assistant",
			parentId: root,
			data: { interactionId: "diagnostic-interaction", content: "private model text" },
		});
		nativeTerminal(f, "diagnostic-assistant", type);
		const events = f.reports.filter((event) => event.kind === "native-event");
		assert.equal(events.length, 3);
		assert.deepEqual(events[2], {
			kind: "native-event",
			seat: registration.id,
			nativeSession: registration.harness_session,
			type,
			eventId: "terminal-diagnostic-assistant",
			nativeMessageId: null,
			parentId: "diagnostic-assistant",
			agentId: null,
			interactionId: null,
			completionNativeId: "native-message-137",
			completionLive: true,
			parentKnown: true,
			descendantCount: 3,
			terminal: { type, eventId: "terminal-diagnostic-assistant" },
			grade: "native-event-not-pij-ack",
		});
		assert.equal(events[0].parentKnown, false);
		assert.equal(events[0].descendantCount, 1);
		assert.equal(events[0].interactionId, "diagnostic-interaction");
		assert.equal(events[1].parentKnown, true);
		assert.equal(events[1].descendantCount, 2);
		assert.equal(f.bridge.completion.descendants.size, 0);
		await flush();
		assert.equal(f.reports.filter((event) => event.kind === "native-completed").length, 1);
		assert.equal(f.trace.filter((event) => event === "claim").length, 2);
	});
}

test("native-event diagnostics identify rejected idle without declaring a stall", async (t) => {
	const f = fixture();
	const run = f.bridge.run();
	t.after(async () => {
		f.bridge.stop();
		await run;
	});
	await f.accepted.promise;
	const root = nativeUser(f, "native-message-137", "diagnostic-user", "owned-interaction");
	nativeTerminal(f, "missing-parent", "session.idle", {
		data: { interactionId: "foreign-interaction" },
	});
	const rejected = f.reports.filter((event) => event.kind === "native-event").at(-1);
	assert.deepEqual(rejected, {
		kind: "native-event",
		seat: registration.id,
		nativeSession: registration.harness_session,
		type: "session.idle",
		eventId: "terminal-missing-parent",
		nativeMessageId: null,
		parentId: "missing-parent",
		agentId: null,
		interactionId: "foreign-interaction",
		completionNativeId: "native-message-137",
		completionLive: true,
		parentKnown: false,
		descendantCount: 2,
		terminal: null,
		grade: "native-event-not-pij-ack",
	});
	nativeTerminal(f, root, "session.idle", { agentId: "subagent" });
	const subagent = f.reports.filter((event) => event.kind === "native-event").at(-1);
	assert.equal(subagent.agentId, "subagent");
	assert.equal(subagent.parentKnown, true);
	assert.equal(subagent.terminal, null);
	await flush();
	assert.equal(f.reports.filter((event) => event.kind === "native-event").length, 3);
	assert.equal(
		f.reports.some((event) =>
			["native-completed", "receive-held", "reconnecting"].includes(event.kind),
		),
		false,
	);
	assert.equal(f.trace.filter((event) => event === "claim").length, 1);
	assert.equal((await f.bridge.send({ to: "pij-peer", message: "still working" })).ok, true);
});

test("native-event diagnostics expose only scalar IDs and never callback or buffered bodies", async (t) => {
	const f = fixture();
	const sending = deferred();
	f.native.send = () => {
		sending.resolve();
		return new Promise(() => {});
	};
	const run = f.bridge.run();
	t.after(async () => {
		f.bridge.stop();
		await run;
	});
	// Before there is a completion, absent metadata is explicit rather than fabricated.
	f.event({ type: "session.idle", id: "before-send", data: {} });
	const before = f.reports.find((event) => event.kind === "native-event");
	assert.equal(before.completionNativeId, null);
	assert.equal(before.completionLive, false);
	assert.equal(before.parentKnown, false);
	assert.equal(before.descendantCount, 0);
	assert.equal(before.terminal, null);
	await sending.promise;
	const secret = "NEVER_LOG_BODY_ARGS_OR_CREDENTIALS";
	for (const type of ["user.message", "assistant.message", "session.idle", "session.error"]) {
		f.event({
			type,
			id: `event-${type}`,
			parentId: "previous",
			agentId: "subagent",
			prompt: secret,
			credentials: secret,
			data: {
				messageId: "unbound-native",
				interactionId: "unbound-interaction",
				content: secret,
				body: secret,
				arguments: { key: secret },
				token: secret,
			},
		});
	}
	// Neither malformed ID objects nor unlogged callback types may leak arbitrary payloads.
	const malformed = { key: secret };
	f.event({
		type: "assistant.message",
		id: malformed,
		parentId: malformed,
		agentId: malformed,
		data: { messageId: malformed, interactionId: malformed },
	});
	f.event({ type: "assistant.message_delta", id: "delta", data: { content: secret } });
	f.event({ type: "tool.execution_complete", id: "tool", data: { arguments: secret } });
	const events = f.reports.filter((event) => event.kind === "native-event");
	assert.equal(events.length, 6);
	assert.equal(JSON.stringify(events).includes(secret), false);
	for (const event of events.slice(1)) {
		assert.equal(event.completionNativeId, null);
		assert.equal(event.completionLive, true);
		assert.equal(event.parentKnown, false);
		assert.equal(event.descendantCount, 0);
		assert.equal(event.terminal, null);
		assert.deepEqual(
			Object.keys(event).sort(),
			[
				"kind",
				"seat",
				"nativeSession",
				"type",
				"eventId",
				"nativeMessageId",
				"parentId",
				"agentId",
				"interactionId",
				"completionNativeId",
				"completionLive",
				"parentKnown",
				"descendantCount",
				"terminal",
				"grade",
			].sort(),
		);
	}
	for (const key of ["eventId", "nativeMessageId", "parentId", "agentId", "interactionId"])
		assert.equal(events.at(-1)[key], null);
	assert.equal(f.trace.includes("ack"), false);
});

test("post-ack barrier blocks next claim until current native completion, not outgoing", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await f.accepted.promise;
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 1);
	assert.equal(f.trace.includes("ack"), true);
	assert.equal(
		(await f.bridge.send({ to: "pij-peer", message: "explicit while working" })).ok,
		true,
	);
	nativeTerminal(f, "old-turn");
	const user = nativeUser(f, "native-message-137");
	nativeTerminal(f, "old-turn");
	nativeTerminal(f, user, "session.idle", { agentId: "subagent" });
	f.event({
		type: "assistant.message",
		id: "assistant-current",
		parentId: user,
		data: { content: "not idle" },
	});
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 1);
	nativeTerminal(f, "assistant-current");
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	f.bridge.stop();
	await run;
});

for (const type of ["session.idle", "session.error"]) {
	test(`post-ack barrier retains ${type} before native send resolves`, async (t) => {
		const f = fixture();
		t.after(() => f.bridge.stop());
		const sending = deferred();
		const acceptance = deferred();
		f.native.send = () => {
			f.trace.push("send");
			const user = nativeUser(f, "early-native");
			nativeTerminal(f, user, type);
			sending.resolve();
			return acceptance.promise;
		};
		const run = f.bridge.run();
		await sending.promise;
		assert.equal(f.trace.includes("ack"), false);
		acceptance.resolve("early-native");
		await f.accepted.promise;
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 2);
		assert.ok(f.reports.some((e) => e.kind === "native-completed" && e.type === type));
		f.bridge.stop();
		await run;
	});
}

test("post-ack barrier captures idle during acknowledgement but cannot overtake ack", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const acknowledging = deferred();
	const ack = deferred();
	const request = f.client.request;
	f.client.request = (p, b, s) => {
		if (p === "/v1/inbox/ack") {
			acknowledging.resolve();
			return ack.promise;
		}
		return request(p, b, s);
	};
	const run = f.bridge.run();
	await acknowledging.promise;
	nativeTerminal(f, nativeUser(f, "native-message-137"));
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 1);
	ack.resolve(claim.job_id);
	await f.accepted.promise;
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	f.bridge.stop();
	await run;
});

test("post-ack barrier cannot release a later send from earlier generation events", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const request = f.client.request;
	let claims = 0;
	let sends = 0;
	f.client.request = (p, b, s) => {
		if (p.startsWith("/v1/inbox?") && ++claims <= 2)
			return [{ ...claim, job_id: claims, message: { ...claim.message, msg_id: `pij-${claims}` } }];
		if (p === "/v1/inbox/ack") return b.job_id;
		if (p.startsWith("/v1/inbox?"))
			return new Promise((resolve) =>
				s.addEventListener("abort", () => resolve([]), { once: true }),
			);
		return request(p, b, s);
	};
	f.native.send = async () => {
		const nativeId = `native-${++sends}`;
		f.history.push({
			type: "user.message",
			id: `user-${nativeId}`,
			data: { messageId: nativeId, interactionId: `interaction-${sends}` },
		});
		return nativeId;
	};
	const run = f.bridge.run();
	await f.accepted.promise;
	nativeTerminal(f, nativeUser(f, "native-1", "user-native-1", "interaction-1"));
	await flush();
	assert.equal(sends, 2);
	assert.equal(claims, 2);
	const currentUser = nativeUser(f, "native-2", "user-native-2", "interaction-2");
	nativeTerminal(f, "user-native-1");
	f.event({
		type: "user.message",
		id: "user-native-1",
		parentId: currentUser,
		data: { messageId: "native-1", interactionId: "interaction-1" },
	});
	nativeTerminal(f, "user-native-1");
	f.event({
		type: "assistant.message",
		id: "old-interaction",
		parentId: "omitted-old-parent",
		data: { interactionId: "interaction-1", turnId: "0" },
	});
	nativeTerminal(f, "old-interaction");
	await flush();
	assert.equal(claims, 2);
	f.event({
		type: "assistant.message",
		id: "current-interaction",
		parentId: "omitted-current-parent",
		data: { interactionId: "interaction-2", turnId: "0" },
	});
	nativeTerminal(f, "current-interaction");
	await flush();
	assert.equal(claims, 3);
	f.bridge.stop();
	await run;
});

test("post-ack barrier completes the owned final turn before a following foreground user", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await f.accepted.promise;
	const user = nativeUser(f, "native-message-137", "owned-user", "owned-interaction");
	f.event({ type: "assistant.message", id: "owned-answer", parentId: user, data: {} });
	f.event({ type: "assistant.turn_end", id: "owned-end", parentId: "owned-answer", data: {} });
	f.event({
		type: "user.message",
		id: "next-user",
		parentId: "owned-end",
		data: { messageId: "next-native", interactionId: "next-interaction" },
	});
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	assert.equal(
		f.reports.filter((event) => event.kind === "native-completed" && event.eventId === "owned-end")
			.length,
		1,
	);
	nativeTerminal(f, "next-user");
	await flush();
	assert.equal(f.reports.filter((event) => event.kind === "native-completed").length, 1);
	f.bridge.stop();
	await run;
});

for (const changed of [
	{ parentId: "unrelated-end" },
	{ parentId: "owned-user" },
	{ agentId: "subagent" },
	{ data: { messageId: "next-native" } },
]) {
	test(`post-ack barrier rejects unproven foreground transition ${JSON.stringify(changed)}`, async (t) => {
		const f = fixture();
		t.after(() => f.bridge.stop());
		const run = f.bridge.run();
		await f.accepted.promise;
		const user = nativeUser(f, "native-message-137", "owned-user", "owned-interaction");
		f.event({ type: "assistant.turn_end", id: "owned-end", parentId: user, data: {} });
		f.event({
			type: "user.message",
			id: "next-user",
			parentId: "owned-end",
			data: { messageId: "next-native", interactionId: "next-interaction" },
			...changed,
		});
		f.event({
			type: "assistant.message",
			id: "next-answer",
			parentId: "next-user",
			data: { interactionId: "next-interaction" },
		});
		nativeTerminal(f, "next-answer");
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 1);
		nativeTerminal(f, "owned-end");
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 2);
		f.bridge.stop();
		await run;
	});
}

test("post-ack barrier keeps accepted duplicate pending without completion evidence", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	f.records.set(claim.message.msg_id, {
		state: "accepted",
		message: claim.message,
		nativeId: "previous-native",
	});
	f.history.push({
		type: "user.message",
		id: "recovered-user",
		data: { messageId: "previous-native" },
	});
	const run = f.bridge.run();
	await f.accepted.promise;
	await flush();
	assert.equal(f.trace.includes("ack"), true);
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.filter((x) => x === "claim").length, 1);
	assert.ok(f.reports.some((e) => e.kind === "completion-wait"));
	assert.equal(
		f.reports.some((e) => e.kind === "native-completed"),
		false,
	);
	assert.equal((await f.bridge.send({ to: "pij-peer", message: "receive is waiting" })).ok, true);
	f.bridge.stop();
	await run;
	assert.equal(f.trace.filter((x) => x === "unsubscribe").length, 1);
});

test("post-ack barrier survives failed ack before reclaiming accepted duplicate", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	const request = f.client.request;
	let claims = 0;
	let acks = 0;
	f.client.request = (p, b, s) => {
		if (p.startsWith("/v1/inbox?")) {
			claims++;
			return [claim];
		}
		if (p === "/v1/inbox/ack" && acks++ === 0)
			throw Object.assign(new Error("lost ack response"), { retryable: true });
		if (p === "/v1/inbox/ack" && acks > 2) f.bridge.stop();
		return request(p, b, s);
	};
	const run = f.bridge.run();
	await flush();
	assert.equal(claims, 1);
	assert.equal(acks, 1);
	nativeTerminal(f, nativeUser(f, "native-message-137"));
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.equal(claims, 2);
	assert.equal(f.trace.filter((x) => x === "send").length, 1);
	assert.equal(f.reports.filter((e) => e.kind === "native-completed").length, 1);
});

test("post-ack barrier shutdown disposes subscription and ignores late completion", async () => {
	const f = fixture();
	const run = f.bridge.run();
	await f.accepted.promise;
	await flush();
	f.event({ type: "session.shutdown", data: {} });
	await run;
	nativeTerminal(f, nativeUser(f, "native-message-137"));
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 1);
	assert.equal(f.trace.filter((x) => x === "unsubscribe").length, 1);
	assert.equal(
		f.reports.some((e) => e.kind === "native-completed"),
		false,
	);
});

test("post-ack barrier follows real native PoC ancestry with sibling ephemeral idles", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	// Native PoC target-events.jsonl:36-54, supplied by PM. Opaque IDs remapped;
	// only the terminal's ancestor chain and sibling assistant.idle retained, no bodies.
	const events = [
		{
			type: "user.message",
			id: "poc-36",
			parentId: "prior-context",
			data: { messageId: "native-poc-second", turnId: "0" },
		},
		{ type: "system.message", id: "poc-37", parentId: "poc-36", data: {} },
		{ type: "assistant.turn_start", id: "poc-39", parentId: "poc-37", data: { turnId: "0" } },
		{ type: "assistant.message", id: "poc-48", parentId: "poc-39", data: {} },
		{ type: "assistant.turn_end", id: "poc-51", parentId: "poc-48", data: { turnId: "0" } },
		{ type: "assistant.idle", id: "poc-53", parentId: "poc-51", ephemeral: true, data: {} },
		{ type: "session.idle", id: "poc-54", parentId: "poc-51", ephemeral: true, data: {} },
	];
	f.native.send = async () => "native-poc-second";
	const run = f.bridge.run();
	await flush();
	f.event(events[0]);
	await f.accepted.promise;
	for (const event of events.slice(1, -1)) f.event(event);
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	f.event(events.at(-1));
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	assert.ok(f.reports.some((e) => e.kind === "native-completed" && e.eventId === "poc-51"));
	f.bridge.stop();
	await run;
});

test("post-ack interaction anchor repairs the direct product permission gap without inferring idle", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const run = f.bridge.run();
	await f.accepted.promise;
	// PM pm-native-product-recipient.jsonl seq6-44, opaque IDs remapped.
	// permission.requested is absent; external_tool.requested IS visible to the product.
	nativeUser(f, "native-message-137", "product-6", "product-interaction");
	const events = [
		{ type: "system.message", id: "product-7", parentId: "product-6", data: {} },
		{
			type: "assistant.turn_start",
			id: "product-9",
			parentId: "product-7",
			data: { interactionId: "product-interaction", turnId: "0" },
		},
		{
			type: "assistant.message",
			id: "product-18",
			parentId: "product-9",
			data: {
				interactionId: "product-interaction",
				toolRequests: [{ toolCallId: "product-tool" }],
			},
		},
		{ type: "tool.execution_start", id: "product-19", parentId: "product-18", data: {} },
		{ type: "permission.completed", id: "product-20", parentId: "omitted-permission", data: {} },
		{ type: "external_tool.requested", id: "product-21", parentId: "product-20", data: {} },
		{ type: "external_tool.completed", id: "product-22", parentId: "product-21", data: {} },
		{
			type: "tool.execution_complete",
			id: "product-24",
			parentId: "product-22",
			data: { interactionId: "product-interaction", turnId: "0" },
		},
		{ type: "assistant.turn_end", id: "product-27", parentId: "product-24", data: { turnId: "0" } },
		{
			type: "assistant.turn_start",
			id: "product-29",
			parentId: "product-27",
			data: { interactionId: "product-interaction", turnId: "1" },
		},
		{
			type: "assistant.message",
			id: "product-38",
			parentId: "product-29",
			data: { interactionId: "product-interaction" },
		},
		{ type: "assistant.turn_end", id: "product-41", parentId: "product-38", data: { turnId: "1" } },
		{ type: "assistant.idle", id: "product-43", parentId: "product-41", ephemeral: true, data: {} },
	];
	for (const event of events) f.event(event);
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	assert.equal(f.trace.filter((x) => x === "ack").length, 1);
	assert.equal(
		(await f.bridge.send({ to: "pij-peer", message: "explicit while waiting" })).ok,
		true,
	);
	f.event({
		type: "session.idle",
		id: "product-44",
		parentId: "product-41",
		ephemeral: true,
		data: { mode: "interactive" },
	});
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	assert.equal(
		f.reports.filter((e) => e.kind === "native-completed" && e.eventId === "product-41").length,
		1,
	);
	f.bridge.stop();
	await run;
});

for (const type of ["assistant.turn_start", "assistant.message", "tool.execution_complete"]) {
	test(`post-ack interaction anchor accepts only the matched foreground ${type}`, async (t) => {
		const f = fixture();
		t.after(() => f.bridge.stop());
		const run = f.bridge.run();
		await f.accepted.promise;
		const anchor = {
			type,
			id: "unbound",
			parentId: "omitted-parent",
			data: { interactionId: "owned-interaction", turnId: "0" },
		};
		// Neither pre-root metadata nor a foreign/subagent user may establish the binding.
		f.event(anchor);
		nativeTerminal(f, anchor.id);
		nativeUser(f, "foreign-native", "foreign-user", "owned-interaction");
		f.event({
			type: "user.message",
			id: "child-user",
			agentId: "child",
			data: { messageId: "native-message-137", interactionId: "owned-interaction" },
		});
		f.event({ ...anchor, id: "still-unbound" });
		nativeTerminal(f, "still-unbound");
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 1);
		nativeUser(f, "native-message-137", "owned-user", "owned-interaction");
		for (const terminal of ["session.idle", "session.error"]) {
			f.event({ ...anchor, type: terminal, id: terminal });
		}
		for (const rejected of [
			{ ...anchor, id: "foreign", data: { interactionId: "foreign-interaction", turnId: "0" } },
			{ ...anchor, id: "child", agentId: "child" },
			{ ...anchor, id: "turn-only", data: { turnId: "0" } },
		]) {
			f.event(rejected);
			nativeTerminal(f, rejected.id);
		}
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 1);
		f.event({ ...anchor, id: "owned-anchor" });
		f.event({
			type: "assistant.turn_end",
			id: "owned-end",
			parentId: "owned-anchor",
			data: { turnId: "0" },
		});
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, type === "assistant.message" ? 2 : 1);
		nativeTerminal(f, "owned-end");
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 2);
		f.bridge.stop();
		await run;
	});
}

for (const interactionId of [undefined, "", "   "]) {
	test(`post-ack interaction anchor rejects absent or blank binding ${JSON.stringify(interactionId)}`, async (t) => {
		const f = fixture();
		t.after(() => f.bridge.stop());
		const run = f.bridge.run();
		await f.accepted.promise;
		const root = nativeUser(f, "native-message-137", "owned-user", interactionId);
		f.event({
			type: "assistant.message",
			id: "unbound-message",
			parentId: "omitted-parent",
			data: { interactionId },
		});
		nativeTerminal(f, "unbound-message");
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 1);
		// Existing complete ancestry still works without interaction metadata.
		nativeTerminal(f, root);
		await flush();
		assert.equal(f.trace.filter((x) => x === "claim").length, 2);
		f.bridge.stop();
		await run;
	});
}

test("post-ack interaction anchor retains correlation before send acceptance", async (t) => {
	const f = fixture();
	t.after(() => f.bridge.stop());
	const sending = deferred();
	const acceptance = deferred();
	f.native.send = () => {
		nativeUser(f, "early-native", "early-user", "early-interaction");
		f.event({
			type: "tool.execution_complete",
			id: "early-tool",
			parentId: "omitted-permission",
			data: { interactionId: "early-interaction" },
		});
		nativeTerminal(f, "early-tool");
		sending.resolve();
		return acceptance.promise;
	};
	const run = f.bridge.run();
	await sending.promise;
	assert.equal(f.trace.includes("ack"), false);
	acceptance.resolve("early-native");
	await f.accepted.promise;
	await flush();
	assert.equal(f.trace.filter((x) => x === "claim").length, 2);
	assert.equal(f.reports.filter((e) => e.kind === "native-completed").length, 1);
	f.bridge.stop();
	await run;
});

test("claim page preserves typed target and transient hold metadata without changing request data", async () => {
	const target =
		"native-target-session:original-native; resume that native session or use a new seat and intentionally reissue the message";
	for (const [meta, hold] of [
		[undefined, null],
		[null, null],
		["observation-only note", null],
		...["native-pane-changed", "human-consent"].map((reason) => [
			`native-consumer-held:${reason}`,
			{ kind: "consent", reason },
		]),
		[`native-consumer-held:${target}`, { kind: "native-target", reason: target }],
	]) {
		const client = new DaemonClient({
			addr: "127.0.0.1:13700",
			stateDir: "/isolated",
			readKey: async () => "key",
			fetch: async () =>
				new Response(JSON.stringify({ ok: true, command: "pij inbox", v: 2, data: [], meta })),
		});
		assert.deepEqual(await client.claimInbox(tuple), { claims: [], hold });
		assert.deepEqual(await client.request("/v1/seats"), []);
	}
});

test("malformed claim hold metadata is not silently downgraded to an empty inbox", async () => {
	const client = new DaemonClient({
		addr: "127.0.0.1:13700",
		stateDir: "/isolated",
		readKey: async () => "key",
		fetch: async () =>
			new Response(
				JSON.stringify({
					ok: true,
					v: 2,
					data: [],
					meta: { reason: "native-target-session:original-native" },
				}),
			),
	});
	await assert.rejects(client.claimInbox(tuple), /Malformed native inbox hold metadata/);
});

test("only the exact native-unavailable inbox refusal is retryable", async () => {
	const inbox = `/v1/inbox?${new URLSearchParams(tuple)}`;
	const typing = `/v1/inbox/typing?${new URLSearchParams(tuple)}`;
	for (const [label, path, status, envelope, expected] of [
		["claim", inbox, 400, unavailableRefusal, true],
		["ack", "/v1/inbox/ack", 400, unavailableRefusal, true],
		["typing snapshot", typing, 400, unavailableRefusal, true],
		[
			"typing foreign refusal",
			typing,
			400,
			{ ...unavailableRefusal, meta: "native incarnation mismatch" },
			false,
		],
		[
			"typing wrong command",
			typing,
			400,
			{ ...unavailableRefusal, command: "pij register" },
			false,
		],
		["typing wrong version", typing, 400, { ...unavailableRefusal, v: 1 }, false],
		["unrelated endpoint", "/v1/register", 400, unavailableRefusal, false],
		["different status", inbox, 409, unavailableRefusal, false],
		["skew", inbox, 400, { ...unavailableRefusal, v: 1 }, false],
		["wrong category", inbox, 400, { ...unavailableRefusal, error: "adapter" }, false],
		["wrong command", inbox, 400, { ...unavailableRefusal, command: "pij register" }, false],
		["not refusal", inbox, 400, { ...unavailableRefusal, ok: true }, false],
		["missing category", inbox, 400, { ...unavailableRefusal, error: undefined }, false],
		[
			"wrong adapter",
			inbox,
			400,
			{
				...unavailableRefusal,
				meta: unavailableRefusal.meta.replace("daemon/native-inbox", "unrelated"),
			},
			false,
		],
		[
			"stale tuple",
			inbox,
			400,
			{
				...unavailableRefusal,
				meta: "daemon/native-inbox: native incarnation mismatch: native_session, pid and proc_start must match the current Copilot seat",
			},
			false,
		],
		[
			"missing or non-Copilot shared legacy diagnostic",
			inbox,
			400,
			{
				...unavailableRefusal,
				meta: "daemon/native-inbox: native incarnation no longer belongs to a current Copilot seat",
			},
			false,
		],
		[
			"quoted marker",
			inbox,
			400,
			{ ...unavailableRefusal, meta: `stale tuple; previously ${unavailableRefusal.meta}` },
			false,
		],
		[
			"malformed metadata",
			inbox,
			400,
			{ ...unavailableRefusal, meta: { reason: unavailableRefusal.meta } },
			false,
		],
	]) {
		const client = new DaemonClient({
			addr: "127.0.0.1:13700",
			stateDir: "/isolated",
			readKey: async () => "fixture-key",
			fetch: async () => new Response(JSON.stringify(envelope), { status }),
		});
		await assert.rejects(client.request(path), (error) => {
			assert.equal(error.retryable, expected, label);
			return true;
		});
	}
});

test("native-unavailable retries re-register with bounded backoff and stop cleanly", async () => {
	let registrations = 0;
	let claims = 0;
	const waits = [];
	const client = new DaemonClient({
		addr: "127.0.0.1:13700",
		stateDir: "/isolated",
		readKey: async () => "fixture-key",
		fetch: async (url) => {
			if (new URL(url).pathname === "/v1/register") {
				registrations++;
				return new Response(JSON.stringify({ ok: true, v: 2, data: descriptor }));
			}
			assert.equal(new URL(url).pathname, "/v1/inbox");
			claims++;
			return new Response(JSON.stringify(unavailableRefusal), { status: 400 });
		},
	});
	const f = fixture({
		client,
		delay: async (ms) => {
			waits.push(ms);
			if (waits.length === 8) f.bridge.stop();
		},
	});
	await f.bridge.run();
	assert.deepEqual(waits, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
	assert.equal(registrations, 8);
	assert.equal(claims, 8);
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.reports.filter((event) => event.kind === "reconnecting").length, 1);
	assert.equal(f.trace.at(-1), "unsubscribe");
});

for (const compatibility of [undefined, true]) {
	test(`self-reported Hold with a valid native tuple delivers once without a status veto (compatibility=${compatibility})`, async (t) => {
		const snapshot = {
			state: "unavailable",
			native_consumer: claim.native_consumer,
			typing_grace_ms: 60000,
			observed_at_ms: 100000,
			reason: "native-typing-sensor-unavailable",
			...(compatibility === undefined ? {} : { semantic_hold: compatibility }),
		};
		const { client, requests } = await deadlineHttpFixture(t, ({ url, body }, response) => {
			let data;
			if (url.pathname === "/v1/register") data = { ...descriptor, semantic_state: "hold" };
			else if (url.pathname === "/v1/inbox") data = [claim];
			else if (url.pathname === "/v1/inbox/typing") data = snapshot;
			else if (url.pathname === "/v1/inbox/ack") {
				assert.deepEqual(body, { ...tuple, job_id: claim.job_id });
				data = claim.job_id;
			} else assert.fail(`Unexpected native request ${url.pathname}`);
			response.end(JSON.stringify({ ok: true, v: 2, data }));
		});
		const f = fixture({ client });
		t.after(() => f.bridge.stop());
		const run = f.bridge.run();
		await Promise.race([f.accepted.promise, run]);
		f.bridge.stop();
		await run;
		assert.equal(f.trace.filter((step) => step === "send").length, 1);
		assert.equal(requests.filter(({ url }) => url.pathname === "/v1/inbox/ack").length, 1);
		assert.equal(f.records.get(claim.message.msg_id)?.state, "accepted");
		assert.equal(
			f.reports.some(({ kind }) => ["consent-held", "receive-held"].includes(kind)),
			false,
		);
		assert.equal(
			requests.some(({ url }) => ["/v1/hold", "/v1/release"].includes(url.pathname)),
			false,
		);
	});
}

test("native identity preflight binds exact tuple and current bearer key over real HTTP", async (t) => {
	let snapshot = observedTyping({ retry_after_ms: 60000 });
	const { client, requests } = await deadlineHttpFixture(t, (_entry, response) => {
		response.end(JSON.stringify({ ok: true, command: "pij inbox", v: 2, data: snapshot }));
	});
	assert.deepEqual(await client.nativeSnapshot(tuple), snapshot);
	client.readKey = async () => "rotated-native-key";
	snapshot = unavailableTyping();
	assert.deepEqual(await client.nativeSnapshot(tuple), snapshot);
	assert.deepEqual(
		requests.map(({ authorization }) => authorization),
		["Bearer deadline-key", "Bearer rotated-native-key"],
	);
	for (const request of requests) {
		assert.equal(request.method, "GET");
		assert.equal(request.url.pathname, "/v1/inbox/typing");
		assert.deepEqual(
			[...request.url.searchParams].sort(),
			Object.entries(tuple)
				.map(([key, value]) => [key, String(value)])
				.sort(),
		);
		assert.equal(request.body, undefined);
	}
});

test("native identity preflight rejects missing or mismatched proof over HTTP", async (t) => {
	let snapshot;
	const { client } = await deadlineHttpFixture(t, (_entry, response) => {
		response.end(JSON.stringify({ ok: true, command: "pij inbox", v: 2, data: snapshot }));
	});
	for (const invalid of [
		null,
		[],
		observedTyping({ native_consumer: undefined }),
		observedTyping({ native_consumer: { ...claim.native_consumer, native_session: "other" } }),
		observedTyping({ native_consumer: { ...claim.native_consumer, pid: 138 } }),
		observedTyping({ native_consumer: { ...claim.native_consumer, proc_start: 1 } }),
		observedTyping({ native_consumer: { ...claim.native_consumer, pid: "137" } }),
	]) {
		snapshot = invalid;
		await assert.rejects(client.nativeSnapshot(tuple), { retryable: false });
	}
});

test("native identity preflight ignores informational typing metadata", async (t) => {
	let snapshot;
	const { client } = await deadlineHttpFixture(t, (_entry, response) => {
		response.end(JSON.stringify({ ok: true, command: "pij inbox", v: 2, data: snapshot }));
	});
	for (const observation of [
		observedTyping({ retry_after_ms: 60000 }),
		unavailableTyping(),
		observedTyping({
			state: "unknown",
			typing_grace_ms: -1,
			observed_at_ms: null,
			retry_after_ms: "busy",
			source: "pane-output",
		}),
		{ native_consumer: claim.native_consumer },
	]) {
		snapshot = observation;
		assert.deepEqual(await client.nativeSnapshot(tuple), snapshot);
	}
});

test("new native send waits for incarnation verification before creating durable intent", async (t) => {
	const snapshot = deferred();
	const f = fixture();
	t.after(() => f.bridge.stop());
	let reads = 0;
	f.client.nativeSnapshot = (consumer, signal) => {
		reads++;
		assert.deepEqual(consumer, tuple);
		assert.equal(signal.aborted, false);
		return snapshot.promise;
	};
	const run = f.bridge.run();
	await flush();
	assert.equal(reads, 1);
	assert.equal(f.trace.filter((step) => step === "claim").length, 1);
	assert.equal(f.records.size, 0);
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.includes("ack"), false);
	snapshot.resolve(unavailableTyping());
	await f.accepted.promise;
	f.bridge.stop();
	await run;
	assert.equal(f.trace.filter((step) => step === "send").length, 1);
});

// Native session sends do not touch the composer, so typing never defers delivery.
for (const [label, snapshot] of [
	["recent human typing", observedTyping({ retry_after_ms: 60000 })],
	["unavailable typing sensor", unavailableTyping()],
]) {
	test(`native delivery proceeds during ${label} without submitting the human draft`, async (t) => {
		const draft = "UNSUBMITTED human draft 日本語  ";
		const composer = Object.freeze({ text: draft });
		const f = fixture({ delay: async () => f.bridge.stop() });
		t.after(() => f.bridge.stop());
		f.client.nativeSnapshot = async () => snapshot;
		const send = f.native.send;
		f.native.send = async (input) => {
			assert.deepEqual(input, {
				prompt: `[pij from ${JSON.stringify(claim.message.from)}; msg_id=${JSON.stringify(claim.message.msg_id)}]\n${claim.message.body}`,
				mode: "immediate",
			});
			assert.equal(input.prompt.includes(draft), false);
			assert.equal(composer.text, draft);
			return send(input);
		};
		const run = f.bridge.run();
		await flush();
		assert.equal(f.trace.filter((step) => step === "send").length, 1);
		assert.equal(f.trace.includes("ack"), true);
		assert.equal(f.trace.includes("hold"), false);
		assert.equal(f.trace.includes("release"), false);
		assert.equal(composer.text, draft);
		assert.equal(
			f.reports.some(({ kind }) =>
				["consent-held", "typing-sensor-wait", "receive-held"].includes(kind),
			),
			false,
		);
		f.bridge.stop();
		await run;
	});
}

// Plan 137: removing typing checks does not authorize another native target or bypass consent.
for (const hold of [
	{ kind: "consent", reason: "human-consent" },
	{ kind: "native-target", reason: "native-target-session:original-native" },
]) {
	test(`native delivery retains ${hold.reason} while the human is typing`, async (t) => {
		const f = fixture({ delay: async () => f.bridge.stop() });
		t.after(() => f.bridge.stop());
		f.client.nativeSnapshot = async () => observedTyping({ retry_after_ms: 60000 });
		f.client.claimInbox = async () => ({ claims: [], hold });
		await f.bridge.run();
		assert.equal(f.trace.includes("send"), false);
		assert.equal(f.trace.includes("ack"), false);
		assert.equal(f.trace.includes("release"), false);
		assert.equal(f.records.size, 0);
		assert.ok(
			f.reports.some((event) =>
				hold.kind === "native-target"
					? event.kind === "receive-held" && event.holdKind === "native-target"
					: event.kind === "consent-held" && event.reason === hold.reason,
			),
		);
	});
}

test("shutdown during native preflight cancels old-context work without intent send or ACK", async (t) => {
	const blocked = deferred();
	let observedSignal;
	const f = fixture();
	t.after(() => f.bridge.stop());
	f.client.nativeSnapshot = (_consumer, signal) => {
		observedSignal = signal;
		return blocked.promise;
	};
	const run = f.bridge.run();
	await flush();
	assert.ok(observedSignal);
	assert.equal(f.trace.includes("send"), false);
	f.event({ type: "session.shutdown", data: {} });
	assert.equal(observedSignal.aborted, true);
	await run;
	blocked.resolve(observedTyping());
	await flush();
	assert.equal(f.records.size, 0);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.trace.filter((step) => step === "claim").length, 1);
	assert.equal(f.trace.filter((step) => step === "unsubscribe").length, 1);
});

test("native identity HTTP wait is externally abortable and its deadline is not inbox renewal", async (t) => {
	const controller = new AbortController();
	const reason = new DOMException("Native consumer stopped", "AbortError");
	const { client, requests } = await deadlineHttpFixture(t, () => controller.abort(reason));
	await assert.rejects(
		client.nativeSnapshot(tuple, controller.signal),
		(error) => error === reason,
	);
	assert.equal(requests.length, 1);
	const stalled = await deadlineHttpFixture(t, () => undefined);
	await assert.rejects(stalled.client.nativeSnapshot(tuple), { retryable: true });
});

for (const snapshot of [
	observedTyping({ native_consumer: undefined }),
	unavailableTyping({ native_consumer: { ...claim.native_consumer, pid: 138 } }),
]) {
	test("invalid native proof holds receiving without hiding outgoing", async (t) => {
		const { client, requests } = await deadlineHttpFixture(t, ({ url, body }, response) => {
			let data;
			if (url.pathname === "/v1/register") data = descriptor;
			else if (url.pathname === "/v1/inbox") data = [claim];
			else if (url.pathname === "/v1/inbox/typing") data = snapshot;
			else if (url.pathname === "/v1/send") data = { msg_id: body.msg_id };
			else assert.fail(`Unexpected held-native request ${url.pathname}`);
			response.end(JSON.stringify({ ok: true, v: 2, data }));
		});
		const f = fixture({ client });
		t.after(() => f.bridge.stop());
		await f.bridge.run();
		assert.equal(f.records.size, 0);
		assert.equal(f.trace.includes("send"), false);
		assert.equal(
			requests.some(({ url }) => url.pathname === "/v1/inbox/ack"),
			false,
		);
		assert.ok(f.reports.some(({ kind }) => kind === "receive-held"));
		assert.equal(
			(await f.bridge.send({ to: "pij-peer", message: "outgoing while held" })).ok,
			true,
		);
	});
}

test("incremental history: expired cursor cannot acknowledge or retry from fallback events", async (t) => {
	const f = discardedQueueFixture();
	t.after(() => f.bridge.stop());
	const request = f.client.request;
	f.client.request = async (...args) => {
		const result = await request(...args);
		if (args[0] === "/v1/inbox/ack") f.bridge.stop();
		return result;
	};
	const read = f.native.rpc.eventLog.read;
	f.native.rpc.eventLog.read = async (input) =>
		input.direction === "backward"
			? read(input)
			: {
					events: [
						{ type: "session.idle", id: "fallback-boundary", data: {} },
						{ type: "user.message", id: "fallback-user", data: { messageId: "queued-native-1" } },
					],
					cursor: "rebased",
					hasMore: false,
					cursorStatus: "expired",
				};
	await f.bridge.run();
	assert.equal(f.state.sends, 1);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.trace.includes("retry-intent"), false);
	assert.ok(f.reports.some(({ kind }) => kind === "receive-held"));
	f.bridge.stop();
});

test("incremental history: accepted recovery spans pages without skipping consumption ancestry", async (t) => {
	const f = fixture({ delay: () => flush() });
	t.after(() => f.bridge.stop());
	f.records.set(claim.message.msg_id, {
		state: "accepted",
		message: claim.message,
		nativeId: "paged-native",
	});
	f.history.push(
		...Array.from({ length: 127 }, (_, i) => ({
			type: "session.idle",
			id: `historic-boundary-${i}`,
			data: {},
		})),
		{ type: "user.message", id: "paged-user", data: { messageId: "paged-native" } },
		{ type: "assistant.message", id: "paged-answer", parentId: "paged-user", data: {} },
		{ type: "assistant.turn_end", id: "paged-end", parentId: "paged-answer", data: {} },
	);
	const run = f.bridge.run();
	await f.accepted.promise;
	await flush();
	assert.equal(f.trace.includes("send"), false);
	assert.equal(f.trace.filter((entry) => entry === "ack").length, 1);
	assert.equal(f.trace.filter((entry) => entry === "claim").length, 2);
	assert.ok(
		f.reports.some(({ kind, eventId }) => kind === "native-completed" && eventId === "paged-end"),
	);
	f.bridge.stop();
	await run;
});

async function receiverLeaseScenario(t, replies) {
	const waits = [];
	let attempts = 0;
	const f = fixture({
		async heartbeatDelay(ms, signal) {
			waits.push(ms);
			await flush();
			if (waits.length === replies.length) f.bridge.stop();
			signal.throwIfAborted();
		},
	});
	t.after(() => f.bridge.stop());
	f.client.request = async () => {
		const reply = replies[attempts++];
		if (reply instanceof Error) throw reply;
		return reply;
	};
	await f.bridge.keepReceiverAlive();
	return {
		attempts,
		waits,
		unavailable: f.reports.filter(({ kind }) => kind === "receiver-lease-unavailable"),
	};
}

for (const [name, change] of [
	["non-live state", { state: "expired" }],
	["malformed lease duration", { lease_ms: "60000" }],
	["malformed renewal delay", { renew_after_ms: "20000" }],
	["renewal at lease expiry", { renew_after_ms: 60000 }],
]) {
	test(`receiver lease: ${name} enters failure backoff instead of renewing`, async (t) => {
		const result = await receiverLeaseScenario(t, [
			{ state: "live", lease_ms: 60000, renew_after_ms: 20000, ...change },
		]);
		assert.equal(result.attempts, 1);
		assert.deepEqual(result.waits, [250]);
		assert.equal(result.unavailable.length, 1);
	});
}

test("receiver lease: repeated failures emit one unavailable episode", async (t) => {
	const result = await receiverLeaseScenario(t, [
		new Error("connection refused"),
		new Error("request timed out"),
		new Error("connection reset"),
	]);
	assert.equal(result.attempts, 3);
	assert.equal(result.unavailable.length, 1);
});

test("receiver lease: failed renewals back off exponentially with a ceiling", async (t) => {
	const result = await receiverLeaseScenario(
		t,
		Array.from({ length: 8 }, () => new Error("connection refused")),
	);
	assert.equal(result.attempts, 8);
	assert.deepEqual(result.waits, [250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
});

test("receiver lease: recovery resets backoff and allows a new failure episode", async (t) => {
	const lease = { state: "live", lease_ms: 60000, renew_after_ms: 20000 };
	const result = await receiverLeaseScenario(t, [
		new Error("connection refused"),
		new Error("connection refused"),
		lease,
		new Error("connection reset"),
		new Error("connection reset"),
		lease,
	]);
	assert.equal(result.attempts, 6);
	assert.deepEqual(result.waits, [250, 500, 20000, 250, 500, 20000]);
	assert.equal(result.unavailable.length, 2);
});

test("receiver lease: stop ignores a late renewal response without rescheduling", async (t) => {
	const response = deferred();
	const waits = [];
	const f = fixture({
		async heartbeatDelay(ms, signal) {
			waits.push(ms);
			signal.throwIfAborted();
		},
	});
	t.after(() => f.bridge.stop());
	f.client.request = () => response.promise;
	const heartbeat = f.bridge.keepReceiverAlive();
	f.bridge.stop();
	response.resolve({ state: "live", lease_ms: 60000, renew_after_ms: 20000 });
	await heartbeat;
	assert.deepEqual(waits, []);
	assert.equal(f.reports.filter(({ kind }) => kind === "receiver-lease-unavailable").length, 0);
});

test("receiver lease: receive-held tears down renewal before a late timer fires", async (t) => {
	const tick = deferred();
	let heartbeatSignal;
	let renewals = 0;
	const f = fixture({
		async heartbeatDelay(_ms, signal) {
			heartbeatSignal = signal;
			await tick.promise;
		},
	});
	t.after(() => {
		f.bridge.stop();
		tick.resolve();
	});
	const request = f.client.request;
	f.client.request = (path, body, signal) => {
		if (path === "/v1/inbox/heartbeat" && ++renewals > 1) f.bridge.stop();
		return request(path, body, signal);
	};
	const read = f.native.rpc.eventLog.read;
	f.native.rpc.eventLog.read = async (input) =>
		input.direction === "backward"
			? read(input)
			: {
					events: [
						{ type: "user.message", id: "stale-user", data: { messageId: "native-message-137" } },
					],
					cursor: "expired",
					hasMore: false,
					cursorStatus: "expired",
				};
	await f.bridge.run();
	const abortedAtHold = heartbeatSignal?.aborted;
	tick.resolve();
	await f.bridge.heartbeat;
	assert.equal(abortedAtHold, true);
	assert.equal(renewals, 1);
	assert.equal(f.reports.filter(({ kind }) => kind === "receive-held").length, 1);
	assert.equal(f.trace.filter((entry) => entry === "unsubscribe").length, 1);
	assert.equal(f.trace.filter((entry) => entry === "send").length, 1);
	assert.equal(f.trace.includes("ack"), false);
	assert.equal(f.trace.includes("retry-intent"), false);
});

for (const blockedBy of ["busy completion", "human consent"]) {
	test(`receiver lease: renews for twenty minutes independently of ${blockedBy}`, async (t) => {
		const ticks = [];
		let renewals = 0;
		let receiveWaits = 0;
		const f = fixture({
			async delay(_ms, signal) {
				receiveWaits++;
				await new Promise((resolve) => signal.addEventListener("abort", resolve, { once: true }));
			},
			async heartbeatDelay(ms, signal) {
				const tick = deferred();
				ticks.push({ ms, ...tick });
				signal.addEventListener("abort", tick.resolve, { once: true });
				try {
					await tick.promise;
					signal.throwIfAborted();
				} finally {
					signal.removeEventListener("abort", tick.resolve);
				}
			},
		});
		t.after(() => f.bridge.stop());
		const request = f.client.request;
		f.client.request = (path, body, signal) => {
			if (path === "/v1/inbox/heartbeat") renewals++;
			return request(path, body, signal);
		};
		if (blockedBy === "human consent")
			f.client.claimInbox = async () => {
				f.trace.push("claim");
				return { claims: [], hold: { kind: "consent", reason: "human-consent" } };
			};
		const run = f.bridge.run();
		await flush();
		assert.equal(receiveWaits, 1, "receiving is blocked before the renewal clock advances");
		for (let step = 0; step < 60; step++) {
			assert.equal(renewals, step + 1);
			assert.equal(ticks.length, step + 1);
			assert.equal(ticks[step].ms, 20000);
			ticks[step].resolve();
			await flush();
		}
		assert.equal(renewals, 61, "the initial lease and all sixty renewals succeeded");
		assert.equal(receiveWaits, 1, "renewal did not depend on advancing the receive loop");
		assert.equal(f.trace.filter((entry) => entry === "claim").length, 1);
		assert.equal(
			f.trace.filter((entry) => entry === "ack").length,
			blockedBy === "busy completion" ? 1 : 0,
		);
		assert.equal(
			f.trace.filter((entry) => entry === "send").length,
			blockedBy === "busy completion" ? 1 : 0,
		);
		assert.equal(
			f.reports.filter(({ kind }) => ["receive-held", "receiver-lease-unavailable"].includes(kind))
				.length,
			0,
		);
		f.bridge.stop();
		await Promise.all([run, f.bridge.heartbeat]);
	});
}

for (const guard of [
	"events not an array",
	"more than 128 events",
	"non-string cursor",
	"nonboolean hasMore",
	"nonadvancing hasMore",
]) {
	test(`incremental history: ${guard} holds without ACK, reinjection or cursor rewind`, async (t) => {
		const f = fixture({ delay: () => flush() });
		t.after(() => f.bridge.stop());
		f.records.set(claim.message.msg_id, {
			state: "accepted",
			message: claim.message,
			nativeId: "guarded-native",
		});
		const proof = [
			{ type: "user.message", id: "guarded-user", data: { messageId: "guarded-native" } },
			{ type: "session.idle", id: "guarded-idle", parentId: "guarded-user", data: {} },
		];
		const malformed = { events: proof, cursor: "501", hasMore: false, cursorStatus: "ok" };
		if (guard === "events not an array") {
			// Strings are iterable: deleting Array.isArray must not pass via an incidental TypeError.
			malformed.events = "not-an-array";
			malformed.cursor = "0";
		} else if (guard === "more than 128 events") {
			malformed.events = [
				...Array.from({ length: 127 }, (_, i) => ({
					type: "tool.execution_complete",
					id: `unrelated-${i}`,
					data: {},
				})),
				...proof,
			];
		} else if (guard === "non-string cursor") {
			malformed.cursor = undefined;
		} else if (guard === "nonboolean hasMore") {
			malformed.hasMore = "false";
		} else {
			malformed.cursor = "500";
			malformed.hasMore = true;
		}
		const cursors = [];
		f.native.rpc.eventLog.tail = async () => ({ cursor: "tail" });
		const read = f.native.rpc.eventLog.read;
		f.native.rpc.eventLog.read = async (input) => {
			if (input.direction === "backward") return read(input);
			const { cursor } = input;
			cursors.push(cursor);
			if (cursors.length === 1)
				return { events: [], cursor: "500", hasMore: true, cursorStatus: "ok" };
			if (cursors.length === 2) return malformed;
			// A removed guard gets one observable bad read, never an unbounded replay loop.
			f.bridge.stop();
			return { events: [], cursor: "501", hasMore: false, cursorStatus: "ok" };
		};
		let claims = 0;
		f.client.claimInbox = async () => {
			if (claims++ === 0) return { claims: [claim], hold: null };
			f.bridge.stop();
			return { claims: [], hold: null };
		};
		await f.bridge.run();
		assert.deepEqual(cursors, [undefined, "500"]);
		assert.equal(f.reports.filter(({ kind }) => kind === "receive-held").length, 1);
		assert.equal(f.trace.includes("ack"), false);
		assert.equal(f.trace.includes("send"), false);
		assert.equal(f.trace.includes("retry-intent"), false);
	});
}
