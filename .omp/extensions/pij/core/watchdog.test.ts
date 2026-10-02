import { describe, expect, it } from "vitest";
import { SEMANTIC_STATES, type WatchdogSidecar } from "./types.js";
import {
	applyCompactPause,
	applyNewWorkTransition,
	applyWatchdogExemption,
	applyWatchdogResume,
	applyWorkingTransition,
	buildWatchdogTurn,
	captureSlice,
	describeWatchdogState,
	effectiveWatchdog,
	evaluateResponse,
	humanizeDurationMs,
	isFireDue,
	mutesWatchdogNudge,
	parseWatchdogInterval,
	reconcileWatchdogExemption,
	renderWatcherRoster,
	shouldCapture,
	watchdogScheduleAnchorMs,
} from "./watchdog.js";

describe("effectiveWatchdog and pause tiers", () => {
	it("is default-on at a twenty-minute interval when the sidecar is absent", () => {
		expect(effectiveWatchdog()).toEqual({
			enabled: true,
			intervalMs: 1_200_000,
			pausedBy: undefined,
		});
	});

	it("respects enabled and interval overrides", () => {
		expect(effectiveWatchdog({ enabled: false, intervalMs: 30_000 })).toEqual({
			enabled: false,
			intervalMs: 30_000,
			pausedBy: undefined,
		});
	});

	it("keeps a self pause through working transitions and resumes it only by verb", () => {
		const sidecar: WatchdogSidecar = { pausedBy: "self", pausedAtMs: 10 };
		expect(applyWorkingTransition(sidecar)).toBe(sidecar);
		expect(applyWatchdogResume(sidecar)).toEqual({});
	});

	it("re-arms only a self pause when new work is assigned", () => {
		expect(
			applyNewWorkTransition({
				enabled: false,
				intervalMs: 60_000,
				pausedBy: "self",
				pausedAtMs: 10,
			}),
		).toEqual({ enabled: false, intervalMs: 60_000 });

		for (const sidecar of [
			{ enabled: false },
			{ pausedBy: "compact" as const, pausedAtMs: 10 },
			{ pausedBy: "exempt" as const, pausedAtMs: 10, exemptUntilMs: 100 },
		]) {
			expect(applyNewWorkTransition(sidecar)).toBe(sidecar);
		}
	});

	it("auto-resumes a compact pause on the next observed working transition", () => {
		const sidecar: WatchdogSidecar = {
			enabled: true,
			intervalMs: 60_000,
			pausedBy: "compact",
			pausedAtMs: 10,
		};
		expect(applyWorkingTransition(sidecar)).toEqual({ enabled: true, intervalMs: 60_000 });
	});

	it("never resumes an exemption through either ordinary transition", () => {
		const sidecar: WatchdogSidecar = { pausedBy: "exempt", pausedAtMs: 10 };
		expect(applyWorkingTransition(sidecar)).toBe(sidecar);
		expect(applyWatchdogResume(sidecar)).toBe(sidecar);
	});
});

describe("bounded watchdog exemptions", () => {
	it("pins the exact expiry boundary and clears an expired exemption", () => {
		const exempt = applyWatchdogExemption({ intervalMs: 1 }, 1_000, 100);
		expect(exempt).toMatchObject({
			pausedBy: "exempt",
			pausedAtMs: 1_000,
			exemptUntilMs: 1_100,
		});
		for (const [nowMs, paused] of [
			[1_099, "exempt"],
			[1_100, undefined],
			[1_101, undefined],
		] as const) {
			const reconciled = reconcileWatchdogExemption(exempt, nowMs);
			expect(reconciled.effectivePause).toBe(paused);
			expect(reconciled.sidecar?.pausedBy).toBe(paused);
		}
	});

	it("migrates a valid legacy exemption once, and fails closed for missing or invalid time", () => {
		const legacy: WatchdogSidecar = { pausedBy: "exempt", pausedAtMs: 10 };
		expect(reconcileWatchdogExemption(legacy, 11)).toMatchObject({
			effectivePause: "exempt",
			sidecar: { exemptUntilMs: 3_600_010 },
		});
		for (const sidecar of [
			{ pausedBy: "exempt" },
			{ pausedBy: "exempt", pausedAtMs: Number.NaN },
			{ pausedBy: "exempt", pausedAtMs: 1, exemptUntilMs: Number.NaN },
		] as const) {
			expect(reconcileWatchdogExemption(sidecar, 2)).toMatchObject({
				effectivePause: undefined,
				sidecar: {},
			});
		}
	});
});

