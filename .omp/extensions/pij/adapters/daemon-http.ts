import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ControlOutcome } from "../core/commands.js";
import {
	type ActivityRequest,
	DaemonRefusalError,
	decodeEnvelope,
	decodeStreamLine,
	type EventFrame,
	type FyiClaim,
	type FyiClaimRequest,
	type HealthPayload,
	type HoldRequest,
	type HoldResponse,
	PijNoDaemonError,
	parseProcessStart,
	type QueueReceipt,
	type Registration,
	type ReleaseRequest,
	type ReleaseResponse,
	type RustSeatDescriptor,
	type SendRequest,
} from "../core/daemon-wire.js";

export const DEFAULT_PIJ_RS_ADDR = "127.0.0.1:7461";

export interface DaemonLocation {
	readonly addr: string;
	readonly stateDir: string;
}

export interface InboxHeartbeat {
	readonly job_id: number;
	readonly state: "running" | "done" | "failed";
}

interface FederatedRoster {
	readonly seats: readonly RustSeatDescriptor[];
	readonly unavailable: readonly { readonly machine: string; readonly reason: string }[];
}

export type DaemonGenerationPreference = "rs" | "legacy";

export type DaemonGeneration =
	| { readonly kind: "legacy"; readonly location: DaemonLocation }
	| {
			readonly kind: "rust";
			readonly location: DaemonLocation;
			readonly health: HealthPayload;
			readonly client: PijDaemonClient;
	  };

export interface DaemonHttpDeps {
	readonly fetch: typeof fetch;
	readonly readFile: (path: string, encoding: "utf8") => Promise<string>;
	readonly processStart: (pid: number) => number;
	readonly random?: () => number;
	readonly sleep?: (ms: number, signal: AbortSignal) => Promise<void>;
}

const DEFAULT_DEPS: DaemonHttpDeps = {
	fetch,
	readFile,
	processStart: observedProcessStart,
};

const INITIAL_RECONNECT_MS = 250;
const MAX_RECONNECT_MS = 5_000;
const RECONNECT_JITTER = 0.2;
const MAX_FRAME_ATTEMPTS = 5;

/** Resolve extension transport controls: explicit environment, then Rust defaults. */
export function daemonLocation(
	env: NodeJS.ProcessEnv = process.env,
	home: string = homedir(),
): DaemonLocation {
	const addr = configuredValue("PIJ_RS_ADDR", env.PIJ_RS_ADDR, DEFAULT_PIJ_RS_ADDR);
	assertDaemonAddr(addr);
	return {
		addr,
		stateDir: configuredValue("PIJ_RS_STATE_DIR", env.PIJ_RS_STATE_DIR, join(home, ".pij-rs")),
	};
}

/** req-0019: explicit generation wins; absence cuts over to Rust with no fallback. */
export function daemonGenerationPreference(
	env: NodeJS.ProcessEnv = process.env,
): DaemonGenerationPreference {
	const value = env.PIJ_DAEMON_GENERATION;
	if (value === undefined) return "rs";
	if (value === "rs" || value === "legacy") return value;
	throw invalidEnvironment("PIJ_DAEMON_GENERATION", value, 'expected exactly "rs" or "legacy"');
}

export async function detectDaemonGeneration(
	location: DaemonLocation = daemonLocation(),
	preference: DaemonGenerationPreference = daemonGenerationPreference(),
	deps: DaemonHttpDeps = DEFAULT_DEPS,
): Promise<DaemonGeneration> {
	if (preference === "legacy") return { kind: "legacy", location };

	let key: string | undefined;
	try {
		key = (await deps.readFile(join(location.stateDir, "daemon.key"), "utf8")).trim();
	} catch {
		// Probe without credentials. A v1 auth envelope still proves which daemon answered.
	}
	try {
		const response = await deps.fetch(`http://${location.addr}/health`, {
			headers: key ? { Authorization: `Bearer ${key}` } : undefined,
		});
		const body = await response.text();
		const health = decodeEnvelope<HealthPayload>(body).data;
		if (!key) {
			throw new DaemonRefusalError(
				"auth",
				"auth",
				`pij-rs answered at ${location.addr}, but ${join(location.stateDir, "daemon.key")} is unreadable`,
			);
		}
		return {
			kind: "rust",
			location,
			health,
			client: new PijDaemonClient(location, key, deps),
		};
	} catch (error) {
		if (isConnectionRefused(error)) throw new PijNoDaemonError(location.addr, location.stateDir);
		throw error;
	}
}

