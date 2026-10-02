import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
	DaemonRefusalError,
	decodeEnvelope,
	decodeStreamLine,
	PIJ_ENVELOPE_VERSION,
	PijNoDaemonError,
	PijWireSkewError,
	parseProcessStart,
	type RustSeatDescriptor,
} from "./daemon-wire.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const WIRE = resolve(HERE, "../../../../crates/testkit/fixtures/wire");
const fixture = (name: string): string => readFileSync(resolve(WIRE, name), "utf8");
const ERROR_ENVELOPES = JSON.parse(
	readFileSync(
		resolve(HERE, "../../../../crates/testkit/fixtures/golden/api/error-envelopes.json"),
		"utf8",
	),
) as Array<{ command: string; error: string; meta: string }>;

describe("pij-rs envelope", () => {
	it("pins the exact consumer floor at envelope v2", () => {
		expect(PIJ_ENVELOPE_VERSION).toBe(2);
		expect(
			decodeEnvelope(
				JSON.stringify({
					ok: true,
					command: "pij ping",
					v: 2,
					data: { status: "healthy", build: "pij-rs 0.2.0", machine: "workstation" },
				}),
			).v,
		).toBe(2);
	});

	it.each([
		"pane",
		"harness",
		undefined,
	] as const)("preserves additive register provenance %s and absent-field compatibility", (procSource) => {
		const seat: RustSeatDescriptor = {
			id: "pij-wire",
			harness: "omp",
			folder: "/abs/tree",
			state: "idle",
			proc: { pid: 123, proc_start: 20260905092248 },
			binding: "rebound",
			...(procSource === undefined ? {} : { proc_source: procSource }),
		};
		const decoded = decodeEnvelope<RustSeatDescriptor>(
			JSON.stringify({ ok: true, command: "pij register", v: 2, data: seat }),
		);
		expect(decoded.data).toEqual(seat);
		expect(decoded.data.proc_source).toBe(procSource);
	});
	it("refuses the old envelope floor with structured versions", () => {
		expect(() => decodeEnvelope(fixture("health-v1.json"))).toThrow(PijWireSkewError);
		try {
			decodeEnvelope(fixture("health-v1.json"));
		} catch (error) {
			expect(error).toMatchObject({ code: "PIJ_WIRE_SKEW", found: 1, supported: 2 });
		}
	});

	it("refuses a future envelope whole with structured versions", () => {
		try {
			decodeEnvelope(JSON.stringify({ ok: true, command: "pij ping", v: 3, data: {} }));
			throw new Error("future envelope was accepted");
		} catch (error) {
			expect(error).toBeInstanceOf(PijWireSkewError);
			expect(error).toMatchObject({ code: "PIJ_WIRE_SKEW", found: 3, supported: 2 });
		}
	});

	it.each(
		ERROR_ENVELOPES,
	)("recognizes canonical $error refusals and retains exact validated bytes", (envelope) => {
		const rawEnvelope = ` \n${JSON.stringify({ ...envelope, future: { retained: true } }, null, "\t")}\n`;
		try {
			decodeEnvelope(rawEnvelope);
			throw new Error("refusal envelope was accepted as success");
		} catch (error) {
			expect(error).toBeInstanceOf(DaemonRefusalError);
			expect(error).toMatchObject({
				command: envelope.command,
				kind: envelope.error,
				message: envelope.meta,
				rawEnvelope,
			});
		}
	});

	it.each(
		ERROR_ENVELOPES.filter((envelope) => envelope.error.includes("_")),
	)("rejects a hyphen alias of canonical $error rather than recognizing a daemon refusal", (envelope) => {
		expect(() =>
			decodeEnvelope(JSON.stringify({ ...envelope, error: envelope.error.replaceAll("_", "-") })),
		).toThrow("daemon envelope error must be a known ErrorKind");
	});

	it.each([
		["malformed JSON", "not JSON"],
		["old version", JSON.stringify({ ...ERROR_ENVELOPES[0], v: 1 })],
		["future version", JSON.stringify({ ...ERROR_ENVELOPES[0], v: 3 })],
		["invalid success flag", JSON.stringify({ ...ERROR_ENVELOPES[0], ok: "false" })],
		["unknown error kind", JSON.stringify({ ...ERROR_ENVELOPES[0], error: "future-error-kind" })],
	] as const)("does not attach unchecked bytes to a %s error", (_label, text) => {
		let caught: unknown;
		try {
			decodeEnvelope(text);
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(Error);
		expect(caught).not.toBeInstanceOf(DaemonRefusalError);
		expect(caught).not.toHaveProperty("rawEnvelope");
	});

	it("does not invent raw daemon bytes for a locally constructed refusal", () => {
		const error = new DaemonRefusalError("auth", "auth", "daemon key is unreadable");
		expect(error).not.toMatchObject({ rawEnvelope: expect.any(String) });
	});

	it("keeps no-daemon address and state directory as fields", () => {
		const error = new PijNoDaemonError("127.0.0.1:7461", "/tmp/pij-rs");
		expect(error).toMatchObject({
			code: "PIJ_NO_DAEMON",
			addr: "127.0.0.1:7461",
			stateDir: "/tmp/pij-rs",
		});
	});
});

describe("pij-rs event stream", () => {
	it("forwards an unknown event and continues to the pushed turn", () => {
		const decoded = fixture("events-v1.ndjson").trim().split("\n").map(decodeStreamLine);
		expect(decoded[0]).toEqual({ hello: true, v: 1, build: "pij-rs 0.1.0" });
		expect(decoded[1]).toMatchObject({
			cursor: 41,
			event: { kind: "future.kind", payload: '{"opaque":true}' },
		});
		expect(decoded[2]).toMatchObject({
			cursor: 42,
			event: { kind: "message.pushed", seat: "pij-test-seat" },
		});
	});
});

describe("process identity", () => {
	it("packs C-locale ps lstart exactly like pij-rs ProcLiveness", () => {
		expect(parseProcessStart("Sat Aug 29 10:30:52 2026")).toBe(20260829103052);
		expect(parseProcessStart("Wed Jan  8 00:20:51 2026")).toBe(20260108002051);
	});
});