describe("isFireDue", () => {
	const enabled = effectiveWatchdog({ intervalMs: 100 });

	it("fires at the exact interval anchored to the newest activity", () => {
		expect(isFireDue(enabled, null, 1_000, 1_099)).toBe(false);
		expect(isFireDue(enabled, null, 1_000, 1_100)).toBe(true);
	});

	it("re-anchors the clock when the peer works after the previous fire", () => {
		expect(isFireDue(enabled, 1_000, 1_090, 1_150)).toBe(false);
		expect(isFireDue(enabled, 1_000, 1_090, 1_190)).toBe(true);
	});

	it("keeps every interval for a frozen peer rather than skipping later fires", () => {
		const fireTimes = [100, 200, 300, 400];
		let lastFireAt: number | null = null;
		for (const nowMs of fireTimes) {
			expect(isFireDue(enabled, lastFireAt, 0, nowMs)).toBe(true);
			lastFireAt = nowMs;
			expect(isFireDue(enabled, lastFireAt, 0, nowMs + 99)).toBe(false);
		}
	});

	it("never fires while disabled, paused, or exempt", () => {
		const blocked = [
			effectiveWatchdog({ enabled: false, intervalMs: 1 }),
			effectiveWatchdog({ intervalMs: 1, pausedBy: "self" }),
			effectiveWatchdog({ intervalMs: 1, pausedBy: "compact" }),
			effectiveWatchdog({ intervalMs: 1, pausedBy: "exempt" }),
		];
		for (const cfg of blocked) expect(isFireDue(cfg, 0, 0, 10_000)).toBe(false);
	});
});

describe("watchdogScheduleAnchorMs", () => {
	it("uses statusAt when present and startedAt as the never-null floor", () => {
		expect(
			watchdogScheduleAnchorMs({
				statusAt: "1970-01-01T00:00:00.090Z",
				startedAt: "1970-01-01T00:00:00.000Z",
			}),
		).toBe(90);
		expect(
			watchdogScheduleAnchorMs({
				statusAt: undefined,
				startedAt: "1970-01-01T00:00:00.000Z",
			}),
		).toBe(0);
	});

	it("returns null only when no scheduling timestamp can be proved", () => {
		expect(watchdogScheduleAnchorMs({ statusAt: "invalid", startedAt: "also-invalid" })).toBeNull();
	});
});