export class PijDaemonClient {
	private reconnecting = false;

	/** Inbox recovery defers transport work to the existing stream reconnect loop. */
	isReconnecting(): boolean {
		return this.reconnecting;
	}

	constructor(
		readonly location: DaemonLocation,
		private key: string,
		private readonly deps: DaemonHttpDeps = DEFAULT_DEPS,
	) {}

	processStart(pid: number): number {
		return this.deps.processStart(pid);
	}

	async refreshKey(): Promise<void> {
		const path = join(this.location.stateDir, "daemon.key");
		const key = (await this.deps.readFile(path, "utf8")).trim();
		if (key === "") throw new Error(`pij daemon key is empty: ${path}`);
		this.key = key;
	}

	health(): Promise<HealthPayload> {
		return this.get<HealthPayload>("/health");
	}

	spawn(request: {
		readonly harness: Registration["harness"];
		readonly model?: string;
		readonly effort?: string;
		readonly cwd: string;
		readonly caller_pane?: string;
		readonly parent: string;
		readonly no_wait: boolean;
		readonly accept_inbound: boolean;
		readonly role?: string;
		readonly caller?: { readonly PIJ_SESSION_ID: string; readonly TMUX_PANE?: string };
	}): Promise<RustSeatDescriptor> {
		return this.post<RustSeatDescriptor>("/v1/spawn", request);
	}

	register(claim: Registration): Promise<RustSeatDescriptor> {
		return this.post<RustSeatDescriptor>("/v1/register", claim);
	}

	async seats(): Promise<readonly RustSeatDescriptor[]> {
		return (await this.get<FederatedRoster>("/v1/seats")).seats;
	}

	send(request: SendRequest): Promise<QueueReceipt> {
		return this.post<QueueReceipt>("/v1/send", request);
	}

	/** Atomic pending → delivered claim; a count of 0 carries an empty block. */
	async claimFyi(request: FyiClaimRequest, signal?: AbortSignal): Promise<FyiClaim> {
		const claim = await this.post<unknown>("/v1/fyi/claim", request, signal);
		if (typeof claim !== "object" || claim === null || Array.isArray(claim))
			throw new Error("pij fyi claim returned a non-object");
		const row = claim as Record<string, unknown>;
		if (
			typeof row.seat !== "string" ||
			typeof row.count !== "number" ||
			!Number.isSafeInteger(row.count) ||
			row.count < 0 ||
			typeof row.block !== "string" ||
			!Array.isArray(row.ids)
		)
			throw new Error("pij fyi claim needs string seat/block, count and ids");
		return {
			seat: row.seat,
			count: row.count,
			block: row.block,
			ids: row.ids.filter((id): id is string => typeof id === "string"),
		};
	}

	/** `pendingFyis` from the state card; a daemon that omits it holds none. */
	async pendingFyis(seat: string): Promise<number> {
		const state = await this.post<unknown>("/v1/state", { id: seat });
		const count =
			typeof state === "object" && state !== null
				? (state as Record<string, unknown>).pendingFyis
				: undefined;
		return typeof count === "number" && Number.isSafeInteger(count) && count > 0 ? count : 0;
	}

	/**
	 * Publish the seat's turn state. Resolves false when the daemon has no
	 * /v1/activity route: a bare 404, as opposed to a refusal envelope.
	 */
	async publishActivity(request: ActivityRequest, signal?: AbortSignal): Promise<boolean> {
		const response = await this.request(
			"/v1/activity",
			{
				method: "POST",
				body: JSON.stringify(request),
				...(signal === undefined ? {} : { signal }),
			},
			{ "Content-Type": "application/json" },
		);
		const text = await response.text();
		try {
			decodeEnvelope<unknown>(text);
		} catch (error) {
			if (response.status === 404 && !(error instanceof DaemonRefusalError)) return false;
			throw error;
		}
		return true;
	}

