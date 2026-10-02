// pij-messaging — in-memory fake adapters (Pattern P8: tests target these,
// not the wiring). Mock-free: real small implementations of every port.

import { filterEvents } from "../core/events.js";
import {
	type BatonDefinition,
	type BatonLease,
	type BatonLogEntry,
	type BatonNotice,
	type BatonNoticeReceipt,
	type BatonNoticeSink,
	type BatonResult,
	type BatonStorePort,
	batonErr,
	batonOk,
} from "../core/orchestration/baton.js";
import type {
	AssignmentStorePort,
	OpJournalPort,
	PendingOp,
	PendingOpPhase,
	PlatformWriteLockPort,
	ProjectStorePort,
	SpineAppendOnceOutcome,
	SpineLogPort,
} from "../core/platform/ports.js";
import { filterSpineEvents, type SpineEventQuery } from "../core/platform/spine.js";
import {
	type Assignment,
	isAssignment,
	isSpineEvent,
	type ProcessSnapshot,
	type Project,
	type SpineEvent,
	type SpineEventDraft,
} from "../core/platform/types.js";
import type {
	DeliveryPort,
	EventLogPort,
	InboxPort,
	NewWindowOpts,
	PiRuntimePort,
	ProcessPort,
	RegistryPort,
	SplitWindowOpts,
	TmuxPort,
} from "../core/ports.js";
import { applyWriteLaw, type DescriptorWriter } from "../core/registry-write.js";
import {
	type DeliveredMessage,
	type EventQuery,
	err,
	type InboxClaim,
	type InboxMark,
	type InboxReadMarker,
	ok,
	type PijEvent,
	type PijMessage,
	type ReceiptState,
	type Result,
	type SessionDescriptor,
	type SessionId,
} from "../core/types.js";

export class FakeBatonStore implements BatonStorePort {
	readonly definitions = new Map<string, BatonDefinition>();
	readonly leases = new Map<string, BatonLease>();
	readonly logs: BatonLogEntry[] = [];
	private readonly failures = new Map<
		"appendLog" | "writeDefinition" | "claimLease" | "releaseLease",
		number
	>();

	constructor(
		definitions: readonly BatonDefinition[] = [],
		leases: readonly (readonly [string, BatonLease])[] = [],
	) {
		for (const definition of definitions) this.definitions.set(definition.name, definition);
		for (const [name, lease] of leases) this.leases.set(name, lease);
	}

	failNext(operation: "appendLog" | "writeDefinition" | "claimLease" | "releaseLease"): void {
		this.failures.set(operation, (this.failures.get(operation) ?? 0) + 1);
	}

	private shouldFail(
		operation: "appendLog" | "writeDefinition" | "claimLease" | "releaseLease",
	): boolean {
		const remaining = this.failures.get(operation) ?? 0;
		if (remaining === 0) return false;
		if (remaining === 1) this.failures.delete(operation);
		else this.failures.set(operation, remaining - 1);
		return true;
	}

	listDefinitions(): BatonResult<readonly BatonDefinition[]> {
		return batonOk([...this.definitions.values()]);
	}

	readDefinition(name: string): BatonResult<BatonDefinition | null> {
		return batonOk(this.definitions.get(name) ?? null);
	}

	writeDefinition(definition: BatonDefinition): BatonResult<void> {
		if (this.shouldFail("writeDefinition")) {
			return batonErr("E-STORE", "injected fake definition write failure");
		}
		this.definitions.set(definition.name, definition);
		return batonOk(undefined);
	}

	readLease(name: string): BatonResult<BatonLease | null> {
		return batonOk(this.leases.get(name) ?? null);
	}

	claimLease(name: string, lease: BatonLease): BatonResult<"claimed" | "held"> {
		if (this.shouldFail("claimLease")) {
			return batonErr("E-STORE", "injected fake lease claim failure");
		}
		if (this.leases.has(name)) return batonOk("held");
		this.leases.set(name, lease);
		return batonOk("claimed");
	}

