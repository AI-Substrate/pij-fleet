import { createHash, randomUUID } from "node:crypto";
import { chmod, lstat, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

export const INITIAL_RETRY_MS = 250;
export const MAX_RETRY_MS = 5000;
export const HOLD_ESCALATION_MS = 600000;
export const HOLD_ESCALATED_RETRY_MS = 60000;
export const NATIVE_RPC_DEADLINE_MS = 15000;
export const RECEIVER_EMPTY_READ_LIMIT = 3;
export const RECEIVER_PROBE_INTERVAL_MS = 1000;
/** A wedged daemon holds neither a typed prompt (held FYIs, plan 158) nor shutdown. */
export const HOOK_REQUEST_TIMEOUT_MS = 3000;
/** Opens every cold-wake refusal's meta (crates/core/src/cold_wake.rs, plan 157 phase 2). */
const COLD_WAKE_CODE = "E-RS-COLD-WAKE";
const ERROR_BODY_LIMIT = 4096;
const hash = (value) => createHash("sha256").update(value).digest("hex");
const nonempty = (value) => typeof value === "string" && value.trim().length > 0;
const positiveInteger = (value) => Number.isSafeInteger(value) && value > 0;
const sameProcess = (seat, host) =>
	seat.proc?.pid === host.pid && seat.proc?.proc_start === host.proc_start;
const samePane = (seat, host) => (seat.pane ?? undefined) === (host.pane ?? undefined);
const nativeSession = (seat) =>
	seat.harness === "copilot" &&
	nonempty(seat.session) &&
	positiveInteger(seat.proc?.pid) &&
	positiveInteger(seat.proc?.proc_start);
const nativeAttested = (seat) =>
	nativeSession(seat) && seat.native_extension_delivery === true && seat.tombstoned_at == null;

export class NativeError extends Error {
	constructor(message, retryable = false, safeDiagnostic = message, holdKind = undefined) {
		super(message);
		this.name = "NativeError";
		this.retryable = retryable;
		// Local NativeError text is authored here; remote/OS text must supply a safe projection.
		this.safeDiagnostic = safeDiagnostic;
		if (retryable && holdKind === "native-session") this.holdKind = holdKind;
	}
}

/** Normalize the Linux kernel's unlinked executable marker before identity matching. */
export function normalizeExecutable(command) {
	const suffix = " (deleted)";
	const replaced = command.endsWith(suffix);
	return { command: replaced ? command.slice(0, -suffix.length) : command, replaced };
}

/** The daemon packs C-locale, local-time ps lstart as YYYYMMDDhhmmss. */
export function parseProcessStart(row) {
	const match =
		/^(?:Mon|Tue|Wed|Thu|Fri|Sat|Sun) ([A-Z][a-z]{2})\s+(\d{1,2}) (\d{2}):(\d{2}):(\d{2}) (\d{4})$/.exec(
			row.trim(),
		);
	if (!match) throw new NativeError("Could not parse host process start");
	const month =
		["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].indexOf(
			match[1],
		) + 1;
	const [day, hour, minute, second, year] = match.slice(2).map(Number);
	if (
		!month ||
		year < 1970 ||
		day < 1 ||
		day > new Date(Date.UTC(year, month, 0)).getUTCDate() ||
		hour > 23 ||
		minute > 59 ||
		second > 59
	)
		throw new NativeError("Invalid host process start calendar fields");
	return (
		year * 10000000000 + month * 100000000 + day * 1000000 + hour * 10000 + minute * 100 + second
	);
}

/** Native runtime identity is authoritative; launch environment is only corroborated intent. */
export function chooseRegistration({ sessionId, host, folder, seats, env }) {
	if (
		!nonempty(sessionId) ||
		!positiveInteger(host.pid) ||
		!positiveInteger(host.proc_start) ||
		!nonempty(folder) ||
		!Array.isArray(seats)
	)
		throw new NativeError("Invalid native host identity or roster");
	const live = seats.filter(
		(seat) => seat.tombstoned_at == null && !["dead", "tombstoned"].includes(seat.state),
	);
	const requestedId = env.PIJ_SESSION_ID?.trim();
	const requestedSpawn = env.PIJ_SPAWN_ID?.trim();
	const prebind =
		requestedId && requestedSpawn && host.pane
			? seats.find(
					(seat) =>
						seat.id === requestedId &&
						seat.harness === "copilot" &&
						!seat.relay &&
						seat.tombstoned_at == null &&
						seat.proc == null &&
						(seat.session == null || seat.session === sessionId) &&
						samePane(seat, host) &&
						seat.spawn_id === requestedSpawn,
				)
			: undefined;
	const candidates = seats.filter(
		(seat) => seat !== prebind && seat.harness === "copilot" && seat.session === sessionId,
	);
	if (candidates.filter((seat) => seat.tombstoned_at == null).length > 1)
		throw new NativeError(
			"Native session has multiple candidate Pij addresses; refreshing roster",
			true,
		);
	candidates.sort(
		(a, b) =>
			Number(a.tombstoned_at != null) - Number(b.tombstoned_at != null) ||
			(b.proc?.proc_start ?? -Infinity) - (a.proc?.proc_start ?? -Infinity) ||
			(a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
	);
	const exact = live.filter((seat) => sameProcess(seat, host));
	if (
		exact.length > 1 ||
		exact.some(
			(seat) => seat.harness !== "copilot" || (!samePane(seat, host) && seat !== candidates[0]),
		)
	)
		throw new NativeError("Native process identity has conflicting seats or pane");
	if (
		host.pane &&
		live.some(
			(seat) =>
				seat !== prebind &&
				seat !== candidates[0] &&
				seat.pane === host.pane &&
				!sameProcess(seat, host) &&
				!nativeAttested(seat),
		)
	)
		throw new NativeError("Pane belongs to a different process identity");
	// A verified spawn allocation owns its id and parent; otherwise the session owns the address.
	const prior = prebind ?? candidates[0] ?? exact[0];
	// A same-process session rollover keeps its original launch environment on extension reload.
	const correlatedSuccessor =
		prior?.native_extension_delivery === true &&
		requestedSpawn &&
		prior.spawn_id === requestedSpawn &&
		sameProcess(prior, host);
	if (requestedId && (!prior || prior.id !== requestedId) && !correlatedSuccessor)
		throw new NativeError("Spawn prebind identity is not corroborated by the host process", !prior);
	if (requestedSpawn && (!prior || prior.spawn_id !== requestedSpawn))
		throw new NativeError("Spawn correlation does not match observed process identity");
	const replacing = prior && nonempty(prior.session) && prior.session !== sessionId;
	// New identities are allocated by the daemon's normal memorable-name allocator.
	// Existing addresses, including older hash names, remain stable on resume.
	const id = !replacing && prior ? prior.id : "";
	const spawnId = prior?.spawn_id;
	return Object.freeze({
		id,
		harness: "copilot",
		harness_session: sessionId,
		folder,
		pid: host.pid,
		proc_start: host.proc_start,
		relay: false,
		native_extension_delivery: true,
		...(host.pane ? { pane: host.pane } : {}),
		...(replacing ? { supersedes: prior.id } : {}),
		...(spawnId ? { spawn_id: spawnId } : {}),
		...(prior?.parent ? { parent: prior.parent } : {}),
	});
}

function verifyRegisteredSeat(seat, registration) {
	if (
		!nonempty(seat?.id) ||
		(registration.id !== "" && seat.id !== registration.id) ||
		seat.harness !== "copilot" ||
		seat.session !== registration.harness_session ||
		!sameProcess(seat, registration) ||
		!samePane(seat, registration) ||
		seat.native_extension_delivery !== true
	)
		throw new NativeError(
			"Daemon registration identity/capability readback mismatch; native receiving refused",
		);
}

/** Resolve a new address before creating its durable journal namespace. */
export async function resolveRegistration(client, registration, signal) {
	if (registration.id !== "") return registration;
	const seat = await client.request("/v1/register", registration, signal);
	throwIfStopped(signal);
	verifyRegisteredSeat(seat, registration);
	const bound = { ...registration, id: seat.id };
	delete bound.supersedes;
	return Object.freeze(bound);
}

class InboxWaitDeadline extends Error {}

export class DaemonClient {
	constructor({
		addr,
		stateDir,
		fetch: fetcher = globalThis.fetch,
		readKey = () => readFile(join(stateDir, "daemon.key"), "utf8"),
		requestTimeoutMs = 65000,
	}) {
		if (!/^(?:[a-zA-Z0-9_.-]+|\[[a-fA-F0-9:]+\]):\d+$/.test(addr))
			throw new NativeError("PIJ_RS_ADDR must be host:port, without a URL path");
		const url = new URL(`http://${addr}`);
		const port = Number(addr.slice(addr.lastIndexOf(":") + 1));
		if (!Number.isInteger(port) || port < 1 || port > 65535)
			throw new NativeError("PIJ_RS_ADDR requires a valid explicit port");
		this.url = url.origin;
		this.fetch = fetcher;
		this.readKey = readKey;
		this.requestTimeoutMs = requestTimeoutMs;
	}

	async request(path, body, signal) {
		return (await this.#requestEnvelope(path, body, signal)).data;
	}

	/** @returns {Promise<{claims: unknown[], hold: {kind: "consent" | "native-target", reason: string} | null}>} */
	async claimInbox(tuple, signal) {
		const query = new URLSearchParams({ ...tuple, wait: "true" });
		const envelope = await this.#requestEnvelope(`/v1/inbox?${query}`, undefined, signal, true);
		if (envelope.meta != null && typeof envelope.meta !== "string")
			throw new NativeError(
				"Malformed native inbox hold metadata; receiving held without acknowledgement",
			);
		const prefix = "native-consumer-held:";
		const reason = envelope.meta?.startsWith(prefix) ? envelope.meta.slice(prefix.length) : null;
		return {
			claims: envelope.data,
			hold:
				reason === null
					? null
					: {
							kind: reason.startsWith("native-target-session:") ? "native-target" : "consent",
							reason,
						},
		};
	}

	async nativeSnapshot(tuple, signal) {
		const query = new URLSearchParams(tuple);
		// Revalidate the native incarnation; typing remains informational.
		const snapshot = await this.request(`/v1/inbox/typing?${query}`, undefined, signal);
		const proof = snapshot?.native_consumer;
		if (
			!proof ||
			proof.native_session !== tuple.native_session ||
			proof.pid !== tuple.pid ||
			proof.proc_start !== tuple.proc_start
		)
			throw new NativeError(
				"Native identity observation unavailable or invalid; receiving stopped without injection",
			);
		return snapshot;
	}

	async #requestEnvelope(path, body, signal, inboxWait = false) {
		let key;
		try {
			key = (await this.readKey()).trim();
		} catch {
			throw new NativeError(
				"Pij daemon.key unavailable; check PIJ_RS_STATE_DIR and daemon readiness",
				true,
			);
		}
		if (!key) throw new NativeError("Pij daemon.key is empty", true);
		const timeout = AbortSignal.timeout(this.requestTimeoutMs);
		const requestSignal = signal ? AbortSignal.any([signal, timeout]) : timeout;
		let response;
		let raw;
		try {
			response = await this.fetch(`${this.url}${path}`, {
				method: body === undefined ? "GET" : "POST",
				headers: {
					Authorization: `Bearer ${key}`,
					...(body === undefined ? {} : { "Content-Type": "application/json" }),
				},
				...(body === undefined ? {} : { body: JSON.stringify(body) }),
				signal: requestSignal,
			});
			raw = await response.text();
		} catch (error) {
			if (signal?.aborted) throw signal.reason;
			// Only our unanswered long-poll deadline renews; body/read and foreign errors still fail.
			if (inboxWait && !response && timeout.aborted && error === timeout.reason)
				throw new InboxWaitDeadline();
			throw new NativeError(
				`Pij transport unavailable (${error?.name ?? "network error"}); retrying with refreshed credentials`,
				true,
				"Pij transport unavailable",
			);
		}
		let envelope;
		try {
			envelope = JSON.parse(raw);
		} catch {
			/* Preserve the real HTTP diagnostic below. */
		}
		if (!response.ok || envelope?.ok !== true || envelope.v !== 2 || !("data" in envelope)) {
			const diagnostic = raw.replaceAll(key, "[redacted]").slice(0, ERROR_BODY_LIMIT);
			const nativeUnavailable =
				(path.startsWith("/v1/inbox?") ||
					path.startsWith("/v1/inbox/typing?") ||
					path === "/v1/inbox/ack") &&
				response.status === 400 &&
				envelope?.ok === false &&
				envelope.v === 2 &&
				envelope.command === "pij inbox" &&
				envelope.error === "refused" &&
				envelope.meta ===
					"daemon/native-inbox: native-extension-unavailable: Copilot requires current native registration";
			// Only this typed registration refusal may project remote fields into the timeline.
			// Everything else retains its bounded raw diagnostic outside the native banner.
			const hostRefusal =
				path === "/v1/register" &&
				envelope?.ok === false &&
				envelope.v === 2 &&
				envelope.command === "pij register" &&
				envelope.error === "refused" &&
				typeof envelope.meta === "string" &&
				/^native Copilot registration refused: claimed pid is not an actual Copilot host executable; observed basename="[^"\\/\r\n]{1,255}", replaced=(?:true|false)$/.test(
					envelope.meta,
				);
			const registrationRetry =
				path === "/v1/register" &&
				response.status === 409 &&
				envelope?.v === 2 &&
				envelope.ok === false &&
				envelope.command === "pij register" &&
				envelope.error === "refused" &&
				envelope.details?.retryable === true;
			// Plan 157 phase 2: the cold-wake refusal names the price and both ways
			// forward, so its meta is the model's whole answer, verbatim.
			const coldWake =
				path === "/v1/send" &&
				response.status === 400 &&
				envelope?.ok === false &&
				envelope.v === 2 &&
				envelope.error === "refused" &&
				typeof envelope.meta === "string" &&
				envelope.meta.startsWith(`${COLD_WAKE_CODE}:`);
			const projected = hostRefusal || coldWake ? envelope.meta.replaceAll(key, "[redacted]") : "";
			const error = new NativeError(
				coldWake ? projected : `Pij HTTP ${response.status}: ${diagnostic}`,
				nativeUnavailable ||
					registrationRetry ||
					response.status === 401 ||
					response.status === 429 ||
					response.status >= 500,
				hostRefusal || coldWake ? projected : `Pij HTTP ${response.status}`,
				registrationRetry && envelope.details.hold === "native-session"
					? "native-session"
					: undefined,
			);
			error.status = response.status;
			if (coldWake) error.coldWake = true;
			throw error;
		}
		return envelope;
	}
}

