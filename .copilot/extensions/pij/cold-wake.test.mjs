// Plan 157 phase 2 — the Copilot pij_send tool's side of the cold-wake guard:
// force + reason reach the /v1/send wire, a reasonless force sends nothing,
// and the daemon's refusal reaches the model verbatim (it names the price).
// Plan 159 — a held FYI that looks like a question surfaces the daemon's warning.
import assert from "node:assert/strict";
import test from "node:test";
import { DaemonClient, NativeBridge } from "./store.mjs";

const COLD_META =
	'E-RS-COLD-WAKE: pij-peer is cold (idle 1h52m, 720k context). Waking it rewrites ~720k tokens ≈ $5.76 at list price. Use --fyi to hold it, or --force --reason "<why>".';

/** A registered bridge over a real DaemonClient whose /v1/send answers `answer`. */
function bridgeOver(answer) {
	const sends = [];
	const client = new DaemonClient({
		addr: "127.0.0.1:1",
		stateDir: "/isolated",
		readKey: async () => "fixture-key",
		fetch: async (url, init) => {
			assert.equal(new URL(url).pathname, "/v1/send");
			const body = JSON.parse(init.body);
			sends.push(body);
			return answer(body);
		},
	});
	const bridge = new NativeBridge({
		registration: { id: "pij-copilot", harness_session: "native-1" },
		native: {},
		client,
		journal: {},
		report: () => {},
	});
	bridge.registered = true;
	return { bridge, sends };
}

const accepted = (body) =>
	new Response(JSON.stringify({ ok: true, v: 2, command: "pij send", data: body }));

test("force and reason reach the /v1/send wire", async () => {
	const { bridge, sends } = bridgeOver((body) =>
		accepted({ msg_id: body.msg_id, outcome: "queued", at: 1, cold_check: "forced" }),
	);
	const result = await bridge.send({
		to: "pij-peer",
		message: "wake",
		force: true,
		reason: "release blocker",
	});
	assert.equal(result.ok, true);
	assert.equal(sends[0].force, true);
	assert.equal(sends[0].reason, "release blocker");
});

for (const [label, extra] of [
	["no reason", {}],
	["a blank reason", { reason: "   " }],
	["fyi", { reason: "why", fyi: true }],
]) {
	test(`force with ${label} is refused before any request`, async () => {
		const { bridge, sends } = bridgeOver(() => assert.fail("nothing may be sent"));
		const result = await bridge.send({ to: "pij-peer", message: "wake", force: true, ...extra });
		assert.equal(result.ok, false);
		assert.match(result.error, /^E-RS-COLD-WAKE: /);
		assert.equal(sends.length, 0);
	});
}

test("a cold refusal reaches the model verbatim and says nothing was sent", async () => {
	const { bridge } = bridgeOver(
		() =>
			new Response(
				JSON.stringify({
					ok: false,
					v: 2,
					command: "pij send",
					error: "refused",
					meta: COLD_META,
				}),
				{ status: 400 },
			),
	);
	const result = await bridge.send({ to: "pij-peer", message: "wake" });
	assert.equal(result.ok, false);
	assert.equal(result.error, COLD_META);
	assert.equal(result.grade, "refused-not-sent");
});

test("a held fyi's question warning is surfaced on the tool result", async () => {
	const warning = "this looks like a question; if you need an answer, resend without --fyi";
	const { bridge, sends } = bridgeOver((body) =>
		accepted({ msg_id: body.msg_id, outcome: { outcome: "held", reason: "fyi" }, at: 1, warning }),
	);
	const result = await bridge.send({ to: "pij-peer", message: "can you check X?", fyi: true });
	assert.equal(sends[0].fyi, true);
	assert.equal(result.ok, true);
	assert.equal(result.warning, warning);
});