	releaseLease(name: string, leaseId: string): BatonResult<"released" | "missing" | "mismatch"> {
		if (this.shouldFail("releaseLease")) {
			return batonErr("E-STORE", "injected fake lease release failure");
		}
		const lease = this.leases.get(name);
		if (!lease) return batonOk("missing");
		if (lease.leaseId !== leaseId) return batonOk("mismatch");
		this.leases.delete(name);
		return batonOk("released");
	}

	appendLog(entry: BatonLogEntry): BatonResult<void> {
		if (this.shouldFail("appendLog")) {
			return batonErr("E-STORE", "injected fake log append failure");
		}
		this.logs.push(entry);
		return batonOk(undefined);
	}
}

export class FakeBatonNoticeSink implements BatonNoticeSink {
	readonly outbox: BatonNotice[] = [];
	private sequence = 0;

	constructor(private readonly state: ReceiptState = "delivered") {}

	push(notice: BatonNotice): BatonNoticeReceipt {
		this.sequence += 1;
		this.outbox.push(notice);
		return { state: this.state, messageId: `baton-fake-${this.sequence}` };
	}
}

export class FakeRegistry implements RegistryPort {
	private readonly map = new Map<SessionId, SessionDescriptor>();

	constructor(initial: readonly SessionDescriptor[] = []) {
		for (const d of initial) this.map.set(d.id, d);
	}

	list(): SessionDescriptor[] {
		return [...this.map.values()].filter((d) => d.lifecycle !== "dissolved");
	}
	read(id: SessionId): SessionDescriptor | null {
		return this.map.get(id) ?? null;
	}
	write(descriptor: SessionDescriptor, writer?: DescriptorWriter): void {
		const existing = this.map.get(descriptor.id);
		// Review round 2 §MED-b — the pid clause is GONE, matching FsRegistry after the
		// s066 hardening. While the fake still carried it, a DIFFERENT-pid write was
		// refused by the real registry but ACCEPTED here, so every core test could
		// resurrect a tombstone that production drops. A fake that is more permissive
		// than the real adapter is the plain-object-fake failure in another costume.
		if (
			existing?.lifecycle === "dissolved" &&
			descriptor.lifecycle !== undefined &&
			descriptor.lifecycle !== "dissolved"
		) {
			return;
		}
		// Same law as the real adapter (plan 071 review §1.2) — a fake that skipped
		// it would let a merge bug pass every unit test, which is precisely the
		// plain-object-fake failure this plan already paid for once.
		this.map.set(descriptor.id, applyWriteLaw(descriptor, existing ?? null, writer));
	}
	writeExact(descriptor: SessionDescriptor): void {
		this.map.set(descriptor.id, descriptor);
	}
	remove(id: SessionId): void {
		this.map.delete(id);
	}
	revive(descriptor: SessionDescriptor): Result<void> {
		const existing = this.map.get(descriptor.id);
		if (!existing) return err("E-NOID", `no dissolved session '${descriptor.id}' to revive`);
		if (existing.lifecycle !== "dissolved") {
			return err("E-ARG", `session '${descriptor.id}' is not dissolved`);
		}
		if (
			existing.harness !== descriptor.harness ||
			existing.harnessSessionId !== descriptor.harnessSessionId
		) {
			return err("E-AMBIG", `revive identity mismatch for '${descriptor.id}'`);
		}
		this.map.set(descriptor.id, descriptor);
		return ok(undefined);
	}

	dissolve(id: SessionId): void {
		const existing = this.map.get(id);
		if (!existing || existing.lifecycle === "dissolved") return;
		this.map.set(id, { ...existing, lifecycle: "dissolved", state: "idle" });
	}
}

export class FakeEventLog implements EventLogPort {
	private readonly events: PijEvent[] = [];
	private readonly onceKeys = new Set<string>();

	constructor(initial: readonly PijEvent[] = []) {
		this.events.push(...initial);
	}

	append(event: PijEvent): void {
		this.events.push(event);
	}
	appendOnce(key: string, event: PijEvent): "appended" | "existing" {
		if (this.onceKeys.has(key)) return "existing";
		this.onceKeys.add(key);
		this.events.push(event);
		return "appended";
	}
	read(query?: EventQuery): PijEvent[] {
		return filterEvents(this.events, query);
	}
	lastSeq(): number {
		let max = 0;
		for (const e of this.events) if (e.seq > max) max = e.seq;
		return max;
	}
	count(): number {
		return this.events.length;
	}
}

