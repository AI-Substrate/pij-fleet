// pij-messaging — whole-life peer watchdog decisions (pure).

import type { SemanticState, WatchdogPauseTier, WatchdogSidecar } from "./types.js";

// ─── watchdog configuration ─────────────────────────────────────────────────
export const DEFAULT_WATCHDOG_INTERVAL_MS = 20 * 60 * 1_000;
/** A safety exemption is deliberately temporary: a daemon restart must never
 * turn one incident response into permanent watchdog blindness. */
export const DEFAULT_WATCHDOG_EXEMPT_TTL_MS = 60 * 60 * 1_000;

export interface EffectiveWatchdogConfig {
	readonly enabled: boolean;
	readonly intervalMs: number;
	readonly pausedBy: WatchdogPauseTier | undefined;
}

/** Resolve a migration-safe watchdog configuration. No sidecar means default-on. */
export function effectiveWatchdog(sidecar?: WatchdogSidecar): EffectiveWatchdogConfig {
	const requestedInterval = sidecar?.intervalMs;
	const intervalMs =
		requestedInterval !== undefined && Number.isFinite(requestedInterval) && requestedInterval > 0
			? requestedInterval
			: DEFAULT_WATCHDOG_INTERVAL_MS;
	return {
		enabled: sidecar?.enabled ?? true,
		intervalMs,
		pausedBy: sidecar?.pausedBy,
	};
}

function withoutPause(sidecar: WatchdogSidecar): WatchdogSidecar {
	const resumed = { ...sidecar };
	delete resumed.pausedBy;
	delete resumed.pausedAtMs;
	delete resumed.exemptUntilMs;
	return resumed;
}

/** Create a bounded incident exemption. Callers share this seam so CLI and
 * PIJ_NO_WATCHDOG boot can never disagree about the re-arm deadline. */
export function applyWatchdogExemption(
	sidecar: WatchdogSidecar | undefined,
	nowMs: number,
	ttlMs = DEFAULT_WATCHDOG_EXEMPT_TTL_MS,
): WatchdogSidecar {
	const duration =
		Number.isSafeInteger(ttlMs) && ttlMs > 0 ? ttlMs : DEFAULT_WATCHDOG_EXEMPT_TTL_MS;
	const deadline = nowMs + duration;
	const validDeadline =
		Number.isSafeInteger(nowMs) && Number.isSafeInteger(deadline) && deadline > nowMs;
	return {
		...sidecar,
		pausedBy: "exempt",
		pausedAtMs: nowMs,
		exemptUntilMs: validDeadline ? deadline : 0,
	};
}

export interface WatchdogExemptionReconciliation {
	readonly sidecar: WatchdogSidecar | undefined;
	readonly effectivePause: WatchdogPauseTier | undefined;
}

/** Normalize an exemption against an injected clock. Legacy sidecars acquire a
 * one-time deadline; malformed time fails closed so safety-off is never extended. */
export function reconcileWatchdogExemption(
	sidecar: WatchdogSidecar | undefined,
	nowMs: number,
): WatchdogExemptionReconciliation {
	if (!sidecar) return { sidecar: undefined, effectivePause: undefined };
	if (sidecar.pausedBy !== "exempt") {
		if (sidecar.exemptUntilMs === undefined) {
			return { sidecar, effectivePause: sidecar.pausedBy };
		}
		const normalized = { ...sidecar };
		delete normalized.exemptUntilMs;
		return { sidecar: normalized, effectivePause: normalized.pausedBy };
	}

	const deadline = sidecar.exemptUntilMs;
	if (deadline !== undefined) {
		if (Number.isSafeInteger(deadline) && Number.isSafeInteger(nowMs) && nowMs < deadline) {
			return { sidecar, effectivePause: "exempt" };
		}
		const normalized = withoutPause(sidecar);
		return { sidecar: normalized, effectivePause: undefined };
	}

	const pausedAtMs = sidecar.pausedAtMs;
	const legacyDeadline =
		typeof pausedAtMs === "number" && Number.isSafeInteger(pausedAtMs)
			? pausedAtMs + DEFAULT_WATCHDOG_EXEMPT_TTL_MS
			: Number.NaN;
	if (
		Number.isSafeInteger(legacyDeadline) &&
		Number.isSafeInteger(nowMs) &&
		nowMs < legacyDeadline
	) {
		return {
			sidecar: { ...sidecar, exemptUntilMs: legacyDeadline },
			effectivePause: "exempt",
		};
	}
	const normalized = withoutPause(sidecar);
	return { sidecar: normalized, effectivePause: undefined };
}