	claimInbox(seat: string): Promise<unknown> {
		return this.get<unknown>(`/v1/inbox?seat=${encodeURIComponent(seat)}`);
	}

	/** Oldest live row, including deferred/running jobs, followed by parked rows.
	 * Ordinary claims never include parked jobs or bypass deferred eligibility. */
	peekInbox(seat: string): Promise<unknown> {
		return this.get<unknown>(`/v1/inbox?seat=${encodeURIComponent(seat)}&peek=true`);
	}

	ackInbox(
		seat: string,
		jobId: number,
		controlOutcome?: ControlOutcome,
		deliveryOutcome?: "undelivered:harness-swallowed",
	): Promise<number> {
		return this.post<number>("/v1/inbox/ack", {
			seat,
			job_id: jobId,
			...(controlOutcome === undefined ? {} : { control_outcome: controlOutcome }),
			...(deliveryOutcome === undefined ? {} : { delivery_outcome: deliveryOutcome }),
		});
	}

	/** Renew a running body claim, or observe that another consumer already settled it. */
	async heartbeatInbox(seat: string, jobId: number): Promise<InboxHeartbeat | undefined> {
		const value = await this.post<unknown>("/v1/inbox/heartbeat", { seat, job_id: jobId });
		if (typeof value !== "object" || value === null || Array.isArray(value)) return undefined;
		const row = value as Record<string, unknown>;
		if (
			row.job_id !== jobId ||
			(row.state !== "running" && row.state !== "done" && row.state !== "failed")
		)
			return undefined;
		return { job_id: jobId, state: row.state };
	}

	hold(request: HoldRequest): Promise<HoldResponse> {
		return this.post<HoldResponse>("/v1/hold", request);
	}

	release(request: ReleaseRequest): Promise<ReleaseResponse> {
		return this.post<ReleaseResponse>("/v1/release", request);
	}

	async watchEvents(
		onFrame: (frame: EventFrame) => unknown,
		onError: (error: Error) => void,
		onNotice?: (text: string) => void,
	): Promise<() => void> {
		const controller = new AbortController();
		const cursors = new Map<string, number>();
		const initial = await this.attachEvents(cursors, controller.signal);
		this.reconnecting = false;
		void this.superviseEvents(
			initial,
			cursors,
			onFrame,
			onError,
			onNotice,
			controller.signal,
		).catch((error: unknown) => {
			if (!controller.signal.aborted) onError(asError(error));
		});
		return () => controller.abort();
	}