export class FakeDelivery implements DeliveryPort {
	/** Everything delivered, in order — assert against this in tests. */
	readonly outbox: Array<{ messageId: string; message: PijMessage }> = [];
	private seq = 0;

	/** Known target ids; empty set means "accept all". */
	constructor(private readonly knownIds: ReadonlySet<SessionId> = new Set()) {}

	deliver(message: PijMessage): Result<{ messageId: string }> {
		if (this.knownIds.size > 0 && !this.knownIds.has(message.to)) {
			return err("E-NOID", `no such session '${message.to}'`);
		}
		this.seq += 1;
		const messageId = `fake-${this.seq}`;
		this.outbox.push({ messageId, message });
		return ok({ messageId });
	}
}

export class FakeInbox implements InboxPort {
	readonly messages = new Map<string, DeliveredMessage>();
	readonly markers = new Map<string, InboxReadMarker>();

	constructor(initial: readonly DeliveredMessage[] = []) {
		for (const message of initial)
			this.messages.set(this.key(message.to, message.messageId), message);
	}

	listUnread(id: SessionId): Result<readonly DeliveredMessage[]> {
		const unread = [...this.messages.values()]
			.filter((message) => message.to === id && !this.markers.has(this.key(id, message.messageId)))
			.sort((a, b) => (a.messageId < b.messageId ? -1 : a.messageId > b.messageId ? 1 : 0));
		return ok(unread);
	}

	claimUnread(
		id: SessionId,
		messageId: string,
		marker: InboxReadMarker = { messageId },
	): Result<InboxClaim> {
		const key = this.key(id, messageId);
		const message = this.messages.get(key);
		if (!message) return err("E-NOREG", `no inbox message '${messageId}' for '${id}'`);
		if (this.markers.has(key)) return ok({ kind: "already-read", messageId });
		this.markers.set(key, { ...marker, messageId });
		return ok({ kind: "claimed", message });
	}

	markRead(
		id: SessionId,
		messageId: string,
		marker: InboxReadMarker = { messageId },
	): Result<InboxMark> {
		const key = this.key(id, messageId);
		if (!this.messages.has(key)) {
			return err("E-NOREG", `no inbox message '${messageId}' for '${id}'`);
		}
		if (this.markers.has(key)) return ok({ kind: "already-read", messageId });
		const persisted = { ...marker, messageId };
		this.markers.set(key, persisted);
		return ok({ kind: "marked", marker: persisted });
	}

	private key(id: SessionId, messageId: string): string {
		return `${id}\0${messageId}`;
	}
}

export class FakePiRuntime implements PiRuntimePort {
	readonly injects: Array<{ text: string; mode: "immediate" | "steer" }> = [];
	compactCount = 0;
	/** Control ops that fired (only when armed). Assert against this in tests. */
	readonly controlCalls: Array<"new" | "reload"> = [];

	constructor(
		private idle = true,
		/** When false, control() returns false (no command context armed). */
		private armed = true,
	) {}

	setIdle(idle: boolean): void {
		this.idle = idle;
	}
	setArmed(armed: boolean): void {
		this.armed = armed;
	}
	isIdle(): boolean {
		return this.idle;
	}
	inject(text: string, mode: "immediate" | "steer"): void {
		this.injects.push({ text, mode });
	}
	compact(): void {
		this.compactCount += 1;
	}
	control(command: "new" | "reload"): boolean {
		if (!this.armed) return false;
		this.controlCalls.push(command);
		return true;
	}
}

export class FakeProcess implements ProcessPort {
	private readonly alive = new Set<number>();

	constructor(
		private readonly selfPid = 1000,
		private nowMs = 0,
		private readonly vars: Record<string, string> = {},
		alivePids: readonly number[] = [1000],
	) {
		for (const p of alivePids) this.alive.add(p);
	}

	pid(): number {
		return this.selfPid;
	}
	isAlive(pid: number): boolean {
		return this.alive.has(pid);
	}
	now(): number {
		return this.nowMs;
	}
	env(key: string): string | undefined {
		return this.vars[key];
	}