/** Apply the explicit `pij watchdog resume` transition. Exemptions are stronger. */
export function applyWatchdogResume(sidecar: WatchdogSidecar): WatchdogSidecar {
	if (sidecar.pausedBy === "exempt" || sidecar.pausedBy === undefined) return sidecar;
	return withoutPause(sidecar);
}

/** Compact pauses resume automatically only when a real working transition occurs. */
export function applyWorkingTransition(sidecar: WatchdogSidecar): WatchdogSidecar {
	return sidecar.pausedBy === "compact" ? withoutPause(sidecar) : sidecar;
}

/** A self pause applies to finished work, not to a later dispatch or assignment. */
export function applyNewWorkTransition(sidecar: WatchdogSidecar): WatchdogSidecar {
	return sidecar.pausedBy === "self" ? withoutPause(sidecar) : sidecar;
}

/** Shared pure seam used before either local or remote compact delivery. */
export function applyCompactPause(
	sidecar: WatchdogSidecar | undefined,
	nowMs: number,
): WatchdogSidecar {
	if (sidecar?.pausedBy !== undefined) return sidecar;
	return { ...sidecar, pausedBy: "compact", pausedAtMs: nowMs };
}

// ─── scheduler ──────────────────────────────────────────────────────────────
export interface WatchdogScheduleSource {
	readonly statusAt?: string;
	readonly startedAt: string;
}

/** The PM reporting clock, newest-wins. `startedAt` is the floor so a PM that
 * has never reported still becomes due; null means no timestamp can be proved. */
export function watchdogScheduleAnchorMs(source: WatchdogScheduleSource): number | null {
	let newest: number | null = null;
	for (const stamp of [source.statusAt, source.startedAt]) {
		if (typeof stamp !== "string") continue;
		const parsed = Date.parse(stamp);
		if (!Number.isFinite(parsed)) continue;
		if (newest === null || parsed > newest) newest = parsed;
	}
	return newest;
}

/**
 * A fire is due one full interval after the newest supplied schedule anchor or
 * delivered fire. WatchdogManager supplies PM `statusAt` with a `startedAt`
 * floor, deliberately removing ordinary activity re-anchoring: a PM that works
 * without reporting is still nudged. Delivered fires retain the freeze cadence.
 */
export function isFireDue(
	cfg: EffectiveWatchdogConfig,
	lastFireAt: number | null,
	scheduleAnchorAt: number | null,
	nowMs: number,
): boolean {
	if (!cfg.enabled || cfg.pausedBy !== undefined) return false;
	const anchors = [lastFireAt, scheduleAnchorAt].filter(
		(value): value is number => value !== null && Number.isFinite(value),
	);
	if (anchors.length === 0) return false;
	return nowMs - Math.max(...anchors) >= cfg.intervalMs;
}

// ─── response derivation ────────────────────────────────────────────────────
/** What a watchdog fire concluded about a peer.
 *
 * `unknown` means **NO EVIDENCE WAS EXAMINED**. It is *never* a health claim and
 * no consumer may read it as one — it is the absence of a grade, not a passing
 * grade.
 *
 * It exists because this type had three values and FOUR meanings (pij#161):
 * `responsive` was returned for *measured alive*, for *supervision is off*, and
 * for *nothing was examined*, and only the first is what a reader takes it to
 * mean. The most dangerous instance was structural — a fire with no response
 * outstanding delivered a variable INITIALISER to the watcher verbatim, so the
 * moment supervision mattered most (a freshly rebuilt RuntimeState after a
 * daemon restart or a pid flap) was the moment it certified health it had not
 * measured.
 *
 * The token matches this repo's existing vocabulary for *no positive
 * identification* — `DeathReason.unknown` (`core/types.ts:71`) — rather than
 * coining a new one.
 */
export type WatchdogResponse = "responsive" | "suspect" | "stalled" | "unknown";

/** Pane-derived inputs are absent for paneless peers. Watchdog-only changes are
 * explicit so the observer's own injected turn cannot fabricate recovery. */
