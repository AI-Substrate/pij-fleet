import { describe, expect, it } from "vitest";

import {
	planCurrentSessionDescriptor,
	resolveAmbientNativeIdentity,
	resolveRegisteredAmbientSelf,
} from "./current-session.js";
import type { SessionDescriptor } from "./types.js";

const COPILOT_UUID = "9A8F8BE6-3670-4E5C-B43E-09F46FE4DFAD";
const CODEX_UUID = "ABCDEF01-2222-4333-8444-555555555555";
const CODEX_PATH =
	"/Users/jo/.codex/sessions/2026/07/12/rollout-2026-07-12T00-00-00-abcdef01-2222-4333-8444-555555555555.jsonl";

describe("resolveAmbientNativeIdentity", () => {
	it("resolves a non-empty Claude current-session id", () => {
		expect(resolveAmbientNativeIdentity({ claudeCodeSessionId: " claude-current " })).toEqual({
			ok: true,
			value: {
				harness: "claude",
				harnessSessionId: "claude-current",
			},
		});
	});

	it("uses the validated Copilot current-session UUID supplied by the bin", () => {
		expect(resolveAmbientNativeIdentity({ copilotCurrentSessionId: COPILOT_UUID })).toEqual({
			ok: true,
			value: {
				harness: "copilot",
				harnessSessionId: COPILOT_UUID.toLowerCase(),
			},
		});
	});

	it("uses the canonical Codex thread id with its exact rollout path", () => {
		expect(
			resolveAmbientNativeIdentity({
				codexCurrentSession: {
					threadId: CODEX_UUID,
					transcriptPath: CODEX_PATH,
				},
			}),
		).toEqual({
			ok: true,
			value: {
				harness: "codex",
				harnessSessionId: CODEX_UUID.toLowerCase(),
				transcriptPath: CODEX_PATH,
			},
		});
	});

	it("returns null when no ambient native identity is present", () => {
		expect(resolveAmbientNativeIdentity({})).toEqual({ ok: true, value: null });
	});

	it("fails loudly when more than one harness identity is present", () => {
		const result = resolveAmbientNativeIdentity({
			claudeCodeSessionId: "claude-current",
			copilotCurrentSessionId: COPILOT_UUID,
		});
		expect(result).toMatchObject({ ok: false, code: "E-AMBIG" });
		if (!result.ok) {
			expect(result.message).toContain("claude");
			expect(result.message).toContain("copilot");
		}
	});
});

describe("planCurrentSessionDescriptor", () => {
	const existing: SessionDescriptor = {
		id: "pij-durable",
		folder: "/old",
		dataDir: "/home/.pij/pij-durable",
		eventsPath: "/home/.pij/pij-durable/events.ndjson",
		pid: 10,
		startedAt: "2026-07-01T00:00:00.000Z",
		state: "working",
		harness: "codex",
		harnessSessionId: CODEX_UUID.toLowerCase(),
		transcriptPath: "/old/rollout.jsonl",
		paneId: "%7",
		lifecycle: "bound",
		prime: true,
		failureReason: "dead",
		lastEventAt: "2026-07-11T00:00:00.000Z",
		lastTickAt: "2026-07-12T00:00:00.000Z",
		spawnedBy: "pij-parent",
		plannedHarnessSessionId: CODEX_UUID.toLowerCase(),
		initInjectedAt: "2026-07-01T00:00:01.000Z",
		transcriptsAtSpawn: ["/old/before.jsonl"],
		boundModel: "openai/gpt-5.6-sol",
		effort: "xhigh",
		branchedFrom: "codex-source",
		agentPack: "flowspace-search",
		agentPackDir: "/home/.pij/pij-durable/pack",
		agentOnce: true,
		reportedAt: "2026-07-11T01:00:00.000Z",
	};

	it("repairs a pane-bound descriptor to external pull while preserving durable history", () => {
		const planned = planCurrentSessionDescriptor({
			id: existing.id,
			identity: {
				harness: "codex",
				harnessSessionId: CODEX_UUID.toLowerCase(),
				transcriptPath: CODEX_PATH,
			},
			pijHome: "/home/.pij",
			folder: "/repo",
			pid: 99,
			startedAt: "2026-07-12T00:00:00.000Z",
			existing,
		});
		expect(planned).toMatchObject({
			id: "pij-durable",
			folder: "/repo",
			dataDir: existing.dataDir,
			eventsPath: existing.eventsPath,
			pid: 99,
			startedAt: existing.startedAt,
			state: "idle",
			harness: "codex",
			harnessSessionId: CODEX_UUID.toLowerCase(),
			transcriptPath: CODEX_PATH,
			lifecycle: "bound",
			deliveryMode: "pull",
			prime: true,
			lastEventAt: existing.lastEventAt,
			spawnedBy: existing.spawnedBy,
			boundModel: existing.boundModel,
			effort: existing.effort,
			branchedFrom: existing.branchedFrom,
			agentPack: existing.agentPack,
			agentPackDir: existing.agentPackDir,
			reportedAt: existing.reportedAt,
		});
		expect(planned.paneId).toBeUndefined();
		expect(planned.lastTickAt).toBeUndefined();
		expect(planned.failureReason).toBeUndefined();
		expect(planned.plannedHarnessSessionId).toBeUndefined();
		expect(planned.initInjectedAt).toBeUndefined();
		expect(planned.transcriptsAtSpawn).toBeUndefined();
		expect(planned.agentOnce).toBeUndefined();
		expect(planned.reportedAt).toBe(existing.reportedAt);
	});

	it("is idempotent when planning an already repaired external descriptor", () => {
		const input = {
			id: existing.id,
			identity: {
				harness: "codex",
				harnessSessionId: CODEX_UUID.toLowerCase(),
				transcriptPath: CODEX_PATH,
			},
			pijHome: "/home/.pij",
			folder: "/repo",
			pid: 99,
			startedAt: "2026-07-12T00:00:00.000Z",
			existing,
		} as const;
		const first = planCurrentSessionDescriptor(input);
		const repeat = planCurrentSessionDescriptor({ ...input, existing: first });
		expect(repeat).toEqual(first);
	});
});