describe("evaluateResponse", () => {
	it("derives suspect then stalled from consecutive delivered-but-silent fires", () => {
		expect(
			evaluateResponse({
				cfg: effectiveWatchdog(),
				consecutiveSilentFires: 1,
				eventAdvanced: false,
				pane: { changed: false, workingTransition: false },
			}),
		).toBe("suspect");
		expect(
			evaluateResponse({
				cfg: effectiveWatchdog(),
				consecutiveSilentFires: 2,
				eventAdvanced: false,
				pane: { changed: false, workingTransition: false },
			}),
		).toBe("stalled");
	});

	it("lets a paneless pi peer become stalled from event silence alone", () => {
		expect(
			evaluateResponse({
				cfg: effectiveWatchdog(),
				consecutiveSilentFires: 2,
				eventAdvanced: false,
			}),
		).toBe("stalled");
	});

	it("clears a prior silent-fire streak when real event activity recovers", () => {
		expect(
			evaluateResponse({
				cfg: effectiveWatchdog(),
				consecutiveSilentFires: 4,
				eventAdvanced: true,
			}),
		).toBe("responsive");
	});

	it("accepts a real pane delta or working transition as recovery", () => {
		const base = {
			cfg: effectiveWatchdog(),
			consecutiveSilentFires: 2,
			eventAdvanced: false,
		};
		expect(evaluateResponse({ ...base, pane: { changed: true, workingTransition: false } })).toBe(
			"responsive",
		);
		expect(evaluateResponse({ ...base, pane: { changed: false, workingTransition: true } })).toBe(
			"responsive",
		);
	});

	it("does not let any watchdog-attributable activity mask a frozen peer", () => {
		// s096 task 3.5 (a) — the true pij#136 intent, now stated precisely: the
		// observer's OWN traffic (an injected pane redraw, a delivery receipt
		// advancing lastEventAt) is not evidence of anything, so a peer that never
		// answered is still `stalled`. Passes before and after the fix; it is the
		// guard against over-applying pij#148, NEVER evidence of it.
		expect(
			evaluateResponse({
				cfg: effectiveWatchdog(),
				consecutiveSilentFires: 2,
				eventAdvanced: true,
				eventAdvanceWasWatchdog: true,
				pane: {
					changed: true,
					changeWasWatchdog: true,
					workingTransition: true,
					workingTransitionWasWatchdog: true,
				},
			}),
		).toBe("stalled");
	});

	it("caps a peer that ANSWERED at suspect instead of stalling it", () => {
		// s096 task 3.5 (b) — the question the assertion above conflated with its
		// own. Identical fixture, one difference: the peer moved `statusAt` by its
		// own hand, which only `pij report` can do. That is LIVENESS, not recovery —
		// it must not promote the peer to `responsive`, and it must stop the climb
		// to `stalled`, because `stalled` means SILENT (pij#148).
		//
		// Fails pre-fix for the opposite reason: pre-fix this input yields `stalled`.
		expect(
			evaluateResponse({
				cfg: effectiveWatchdog(),
				consecutiveSilentFires: 2,
				eventAdvanced: true,
				eventAdvanceWasWatchdog: true,
				answeredSinceLastFire: true,
				pane: {
					changed: true,
					changeWasWatchdog: true,
					workingTransition: true,
					workingTransitionWasWatchdog: true,
				},
			}),
		).toBe("suspect");
	});

	it("keeps answering a seat that answers forever at suspect, never stalled", () => {
		// AC-07 at the derivation level: the counter may climb arbitrarily high and
		// the verdict still caps, so the cap is a property of ANSWERING rather than
		// an artefact of a small counter.
		for (const consecutiveSilentFires of [1, 2, 7, 99]) {
			expect(
				evaluateResponse({
					cfg: effectiveWatchdog(),
					consecutiveSilentFires,
					eventAdvanced: false,
					answeredSinceLastFire: true,
				}),
			).toBe("suspect");
		}
	});

	it("excludes exempt peers from unresponsive derivation without certifying health", () => {
		// s096 task 1.8 — SPLIT, not flipped. `pausedBy` means SUPERVISION IS OFF,
		// which is a statement about the watchdog and not about the peer. The
		// surviving intent is that an exempt peer is never CLASSIFIED; what the old
		// single assertion additionally claimed — that it is healthy — is meaning (2)
		// of the four this plan separates, and nothing measured it.
		const verdict = evaluateResponse({
			cfg: effectiveWatchdog({ pausedBy: "exempt" }),
			consecutiveSilentFires: 99,
			eventAdvanced: false,
		});
		// (a) never classified — passes before and after.
		expect(verdict).not.toBe("suspect");
		expect(verdict).not.toBe("stalled");
		// (b) and never certified healthy either. Positively identified, so it
		//     cannot be satisfied by absence. Fails pre-fix, which returns
		//     "responsive" — the opposite reason.
		expect(verdict).toBe("unknown");
	});
});

