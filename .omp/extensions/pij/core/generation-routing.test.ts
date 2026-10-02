import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { COPILOT_CONTROL_REFUSAL } from "./commands.js";
import {
	classifyRsResponse,
	copilotControlRefusal,
	decidePreCall,
	describeOutcome,
	findRoute,
	LEAF_CLOSED_VERBS,
	type RouteOutcome,
	RS_ROUTE_TABLE,
	type RsHttpRoute,
	type RsResponseFacts,
	refusalText,
	renderRsAnswer,
	resolveGenerationForce,
	routingRefusalEnvelope,
} from "./generation-routing.js";

const ADDR = "127.0.0.1:18741";
const STATE_DIR = "/tmp/pij-routing-core-fixture";
const ROW: RsHttpRoute = {
	verb: "adopt",
	rsPath: "/v1/adopt",
	method: "POST",
	why: "test row",
};

const pre = (over: Partial<Parameters<typeof decidePreCall>[0]> = {}) =>
	decidePreCall({ verb: "adopt", force: undefined, addr: ADDR, rsLive: true, ...over });

describe("Copilot native control boundary", () => {
	it.each(["compact", "new", "reload"])("refuses programmatic %s only for Copilot", (command) => {
		expect(copilotControlRefusal("copilot", command)).toBe(COPILOT_CONTROL_REFUSAL);
		for (const harness of ["pi", "omp", "claude", "codex", undefined]) {
			expect(copilotControlRefusal(harness, command)).toBeUndefined();
		}
	});

	it("does not invent a protocol for bodies or unknown controls", () => {
		expect(copilotControlRefusal("copilot", "hello")).toBeUndefined();
		expect(copilotControlRefusal("copilot", "compact with instructions")).toBeUndefined();
	});
});

