// pij-orchestration — pure baton lifecycle + local hexagonal ports.

import type { ReceiptState } from "../types.js";

export type BatonErrorCode =
	| "E-ARG"
	| "E-NOBATON"
	| "E-NOREQUEST"
	| "E-NOLEASE"
	| "E-HELD"
	| "E-PIN"
	| "E-STORE";

export type BatonResult<T> =
	| { readonly ok: true; readonly value: T }
	| { readonly ok: false; readonly code: BatonErrorCode; readonly message: string };

export function batonOk<T>(value: T): BatonResult<T> {
	return { ok: true, value };
}

export function batonErr<T>(code: BatonErrorCode, message: string): BatonResult<T> {
	return { ok: false, code, message };
}

export interface BatonRequest {
	readonly id: string;
	readonly requester: string;
	readonly purpose: string;
	readonly pin?: string;
	readonly declaredEvidence?: string;
	readonly requestedAt: string;
}

export interface BatonLease {
	readonly leaseId: string;
	readonly holder: string;
	readonly purpose: string;
	readonly pin?: string;
	readonly repinAck?: true;
	readonly declaredEvidence?: string;
	readonly grantedBy: string;
	readonly requestedAt: string;
	readonly grantedAt: string;
}

export interface BatonLeaseHistory extends BatonLease {
	readonly endedAt?: string;
	readonly endKind?: "return" | "reclaim";
	readonly evidence?: string;
}

export type HolderHealth = "healthy" | "dead" | "stalled" | "unknown";

export interface BatonHolderHealth {
	readonly leaseId: string;
	readonly status: HolderHealth;
}

export interface BatonDefinition {
	readonly name: string;
	readonly resource: string;
	readonly probe?: string;
	readonly repo?: string;
	readonly createdBy: string;
	readonly createdAt: string;
	readonly queue: readonly BatonRequest[];
	readonly lastLease?: BatonLeaseHistory;
	readonly holderHealth?: BatonHolderHealth;
}

export type BatonVerb =
	| "define"
	| "list"
	| "show"
	| "request"
	| "grant"
	| "return"
	| "reclaim"
	| "alert";

export interface BatonLogEntry {
	readonly timestamp: string;
	readonly baton: string;
	readonly actor: string;
	readonly verb: BatonVerb;
	readonly leaseId?: string;
	readonly requestId?: string;
	readonly purpose?: string;
	readonly evidence?: string;
	readonly pin?: string;
	readonly previousPin?: string;
	readonly repinAck?: true;
	readonly blockedTimeMs?: number;
	readonly transition?: "dead" | "stalled";
}

export interface BatonStorePort {
	listDefinitions(): BatonResult<readonly BatonDefinition[]>;
	readDefinition(name: string): BatonResult<BatonDefinition | null>;
	writeDefinition(definition: BatonDefinition): BatonResult<void>;
	readLease(name: string): BatonResult<BatonLease | null>;
	claimLease(name: string, lease: BatonLease): BatonResult<"claimed" | "held">;
	releaseLease(name: string, leaseId: string): BatonResult<"released" | "missing" | "mismatch">;
	appendLog(entry: BatonLogEntry): BatonResult<void>;
}

export type BatonNotice =
	| {
			readonly kind: "request";
			readonly baton: string;
			readonly from: string;
			readonly to: string;
			readonly request: BatonRequest;
	  }
	| {
			readonly kind: "grant";
			readonly baton: string;
			readonly from: string;
			readonly to: string;
			readonly lease: BatonLease;
	  }
	| {
			readonly kind: "return" | "reclaim";
			readonly baton: string;
			readonly from: string;
			readonly to: string;
			readonly lease: BatonLease;
			readonly evidence?: string;
	  }
	| {
			readonly kind: "alert";
			readonly baton: string;
			readonly from: string;
			readonly to: string;
			readonly lease: BatonLease;
			readonly transition: "dead" | "stalled";
	  };

export interface BatonNoticeReceipt {
	readonly state: ReceiptState;
	readonly messageId?: string;
}

export interface BatonNoticeSink {
	push(notice: BatonNotice): BatonNoticeReceipt;
}