describe("buildWatchdogTurn", () => {
	it("builds a concise self-teaching turn", () => {
		const body = buildWatchdogTurn("pij-frozen-peer", 2, {
			...effectiveWatchdog(),
			paneAvailable: true,
		});
		expect(body).toMatchInlineSnapshot(
			`"[pij watchdog #2 for pij-frozen-peer] Keep going if working. Report in one call with \`pij report now "<what I just did>" "<what's next>"\`. If this unit of work is finished, run \`pij report state done\`; if you are idle but available on a standing assignment, run \`pij report state ready\`."`,
		);
		expect(body).toContain("pij report state done");
		expect(body).not.toContain("pij watchdog pause");
		expect(body).not.toContain("pij watchdog resume");
		expect(body).toContain('pij report now "<what I just did>" "<what\'s next>"');
		expect(body.length).toBeLessThanOrEqual(400);
	});

	it("notes event-only detection when pane capture is unavailable", () => {
		const body = buildWatchdogTurn("pij-paneless", 1, {
			...effectiveWatchdog(),
			paneAvailable: false,
		});
		expect(body).toContain("Pane capture unavailable; watching event activity only.");
		expect(body.length).toBeLessThanOrEqual(400);
	});

	// ── a seat that owes no card still gets pinged, just not for a card ───────
	it("asks a card-less seat for LIVENESS, never for a card", () => {
		// Jordan's ruling 2026-07-30: a prime owes no status card. But it must not
		// go silent either — a prime is the only seat with NO supervisor, so this
		// ping is its sole external heartbeat.
		const body = buildWatchdogTurn("pij-some-prime", 3, {
			...effectiveWatchdog(),
			paneAvailable: true,
			owesCard: false,
		});
		expect(body).toContain("Keep going if working.");
		// SPEC, not a pin: this used to assert the prime copy "do NOT owe a status
		// card". Jordan REVERSED that on 2026-07-31
		// (.harness/government/rulings/2026-07-31-primes-owe-status-cards.md), so the
		// card-less branch now serves the PA only and carries no prime language.
		expect(body).toContain("You owe no status card");
		expect(body).not.toContain("prime reports to its human in-pane");
		expect(body).not.toContain('pij report now "<what I just did>"');
		// Lifecycle survives: not owing a card is not the same as never finishing.
		expect(body).toContain("pij report state done");
		// The ALTITUDE clause moved with the obligation: it now rides the
		// card-OWING copy for a prime, so a card-less seat must not carry it.
		expect(body).not.toContain("never a restatement of what a stream already reported");
	});

	it("still demands a card when the seat owes one, wired or defaulted", () => {
		for (const cfg of [{ owesCard: true }, {}]) {
			const body = buildWatchdogTurn("pij-some-pm", 1, {
				...effectiveWatchdog(),
				paneAvailable: true,
				...cfg,
			});
			expect(body).toContain('pij report now "<what I just did>" "<what\'s next>"');
			expect(body).not.toContain("do NOT owe a status card");
		}
	});
});

describe("capture policy", () => {
	it("defaults to anomaly-only capture", () => {
		expect(shouldCapture({}, false)).toBe(false);
		expect(shouldCapture({}, true)).toBe(true);
	});

	it("supports always and never modes", () => {
		expect(shouldCapture({ mode: "always" }, false)).toBe(true);
		expect(shouldCapture({ mode: "never" }, true)).toBe(false);
		expect(shouldCapture({ mode: "anomaly" }, true)).toBe(true);
	});

	it("returns an empty slice for an empty pane or zero caps", () => {
		expect(captureSlice("", {})).toBe("");
		expect(captureSlice("text", { maxLines: 0 })).toBe("");
		expect(captureSlice("text", { maxBytes: 0 })).toBe("");
	});

	it("uses the default 40-line and 4096-byte caps", () => {
		const lines = Array.from({ length: 41 }, (_, index) => `line-${index}`);
		expect(captureSlice(lines.join("\n"), {}).split("\n")).toHaveLength(40);
		expect(Buffer.byteLength(captureSlice("x".repeat(4_097), {}), "utf8")).toBe(4_096);
	});

	it("keeps an exact line cap and takes only the tail when over it", () => {
		expect(captureSlice("one\ntwo", { maxLines: 2, maxBytes: 100 })).toBe("one\ntwo");
		expect(captureSlice("one\ntwo\nthree", { maxLines: 2, maxBytes: 100 })).toBe("two\nthree");
	});

	it("keeps an exact byte cap and takes a code-point-safe tail when over it", () => {
		expect(captureSlice("1234", { maxLines: 40, maxBytes: 4 })).toBe("1234");
		expect(captureSlice("12345", { maxLines: 40, maxBytes: 4 })).toBe("2345");
		expect(captureSlice("A🙂B", { maxLines: 40, maxBytes: 5 })).toBe("🙂B");
		expect(captureSlice("🙂🙂", { maxLines: 40, maxBytes: 3 })).toBe("");
	});

	it("enforces hard ceilings even when a policy asks for more", () => {
		const lines = Array.from({ length: 205 }, (_, index) => `line-${index}`);
		const captured = captureSlice(lines.join("\n"), {
			maxLines: 999,
			maxBytes: 999_999,
		});
		expect(captured.split("\n")).toHaveLength(200);
		expect(captured).toContain("line-204");
		expect(Buffer.byteLength(captured, "utf8")).toBeLessThanOrEqual(16_384);

		const bytes = captureSlice("x".repeat(16_385), {
			maxLines: 999,
			maxBytes: 999_999,
		});
		expect(Buffer.byteLength(bytes, "utf8")).toBe(16_384);
	});
});

