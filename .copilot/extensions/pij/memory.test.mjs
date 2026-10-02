import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { setImmediate as flush } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { NativeBridge } from "./store.mjs";

// Run the real bridge in a constrained, GC-observable process. The native seam
// stores history as wire bytes, not parsed objects artificially kept by the test.
if (!process.env.PIJ_NATIVE_MEMORY_CHILD) {
	test("50 deliveries and a 20k-event accepted replay over 60 MB history retain less than 32 MiB", () => {
		const env = { ...process.env, PIJ_NATIVE_MEMORY_CHILD: "1" };
		delete env.NODE_TEST_CONTEXT;
		const result = spawnSync(
			process.execPath,
			["--expose-gc", "--max-old-space-size=512", fileURLToPath(import.meta.url)],
			{
				env,
				encoding: "utf8",
				timeout: 120000,
				maxBuffer: 1024 * 1024,
			},
		);
		console.log(result.stdout);
		assert.equal(result.status, 0, `${result.signal ?? ""}\n${result.stderr}\n${result.stdout}`);
	});
} else {
	test("bounded memory during delivery, completion and discard observation", async (t) => {
		const body =
			"A realistic tool result line describing a source file and its diagnostics.\n".repeat(47);
		const wire = Buffer.from(
			JSON.stringify(
				Array.from({ length: 20000 }, (_, i) => ({
					type: "tool.execution_complete",
					id: `historic-${i}`,
					parentId: `historic-${i - 1}`,
					data: { toolCallId: `tool-${i}`, result: { content: `${i}: ${body}` } },
				})),
			),
		);
		assert.ok(wire.byteLength >= 60 * 1024 * 1024);
		const registration = {
			id: "pij-memory",
			harness: "copilot",
			harness_session: "memory-session",
			pid: 150,
			proc_start: 150,
		};
		let observer;
		let sends = 0;
		let claims = 0;
		let acknowledgements = 0;
		let polls = 0;
		let fullReads = 0;
		const recent = [];
		let record;
		const emit = (event) => {
			recent.push(event);
			observer(event);
		};
		const native = {
			on(fn) {
				observer = fn;
				return () => {};
			},
			async getEvents() {
				fullReads++;
				return [...JSON.parse(wire.toString()), ...recent];
			},
			rpc: {
				eventLog: {
					async tail() {
						return { cursor: String(recent.length) };
					},
					async read({ cursor = "0", max }) {
						const events = recent.slice(Number(cursor), Number(cursor) + max);
						return {
							events,
							cursor: String(Number(cursor) + events.length),
							hasMore: Number(cursor) + events.length < recent.length,
							cursorStatus: "ok",
						};
					},
				},
				queue: {
					async pendingItems() {
						return { items: [], steeringMessages: [] };
					},
				},
				metadata: {
					async isProcessing() {
						return { processing: true };
					},
				},
			},
			async send() {
				const id = `native-${++sends}`;
				if (sends < 50) {
					emit({ type: "user.message", id: `user-${id}`, data: { messageId: id } });
					emit({ type: "assistant.message", id: `answer-${id}`, parentId: `user-${id}`, data: {} });
					emit({ type: "assistant.turn_end", id: `end-${id}`, parentId: `answer-${id}`, data: {} });
				} else emit({ type: "session.idle", id: "foreground-boundary", data: {} });
				return id;
			},
		};
		const samples = [];
		await flush();
		global.gc();
		const baseline = process.memoryUsage().heapUsed;
		const bridge = new NativeBridge({
			registration,
			native,
			journal: {
				async load(id) {
					return record?.message.msg_id === id ? record : undefined;
				},
				async begin(message) {
					record = { state: "pending", message };
					return true;
				},
				async accept(message, nativeId) {
					record = { state: "accepted", message, nativeId };
				},
			},
			client: {
				async nativeSnapshot(consumer) {
					return {
						native_consumer: {
							native_session: consumer.native_session,
							pid: consumer.pid,
							proc_start: consumer.proc_start,
						},
					};
				},
				async claimInbox() {
					return {
						claims: [
							{
								job_id: ++claims,
								message: {
									msg_id: `msg-${claims}`,
									from: "pij-sender",
									to: registration.id,
									body: "memory proof",
								},
								native_consumer: {
									native_session: registration.harness_session,
									pid: registration.pid,
									proc_start: registration.proc_start,
								},
							},
						],
						hold: null,
					};
				},
				async request(path, input) {
					if (path === "/v1/inbox/heartbeat")
						return { state: "live", lease_ms: 60000, renew_after_ms: 20000 };
					if (path === "/v1/register")
						return {
							...registration,
							session: registration.harness_session,
							proc: { pid: registration.pid, proc_start: registration.proc_start },
							native_extension_delivery: true,
						};
					assert.equal(path, "/v1/inbox/ack");
					acknowledgements++;
					return input.job_id;
				},
			},
			report(event) {
				if (event.kind === "receive-held") assert.fail(event.diagnostic);
			},
			async delay() {
				await flush();
				global.gc();
				const heap = process.memoryUsage().heapUsed;
				samples.push(heap);
				console.log(
					JSON.stringify({ sends, polls: ++polls, fullReads, heap, growth: heap - baseline }),
				);
				if (polls === 50) bridge.stop();
			},
		});
		t.after(() => bridge.stop());
		console.log(JSON.stringify({ events: 20000, serializedBytes: wire.byteLength, baseline }));
		await bridge.run();
		assert.equal(sends, 50);
		assert.equal(acknowledgements, 49);
		assert.equal(polls, 50);
		assert.ok(
			Math.max(...samples) - baseline < 32 * 1024 * 1024,
			"retained history grew beyond 32 MiB",
		);
	});

	test("accepted-journal replay bounds forced-GC peak across every history page", async (t) => {
		const historyEvents = 20000;
		const body =
			"A realistic tool result line describing a source file and its diagnostics.\n".repeat(47);
		const historicEvent = (i) => ({
			type: "tool.execution_complete",
			id: `historic-${i}`,
			parentId: `historic-${i - 1}`,
			data: { toolCallId: `tool-${i}`, result: { content: `${i}: ${body}` } },
		});
		let historyBytes = 0;
		for (let i = 0; i < historyEvents; i++)
			historyBytes += Buffer.byteLength(JSON.stringify(historicEvent(i)));
		assert.ok(historyBytes >= 60 * 1024 * 1024);
		const registration = {
			id: "pij-memory",
			harness: "copilot",
			harness_session: "memory-session",
			pid: 150,
			proc_start: 150,
		};
		const message = {
			msg_id: "message-replayed",
			from: "pij-sender",
			to: registration.id,
			body: "accepted before restart",
		};
		const record = { state: "accepted", message, nativeId: "native-replayed" };
		const proof = [
			{ type: "user.message", id: "replayed-user", data: { messageId: record.nativeId } },
			{ type: "session.idle", id: "replayed-idle", parentId: "replayed-user", data: {} },
		];
		let pages = 0;
		let eventsRead = 0;
		let startupPages = 0;
		let startupEventsRead = 0;
		let sends = 0;
		let acknowledgements = 0;
		let fullReads = 0;
		let gcSamples = 0;
		const reports = [];
		await flush();
		global.gc();
		const baseline = process.memoryUsage().heapUsed;
		let peak = baseline;
		const sample = () => {
			global.gc();
			gcSamples++;
			peak = Math.max(peak, process.memoryUsage().heapUsed);
		};
		const native = {
			on() {
				return () => {};
			},
			async getEvents() {
				fullReads++;
				throw new Error("accepted replay must use bounded eventLog pages");
			},
			async send() {
				sends++;
				throw new Error("accepted replay must not reinject");
			},
			rpc: {
				eventLog: {
					async tail() {
						return { cursor: String(historyEvents + proof.length) };
					},
					async read({ cursor, max, direction }) {
						if (direction === "backward") {
							sample();
							const from = Math.max(0, historyEvents + proof.length - max);
							const events = JSON.parse(
								JSON.stringify([
									...Array.from({ length: historyEvents - from }, (_, index) =>
										historicEvent(from + index),
									),
									...proof,
								]),
							);
							startupPages++;
							startupEventsRead += events.length;
							sample();
							return {
								events,
								cursor: "startup-backward-tail",
								hasMore: from > 0,
								cursorStatus: "ok",
							};
						}
						// Drain can stay in microtasks throughout: a timer cannot measure its peak.
						// Sample before allocation to catch pages retained by the previous read,
						// and after allocation to include the current RPC window.
						sample();
						const from = cursor === undefined ? 0 : Number(cursor);
						assert.equal(from, eventsRead, "replay must neither skip nor rewind history");
						assert.ok(Number.isSafeInteger(max) && max > 0 && max <= 128);
						const count = Math.min(max, historyEvents - from);
						const events = JSON.parse(
							JSON.stringify(
								from < historyEvents
									? Array.from({ length: count }, (_, i) => historicEvent(from + i))
									: proof,
							),
						);
						pages++;
						eventsRead += events.length;
						sample();
						return {
							events,
							cursor: String(eventsRead),
							hasMore: eventsRead < historyEvents + proof.length,
							cursorStatus: "ok",
						};
					},
				},
			},
		};
		let claims = 0;
		const bridge = new NativeBridge({
			registration,
			native,
			journal: {
				async load(id) {
					assert.equal(id, message.msg_id);
					return record;
				},
				async begin() {
					throw new Error("accepted replay must not create another send intent");
				},
				async accept() {
					throw new Error("accepted replay must not replace durable acceptance");
				},
			},
			client: {
				async claimInbox() {
					if (claims++ > 0) {
						bridge.stop();
						return { claims: [], hold: null };
					}
					return {
						claims: [
							{
								job_id: 150,
								message,
								native_consumer: {
									native_session: registration.harness_session,
									pid: registration.pid,
									proc_start: registration.proc_start,
								},
							},
						],
						hold: null,
					};
				},
				async request(path, input) {
					if (path === "/v1/inbox/heartbeat")
						return { state: "live", lease_ms: 60000, renew_after_ms: 20000 };
					if (path === "/v1/register")
						return {
							...registration,
							session: registration.harness_session,
							proc: { pid: registration.pid, proc_start: registration.proc_start },
							native_extension_delivery: true,
						};
					assert.equal(path, "/v1/inbox/ack");
					assert.equal(input.job_id, 150);
					acknowledgements++;
					return input.job_id;
				},
			},
			report: (event) => reports.push(event),
			async delay() {
				throw new Error("replay stopped paging before consumption and completion");
			},
		});
		t.after(() => bridge.stop());
		await bridge.run();
		sample();
		console.log(
			JSON.stringify({
				historyEvents,
				historyBytes,
				pages,
				eventsRead,
				startupPages,
				startupEventsRead,
				gcSamples,
				acknowledgements,
				sends,
				fullReads,
				baseline,
				peak,
				finalHeap: process.memoryUsage().heapUsed,
				growth: peak - baseline,
			}),
		);
		assert.equal(reports.filter(({ kind }) => kind === "receive-held").length, 0);
		assert.equal(eventsRead, historyEvents + proof.length);
		assert.ok(pages >= Math.ceil(historyEvents / 128) + 1);
		assert.equal(acknowledgements, 1);
		assert.equal(sends, 0);
		assert.equal(fullReads, 0);
		assert.ok(peak - baseline < 32 * 1024 * 1024, "paged replay retained more than 32 MiB");
	});
}
