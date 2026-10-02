import assert from "node:assert/strict";
import test from "node:test";
import { createNativeReporter, DaemonClient, NativeError } from "./store.mjs";

test("transient registration hold is silent before ten seconds and reports once afterward", async () => {
	const logs = [];
	const report = createNativeReporter({
		log: async (message) => logs.push(message),
		capture: () => {},
	});
	const hold = (elapsedMs) =>
		report({ kind: "registration-wait", holdKind: "native-session", elapsedMs, retryMs: 5000 });
	await hold(0);
	await hold(9999);
	assert.deepEqual(logs, [], "a transient hold must not print an unavailable banner");
	await hold(10000);
	await hold(15000);
	assert.equal(logs.length, 1);
	assert.match(logs[0], /waiting for this pane's resumed Copilot session/);
	assert.doesNotMatch(logs[0], /unavailable|restart the Copilot CLI/);
});

test("registration hold escalates once at ten minutes even when the initial notice was skipped", async () => {
	for (const initialNotice of [true, false]) {
		const logs = [];
		const report = createNativeReporter({
			log: async (message) => logs.push(message),
			capture: () => {},
		});
		const hold = (elapsedMs) =>
			report({ kind: "registration-wait", holdKind: "native-session", elapsedMs });
		if (initialNotice) {
			await hold(10000);
			await hold(599999);
			assert.deepEqual(logs, ["[pij native] waiting for this pane's resumed Copilot session"]);
		}
		await hold(600000);
		assert.equal(
			logs.at(-1),
			"[pij native] still waiting for this pane's resumed Copilot session after 10 min; pij delivery is unavailable in this window",
		);
		for (const elapsedMs of [600001, 3600000, 3660000, 7200000]) await hold(elapsedMs);
		assert.equal(logs.length, Number(initialNotice) + 1);
		assert.doesNotMatch(logs.join("\n"), /restart the Copilot CLI/);
	}
});

test("native diagnostic sink counts one call per failure episode and never logs raw remote secrets", async () => {
	const lines = [];
	const report = createNativeReporter({
		log: async (message, options) => lines.push({ message, options }),
		capture: () => undefined,
	});
	for (let i = 0; i < 5; i++)
		await report({ kind: "reconnecting", diagnostic: "SECRET_SERVER_BODY" });
	assert.equal(lines.length, 1);
	await report({ kind: "connection-ready" });
	await report({ kind: "reconnecting", diagnostic: "SECRET_SERVER_BODY" });
	assert.equal(lines.length, 2);
	assert.ok(
		lines.every(
			(line) =>
				line.message.startsWith("[pij native] unavailable:") && !line.message.includes("SECRET"),
		),
	);
});

test("registration wait episodes reset on success and do not hide unrelated failures", async () => {
	const logs = [];
	const captured = [];
	const report = createNativeReporter({
		log: async (message) => logs.push(message),
		capture: (event) => captured.push(event),
	});
	const hold = (elapsedMs) =>
		report({
			kind: "registration-wait",
			holdKind: "native-session",
			elapsedMs,
			retryMs: 5000,
			diagnostic: "SECRET_BODY",
			safeDiagnostic: "SECRET_KEY",
		});
	for (const transition of [
		"registered",
		"connection-ready",
		"registration-wait",
		"reconnecting",
		"receive-held",
		"extension-unavailable",
	]) {
		await hold(10000);
		await hold(600000);
		const before = logs.length;
		const failure = !["registered", "connection-ready"].includes(transition);
		await report({ kind: transition, ...(failure ? { diagnostic: "SECRET_BODY" } : {}) });
		assert.equal(logs.length, before + Number(failure), transition);
		if (failure) assert.match(logs.at(-1), /unavailable/);
		const after = logs.length;
		await hold(0);
		await hold(9999);
		assert.equal(logs.length, after, transition);
		await hold(10000);
		await hold(15000);
		assert.equal(logs.length, after + 1, transition);
		assert.equal(logs.at(-1), "[pij native] waiting for this pane's resumed Copilot session");
		await hold(599999);
		assert.equal(logs.length, after + 1, transition);
		await hold(600000);
		await hold(7200000);
		assert.equal(logs.length, after + 2, transition);
		assert.match(logs.at(-1), /still waiting.*after 10 min; pij delivery is unavailable/);
	}
	assert.doesNotMatch(JSON.stringify({ logs, captured }), /SECRET_BODY|SECRET_KEY/);
	assert.deepEqual(
		captured.find((event) => event.holdKind === "native-session"),
		{ kind: "registration-wait", holdKind: "native-session", elapsedMs: 10000, retryMs: 5000 },
	);
});

test("only a typed retryable registration 409 becomes a native-session hold", async () => {
	const hold = {
		v: 2,
		ok: false,
		command: "pij register",
		error: "refused",
		details: { retryable: true, hold: "native-session" },
		meta: "native Copilot registration held: pane owned by seat `pij-owner`; awaiting resumed session",
		data: { body: "SECRET_BODY", key: "fixture-key" },
	};
	for (const { label, path = "/v1/register", status = 409, change = {}, held = false } of [
		{ label: "typed hold", held: true },
		{ label: "generic registration retry", change: { details: { retryable: true } } },
		{ label: "foreign hold kind", change: { details: { retryable: true, hold: "native-target" } } },
		{ label: "terminal refusal", status: 400 },
		{
			label: "untyped retryability",
			change: { details: { retryable: "true", hold: "native-session" } },
		},
		{ label: "wrong version", change: { v: 1 } },
		{ label: "wrong command", change: { command: "pij inbox" } },
		{ label: "wrong error", change: { error: "invalid-request" } },
		{ label: "success-shaped body", change: { ok: true } },
		{ label: "wrong endpoint", path: "/v1/send" },
	]) {
		const client = new DaemonClient({
			addr: "127.0.0.1:1",
			stateDir: "/isolated",
			readKey: async () => "fixture-key",
			fetch: async () => new Response(JSON.stringify({ ...hold, ...change }), { status }),
		});
		await assert.rejects(client.request(path, {}), (error) => {
			assert.ok(error instanceof NativeError, label);
			assert.equal(error.holdKind, held ? "native-session" : undefined, label);
			if (held || label === "generic registration retry")
				assert.equal(error.retryable, true, label);
			assert.equal(error.safeDiagnostic, `Pij HTTP ${status}`, label);
			assert.doesNotMatch(error.message, /fixture-key/, label);
			return true;
		});
	}
});