describe("applyCompactPause", () => {
	it("creates a compact pause and is idempotent", () => {
		const paused = applyCompactPause(undefined, 1_000);
		expect(paused).toEqual({ pausedBy: "compact", pausedAtMs: 1_000 });
		expect(applyCompactPause(paused, 2_000)).toBe(paused);
	});

	it("does not downgrade a self pause or change an exemption", () => {
		const self: WatchdogSidecar = { pausedBy: "self", pausedAtMs: 1 };
		const exempt: WatchdogSidecar = { pausedBy: "exempt", pausedAtMs: 1 };
		expect(applyCompactPause(self, 2)).toBe(self);
		expect(applyCompactPause(exempt, 2)).toBe(exempt);
	});
});

describe("parseWatchdogInterval", () => {
	it("parses human durations to milliseconds", () => {
		expect(parseWatchdogInterval("30s")).toBe(30_000);
		expect(parseWatchdogInterval("20m")).toBe(1_200_000);
		expect(parseWatchdogInterval("1h")).toBe(3_600_000);
		expect(parseWatchdogInterval("90m")).toBe(5_400_000);
	});

	it("accepts a bare integer as milliseconds", () => {
		expect(parseWatchdogInterval("1200000")).toBe(1_200_000);
	});

	it("rejects non-positive, fractional, malformed, and unsafe input", () => {
		for (const bad of [
			"0",
			"0s",
			"-5m",
			"abc",
			"",
			"5x",
			"1.5h",
			"m",
			"10 m",
			"9007199254740991h",
		]) {
			expect(parseWatchdogInterval(bad)).toBeNull();
		}
	});
});

describe("describeWatchdogState", () => {
	// Activation-day finding (pij-civilian-takin, echoed by a mastodon seat): a
	// SUCCESSFUL pause rendered "enabled · self" — the word "paused" never
	// appeared, and the field carrying the pause state was positional and
	// unlabelled. The state must say what it is.
	it("says paused, and by whom, for each pause tier", () => {
		expect(describeWatchdogState({ enabled: true, pausedBy: "self" })).toBe("paused (self)");
		expect(describeWatchdogState({ enabled: true, pausedBy: "compact" })).toBe("paused (compact)");
	});

	it("names a live exemption as its own state", () => {
		expect(describeWatchdogState({ enabled: true, pausedBy: "exempt" })).toBe("exempt");
	});

	it("says watching when it is actually watching", () => {
		expect(describeWatchdogState({ enabled: true })).toBe("watching");
	});

	it("says disabled regardless of tier", () => {
		expect(describeWatchdogState({ enabled: false })).toBe("disabled");
		expect(describeWatchdogState({ enabled: false, pausedBy: "self" })).toBe("disabled");
	});

	it("surfaces the Plan-056 dominating states in priority order", () => {
		// globally-disabled dominates everything; relay dominates the sidecar.
		expect(describeWatchdogState({ enabled: false, globallyDisabled: true })).toBe(
			"globally-disabled",
		);
		expect(
			describeWatchdogState({ enabled: false, globallyDisabled: true, pausedBy: "self" }),
		).toBe("globally-disabled");
		expect(describeWatchdogState({ enabled: false, relay: true })).toBe("relay (never watched)");
	});

	it("never renders a paused, relay, or globally-disabled watchdog as bare 'enabled'", () => {
		for (const state of [
			{ enabled: true, pausedBy: "self" as const },
			{ enabled: true, pausedBy: "compact" as const },
			{ enabled: true, pausedBy: "exempt" as const },
			{ enabled: false, relay: true },
			{ enabled: false, globallyDisabled: true },
		]) {
			expect(describeWatchdogState(state)).not.toBe("enabled");
		}
	});
});

