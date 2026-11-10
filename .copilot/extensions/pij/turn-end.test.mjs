// Plan 226: a reply that ends with a pij block runs it at the natural end of the
// run. The tracker picks the root agent's final reply; the bridge calls the
// daemon only when that reply ends with a pij fence, shows one line, and asks
// for one follow-up turn only on failure and only when this stop is not one.
import assert from "node:assert/strict";
import test from "node:test";
import {
	clipReason,
	DaemonClient,
	endsWithPijFence,
	NativeBridge,
	TURN_END_REASON_CHARS,
	TURN_END_ROUTE,
	TurnEndTracker,
} from "./store.mjs";

const BLOCK =
	'Merged.\n\n```pij\nsend pij-steady-otter "MERGED #210"\noutput verdict --result pass\n```\n';
const registration = {
	id: "pij-copilot",
	harness_session: "native-1",
	pane: "%7",
	folder: "/work/tree",
};

/** A registered bridge whose /v1/turn-end answers `answer`; logs and calls recorded. */
function bridgeOver(answer) {
	const calls = [];
	const logs = [];
	const client = new DaemonClient({
		addr: "127.0.0.1:1",
		stateDir: "/isolated",
		readKey: async () => "fixture-key",
		fetch: async (url, init) => {
			calls.push({ path: new URL(url).pathname, body: JSON.parse(init.body) });
			return answer();
		},
	});
	const bridge = new NativeBridge({
		registration,
		native: { log: async (line, options) => logs.push({ line, level: options?.level }) },
		client,
		journal: {},
		report: () => {},
	});
	bridge.registered = true;
	return { bridge, calls, logs };
}

const answered = (data) =>
	new Response(JSON.stringify({ ok: true, v: 2, command: "pij turn-end", data }));
const RAN = {
	outcome: "ran",
	lines: [],
	ran: 2,
	summary: "pij ✓ 1 sent, 1 output",
	followup: null,
};
const REFUSED = {
	outcome: "refused",
	lines: [],
	ran: 1,
	summary: "pij ✗ line 2 refused (1 ran)",
	followup: "Ran, do not resend: line 1. Refused: line 2. Not run: none.",
};

test("the tail check wants a closing fence and a pij opening", () => {
	assert.equal(endsWithPijFence(BLOCK), true);
	assert.equal(endsWithPijFence('  ```pij\nsend a "b"\n  ```   \n\n'), true);
	assert.equal(endsWithPijFence('Example:\n```pij\nsend a "b"\n```\nAnd prose after.'), false);
	assert.equal(endsWithPijFence("```bash\nls\n```"), false);
	assert.equal(endsWithPijFence("plain"), false);
	assert.equal(endsWithPijFence(undefined), false);
});

test("the tracker keeps the root agent's last reply of the run, once", () => {
	const turnEnd = new TurnEndTracker();
	const message = (messageId, content, agentId) => ({
		type: "assistant.message",
		...(agentId ? { agentId } : {}),
		data: { messageId, content },
	});
	turnEnd.observe({ type: "user.message", data: {} });
	turnEnd.observe(message("m1", "calling a tool"));
	turnEnd.observe(message("m2", BLOCK));
	turnEnd.observe(message("s1", "sub-agent reply", "agent-9"));
	assert.equal(turnEnd.take(), BLOCK, "the sub-agent's later message is not the run's reply");
	assert.equal(turnEnd.take(), undefined, "taken once");
	turnEnd.observe(message("m2", BLOCK));
	assert.equal(turnEnd.take(), undefined, "a replayed message never runs twice");

	turnEnd.observe(message("m3", BLOCK));
	turnEnd.observe({ type: "user.message", data: {} });
	assert.equal(turnEnd.take(), undefined, "a new prompt starts a new run");
	turnEnd.observe(message("m4", BLOCK));
	turnEnd.observe({ type: "abort", data: {} });
	assert.equal(turnEnd.take(), undefined, "an aborted run has no final reply");
});

test("a reply without a pij block makes no daemon call", async () => {
	const { bridge, calls, logs } = bridgeOver(() => assert.fail("no call"));
	for (const text of [undefined, "plain", 'Example:\n```pij\nsend a "b"\n```\nmore'])
		assert.equal(await bridge.turnEnd(text, false), undefined);
	assert.deepEqual(calls, []);
	assert.deepEqual(logs, []);
});

test("a block that ran shows its summary and asks for nothing", async () => {
	const { bridge, calls, logs } = bridgeOver(() => answered(RAN));
	assert.equal(await bridge.turnEnd(BLOCK, false), undefined);
	assert.deepEqual(calls, [
		{
			path: TURN_END_ROUTE,
			body: {
				text: BLOCK,
				caller: {
					PIJ_SESSION_ID: "pij-copilot",
					HARNESS_SESSION_ID: "native-1",
					TMUX_PANE: "%7",
					cwd: "/work/tree",
				},
				via: "hook:copilot",
			},
		},
	]);
	assert.deepEqual(logs, [{ line: "pij ✓ 1 sent, 1 output", level: "info" }]);
});

test("a failed block gets one follow-up turn naming what ran, never a second", async () => {
	const { bridge, logs } = bridgeOver(() => answered(REFUSED));
	assert.deepEqual(await bridge.turnEnd(BLOCK, false), {
		decision: "block",
		reason: REFUSED.followup,
	});
	assert.equal(await bridge.turnEnd(BLOCK, true), undefined, "a follow-up's own stop never blocks");
	assert.deepEqual(logs, [
		{ line: REFUSED.summary, level: "warning" },
		{ line: `${REFUSED.summary} (no follow-up: this turn was already one)`, level: "warning" },
	]);
});

test("a daemon refusal ran nothing; a lost call may have run some", async () => {
	const refused = bridgeOver(
		() => new Response(JSON.stringify({ ok: false, v: 2, error: "not found" }), { status: 404 }),
	);
	const followUp = await refused.bridge.turnEnd(BLOCK, false);
	assert.equal(followUp.decision, "block");
	assert.match(followUp.reason, /Nothing was sent\.\nFix it and end your reply/);
	assert.match(
		refused.logs[0].line,
		/^pij ✗ block not run: the daemon refused it \(Pij HTTP 404\)$/,
	);

	const lost = bridgeOver(() => {
		throw new TypeError("fetch failed");
	});
	const unknown = await lost.bridge.turnEnd(BLOCK, false);
	assert.match(unknown.reason, /some of its commands may have run\.\nCheck before you resend/);
	assert.match(lost.logs[0].line, /^pij \? block outcome unknown: /);
});

test("a no-result reason is one escaped, clipped line", () => {
	assert.equal(clipReason("  bad\u001b[31m body \nsecond"), "bad\\u{1b}[31m body");
	assert.equal(
		clipReason("x".repeat(TURN_END_REASON_CHARS + 5)),
		`${"x".repeat(TURN_END_REASON_CHARS)}…`,
	);
});

test("a stopped bridge neither calls nor reports", async () => {
	const { bridge, calls, logs } = bridgeOver(() => answered(RAN));
	bridge.registered = false;
	assert.equal(await bridge.turnEnd(BLOCK, false), undefined);
	assert.deepEqual(calls, []);
	assert.deepEqual(logs, []);
});