	// ─── test helpers ───────────────────────────────────────────────
	advance(ms: number): void {
		this.nowMs += ms;
	}
	kill(pid: number): void {
		this.alive.delete(pid);
	}
}

export class FakeTmux implements TmuxPort {
	/** Recorded newWindow calls, in order. Assert against this in tests. */
	readonly windows: Array<{ opts: NewWindowOpts; paneId: string }> = [];
	/** Recorded splitWindow calls, in order. */
	readonly splits: Array<{ opts: SplitWindowOpts; paneId: string }> = [];
	/** Pane→window join (plan 054 P2 T006): the AC-09 addressability twin —
	 *  `select-window -t windowOf(pane)` would land on the pane's window. */
	private readonly paneWindows = new Map<string, string>();
	private windowCounter = 1;
	/** Recorded killWindow pane ids, in order. */
	readonly killed: string[] = [];
	/** Recorded killPane pane ids, in order (close() uses killPane). */
	readonly killedPanes: string[] = [];
	/** Synthetic session name returned by currentSession().
	 *  null = not inside tmux (enables E-NOTMUX unit tests — F004). */
	readonly sessionName: string | null;

	private paneCounter: number;
	private readonly curPane: string;
	/** Panes "in the current window": orchestrator pane + live split children. */
	private readonly windowPanes: Set<string>;

	constructor({
		paneStart = 900,
		sessionName = "fake-session",
		currentPane = "%500",
		windowPanes = [],
	}: {
		paneStart?: number;
		sessionName?: string | null;
		currentPane?: string;
		windowPanes?: readonly string[];
	} = {}) {
		this.paneCounter = paneStart;
		this.sessionName = sessionName;
		this.curPane = currentPane;
		this.windowPanes = new Set([currentPane, ...windowPanes]);
	}

	newWindow(opts: NewWindowOpts): Result<{ paneId: string; windowId?: string }> {
		const paneId = `%${this.paneCounter++}`;
		const windowId = `@${this.windowCounter++}`;
		this.paneWindows.set(paneId, windowId);
		this.windows.push({ opts, paneId });
		return ok({ paneId, windowId });
	}

	splitWindow(opts: SplitWindowOpts): Result<{ paneId: string; windowId?: string }> {
		const paneId = `%${this.paneCounter++}`;
		// A split lands in its TARGET pane's window; an unknown target gets a
		// fresh window id (the fake's panes may predate this join).
		const windowId = this.paneWindows.get(opts.target) ?? `@${this.windowCounter++}`;
		this.paneWindows.set(paneId, windowId);
		this.splits.push({ opts, paneId });
		this.windowPanes.add(paneId);
		return ok({ paneId, windowId });
	}

	/** The window a fake pane lives in (AC-09 proof shape for tests). */
	windowOf(paneId: string): string | undefined {
		return this.paneWindows.get(paneId);
	}

	killWindow(paneId: string): Result<void> {
		this.killed.push(paneId);
		return ok(undefined);
	}

	killPane(paneId: string): Result<void> {
		this.killedPanes.push(paneId);
		this.windowPanes.delete(paneId);
		return ok(undefined);
	}

	currentSession(): string | null {
		return this.sessionName;
	}

	currentPane(): string | null {
		return this.sessionName === null ? null : this.curPane;
	}

	currentWindowPanes(): string[] {
		return [...this.windowPanes];
	}
}

// ─── platform stores (plan 054, T008) ───────────────────────────────────────
// In-memory twins of the fs platform adapters. Records are stored and
// returned as JSON ROUND-TRIP copies (jsonClone) so callers never alias
// store state AND the copy discipline is exactly the fs adapters'
// serialize/parse: undefined-valued optional keys are dropped and non-finite
// numbers become null (structuredClone would preserve both and let a
// fake-backed test pass where the fs impl diverges). Name guards mirror the
// frozen fs adapters: E-ARG on write paths, null on read paths. Fault
// injection uses E-NOREG, the house store-fault code of the fs platform
// adapters (project-store.ts / assignment-store.ts) — E-STORE is baton-only.

