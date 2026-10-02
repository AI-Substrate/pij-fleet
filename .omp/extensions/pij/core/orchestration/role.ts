// pij-orchestration — stored role designation, total projection, and audit append.

import type { PlatformWriteLockPort, SpineLogPort } from "../platform/ports.js";
import { buildSpineEvent } from "../platform/spine.js";
import type { SPINE_KIND_PRIME_SET, SPINE_KIND_ROLE_SET } from "../platform/types.js";
import type { RegistryPort } from "../ports.js";
import { effectiveParent } from "../tree.js";
import { err, ok, type Result, type SessionDescriptor, type SessionId } from "../types.js";

/** The orchestration role persisted on a session descriptor.
 *
 * Stored and projected roles are intentionally different: `"prime"` is NEVER
 * stored here. Prime-ness remains `SessionDescriptor.prime?: boolean`, owned by
 * PrimeService; projections join that flag with this stored partial role.
 */
/** The stored role vocabulary, as DATA.
 *
 * The type is derived FROM this array rather than declared beside it, because a
 * type and a hand-written validator drift silently: `role !== "pm" && role !==
 * "worker"` compares a `string`, so widening a union produces ZERO compile
 * errors at every parser that guards on literals. That is exactly how `pa`
 * became a legal type and an illegal argument. Deriving the type from the array
 * makes the vocabulary single-sourced: a new member is admitted by the guard and
 * named in the usage text by construction, not by remembering.
 */
export const STORED_ORCHESTRATION_ROLES = ["pm", "worker", "pa"] as const;

export type StoredOrchestrationRole = (typeof STORED_ORCHESTRATION_ROLES)[number];

/** Runtime admission for the stored vocabulary — the ONLY parser-side guard. */
export function isStoredOrchestrationRole(value: unknown): value is StoredOrchestrationRole {
	return (
		typeof value === "string" && (STORED_ORCHESTRATION_ROLES as readonly string[]).includes(value)
	);
}

/** `pm|worker|pa` — the vocabulary rendered for usage and error text. */
export const STORED_ROLE_CHOICES = STORED_ORCHESTRATION_ROLES.join("|");

/** The total orchestration role exposed by projections. */
export type OrchestrationRole = "prime" | StoredOrchestrationRole;

type Exact<Left, Right> =
	(<Value>() => Value extends Left ? 1 : 2) extends <Value>() => Value extends Right ? 1 : 2
		? true
		: false;
type Assert<Condition extends true> = Condition;

/** Compiled invariants: widening either alias collapses store-partial/project-total. */
export type StoredOrchestrationRoleExactnessInvariant = Assert<
	Exact<StoredOrchestrationRole, "pm" | "worker" | "pa">
>;
export type OrchestrationRoleExactnessInvariant = Assert<
	Exact<OrchestrationRole, "prime" | "pm" | "worker" | "pa">
>;

type RoleProjectionSource = Pick<SessionDescriptor, "prime" | "orchestrationRole">;

export function projectOrchestrationRole(
	descriptor: RoleProjectionSource,
): OrchestrationRole | null {
	return descriptor.prime === true ? "prime" : (descriptor.orchestrationRole ?? null);
}

export function hasRoleConflict(descriptor: RoleProjectionSource): boolean {
	return descriptor.prime === true && descriptor.orchestrationRole !== undefined;
}

type CardSource = RoleProjectionSource & Pick<SessionDescriptor, "statusAt">;

/** Does this seat OWE a status card — i.e. may it be CHASED for one?
 *
 * PM only (Jordan's ruling, 2026-07-30). A card exists so a layer above can see
 * progress without asking; a PM reports up to a prime, so its card is
 * load-bearing. A PRIME reports up to its human in-pane, conversationally — a
 * card there duplicates a richer channel that already exists. A worker's card
 * renders nowhere at all.
 *
 * This drives NUDGING and the never-reported fallback ONLY. It deliberately does
 * NOT drive staleness alerting — see `cardCanMislead`, and do not merge the two:
 * that conflation is exactly what put a card obligation on five primes.
 */
export function owesStatusCard(descriptor: RoleProjectionSource): boolean {
	const role = projectOrchestrationRole(descriptor);
	// PM and PRIME. Jordan reversed the prime exclusion on 2026-07-31
	// (.harness/government/rulings/2026-07-31-primes-owe-status-cards.md); the reversal
	// reached the skill payload and the ruling file and NOT this predicate, so
	// the watchdog went on telling primes the opposite on a timer for a day.
	// A PA still owes nothing — it is an assistant to a prime, not a reporter.
	return role === "pm" || role === "prime";
}