export function renderBatonNotice(notice: BatonNotice): string {
	switch (notice.kind) {
		case "request":
			return `[pij orchestration] baton '${notice.baton}' requested by ${notice.request.requester}: ${notice.request.purpose} (request ${notice.request.id})`;
		case "grant":
			return `[pij orchestration] baton '${notice.baton}' granted to you by ${notice.from}: ${notice.lease.purpose} (lease ${notice.lease.leaseId})`;
		case "return":
			return `[pij orchestration] baton '${notice.baton}' returned by ${notice.from}; lease ${notice.lease.leaseId} is free${notice.evidence ? ` — evidence: ${notice.evidence}` : ""}`;
		case "reclaim":
			return `[pij orchestration] baton '${notice.baton}' reclaimed by ${notice.from}; lease ${notice.lease.leaseId} is free${notice.evidence ? ` — evidence: ${notice.evidence}` : ""}`;
		case "alert":
			return `[pij orchestration] baton '${notice.baton}' holder ${notice.lease.holder} is ${notice.transition}; lease ${notice.lease.leaseId} remains held — inspect evidence before explicit reclaim`;
	}
}

export interface BatonServiceDeps {
	readonly store: BatonStorePort;
	readonly notices: BatonNoticeSink;
	readonly now: () => number;
	readonly newId: () => string;
}

export interface BatonView {
	readonly definition: BatonDefinition;
	readonly lease: BatonLease | null;
	readonly blockedTimeMs: number | null;
}

export interface GrantPlan {
	readonly definition: BatonDefinition;
	readonly lease: BatonLease;
	readonly blockedTimeMs: number;
	readonly repin?: { readonly from: string; readonly to: string };
	readonly repinAck?: true;
	readonly log: BatonLogEntry;
}

export interface ReleasePlan {
	readonly definition: BatonDefinition;
	readonly noticeTo: string;
	readonly log: BatonLogEntry;
}

export type HolderTransitionDecision =
	| { readonly kind: "none" }
	| { readonly kind: "record"; readonly health: BatonHolderHealth }
	| {
			readonly kind: "alert";
			readonly transition: "dead" | "stalled";
			readonly health: BatonHolderHealth;
	  };

export function createBaton(
	input: {
		readonly name: string;
		readonly resource: string;
		readonly probe?: string;
		readonly repo?: string;
		readonly createdBy: string;
	},
	at: string,
): BatonDefinition {
	return {
		name: input.name,
		resource: input.resource,
		...(input.probe ? { probe: input.probe } : {}),
		...(input.repo ? { repo: input.repo } : {}),
		createdBy: input.createdBy,
		createdAt: at,
		queue: [],
	};
}

export function planRequest(
	definition: BatonDefinition,
	input: {
		readonly requester: string;
		readonly purpose: string;
		readonly pin?: string;
		readonly declaredEvidence?: string;
	},
	requestId: string,
	at: string,
): BatonResult<{
	readonly definition: BatonDefinition;
	readonly request: BatonRequest;
	readonly log: BatonLogEntry;
}> {
	if (definition.queue.some((request) => request.id === requestId)) {
		return batonErr("E-ARG", `request id '${requestId}' already exists`);
	}
	const request: BatonRequest = {
		id: requestId,
		requester: input.requester,
		purpose: input.purpose,
		...(input.pin ? { pin: input.pin } : {}),
		...(input.declaredEvidence ? { declaredEvidence: input.declaredEvidence } : {}),
		requestedAt: at,
	};
	return batonOk({
		definition: { ...definition, queue: [...definition.queue, request] },
		request,
		log: {
			timestamp: at,
			baton: definition.name,
			actor: input.requester,
			verb: "request",
			requestId,
			purpose: input.purpose,
			...(input.pin ? { pin: input.pin } : {}),
			...(input.declaredEvidence ? { evidence: input.declaredEvidence } : {}),
		},
	});
}

/** Do these two commit-ish strings name the same commit? (plan 071 D5.)
 *
 *  Git identifies one commit by many strings: `d5b9b6d7` and
 *  `d5b9b6d7afe592cadbb5d115d0dc4ff37bc89c23` are the SAME commit, and string
 *  equality said otherwise — so a baton pinned with an abbreviated sha demanded
 *  `--repin` against an unmoved HEAD (observed on baton `landing-main`,
 *  request ae8ba022). A prefix comparison is exactly git's own abbreviation rule.
 *
 *  Deliberately conservative: the shorter string must be a genuine prefix of the
 *  longer one AND long enough to be a real abbreviation (git's own floor is 4),
 *  so an empty or 1-char pin never silently matches everything. */
export function sameCommit(left: string, right: string): boolean {
	const a = left.trim().toLowerCase();
	const b = right.trim().toLowerCase();
	if (a === b) return true;
	const [short, long] = a.length <= b.length ? [a, b] : [b, a];
	if (short.length < MIN_ABBREV_SHA_LEN) return false;
	if (!/^[0-9a-f]+$/.test(short) || !/^[0-9a-f]+$/.test(long)) return false;
	return long.startsWith(short);
}