/** CONSTRAINT: lockstep replica of the frozen fs name guards — the fs
 *  adapters keep SLUG_PATTERN (project-store.ts:32) and ASSIGNMENT_ID_RE
 *  (assignment-store.ts:20) module-private, so the shared alphabet is
 *  duplicated here. Keep all three in lockstep. */
const PLATFORM_NAME_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

/** Canonical fake copy discipline: a real JSON round-trip, matching what the
 *  fs adapters do to every record on its way through the disk. */
function jsonClone<T>(value: T): T {
	return JSON.parse(JSON.stringify(value)) as T;
}

export class FakeProjectStore implements ProjectStorePort {
	readonly projects = new Map<string, Project>();
	private readonly failures = new Map<"create" | "update", number>();

	constructor(initial: readonly Project[] = []) {
		for (const project of initial) this.projects.set(project.slug, jsonClone(project));
	}

	failNext(operation: "create" | "update"): void {
		this.failures.set(operation, (this.failures.get(operation) ?? 0) + 1);
	}

	private shouldFail(operation: "create" | "update"): boolean {
		const remaining = this.failures.get(operation) ?? 0;
		if (remaining === 0) return false;
		if (remaining === 1) this.failures.delete(operation);
		else this.failures.set(operation, remaining - 1);
		return true;
	}

	create(project: Project): Result<"claimed" | "exists"> {
		// fs parity: the name guard precedes any store activity (project-store.ts).
		if (!PLATFORM_NAME_PATTERN.test(project.slug)) {
			return err(
				"E-ARG",
				`invalid project slug '${project.slug}' (use letters, digits, dot, underscore, or hyphen)`,
			);
		}
		if (this.shouldFail("create")) {
			return err("E-NOREG", "injected fake project create failure");
		}
		// First-writer-wins (publishNoReplace semantics): the original record survives.
		if (this.projects.has(project.slug)) return ok("exists");
		this.projects.set(project.slug, jsonClone(project));
		return ok("claimed");
	}

	update(project: Project): Result<void> {
		if (!PLATFORM_NAME_PATTERN.test(project.slug)) {
			return err(
				"E-ARG",
				`invalid project slug '${project.slug}' (use letters, digits, dot, underscore, or hyphen)`,
			);
		}
		if (this.shouldFail("update")) {
			return err("E-NOREG", "injected fake project update failure");
		}
		if (!this.projects.has(project.slug)) {
			return err("E-NOREG", `no project '${project.slug}' — create it first`);
		}
		this.projects.set(project.slug, jsonClone(project));
		return ok(undefined);
	}

	read(slug: string): Project | null {
		if (!PLATFORM_NAME_PATTERN.test(slug)) return null;
		const project = this.projects.get(slug);
		return project === undefined ? null : jsonClone(project);
	}

	list(): Project[] {
		return [...this.projects.values()]
			.map((project) => jsonClone(project))
			.sort((left, right) => left.slug.localeCompare(right.slug));
	}
}

export class FakeAssignmentStore implements AssignmentStorePort {
	readonly assignments = new Map<string, Assignment>();
	private readonly failures = new Map<"write", number>();

	constructor(initial: readonly Assignment[] = []) {
		for (const assignment of initial) {
			this.assignments.set(assignment.id, jsonClone(assignment));
		}
	}

	failNext(operation: "write"): void {
		this.failures.set(operation, (this.failures.get(operation) ?? 0) + 1);
	}

	private shouldFail(operation: "write"): boolean {
		const remaining = this.failures.get(operation) ?? 0;
		if (remaining === 0) return false;
		if (remaining === 1) this.failures.delete(operation);
		else this.failures.set(operation, remaining - 1);
		return true;
	}