/** Can this seat's card MISLEAD a reader if it goes stale?
 *
 * True for ANY rendered seat that actually carries a card, regardless of whether
 * it owed one. The asymmetry with `owesStatusCard` is the whole point: **the
 * consumer cannot tell who was obliged to write it.** A rotten card misinforms
 * identically either way, so a prime that VOLUNTARILY writes one has put itself
 * in the render surface and is held to freshness from then on — writing the card
 * is the act that creates the expectation.
 *
 * Not hypothetical: on 2026-07-30 a prime wrote a real card, let it rot 43min
 * past threshold, and was correctly flagged. A blanket role exclusion would have
 * left that unpoliced on the seat with the widest readership.
 *
 * DESIGNED ASYMMETRY WITH THE RAIL (ruled, spine 25457) — do not "reconcile" it:
 * chainglass renders a prime card's AGE but never applies the stale LABEL, while
 * this predicate keeps `pij anomalies` raising status-stale for the same seat.
 * Both are correct because the consumers differ. A row here is NOT an accusation
 * from above: a prime has no supervisor, so the only party who can act on it is
 * the prime itself running its own unscoped sweep. It is SELF-SERVICE signal —
 * nobody is nudged, nobody is chased, but the seat can see its own rot. The LABEL
 * is what would imply a watchdog obligation, which is why the rail withholds it
 * and this does not.
 *
 * `statusAt === undefined` means nothing renders, so nothing can mislead — that
 * is what makes the never-reported prime a non-event rather than a false positive.
 *
 * Note this is `prime || pm`, NOT `role !== null`. The predicate it replaced used
 * the latter, which let an explicitly-stamped `worker` through even though the
 * comment beside it claimed workers were excluded — a stamped worker's card
 * renders nowhere, exactly like an unstamped seat's, and the watchdog already
 * agrees (its eligibility gate is `pm || prime`).
 */
export function cardCanMislead(descriptor: CardSource): boolean {
	if (descriptor.statusAt === undefined) return false;
	const role = projectOrchestrationRole(descriptor);
	// `pa` is EXCLUDED BY DECISION (s080), and the decision rests on the
	// OBLIGATION — never on what renders.
	//
	// A PA's card DOES render. Measured in the emitted DOM 2026-08-01 from
	// anaconda's real row: NOW/NEXT plus an "updated 22h ago" age line, muted
	// rather than amber. So "a PA's card renders nowhere" — which an earlier
	// version of this comment asserted — is simply false, and `floating card` in
	// paLineageRefusal is literal.
	//
	// THE GROUND IS THAT A STALENESS LABEL IS WATCHDOG LANGUAGE, AND WATCHDOG
	// LANGUAGE IS A LIE WHERE NO OBLIGATION EXISTS. This predicate gates "that
	// card is rotten", which tells a reader someone is late. A PA owes no card
	// (Jordan, 2026-07-31), so nobody is late and nothing is owed — the same
	// reasoning that took primes off the card-obligation hook, extended.
	// Supervision is what makes an age MEANINGFUL: labelling a seat nobody
	// nudges does not remove a false claim, it swaps "this is current" for
	// "someone is chasing this".
	//
	// DO NOT RE-GROUND THIS ON RENDERING. Rendering is another repo's live
	// decision and will drift under this file: the consumer has deliberately
	// SPLIT `carriesStatus` (who OWES a card) from `hasOptionalCard` (whose card
	// RENDERS — prime|pa) precisely so the two cannot be re-welded, and an
	// earlier version of this comment welded them here, in a file that outlives
	// the memory of why. Obligation is checkable from inside pij; rendering is
	// not.
	//
	// RUNNING IS NOT MERGED, which is why the rendering ground would have been
	// doubly wrong: the consumer's PA-card fix is an UN-PR'd local commit
	// (0e6da0a9b). Jordan's dev server runs that working tree, so PA cards
	// render FOR HIM AND NOWHERE ELSE — pull main or clone clean and they
	// silently vanish. Any comment here asserting what renders is describing one
	// unmerged working tree.
	return role === "prime" || role === "pm";
}