describe("the explicit rs routing inventory", () => {
	it.each(["create", "list", "tail", "kill"])("bg %s remains daemon-owned", (leaf) => {
		expect(findRoute("bg", leaf)).toMatchObject({ rsPath: "/v1/bg", method: "POST" });
		expect(pre({ verb: "bg", leaf })).toMatchObject({ kind: "try-rs" });
	});

	it("unknown verbs and declared unported verbs are both named refusals", () => {
		expect(findRoute("unknown-command", undefined)).toBeUndefined();
		expect(findRoute("spawn", undefined)).toHaveProperty("unported");
		for (const verb of ["unknown-command", "spawn"]) {
			expect(pre({ verb })).toMatchObject({ kind: "rs-error", verb, code: "E-RS-UNPORTED" });
		}
	});

	it("adding an HTTP row needs no decision-function change", () => {
		const extended = [...RS_ROUTE_TABLE, { ...ROW, verb: "fixture-command" }];
		expect(
			decidePreCall(
				{ verb: "fixture-command", force: undefined, addr: ADDR, rsLive: true },
				extended,
			),
		).toMatchObject({ kind: "try-rs", row: { verb: "fixture-command" } });
	});

	it("an exact leaf wins over the family regardless of row ordering", () => {
		const family: RsHttpRoute = { ...ROW, verb: "report", why: "family" };
		const leaf: RsHttpRoute = { ...family, leaf: "verify", why: "leaf" };
		for (const table of [
			[family, leaf],
			[leaf, family],
		]) {
			expect(findRoute("report", "verify", table)).toBe(leaf);
			expect(findRoute("report", "now", table)).toBe(family);
		}
	});

	it.each([
		["list", "/v1/seats"],
		["sessions", "/v1/shim/sessions"],
	])("%s uses the shipped GET endpoint", (verb, rsPath) => {
		expect(findRoute(verb, undefined)).toMatchObject({ rsPath, method: "GET" });
	});

	it("whoami --json preserves the complete original envelope, not a projection", () => {
		const payload = { id: "pij-rs-seat", harness: "omp", extension: { retained: true } };
		const rawEnvelope = ` {"ok":true,"command":"pij whoami","v":2,"data":${JSON.stringify(payload)},"meta":"keep me"}\n`;
		expect(
			renderRsAnswer(findRoute("whoami", undefined), payload, ["whoami", "--json"], rawEnvelope),
		).toEqual({ kind: "json", text: rawEnvelope });
		expect(renderRsAnswer(findRoute("whoami", undefined), payload, ["whoami", "--json"]).kind).toBe(
			"json-refused",
		);
	});

	it("human send receipts expose native receiver refusal reasons", () => {
		const reason =
			"native-extension-unavailable: native receiver for pij-copilot has not renewed its lease";
		const rendered = renderRsAnswer(
			findRoute("send", undefined),
			{
				msg_id: "native-refused",
				outcome: { outcome: "refused", reason },
			},
			["send", "pij-copilot", "hello"],
		);
		expect(rendered.kind).toBe("rendered");
		if (rendered.kind !== "rendered") throw new Error("send receipt was not rendered");
		expect(rendered.text).toContain("native-refused");
		expect(rendered.text).toContain("refused");
		expect(rendered.text).toContain(reason);
	});

	it("human send receipts show the cold-wake guard's verdict", () => {
		const rendered = renderRsAnswer(
			findRoute("send", undefined),
			{ msg_id: "m-1", outcome: "queued", at: 1, cold_check: "unknown: no answer within 3s" },
			["send", "pij-peer", "hello"],
		);
		expect(rendered).toEqual({
			kind: "rendered",
			text: "sent (rs) — msg m-1 — receipt queued — cold-check: unknown: no answer within 3s",
		});
	});

	it("a held fyi receipt shows the daemon's question warning", () => {
		const warning = "this looks like a question; if you need an answer, resend without --fyi";
		const rendered = renderRsAnswer(
			findRoute("send", undefined),
			{ msg_id: "m-2", outcome: { outcome: "held", reason: "fyi" }, at: 1, warning },
			["send", "pij-peer", "--fyi", "can you check X?"],
		);
		expect(rendered).toEqual({
			kind: "rendered",
			text: `pij send: held (fyi) — m-2\n${warning}`,
		});
	});

	it("a cold-wake refusal prints its meta alone, naming the price and both ways forward", () => {
		const meta =
			'E-RS-COLD-WAKE: pij-peer is cold (idle 1h52m, 720k context). Waking it rewrites ~720k tokens ≈ $5.76 at list price. Use --fyi to hold it, or --force --reason "<why>".';
		expect(refusalText({ kind: "rs-error", verb: "send", addr: ADDR, detail: meta })).toBe(meta);
	});

	it("other refusals keep the E-RS prefix and the daemon address", () => {
		expect(
			refusalText({ kind: "rs-error", verb: "send", addr: ADDR, detail: "no such seat" }),
		).toBe(`E-RS: no such seat (rs at ${ADDR})`);
	});

	it("human state exposes counted delivery deferrals and drops them after completion", () => {
		const card = {
			id: "pij-waiting",
			state: "idle",
			liveness: "active",
			deliveryDeferrals: [
				{ job_id: 12, msg_id: "waiting", reason: "unrecognized", count: 1029, since_ms: 1000 },
			],
		};
		const render = (payload: unknown) =>
			renderRsAnswer(findRoute("state", undefined), payload, ["state", "pij-waiting"]);
		const waiting = render(card);
		expect(waiting.kind).toBe("rendered");
		if (waiting.kind !== "rendered") throw new Error("state was not rendered");
		expect(waiting.text).toContain("delivery deferred: unrecognized ×1029 since 1000 ms");
		const completed = render({ ...card, deliveryDeferrals: [] });
		expect(completed.kind).toBe("rendered");
		if (completed.kind !== "rendered") throw new Error("state was not rendered");
		expect(completed.text).not.toContain("delivery deferred:");
	});

	it("human state exposes stalled native observation beside active host liveness", () => {
		const rendered = renderRsAnswer(
			findRoute("state", undefined),
			{
				id: "pij-copilot",
				state: "idle",
				liveness: "active",
				native_receiver_reason: "native-receiver-stale",
			},
			["state", "pij-copilot"],
		);
		expect(rendered.kind).toBe("rendered");
		if (rendered.kind !== "rendered") throw new Error("state was not rendered");
		expect(rendered.text).toContain("idle · active");
		expect(rendered.text).toContain("native-receiver-stale");
	});
});