	private async superviseEvents(
		initial: ReadableStream<Uint8Array>,
		cursors: Map<string, number>,
		onFrame: (frame: EventFrame) => unknown,
		onError: (error: Error) => void,
		onNotice: ((text: string) => void) | undefined,
		signal: AbortSignal,
	): Promise<void> {
		let stream = initial;
		let attempt = 0;
		const frameAttempts = new Map<string, number>();
		while (!signal.aborted) {
			let sawFrame = false;
			try {
				await pumpEventStream(
					stream,
					async (frame) => {
						const key = `${frame.machine}\0${frame.cursor}`;
						try {
							await onFrame(frame);
							frameAttempts.delete(key);
						} catch (error) {
							const attempts = (frameAttempts.get(key) ?? 0) + 1;
							if (attempts < MAX_FRAME_ATTEMPTS) {
								frameAttempts.set(key, attempts);
								throw error;
							}
							frameAttempts.delete(key);
							onNotice?.(
								`pij: skipped poison frame ${frameMessageId(frame)} after ${MAX_FRAME_ATTEMPTS} attempts: ${asError(error).message}; message remains in pij inbox`,
							);
						}
						cursors.set(frame.machine, frame.cursor);
						sawFrame = true;
					},
					signal,
				);
			} catch (error) {
				if (!signal.aborted) onError(asError(error));
			}
			if (signal.aborted) return;
			if (sawFrame) attempt = 0;

			this.reconnecting = true;
			onNotice?.("pij: daemon stream lost, reconnecting…");
			while (!signal.aborted) {
				const base = Math.min(INITIAL_RECONNECT_MS * 2 ** Math.min(attempt, 5), MAX_RECONNECT_MS);
				const random = this.deps.random?.() ?? Math.random();
				const delay = Math.min(
					MAX_RECONNECT_MS,
					Math.round(base * (1 - RECONNECT_JITTER + random * RECONNECT_JITTER * 2)),
				);
				await (this.deps.sleep ?? sleep)(delay, signal);
				if (signal.aborted) return;
				try {
					stream = await this.attachEvents(cursors, signal);
					this.reconnecting = false;
					let cursor = 0;
					for (const value of cursors.values()) cursor = Math.max(cursor, value);
					onNotice?.(`pij: re-attached at cursor ${cursor}`);
					attempt += 1;
					break;
				} catch (error) {
					if (isCursorRefusal(error) && cursors.size > 0) {
						onNotice?.(`pij: ${asError(error).message}; re-attaching live-only`);
						cursors.clear();
						try {
							stream = await this.attachEvents(cursors, signal);
							this.reconnecting = false;
							onNotice?.("pij: re-attached live-only");
							attempt += 1;
							break;
						} catch {
							// The live-only retry joins the same bounded reconnect loop.
						}
					}
					attempt += 1;
				}
			}
		}
	}

	private async attachEvents(
		cursors: ReadonlyMap<string, number>,
		signal: AbortSignal,
	): Promise<ReadableStream<Uint8Array>> {
		await this.refreshKey();
		const since =
			cursors.size === 0
				? ""
				: `?since=${encodeURIComponent(JSON.stringify(Object.fromEntries(cursors)))}`;
		const response = await this.request(`/v1/events${since}`, { signal });
		if (!response.ok) {
			const status = response.status;
			try {
				decodeEnvelope<unknown>(await response.text());
			} catch (error) {
				if (error instanceof DaemonRefusalError) throw error;
				throw new Error(`pij events failed with HTTP ${status}: ${asError(error).message}`);
			}
			throw new Error(`pij events failed with HTTP ${status}`);
		}
		if (!response.body) throw new Error("pij events returned no response body");
		return response.body;
	}

	private async get<T>(path: string): Promise<T> {
		const response = await this.request(path);
		return decodeEnvelope<T>(await response.text()).data;
	}

	private async post<T>(path: string, body: unknown, signal?: AbortSignal): Promise<T> {
		const response = await this.request(
			path,
			{ method: "POST", body: JSON.stringify(body), ...(signal === undefined ? {} : { signal }) },
			{ "Content-Type": "application/json" },
		);
		return decodeEnvelope<T>(await response.text()).data;
	}

	private async request(
		path: string,
		init: RequestInit = {},
		extraHeaders: Readonly<Record<string, string>> = {},
	): Promise<Response> {
		const request = (): Promise<Response> =>
			this.deps.fetch(`http://${this.location.addr}${path}`, {
				...init,
				headers: { ...this.headers(), ...extraHeaders },
			});
		let response = await request();
		if (response.status === 401 || response.status === 403) {
			await response.body?.cancel();
			await this.refreshKey();
			response = await request();
		}
		return response;
	}

	private headers(): { readonly Authorization: string } {
		return { Authorization: `Bearer ${this.key}` };
	}
}

