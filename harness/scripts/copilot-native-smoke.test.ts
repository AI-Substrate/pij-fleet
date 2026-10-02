import {
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
	fixtureReply,
	object,
	sendPrompt,
	smokeEnvironment,
	startFixtureProvider,
} from "./copilot-native-fixture.js";
import {
	captureSmokeFinalTyping,
	captureSmokePanes,
	confirmSmokeToolInvocation,
	driveSmokeLifecycleCommand,
	driveSmokeTypingWitness,
	finishSmokePanes,
	launchSmokeNegativeControl,
	nativeComposer,
	nativeMessages,
	parseSmokeArgs,
	readNativeEvents,
	runSmoke,
	stopSmokeNativeHost,
	verifyNativeContextHeld,
	verifyNativeLifecycleIdentity,
	verifyOrdinaryUsability,
	verifySmokePrimarySettings,
	verifyToolApproval,
	verifyWorkspaceTrustDiscriminator,
} from "./copilot-native-smoke.js";
import { manageCopilotExtension } from "./link-global.js";

const roots: string[] = [];
function temporary(): string {
	const root = mkdtempSync(join(tmpdir(), "copilot-proof-test-"));
	roots.push(root);
	return root;
}
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

const tools = [
	{ type: "function", function: { name: "pij_send", parameters: { type: "object" } } },
];

describe("native smoke fixture protocol", () => {
	it("requests only the advertised native tool, once per explicit marker", () => {
		const prompt = sendPrompt("pij-target", "nonce-1");
		const messages = [{ role: "user", content: prompt }];
		const first = fixtureReply({ messages, tools });
		const call = object((first.tool_calls as unknown[])[0]);
		expect(object(call.function)).toMatchObject({
			name: "pij_send",
			arguments: JSON.stringify({ to: "pij-target", message: "nonce-1" }),
		});
		expect(
			fixtureReply({
				messages: [...messages, first, { role: "tool", tool_call_id: call.id, content: "receipt" }],
				tools,
			}),
		).toEqual({ role: "assistant", content: "PIJ_NATIVE_DONE" });
		expect(
			fixtureReply({ messages: [{ role: "user", content: "ordinary incoming data" }], tools })
				.tool_calls,
		).toBeUndefined();
	});

	it("refuses a missing native tool instead of fabricating an outgoing reply", () => {
		expect(() =>
			fixtureReply({
				messages: [{ role: "user", content: sendPrompt("pij-target", "nonce") }],
				tools: [],
			}),
		).toThrow("PIJ_NATIVE_TOOL_MISSING");
		expect(() => fixtureReply({ messages: [] })).not.toThrow();
		expect(() => fixtureReply({ messages: "bad" })).toThrow("messages must be an array");
	});

	it("serves actual streaming and non-streaming OpenAI protocol with explicit failure", async () => {
		const fixture = await startFixtureProvider();
		try {
			const send = (body: unknown) =>
				fetch(`${fixture.url}/chat/completions`, { method: "POST", body: JSON.stringify(body) });
			const body = {
				messages: [{ role: "user", content: sendPrompt("pij-peer", "nonce") }],
				tools,
			};
			const response = await send(body);
			expect(response.status).toBe(200);
			expect(object(await response.json()).object).toBe("chat.completion");
			const streaming = await send({ ...body, stream: true });
			expect(streaming.headers.get("content-type")).toBe("text/event-stream");
			const events = (await streaming.text()).split("\n\n").filter(Boolean);
			expect(events.at(-1)).toBe("data: [DONE]");
			expect(events.join("\n")).toContain('"finish_reason":"tool_calls"');
			const failed = await send({ ...body, tools: [] });

			expect(failed.status).toBe(400);
			expect(await failed.text()).toContain("PIJ_NATIVE_TOOL_MISSING");
			expect((await fetch(`${fixture.url}/models`)).status).toBe(200);
			expect((await fetch(`${fixture.url}/unknown`)).status).toBe(404);
		} finally {
			await fixture.close();
		}
	});
});

describe("negative CLI home isolation, not live CLI proof", () => {
	function setup() {
		const run = join(temporary(), "run with spaces");
		const home = join(run, "home");
		const copilotHome = join(home, ".copilot");
		const cwd = join(run, "workspace");
		mkdirSync(cwd, { recursive: true });
		expect(
			manageCopilotExtension({
				pijRoot: new URL("../..", import.meta.url).pathname,
				home,
				copilotHome,
				args: [],
				stdout: () => {},
				stderr: () => {},
			}).skipped,
		).toBe(0);
		const env: Readonly<NodeJS.ProcessEnv> = Object.freeze({
			...smokeEnvironment({}, home, copilotHome, join(run, "daemon"), "127.0.0.1:1234", "local"),
			COPILOT_PROVIDER_BASE_URL: "http://127.0.0.1:5678",
			COPILOT_MODEL: "pij-native-fixture",
		});
		const options = {
			run,
			primaryCopilotHome: copilotHome,
			cwd,
			trustedFolders: [realpathSync(cwd)],
			copilotBin: "/fixture/copilot",
			common: ["--no-auto-update", "--allow-tool=pij_send"],
		};
		const launches: NodeJS.ProcessEnv[] = [];
		const commands: string[][] = [];
		const terminal = String(
			object(
				JSON.parse(
					readFileSync(new URL("./fixtures/copilot-native-disabled.json", import.meta.url), "utf8"),
				),
			).terminal,
		);
		const state: { corruptPrimary?: boolean; launchError?: Error; terminal: string } = { terminal };
		const tmux = (args: string[]) => {
			commands.push(args);
			if (args[0] === "capture-pane") return state.terminal;
			expect(args[0]).toBe("new-window");
			const child: NodeJS.ProcessEnv = { ...env };
			const overrideKeys: string[] = [];
			for (let i = 0; i < args.length; i++) {
				if (args[i] !== "-e") continue;
				const assignment = args[++i];
				if (!assignment) throw new Error("missing window environment");
				const equals = assignment.indexOf("=");
				const name = assignment.slice(0, equals);
				overrideKeys.push(name);
				child[name] = assignment.slice(equals + 1);
			}
			launches.push(child);
			// Simulate the observed CLI persistence in the home actually passed to this window.
			writeFileSync(join(String(child.COPILOT_HOME), "settings.json"), '{"experimental":false}\n');
			if (state.corruptPrimary)
				writeFileSync(join(copilotHome, "settings.json"), '{"experimental":false}\n');
			if (state.launchError) throw state.launchError;
			if (overrideKeys.length)
				expect(overrideKeys.sort()).toEqual(["COPILOT_HOME", "HOME", "XDG_CONFIG_HOME"]);
			return `%${launches.length}`;
		};
		return { options, env, launches, commands, tmux, state };
	}

	it("keeps persisted opt-out private for each manual and spawned negative window", async () => {
		const f = setup();
		const originalEnv = { ...f.env };
		const originalSettings = readFileSync(
			join(f.options.primaryCopilotHome, "settings.json"),
			"utf8",
		);
		const homes = new Set<string>();
		for (const mode of ["manual", "spawned"]) {
			const witness: Record<string, unknown> = { mode };
			await expect(launchSmokeNegativeControl(f.tmux, f.options, 1, witness)).resolves.toMatch(
				/^%\d+$/,
			);
			const child = f.launches.at(-1);
			if (!child) throw new Error("missing negative launch");
			expect(child.HOME).not.toBe(f.env.HOME);
			expect(child.COPILOT_HOME).toBe(join(String(child.HOME), ".copilot"));
			expect(child.XDG_CONFIG_HOME).toBe(join(String(child.HOME), ".config"));
			expect(String(child.HOME).startsWith(`${f.options.run}/`)).toBe(true);
			homes.add(String(child.HOME));
			expect(child).toMatchObject({
				PIJ_RS_ADDR: f.env.PIJ_RS_ADDR,
				PIJ_RS_STATE_DIR: f.env.PIJ_RS_STATE_DIR,
				COPILOT_PROVIDER_BASE_URL: f.env.COPILOT_PROVIDER_BASE_URL,
				COPILOT_MODEL: f.env.COPILOT_MODEL,
			});
			const negativeHome = String(child.COPILOT_HOME);
			expect(realpathSync(join(negativeHome, "extensions/pij"))).toBe(
				realpathSync(new URL("../../.copilot/extensions/pij", import.meta.url)),
			);
			expect(JSON.parse(readFileSync(join(negativeHome, "config.json"), "utf8"))).toEqual({
				trustedFolders: f.options.trustedFolders,
			});
			expect(existsSync(join(negativeHome, "auth.json"))).toBe(false);
			expect(witness).toMatchObject({
				primary_settings_before: originalSettings,
				primary_settings_after: originalSettings,
				negative_settings_after: '{"experimental":false}\n',
				launch_args: [...f.options.common, "--no-experimental"],
				status: "ready-not-absence-proven",
			});
			expect(JSON.parse(String(witness.negative_settings_before)).experimental).toBe(true);
			expect(f.env).toEqual(originalEnv);
			const resume: Record<string, unknown> = {};
			verifySmokePrimarySettings(
				f.options.primaryCopilotHome,
				resume,
				"primary_settings_before_resume",
			);
			expect(resume.primary_settings_before_resume).toBe(originalSettings);
		}
		expect(homes.size).toBe(2);
		expect(f.commands.filter((args) => args[0] === "new-window")).toHaveLength(2);
	});

	it("does not seed remembered trust in prompt mode", async () => {
		const f = setup();
		f.options.trustedFolders = [];
		const witness: Record<string, unknown> = {};
		await launchSmokeNegativeControl(f.tmux, f.options, 1, witness);
		expect(existsSync(join(String(witness.copilot_home), "config.json"))).toBe(false);
	});

	it.each([
		false,
		"true",
		undefined,
	])("rejects primary experimental %s before negative or resume launch", async (experimental) => {
		const f = setup();
		const raw = JSON.stringify({ experimental });
		writeFileSync(join(f.options.primaryCopilotHome, "settings.json"), raw);
		const witness: Record<string, unknown> = {};
		await expect(launchSmokeNegativeControl(f.tmux, f.options, 1, witness)).rejects.toThrow(
			"PIJ_NATIVE_PRIMARY_SETTINGS",
		);
		expect(f.commands).toEqual([]);
		expect(witness.primary_settings_before).toBe(raw);
		expect(() =>
			verifySmokePrimarySettings(
				f.options.primaryCopilotHome,
				witness,
				"primary_settings_before_resume",
			),
		).toThrow("PIJ_NATIVE_PRIMARY_SETTINGS");
		expect(witness.primary_settings_before_resume).toBe(raw);
		expect(readFileSync(join(f.options.primaryCopilotHome, "settings.json"), "utf8")).toBe(raw);
	});

	it("fails and retains primary contamination instead of repairing it", async () => {
		const f = setup();
		f.state.corruptPrimary = true;
		const witness: Record<string, unknown> = {};
		await expect(launchSmokeNegativeControl(f.tmux, f.options, 1, witness)).rejects.toThrow(
			"PIJ_NATIVE_PRIMARY_SETTINGS",
		);
		expect(witness).toMatchObject({
			status: "failed",
			primary_settings_after: '{"experimental":false}\n',
		});
	});

	it("preserves launch failure if settings proof also fails", async () => {
		const f = setup();
		f.state.corruptPrimary = true;
		const original = new Error("original negative launch failure");
		f.state.launchError = original;
		const witness: Record<string, unknown> = {};
		await expect(launchSmokeNegativeControl(f.tmux, f.options, 1, witness)).rejects.toBe(original);
		expect(witness.settings_error).toContain("PIJ_NATIVE_PRIMARY_SETTINGS");
		expect(witness.error).toBe(String(original));
	});

	it("still requires an empty ready composer and retains settings on timeout", async () => {
		const f = setup();
		f.state.terminal = "╻▄▄▄▄▄▄\n┃ unfinished draft\n╹▀▀▀▀▀▀";
		const witness: Record<string, unknown> = {};
		await expect(launchSmokeNegativeControl(f.tmux, f.options, 1, witness)).rejects.toThrow(
			"PIJ_NATIVE_TIMEOUT: disabled CLI composer ready",
		);
		expect(JSON.parse(String(witness.primary_settings_after)).experimental).toBe(true);
		expect(JSON.parse(String(witness.negative_settings_after)).experimental).toBe(false);
		expect(witness.status).toBe("failed");
	});
});