	write(assignment: Assignment): Result<void> {
		// fs parity: the id guard precedes any store activity (assignment-store.ts).
		if (!PLATFORM_NAME_PATTERN.test(assignment.id)) {
			return err(
				"E-ARG",
				`invalid assignment id '${assignment.id}' (use letters, digits, dot, underscore, or hyphen)`,
			);
		}
		// Review 001 F5: a type-valid record the JSON round-trip poisons
		// (states: [NaN] → null) dies HERE, identically to the fs write boundary
		// — never stored to be silently lost on read.
		const clone = jsonClone(assignment);
		if (!isAssignment(clone)) {
			return err(
				"E-ARG",
				`assignment '${assignment.id}' fails the record contract after JSON round-trip (non-finite state ref?)`,
			);
		}
		if (this.shouldFail("write")) {
			return err("E-NOREG", "injected fake assignment write failure");
		}
		this.assignments.set(assignment.id, clone);
		return ok(undefined);
	}

	read(id: string): Assignment | null {
		if (!PLATFORM_NAME_PATTERN.test(id)) return null;
		const assignment = this.assignments.get(id);
		if (assignment === undefined) return null;
		// fs read parity (review 001 F5): guard AFTER the round-trip copy, so a
		// constructor-seeded poisoned record reads null like corrupt disk bytes.
		const clone = jsonClone(assignment);
		return isAssignment(clone) ? clone : null;
	}

	list(): Assignment[] {
		return [...this.assignments.values()]
			.map((assignment) => jsonClone(assignment))
			.filter((assignment) => isAssignment(assignment))
			.sort((left, right) => left.id.localeCompare(right.id));
	}

	listByNode(nodeId: string): Assignment[] {
		return this.list().filter((assignment) => assignment.nodeId === nodeId);
	}
}

export class FakeSpineLog implements SpineLogPort {
	private readonly events: SpineEvent[] = [];
	private readonly onceEvents = new Map<string, SpineEvent>();
	private readonly failures = new Map<"append" | "appendOnce", number>();

	constructor(initial: readonly SpineEvent[] = []) {
		for (const event of initial) this.push(event);
	}

	failNext(operation: "append" | "appendOnce"): void {
		this.failures.set(operation, (this.failures.get(operation) ?? 0) + 1);
	}

	private shouldFail(operation: "append" | "appendOnce"): boolean {
		const remaining = this.failures.get(operation) ?? 0;
		if (remaining === 0) return false;
		if (remaining === 1) this.failures.delete(operation);
		else this.failures.set(operation, remaining - 1);
		return true;
	}

	append(draft: SpineEventDraft): Result<SpineEvent> {
		// The newline-guard is fs-only torn-tail hygiene (spine-store.ts); the
		// in-memory log has no torn tails. Seq allocation lives INSIDE the
		// operation (review 001 F1): stamped here, never by callers.
		if (this.shouldFail("append")) {
			return err("E-NOREG", "injected fake spine append failure");
		}
		const stamped = this.stamp(draft);
		this.push(stamped);
		return ok(stamped);
	}

	appendOnce(key: string, draft: SpineEventDraft): Result<SpineAppendOnceOutcome> {
		// Durable idempotence: replay returns the ORIGINALLY stamped event.
		if (this.shouldFail("appendOnce")) {
			return err("E-NOREG", "injected fake spine appendOnce failure");
		}
		const prior = this.onceEvents.get(key);
		if (prior !== undefined) return ok({ outcome: "existing", event: jsonClone(prior) });
		const stamped = this.stamp(draft);
		this.onceEvents.set(key, jsonClone(stamped));
		this.push(stamped);
		return ok({ outcome: "appended", event: stamped });
	}

	hasOnce(key: string): boolean {
		// fs parity (review 003 H1): pure existence of the durable once-record.
		return this.onceEvents.has(key);
	}

	/** fs parity: seq = readAll-max + 1, exactly the allocation the fs adapter
	 *  performs under its lock, over a JSON round-trip of the draft — so
	 *  undefined-valued optional keys drop as serialization would drop them
	 *  and the returned event never aliases caller-held draft state. */
	private stamp(draft: SpineEventDraft): SpineEvent {
		return { ...jsonClone(draft), seq: this.lastSeq() + 1 };
	}

	/** fs write→read parity: serialize, re-parse, and guard. A record the fs
	 *  adapter would drop on read (e.g. a non-finite seq — JSON.stringify turns
	 *  it into null, failing isSpineEvent) never enters the fake log either,
	 *  so it is equally invisible to read() and lastSeq(). */
	private push(event: SpineEvent): void {
		const stored = jsonClone(event);
		if (isSpineEvent(stored)) this.events.push(stored);
	}

