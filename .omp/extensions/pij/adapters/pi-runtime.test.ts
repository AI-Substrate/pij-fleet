import { EventEmitter } from "node:events";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { describe, expect, it, vi } from "vitest";
import { frame } from "../core/message.js";
import {
	PiRuntimeAdapter,
	publishReloadCompleted,
	registerOmpReloadCompletion,
	reloadWithCompletion,
} from "./pi-runtime.js";

describe("draft-safe extension submission", () => {
	it.each([
		"immediate",
		"steer",
	] as const)("delivers %s without touching the composer", async (mode) => {
		const draft = "Unsubmitted human draft\nwith Unicode 你好";
		const incoming = frame("pij-sender", "Incoming pij message");
		let editorText = draft;
		const editorWrites: string[] = [];
		const modelMessages: string[] = [];
		const visibleMessages: string[] = [];
		const pending: Array<() => void> = [];
		const ui = {
			getEditorText: () => editorText,
			setEditorText: (text: string) => {
				editorWrites.push(text);
				editorText = text;
			},
			setStatus: () => {},
			notify: () => {},
		};
		const api = {
			// OMP cli.js:19099 message_start clears the editor for an external,
			// non-synthetic user message without a local-submit signature.
			sendUserMessage: (content: Parameters<ExtensionAPI["sendUserMessage"]>[0]) => {
				pending.push(() => {
					ui.setEditorText("");
					modelMessages.push(String(content));
					visibleMessages.push(String(content));
				});
			},
			// OMP cli.js:17381 uses custom messages directly: steer when busy,
			// trigger a prompt when idle. message_start renders without clearing.
			sendMessage: (
				message: Parameters<ExtensionAPI["sendMessage"]>[0],
				options?: Parameters<ExtensionAPI["sendMessage"]>[1],
			) => {
				// OMP also restores user-attributed custom messages into the editor on Escape.
				expect(message).not.toHaveProperty("attribution", "user");
				pending.push(() => {
					if (mode === "steer" || options?.triggerTurn) modelMessages.push(String(message.content));
					if (message.display) visibleMessages.push(String(message.content));
				});
			},
		};
		const adapter = new PiRuntimeAdapter(
			api,
			{
				isIdle: () => mode === "immediate",
				compact: () => {},
				ui,
			},
			"omp",
		);
		adapter.inject(incoming, mode);
		// Delivery occurs after the extension returns, not synchronously inside
		// sendUserMessage. Restoring a snapshot cannot defend this boundary.
		await Promise.resolve();
		for (const deliver of pending) deliver();
		expect(editorText).toBe(draft);
		expect(editorWrites).toEqual([]);
		expect(modelMessages).toEqual([incoming]);
		expect(visibleMessages).toEqual([incoming]);
	});

	it("starts an OMP turn if the runtime becomes idle before queued delivery", () => {
		const calls: Array<Parameters<ExtensionAPI["sendMessage"]>> = [];
		const adapter = new PiRuntimeAdapter(
			{
				sendUserMessage: () => {
					throw new Error("OMP must not use the user-message path");
				},
				sendMessage: (message, options) => {
					calls.push([message, options]);
				},
			},
			{
				isIdle: () => true,
				compact: () => {},
				ui: { setStatus: () => {}, notify: () => {} },
			},
			"omp",
		);
		adapter.inject("arrived while busy", "steer");
		expect(calls).toEqual([
			[
				{ customType: "pij", content: "arrived while busy", display: true },
				{ triggerTurn: true, deliverAs: "steer" },
			],
		]);
	});

	it.each([
		"pi",
		"omp",
	] as const)("carries %s peer consumption identity outside the human draft", (runtimeBin) => {
		const sendMessage = vi.fn();
		const sendUserMessage = vi.fn();
		const adapter = new PiRuntimeAdapter(
			{ sendMessage, sendUserMessage },
			{ isIdle: () => false, compact: () => {}, ui: { setStatus: () => {}, notify: () => {} } },
			runtimeBin,
		);
		adapter.inject(frame("pij-sender", "same body"), "steer", "durable-message-id");
		expect(sendMessage).toHaveBeenCalledWith(
			{
				customType: "pij",
				content: frame("pij-sender", "same body"),
				display: true,
				details: { pijMessageId: "durable-message-id" },
			},
			{ triggerTurn: true, deliverAs: "steer" },
		);
		expect(sendUserMessage).not.toHaveBeenCalled();
	});

	it("uses a recognizable prompt with encoded identity when a custom delivery needs recovery", () => {
		const sendMessage = vi.fn();
		const sendUserMessage = vi.fn();
		const adapter = new PiRuntimeAdapter(
			{ sendMessage, sendUserMessage },
			{ isIdle: () => true, compact: () => {}, ui: { setStatus: () => {}, notify: () => {} } },
			"omp",
		);
		adapter.inject(frame("pij-sender", "body\npreserved"), "immediate", "id]\n你好", 2);
		expect(sendMessage).not.toHaveBeenCalled();
		expect(sendUserMessage).toHaveBeenCalledExactlyOnceWith(
			`[pij resend 2]\n[pijMessageId:${encodeURIComponent("id]\n你好")}]\n${frame("pij-sender", "body\npreserved")}`,
		);
	});

	it.each([
		"immediate",
		"steer",
	] as const)("keeps Pi's %s user-message API for boot announcements", (mode) => {
		const calls: Array<Parameters<ExtensionAPI["sendUserMessage"]>> = [];
		const adapter = new PiRuntimeAdapter(
			{
				sendUserMessage: (...args) => {
					calls.push(args);
				},
				sendMessage: () => {
					throw new Error("Pi must retain user-message semantics");
				},
			},
			{
				isIdle: () => mode === "immediate",
				compact: () => {},
				ui: { setStatus: () => {}, notify: () => {} },
			},
			"pi",
		);
		// Pi interactive-mode.js:2314-2317 renders user message_start without an
		// editor mutation, unlike OMP. Keep the existing public submission path.
		adapter.inject("Incoming pij message", mode);
		expect(calls).toEqual(
			mode === "steer"
				? [["Incoming pij message", { deliverAs: "steer" }]]
				: [["Incoming pij message"]],
		);
	});
});