describe("isolated native tool human approval", () => {
	const expected = { pane: "%3", to: "pij-original-sender", message: "PIJ_NATIVE_exact-nonce" };
	const args = { to: expected.to, message: expected.message };
	// Observed CLI modal structure; the transcript intentionally repeats the correct arguments.
	const dialog = (parameters: unknown = args) =>
		[
			`Previous user message ${JSON.stringify(args)}`,
			"╭────────────────╮",
			'│ Run extension tool "pij_send" │',
			"│ Send a message to a Pij peer. │",
			...JSON.stringify(parameters, null, 2)
				.split("\n")
				.map((line) => `│ ${line} │`),
			"│ │",
			"│ ❯ 1. Yes │",
			'│ 2. Yes, and approve "pij_send" for the rest of the session │',
			"│ 3. No │",
			"│ 4. No, and tell Copilot why... (Esc to stop) │",
			"│ │",
			"│ ↑/↓ to navigate · enter to select · esc to cancel │",
			"╰────────────────╯",
		].join("\n");
	const welcome = object(
		JSON.parse(
			readFileSync(new URL("./fixtures/copilot-native-welcome.json", import.meta.url), "utf8"),
		),
	);
	const welcomeTerminal = String(welcome.terminal);
	const welcomeRows = welcomeTerminal.split("\n");
	const welcomeBeforeComposer = welcomeRows
		.slice(0, welcomeRows.findIndex((line) => /^╰─+╯$/.test(line.trim())) + 1)
		.join("\n");
	const incomplete = dialog().replace("╰────────────────╯", "");

	it("recognizes the retained real welcome panel as history above the composer", () => {
		const nativeExpected = object(welcome.expected);
		expect(
			verifyToolApproval(welcomeTerminal, {
				to: String(nativeExpected.to),
				message: String(nativeExpected.message),
			}),
		).toBe(false);
		expect(nativeComposer(welcomeTerminal)).toBe("");
	});
	it("treats retained welcome truncated at its closer as an incomplete repaint", () => {
		expect(verifyToolApproval(welcomeBeforeComposer, args)).toBeUndefined();
		expect(nativeComposer(welcomeBeforeComposer)).toBeUndefined();
	});
	it.each([
		welcomeBeforeComposer.replace("Getting started", "Confirm folder trust"),
		welcomeBeforeComposer.replace(
			"Use the tabs above to explore your sessions and pull requests",
			"Approve access to files",
		),
		welcomeBeforeComposer.replace(/(^╰─+╯$)/m, "│ ❯ 1. Yes │\n$1"),
	])("still throws for complete unknown or modified welcome-shaped boxes", (terminal) => {
		expect(() => verifyToolApproval(terminal, args)).toThrow("PIJ_NATIVE_UNEXPECTED_APPROVAL");
		expect(() => nativeComposer(terminal)).toThrow("PIJ_NATIVE_UNEXPECTED_LIFECYCLE_DIALOG");
	});
	it.each([
		welcomeTerminal,
		welcomeBeforeComposer,
		`${dialog()}\n❯ ordinary composer`,
		incomplete,
		dialog().replace("╭────────────────╮", "╭────"),
		dialog().replace("╰────────────────╯", "╰────"),
	])("waits boundedly without keys on non-active boxes or incomplete repaint", async (terminal) => {
		const keys: string[][] = [];
		const witness: Record<string, unknown> = {};
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					if (command[0] === "send-keys") keys.push(command);
					return terminal;
				},
				expected,
				1,
				witness,
			),
		).rejects.toThrow("PIJ_NATIVE_TIMEOUT");
		expect(keys).toEqual([]);
		expect(witness.decision).toBe("withhold");
	});
	it("matches only the active bottom dialog after welcome and transcript boxes", () => {
		expect(verifyToolApproval(`${welcomeTerminal}\n${dialog()}`, args)).toBe(true);
		expect(
			verifyToolApproval(`${dialog({ ...args, to: "historic-peer" })}\n${dialog()}`, args),
		).toBe(true);
	});
	it.each([
		dialog({ ...args, to: "wrong-active-peer" }),
		dialog().replace('Run extension tool "pij_send"', "Confirm folder trust"),
	])("still refuses a complete mismatching active dialog below welcome", (active) => {
		expect(() => verifyToolApproval(`${welcomeTerminal}\n${active}`, args)).toThrow(
			"PIJ_NATIVE_UNEXPECTED_APPROVAL",
		);
	});
	it("waits through welcome and incomplete repaint before two exact confirmations", async () => {
		const frames = [
			welcomeTerminal,
			welcomeBeforeComposer,
			incomplete,
			dialog(),
			dialog(),
			welcomeTerminal,
		];
		const keys: string[][] = [];
		await confirmSmokeToolInvocation(
			(command) => {
				if (command[0] === "capture-pane") return frames.shift() ?? welcomeTerminal;
				expect(frames).toHaveLength(1);
				keys.push(command);
				return "";
			},
			expected,
			2000,
			{},
		);
		expect(keys).toEqual([["send-keys", "-t", expected.pane, "Enter"]]);
	});
	it.each([
		incomplete,
		welcomeTerminal,
	])("retries a pre-key repaint or vanished dialog without approval", async (repaint) => {
		const frames = [dialog(), repaint, dialog(), dialog(), welcomeTerminal];
		const keys: string[][] = [];
		await confirmSmokeToolInvocation(
			(command) => {
				if (command[0] === "capture-pane") return frames.shift() ?? welcomeTerminal;
				expect(frames).toHaveLength(1);
				keys.push(command);
				return "";
			},
			expected,
			2000,
			{},
		);
		expect(keys).toHaveLength(1);
	});
	it.each([
		incomplete,
		welcomeBeforeComposer,
	])("does not equate a post-key incomplete repaint with dismissal", async (repaint) => {
		const frames = [dialog(), dialog(), repaint];
		const keys: string[][] = [];
		const witness: Record<string, unknown> = {};
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					if (command[0] === "capture-pane") return frames.shift() ?? repaint;
					keys.push(command);
					return "";
				},
				expected,
				1,
				witness,
			),
		).rejects.toThrow("PIJ_NATIVE_TIMEOUT");
		expect(keys).toHaveLength(1);
		expect(witness).toMatchObject({ status: "failed", after_input: repaint });
	});
	it("fails a complete unknown dialog after a post-key repaint without another key", async () => {
		const unknown = dialog().replace('Run extension tool "pij_send"', "Confirm folder trust");
		const frames = [dialog(), dialog(), incomplete, unknown];
		const keys: string[][] = [];
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					if (command[0] === "capture-pane") return frames.shift() ?? unknown;
					keys.push(command);
					return "";
				},
				expected,
				1,
				{},
			),
		).rejects.toThrow("PIJ_NATIVE_UNEXPECTED_APPROVAL");
		expect(keys).toHaveLength(1);
	});

	it("approves once only after two exact matches and retains before/after screens", async () => {
		const before = dialog({ message: expected.message, to: expected.to });
		const after = "PIJ_NATIVE_DONE\n❯";
		const frames = [before, before, before, after];
		const commands: string[][] = [];
		const witness: Record<string, unknown> = {};
		await confirmSmokeToolInvocation(
			(command) => {
				commands.push(command);
				if (command[0] === "capture-pane") return frames.shift() ?? after;
				expect(witness.before).toBe(before);
				expect(witness.before_confirm).toBe(before);
				return "";
			},
			expected,
			1,
			witness,
		);
		expect(commands.filter((command) => command[0] === "send-keys")).toEqual([
			["send-keys", "-t", expected.pane, "Enter"],
		]);
		expect(witness).toMatchObject({
			kind: "native-tool-human-approval",
			expected,
			status: "dismissed",
			decision: "approve-once",
			keys: ["Enter"],
			before,
			before_confirm: before,
			after_input: before,
			after,
		});
	});

	it("accepts immediate dismissal without another capture or approval key", async () => {
		const after = "PIJ_NATIVE_DONE\n❯";
		const frames = [dialog(), dialog(), after];
		const commands: string[][] = [];
		const witness: Record<string, unknown> = {};
		await confirmSmokeToolInvocation(
			(command) => {
				commands.push(command);
				if (command[0] !== "capture-pane") return "";
				const frame = frames.shift();
				if (frame === undefined) throw new Error("capture after observed dismissal");
				return frame;
			},
			expected,
			1,
			witness,
		);
		expect(commands.filter((command) => command[0] === "send-keys")).toEqual([
			["send-keys", "-t", expected.pane, "Enter"],
		]);
		expect(witness).toMatchObject({
			status: "dismissed",
			confirmation_sent: true,
			after_input: after,
			after,
		});
	});

	it.each([
		["wrong peer despite correct transcript", dialog({ ...args, to: "pij-wrong-peer" })],
		[
			"wrong nonce despite correct transcript",
			dialog({ ...args, message: `${args.message}-extra` }),
		],
		["extra arguments", dialog({ ...args, permission: "all" })],
		[
			"unknown tool",
			dialog().replace('Run extension tool "pij_send"', 'Run extension tool "shell"'),
		],
		["unknown dialog", dialog().replace('Run extension tool "pij_send"', "Confirm folder trust")],
		[
			"session approval selected",
			dialog().replace("❯ 1. Yes", "1. Yes").replace("│ 2. Yes", "│ ❯ 2. Yes"),
		],
		["missing selection", dialog().replace("❯ 1. Yes", "1. Yes")],
		["malformed JSON", dialog().replace('│   "to":', '│   "to" INVALID:')],
		["missing arguments", dialog().replace("│ { │", "│ missing │")],
		[
			"multiple argument objects",
			dialog().replace("│ ❯ 1. Yes │", '│ { │\n│ "to": "wrong" │\n│ } │\n│ ❯ 1. Yes │'),
		],
	])("rejects %s without pressing any key", async (_label, terminal) => {
		const commands: string[][] = [];
		const witness: Record<string, unknown> = {};
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					commands.push(command);
					return terminal;
				},
				expected,
				1,
				witness,
			),
		).rejects.toThrow("PIJ_NATIVE_UNEXPECTED_APPROVAL");
		expect(commands.every((command) => command[0] === "capture-pane")).toBe(true);
		expect(witness).toMatchObject({ status: "failed", before: terminal });
		expect(witness.decision).toBe("withhold");
	});

	it("rejects changed arguments at the last pre-key capture", async () => {
		const before = dialog();
		const changed = dialog({ ...args, to: "pij-wrong-peer" });
		const frames = [before, changed];
		const commands: string[][] = [];
		const witness: Record<string, unknown> = {};
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					commands.push(command);
					return frames.shift() ?? changed;
				},
				expected,
				1,
				witness,
			),
		).rejects.toThrow("PIJ_NATIVE_UNEXPECTED_APPROVAL");
		expect(commands.every((command) => command[0] === "capture-pane")).toBe(true);
		expect(witness).toMatchObject({ status: "failed", before, before_confirm: changed });
	});

	it.each([
		["unknown dialog", dialog().replace('Run extension tool "pij_send"', "Confirm folder trust")],
		["malformed dialog", dialog().replace('│   "to":', '│   "to" INVALID:')],
	])("fails %s immediately after the one-time confirmation", async (_label, terminal) => {
		const frames = [dialog(), dialog(), terminal, "ordinary composer"];
		const commands: string[][] = [];
		const witness: Record<string, unknown> = {};
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					commands.push(command);
					return command[0] === "capture-pane" ? (frames.shift() ?? "ordinary composer") : "";
				},
				expected,
				1,
				witness,
			),
		).rejects.toThrow("PIJ_NATIVE_UNEXPECTED_APPROVAL");
		expect(commands.filter((command) => command[0] === "send-keys")).toHaveLength(1);
		expect(witness).toMatchObject({ status: "failed", after_input: terminal });
	});

	it("times out without a modal and never resends approval if dismissal stalls", async () => {
		expect(verifyToolApproval(`Previous user ${JSON.stringify(args)}\n❯`, args)).toBe(false);
		for (const terminal of ["ordinary composer", dialog()]) {
			const commands: string[][] = [];
			const witness: Record<string, unknown> = {};
			await expect(
				confirmSmokeToolInvocation(
					(command) => {
						commands.push(command);
						return terminal;
					},
					expected,
					1,
					witness,
				),
			).rejects.toThrow("PIJ_NATIVE_TIMEOUT");
			expect(commands.filter((command) => command[0] === "send-keys")).toHaveLength(
				terminal === "ordinary composer" ? 0 : 1,
			);
			expect(witness.status).toBe("failed");
		}
	});

	it("retains capture failures after confirmation without pressing again", async () => {
		const commands: string[][] = [];
		const witness: Record<string, unknown> = {};
		await expect(
			confirmSmokeToolInvocation(
				(command) => {
					commands.push(command);
					if (commands.length > 3) throw new Error("pane exited after consent");
					return dialog();
				},
				expected,
				1,
				witness,
			),
		).rejects.toThrow("pane exited after consent");
		expect(commands.filter((command) => command[0] === "send-keys")).toHaveLength(1);
		expect(witness).toMatchObject({ decision: "approve-once", status: "failed", before: dialog() });
	});

	describe("retained preauthorized execution, not human approval", () => {
		type Preauthorization = NonNullable<Parameters<typeof confirmSmokeToolInvocation>[4]>;
		type Sample = ReturnType<Preauthorization["observe"]>;
		type Fixture = {
			launch: Preauthorization["launch"];
			incoming_event_id: string;
			expected: Parameters<typeof confirmSmokeToolInvocation>[1];
			native_events: Sample["nativeEvents"];
			jobs: Sample["jobs"];
			terminal: string;
		};
		const retained = JSON.parse(
			readFileSync(
				new URL("./fixtures/copilot-native-preauthorized.json", import.meta.url),
				"utf8",
			),
		) as Fixture;
		const context = (fixture: Fixture): Preauthorization => ({
			launch: fixture.launch,
			incomingEventId: fixture.incoming_event_id,
			observe: () => ({ nativeEvents: fixture.native_events, jobs: fixture.jobs }),
		});
		const event = (fixture: Fixture, type: string) => {
			const value = fixture.native_events.find((item) => item.type === type);
			if (!value) throw new Error(`Retained fixture missing ${type}`);
			return value;
		};
		const changePayload = (fixture: Fixture, key: string, value: string) => {
			const row = fixture.jobs[0];
			if (!row) throw new Error("Retained fixture missing durable outgoing");
			row.payload = JSON.stringify({ ...object(JSON.parse(String(row.payload))), [key]: value });
		};
		it("proves actual native execution and durable outgoing without consent keys", async () => {
			const keys: string[][] = [];
			const witness: Record<string, unknown> = {};
			await confirmSmokeToolInvocation(
				(args) => {
					if (args[0] === "send-keys") keys.push(args);
					return retained.terminal;
				},
				retained.expected,
				1,
				witness,
				context(retained),
			);
			expect(keys).toEqual([]);
			expect(witness).toMatchObject({
				kind: "native-tool-preauthorized-execution",
				decision: "observe-existing-preauthorization",
				status: "preauthorized-executed",
				grade: "native-tool-executed-not-human-approved-not-model-complete",
				launch_policy: retained.launch,
				before: retained.terminal,
				before_complete: retained.terminal,
				proof: {
					start: event(retained, "tool.execution_start"),
					completed: event(retained, "tool.execution_complete"),
					outbound: retained.jobs[0],
				},
			});
			expect(witness.confirmation_sent).toBeUndefined();
		});
		it("waits keylessly for matching completion after invocation", async () => {
			const witness: Record<string, unknown> = {};
			const preauthorized = context(retained);
			let observations = 0;
			await confirmSmokeToolInvocation(
				(args) => {
					expect(args[0]).toBe("capture-pane");
					return retained.terminal;
				},
				retained.expected,
				1000,
				witness,
				{
					...preauthorized,
					observe: () =>
						observations++ === 0
							? {
									nativeEvents: retained.native_events.filter(
										(e) => e.type !== "tool.execution_complete",
									),
									jobs: [],
								}
							: preauthorized.observe(),
				},
			);
			expect(observations).toBe(2);
			expect(witness.status).toBe("preauthorized-executed");
			expect(witness.confirmation_sent).toBeUndefined();
		});
		it.each<[string, (fixture: Fixture) => void]>([
			[
				"late permission grant after execution",
				(f) => {
					const permission = event(f, "session.permissions_changed");
					const revoked = {
						...structuredClone(permission),
						id: "permissions-revoked",
						data: { allowAllPermissions: false, allowAllPermissionMode: "off" },
					};
					f.native_events.splice(
						f.native_events.indexOf(event(f, "tool.execution_start")),
						0,
						revoked,
					);
					f.native_events.splice(
						f.native_events.indexOf(event(f, "tool.execution_complete")) + 1,
						0,
						{ ...structuredClone(permission), id: "permissions-granted-later" },
					);
				},
			],
			[
				"dialog absence without execution",
				(f) => {
					f.native_events = f.native_events.filter((e) => !e.type.startsWith("tool.execution_"));
				},
			],
			[
				"missing tool completion",
				(f) => {
					f.native_events = f.native_events.filter((e) => e.type !== "tool.execution_complete");
				},
			],
			[
				"wrong tool",
				(f) => {
					event(f, "tool.execution_start").data.toolName = "other_tool";
				},
			],
			[
				"wrong tool recipient",
				(f) => {
					object(event(f, "tool.execution_start").data.arguments).to = "wrong-peer";
				},
			],
			[
				"wrong tool body",
				(f) => {
					object(event(f, "tool.execution_start").data.arguments).message = "wrong-body";
				},
			],
			[
				"extra tool arguments",
				(f) => {
					object(event(f, "tool.execution_start").data.arguments).extra = true;
				},
			],
			[
				"unrelated completion",
				(f) => {
					event(f, "tool.execution_complete").data.toolCallId = "other-call";
				},
			],
			[
				"native tool failure",
				(f) => {
					event(f, "tool.execution_complete").data.success = false;
				},
			],
			[
				"completion before invocation",
				(f) => {
					const start = event(f, "tool.execution_start");
					const end = event(f, "tool.execution_complete");
					const startIndex = f.native_events.indexOf(start);
					const endIndex = f.native_events.indexOf(end);
					[f.native_events[startIndex], f.native_events[endIndex]] = [end, start];
				},
			],
			[
				"duplicate invocation",
				(f) => {
					f.native_events.push(structuredClone(event(f, "tool.execution_start")));
				},
			],
			[
				"missing durable outgoing",
				(f) => {
					f.jobs = [];
				},
			],
			["wrong durable sender", (f) => changePayload(f, "from", "wrong-peer")],
			["wrong durable recipient", (f) => changePayload(f, "to", "wrong-peer")],
			["wrong durable body", (f) => changePayload(f, "body", "wrong-body")],
			["wrong durable message ID", (f) => changePayload(f, "msg_id", "other-message")],
			[
				"unrelated durable job",
				(f) => {
					f.jobs = f.jobs.map((row) => ({ ...row, dedupe_key: "other-message" }));
				},
			],
			[
				"failed outgoing receipt",
				(f) => {
					object(event(f, "tool.execution_complete").data.result).content = '{"ok":false}';
				},
			],
			[
				"malformed tool result",
				(f) => {
					object(event(f, "tool.execution_complete").data.result).content = "not-json";
				},
			],
			[
				"missing launch permission",
				(f) => {
					f.launch.process = f.launch.process.replace(" --yolo", "");
				},
			],
			[
				"permission substring only",
				(f) => {
					f.launch.process = f.launch.process.replace("--yolo", "--yolo-disabled");
				},
			],
			[
				"revoked native permissions",
				(f) => {
					event(f, "session.permissions_changed").data.allowAllPermissions = false;
				},
			],
			[
				"wrong native session",
				(f) => {
					event(f, "session.start").data.sessionId = "other-session";
				},
			],
			[
				"wrong launch PID",
				(f) => {
					f.launch.seat.proc.pid++;
				},
			],
			[
				"wrong launch pane",
				(f) => {
					f.launch.seat.pane = "%999";
				},
			],
		])("refuses %s without pretending human consent", async (_label, change) => {
			const fixture = structuredClone(retained);
			change(fixture);
			const keys: string[][] = [];
			const witness: Record<string, unknown> = {};
			await expect(
				confirmSmokeToolInvocation(
					(args) => {
						if (args[0] === "send-keys") keys.push(args);
						return fixture.terminal;
					},
					fixture.expected,
					1,
					witness,
					context(fixture),
				),
			).rejects.toThrow(/PIJ_NATIVE_/);
			expect(keys).toEqual([]);
			expect(witness.status).toBe("failed");
			expect(witness.confirmation_sent).toBeUndefined();
		});
		it.each([
			dialog({ to: retained.expected.to, message: retained.expected.message }),
			dialog({ to: "wrong-peer", message: retained.expected.message }),
			dialog().replace('Run extension tool "pij_send"', "Confirm folder trust"),
			dialog().replace("╰────────────────╯", ""),
		])("never approves active or incomplete modal on preauthorized path", async (terminal) => {
			for (const atRecheck of [false, true]) {
				let captures = 0;
				const keys: string[][] = [];
				const witness: Record<string, unknown> = {};
				await expect(
					confirmSmokeToolInvocation(
						(args) => {
							if (args[0] === "send-keys") keys.push(args);
							return atRecheck && captures++ === 0 ? retained.terminal : terminal;
						},
						retained.expected,
						1,
						witness,
						context(retained),
					),
				).rejects.toThrow(/PIJ_NATIVE_/);
				expect(keys).toEqual([]);
				expect(witness.status).toBe("failed");
			}
		});
	});
});