export interface WatchdogPaneObservation {
	readonly changed: boolean;
	readonly workingTransition: boolean;
	readonly changeWasWatchdog?: boolean;
	readonly workingTransitionWasWatchdog?: boolean;
}

export interface WatchdogResponseInputs {
	readonly cfg: EffectiveWatchdogConfig;
	readonly consecutiveSilentFires: number;
	readonly eventAdvanced: boolean;
	readonly eventAdvanceWasWatchdog?: boolean;
	readonly pane?: WatchdogPaneObservation;
	/** Did the PEER ITSELF answer since the last fire?
	 *
	 * True only when `statusAt` advanced, which only the peer's own `pij report`
	 * can do — `registry-write.ts:90` maps `statusAt` to the `"cli"` writer, so
	 * neither the daemon nor the delivery path can fabricate it. The watchdog turn
	 * literally instructs the peer to run `pij report now …`, so *"statusAt
	 * advanced since the fire"* IS the definition of answering.
	 *
	 * **Deliberately NOT `lastEventAt`** (plan 096, KF-13): delivering a watchdog
	 * turn makes the target emit a RECEIPT, and that persists `lastEventAt` with
	 * zero model involvement (`core/session.ts` `onInbound` → `emitReceipt` →
	 * `capture("receipt")` → `persist`). The act of supervising writes that field,
	 * so keying on it would make `stalled` UNREACHABLE for a wedged peer — a
	 * false-positive traded for a false-negative, which for a supervision
	 * instrument is strictly worse.
	 *
	 * This is **liveness** evidence, never **recovery** evidence. The pij#136
	 * disqualification below is untouched by it: an answered peer is still never
	 * called `responsive`, only capped at `suspect`. */
	readonly answeredSinceLastFire?: boolean;
}

/** Derive response health from delivered fires and independently observed work. */
export function evaluateResponse(inputs: WatchdogResponseInputs): WatchdogResponse {
	// Supervision is OFF for this peer. That is a statement about the WATCHDOG,
	// not about the peer, so it must not be reported as health (pij#161).
	if (!inputs.cfg.enabled || inputs.cfg.pausedBy !== undefined) return "unknown";

	const realEventAdvance = inputs.eventAdvanced && !inputs.eventAdvanceWasWatchdog;
	const realPaneChange = inputs.pane?.changed === true && !inputs.pane.changeWasWatchdog;
	const realWorkingTransition =
		inputs.pane?.workingTransition === true && !inputs.pane.workingTransitionWasWatchdog;
	if (realEventAdvance || realPaneChange || realWorkingTransition) return "responsive";
	// A peer that ANSWERED is alive, so `stalled` — which must mean SILENT — is off
	// the table for it; the climb caps at `suspect` and stays there (pij#148). This
	// is placed AFTER the recovery block on purpose: answering proves life, not
	// independent work, so it can never promote a peer to `responsive`.
	if (inputs.answeredSinceLastFire === true) return "suspect";
	if (inputs.consecutiveSilentFires >= 2) return "stalled";
	if (inputs.consecutiveSilentFires === 1) return "suspect";
	return "responsive";
}

/** Is this verdict an ANOMALY — a graded finding an anomaly-policy watcher should
 * capture?
 *
 * Written as an explicit POSITIVE test, never as `response !== "responsive"`.
 * That set-level shape is precisely what made widening the union dangerous: a
 * negation over a set silently reclassifies every NEW member, so a no-evidence
 * fire would have counted as an anomaly and triggered watcher captures on every
 * first fire after a daemon restart — fresh noise created by the fix
 * (plan 096, KF-05).
 *
 * Exhaustive by compiler, in the same style as `mutesWatchdogNudge`: a future
 * fifth member fails to compile here rather than defaulting into anomaly. */
export function isAnomalyVerdict(response: WatchdogResponse): boolean {
	switch (response) {
		case "suspect":
		case "stalled":
			return true;
		case "responsive":
		case "unknown":
			return false;
		default: {
			const exhaustive: never = response;
			return exhaustive;
		}
	}
}