async function privateDirectory(path) {
	await mkdir(path, { recursive: true, mode: 0o700 });
	const info = await lstat(path);
	if (
		!info.isDirectory() ||
		info.isSymbolicLink() ||
		(process.getuid && info.uid !== process.getuid())
	)
		throw new NativeError(
			"Acceptance directory must be owned by the current user and not a symlink",
		);
	await chmod(path, 0o700);
}

async function syncDirectory(path) {
	const handle = await open(path, "r");
	try {
		await handle.sync();
	} finally {
		await handle.close();
	}
}

function validRecord(record, msgId) {
	return (
		record?.message?.msg_id === msgId &&
		nonempty(record.message.from) &&
		nonempty(record.message.to) &&
		typeof record.message.body === "string" &&
		(record.state === "pending" || (record.state === "accepted" && nonempty(record.nativeId)))
	);
}

/** Each exclusive intent file is also an inter-process duplicate barrier. Nothing is evicted. */
export class FileJournal {
	constructor(stateDir, registration) {
		this.root = join(stateDir, "native-extensions");
		this.base = join(this.root, "copilot");
		// Acceptance belongs to the conversation, not the live process resuming it.
		this.directory = join(
			this.base,
			hash(JSON.stringify([registration.id, registration.harness_session])),
		);
	}
	path(msgId) {
		return join(this.directory, `${hash(msgId)}.json`);
	}
	async prepare() {
		await privateDirectory(this.root);
		await privateDirectory(this.base);
		await privateDirectory(this.directory);
	}
	async load(msgId) {
		await this.prepare();
		const path = this.path(msgId);
		try {
			const info = await lstat(path);
			if (
				!info.isFile() ||
				info.isSymbolicLink() ||
				(info.mode & 0o077) !== 0 ||
				(process.getuid && info.uid !== process.getuid())
			)
				throw new NativeError("Unsafe acceptance record permissions or file type");
			const record = JSON.parse(await readFile(path, "utf8"));
			if (!validRecord(record, msgId))
				throw new NativeError("Malformed acceptance record; receive held for recovery");
			return record;
		} catch (error) {
			if (error.code === "ENOENT") return undefined;
			throw error;
		}
	}
	async begin(message) {
		await this.prepare();
		let handle;
		try {
			handle = await open(this.path(message.msg_id), "wx", 0o600);
		} catch (error) {
			if (error.code === "EEXIST") return false;
			throw error;
		}
		try {
			await handle.writeFile(JSON.stringify({ state: "pending", message }));
			await handle.sync();
		} finally {
			await handle.close();
		}
		await syncDirectory(this.directory);
		return true;
	}
	async rearm(message, nativeId) {
		const record = await this.load(message.msg_id);
		if (
			record?.state !== "accepted" ||
			record.nativeId !== nativeId ||
			!sameMessage(record.message, message)
		)
			return false;
		// A lease can expire while the old consumer is still alive. Only one may retire this acceptance.
		let handle;
		try {
			handle = await open(`${this.path(message.msg_id)}.${hash(nativeId)}.retry`, "wx", 0o600);
		} catch (error) {
			if (error.code === "EEXIST") return false;
			throw error;
		}
		try {
			await handle.writeFile(JSON.stringify({ message, nativeId }));
			await handle.sync();
		} finally {
			await handle.close();
		}
		await syncDirectory(this.directory);
		const current = await this.load(message.msg_id);
		if (
			current?.state !== "accepted" ||
			current.nativeId !== nativeId ||
			!sameMessage(current.message, message)
		)
			return false;
		await this.#replace({ state: "pending", message });
		return true;
	}
	async accept(message, nativeId) {
		if (!nonempty(nativeId)) throw new NativeError("Native acceptance did not supply a message ID");
		await this.#replace({ state: "accepted", message, nativeId });
	}
	async #replace(record) {
		const target = this.path(record.message.msg_id);
		const temporary = `${target}.${randomUUID()}.tmp`;
		const handle = await open(temporary, "wx", 0o600);
		try {
			await handle.writeFile(JSON.stringify(record));
			await handle.sync();
		} finally {
			await handle.close();
		}
		try {
			await rename(temporary, target);
			await syncDirectory(this.directory);
		} finally {
			await unlink(temporary).catch((error) => {
				if (error.code !== "ENOENT") throw error;
			});
		}
	}
}