describe("the tail false friend remains refused", () => {
	it.each([
		undefined,
		"rs",
	] as const)("does not alias peer transcripts to daemon events (force %s)", (force) => {
		expect(findRoute("tail", undefined)).toMatchObject({
			unported: expect.stringContaining("not the native daemon event tail"),
		});
		expect(pre({ verb: "tail", force })).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
	});
});

describe("explicit generation forcing", () => {
	it("recognizes the retired legacy value so it can refuse it by name", () => {
		expect(resolveGenerationForce({ PIJ_DAEMON_GENERATION: "legacy" })).toBe("legacy");
		expect(resolveGenerationForce({ PIJ_DAEMON_GENERATION: "rs" })).toBe("rs");
		expect(resolveGenerationForce({})).toBeUndefined();
		expect(resolveGenerationForce({ PIJ_DAEMON_GENERATION: "" })).toBeUndefined();
		for (const value of ["rust", "RS", " rs "]) {
			expect(() => resolveGenerationForce({ PIJ_DAEMON_GENERATION: value })).toThrow(/malformed/);
		}
	});

	it("forced legacy refuses even a supported live route", () => {
		expect(pre({ force: "legacy" })).toMatchObject({
			kind: "rs-error",
			code: "E-RS-UNPORTED",
			detail: expect.stringContaining("legacy is retired"),
		});
	});

	it("forced rs still needs an HTTP daemon", () => {
		expect(pre({ force: "rs" })).toMatchObject({ kind: "try-rs" });
		expect(pre({ force: "rs", rsLive: false })).toMatchObject({
			kind: "rs-error",
			code: "E-RS-UNPORTED",
		});
	});

	it("native trailers do not require an HTTP health probe", () => {
		expect(pre({ verb: "commit-trailers", rsLive: false })).toMatchObject({
			kind: "try-rs",
			row: { nativeCommand: "commit-trailers" },
		});
	});

	it("native fleet-report needs no daemon: it folds transcripts and reads the store itself", () => {
		expect(pre({ verb: "fleet-report", rsLive: false })).toMatchObject({
			kind: "try-rs",
			row: { nativeCommand: "fleet-report" },
		});
	});
});

describe("local refusals use named v2 envelopes", () => {
	it("unknown, unported, forced-legacy and absent-daemon reasons stay distinguishable", () => {
		const outcomes = [
			pre({ verb: "unknown-command" }),
			pre({ verb: "spawn" }),
			pre({ force: "legacy" }),
			pre({ rsLive: false }),
		];
		const details = new Set<string>();
		for (const outcome of outcomes) {
			expect(outcome.kind).toBe("rs-error");
			if (outcome.kind !== "rs-error") throw new Error("expected a local refusal");
			details.add(outcome.detail);
			expect(JSON.parse(routingRefusalEnvelope(outcome))).toMatchObject({
				ok: false,
				command: `pij ${outcome.verb}`,
				v: 2,
				error: "refused",
				details: {
					code: "E-RS-UNPORTED",
					verb: outcome.verb,
					ledger_item: expect.stringContaining("unsupported-status"),
				},
			});
		}
		expect(details.size).toBe(outcomes.length);
	});

	it("authentication and unreadable-wire refusals have different codes", () => {
		for (const [outcome, code, error] of [
			[
				{ kind: "rs-auth-rejected", verb: "adopt", addr: ADDR, stateDir: STATE_DIR },
				"E-RS-AUTH",
				"auth",
			],
			[
				{ kind: "rs-unreadable", verb: "adopt", addr: ADDR, detail: "invalid JSON" },
				"E-RS-WIRE",
				"refused",
			],
		] satisfies [RouteOutcome, string, string][]) {
			expect(JSON.parse(routingRefusalEnvelope(outcome))).toMatchObject({
				ok: false,
				v: 2,
				error,
				details: { code },
			});
		}
	});
});