	lastSeq(): number {
		let max = 0;
		for (const e of this.events) if (e.seq > max) max = e.seq;
		return max;
	}

	read(query?: SpineEventQuery): SpineEvent[] {
		// fs parity: seq-ascending merge, then the real core filter.
		const ascending = [...this.events].sort((left, right) => left.seq - right.seq);
		return filterSpineEvents(ascending, query).map((event) => jsonClone(event));
	}
}

/** In-memory journal entry: the durable fields of the fs layout minus the
 *  envelope (schema_version/opId live in the map key + record on disk). */
interface FakeJournalEntry {
	readonly order: number;
	readonly phase: PendingOpPhase;
	readonly draft: SpineEventDraft;
}

export class FakeOpJournal implements OpJournalPort {
	readonly ops = new Map<string, FakeJournalEntry>();
	private opCounter = 0;
	private readonly failures = new Map<"record" | "markCommitted" | "clear", number>();

	failNext(operation: "record" | "markCommitted" | "clear"): void {
		this.failures.set(operation, (this.failures.get(operation) ?? 0) + 1);
	}

	private shouldFail(operation: "record" | "markCommitted" | "clear"): boolean {
		const remaining = this.failures.get(operation) ?? 0;
		if (remaining === 0) return false;
		if (remaining === 1) this.failures.delete(operation);
		else this.failures.set(operation, remaining - 1);
		return true;
	}

	record(draft: SpineEventDraft): Result<string> {
		// fs parity (op-journal.ts): pending()'s read-back probe runs at record
		// time too — a draft the replay pass would silently skip is refused
		// BEFORE it can strand a committed state write unaudited (audit F2).
		if (!isSpineEvent({ ...draft, seq: 1 })) {
			return err("E-ARG", "invalid spine event draft — refusing to journal");
		}
		if (this.shouldFail("record")) {
			return err("E-NOREG", "injected fake op-journal record failure");
		}
		// Deterministic ids; a failed record burns nothing (FakeSpineLog parity).
		this.opCounter += 1;
		const opId = `op-${this.opCounter}`;
		// fs parity (review 002 G3): order = max over surviving entries + 1 —
		// coexisting entries were recorded by concurrent writers (the recovery
		// gate empties the journal before any sequential successor records), so
		// the max-pending rule yields a durable causality-respecting order.
		this.ops.set(opId, { order: this.nextOrder(), phase: "intent", draft: jsonClone(draft) });
		return ok(opId);
	}

	private nextOrder(): number {
		let max = 0;
		for (const entry of this.ops.values()) if (entry.order > max) max = entry.order;
		return max + 1;
	}

	markCommitted(opId: string): Result<void> {
		if (this.shouldFail("markCommitted")) {
			return err("E-NOREG", "injected fake op-journal markCommitted failure");
		}
		const entry = this.ops.get(opId);
		if (entry === undefined) {
			return err("E-NOREG", `no journaled op '${opId}' to mark committed`);
		}
		this.ops.set(opId, { ...entry, phase: "committed" });
		return ok(undefined);
	}

	clear(opId: string): Result<void> {
		// One-shot LOST clear (audit F2 / review 003 M3): the entry SURVIVES
		// and the failure is reported honestly — recovery must stop on it, and
		// the "verb succeeded, clear lost" crash window stays drivable
		// end-to-end through the CLI.
		if (this.shouldFail("clear")) {
			return err("E-NOREG", "injected fake op-journal clear failure");
		}
		this.ops.delete(opId);
		return ok(undefined);
	}

	pending(): Result<readonly PendingOp[]> {
		// Port contract (review 002 G3): durable order ascending, opId tiebreak —
		// never opId-lexical alone (op-10 must not sort before op-2's successor).
		// The in-memory map cannot hold a corrupt entry, so enumeration is
		// always ok — the H2 unreadable-entry law is an fs-only failure mode
		// pinned in op-journal.test.ts.
		return ok(
			[...this.ops.entries()]
				.map(([opId, entry]) => ({
					opId,
					order: entry.order,
					phase: entry.phase,
					draft: jsonClone(entry.draft),
				}))
				.sort((left, right) =>
					left.order !== right.order
						? left.order - right.order
						: left.opId < right.opId
							? -1
							: left.opId > right.opId
								? 1
								: 0,
				),
		);
	}
}