function throwIfStopped(signal) {
	signal.throwIfAborted();
}

/** Cancel the wait, not an already-issued native send. Its durable intent remains ambiguous. */
export function abortable(promise, signal) {
	throwIfStopped(signal);
	return new Promise((resolve, reject) => {
		const abort = () => reject(signal.reason);
		signal.addEventListener("abort", abort, { once: true });
		Promise.resolve(promise)
			.then(resolve, reject)
			.finally(() => signal.removeEventListener("abort", abort));
	});
}

function messageFromClaim(claim, seat) {
	const message = claim?.message;
	if (
		!positiveInteger(claim?.job_id) ||
		!nonempty(message?.msg_id) ||
		!nonempty(message.from) ||
		message.to !== seat ||
		typeof message.body !== "string" ||
		message.command != null ||
		claim.command != null
	)
		throw new NativeError(
			"Malformed, unsupported command or wrong-recipient claim; receiving held without acknowledgement",
		);
	return { msg_id: message.msg_id, from: message.from, to: message.to, body: message.body };
}

function sameMessage(left, right) {
	return (
		left?.msg_id === right.msg_id &&
		left.from === right.from &&
		left.to === right.to &&
		left.body === right.body
	);
}

/** Consumption and completion belong to one accepted native message, including after restart. */
class NativeCompletion {
	constructor(msgId, cursor, replay = false, jobId) {
		this.msgId = msgId;
		this.jobId = jobId;
		this.tailCursor = cursor;
		this.emptyReads = 0;
		this.lastProbeAt = 0;
		this.live = !replay;
		this.cursor = replay ? undefined : cursor;
		this.replay = replay;
		this.pending = [];
		this.descendants = new Set();
		this.finalMessages = new Set();
		this.waiters = new Set();
	}
	notify() {
		for (const wake of this.waiters) wake();
	}
	async wait(promise, terminal, signal) {
		let wake;
		const observed = new Promise((resolve) => {
			wake = () => {
				if (terminal ? this.terminal : this.consumption) resolve();
			};
			this.waiters.add(wake);
			wake();
		});
		try {
			return await abortable(Promise.race([promise, observed]), signal);
		} finally {
			this.waiters.delete(wake);
		}
	}
	bind(nativeId) {
		this.nativeId = nativeId;
		for (const { event, finalMessage } of this.pending) this.observe(event, finalMessage);
		this.pending = undefined;
	}
	observe(
		event,
		finalMessage = event.type === "assistant.message" &&
			(event.data?.toolRequests === undefined ||
				(Array.isArray(event.data.toolRequests) && event.data.toolRequests.length === 0)),
		fresh = true,
		notify = true,
	) {
		if (this.terminal) return;
		if (
			fresh &&
			!this.consumption &&
			!event.agentId &&
			nonempty(event.id) &&
			["assistant.turn_end", "session.idle"].includes(event.type)
		) {
			this.boundary = event.id;
		}
		if (!this.nativeId) {
			// Keep correlation metadata only, never buffered prompts, tool arguments or model text.
			this.pending.push({
				event: {
					type: event.type,
					id: event.id,
					parentId: event.parentId,
					agentId: event.agentId,
					data: { messageId: event.data?.messageId, interactionId: event.data?.interactionId },
				},
				finalMessage,
			});
			return;
		}
		if (!nonempty(event.id) || this.descendants.has(event.id)) return;
		if (event.type === "user.message") {
			if (!event.agentId && event.data?.messageId === this.nativeId) {
				this.descendants.add(event.id);
				if (nonempty(event.data.interactionId)) this.interactionId ??= event.data.interactionId;
				if (!this.consumption) {
					this.consumption = { eventId: event.id };
					if (notify) this.notify();
				}
			}
			return;
		}
		// Native callbacks can omit permission ancestors; only the matched user's foreground turn can re-anchor.
		const sameInteraction =
			!event.agentId &&
			this.interactionId !== undefined &&
			event.data?.interactionId === this.interactionId &&
			(event.type === "assistant.turn_start" ||
				event.type === "assistant.message" ||
				event.type === "tool.execution_complete");
		if (!sameInteraction && !this.descendants.has(event.parentId)) return;
		this.descendants.add(event.id);
		if (!event.agentId && finalMessage) this.finalMessages.add(event.id);
		if (
			!event.agentId &&
			(["session.idle", "session.error", "abort"].includes(event.type) ||
				(event.type === "assistant.turn_end" && this.finalMessages.has(event.parentId)))
		) {
			this.terminal = { type: event.type, eventId: event.id };
			this.descendants.clear();
			this.finalMessages.clear();
			if (notify) this.notify();
		}
	}
}