describe("resolveRegisteredAmbientSelf", () => {
	const identity = {
		harness: "claude",
		harnessSessionId: "claude-current",
	} as const;
	const descriptor = (
		id: string,
		overrides: Partial<SessionDescriptor> = {},
	): SessionDescriptor => ({
		id,
		folder: "/repo",
		dataDir: `/home/.pij/${id}`,
		eventsPath: `/home/.pij/${id}/events.ndjson`,
		pid: 10,
		startedAt: "2026-07-12T00:00:00.000Z",
		harness: identity.harness,
		harnessSessionId: identity.harnessSessionId,
		deliveryMode: "pull",
		lifecycle: "bound",
		...overrides,
	});

	it("accepts a paneless pull descriptor outside tmux", () => {
		expect(
			resolveRegisteredAmbientSelf(identity, [descriptor("pij-exact")], "pij-exact", undefined),
		).toEqual({ ok: true, value: "pij-exact" });
	});

	it("accepts only the exact current pane when the descriptor is not pull-owned", () => {
		expect(
			resolveRegisteredAmbientSelf(
				identity,
				[descriptor("pij-exact", { paneId: "%7", deliveryMode: undefined })],
				"pij-exact",
				"%7",
			),
		).toEqual({ ok: true, value: "pij-exact" });
	});

	it("fails loudly for duplicate or contradictory exact joins", () => {
		expect(
			resolveRegisteredAmbientSelf(
				identity,
				[descriptor("pij-first"), descriptor("pij-second")],
				"pij-first",
				undefined,
			),
		).toMatchObject({ ok: false, code: "E-AMBIG" });
		const contradictory = resolveRegisteredAmbientSelf(
			identity,
			[descriptor("pij-live")],
			"pij-durable",
			undefined,
		);
		expect(contradictory).toMatchObject({
			ok: false,
			code: "E-AMBIG",
		});
		if (!contradictory.ok) expect(contradictory.message).toContain("pij inbox register");
	});

	it("returns E-NOID when the ambient native tuple has no durable registration", () => {
		const result = resolveRegisteredAmbientSelf(identity, [], undefined, undefined);
		expect(result).toMatchObject({ ok: false, code: "E-NOID" });
		if (!result.ok) expect(result.message).toContain("pij inbox register");
	});

	it("rejects stale push attachment outside tmux and mismatched attachment inside tmux", () => {
		const stale = descriptor("pij-stale", { paneId: "%0", deliveryMode: undefined });
		const external = resolveRegisteredAmbientSelf(identity, [stale], stale.id, undefined);
		expect(external).toMatchObject({ ok: false, code: "E-NOID" });
		if (!external.ok) expect(external.message).toContain("pij inbox register");

		const tmux = resolveRegisteredAmbientSelf(identity, [stale], stale.id, "%7");
		expect(tmux).toMatchObject({ ok: false, code: "E-NOID" });
		if (!tmux.ok) {
			expect(tmux.message).toContain('pij adopt "$TMUX_PANE"');
			expect(tmux.message).toContain("%7");
		}
	});

	it("rejects missing or non-pull external descriptors", () => {
		const missing = resolveRegisteredAmbientSelf(identity, [], "pij-missing", undefined);
		expect(missing).toMatchObject({ ok: false, code: "E-NOID" });
		if (!missing.ok) expect(missing.message).toContain("pij inbox register");

		const nonPull = resolveRegisteredAmbientSelf(
			identity,
			[descriptor("pij-push", { deliveryMode: "push" })],
			"pij-push",
			undefined,
		);
		expect(nonPull).toMatchObject({ ok: false, code: "E-NOID" });
	});
});