/** Render a verdict as the head of a watcher notice.
 *
 * `unknown` carries a second line saying WHY there is no grade, so a watcher can
 * never read it as a quiet pass. Suppressing the notice entirely was the other
 * option in pij#161 and was rejected: silence would then mean *nothing happened*
 * OR *something happened I could not grade*, which is the same
 * absence-renders-as-something defect one level up, and harder to notice because
 * there is nothing to look at.
 *
 * Exhaustive by compiler for the same reason as `isAnomalyVerdict`. */
export function verdictNoticeLines(response: WatchdogResponse, id: string): string[] {
	switch (response) {
		case "responsive":
		case "suspect":
		case "stalled":
			return [`watchdog ${response}: ${id}`];
		case "unknown":
			return [
				`watchdog unknown: ${id}`,
				"no response was outstanding — nothing was examined; this is not a health claim",
			];
		default: {
			const exhaustive: never = response;
			return exhaustive;
		}
	}
}

// ─── parked-state nudge muting (plan 076, DL-002) ───────────────────────────
/** Does this declared semantic state mute the PEER-FACING NUDGE?
 *
 * Both anomaly detectors already exempt the parked states; the watchdog — the
 * only mechanism that actually pushes a turn into a human-visible pane — did
 * not, so a seat that correctly declared `question` kept being nudged. The
 * incentive damage is the mission: **a seat punished for declaring learns to
 * stay silent**, which destroys the axis the declaration exists to populate.
 * (Live evidence: the seat that wrote this fix burned four nudges while
 * correctly parked on a human-gated merge.)
 *
 * MUTING IS NOT UNWATCHING. This suppresses one outbound nudge and nothing
 * else — eligibility, liveness classification, the stall detector, and the
 * dead/provider-failure axes are all untouched, because a parked seat can
 * still die and that must still be noticed. Muting and supervising are
 * different acts, exactly as muting and discharging were in s075.
 *
 * `done`/`failed`/`cancelled` are deliberately NOT parked: a terminal claim is
 * something to be VERIFIED, not a reason to stop watching. `ready` is not
 * parked either — it is the active word.
 *
 * Exhaustive by construction: a new SEMANTIC_STATES member fails the
 * `satisfies never` check below rather than silently defaulting into muting.
 * A state that mutes supervision must be chosen, never inherited.
 */
export function mutesWatchdogNudge(state: SemanticState | undefined): boolean {
	if (state === undefined) return false;
	switch (state) {
		case "blocked":
		case "question":
		case "hold":
		case "waiting":
			return true;
		case "ready":
		case "failed":
		case "cancelled":
		case "done":
			return false;
		default: {
			const exhaustive: never = state;
			return exhaustive;
		}
	}
}

// ─── self-teaching watchdog turn ────────────────────────────────────────────
export interface WatchdogTurnConfig extends EffectiveWatchdogConfig {
	readonly paneAvailable?: boolean;
	/** Does this seat owe a status card? See `owesStatusCard` — PM yes, prime no.
	 *  Selects the COPY only; eligibility to be watched at all is a separate
	 *  question and primes remain watched. Defaults to the card-owing copy so an
	 *  un-wired caller keeps today's behaviour. */
	readonly owesCard?: boolean;
	/** True for a PRIME: appends the altitude clause to the card ask, because a
	 *  prime's card must be its own governance work rather than a restatement of
	 *  what a stream already reported (Jordan's 2026-07-30 altitude ruling, which
	 *  SURVIVED the 2026-07-31 reversal of the card obligation itself). */
	readonly ownAltitude?: boolean;
}