describe("native lifecycle assertion boundaries, not live CLI proof", () => {
	const composer = (text = "") =>
		`Previous user /new\n╻▄▄▄▄▄▄\n┃ ${text}\n╹▀▀▀▀▀▀\n← open sidebar · / commands`;
	const dialog = "╭────────╮\n│ Confirm folder trust │\n╰────────╯";
	const measured = (terminal: string) => {
		const lines = terminal.split("\n");
		const y = lines.findIndex((line) => line.startsWith("┃"));
		return `${(lines[y] ?? "").trimEnd().length}\t${Math.max(0, y)}\t160\t48\n${terminal}`;
	};
	const newFixture = object(
		JSON.parse(
			readFileSync(new URL("./fixtures/copilot-native-new-command.json", import.meta.url), "utf8"),
		),
	);
	const newFrame = object(newFixture.pane_capture);

	it.each<[string, string, (screen: string) => string]>([
		["cursor before command end", "5\t45\t160\t48", (screen) => screen],
		["cursor inside placeholder", "7\t45\t160\t48", (screen) => screen],
		["placeholder actually typed", "15\t45\t160\t48", (screen) => screen],
		["cursor on suggestion", "6\t38\t160\t48", (screen) => screen],
		["cursor outside pane", "6\t45\t160\t45", (screen) => screen],
		["missing measured cursor", "", (screen) => screen],
		[
			"unknown ghost",
			"6\t45\t160\t48",
			(screen) => screen.replace("┃ /new [prompt]", "┃ /new [other]"),
		],
		[
			"wrong selected command",
			"6\t45\t160\t48",
			(screen) => screen.replace("  ❯ /new ", "  ❯ /fork "),
		],
		[
			"prefix-only selection match",
			"6\t45\t160\t48",
			(screen) => screen.replace("  ❯ /new ", "  ❯ /newer "),
		],
		[
			"plain command with wrong selection",
			"6\t45\t160\t48",
			(screen) => screen.replace("┃ /new [prompt]", "┃ /new").replace("  ❯ /new ", "  ❯ /fork "),
		],
		["missing selection", "6\t45\t160\t48", (screen) => screen.replace("  ❯ /new ", "    /new ")],
		[
			"duplicate selection",
			"6\t45\t160\t48",
			(screen) => screen.replace("    /fork ", "  ❯ /fork "),
		],
		[
			"malformed selection row",
			"6\t45\t160\t48",
			(screen) => screen.replace("  ❯ /new ", " ❯ /new "),
		],
		[
			"hidden duplicate selection across malformed indentation",
			"6\t45\t160\t48",
			(screen) => {
				const lines = screen.split("\n");
				lines[38] = "  ❯ /fork  Fork";
				lines[39] = "   /after  Schedule";
				lines[40] = "  ❯ /new  Start";
				return lines.join("\n");
			},
		],
		[
			"hidden duplicate selection across unknown menu text",
			"6\t45\t160\t48",
			(screen) => {
				const lines = screen.split("\n");
				lines[38] = "  ❯ /fork  Fork";
				lines[39] = "  unexpected menu repaint";
				lines[40] = "  ❯ /new  Start";
				return lines.join("\n");
			},
		],
		["incomplete composer repaint", "6\t45\t160\t48", (screen) => screen.replace(/^╹▀.*$/m, "")],
		[
			"historical selection only",
			"6\t45\t160\t48",
			(screen) => {
				const lines = screen.split("\n");
				lines[20] = "  ❯ /new                                Start a new conversation";
				lines.fill("", 38, 43);
				return lines.join("\n");
			},
		],
	])("never submits a measured lifecycle command with %s", async (_label, metadata, change) => {
		for (const atRecheck of [false, true]) {
			const keys: string[][] = [];
			const witness: Record<string, unknown> = {};
			let captures = 0;
			const changed = change(String(newFrame.terminal));
			await expect(
				driveSmokeLifecycleCommand(
					(args) => {
						if (args[0] === "display-message") {
							if (atRecheck && captures++ === 0)
								return `${newFrame.metadata_raw}\n${newFrame.terminal}`;
							return `${metadata}\n${changed}`;
						}
						if (args[0] === "capture-pane") return String(newFixture.before);
						keys.push(args);
						return "";
					},
					{ pane: "%3", action: "newSession" },
					1,
					witness,
				),
			).rejects.toThrow(/PIJ_NATIVE_/);
			expect(keys).toEqual([["send-keys", "-t", "%3", "-l", "--", "/new"]]);
			expect(witness.command_submitted).toBeUndefined();
			expect(witness.status).toBe("failed");
			expect(
				witness[atRecheck ? "before_submit_pane_capture" : "staged_pane_capture"],
			).toMatchObject({ terminal: changed, metadata_raw: metadata });
		}
	});

	it("recognizes the retained disabled CLI frame as an empty ready composer", () => {
		const frame = object(
			JSON.parse(
				readFileSync(new URL("./fixtures/copilot-native-disabled.json", import.meta.url), "utf8"),
			),
		);
		expect(nativeComposer(String(frame.terminal))).toBe("");
	});

	it.each([
		["/new", "newSession", "new", 6],
		["/exit", "exit", "exit", 7],
	] as const)("submits only %s from the retained measured ghost frame", async (command, action, name, cursorX) => {
		const fixture = object(
			JSON.parse(
				readFileSync(
					new URL(`./fixtures/copilot-native-${name}-command.json`, import.meta.url),
					"utf8",
				),
			),
		);
		const frame = object(fixture.pane_capture);
		const keys: string[][] = [];
		const witness: Record<string, unknown> = {};
		let held = false;
		await driveSmokeLifecycleCommand(
			(args, preserveOutput) => {
				if (args[0] === "display-message") {
					expect(preserveOutput).toBe(true);
					expect(args).toEqual([
						"display-message",
						"-p",
						"-t",
						"%3",
						"#{cursor_x}\t#{cursor_y}\t#{pane_width}\t#{pane_height}",
						";",
						"capture-pane",
						"-p",
						"-t",
						"%3",
					]);
					return `${frame.metadata_raw}\n${frame.terminal}`;
				}
				if (args[0] === "capture-pane") {
					return keys.length === 0 ? String(fixture.before) : String(fixture.staged);
				}
				if (args.at(-1) === "Enter") expect(held).toBe(true);
				keys.push(args);
				return "";
			},
			{ pane: "%3", action },
			1,
			witness,
			async () => {
				held = true;
			},
		);
		expect(keys).toEqual([
			["send-keys", "-t", "%3", "-l", "--", command],
			["send-keys", "-t", "%3", "Enter"],
		]);
		expect(witness).toMatchObject({
			staged: frame.terminal,
			staged_pane_capture: { cursor_x: cursorX, cursor_y: 45, terminal: frame.terminal },
			before_submit_pane_capture: { cursor_x: cursorX, cursor_y: 45, terminal: frame.terminal },
			command_submitted: true,
			status: "submitted-not-yet-lifecycle-proven",
		});
	});

	const exitFixture = object(
		JSON.parse(
			readFileSync(new URL("./fixtures/copilot-native-exit-command.json", import.meta.url), "utf8"),
		),
	);
	const exitFrame = object(exitFixture.pane_capture);
	it.each<[string, string, (screen: string) => string]>([
		["typed placeholder", "15\t45\t160\t48", (screen) => screen],
		["cursor inside ghost", "8\t45\t160\t48", (screen) => screen],
		["cursor on suggestion", "7\t38\t160\t48", (screen) => screen],
		[
			"unknown ghost",
			"7\t45\t160\t48",
			(screen) => screen.replace("/exit [print]", "/exit [unknown]"),
		],
		[
			"typed print argument",
			"13\t45\t160\t48",
			(screen) => screen.replace("/exit [print]", "/exit print"),
		],
		[
			"selected print alternative",
			"7\t45\t160\t48",
			(screen) => screen.replace("  ❯ /exit\n    /exit print", "    /exit\n  ❯ /exit print"),
		],
		["wrong selection", "7\t45\t160\t48", (screen) => screen.replace("  ❯ /exit\n", "  ❯ /new\n")],
		[
			"missing selection",
			"7\t45\t160\t48",
			(screen) => screen.replace("  ❯ /exit\n", "    /exit\n"),
		],
		[
			"duplicate selection",
			"7\t45\t160\t48",
			(screen) => screen.replace("    /exit print", "  ❯ /exit print"),
		],
		[
			"malformed menu",
			"7\t45\t160\t48",
			(screen) => screen.replace("    /exit print", "   /exit print"),
		],
		["incomplete composer", "7\t45\t160\t48", (screen) => screen.replace(/^╹▀.*$/m, "")],
		[
			"historical selection",
			"7\t45\t160\t48",
			(screen) => {
				const lines = screen.split("\n");
				lines[30] = "  ❯ /exit";
				lines[38] = "";
				lines[39] = "";
				return lines.join("\n");
			},
		],
	])("never submits retained exit ghost with %s", async (_label, metadata, change) => {
		for (const atRecheck of [false, true]) {
			const keys: string[][] = [];
			const witness: Record<string, unknown> = {};
			let captures = 0;
			const changed = change(String(exitFrame.terminal));
			await expect(
				driveSmokeLifecycleCommand(
					(args) => {
						if (args[0] === "display-message") {
							if (atRecheck && captures++ === 0)
								return `${exitFrame.metadata_raw}\n${exitFrame.terminal}`;
							return `${metadata}\n${changed}`;
						}
						if (args[0] === "capture-pane") return String(exitFixture.before);
						keys.push(args);
						return "";
					},
					{ pane: "%3", action: "exit" },
					1,
					witness,
				),
			).rejects.toThrow(/PIJ_NATIVE_/);
			expect(keys).toEqual([["send-keys", "-t", "%3", "-l", "--", "/exit"]]);
			expect(witness.command_submitted).toBeUndefined();
			expect(witness.status).toBe("failed");
			expect(
				witness[atRecheck ? "before_submit_pane_capture" : "staged_pane_capture"],
			).toMatchObject({ terminal: changed, metadata_raw: metadata });
		}
	});

	it.each([
		"newSession",
		"exit",
	] as const)("stages and submits only the documented %s command", async (action) => {
		const command = action === "newSession" ? "/new" : "/exit";
		const frames = [composer(), composer(command), composer(command), composer()];
		const keys: string[][] = [];
		const witness: Record<string, unknown> = {};
		let heldBeforeSubmit = false;
		await driveSmokeLifecycleCommand(
			(args) => {
				if (args[0] === "capture-pane") return frames.shift() ?? composer();
				if (args[0] === "display-message") return measured(frames.shift() ?? composer());
				if (args.at(-1) === "Enter") expect(heldBeforeSubmit).toBe(true);
				keys.push(args);
				return "";
			},
			{ pane: "%9", action },
			1,
			witness,
			async () => {
				expect(keys).toEqual([["send-keys", "-t", "%9", "-l", "--", command]]);
				heldBeforeSubmit = true;
			},
		);
		expect(keys).toEqual([
			["send-keys", "-t", "%9", "-l", "--", command],
			["send-keys", "-t", "%9", "Enter"],
		]);
		expect(witness).toMatchObject({
			command,
			before: composer(),
			staged: composer(command),
			before_submit: composer(command),
			after: composer(),
			command_submitted: true,
		});
	});

	it("uses only the composer and rejects unknown dialogs and existing drafts without keys", async () => {
		expect(nativeComposer(composer("/exit"))).toBe("/exit");
		expect(nativeComposer("Previous user /new")).toBeUndefined();
		for (const terminal of [dialog, composer("human draft")]) {
			const keys: string[][] = [];
			const witness: Record<string, unknown> = {};
			await expect(
				driveSmokeLifecycleCommand(
					(args) => {
						if (args[0] === "send-keys") keys.push(args);
						return terminal;
					},
					{ pane: "%9", action: "newSession" },
					1,
					witness,
				),
			).rejects.toThrow(/PIJ_NATIVE_/);
			expect(keys).toEqual([]);
			expect(witness).toMatchObject({ status: "failed", before: terminal });
		}
	});

	it.each([
		composer("/exit"),
		dialog,
	])("does not submit a changed staged control", async (changed) => {
		const frames = [composer(), composer("/new"), changed];
		const keys: string[][] = [];
		await expect(
			driveSmokeLifecycleCommand(
				(args) => {
					if (args[0] === "capture-pane") return frames.shift() ?? changed;
					if (args[0] === "display-message") return measured(frames.shift() ?? changed);
					keys.push(args);
					return "";
				},
				{ pane: "%9", action: "newSession" },
				1,
				{},
			),
		).rejects.toThrow(/PIJ_NATIVE_/);
		expect(keys).toEqual([["send-keys", "-t", "%9", "-l", "--", "/new"]]);
	});

	it("retains an exited pane capture error without claiming host death", async () => {
		const frames = [composer(), composer("/exit"), composer("/exit")];
		const witness: Record<string, unknown> = {};
		await driveSmokeLifecycleCommand(
			(args) => {
				if (args[0] !== "capture-pane" && args[0] !== "display-message") return "";
				const frame = frames.shift();
				if (frame === undefined) throw new Error("owned pane exited");
				return args[0] === "display-message" ? measured(frame) : frame;
			},
			{ pane: "%9", action: "exit" },
			1,
			witness,
		);
		expect(witness).toMatchObject({
			command_submitted: true,
			after_capture_error: "Error: owned pane exited",
			status: "submitted-not-yet-lifecycle-proven",
		});
		expect(witness.host_death).toBeUndefined();
	});

	const previous = {
		id: "pij-old",
		session: "00000000-0000-4000-8000-000000000137",
		pane: "%9",
		harness: "copilot",
		native_extension_delivery: true,
		proc: { pid: 137, proc_start: 1 },
	};
	const successor = { ...previous, id: "pij-new", session: "00000000-0000-4000-8000-000000000138" };
	describe("owned native host teardown", () => {
		function setup() {
			const calls: string[][] = [];
			const probes: number[][] = [];
			const state: {
				pane: boolean;
				alive: boolean;
				socket: string;
				keepAlive?: boolean;
				closeError?: Error;
				recheckError?: Error;
				probeError?: Error;
				deathProbeError?: Error;
				exitDuringClose?: boolean;
			} = { pane: true, alive: true, socket: "/isolated-run/tmux.sock" };
			const expected = { previous: successor, launch: previous, socket: state.socket };
			const witness: Record<string, unknown> = {};
			let closeAttempted = false;
			const io: Parameters<typeof stopSmokeNativeHost>[0] = {
				tmux(args) {
					calls.push(args);
					if (args[0] === "display-message") {
						return args.includes("#{socket_path}") ? state.socket : measured(composer());
					}
					if (args[0] === "list-panes") {
						if (closeAttempted && state.recheckError) throw state.recheckError;
						return state.pane ? "%0\n%9\n%12" : "%0\n%12";
					}
					if (args[0] === "kill-pane") {
						expect(args).toEqual(["kill-pane", "-t", "%9"]);
						closeAttempted = true;
						if (!state.closeError || state.exitDuringClose) {
							state.pane = false;
							if (!state.keepAlive) state.alive = false;
						}
						if (state.closeError) throw state.closeError;
						return "";
					}
					throw new Error(`Unexpected tmux action ${args.join(" ")}`);
				},
				probe(pid, signal) {
					probes.push([pid, signal]);
					if (closeAttempted && state.deathProbeError) throw state.deathProbeError;
					if (state.probeError) throw state.probeError;
					if (state.alive) return true;
					throw Object.assign(new Error("No such process"), { code: "ESRCH" });
				},
			};
			return { calls, probes, state, expected, witness, io };
		}
		it("closes the owned pane after tab-only exit and proves original host death", async () => {
			const { calls, probes, expected, witness, io } = setup();
			const before = structuredClone(expected);
			const death = await stopSmokeNativeHost(io, expected, 1, witness);
			expect(death).toMatchObject({ ...successor.proc, evidence: "kill(pid, 0): ESRCH" });
			expect(witness).toMatchObject({
				ownership: { socket: expected.socket, launch: previous },
				previous: successor,
				after_native_exit: { ...successor.proc, state: "alive" },
				before_teardown_pane_capture: { pane: "%9", terminal: composer() },
				teardown_command: { socket: expected.socket, args: ["kill-pane", "-t", "%9"] },
				decision: "closed-owned-pane",
				status: "host-death-proven-not-resume-proven",
				host_death: death,
			});
			expect(calls.map((args) => args[0])).toEqual([
				"display-message",
				"list-panes",
				"display-message",
				"kill-pane",
			]);
			expect(probes).toEqual([
				[137, 0],
				[137, 0],
			]);
			expect(expected).toEqual(before);
		});
		it("records an already-exited pane and process without attempting closure", async () => {
			const { calls, state, expected, witness, io } = setup();
			state.pane = false;
			state.alive = false;
			await stopSmokeNativeHost(io, expected, 1, witness);
			expect(witness).toMatchObject({
				decision: "owned-pane-already-absent",
				after_native_exit: { code: "ESRCH" },
				host_death: { ...successor.proc, evidence: "kill(pid, 0): ESRCH" },
			});
			expect(calls.some((args) => args[0] === "kill-pane")).toBe(false);
		});
		it("accepts a natural exit racing closure only with subsequent ESRCH", async () => {
			const { state, expected, witness, io } = setup();
			state.closeError = new Error("pane already gone");
			state.exitDuringClose = true;
			await stopSmokeNativeHost(io, expected, 1, witness);
			expect(witness).toMatchObject({
				decision: "pane-disappeared-during-teardown",
				teardown_error: "Error: pane already gone",
				panes_after_failed_teardown: "%0\n%12",
				host_death: { evidence: "kill(pid, 0): ESRCH" },
			});
		});
		it.each([
			false,
			true,
		])("refuses a surviving process when pane initially absent=%s", async (absent) => {
			const { state, expected, witness, io } = setup();
			state.pane = !absent;
			state.keepAlive = true;
			await expect(stopSmokeNativeHost(io, expected, 1, witness)).rejects.toThrow(
				"PIJ_NATIVE_TIMEOUT: actual native host exits before resume",
			);
			expect(witness.status).toBe("failed");
			expect(witness.host_death).toBeUndefined();
			expect(witness.last_liveness).toMatchObject({ ...successor.proc, state: "alive" });
		});
		it.each([
			false,
			true,
		])("retains original teardown failure when recheck also fails=%s", async (recheckFails) => {
			const { state, expected, witness, io } = setup();
			const original = new Error("owned pane teardown refused");
			state.closeError = original;
			if (recheckFails) state.recheckError = new Error("diagnostic socket unavailable");
			await expect(stopSmokeNativeHost(io, expected, 1, witness)).rejects.toBe(original);
			expect(witness).toMatchObject({
				status: "failed",
				error: String(original),
				teardown_error: String(original),
				previous: successor,
			});
			expect(witness.host_death).toBeUndefined();
		});
		it.each([
			"timeout",
			"EPERM",
		])("preserves teardown failure when raced death proof yields %s", async (failure) => {
			const { state, expected, witness, io } = setup();
			const original = new Error("original owned pane teardown error");
			state.closeError = original;
			state.exitDuringClose = true;
			state.keepAlive = true;
			if (failure === "EPERM")
				state.deathProbeError = Object.assign(new Error("probe denied"), { code: "EPERM" });
			await expect(stopSmokeNativeHost(io, expected, 1, witness)).rejects.toBe(original);
			expect(witness).toMatchObject({
				status: "failed",
				error: String(original),
				teardown_error: String(original),
				death_proof_error:
					failure === "EPERM"
						? "Error: probe denied"
						: "Error: PIJ_NATIVE_TIMEOUT: actual native host exits before resume",
				panes_after_failed_teardown: "%0\n%12",
			});
			expect(witness.host_death).toBeUndefined();
		});
		it.each(["EPERM", "EACCES", undefined])("does not treat %s as process death", async (code) => {
			const { calls, state, expected, witness, io } = setup();
			const error = Object.assign(new Error("probe failed"), { code });
			state.probeError = error;
			await expect(stopSmokeNativeHost(io, expected, 1, witness)).rejects.toBe(error);
			expect(witness.host_death).toBeUndefined();
			expect(calls.some((args) => args[0] === "kill-pane")).toBe(false);
		});
		it.each([
			["wrong socket", { socket: "/other/tmux.sock" }],
			["unowned pane", { launch: { ...previous, pane: "%12" } }],
			["other PID", { launch: { ...previous, proc: { ...previous.proc, pid: 138 } } }],
			[
				"other process start",
				{ launch: { ...previous, proc: { ...previous.proc, proc_start: 2 } } },
			],
			["unattested launch", { launch: { ...previous, native_extension_delivery: false } }],
		] as const)("refuses %s before probing or teardown", async (_label, mismatch) => {
			const { calls, probes, expected, witness, io } = setup();
			await expect(
				stopSmokeNativeHost(io, { ...expected, ...mismatch }, 1, witness),
			).rejects.toThrow("PIJ_NATIVE_HOST_OWNERSHIP");
			expect(probes).toEqual([]);
			expect(calls.some((args) => args[0] === "kill-pane")).toBe(false);
			expect(witness.host_death).toBeUndefined();
		});
	});
	it("requires a new address on /new but stable identity on fresh-host resume", () => {
		expect(() => verifyNativeLifecycleIdentity(previous, successor, "new")).not.toThrow();
		expect(() =>
			verifyNativeLifecycleIdentity(
				successor,
				{ ...successor, proc: { pid: 138, proc_start: 2 } },
				"resume",
			),
		).not.toThrow();
	});
	it.each([
		{ ...successor, session: previous.session },
		{ ...successor, id: previous.id },
		{ ...successor, pane: "%10" },
		{ ...successor, proc: { pid: 138, proc_start: 2 } },
		{ ...successor, native_extension_delivery: false },
	])("rejects untruthful /new registration", (current) => {
		expect(() => verifyNativeLifecycleIdentity(previous, current, "new")).toThrow();
	});
	it.each([
		previous,
		{ ...previous, id: "different", proc: { pid: 138, proc_start: 2 } },
		{ ...previous, session: successor.session, proc: { pid: 138, proc_start: 2 } },
	])("rejects fake or identity-changing resume", (current) => {
		expect(() => verifyNativeLifecycleIdentity(previous, current, "resume")).toThrow();
	});

	const expected = {
		seat: previous.id,
		session: previous.session,
		msgId: "old-id",
		nonce: "OLD_CONTEXT_NONCE",
	};
	const payload = {
		to: previous.id,
		native_target_session: previous.session,
		msg_id: expected.msgId,
		body: expected.nonce,
	};
	const job = { state: "pending", payload: JSON.stringify(payload) };
	const held = {
		jobs: [job],
		acknowledgements: [],
		predecessorEvents: [],
		successorEvents: [],
		successorJobs: [],
	};
	const oldEvent = {
		id: "event",
		type: "user.message",
		data: { content: expected.nonce, messageId: "native-id" },
	};
	it("accepts only preserved unacknowledged old-context work", () => {
		expect(() => verifyNativeContextHeld(expected, held)).not.toThrow();
	});
	it.each([
		{ jobs: [] },
		{ jobs: [{ ...job, state: "done" }] },
		{ acknowledgements: [{ recipient: successor.id, msg_id: expected.msgId }] },
		{ predecessorEvents: [oldEvent] },
		{ successorEvents: [oldEvent] },
		{ successorJobs: [job] },
	])("rejects lost, consumed, acknowledged or migrated old-context work", (changed) => {
		expect(() => verifyNativeContextHeld(expected, { ...held, ...changed })).toThrow();
	});
	it.each([
		"to",
		"native_target_session",
		"msg_id",
		"body",
	] as const)("rejects changed old-context %s", (field) => {
		expect(() =>
			verifyNativeContextHeld(expected, {
				...held,
				jobs: [{ ...job, payload: JSON.stringify({ ...payload, [field]: "changed" }) }],
			}),
		).toThrow();
	});
});