export class NativeBridge {
	constructor({
		registration,
		native,
		client,
		journal,
		report,
		delay = (ms, signal) => sleep(ms, undefined, { signal }),
		heartbeatDelay = (ms, signal) => sleep(ms, undefined, { signal }),
		now = Date.now,
		setTimer = setTimeout,
		clearTimer = clearTimeout,
	}) {
		this.registration = Object.freeze({ ...registration });
		this.native = native;
		this.client = client;
		this.journal = journal;
		this.report = report;
		this.delay = delay;
		this.heartbeatDelay = heartbeatDelay;
		this.now = now;
		this.setTimer = setTimer;
		this.clearTimer = clearTimer;
		this.observedAt = 0;
		this.observedSeq = 0;
		this.recentEventIds = new Set();
		this.receiverController = new AbortController();
		this.heartbeatController = new AbortController();
		this.controller = new AbortController();
		this.registered = false;
		this.running = undefined;
		this.unsubscribe = undefined;
		this.completion = undefined;
		// Plan 158 addendum 3: busy/idle publication, serialized so a late
		// `working` can never land after `idle`.
		this.activity = Promise.resolve();
		this.activityState = undefined;
		this.activityUnsupported = false;
		/** The last state the daemon accepted; stop settles a seat left `working`. */
		this.activityPublished = undefined;
		this.stopped = undefined;
	}
	emit(kind, details = {}) {
		this.report({
			kind,
			seat: this.registration.id,
			nativeSession: this.registration.harness_session,
			...details,
		});
	}
	/**
	 * Stops synchronously. The returned promise settles once a seat that may still
	 * read `working` has published `idle`, bounded by HOOK_REQUEST_TIMEOUT_MS; it
	 * never rejects, and repeated stops share it.
	 */
	stop(cause = "stop") {
		if (this.controller.signal.aborted) return this.stopped;
		if (this.completion && !this.completion.terminal)
			this.emit("receiver-stopped", {
				cause,
				msgId: this.completion.msgId,
				jobId: this.completion.jobId,
				nativeMessageId: this.completion.nativeId ?? null,
			});
		this.registered = false;
		this.controller.abort();
		this.receiverController.abort();
		this.heartbeatController.abort();
		this.unsubscribe?.();
		this.unsubscribe = undefined;
		// The aborted signal settles any in-flight publication before this runs.
		this.stopped = this.activity.then(() => this.publishStopIdle());
		return this.stopped;
	}
	async publishStopIdle() {
		if (
			this.activityUnsupported ||
			(this.activityPublished !== "working" && this.activityState !== "working")
		)
			return;
		try {
			await this.client.request(
				"/v1/activity",
				{
					seat: this.registration.id,
					native_session: this.registration.harness_session,
					state: "idle",
				},
				AbortSignal.timeout(HOOK_REQUEST_TIMEOUT_MS),
			);
			this.activityPublished = "idle";
		} catch (error) {
			this.emit("activity-unpublished", {
				state: "idle",
				status: error?.status ?? null,
				safeDiagnostic: error instanceof NativeError ? error.safeDiagnostic : null,
			});
		}
	}
	run() {
		if (!this.running) this.running = this.consume();
		return this.running;
	}
	/** SDK cancellation cancels our wait; an issued send can still commit remotely. */
	nativeRpc(call, operation) {
		const signal = this.receiverController.signal;
		throwIfStopped(signal);
		return new Promise((resolve, reject) => {
			let settled = false;
			let timer;
			const finish = (error, value) => {
				if (settled) return;
				settled = true;
				this.clearTimer(timer);
				signal.removeEventListener("abort", abort);
				if (error) reject(error);
				else resolve(value);
			};
			const failure = (deadline) =>
				new NativeError(
					call === "native.send"
						? "Native send outcome is ambiguous; intent preserved without ack or reinjection"
						: `Native ${call} ${deadline ? "deadline exceeded" : "failed"}; receiving held without unproven acknowledgement or reinjection`,
					false,
					`${call} ${deadline ? `deadline exceeded (${NATIVE_RPC_DEADLINE_MS}ms)` : "failed"}${call === "native.send" ? "; send outcome ambiguous, intent preserved" : ""}`,
				);
			const abort = () => finish(signal.reason);
			signal.addEventListener("abort", abort, { once: true });
			timer = this.setTimer(() => {
				const error = failure(true);
				finish(error);
				this.holdReceiving(error);
			}, NATIVE_RPC_DEADLINE_MS);
			try {
				Promise.resolve(operation()).then(
					(value) => finish(undefined, value),
					() => finish(failure(false)),
				);
			} catch {
				finish(failure(false));
			}
		});
	}
	observeProgress(event) {
		if (!nonempty(event?.id) || this.recentEventIds.has(event.id)) return;
		// Bound callback/history overlap bookkeeping, never retaining event bodies.
		this.recentEventIds.add(event.id);
		if (this.recentEventIds.size > 256)
			this.recentEventIds.delete(this.recentEventIds.values().next().value);
		this.observedSeq = Math.min(Number.MAX_SAFE_INTEGER, this.observedSeq + 1);
		this.observedAt = Math.min(
			Number.MAX_SAFE_INTEGER,
			Math.max(this.observedAt, Math.floor(this.now())),
		);
	}
	holdReceiving(error) {
		if (this.receiverController.signal.aborted) return;
		this.emit("receive-held", {
			diagnostic: error.message,
			safeDiagnostic:
				error instanceof NativeError
					? error.safeDiagnostic
					: `non-native ${error?.name ?? "error"}`,
			action:
				"Inspect native history and durable acceptance before recovery; do not blindly resend or acknowledge.",
		});
		this.receiverController.abort(error);
		this.heartbeatController.abort();
	}
	async register() {
		const signal = this.controller.signal;
		const registration = this.registration;
		const seat = await this.client.request(
			"/v1/register",
			this.reconnectRegistration ?? registration,
			signal,
		);
		throwIfStopped(signal);
		verifyRegisteredSeat(seat, registration);
		this.registered = true;
		if (!this.reconnectRegistration) {
			this.reconnectRegistration = { ...registration };
			delete this.reconnectRegistration.supersedes;
			this.emit("registered", {
				pid: registration.pid,
				procStart: registration.proc_start,
				pane: registration.pane ?? null,
			});
		}
	}
	async keepReceiverAlive() {
		const signal = this.heartbeatController.signal;
		let retry = INITIAL_RETRY_MS;
		let unavailable = false;
		let observedAt = 0;
		let observedSeq = 0;
		while (!signal.aborted) {
			try {
				if (this.observedSeq !== observedSeq) {
					// One logical tick per changed report handles clock rollback;
					// replaying many events must never invent future wall-clock seconds.
					observedAt = Math.min(Number.MAX_SAFE_INTEGER, Math.max(observedAt + 1, this.observedAt));
					observedSeq = this.observedSeq;
				}
				const lease = await this.client.request(
					"/v1/inbox/heartbeat",
					{ ...this.tuple(), observed_at: observedAt, observed_seq: observedSeq },
					signal,
				);
				throwIfStopped(signal);
				if (lease?.state === "stale" && lease.reason === "native-receiver-stale") {
					this.holdReceiving(new NativeError("native-receiver-stale"));
					return;
				}
				if (
					lease?.state !== "live" ||
					!positiveInteger(lease.lease_ms) ||
					!positiveInteger(lease.renew_after_ms) ||
					lease.renew_after_ms >= lease.lease_ms
				)
					throw new NativeError("Native receiver lease response is malformed; receiving held");
				unavailable = false;
				retry = INITIAL_RETRY_MS;
				await this.heartbeatDelay(lease.renew_after_ms, signal);
			} catch (error) {
				if (signal.aborted) return;
				if (!unavailable)
					this.emit("receiver-lease-unavailable", {
						safeDiagnostic:
							error instanceof NativeError ? error.safeDiagnostic : "lease renewal failed",
					});
				unavailable = true;
				// The daemon independently expires the lease if it cannot be renewed.
				try {
					await this.heartbeatDelay(retry, signal);
				} catch {
					return;
				}
				retry = Math.min(retry * 2, MAX_RETRY_MS);
			}
		}
	}
	tuple() {
		const r = this.registration;
		return { seat: r.id, native_session: r.harness_session, pid: r.pid, proc_start: r.proc_start };
	}
	async consume() {
		const signal = this.receiverController.signal;
		let retry = INITIAL_RETRY_MS;
		let failureEpisode = false;
		let holdStarted;
		try {
			throwIfStopped(signal);
			if (
				typeof this.native.rpc?.eventLog?.tail !== "function" ||
				typeof this.native.rpc?.eventLog?.read !== "function"
			)
				throw new NativeError(
					"Native incremental history API eventLog.tail/read is unavailable; receiving held without acknowledgement",
				);
			this.unsubscribe = this.native.on((event) => {
				if (signal.aborted) return;
				this.observeProgress(event);
				this.observeActivity(event);
				const completion = this.completion;
				// Successful terminal observation clears ancestry; capture its pre-state, not the aftermath.
				const metadata = [
					"user.message",
					"assistant.message",
					"session.idle",
					"session.error",
				].includes(event.type)
					? {
							completionNativeId: completion?.nativeId ?? null,
							completionLive: completion?.live ?? false,
							parentKnown: completion?.descendants.has(event.parentId) ?? false,
							descendantCount: completion?.descendants.size ?? 0,
						}
					: undefined;
				completion?.observe(event);
				if (metadata)
					this.emit("native-event", {
						type: event.type,
						eventId: typeof event.id === "string" ? event.id : null,
						nativeMessageId:
							typeof event.data?.messageId === "string" ? event.data.messageId : null,
						parentId: typeof event.parentId === "string" ? event.parentId : null,
						agentId: typeof event.agentId === "string" ? event.agentId : null,
						interactionId:
							typeof event.data?.interactionId === "string" ? event.data.interactionId : null,
						...metadata,
						terminal: completion?.terminal
							? { type: completion.terminal.type, eventId: completion.terminal.eventId }
							: null,
						grade: "native-event-not-pij-ack",
					});
				if (event.type === "session.shutdown") this.stop("session.shutdown");
			});
			while (!signal.aborted) {
				try {
					if (!this.registered) {
						await this.register();
						if (holdStarted !== undefined) {
							this.emit("connection-ready");
							retry = INITIAL_RETRY_MS;
						}
						holdStarted = undefined;
					}
					if (!this.heartbeat) {
						await this.observeStartup();
						this.heartbeat = this.keepReceiverAlive();
					}
					if (this.completion) await this.waitForCompletion();
					throwIfStopped(signal);
					const { claims, hold } = await this.client.claimInbox(this.tuple(), signal);
					throwIfStopped(signal);
					if (!Array.isArray(claims) || claims.length > 1)
						throw new NativeError("Expected zero or one native claim; receiving held");
					if (failureEpisode) this.emit("connection-ready");
					failureEpisode = false;
					if (hold?.kind === "native-target") {
						this.emit("receive-held", {
							holdKind: hold.kind,
							diagnostic: hold.reason,
							safeDiagnostic: hold.reason,
							action:
								"Resume the targeted native session or use a new seat and intentionally reissue the message; no acknowledgement was sent.",
						});
						return;
					}
					if (hold || claims.length === 0) {
						if (hold) this.emit("consent-held", { reason: hold.reason, retryMs: retry });
						await this.delay(retry, signal);
						retry = Math.min(retry * 2, MAX_RETRY_MS);
						continue;
					}
					await this.deliver(claims[0]);
					retry = INITIAL_RETRY_MS;
				} catch (error) {
					if (signal.aborted) return;
					if (!(error instanceof InboxWaitDeadline)) {
						if (!error.retryable) throw error;
						this.registered = false;
						if (error instanceof NativeError && error.holdKind === "native-session") {
							const now = performance.now();
							holdStarted ??= now;
							const elapsedMs = now - holdStarted;
							if (elapsedMs >= HOLD_ESCALATION_MS) retry = HOLD_ESCALATED_RETRY_MS;
							failureEpisode = false;
							this.emit("registration-wait", {
								holdKind: error.holdKind,
								elapsedMs,
								retryMs: retry,
							});
						} else {
							if (holdStarted !== undefined) retry = INITIAL_RETRY_MS;
							holdStarted = undefined;
							if (!failureEpisode)
								this.emit("reconnecting", {
									diagnostic: error.message,
									safeDiagnostic: error instanceof NativeError ? error.safeDiagnostic : null,
									retryMs: retry,
								});
							failureEpisode = true;
						}
					}
					await this.delay(retry, signal);
					retry = Math.min(retry * 2, MAX_RETRY_MS);
				}
			}
		} catch (error) {
			if (!signal.aborted) this.holdReceiving(error);
		} finally {
			this.heartbeatController.abort();
			this.receiverController.abort();
			this.unsubscribe?.();
			this.unsubscribe = undefined;
		}
	}
	async waitForCompletion() {
		const completion = this.completion;
		if (completion.reported) return;
		this.emit("completion-wait", {
			msgId: completion.msgId,
			jobId: completion.jobId,
			nativeMessageId: completion.nativeId,
		});
		await this.waitForObservation(completion, true);
		const terminal = completion.terminal;
		throwIfStopped(this.receiverController.signal);
		completion.reported = true;
		this.emit("native-completed", {
			msgId: completion.msgId,
			nativeMessageId: completion.nativeId,
			...terminal,
			grade: "observed-native-terminal-not-model-success",
		});
	}
	async readEvents(completion) {
		// One outstanding SDK read per delivery; racing a callback must not fan out
		// uncancellable requests or retain full response bodies on a pending promise.
		if (!completion.reading) {
			completion.reading = (async () => {
				const page = await this.nativeRpc("eventLog.read", () =>
					this.native.rpc.eventLog.read({
						cursor: completion.cursor,
						max: 128,
						includeEphemeral: false,
					}),
				);
				throwIfStopped(this.receiverController.signal);
				if (
					!Array.isArray(page?.events) ||
					page.events.length > 128 ||
					typeof page.cursor !== "string" ||
					typeof page.hasMore !== "boolean" ||
					page.cursorStatus !== "ok" ||
					(page.hasMore && page.cursor === completion.cursor)
				)
					throw new NativeError(
						"Native incremental history cursor expired or page malformed; receiving held without reinjection",
					);
				for (const event of page.events) {
					this.observeProgress(event);
					completion.observe(event, undefined, !completion.replay);
					if (nonempty(event.id)) completion.lastEventId = event.id;
				}
				completion.cursor = page.cursor;
				completion.hasMore = page.hasMore;
				if (!page.hasMore) completion.replay = false;
				completion.emptyReads = page.events.length ? 0 : (completion.emptyReads ?? 0) + 1;
				if (
					!page.hasMore &&
					completion.emptyReads >= RECEIVER_EMPTY_READ_LIMIT &&
					this.now() - completion.lastProbeAt >= RECEIVER_PROBE_INTERVAL_MS
				)
					await this.probeReceiver(completion);
			})().finally(() => {
				completion.reading = undefined;
			});
		}
		return completion.reading;
	}
	async observeStartup() {
		// A recycled child must prove its current SDK view before asking an old
		// lease to renew. This is observation only, never consumption/terminal proof.
		const page = await this.nativeRpc("eventLog.read", () =>
			this.native.rpc.eventLog.read({ direction: "backward", max: 128, includeEphemeral: false }),
		);
		if (
			!Array.isArray(page?.events) ||
			page.events.length > 128 ||
			typeof page.cursor !== "string" ||
			typeof page.hasMore !== "boolean" ||
			page.cursorStatus !== "ok"
		)
			throw new NativeError(
				"Native startup observation page is unavailable or malformed; receiving held",
			);
		for (const event of page.events) this.observeProgress(event);
	}
	async probeReceiver(completion) {
		const signal = this.receiverController.signal;
		completion.lastProbeAt = this.now();
		const tail = await this.nativeRpc("eventLog.tail", () => this.native.rpc.eventLog.tail());
		if (typeof tail?.cursor !== "string")
			throw new NativeError("Native receiver progress tail is malformed; receiving held");
		if (tail.cursor === completion.tailCursor || tail.cursor === completion.cursor) {
			completion.tailCursor = tail.cursor;
			return;
		}
		// Capture the FORWARD continuation before reading a chronological bounded tail
		// window. A backward read cursor points to older events and must never replace it.
		const page = await this.nativeRpc("eventLog.read", () =>
			this.native.rpc.eventLog.read({
				direction: "backward",
				max: 128,
				includeEphemeral: false,
			}),
		);
		throwIfStopped(signal);
		if (
			!Array.isArray(page?.events) ||
			page.events.length === 0 ||
			page.events.length > 128 ||
			typeof page.cursor !== "string" ||
			typeof page.hasMore !== "boolean" ||
			page.cursorStatus !== "ok"
		)
			throw new NativeError("Native receiver progress gap is inaccessible; receiving held");
		const anchor = page.events.findIndex((event) => event.id === completion.lastEventId);
		if (anchor === page.events.length - 1) {
			completion.tailCursor = tail.cursor;
			return;
		}
		completion.discardUncertain = true;
		const start = anchor < 0 ? 0 : anchor + 1;
		for (let index = start; index < page.events.length; index++) {
			const event = page.events[index];
			this.observeProgress(event);
			// A recovered historical boundary is never authority to re-inject an
			// accepted message whose consumption may have fallen in the gap.
			completion.observe(event, undefined, false, false);
		}
		if (completion.msgId && anchor < 0 && !completion.terminal) {
			const error = new NativeError(
				"Native receiver progress gap lacks correlated terminal evidence; receiving held without acknowledgement or reinjection",
			);
			this.holdReceiving(error);
			throw error;
		}
		const previousEventId = completion.lastEventId ?? null;
		completion.lastEventId = page.events.at(-1).id;
		completion.cursor = tail.cursor;
		completion.tailCursor = tail.cursor;
		completion.emptyReads = 0;
		completion.replay = false;
		this.emit("receiver-rebaselined", {
			msgId: completion.msgId ?? null,
			jobId: completion.jobId ?? null,
			gap: {
				afterEventId: previousEventId,
				throughEventId: completion.lastEventId,
				observed: page.events.length - start,
				contiguous: anchor >= 0,
			},
		});
		completion.notify?.();
	}
	async waitForObservation(completion, terminal = false, message) {
		const signal = this.receiverController.signal;
		let retry = INITIAL_RETRY_MS;
		while (!(terminal ? completion.terminal : completion.consumption)) {
			throwIfStopped(signal);
			try {
				await completion.wait(this.readEvents(completion), terminal, signal);
			} catch (error) {
				if (signal.aborted || error instanceof NativeError) throw error;
				throw new NativeError(
					"Native incremental history could not be read; accepted delivery held without reinjection or unproven acknowledgement",
				);
			}
			throwIfStopped(signal);
			if (terminal ? completion.terminal : completion.consumption) return;
			if (completion.hasMore) continue;
			if (message && completion.boundary && (await this.canRetryDiscarded(completion, message)))
				return true;
			await completion.wait(this.delay(retry, signal), terminal, signal);
			retry = Math.min(retry * 2, MAX_RETRY_MS);
		}
	}
	async canRetryDiscarded(completion, message) {
		const signal = this.receiverController.signal;
		if (completion.discardUncertain)
			throw new NativeError(
				"Native receiver progress gap cannot prove discarded consumption; receiving held without reinjection",
			);
		const queue = this.native.rpc?.queue;
		const metadata = this.native.rpc?.metadata;
		if (typeof queue?.pendingItems !== "function" || typeof metadata?.isProcessing !== "function")
			throw new NativeError(
				"Native discard recovery requires supported queue and processing APIs; receiving held",
			);
		try {
			const pending = await this.nativeRpc("queue.pendingItems", () => queue.pendingItems());
			throwIfStopped(signal);
			if (
				!Array.isArray(pending?.items) ||
				!pending.items.every(
					(item) => nonempty(item?.id) && typeof item.displayText === "string",
				) ||
				!Array.isArray(pending.steeringMessages) ||
				!pending.steeringMessages.every((item) => typeof item === "string") ||
				(pending.inFlightSteeringCount !== undefined &&
					(!Number.isSafeInteger(pending.inFlightSteeringCount) ||
						pending.inFlightSteeringCount < 0))
			)
				throw new NativeError(
					"Native discard recovery received malformed queue snapshot; receiving held",
				);
			const prefix = `[pij from ${JSON.stringify(message.from)}; msg_id=${JSON.stringify(message.msg_id)}]\n`;
			if (
				pending.items.some(
					(item) => item.id === completion.nativeId || item.displayText.startsWith(prefix),
				) ||
				pending.steeringMessages.some((item) => item.startsWith(prefix)) ||
				pending.inFlightSteeringCount > 0
			)
				return false;
			const state = await this.nativeRpc("metadata.isProcessing", () => metadata.isProcessing());
			throwIfStopped(signal);
			if (typeof state?.processing !== "boolean")
				throw new NativeError(
					"Native discard recovery received malformed processing snapshot; receiving held",
				);
			if (state.processing) return false;
			// A fresh cursor read after both snapshots catches consumption whose
			// live callback was omitted. Drain pages before concluding absence.
			do {
				await this.readEvents(completion);
			} while (completion.hasMore && !completion.consumption);
			// Empty queue + idle is not proof of non-consumption if the forward
			// cursor is silently stale. Check the independent authority before rearm.
			if (!completion.consumption) await this.probeReceiver(completion);
			if (!completion.consumption && completion.discardUncertain)
				throw new NativeError(
					"Native receiver progress gap cannot prove discarded consumption; receiving held without reinjection",
				);
			return !completion.consumption;
		} catch (error) {
			if (signal.aborted || error instanceof NativeError) throw error;
			throw new NativeError(
				"Native discard recovery queue, processing or history snapshot failed; receiving held without reinjection",
			);
		}
	}
	async newCompletion(msgId, replay = false, jobId) {
		const tail = await this.nativeRpc("eventLog.tail", () => this.native.rpc.eventLog.tail());
		throwIfStopped(this.receiverController.signal);
		if (typeof tail?.cursor !== "string")
			throw new NativeError("Native incremental history baseline is malformed; receiving held");
		const completion = new NativeCompletion(msgId, tail.cursor, replay, jobId);
		if (!replay) {
			// The SDK tail cursor also counts ephemeral events, which includeEphemeral:false
			// reads never return. Anchor the baseline to the newest durable event so the
			// progress probe can tell ephemeral-only tail movement from a real durable gap.
			// Read after tail: any event between the two precedes our send either way.
			const page = await this.nativeRpc("eventLog.read", () =>
				this.native.rpc.eventLog.read({ direction: "backward", max: 1, includeEphemeral: false }),
			);
			throwIfStopped(this.receiverController.signal);
			if (!Array.isArray(page?.events) || page.events.length > 1 || page.cursorStatus !== "ok")
				throw new NativeError(
					"Native incremental history baseline anchor is malformed; receiving held",
				);
			if (nonempty(page.events[0]?.id)) completion.lastEventId = page.events[0].id;
		}
		return completion;
	}
	async enqueue(message, completion) {
		const signal = this.receiverController.signal;
		throwIfStopped(signal);
		const prompt = `[pij from ${JSON.stringify(message.from)}; msg_id=${JSON.stringify(message.msg_id)}]\n${message.body}`;
		this.completion = completion;
		const nativeId = await this.nativeRpc("native.send", () =>
			this.native.send({ prompt, mode: "immediate" }),
		);
		throwIfStopped(signal);
		if (!nonempty(nativeId))
			throw new NativeError("Native send returned no message ID; acceptance ambiguous");
		completion.bind(nativeId);
		await this.journal.accept(message, nativeId);
		throwIfStopped(signal);
		this.emit("native-accepted", {
			msgId: message.msg_id,
			nativeMessageId: nativeId,
			grade: "native-accepted-not-model-complete",
		});
		return { state: "accepted", message, nativeId };
	}
	async #prepareNewSend() {
		const signal = this.receiverController.signal;
		throwIfStopped(signal);
		await abortable(this.client.nativeSnapshot(this.tuple(), signal), signal);
		throwIfStopped(signal);
	}
	async deliver(claim) {
		const signal = this.receiverController.signal;
		const proof = claim?.native_consumer;
		const expected = this.tuple();
		if (
			!proof ||
			proof.native_session !== expected.native_session ||
			proof.pid !== expected.pid ||
			proof.proc_start !== expected.proc_start
		)
			throw new NativeError(
				"Native consumer proof absent or mismatched; no injection or acknowledgement",
			);
		const message = messageFromClaim(claim, this.registration.id);
		let record = await this.journal.load(message.msg_id);
		throwIfStopped(signal);
		if (!record) {
			await this.#prepareNewSend();
			throwIfStopped(signal);
			const completion = await this.newCompletion(message.msg_id, false, claim.job_id);
			throwIfStopped(signal);
			if (await this.journal.begin(message)) {
				record = await this.enqueue(message, completion);
			} else record = await this.journal.load(message.msg_id);
		}
		throwIfStopped(signal);
		if (!record || !sameMessage(record.message, message))
			throw new NativeError("Duplicate message ID has conflicting content; receive held");
		if (record.state !== "accepted" || !nonempty(record.nativeId))
			throw new NativeError(
				"Previous native send is ambiguous; held without reinjection or acknowledgement",
			);
		if (this.completion?.msgId !== message.msg_id || this.completion.nativeId !== record.nativeId) {
			this.completion = await this.newCompletion(message.msg_id, true, claim.job_id);
			this.completion.bind(record.nativeId);
		}
		while (await this.waitForObservation(this.completion, false, message)) {
			throwIfStopped(signal);
			await this.#prepareNewSend();
			if (this.completion.consumption) break;
			const completion = await this.newCompletion(message.msg_id, false, claim.job_id);
			if (!(await this.canRetryDiscarded(this.completion, message))) continue;
			if (this.completion.consumption) break;
			if (!(await this.journal.rearm(message, record.nativeId)))
				throw new NativeError(
					"Native discard retry intent is already owned or ambiguous; receiving held without reinjection",
				);
			throwIfStopped(signal);
			if (this.completion.consumption) {
				await this.journal.accept(message, record.nativeId);
				break;
			}
			const discarded = this.completion;
			record = await this.enqueue(message, completion);
			this.emit("native-requeued", {
				msgId: message.msg_id,
				nativeMessageId: record.nativeId,
				discardedNativeId: discarded.nativeId,
				boundaryId: discarded.boundary,
			});
		}
		throwIfStopped(signal);
		const acknowledged = await this.client.request(
			"/v1/inbox/ack",
			{ ...this.tuple(), job_id: claim.job_id },
			signal,
		);
		throwIfStopped(signal);
		if (acknowledged !== claim.job_id)
			throw new NativeError("Daemon ack did not confirm this job ID");
		this.emit("inbox-acknowledged", {
			jobId: claim.job_id,
			msgId: message.msg_id,
			nativeMessageId: record.nativeId,
			grade: "native-consumed-not-model-complete",
		});
	}
	/** Foreground turns only: sub-agent turns never flip the seat's busy/idle state. */
	observeActivity(event) {
		if (event?.agentId) return;
		const state =
			event?.type === "assistant.turn_start"
				? "working"
				: event?.type === "assistant.turn_end" || event?.type === "session.idle"
					? "idle"
					: undefined;
		if (!state || state === this.activityState || this.activityUnsupported) return;
		this.activityState = state;
		this.activity = this.activity.then(() => this.publishActivity(state));
	}
	async publishActivity(state) {
		if (this.activityUnsupported || !this.registered || this.controller.signal.aborted) {
			// Not published: the next observed transition must try again.
			if (this.activityState === state) this.activityState = undefined;
			return;
		}
		try {
			await this.client.request(
				"/v1/activity",
				{ seat: this.registration.id, native_session: this.registration.harness_session, state },
				this.controller.signal,
			);
			this.activityPublished = state;
		} catch (error) {
			if (this.controller.signal.aborted) return;
			if (this.activityState === state) this.activityState = undefined;
			// An older daemon has no activity route; stop asking for this runtime.
			if (error?.status === 404) this.activityUnsupported = true;
			this.emit("activity-unpublished", {
				state,
				status: error?.status ?? null,
				safeDiagnostic: error instanceof NativeError ? error.safeDiagnostic : null,
			});
		}
	}
	/**
	 * Claim this seat's held FYIs for a typed prompt (plan 158). The daemon's
	 * block is the only rendering; it passes through verbatim. Never throws.
	 */
	async claimFyis() {
		if (!this.registered || this.controller.signal.aborted) return undefined;
		try {
			const claim = await this.client.request(
				"/v1/fyi/claim",
				{
					seat: this.registration.id,
					native_session: this.registration.harness_session,
					via: "hook:copilot",
				},
				AbortSignal.any([this.controller.signal, AbortSignal.timeout(HOOK_REQUEST_TIMEOUT_MS)]),
			);
			return claim?.count > 0 && nonempty(claim.block)
				? { additionalContext: claim.block }
				: undefined;
		} catch (error) {
			this.emit("fyi-claim-failed", {
				safeDiagnostic: error instanceof NativeError ? error.safeDiagnostic : null,
			});
			return undefined;
		}
	}
	async send(input) {
		if (!this.registered || this.controller.signal.aborted)
			return { ok: false, error: "Pij native session is not registered; no message sent" };
		if (!nonempty(input?.to) || !nonempty(input?.message))
			return { ok: false, error: "pij_send requires nonempty to and message strings" };
		const force = input.force === true;
		// The daemon refuses these too; refusing here sends nothing at all.
		if (force && input.fyi === true)
			return { ok: false, error: `${COLD_WAKE_CODE}: force wakes a message; never with fyi` };
		if (force && !nonempty(input.reason))
			return {
				ok: false,
				error: `${COLD_WAKE_CODE}: force needs a non-empty reason saying why the wake is worth it`,
			};
		const msgId = randomUUID();
		try {
			const receipt = await this.client.request(
				"/v1/send",
				{
					from: this.registration.id,
					to: { seat: input.to },
					body: input.message,
					msg_id: msgId,
					...(input.fyi === true ? { fyi: true } : {}),
					...(force ? { force: true, reason: input.reason } : {}),
				},
				this.controller.signal,
			);
			throwIfStopped(this.controller.signal);
			if (receipt?.msg_id !== msgId)
				throw new NativeError("Daemon send receipt has a different message ID");
			return {
				ok: true,
				receipt,
				grade: "pij-receipt-not-model-complete",
				// Plan 159: a held FYI that looks like a question; still held.
				...(typeof receipt.warning === "string" ? { warning: receipt.warning } : {}),
			};
		} catch (error) {
			if (error?.coldWake === true)
				return { ok: false, msg_id: msgId, error: error.message, grade: "refused-not-sent" };
			return { ok: false, msg_id: msgId, error: error.message, grade: "send-outcome-unconfirmed" };
		}
	}
}