async function pumpEventStream(
	stream: ReadableStream<Uint8Array>,
	onFrame: (frame: EventFrame) => unknown,
	signal: AbortSignal,
): Promise<void> {
	const reader = stream.getReader();
	const decoder = new TextDecoder();
	let buffered = "";
	let sawHello = false;
	const abort = (): void => {
		void reader.cancel();
	};
	signal.addEventListener("abort", abort, { once: true });
	try {
		while (!signal.aborted) {
			const { done, value } = await reader.read();
			if (done) break;
			buffered += decoder.decode(value, { stream: true });
			const lines = buffered.split("\n");
			buffered = lines.pop() ?? "";
			for (const line of lines) {
				if (line.trim() === "") continue;
				const decoded = decodeStreamLine(line);
				if ("hello" in decoded) {
					if (sawHello) throw new Error("pij events sent a second Hello line");
					sawHello = true;
					continue;
				}
				if (!sawHello) throw new Error("pij events frame arrived before Hello");
				await onFrame(decoded);
			}
		}
		if (buffered.trim() !== "") {
			const decoded = decodeStreamLine(buffered);
			if (!("hello" in decoded)) {
				if (!sawHello) throw new Error("pij events frame arrived before Hello");
				await onFrame(decoded);
			}
		}
	} finally {
		signal.removeEventListener("abort", abort);
		reader.releaseLock();
	}
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
	if (signal.aborted) return Promise.resolve();
	return new Promise((resolveSleep) => {
		let settled = false;
		const finish = (): void => {
			if (settled) return;
			settled = true;
			signal.removeEventListener("abort", finish);
			clearTimeout(timer);
			resolveSleep();
		};
		const timer = setTimeout(finish, ms);
		signal.addEventListener("abort", finish, { once: true });
	});
}

function isCursorRefusal(error: unknown): boolean {
	return (
		error instanceof DaemonRefusalError &&
		(error.kind === "cursor_reset" || error.kind === "refused")
	);
}

function frameMessageId(frame: EventFrame): string {
	try {
		const value: unknown = JSON.parse(frame.event.payload);
		if (typeof value === "object" && value !== null && !Array.isArray(value)) {
			const msgId = (value as Record<string, unknown>).msg_id;
			if (typeof msgId === "string" && msgId !== "") return msgId;
		}
	} catch {
		// The wire payload itself is the poison; the cursor still identifies it.
	}
	return `${frame.machine}:${frame.cursor}`;
}

function configuredValue(name: string, value: string | undefined, fallback: string): string {
	if (value === undefined) return fallback;
	if (value.trim() === "") throw invalidEnvironment(name, value, "value must not be empty");
	return value;
}

function assertDaemonAddr(value: string): void {
	const match = /^(?:\[[^\]]+\]|[^:/\s]+):(\d+)$/.exec(value);
	const port = match ? Number(match[1]) : Number.NaN;
	if (!Number.isInteger(port) || port < 1 || port > 65_535) {
		throw invalidEnvironment("PIJ_RS_ADDR", value, "expected host:port with port 1..65535");
	}
}

function invalidEnvironment(name: string, value: string, detail: string): Error {
	return new Error(`${name} has malformed value ${JSON.stringify(value)}: ${detail}`);
}
/** Exported so the generation router can stamp a caller's process start
 *  without minting a second answer to "when did this process begin". A pid is
 *  recycled at boot; the start time is what makes it identifying. */
export function observedProcessStart(pid: number): number {
	const row = execFileSync("ps", ["-o", "lstart=", "-p", String(pid)], {
		encoding: "utf8",
		env: { ...process.env, LC_ALL: "C" },
	}).trim();
	if (!row) throw new Error(`process ${pid} did not produce a start time`);
	return parseProcessStart(row);
}

function isConnectionRefused(error: unknown): boolean {
	let current: unknown = error;
	for (let depth = 0; depth < 5 && current !== null && typeof current === "object"; depth++) {
		const record = current as { readonly code?: unknown; readonly cause?: unknown };
		// Node fetch wraps ECONNREFUSED in cause.code; Bun reports ConnectionRefused
		// directly on the error. Pi and OMP exercise one shape each.
		if (record.code === "ECONNREFUSED" || record.code === "ConnectionRefused") return true;
		current = record.cause;
	}
	return false;
}

function asError(error: unknown): Error {
	return error instanceof Error ? error : new Error(String(error));
}