// Native typing and self-reported status never hold delivery; consent and native-target isolation still do.
describe("AC6 delivery-while-typing witness boundaries, not live CLI proof", () => {
	const seat = {
		id: "pij-target",
		session: "00000000-0000-4000-8000-000000000137",
		pane: "%9",
		harness: "copilot",
		proc: { pid: 137, proc_start: 1370 },
		native_extension_delivery: true,
	};
	const expected = { seat, draft: "HUMAN_DRAFT_137", nonce: "PIJ_TYPING_137", msgId: "msg-137" };
	const composer = (draft: string) => `Transcript\n╻▄▄▄▄▄▄\n┃ ${draft}\n╹▀▀▀▀▀▀`;
	const event = (seq: number, kind: string, payload: Record<string, unknown>) => ({
		seq,
		at: 100000 + seq,
		kind,
		seat: seat.id,
		payload: JSON.stringify({ seat: seat.id, msg_id: expected.msgId, ...payload }),
	});
	const ackEvent = event(1, "delivery.inbox-ack", {
		job_id: 137,
		authenticated_machine: "local",
		evidence_grade: "machine",
		outcome: "reader-read",
	});
	const payload = {
		to: seat.id,
		msg_id: expected.msgId,
		body: expected.nonce,
		native_target_session: seat.session,
	};
	const job = { id: 137, state: "done", payload: JSON.stringify(payload) };
	const acceptedEvent = {
		id: "native-event",
		type: "user.message",
		data: { content: expected.nonce, messageId: "native-message" },
	};
	const ack = { recipient: seat.id, msg_id: expected.msgId, origin: "reader-read" };
	type Sample = Awaited<ReturnType<Parameters<typeof driveSmokeTypingWitness>[0]["observe"]>>;
	const sample = (overrides: Partial<Sample> = {}): Sample => ({
		jobs: [job],
		acknowledgements: [ack],
		nativeEvents: [acceptedEvent],
		deliveryEvents: [ackEvent],
		...overrides,
	});
	const initial = (): Sample => ({
		jobs: [],
		acknowledgements: [],
		nativeEvents: [],
		deliveryEvents: [],
	});
	const samples = () => [initial(), sample()];
	async function run(
		observations = samples(),
		changedDraft?: string,
		beforeObservation?: () => void,
		timeout = 100,
	) {
		let draft = "";
		let sends = 0;
		const keys: string[][] = [];
		const witness: Record<string, unknown> = {};
		const result = driveSmokeTypingWitness(
			{
				tmux(args: string[]) {
					if (args[0] === "display-message")
						return `7\t2\t80\t24\n${composer(draft)}\n@ files · # issues\n`;
					if (args[0] === "capture-pane") return composer(draft);
					keys.push(args);
					draft = args.at(-1) === "BSpace" ? draft.slice(0, -1) : draft + String(args.at(-1));
					return "";
				},
				async observe() {
					beforeObservation?.();
					const next = observations.shift();
					if (!next) throw new Error("proof snapshots exhausted");
					if (changedDraft !== undefined && observations.length === 0) draft = changedDraft;
					return next;
				},
				async send() {
					sends++;
					return { outcome: "queued" };
				},
			},
			expected,
			timeout,
			witness,
		);
		return { result, keys, witness, sends: () => sends };
	}
	it("accepts native delivery while editing without a typing hold or clearing the draft", async () => {
		const proof = await run();
		await proof.result;
		expect(proof.sends()).toBe(1);
		expect(proof.keys).toEqual([
			["send-keys", "-t", seat.pane, "-l", "--", expected.draft],
			["send-keys", "-t", seat.pane, "-l", "--", "x"],
			["send-keys", "-t", seat.pane, "BSpace"],
		]);
		expect(proof.witness).toMatchObject({
			status: "passed",
			draft: expected.draft,
			draft_intact: true,
			draft_submitted: false,
			grade: "native-accepted-not-model-complete",
			draft_before: composer(expected.draft),
			after: composer(expected.draft),
		});
		expect(proof.witness.delivered_within_ms).toBeLessThanOrEqual(5000);
		expect(proof.witness).not.toHaveProperty("held");
		expect(proof.witness).not.toHaveProperty("expired");
	});
	it("retains paired cursor evidence without substituting a typing sensor", async () => {
		const proof = await run();
		await proof.result;
		for (const field of ["typing_observed", "accepted", "last_observation"]) {
			expect(object(proof.witness[field]).pane_capture).toMatchObject({
				pane: seat.pane,
				cursor_x: 7,
				cursor_y: 2,
				pane_width: 80,
				pane_height: 24,
				terminal: `${composer(expected.draft)}\n@ files · # issues\n`,
			});
			expect(object(proof.witness[field])).not.toHaveProperty("sensor");
		}
	});
	it.each([
		"",
		"HUMAN_DRAFT_corrupted",
		`${expected.draft}${expected.nonce}`,
	])("rejects cleared or corrupted composer even after native acceptance: %s", async (draft) => {
		const proof = await run(samples(), draft);
		await expect(proof.result).rejects.toThrow("same nonempty composer must remain intact");
		expect(proof.keys).toHaveLength(3);
	});
	it("rejects submission of a draft even if the editor was restored", async () => {
		const proof = await run([
			initial(),
			sample({
				nativeEvents: [
					acceptedEvent,
					{
						id: "submitted-draft",
						type: "user.message",
						data: { content: `${expected.draft}x` },
					},
				],
			}),
		]);
		await expect(proof.result).rejects.toThrow("human draft must not be submitted");
	});
	it("rejects a human-typing hold instead of waiting for release", async () => {
		const proof = await run([
			initial(),
			sample({ deliveryEvents: [ackEvent, event(2, "delivery.held", { reason: "human-typing" })] }),
		]);
		await expect(proof.result).rejects.toThrow("native delivery must not hold for human typing");
	});
	it.each([
		{ nativeEvents: [acceptedEvent] },
		{ acknowledgements: [ack] },
		{ jobs: [job] },
	])("rejects reused message identity before sending %j", async (changed) => {
		const proof = await run([{ ...initial(), ...changed }]);
		await expect(proof.result).rejects.toThrow("message identity must be unused before sending");
		expect(proof.sends()).toBe(0);
	});
	it.each([
		{ nativeEvents: [] },
		{ nativeEvents: [{ ...acceptedEvent, type: "assistant.message" }] },
		{ nativeEvents: [{ ...acceptedEvent, id: "" }] },
		{ nativeEvents: [{ ...acceptedEvent, data: { content: expected.nonce, messageId: "" } }] },
		{ nativeEvents: [acceptedEvent, { ...acceptedEvent, id: "duplicate-event" }] },
		{ acknowledgements: [] },
		{ acknowledgements: [ack, ack] },
		{ acknowledgements: [{ ...ack, origin: "tmux" }] },
		{ acknowledgements: [{ ...ack, recipient: "wrong-seat" }] },
		{ acknowledgements: [{ ...ack, msg_id: "wrong-message" }] },
		{ jobs: [] },
		{ jobs: [{ ...job, state: "pending" }] },
		{ jobs: [{ ...job, id: 0 }] },
		{ deliveryEvents: [] },
		{ deliveryEvents: [{ ...ackEvent, seat: "wrong-seat" }] },
		{ deliveryEvents: [event(1, "delivery.inbox-ack", { job_id: 138, outcome: "reader-read" })] },
		{ deliveryEvents: [event(1, "delivery.inbox-ack", { job_id: 137, outcome: "tmux" })] },
		{ deliveryEvents: [ackEvent, { ...ackEvent, seq: 2 }] },
	])("rejects missing, duplicate or uncorrelated native acceptance %j", async (changed) => {
		const proof = await run([initial(), sample(changed)]);
		await expect(proof.result).rejects.toThrow(/PIJ_NATIVE_(?:TYPING_PROOF|TIMEOUT)/);
	});
	it.each([
		"to",
		"native_target_session",
		"msg_id",
		"body",
	] as const)("rejects changed delivery job %s", async (field) => {
		const proof = await run([
			initial(),
			sample({ jobs: [{ ...job, payload: JSON.stringify({ ...payload, [field]: "changed" }) }] }),
		]);
		await expect(proof.result).rejects.toThrow("job identity or native context changed");
	});
	it("rejects delivery beyond one sweep even with a longer smoke timeout", async () => {
		let clock = 100000;
		let count = 0;
		const now = vi.spyOn(Date, "now").mockImplementation(() => clock);
		try {
			const proof = await run(
				samples(),
				undefined,
				() => {
					if (++count === 2) clock += 5001;
				},
				90000,
			);
			await expect(proof.result).rejects.toThrow("native delivery exceeded one sweep");
			expect(proof.witness.delivered_within_ms).toBe(5001);
		} finally {
			now.mockRestore();
		}
	});
});