/** Follow OS parent links; neither the extension child nor pane shell is the host. */
export async function resolveNativeHost({ parentPid, inspectProcess, pane, paneProcess }) {
	if (pane !== undefined && (!/^%\d+$/.test(pane) || !positiveInteger(paneProcess)))
		throw new NativeError("Native pane identity could not be observed");
	const seen = new Set();
	let pid = parentPid;
	let host;
	let inPane = pane === undefined;
	while (positiveInteger(pid) && pid > 1 && seen.size < 64 && !seen.has(pid)) {
		seen.add(pid);
		const observed = await inspectProcess(pid);
		if (observed.pid !== pid || !positiveInteger(observed.proc_start))
			throw new NativeError("Native ancestor identity changed during observation");
		if (!host && /(?:^|[/\\])copilot(?:\.exe)?$/.test(observed.command)) host = observed;
		if (pid === paneProcess) inPane = true;
		if (host && inPane) break;
		pid = observed.ppid;
	}
	if (!host || !inPane)
		throw new NativeError(
			"Cannot verify a Copilot host ancestor in the exact pane; native registration refused",
		);
	const confirmed = await inspectProcess(host.pid);
	if (
		confirmed.proc_start !== host.proc_start ||
		confirmed.ppid !== host.ppid ||
		confirmed.command !== host.command
	)
		throw new NativeError("Native host incarnation changed during observation");
	return Object.freeze({
		pid: host.pid,
		proc_start: host.proc_start,
		...(pane === undefined ? {} : { pane }),
	});
}