describe("mutesWatchdogNudge — parked seats are not nudged (DL-002)", () => {
	// Both anomaly detectors already exempt the parked states; the watchdog, the
	// only mechanism that pushes a turn into a human-visible pane, did not. A seat
	// punished for declaring learns to stay silent — which destroys the axis the
	// declaration exists to populate.
	it("mutes exactly the four parked states", () => {
		for (const state of ["waiting", "hold", "blocked", "question"] as const) {
			expect(mutesWatchdogNudge(state)).toBe(true);
		}
	});

	it("does NOT mute an active or terminal seat", () => {
		// `done`/`failed`/`cancelled` are claims to be VERIFIED, not reasons to stop
		// watching — the s075 lesson that muting and discharging are different acts.
		for (const state of ["ready", "done", "failed", "cancelled"] as const) {
			expect(mutesWatchdogNudge(state)).toBe(false);
		}
	});

	it("does not mute an UNDECLARED seat — absence is not a declaration", () => {
		// The whole point is to reward declaring. Muting on undefined would reward
		// silence instead, which is the exact inversion.
		expect(mutesWatchdogNudge(undefined)).toBe(false);
	});

	it("covers every SEMANTIC_STATES member — no state defaults into muting", () => {
		// Guards the enum growing without a decision: a new state must be classified
		// deliberately, never inherit muting by omission.
		for (const state of SEMANTIC_STATES) {
			expect(typeof mutesWatchdogNudge(state)).toBe("boolean");
		}
		expect(SEMANTIC_STATES.filter((s) => mutesWatchdogNudge(s)).sort()).toEqual([
			"blocked",
			"hold",
			"question",
			"waiting",
		]);
	});
});

/** Two primes hit these within an hour, and one drew a false conclusion from
 *  the count that cost it a wrong belief about its own notification path. */
describe("watchdog status is legible to the human reading it", () => {
	it("names the watchers instead of counting them", () => {
		expect(renderWatcherRoster(["pij-mere-mackerel"])).toBe("watchers 1 (pij-mere-mackerel)");
		expect(renderWatcherRoster(["pij-a", "pij-b"])).toBe("watchers 2 (pij-a, pij-b)");
	});

	it("says none rather than zero — an absence a reader can act on", () => {
		expect(renderWatcherRoster([])).toBe("watchers none");
	});

	it("renders a count of 2 with BOTH names, which is the case that misled a prime", () => {
		// It read "watchers 2", inferred "me plus presumably an earlier
		// registrant", and was wrong — there was no earlier registrant. At
		// count >= 2 a bare number is a coin flip about who is in the path.
		const line = renderWatcherRoster(["pij-statutory-seahorse", "pij-wee-albatross"]);
		expect(line).toContain("pij-statutory-seahorse");
		expect(line).toContain("pij-wee-albatross");
	});

	it("humanises durations so nobody hand-divides milliseconds", () => {
		expect(humanizeDurationMs(7_200_000)).toBe("2h");
		expect(humanizeDurationMs(1_200_000)).toBe("20m");
		expect(humanizeDurationMs(45_000)).toBe("45s");
	});

	it("keeps the remainder rather than rounding a supervision interval", () => {
		// A rounded interval invites the same arithmetic the raw ms did.
		expect(humanizeDurationMs(9_000_000)).toBe("2h 30m");
		expect(humanizeDurationMs(90_000)).toBe("1m 30s");
	});

	it("degrades honestly on sub-second, zero, and nonsense inputs", () => {
		expect(humanizeDurationMs(0)).toBe("0s");
		expect(humanizeDurationMs(250)).toBe("250ms");
		expect(humanizeDurationMs(-1)).toBe("-1ms");
		expect(humanizeDurationMs(Number.NaN)).toBe("NaNms");
	});
});
