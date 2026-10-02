import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
	busyProbePlan,
	composerText,
	parentSteeringContext,
	parentTranscript,
	parseProbeArgs,
	runProbe,
	transcriptTexts,
	transcriptToolCalls,
} from "./step-on-probe.js";

const border = "─".repeat(80);
describe("step-on probe evidence boundary", () => {
	it.each([
		"omp",
		"claude",
	] as const)("extracts only the %s composer, not a quoted draft", (harness) => {
		const draft = "UNSUBMITTED keep  two spaces π";
		const gap = harness === "claude" ? "\u00a0" : " ";
		expect(
			composerText(
				harness,
				`Transcript repeats wrong draft\n${border}\n❯${gap}${draft}\n${border}\nstatus`,
			),
		).toBe(draft);
		expect(composerText(harness, `${border}\nquoted draft\n${border}`)).toBeUndefined();
		expect(composerText(harness, "Accessing workspace: Yes, I trust this folder")).toBeUndefined();
	});
	it.each([
		"omp",
		"claude",
		"copilot",
	] as const)("preserves %s leading/trailing draft spaces using the paired cursor", (harness) => {
		const draft = " UNSUBMITTED keep  spaces ";
		const top = harness === "copilot" ? `╻${"▄".repeat(80)}` : border;
		const bottom = harness === "copilot" ? `╹${"▀".repeat(80)}` : border;
		const prefix = harness === "copilot" ? "┃ " : harness === "claude" ? "❯\u00a0" : "❯ ";
		const viewport = `${top}\n${prefix}${draft}${" ".repeat(20)}\n${bottom}`;
		const cursor = { x: prefix.length + draft.length, y: 1 };
		expect(composerText(harness, viewport, cursor)).toBe(draft);
		expect(composerText(harness, viewport, { ...cursor, x: cursor.x + 1 })).toBe(`${draft} `);
		expect(composerText(harness, viewport, { ...cursor, y: 0 })).toBeUndefined();
		expect(
			composerText(harness, `${top}\n${prefix}${draft}EXTRA\n${bottom}`, cursor),
		).toBeUndefined();
	});
	it("requires a bounded real-resource observation and an explicit fallback mode", () => {
		expect(() => parseProbeArgs(["--observe-ms", "0"])).toThrow();
		expect(() => parseProbeArgs(["--harness", "fake"])).toThrow();
		expect(() => parseProbeArgs(["--send-keys-only", "true"])).toThrow();
		expect(parseProbeArgs(["--harness", "claude", "--send-keys-only", "true"]).sendKeysOnly).toBe(
			true,
		);
	});
	it("preserves idle defaults and accepts explicit OMP busy states", () => {
		expect(parseProbeArgs([])).toMatchObject({
			harness: "omp",
			state: "idle",
			observeMs: 10_000,
			timeoutMs: 90_000,
		});
		for (const state of ["idle", "bash", "subagents"])
			expect(parseProbeArgs(["--state", state, "--harness", "omp"]).state).toBe(state);
		for (const harness of ["claude", "copilot"])
			expect(parseProbeArgs(["--harness", harness, "--state", "idle"]).state).toBe("idle");
	});
	it("passes explicit OMP thinking effort without changing inherited defaults", () => {
		expect(parseProbeArgs([]).thinking).toBeUndefined();
		expect(parseProbeArgs(["--thinking", "minimal"]).thinking).toBe("minimal");
		expect(() => parseProbeArgs(["--thinking"])).toThrow("missing value");
		expect(() => parseProbeArgs(["--harness", "claude", "--thinking", "minimal"])).toThrow(
			"--thinking requires --harness omp",
		);
	});
	it("rejects unknown or missing states and non-OMP busy work regardless of flag order", () => {
		expect(() => parseProbeArgs(["--state"])).toThrow("missing value");
		expect(() => parseProbeArgs(["--state", "sleep"])).toThrow("invalid probe state");
		for (const harness of ["claude", "copilot"])
			for (const state of ["bash", "subagents"]) {
				expect(() => parseProbeArgs(["--harness", harness, "--state", state])).toThrow(
					"busy probe states require --harness omp",
				);
				expect(() => parseProbeArgs(["--state", state, "--harness", harness])).toThrow(
					"busy probe states require --harness omp",
				);
			}
	});
	it.each([
		"bash",
		"subagents",
	] as const)("keeps %s remaining work separate from the first blocking tool", (state) => {
		const plan = busyProbePlan(state, "/tmp/owned probe's workspace");
		expect(plan.first.command).toMatch(/^\/bin\/sh /);
		expect(plan.first.scriptContent).toContain(`/bin/sleep ${state === "bash" ? 30 : 45}`);
		expect(plan.remaining.scriptContent).toContain("/bin/sleep 10");
		expect(plan.first.command).not.toContain(plan.remaining.done);
		expect(plan.remaining.done).not.toBe(plan.first.done);
		expect(plan.children).toHaveLength(state === "subagents" ? 2 : 0);
		for (const child of plan.children) {
			expect(child.scriptContent).toContain("/bin/sleep 30");
			expect(plan.prompt).toContain(JSON.stringify(child.command).slice(1, -1));
		}
		expect(plan.first.command).toContain("probe'\\''s workspace");
		expect(plan.prompt).toContain("SECOND separate bash call");
		expect(plan.prompt).toContain(plan.remaining.command);
		expect(plan.prompt).toContain(plan.doneMarker);
	});
	it("takes busy evidence only from the native parent session, never child replies or tool-looking text", () => {
		const root = mkdtempSync(join(tmpdir(), "step-on-parent-"));
		const parent = join(root, "parent.jsonl");
		const child = join(root, "child.jsonl");
		const call = {
			type: "toolCall",
			id: "call-parent",
			name: "bash",
			arguments: { command: "/bin/sleep 30" },
		};
		try {
			writeFileSync(
				parent,
				[
					JSON.stringify({ type: "session", id: "native-parent" }),
					JSON.stringify({
						type: "message",
						timestamp: "2026-09-07T12:00:00Z",
						message: { role: "assistant", content: [call] },
					}),
					JSON.stringify({
						type: "message",
						message: {
							role: "assistant",
							content: [{ type: "text", text: "quoted toolCall is not work evidence" }],
						},
					}),
					JSON.stringify({ type: "message", message: { role: "user", content: [call] } }),
					'{"type":"message',
				].join("\n"),
			);
			writeFileSync(
				child,
				[
					JSON.stringify({ type: "session", id: "native-child" }),
					JSON.stringify({
						type: "message",
						message: { role: "assistant", content: "MESSAGE_only_child" },
					}),
				].join("\n"),
			);
			expect(parentTranscript([child, parent], "native-parent")).toBe(parent);
			expect(parentTranscript([child], "native-parent")).toBeUndefined();
			expect(transcriptToolCalls(parent)).toEqual([{ ...call, timestamp: "2026-09-07T12:00:00Z" }]);
			expect(transcriptTexts([parent], ["assistant"])).not.toContain("MESSAGE_only_child");
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
	it("requires direct pij context at the blocking tool boundary, not a child notice or follow-up", () => {
		const root = mkdtempSync(join(tmpdir(), "step-on-boundary-"));
		const path = join(root, "parent.jsonl");
		const nonce = "MESSAGE_boundary";
		const command = "/bin/sh /tmp/owned.sh";
		const tool = {
			type: "message",
			message: {
				role: "assistant",
				content: [{ type: "toolCall", name: "bash", arguments: { command } }],
			},
		};
		const result = { type: "message", message: { role: "toolResult", content: [] } };
		const incoming = {
			type: "custom_message",
			customType: "pij",
			content: `[pij-rs from pij-step-on-probe]\n${nonce}\n[/pij]`,
		};
		const save = (entries: unknown[]) =>
			writeFileSync(path, entries.map((entry) => JSON.stringify(entry)).join("\n"));
		try {
			save([tool, result, incoming]);
			expect(parentSteeringContext(path, nonce, command)).toBe(true);
			for (const wrong of [
				{ ...incoming, customType: "task-result" },
				{ ...incoming, content: `<system-notice>${incoming.content}</system-notice>` },
				{ ...incoming, content: incoming.content.replace("pij-step-on-probe", "another-sender") },
			]) {
				save([tool, result, wrong]);
				expect(parentSteeringContext(path, nonce, command)).toBe(false);
			}
			save([
				tool,
				result,
				{
					type: "message",
					message: { role: "assistant", content: [{ type: "text", text: "done early" }] },
				},
				incoming,
			]);
			expect(parentSteeringContext(path, nonce, command)).toBe(false);
			save([tool, result, incoming]);
			expect(parentSteeringContext(path, nonce, "another command")).toBe(false);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
	it("distinguishes custom model context from human submission and ignores partial rows", () => {
		const root = mkdtempSync(join(tmpdir(), "step-on-transcript-"));
		const path = join(root, "events.jsonl");
		try {
			writeFileSync(
				path,
				[
					JSON.stringify({ type: "custom_message", customType: "pij", content: "peer nonce" }),
					JSON.stringify({
						type: "message",
						message: { role: "user", content: [{ type: "text", text: "human draft" }] },
					}),
					JSON.stringify({ type: "assistant.message", data: { content: "model reply" } }),
					'{"type":"partial',
				].join("\n"),
			);
			expect(transcriptTexts([path], ["custom"])).toEqual(["peer nonce"]);
			expect(transcriptTexts([path], ["user"])).toEqual(["human draft"]);
			expect(transcriptTexts([path], ["assistant"])).toEqual(["model reply"]);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
});

// Explicit opt-in only: launches real clients/daemon/tmux and spends provider tokens.
describe.skipIf(process.env.PIJ_STEP_ON_REAL !== "1")("real-client draft safety", () => {
	for (const harness of ["omp", "claude", "copilot"] as const) {
		it(`${harness} receives without changing or submitting the draft`, async () => {
			const result = await runProbe(parseProbeArgs(["--harness", harness]));
			const receipt = JSON.parse(readFileSync(result.output, "utf8"));
			expect(receipt.error, result.output).toBeUndefined();
			expect(receipt.verdict, result.output).toMatchObject({
				draft_intact: true,
				message_reached_model: true,
				draft_submitted: false,
			});
			expect(receipt.verdict.delivered_within_ms, result.output).toBeLessThanOrEqual(5000);
			expect(result.code, result.output).toBe(0);
		}, 240_000);
	}
	it("retains the gate when socket discovery is unavailable", async () => {
		const result = await runProbe(
			parseProbeArgs(["--harness", "claude", "--send-keys-only", "true"]),
		);
		const receipt = JSON.parse(readFileSync(result.output, "utf8"));
		expect(receipt.error, result.output).toBeUndefined();
		expect(receipt.verdict, result.output).toMatchObject({
			transport_rung: "typed-body",
			draft_intact: true,
			message_reached_model: false,
			draft_submitted: false,
		});
		expect(result.code, result.output).toBe(0);
	}, 240_000);
});