export function buildWatchdogTurn(id: string, ordinal: number, cfg: WatchdogTurnConfig): string {
	const head = `[pij watchdog #${ordinal} for ${id}] Keep going if working.`;
	// A seat that owes no card still needs the ping — it must not go silent. A
	// prime is the only seat on the box with NO supervisor: a wedged PM is caught
	// by its prime, a wedged prime is caught by nobody, so this is its sole
	// external heartbeat.
	//
	// PRIMES OWE A CARD (Jordan, 2026-07-31 — .harness/government/rulings/
	// 2026-07-31-primes-owe-status-cards.md, REVERSING the 2026-07-30 position).
	// That reversal reached the skill payload and a ruling file and never reached
	// this emitter, which went on telling every prime the opposite on a timer —
	// roughly 3/hour/prime. A STALE DOCUMENT IS PASSIVE: it fails to correct you.
	// A STALE ENFORCER IS ACTIVE: it propagates the wrong rule to every seat it
	// touches, on schedule, and looks authoritative doing it.
	//
	// The card-less branch now serves the PA only, so it carries no prime
	// language: a PA owes no card, and a staleness label is watchdog language
	// that lies where no obligation exists.
	//
	// THE ALTITUDE CLAUSE SURVIVES the reversal and rides the card-owing copy for
	// a prime: its card must be its OWN governance work, never a restatement of
	// what a stream already reported, which double-renders the same fact in the
	// rail. Observed live: an o-prime led two consecutive cards with its stream's
	// merge, a fact that stream had already filed itself.
	const ask =
		`Report in one call with \`pij report now "<what I just did>" "<what's next>"\`.` +
		(cfg.ownAltitude === true
			? " Make it your OWN governance work, never a restatement of what a stream already reported."
			: "");
	// THE CLOSE OFFERED A FALSE DICHOTOMY: keep going, or declare `done`. On a
	// STANDING assignment — PM a stream, run a government — there is no
	// completion to declare, so `done` asserts the stream is finished and
	// silence rots the card. A prompt that offers only wrong answers gets wrong
	// answers from honest seats, and the ones who answer accurately look
	// non-compliant. `ready` already exists for exactly this (state.ts:
	// "awaiting pickup"), so this names an existing state rather than adding
	// vocabulary, and leads with the CONDITION rather than the verb.
	//
	// `waiting` is DELIBERATELY NOT OFFERED here: parking with no blocker
	// recreates the parked-but-working state, which is a permanent silencer —
	// offering it to an unblocked seat would manufacture that defect on a timer.
	// `ready` is the honest answer for idle-but-available; `waiting` stays for an
	// actual blocker.
	const close =
		"If this unit of work is finished, run `pij report state done`; " +
		"if you are idle but available on a standing assignment, run `pij report state ready`.";
	const turn =
		cfg.owesCard === false
			? `${head} You owe no status card — keep the ping honest by staying responsive. ${close}`
			: `${head} ${ask} ${close}`;
	return cfg.paneAvailable === false
		? `${turn} Pane capture unavailable; watching event activity only.`
		: turn;
}

// ─── bounded pane capture ───────────────────────────────────────────────────
export type CaptureMode = "anomaly" | "always" | "never";

export interface CapturePolicy {
	readonly mode?: CaptureMode;
	readonly maxLines?: number;
	readonly maxBytes?: number;
}

export const DEFAULT_CAPTURE_LINES = 40;
export const DEFAULT_CAPTURE_BYTES = 4_096;
export const MAX_CAPTURE_LINES = 200;
export const MAX_CAPTURE_BYTES = 16_384;

export function shouldCapture(policy: CapturePolicy, anomaly: boolean): boolean {
	switch (policy.mode ?? "anomaly") {
		case "always":
			return true;
		case "never":
			return false;
		case "anomaly":
			return anomaly;
	}
}

function boundedCap(requested: number | undefined, fallback: number, ceiling: number): number {
	if (requested === undefined || Number.isNaN(requested)) return fallback;
	if (requested === Number.POSITIVE_INFINITY) return ceiling;
	return Math.max(0, Math.min(ceiling, Math.floor(requested)));
}

const UTF8_ENCODER = new TextEncoder();

function utf8Length(value: string): number {
	return UTF8_ENCODER.encode(value).byteLength;
}

/** Return a UTF-8-safe suffix whose line and byte limits are both satisfied. */
export function captureSlice(paneText: string, policy: CapturePolicy): string {
	if (paneText.length === 0) return "";
	const maxLines = boundedCap(policy.maxLines, DEFAULT_CAPTURE_LINES, MAX_CAPTURE_LINES);
	const maxBytes = boundedCap(policy.maxBytes, DEFAULT_CAPTURE_BYTES, MAX_CAPTURE_BYTES);
	if (maxLines === 0 || maxBytes === 0) return "";

	const lineTail = paneText.split("\n").slice(-maxLines).join("\n");
	if (utf8Length(lineTail) <= maxBytes) return lineTail;

	const codePoints = Array.from(lineTail);
	let usedBytes = 0;
	let firstIncluded = codePoints.length;
	for (let index = codePoints.length - 1; index >= 0; index -= 1) {
		const width = utf8Length(codePoints[index] ?? "");
		if (usedBytes + width > maxBytes) break;
		usedBytes += width;
		firstIncluded = index;
	}
	return codePoints.slice(firstIncluded).join("");
}