/** Git's own minimum abbreviation length. Below this a "prefix match" is noise. */
const MIN_ABBREV_SHA_LEN = 4;

export function planGrant(
	definition: BatonDefinition,
	currentLease: BatonLease | null,
	input: {
		readonly requestId: string;
		readonly grantedBy: string;
		readonly currentHead: string | null;
		readonly repin?: boolean;
	},
	leaseId: string,
	at: string,
): BatonResult<GrantPlan> {
	if (currentLease) {
		return batonErr(
			"E-HELD",
			`baton '${definition.name}' is held by ${currentLease.holder} (${currentLease.leaseId})`,
		);
	}
	const request = definition.queue.find((candidate) => candidate.id === input.requestId);
	if (!request) {
		return batonErr(
			"E-NOREQUEST",
			`baton '${definition.name}' has no queued request '${input.requestId}'`,
		);
	}
	const pinUnverifiable = request.pin !== undefined && input.currentHead === null;
	const pinMoved =
		request.pin !== undefined &&
		input.currentHead !== null &&
		!sameCommit(request.pin, input.currentHead);
	if (pinUnverifiable && input.repin !== true) {
		return batonErr(
			"E-PIN",
			`baton '${definition.name}' request '${request.id}' pin unverifiable — HEAD unavailable; re-run with --repin to acknowledge`,
		);
	}
	if (pinMoved && input.repin !== true) {
		return batonErr(
			"E-PIN",
			`baton '${definition.name}' request '${request.id}' pinned ${request.pin}, current HEAD is ${input.currentHead}; re-run with --repin`,
		);
	}
	const repin =
		pinMoved && request.pin !== undefined && input.currentHead !== null
			? { from: request.pin, to: input.currentHead }
			: undefined;
	const repinAck = pinUnverifiable && input.repin === true ? true : undefined;
	const lease: BatonLease = {
		leaseId,
		holder: request.requester,
		purpose: request.purpose,
		...(repin ? { pin: repin.to } : request.pin ? { pin: request.pin } : {}),
		...(repinAck ? { repinAck } : {}),
		...(request.declaredEvidence ? { declaredEvidence: request.declaredEvidence } : {}),
		grantedBy: input.grantedBy,
		requestedAt: request.requestedAt,
		grantedAt: at,
	};
	const blocked = blockedTimeMs(lease) ?? 0;
	return batonOk({
		definition: {
			...definition,
			queue: definition.queue.filter((candidate) => candidate.id !== request.id),
			lastLease: lease,
			holderHealth: undefined,
		},
		lease,
		blockedTimeMs: blocked,
		...(repin ? { repin } : {}),
		...(repinAck ? { repinAck } : {}),
		log: {
			timestamp: at,
			baton: definition.name,
			actor: input.grantedBy,
			verb: "grant",
			leaseId,
			requestId: request.id,
			purpose: request.purpose,
			blockedTimeMs: blocked,
			...(lease.pin ? { pin: lease.pin } : {}),
			...(repin ? { previousPin: repin.from } : {}),
			...(repinAck ? { repinAck } : {}),
		},
	});
}

export function planRelease(
	definition: BatonDefinition,
	lease: BatonLease,
	input: {
		readonly kind: "return" | "reclaim";
		readonly actor: string;
		readonly evidence?: string;
	},
	at: string,
): ReleasePlan {
	const history: BatonLeaseHistory = {
		...lease,
		endedAt: at,
		endKind: input.kind,
		...(input.evidence ? { evidence: input.evidence } : {}),
	};
	return {
		definition: { ...definition, lastLease: history, holderHealth: undefined },
		noticeTo: input.kind === "return" ? lease.grantedBy : lease.holder,
		log: {
			timestamp: at,
			baton: definition.name,
			actor: input.actor,
			verb: input.kind,
			leaseId: lease.leaseId,
			purpose: lease.purpose,
			...(input.evidence ? { evidence: input.evidence } : {}),
		},
	};
}

export function blockedTimeMs(lease: BatonLease): number | null {
	const requested = Date.parse(lease.requestedAt);
	const granted = Date.parse(lease.grantedAt);
	if (Number.isNaN(requested) || Number.isNaN(granted)) return null;
	return Math.max(0, granted - requested);
}

export function planHolderTransition(
	lease: BatonLease,
	previous: BatonHolderHealth | undefined,
	current: HolderHealth,
): HolderTransitionDecision {
	if (previous?.leaseId === lease.leaseId && previous.status === current) {
		return { kind: "none" };
	}
	const health = { leaseId: lease.leaseId, status: current } as const;
	if (current === "dead" || current === "stalled") {
		return { kind: "alert", transition: current, health };
	}
	return { kind: "record", health };
}