describe("final typing API diagnostics before teardown", () => {
	const seat = {
		id: "current-seat",
		harness: "copilot",
		session: "new-native-session",
		pane: "%2",
		proc: { pid: 222, proc_start: 20260905133000 },
		native_extension_delivery: true,
	};
	function setup() {
		const calls: string[] = [];
		const sensor = {
			state: "unavailable",
			reason: "native-typing-sensor-unavailable",
			observed_at_ms: 777,
		};
		const io = {
			tmux: (args: string[], preserveOutput?: boolean) => {
				calls.push(`tmux:${args[0]}`);
				if (args[0] === "list-panes")
					return args.at(-1) === "#{pane_id}" ? "%2\n%3" : "%2 222 node\n%3 333 node";
				if (args[0] === "display-message") {
					expect(preserveOutput).toBe(true);
					return "2\t45\t160\t48\n\nactual later viewport\n\n";
				}
				if (args[0] === "capture-pane") return "joined history";
				if (args[0] === "kill-server") return "";
				throw new Error(`unexpected mutation ${args.join(" ")}`);
			},
			seats: async (_signal: AbortSignal) => {
				calls.push("GET:/v1/seats");
				return [seat];
			},
			get: async (path: string, _signal: AbortSignal): Promise<unknown> => {
				calls.push(`GET:${path}`);
				return sensor;
			},
		};
		return { io, calls, sensor };
	}
	it("captures the fresh roster tuple and nearby real viewport before cleanup without keys or sends", async () => {
		const { io, calls, sensor } = setup();
		const receipt: Record<string, unknown> = { status: "failed", error: "original typing failure" };
		expect(await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io))).toBe(true);
		const capture = object(receipt.final_typing_capture);
		expect(capture).toMatchObject({
			timing: "nearby-not-atomic",
			started_at_ms: expect.any(Number),
			completed_at_ms: expect.any(Number),
		});
		expect(capture.observations).toMatchObject([
			{
				seat,
				sensor,
				requested_at_ms: expect.any(Number),
				completed_at_ms: expect.any(Number),
				sensor_request:
					"/v1/inbox/typing?seat=current-seat&native_session=new-native-session&pid=222&proc_start=20260905133000",
				pane_capture: { cursor_x: 2, cursor_y: 45, terminal: "\nactual later viewport\n\n" },
			},
		]);
		expect(calls.filter((call) => call.startsWith("GET:"))).toEqual([
			"GET:/v1/seats",
			"GET:/v1/inbox/typing?seat=current-seat&native_session=new-native-session&pid=222&proc_start=20260905133000",
		]);
		expect(calls.at(-1)).toBe("tmux:kill-server");
		expect(receipt).toMatchObject({ status: "failed", error: "original typing failure" });
	});
	it("queries only current attested Copilot seats on the isolated socket", async () => {
		const { io, calls } = setup();
		io.seats = async () => [
			{ ...seat, id: "revoked-old", native_extension_delivery: false },
			{ ...seat, id: "foreign", pane: "%99" },
			{ ...seat, id: "non-copilot", harness: "omp" },
			seat,
		];
		const capture = await captureSmokeFinalTyping(io);
		expect(capture.observations).toHaveLength(1);
		expect(calls.filter((call) => call.startsWith("GET:/v1/inbox/typing"))).toHaveLength(1);
	});
	it("rereads current tuples and preserves a later observed result without rewriting the original failure", async () => {
		const { io, calls } = setup();
		const original = await captureSmokeFinalTyping(io);
		const current = {
			...seat,
			session: "resumed-native",
			proc: { pid: 444, proc_start: 20260905134000 },
		};
		io.seats = async () => [current];
		const observed = {
			state: "observed",
			retry_after_ms: 123,
			observed_at_ms: 888,
		};
		io.get = async (path) => {
			calls.push(`GET:${path}`);
			return observed;
		};
		const receipt: Record<string, unknown> = {
			status: "failed",
			error: "original unavailable",
			original,
		};
		await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io));
		expect(object(receipt.final_typing_capture).observations).toMatchObject([
			{ seat: current, sensor: observed },
		]);
		expect(calls).toContain(
			"GET:/v1/inbox/typing?seat=current-seat&native_session=resumed-native&pid=444&proc_start=20260905134000",
		);
		expect(receipt.original).toBe(original);
		expect(receipt.error).toBe("original unavailable");
		expect(receipt.status).toBe("failed");
	});
	it("retains invalid tuple errors without inventing identity or requesting the sensor", async () => {
		const { io, calls } = setup();
		io.seats = async () => [{ ...seat, proc: { ...seat.proc, pid: 0 } }];
		const capture = await captureSmokeFinalTyping(io);
		expect(capture.observations).toMatchObject([
			{
				error: expect.stringContaining("invalid current native tuple"),
				pane_capture: { cursor_x: 2 },
			},
		]);
		expect(calls.some((call) => call.startsWith("GET:/v1/inbox/typing"))).toBe(false);
	});
	it("retains per-seat API and pane errors without hiding other seats or replacing the outcome", async () => {
		const { io, sensor } = setup();
		io.seats = async () => [seat, { ...seat, id: "second", pane: "%3" }];
		io.get = async (path) => {
			if (path.includes("seat=current-seat")) throw new Error("HTTP 503 offline");
			return sensor;
		};
		const tmux = io.tmux;
		io.tmux = (args, raw) => {
			if (args[0] === "display-message" && args.at(-1) === "%2") throw new Error("pane exited");
			return tmux(args, raw);
		};
		const receipt: Record<string, unknown> = { status: "passed" };
		expect(await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io))).toBe(true);
		expect(object(receipt.final_typing_capture).observations).toMatchObject([
			{
				seat,
				error: "PIJ_NATIVE_FINAL_TYPING: Error: HTTP 503 offline",
				pane_capture: { error: "Error: pane exited" },
			},
			{ seat: { id: "second" }, sensor },
		]);
		expect(receipt.status).toBe("passed");
		expect(receipt.error).toBeUndefined();
	});
	it.each([
		"daemon unavailable",
		"ENOENT daemon.key",
	])("retains %s and still cleans up with original failure", async (reason) => {
		const { io, calls } = setup();
		io.seats = async () => {
			throw new Error(reason);
		};
		const receipt: Record<string, unknown> = {
			status: "failed",
			error: "original startup outcome",
		};
		expect(await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io))).toBe(true);
		expect(object(receipt.final_typing_capture).error).toBe(
			`PIJ_NATIVE_FINAL_TYPING: Error: ${reason}`,
		);
		expect(receipt.error).toBe("original startup outcome");
		expect(calls.at(-1)).toBe("tmux:kill-server");
	});
	it("bounds a stuck sensor request with the shared abort signal before teardown", async () => {
		const { io, calls } = setup();
		let rosterSignal: AbortSignal | undefined;
		io.seats = async (signal) => {
			rosterSignal = signal;
			return [seat];
		};
		io.get = async (_path, signal) => {
			expect(signal).toBe(rosterSignal);
			return new Promise((_resolve, reject) =>
				signal.addEventListener("abort", () => reject(signal.reason), { once: true }),
			);
		};
		const receipt: Record<string, unknown> = { status: "failed", error: "original" };
		expect(await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io, 10))).toBe(
			true,
		);
		expect(object(receipt.final_typing_capture).observations).toMatchObject([
			{ error: expect.stringContaining("TimeoutError"), pane_capture: { cursor_x: 2 } },
		]);
		expect(calls.at(-1)).toBe("tmux:kill-server");
		expect(receipt.error).toBe("original");
	});
	it("skips the optional API callback before daemon initialization and still cleans up", async () => {
		const { io, calls } = setup();
		const receipt: Record<string, unknown> = {
			status: "failed",
			error: "ordinary startup timeout",
		};
		expect(await finishSmokePanes(io.tmux, receipt)).toBe(true);
		expect(receipt.final_typing_capture_skipped).toBe("daemon-api-roster-not-initialized");
		expect(calls.some((call) => call.startsWith("GET:"))).toBe(false);
		expect(calls.at(-1)).toBe("tmux:kill-server");
		expect(receipt.error).toBe("ordinary startup timeout");
	});
	it("retains unexpected diagnostic callback failure without blocking teardown", async () => {
		const { io, calls } = setup();
		const receipt: Record<string, unknown> = { status: "failed", error: "original" };
		expect(
			await finishSmokePanes(io.tmux, receipt, async () => {
				throw new Error("diagnostic failed");
			}),
		).toBe(true);
		expect(receipt.final_typing_capture_error).toBe(
			"PIJ_NATIVE_FINAL_TYPING: Error: diagnostic failed",
		);
		expect(calls.at(-1)).toBe("tmux:kill-server");
		expect(receipt.error).toBe("original");
	});
	it("retains terminal capture failure and still performs cleanup", async () => {
		const { io, calls } = setup();
		const tmux = io.tmux;
		io.tmux = (args, raw) => {
			if (args[0] === "list-panes") throw new Error("capture failed");
			return tmux(args, raw);
		};
		const receipt: Record<string, unknown> = { status: "passed" };
		expect(await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io))).toBe(true);
		expect(receipt.final_capture_error).toBe("Error: capture failed");
		expect(calls.at(-1)).toBe("tmux:kill-server");
		expect(receipt.status).toBe("passed");
	});
	it("keeps cleanup failures distinct from diagnostic failures", async () => {
		const { io } = setup();
		const tmux = io.tmux;
		io.tmux = (args, raw) => {
			if (args[0] === "kill-server") throw new Error("cleanup failed");
			return tmux(args, raw);
		};
		const receipt: Record<string, unknown> = { status: "failed", error: "original" };
		expect(await finishSmokePanes(io.tmux, receipt, () => captureSmokeFinalTyping(io))).toBe(false);
		expect(receipt.tmux_cleanup_error).toBe("Error: cleanup failed");
		expect(receipt.error).toBe("original");
	});
});