describe("HTTP responses never authorize another store", () => {
	const classify = (facts: RsResponseFacts, forced = false) =>
		classifyRsResponse("adopt", ADDR, ROW, facts, forced, STATE_DIR);

	it.each([404, 405])("bodiless %s is named unported, not a daemon business refusal", (status) => {
		expect(classify({ status, envelopeDecoded: false })).toMatchObject({
			kind: "rs-error",
			code: "E-RS-UNPORTED",
			detail: expect.stringContaining("/v1/adopt"),
		});
	});

	it.each([
		400, 403, 404, 405, 500,
	])("a decoded %s refusal remains the daemon's error", (status) => {
		const outcome = classify({
			status,
			envelopeDecoded: true,
			refused: true,
			detail: "daemon refused",
		});
		expect(outcome).toMatchObject({ kind: "rs-error", detail: "daemon refused" });
		expect(outcome).not.toHaveProperty("code", "E-RS-UNPORTED");
	});

	it("a refusal carried over HTTP 200 is still an error", () => {
		expect(classify({ status: 200, envelopeDecoded: true, refused: true }).kind).toBe("rs-error");
	});

	it.each([
		{ status: 401, envelopeDecoded: true },
		{ status: 401, envelopeDecoded: false },
		{ status: 403, envelopeDecoded: false },
	])("authentication rejection remains distinct ($status, decoded $envelopeDecoded)", (facts) => {
		const outcome = classify(facts);
		expect(outcome).toMatchObject({ kind: "rs-auth-rejected", stateDir: STATE_DIR });
		expect(describeOutcome(outcome)).toMatch(/rejected the key/i);
		expect(describeOutcome(outcome)).toContain(`${STATE_DIR}/daemon.key`);
	});

	it.each([200, 500])("undecodable HTTP %s is unreadable, not success", (status) => {
		expect(classify({ status, envelopeDecoded: false, detail: "wire skew" })).toMatchObject({
			kind: "rs-unreadable",
			detail: "wire skew",
		});
	});

	it("valid success preserves the explicit force marker", () => {
		for (const forced of [false, true]) {
			expect(classify({ status: 200, envelopeDecoded: true }, forced)).toEqual({
				kind: "rs",
				verb: "adopt",
				addr: ADDR,
				forced,
			});
		}
	});
});

describe("route diagnostics name the selected daemon", () => {
	it("success, absence, auth and HTTP failures name the address", () => {
		const outcomes = [
			classifyRsResponse(
				"adopt",
				ADDR,
				ROW,
				{ status: 200, envelopeDecoded: true },
				false,
				STATE_DIR,
			),
			classifyRsResponse(
				"adopt",
				ADDR,
				ROW,
				{ status: 404, envelopeDecoded: false },
				false,
				STATE_DIR,
			),
			classifyRsResponse(
				"adopt",
				ADDR,
				ROW,
				{ status: 500, envelopeDecoded: true },
				false,
				STATE_DIR,
			),
			classifyRsResponse(
				"adopt",
				ADDR,
				ROW,
				{ status: 401, envelopeDecoded: true },
				false,
				STATE_DIR,
			),
			pre({ rsLive: false }),
		];
		for (const outcome of outcomes) {
			if (outcome.kind === "try-rs") throw new Error("expected a routing outcome");
			expect(describeOutcome(outcome)).toContain(ADDR);
		}
	});

	it("a different address changes the line", () => {
		const other = "127.0.0.1:18742";
		const outcome = { kind: "rs", verb: "adopt", addr: ADDR, forced: false } as const;
		expect(describeOutcome({ ...outcome, addr: other })).not.toBe(describeOutcome(outcome));
		expect(describeOutcome({ ...outcome, addr: other })).toContain(other);
		expect(describeOutcome({ ...outcome, forced: true })).toContain("PIJ_DAEMON_GENERATION=rs");
	});
});

/** ac-1147: the shim has NO name-generation path. rs mints; the shim never
 *  invents a seat name. Two minters would be two namespaces — the headline
 *  split-brain by construction. */