/** Parse a watchdog interval from a human string to milliseconds (Plan 056).
 *  Accepts `<n>s`/`<n>m`/`<n>h` or a bare integer of milliseconds. Returns null
 *  for anything non-positive, fractional, or malformed — the caller reports the
 *  usage error. Makes `pij watchdog interval <id> 20m` ergonomic. */
export function parseWatchdogInterval(text: string): number | null {
	const match = /^(\d+)(ms|s|m|h)?$/.exec(text.trim());
	if (!match) return null;
	const value = Number(match[1]);
	if (!Number.isSafeInteger(value) || value <= 0) return null;
	const unitMs = { ms: 1, s: 1_000, m: 60_000, h: 3_600_000 } as const;
	const unit = (match[2] ?? "ms") as keyof typeof unitMs;
	const intervalMs = value * unitMs[unit];
	return Number.isSafeInteger(intervalMs) && intervalMs > 0 ? intervalMs : null;
}

// ─── human-readable state ───────────────────────────────────────────────────

/** What the watchdog is actually DOING, in words a reader can act on.
 *
 * The status line used to render `enabled · self` for a successful pause: the
 * word "paused" never appeared, and the tier sat in an unlabelled positional
 * field. `enabled` describes configuration; a reader asking "did my pause land?"
 * needs behaviour. This says the behaviour — and, in priority order, the two
 * states that dominate the per-session config (Plan 056): a globally-disabled
 * runtime and a relay/bridge that is never watched. */
export function describeWatchdogState(state: {
	enabled: boolean;
	pausedBy?: WatchdogPauseTier | null;
	globallyDisabled?: boolean;
	relay?: boolean;
}): string {
	if (state.globallyDisabled) return "globally-disabled";
	if (state.relay) return "relay (never watched)";
	if (!state.enabled) return "disabled";
	if (state.pausedBy === "exempt") return "exempt";
	if (state.pausedBy) return `paused (${state.pausedBy})`;
	return "watching";
}

/** Render a duration for HUMANS, keeping the exact ms in `--json`.
 *
 * The text surface printed `interval 7200000ms`, so every reader hand-divided
 * — and a hand-conversion is exactly what produced a 1200-second error in
 * another government the same day. The machine surface is unchanged; this is
 * only for the line a person reads.
 *
 * Deliberately COARSE and exact-only: `2h`, `20m`, `45s`. A duration that is
 * not a whole unit keeps the larger unit plus the remainder (`2h 30m`) rather
 * than rounding, because a rounded supervision interval invites the same
 * arithmetic the raw ms did.
 */
export function humanizeDurationMs(ms: number): string {
	if (!Number.isFinite(ms) || ms < 0) return `${ms}ms`;
	if (ms === 0) return "0s";
	if (ms < 1_000) return `${ms}ms`;
	const parts: string[] = [];
	const h = Math.floor(ms / 3_600_000);
	const m = Math.floor((ms % 3_600_000) / 60_000);
	const s = Math.floor((ms % 60_000) / 1_000);
	if (h > 0) parts.push(`${h}h`);
	if (m > 0) parts.push(`${m}m`);
	if (s > 0) parts.push(`${s}s`);
	return parts.join(" ");
}

/** Render the watcher ROSTER, not just its size.
 *
 * The text line printed `watchers 2` while `--json` carried the ids. Two primes
 * hit that within an hour: one read `watchers 2`, inferred "me plus presumably
 * some earlier registrant", and was WRONG — there was no earlier registrant. At
 * count >= 2 it is a coin flip whether a government believes an external party
 * is in its notification path when nobody is.
 *
 * A COUNT IS AN ANSWER TO A QUESTION NOBODY ASKED. The names were already in
 * the projection; only the human surface dropped them.
 */
export function renderWatcherRoster(watchers: readonly string[]): string {
	if (watchers.length === 0) return "watchers none";
	return `watchers ${watchers.length} (${watchers.join(", ")})`;
}