/** A PA is defined RELATIVE TO a prime — so a PA with no effective parent is
 *  not a lesser PA, it is an instrument with no attested subject. It renders as
 *  a floating card wearing a PA chip and no visible prime, which invites the
 *  reader to supply the missing prime from context: exactly the "unexamined
 *  read as proven" shape, aimed at a human looking at a rail.
 *
 *  Every other role survives orphaning with a degraded but honest meaning — an
 *  unadopted worker has nobody to report to, which is a real and readable
 *  state. `pa` is the one role whose SUBJECT is the parent, so absence of the
 *  parent is absence of the role's referent, not a property of it.
 *
 *  Pure, and consulted at BOTH role-writing seams (`pij link --role`,
 *  `pij orchestration role set`) — the seams are separate parsers and a guard
 *  in only one is a guard that looks total. Takes the state AFTER the write
 *  would land, so it catches all three routes to a floating PA: stamping `pa`
 *  on an unadopted seat, `--root`ing a seat that already IS a `pa`, and doing
 *  both at once. Returns the refusal text, or null to permit. */
export function paLineageRefusal(
	roleAfter: StoredOrchestrationRole | undefined,
	effectiveParentAfter: SessionId | null,
	id: SessionId,
): string | null {
	if (roleAfter !== "pa" || effectiveParentAfter !== null) return null;
	return `a pa has no meaning without a prime: '${id}' would be left with no parent, rendering as a PA chip with no visible subject — link it to its prime in the same breath (pij link ${id} --parent <prime> --role pa)`;
}

export interface RoleChange {
	readonly id: SessionId;
	readonly previousRole: StoredOrchestrationRole | undefined;
	readonly role: StoredOrchestrationRole | undefined;
	readonly changed: boolean;
}

export class RoleService {
	constructor(private readonly registry: RegistryPort) {}

	set(id: SessionId, role: StoredOrchestrationRole): Result<RoleChange> {
		return this.update(id, role);
	}

	unset(id: SessionId): Result<RoleChange> {
		return this.update(id, undefined);
	}

	private update(id: SessionId, role: StoredOrchestrationRole | undefined): Result<RoleChange> {
		const descriptor = this.registry.read(id);
		if (!descriptor) return err("E-NOID", `no session '${id}' in registry`);
		// The role-write seam: every caller of `set` passes through here, so a pa
		// can never be stamped onto a seat with nobody to assist. The parent side
		// of the same invariant lives at `pij link` — a role guard alone would
		// permit orphaning an EXISTING pa, which reaches the identical floating
		// card by the other door.
		const lineage = paLineageRefusal(role, effectiveParent(descriptor), id);
		if (lineage !== null) return err("E-ARG", lineage);
		const previousRole = descriptor.orchestrationRole;
		const changed = previousRole !== role;
		// Declares "cli": RoleService OWNS orchestrationRole, so its computed value
		// must win. Without the declaration the write law would restore disk and
		// the verb would silently no-op.
		if (changed) {
			const updated = { ...descriptor, orchestrationRole: role };
			if (role === undefined) delete updated.orchestrationRole;
			this.registry.write(updated, "cli");
		}
		return ok({ id, previousRole, role, changed });
	}
}

export type DesignationAuditKind = typeof SPINE_KIND_ROLE_SET | typeof SPINE_KIND_PRIME_SET;
export type DesignationWord = OrchestrationRole | "old-prime";

export interface DesignationAuditInput {
	readonly kind: DesignationAuditKind;
	readonly id: SessionId;
	readonly prev?: DesignationWord;
	readonly next?: DesignationWord;
}

export interface DesignationAuditPort {
	append(input: DesignationAuditInput): Result<number>;
}

export interface DesignationAuditDeps {
	readonly spineLog: SpineLogPort;
	readonly platformWriteLock: PlatformWriteLockPort;
	readonly recover: () => Result<unknown>;
	readonly now: () => number;
	readonly actor: string;
}

/** Uncoupled designation history: descriptor truth lands first; this append is audit only. */
export class DesignationAuditService implements DesignationAuditPort {
	constructor(private readonly deps: DesignationAuditDeps) {}

	append(input: DesignationAuditInput): Result<number> {
		const locked = this.deps.platformWriteLock.withPlatformWriteLock((): Result<number> => {
			const recovered = this.deps.recover();
			if (!recovered.ok) return recovered;
			const draft = buildSpineEvent({
				nowMs: this.deps.now(),
				actor: this.deps.actor,
				kind: input.kind,
				refs: [`node:${input.id}`],
				peer: input.id,
				...(input.prev === undefined ? {} : { prev: input.prev }),
				...(input.next === undefined ? {} : { next: input.next }),
			});
			if (!draft.ok) return draft;
			const event = this.deps.spineLog.append(draft.value);
			return event.ok ? ok(event.value.seq) : event;
		});
		return locked.ok ? locked.value : locked;
	}
}