describe("the shim has no minter (ac-1147)", () => {
	const SHIM_SOURCES = ["./generation-routing.ts", "../adapters/generation-router.ts"].map((rel) =>
		fileURLToPath(new URL(rel, import.meta.url)),
	);

	/** Strip comments before scanning: this file and the sources both TALK about
	 *  minting at length, and a scanner that counted prose would be tripped by
	 *  its own documentation. */
	const scanForMinter = (source: string): readonly string[] => {
		const code = source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
		const signals: [RegExp, string][] = [
			[/memorable/i, "memorable-id vocabulary"],
			[/allocate_?[Mm]emorable/, "the rs allocator by name"],
			[/candidates?\s*\(/, "a candidate-name generator call"],
			[/Math\.random/, "randomness (a name generator's tell)"],
			[/randomUUID|randomBytes/, "crypto randomness"],
			[/`pij-\$\{/, "an interpolated pij- id literal"],
			[/["'`]pij-[a-z]+-[a-z]+["'`]/, "a hardcoded seat-name-shaped literal"],
		];
		return signals.filter(([re]) => re.test(code)).map(([, label]) => label);
	};

	it("finds no name-generation path in any shim source", () => {
		for (const path of SHIM_SOURCES) {
			expect({ path, found: scanForMinter(readFileSync(path, "utf8")) }).toEqual({
				path,
				found: [],
			});
		}
	});

	// POSITIVE CONTROL. Without this, the test above passes just as happily if
	// the scanner is broken, and "no minter found" would mean "found nothing"
	// rather than "there is nothing". Every arm must be able to fail.
	it("the scanner actually catches a minter when one is present", () => {
		const injected = `
			function mintSeatId(): string {
				return \`pij-\${Math.random().toString(36).slice(2)}\`;
			}
		`;
		expect(scanForMinter(injected).length).toBeGreaterThan(0);
	});

	it("the scanner is not fooled by a minter hidden in a comment-shaped string", () => {
		expect(scanForMinter(`const x = "pij-long-wombat";`).length).toBeGreaterThan(0);
	});

	it("prose about minting does not trip the scanner", () => {
		expect(scanForMinter(`/* the memorable id minter lives in rs, never here */`)).toEqual([]);
	});
});

describe("mailbox leaf boundaries", () => {
	it("registration uses native HTTP admission without changing read siblings", () => {
		expect(findRoute("inbox", "register")).toMatchObject({
			rsPath: "/v1/register",
			method: "POST",
		});
		expect(pre({ verb: "inbox", leaf: "register" })).toMatchObject({
			kind: "try-rs",
		});
		for (const leaf of [undefined, "check"]) {
			expect(findRoute("inbox", leaf)).toMatchObject({
				rsPath: "/v1/shim/inbox",
				acknowledgePath: "/v1/shim/inbox/ack",
			});
			expect(pre({ verb: "inbox", leaf })).toMatchObject({ kind: "try-rs" });
		}
	});

	it("an unknown inbox leaf cannot fall through to the bare read row", () => {
		expect(LEAF_CLOSED_VERBS).toContain("inbox");
		const table: RsHttpRoute[] = [
			{ ...ROW, verb: "inbox" },
			{ ...ROW, verb: "inbox", leaf: "check" },
		];
		expect(findRoute("inbox", "frobnicate", table)).toBeUndefined();
		expect(findRoute("inbox", "frobnicate")).toBeUndefined();
		expect(
			decidePreCall(
				{ verb: "inbox", leaf: "frobnicate", force: undefined, addr: ADDR, rsLive: true },
				table,
			),
		).toMatchObject({ kind: "rs-error", code: "E-RS-UNPORTED" });
		expect(findRoute("inbox", "check", table)).toBeDefined();
	});

	it("send uses the shim caller-evidence endpoint and needs no acknowledgement", () => {
		expect(findRoute("send", undefined)).toMatchObject({
			rsPath: "/v1/shim/send",
			readsBodyFile: true,
		});
		expect(findRoute("send", undefined)).not.toHaveProperty("acknowledgePath");
	});
});