function iso(now: () => number): string {
	return new Date(now()).toISOString();
}

function writeLog(store: BatonStorePort, entry: BatonLogEntry): BatonResult<void> {
	return store.appendLog(entry);
}

export class BatonService {
	constructor(private readonly deps: BatonServiceDeps) {}

	define(input: {
		readonly name: string;
		readonly resource: string;
		readonly probe?: string;
		readonly repo?: string;
		readonly actor: string;
	}): BatonResult<BatonDefinition> {
		const existing = this.deps.store.readDefinition(input.name);
		if (!existing.ok) return existing;
		if (existing.value) {
			return batonErr("E-ARG", `baton '${input.name}' is already defined`);
		}
		const at = iso(this.deps.now);
		const definition = createBaton({ ...input, createdBy: input.actor }, at);
		const logged = writeLog(this.deps.store, {
			timestamp: at,
			baton: input.name,
			actor: input.actor,
			verb: "define",
			purpose: input.resource,
		});
		if (!logged.ok) return logged;
		const written = this.deps.store.writeDefinition(definition);
		if (!written.ok) return written;
		return batonOk(definition);
	}

	list(actor: string): BatonResult<readonly BatonView[]> {
		const definitions = this.deps.store.listDefinitions();
		if (!definitions.ok) return definitions;
		const views: BatonView[] = [];
		for (const definition of definitions.value) {
			const lease = this.deps.store.readLease(definition.name);
			if (!lease.ok) return lease;
			views.push({
				definition,
				lease: lease.value,
				blockedTimeMs: lease.value ? blockedTimeMs(lease.value) : null,
			});
		}
		const logged = writeLog(this.deps.store, {
			timestamp: iso(this.deps.now),
			baton: "*",
			actor,
			verb: "list",
		});
		if (!logged.ok) return logged;
		return batonOk(views);
	}

	show(name: string, actor: string): BatonResult<BatonView> {
		const definition = this.requireDefinition(name);
		if (!definition.ok) return definition;
		const lease = this.deps.store.readLease(name);
		if (!lease.ok) return lease;
		const logged = writeLog(this.deps.store, {
			timestamp: iso(this.deps.now),
			baton: name,
			actor,
			verb: "show",
			...(lease.value ? { leaseId: lease.value.leaseId, purpose: lease.value.purpose } : {}),
		});
		if (!logged.ok) return logged;
		return batonOk({
			definition: definition.value,
			lease: lease.value,
			blockedTimeMs: lease.value ? blockedTimeMs(lease.value) : null,
		});
	}

	request(input: {
		readonly name: string;
		readonly requester: string;
		readonly purpose: string;
		readonly pin?: string;
		readonly declaredEvidence?: string;
	}): BatonResult<{ readonly request: BatonRequest; readonly receipt: BatonNoticeReceipt }> {
		const definition = this.requireDefinition(input.name);
		if (!definition.ok) return definition;
		const planned = planRequest(
			definition.value,
			input,
			`request-${this.deps.newId()}`,
			iso(this.deps.now),
		);
		if (!planned.ok) return planned;
		const logged = writeLog(this.deps.store, planned.value.log);
		if (!logged.ok) return logged;
		const written = this.deps.store.writeDefinition(planned.value.definition);
		if (!written.ok) return written;
		const receipt = this.deps.notices.push({
			kind: "request",
			baton: input.name,
			from: input.requester,
			to: definition.value.createdBy,
			request: planned.value.request,
		});
		return batonOk({ request: planned.value.request, receipt });
	}

	grant(input: {
		readonly name: string;
		readonly requestId: string;
		readonly grantedBy: string;
		readonly currentHead: string | null;
		readonly repin?: boolean;
	}): BatonResult<{
		readonly lease: BatonLease;
		readonly blockedTimeMs: number;
		readonly repin?: { readonly from: string; readonly to: string };
		readonly repinAck?: true;
		readonly receipt: BatonNoticeReceipt;
	}> {
		const definition = this.requireDefinition(input.name);
		if (!definition.ok) return definition;
		const currentLease = this.deps.store.readLease(input.name);
		if (!currentLease.ok) return currentLease;
		const planned = planGrant(
			definition.value,
			currentLease.value,
			input,
			`lease-${this.deps.newId()}`,
			iso(this.deps.now),
		);
		if (!planned.ok) return planned;
		const logged = writeLog(this.deps.store, planned.value.log);
		if (!logged.ok) return logged;
		const claimed = this.deps.store.claimLease(input.name, planned.value.lease);
		if (!claimed.ok) return claimed;
		if (claimed.value === "held") {
			return batonErr("E-HELD", `baton '${input.name}' was claimed by another granter`);
		}
		const written = this.deps.store.writeDefinition(planned.value.definition);
		if (!written.ok) return written;
		const receipt = this.deps.notices.push({
			kind: "grant",
			baton: input.name,
			from: input.grantedBy,
			to: planned.value.lease.holder,
			lease: planned.value.lease,
		});
		return batonOk({
			lease: planned.value.lease,
			blockedTimeMs: planned.value.blockedTimeMs,
			...(planned.value.repin ? { repin: planned.value.repin } : {}),
			...(planned.value.repinAck ? { repinAck: planned.value.repinAck } : {}),
			receipt,
		});
	}