/** Native timeline diagnostics share an episode across bootstrap and receive reconnects. */
export function createNativeReporter({ log, capture }) {
	let unavailable = false;
	let registrationWaiting = false;
	let registrationEscalated = false;
	return (event) => {
		if (event.kind === "registered" || event.kind === "connection-ready") {
			unavailable = false;
			registrationWaiting = false;
			registrationEscalated = false;
		}
		if (event.kind === "connection-ready") return;
		if (event.kind === "registration-wait" && event.holdKind === "native-session") {
			unavailable = false;
			capture({
				kind: event.kind,
				holdKind: event.holdKind,
				elapsedMs: event.elapsedMs,
				retryMs: event.retryMs,
			});
			const escalated = event.elapsedMs >= HOLD_ESCALATION_MS;
			if (escalated ? registrationEscalated : registrationWaiting || !(event.elapsedMs >= 10000))
				return;
			registrationWaiting = true;
			registrationEscalated ||= escalated;
			const message = escalated
				? "[pij native] still waiting for this pane's resumed Copilot session after 10 min; pij delivery is unavailable in this window"
				: "[pij native] waiting for this pane's resumed Copilot session";
			return Promise.resolve()
				.then(() => log(message, { level: "info" }))
				.catch(() => capture({ kind: "native-diagnostic-unavailable", failureKind: event.kind }));
		}
		if (event.kind === "native-requeued") {
			capture(event);
			return Promise.resolve()
				.then(() => log("📨 re-queued 1 pij messages after interrupt", { level: "info" }))
				.catch(() => capture({ kind: "native-diagnostic-unavailable", failureKind: event.kind }));
		}
		if (
			["registration-wait", "reconnecting", "receive-held", "extension-unavailable"].includes(
				event.kind,
			)
		) {
			registrationWaiting = false;
			registrationEscalated = false;
			if (unavailable && !["receive-held", "extension-unavailable"].includes(event.kind)) return;
			unavailable = true;
			const advice =
				event.kind === "receive-held"
					? event.holdKind === "native-target"
						? "queued work targets another native session; resume that native session or use a new seat and intentionally reissue the message; receiving stopped without acknowledgement; outgoing pij_send remains available"
						: "receiving held; inspect native history and acceptance before recovery, do not blindly resend or acknowledge"
					: event.kind === "extension-unavailable"
						? `native registration failed: ${event.safeDiagnostic ?? "native initialization failed"}; no retry is scheduled — restart the Copilot CLI (/restart or relaunch) to recover; ordinary Copilot remains usable`
						: "check the Pij daemon and PIJ_RS_ADDR/PIJ_RS_STATE_DIR; retrying when available; ordinary Copilot remains usable";
			// Never project the raw diagnostic: HTTP/OS errors may carry bodies or credentials.
			// The stderr log keeps the SAFE diagnostic so a hold is diagnosable after the fact.
			capture({
				kind: event.kind,
				holdKind: event.holdKind ?? null,
				safeDiagnostic: event.safeDiagnostic ?? null,
				retryMs: event.retryMs ?? null,
			});
			return Promise.resolve()
				.then(() => log(`[pij native] unavailable: ${event.kind}; ${advice}`, { level: "info" }))
				.catch(() => capture({ kind: "native-diagnostic-unavailable", failureKind: event.kind }));
		}
		capture(event);
	};
}