describe("native control completion", () => {
	it("propagates OMP compact promise rejection even without an error callback", async () => {
		const adapter = new PiRuntimeAdapter(
			{ sendUserMessage: () => {}, sendMessage: () => {} },
			{
				isIdle: () => true,
				compact: async () => {
					throw new Error("native compact rejected");
				},
				ui: { notify: () => {}, setStatus: () => {} },
			},
			"omp",
		);
		await expect(adapter.compact()).rejects.toThrow("native compact rejected");
	});
	it("refuses busy reload before calling a runtime that would silently ignore it", async () => {
		const reload = vi.fn(async () => {});
		const adapter = new PiRuntimeAdapter(
			{ sendUserMessage: () => {}, sendMessage: () => {} },
			{ isIdle: () => false, compact: () => {}, ui: { notify: () => {}, setStatus: () => {} } },
			"omp",
			() => ({ newSession: async () => ({ cancelled: false }), reload }),
		);
		await expect(adapter.control("reload")).rejects.toThrow(/busy/i);
		expect(reload).not.toHaveBeenCalled();
	});
	it("resolves compact only after the runtime completion callback", async () => {
		let options: Parameters<ExtensionContext["compact"]>[0];
		const adapter = new PiRuntimeAdapter(
			{ sendUserMessage: () => {}, sendMessage: () => {} },
			{
				isIdle: () => true,
				compact: (value) => {
					options = value;
				},
				ui: { notify: () => {}, setStatus: () => {} },
			},
			"omp",
		);
		let finished = false;
		const completion = Promise.resolve(adapter.compact()).then(() => {
			finished = true;
		});
		await Promise.resolve();
		expect(finished).toBe(false);
		options?.onComplete?.({} as never);
		await completion;
		expect(finished).toBe(true);
	});

	it("propagates compact failure instead of claiming execution", async () => {
		const adapter = new PiRuntimeAdapter(
			{ sendUserMessage: () => {}, sendMessage: () => {} },
			{
				isIdle: () => true,
				compact: (options) => options?.onError?.(new Error("runtime failed")),
				ui: { notify: () => {}, setStatus: () => {} },
			},
			"omp",
		);
		await expect(adapter.compact()).rejects.toThrow("runtime failed");
	});

	it("treats a cancelled new session as refused", async () => {
		const adapter = new PiRuntimeAdapter(
			{ sendUserMessage: () => {}, sendMessage: () => {} },
			{ isIdle: () => true, compact: () => {}, ui: { notify: () => {}, setStatus: () => {} } },
			"omp",
			() => ({ newSession: async () => ({ cancelled: true }), reload: async () => {} }),
		);
		await expect(adapter.control("new")).rejects.toThrow(/cancel/i);
	});
});

describe("reload lifecycle outcomes", () => {
	function lifecycleApi() {
		const bus = new EventEmitter();
		const lifecycle = new EventEmitter();
		const api = {
			events: {
				emit: (channel: string, data: unknown) => {
					bus.emit(channel, data);
				},
				on: (channel: string, handler: (data: unknown) => void) => {
					bus.on(channel, handler);
					return () => {
						bus.off(channel, handler);
					};
				},
			},
			on: (event: string, handler: (value: unknown) => void) => {
				lifecycle.on(event, handler);
			},
		} as unknown as Pick<ExtensionAPI, "events" | "on">;
		return { api, lifecycle };
	}

	it.each([
		"pi",
		"omp",
	] as const)("refuses %s reload returning normally without a lifecycle event", async (runtime) => {
		const { api } = lifecycleApi();
		if (runtime === "omp") registerOmpReloadCompletion(api);
		await expect(reloadWithCompletion(api, async () => {})).rejects.toThrow(/did not complete/i);
	});

	it.each([
		["pi", "session_start", "reload"],
		["omp", "session_switch", "resume"],
	] as const)("accepts %s only after its awaited native lifecycle event", async (runtime, event, reason) => {
		const { api, lifecycle } = lifecycleApi();
		if (runtime === "omp") registerOmpReloadCompletion(api);
		else
			api.on("session_start", (started) => {
				if (started.reason === "reload") publishReloadCompleted(api);
			});
		await expect(
			reloadWithCompletion(api, async () => {
				lifecycle.emit(event, { reason });
			}),
		).resolves.toBeUndefined();
		// A prior event must not make the next silent no-op look successful.
		await expect(reloadWithCompletion(api, async () => {})).rejects.toThrow(/did not complete/i);
	});
});