	return(input: {
		readonly name: string;
		readonly actor: string;
		readonly evidence?: string;
	}): BatonResult<{ readonly lease: BatonLease; readonly receipt: BatonNoticeReceipt }> {
		return this.release({ ...input, kind: "return" });
	}

	reclaim(input: {
		readonly name: string;
		readonly actor: string;
		readonly evidence: string;
	}): BatonResult<{ readonly lease: BatonLease; readonly receipt: BatonNoticeReceipt }> {
		return this.release({ ...input, kind: "reclaim" });
	}

	observeHolder(
		name: string,
		current: HolderHealth,
	): BatonResult<
		| { readonly kind: "none" | "record" }
		| {
				readonly kind: "alert";
				readonly transition: "dead" | "stalled";
				readonly receipt: BatonNoticeReceipt;
		  }
	> {
		const definition = this.requireDefinition(name);
		if (!definition.ok) return definition;
		const lease = this.deps.store.readLease(name);
		if (!lease.ok) return lease;
		if (!lease.value) return batonOk({ kind: "none" });
		const decision = planHolderTransition(lease.value, definition.value.holderHealth, current);
		if (decision.kind === "none") return batonOk({ kind: "none" });
		if (decision.kind === "alert") {
			const logged = writeLog(this.deps.store, {
				timestamp: iso(this.deps.now),
				baton: name,
				actor: "pij-daemon",
				verb: "alert",
				leaseId: lease.value.leaseId,
				purpose: lease.value.purpose,
				transition: decision.transition,
			});
			if (!logged.ok) return logged;
		}
		const written = this.deps.store.writeDefinition({
			...definition.value,
			holderHealth: decision.health,
		});
		if (!written.ok) return written;
		if (decision.kind === "record") return batonOk({ kind: "record" });
		const receipt = this.deps.notices.push({
			kind: "alert",
			baton: name,
			from: "pij-orchestration",
			to: lease.value.grantedBy,
			lease: lease.value,
			transition: decision.transition,
		});
		return batonOk({ kind: "alert", transition: decision.transition, receipt });
	}

	private release(input: {
		readonly name: string;
		readonly actor: string;
		readonly evidence?: string;
		readonly kind: "return" | "reclaim";
	}): BatonResult<{ readonly lease: BatonLease; readonly receipt: BatonNoticeReceipt }> {
		const definition = this.requireDefinition(input.name);
		if (!definition.ok) return definition;
		const lease = this.deps.store.readLease(input.name);
		if (!lease.ok) return lease;
		if (!lease.value) return batonErr("E-NOLEASE", `baton '${input.name}' is not held`);
		const planned = planRelease(definition.value, lease.value, input, iso(this.deps.now));
		const logged = writeLog(this.deps.store, planned.log);
		if (!logged.ok) return logged;
		const released = this.deps.store.releaseLease(input.name, lease.value.leaseId);
		if (!released.ok) return released;
		if (released.value !== "released") {
			return batonErr("E-NOLEASE", `baton '${input.name}' lease changed before release`);
		}
		const written = this.deps.store.writeDefinition(planned.definition);
		if (!written.ok) return written;
		const receipt = this.deps.notices.push({
			kind: input.kind,
			baton: input.name,
			from: input.actor,
			to: planned.noticeTo,
			lease: lease.value,
			...(input.evidence ? { evidence: input.evidence } : {}),
		});
		return batonOk({ lease: lease.value, receipt });
	}

	private requireDefinition(name: string): BatonResult<BatonDefinition> {
		const definition = this.deps.store.readDefinition(name);
		if (!definition.ok) return definition;
		return definition.value
			? batonOk(definition.value)
			: batonErr("E-NOBATON", `no baton '${name}' is defined`);
	}
}