/** One machine home's write-lock state, shared by every handle that models
 *  that home (review 003 M5 — fs parity: all FsPlatformWriteLock instances
 *  over one pijHome contend on the single spine/write.lock file). */
interface FakeWriteLockMachine {
	held: boolean;
}

/** In-process contract twin of FsPlatformWriteLock (review 002 G2/G3 +
 *  review 003 M5): held-state semantics, NON-reentrant, machine-wide via the
 *  shared backing — a nested or contended acquisition fails E-NOREG without
 *  running its operation, exactly like the fs lock's budget timeout, so
 *  downstream tests can never admit an interleaving production serialization
 *  forbids. Throws from the operation propagate (fs parity: the dispatch
 *  containment gate owns them), the lock releasing either way. */
export class FakePlatformWriteLock implements PlatformWriteLockPort {
	/** Completed acquisitions — every platform WRITE verb must take exactly one. */
	acquisitions = 0;
	private failures = 0;
	private readonly machine: FakeWriteLockMachine;

	constructor(machine: FakeWriteLockMachine = { held: false }) {
		this.machine = machine;
	}

	/** A second handle onto the SAME machine home's lock (fs parity: another
	 *  FsPlatformWriteLock constructed over the same pijHome). */
	fork(): FakePlatformWriteLock {
		return new FakePlatformWriteLock(this.machine);
	}

	failNext(): void {
		this.failures += 1;
	}

	withPlatformWriteLock<T>(operation: () => T): Result<T> {
		if (this.failures > 0) {
			this.failures -= 1;
			return err("E-NOREG", "injected fake platform write-lock acquisition failure");
		}
		// Held-state check (review 003 M5): the fs lock times out E-NOREG on a
		// nested or contended acquisition — the single-threaded fake fails
		// FAST instead of burning a retry budget, same verdict, same shape.
		if (this.machine.held) {
			return err(
				"E-NOREG",
				"platform write lock is held — locks are never stolen; release the holder first",
			);
		}
		this.machine.held = true;
		this.acquisitions += 1;
		try {
			return ok(operation());
		} finally {
			// fs parity: released on ok AND on a propagating throw.
			this.machine.held = false;
		}
	}
}

// ─── process snapshots (plan 095 T-1.6) ────────────────────────────────────

/** One constructible process row. `ppid` defaults to 1 (a root), so a fixture
 *  only states the parent links it actually cares about. */
export interface FakeProcessRow {
	readonly pid: number;
	readonly ppid?: number;
	readonly command: string;
	readonly startedAtMs?: number;
	readonly truncated?: boolean;
}

/** Build a deterministic {@link ProcessSnapshot} for the liveness ladder.
 *
 *  The whole point of classifying in pure core code is that every case the real
 *  world produced — an agent at depth 0, an agent at depth 1, a bare `-zsh`, a
 *  recycled pid holding an unrelated system daemon, another seat's `--session-id`, a
 *  truncated command line — is a TABLE ROW here rather than a live-process
 *  experiment nobody can re-run. */
export function fakeProcessSnapshot(
	rows: readonly FakeProcessRow[],
	capturedAtMs = 0,
): ProcessSnapshot {
	return {
		ok: true,
		capturedAtMs,
		processes: rows.map((row) => ({
			pid: row.pid,
			ppid: row.ppid ?? 1,
			command: row.command,
			...(row.startedAtMs !== undefined ? { startedAtMs: row.startedAtMs } : {}),
			...(row.truncated !== undefined ? { truncated: row.truncated } : {}),
		})),
	};
}

/** A capture that FAILED — distinct from an empty table, and the only correct
 *  input for "we could not look". */
export function fakeProcessSnapshotUnavailable(reason = "ps unavailable"): ProcessSnapshot {
	return { ok: false, reason };
}