describe("native witness boundaries", () => {
	it("captures all final panes before cleanup and preserves individual capture errors", () => {
		const commands: string[][] = [];
		const result = captureSmokePanes((args) => {
			commands.push(args);
			if (args[0] === "list-panes") return "%1 101 node\n%2 102 node\n%3 103 node";
			if (args.at(-1) === "%2") throw new Error("pane exited");
			if (args[0] === "display-message") return "7\t2\t80\t24\nactual viewport\n";
			return args.at(-1) === "%1" ? "Confirm folder trust" : "[pij native] unavailable: offline";
		});
		expect(result.panes).toBe("%1 101 node\n%2 102 node\n%3 103 node");
		expect(result.captures).toMatchObject([
			{ pane: "%1", terminal: "Confirm folder trust" },
			{
				pane: "%2",
				error: "Error: pane exited",
				pane_capture: {
					pane: "%2",
					error: "Error: pane exited",
					captured_at_ms: expect.any(Number),
				},
			},
			{ pane: "%3", terminal: "[pij native] unavailable: offline" },
		]);
		expect(
			commands.filter((args) => args[0] === "capture-pane").every((args) => args.includes("-2000")),
		).toBe(true);
	});
	it("retains paired viewport when joined scrollback fails", () => {
		const result = captureSmokePanes((args) => {
			if (args[0] === "list-panes") return "%1 101 node";
			if (args[0] === "capture-pane") throw new Error("scrollback failed");
			return "7\t2\t80\t24\nsurviving viewport\n";
		});
		expect(result.captures[0]).toMatchObject({
			pane: "%1",
			error: "Error: scrollback failed",
			pane_capture: { pane: "%1", cursor_x: 7, cursor_y: 2, terminal: "surviving viewport\n" },
		});
	});
	it("pairs actual cursor coordinates with unjoined final viewport, preserving scrollback", () => {
		const commands: string[][] = [];
		const viewport = "\nwrapped row one\nwrapped row two\n@ files · # issues\n\n\n";
		const result = captureSmokePanes((args, preserveOutput) => {
			commands.push(args);
			if (args[0] === "list-panes") return "%1 101 node";
			if (args[0] === "display-message") {
				const output = `0\t2\t80\t24\n${viewport}`;
				return preserveOutput ? output : output.trim();
			}
			return "full joined scrollback";
		});
		expect(result.captures[0]).toMatchObject({
			pane: "%1",
			terminal: "full joined scrollback",
			pane_capture: {
				pane: "%1",
				cursor_x: 0,
				cursor_y: 2,
				pane_width: 80,
				pane_height: 24,
				terminal: viewport,
			},
		});
		expect(commands.find((args) => args[0] === "display-message")).toEqual([
			"display-message",
			"-p",
			"-t",
			"%1",
			"#{cursor_x}\t#{cursor_y}\t#{pane_width}\t#{pane_height}",
			";",
			"capture-pane",
			"-p",
			"-t",
			"%1",
		]);
	});
	it.each([
		"",
		"-1\t2\t80\t24",
		"unknown\t2\t80\t24",
	])("retains capture failure instead of inventing cursor geometry: %j", (metadata) => {
		const result = captureSmokePanes((args) => {
			if (args[0] === "list-panes") return "%1 101 node";
			return args[0] === "display-message"
				? `${metadata}\nactual viewport\n`
				: "existing scrollback";
		});
		expect(result.captures[0]).toMatchObject({
			terminal: "existing scrollback",
			pane_capture: {
				terminal: "actual viewport\n",
				metadata_raw: metadata,
				error: expect.any(String),
			},
		});
		expect(object(object(result.captures[0]).pane_capture).cursor_x).toBeUndefined();
	});

	it("requires ordinary native usability and exactly one actionable outage diagnostic", () => {
		const user = {
			id: "user-event",
			type: "user.message",
			data: { content: "ordinary-nonce", messageId: "user-native", interactionId: "turn-a" },
		};
		const assistant = {
			id: "assistant-event",
			type: "assistant.message",
			data: {
				content: "PIJ_NATIVE_OBSERVED",
				messageId: "assistant-native",
				interactionId: "turn-a",
			},
		};
		const diagnostic = "[pij native] unavailable: start the daemon";
		expect(() =>
			verifyOrdinaryUsability([user, assistant], "ordinary-nonce", diagnostic),
		).not.toThrow();
		expect(() => verifyOrdinaryUsability([user], "ordinary-nonce", diagnostic)).toThrow(
			"same native interaction",
		);
		expect(() =>
			verifyOrdinaryUsability(
				[user, { ...assistant, data: { ...assistant.data, interactionId: "other-turn" } }],
				"ordinary-nonce",
				diagnostic,
			),
		).toThrow("same native interaction");
		expect(() => verifyOrdinaryUsability([user, assistant], "ordinary-nonce", "")).toThrow(
			"one diagnostic",
		);
		expect(() =>
			verifyOrdinaryUsability([user, assistant], "ordinary-nonce", `${diagnostic}\n${diagnostic}`),
		).toThrow("one diagnostic");
		const positive = {
			run: "/fresh-seeded",
			isolation: {
				home: "/fresh-seeded/home",
				workspace: "/fresh-seeded/workspace",
				trust_config_before: '{"trustedFolders":["/fresh-seeded/workspace"]}',
			},
			witnesses: [
				{
					kind: "daemon-key-absent-ordinary-use",
					events: [user, assistant],
					nonce: "ordinary-nonce",
					terminal: diagnostic,
					daemon_started: false,
					key_present: false,
				},
			],
		};
		const negative = {
			run: "/fresh-prompt",
			isolation: {
				home: "/fresh-prompt/home",
				workspace: "/fresh-prompt/workspace",
				trust_config_before: null,
			},
			witnesses: [],
			fixture_requests: [],
			final_pane_captures: [
				{ pane: "%1", terminal: "Confirm folder trust\n/fresh-prompt/workspace" },
			],
		};
		expect(() => verifyWorkspaceTrustDiscriminator(positive, negative)).not.toThrow();
		expect(() =>
			verifyWorkspaceTrustDiscriminator(positive, { ...negative, isolation: positive.isolation }),
		).toThrow("distinct private HOMEs");
		expect(() =>
			verifyWorkspaceTrustDiscriminator(positive, {
				...negative,
				fixture_requests: ["POST /v1/chat/completions"],
			}),
		).toThrow("no provider activity");
		expect(() =>
			verifyWorkspaceTrustDiscriminator(positive, {
				...negative,
				final_pane_captures: [{ terminal: "Confirm folder trust\n/other-workspace" }],
			}),
		).toThrow("exact isolated workspace");
		expect(() =>
			verifyWorkspaceTrustDiscriminator({ ...positive, witnesses: [] }, negative),
		).toThrow("ordinary-use witness");
		expect(() =>
			verifyWorkspaceTrustDiscriminator(
				{ ...positive, isolation: { ...positive.isolation, trust_config_before: null } },
				negative,
			),
		).toThrow("seeded native config");
		expect(() =>
			verifyWorkspaceTrustDiscriminator(
				{
					...positive,
					isolation: { ...positive.isolation, trust_config_before: '{"trustedFolders":["/"]}' },
				},
				negative,
			),
		).toThrow("only the isolated workspace");
		expect(() =>
			verifyWorkspaceTrustDiscriminator(positive, {
				...negative,
				isolation: { ...negative.isolation, trust_config_before: "{}" },
			}),
		).toThrow("without native config");
		expect(() =>
			verifyWorkspaceTrustDiscriminator(positive, {
				...negative,
				final_pane_captures: [{ terminal: "Confirm folder trust\n/fresh-prompt/workspace-other" }],
			}),
		).toThrow("exact isolated workspace");
	});
	it("manual launch drops identity and SDK context, only real mode inherits auth in memory", () => {
		const inherited = {
			PATH: "/bin",
			PIJ_SESSION_ID: "real-seat",
			PIJ_SPAWN_ID: "real-spawn",
			TMUX: "live-socket",
			COPILOT_AGENT_SESSION_ID: "live-native",
			COPILOT_PROVIDER_BASE_URL: "http://foreign",
			COPILOT_PROVIDER_API_KEY: "foreign-secret",
			GH_TOKEN: "real-secret",
			NODE_OPTIONS: "--import=evil.js",
		};
		const local = smokeEnvironment(
			inherited,
			"/fixture/home",
			"/fixture/config",
			"/fixture/state",
			"127.0.0.1:1234",
			"local",
		);
		expect(local).toMatchObject({
			HOME: "/fixture/home",
			COPILOT_HOME: "/fixture/config",
			PIJ_RS_ADDR: "127.0.0.1:1234",
			PIJ_RS_STATE_DIR: "/fixture/state",
			COPILOT_OFFLINE: "true",
		});
		for (const key of [
			"PIJ_SESSION_ID",
			"PIJ_SPAWN_ID",
			"TMUX",
			"COPILOT_AGENT_SESSION_ID",
			"COPILOT_PROVIDER_BASE_URL",
			"COPILOT_PROVIDER_API_KEY",
			"GH_TOKEN",
			"NODE_OPTIONS",
		])
			expect(local[key]).toBeUndefined();
		const real = smokeEnvironment(inherited, "/h", "/c", "/s", "127.0.0.1:1234", "real");
		expect(real.GH_TOKEN).toBe("real-secret");
		expect(real.COPILOT_OFFLINE).toBe("false");
	});

	it("counts only actual native user messages, retains IDs, ignores partial final writes", () => {
		const path = join(temporary(), "events.jsonl");
		writeFileSync(
			path,
			[
				JSON.stringify({
					id: "assistant-id",
					type: "assistant.message",
					data: { content: "nonce" },
				}),
				JSON.stringify({
					id: "user-id",
					type: "user.message",
					data: { content: "[pij from peer]\nnonce" },
				}),
				'{"id":"partial',
			].join("\n"),
		);
		expect(nativeMessages(readNativeEvents(path), "nonce")).toEqual([
			{ id: "user-id", type: "user.message", data: { content: "[pij from peer]\nnonce" } },
		]);
		writeFileSync(path, '{"type":"user.message","data":{"content":"nonce"}}\n');
		expect(() => readNativeEvents(path)).toThrow("native event must have its real event id");
	});

	it("rejects ambiguous CLI inputs and never upgrades a missing binary to success", async () => {
		expect(() => parseSmokeArgs(["--provider", "fake"])).toThrow();
		expect(() => parseSmokeArgs(["--mode"])).toThrow();
		expect(() => parseSmokeArgs(["--timeout-seconds", "NaN"])).toThrow();
		expect(parseSmokeArgs([]).workspaceTrust).toBe("seeded");
		expect(parseSmokeArgs(["--workspace-trust", "prompt"]).workspaceTrust).toBe("prompt");
		expect(() => parseSmokeArgs(["--workspace-trust", "all"])).toThrow();
		const root = temporary();
		const output = join(root, "receipt.json");
		const options = parseSmokeArgs(["--pij-bin", join(root, "missing"), "--output", output]);
		const result = await runSmoke(options);
		expect(result.code).toBe(2);
		const receipt = JSON.parse(readFileSync(output, "utf8"));
		expect(receipt).toMatchObject({
			status: "prerequisite-missing",
			provider: "deterministic-local-fixture-not-real-inference",
			witnesses: [],
		});
		expect(receipt.error).toContain("PIJ_NATIVE_PREREQUISITE_BINARY");
		roots.push(receipt.run);
		await expect(runSmoke(options)).rejects.toThrow("refusing to overwrite receipt");
	});
});
