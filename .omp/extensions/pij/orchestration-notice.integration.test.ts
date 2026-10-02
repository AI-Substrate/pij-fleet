// Plan 139: retain all eight legacy keeper cases at the current caller boundary.
// Prime ruling: native baton.request must send through DeliveryService and return
// data.notice; missing/dissolved keeper means null and no send. This scripted HTTP
// fixture proves argv/caller and complete receipt forwarding, NOT native delivery.
// Native parity must separately prove one send for the other six cases and zero
// for missing/dissolved keepers. Never recreate the retired CliBatonNoticeSink here.
import { execFile } from "node:child_process";
import {
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	realpathSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { describe, expect, it } from "vitest";
import type { HarnessKind, SessionDescriptor } from "./core/types.js";

const CLI = join(import.meta.dirname, "cli.ts");
const TSX = createRequire(import.meta.url).resolve("tsx");
const execute = promisify(execFile);
const AT = Date.parse("2026-09-07T12:00:00.000Z");

type Notice = "delivered" | "queued" | "unverified" | null;

function descriptor(input: {
	readonly state: "idle" | "working";
	readonly pid: number;
	readonly harness?: HarnessKind;
	readonly lastTickAt?: string;
	readonly deliveryMode?: SessionDescriptor["deliveryMode"];
	readonly lifecycle?: SessionDescriptor["lifecycle"];
}): SessionDescriptor {
	return {
		id: "pij-target",
		folder: "/repo",
		dataDir: "/tmp/pij-target",
		eventsPath: "/tmp/pij-target/events.ndjson",
		pid: input.pid,
		startedAt: "2026-07-11T09:00:00.000Z",
		state: input.state,
		...(input.harness ? { harness: input.harness } : {}),
		...(input.lastTickAt ? { lastTickAt: input.lastTickAt } : {}),
		...(input.deliveryMode ? { deliveryMode: input.deliveryMode } : {}),
		...(input.lifecycle ? { lifecycle: input.lifecycle } : {}),
	};
}

function snapshot(home: string): Record<string, string> {
	return Object.fromEntries(
		readdirSync(home)
			.sort()
			.map((name) => [name, readFileSync(join(home, name)).toString("base64")]),
	);
}

describe("baton keeper notice caller parity — scripted rs wire, native delivery separately required", () => {
	it.each<{ readonly label: string; readonly target?: SessionDescriptor; readonly notice: Notice }>(
		[
			// #374 honest-receipt rule: an idle Pi extension stream stays queued until ReaderRead.
			{
				label: "queued for an idle live pi target until ReaderRead",
				target: descriptor({ state: "idle", pid: process.pid }),
				notice: "queued",
			},
			{
				label: "queued for a working live pi target",
				target: descriptor({ state: "working", pid: process.pid }),
				notice: "queued",
			},
			{
				label: "queued for a live control-plane target with a fresh heartbeat",
				target: descriptor({
					state: "idle",
					pid: process.pid,
					harness: "codex",
					lastTickAt: new Date(AT + 60_000).toISOString(),
				}),
				notice: "queued",
			},
			// Native stale evidence is Dead/Recycled (proc_start-aware), never heartbeat age; this legacy metadata is only a decoy.
			{
				label: "unverified for a stale native incarnation (legacy control-plane case)",
				target: descriptor({
					state: "idle",
					pid: process.pid,
					harness: "copilot",
					lastTickAt: "2026-01-01T00:00:00.000Z",
				}),
				notice: "unverified",
			},
			{
				label: "unverified for a dead target",
				target: descriptor({ state: "idle", pid: 2_147_483_647 }),
				notice: "unverified",
			},
			{ label: "no notice or send for a missing target", notice: null },
			{
				label: "no notice or send for a dissolved pull target",
				target: descriptor({
					state: "idle",
					pid: process.pid,
					harness: "copilot",
					deliveryMode: "pull",
					lifecycle: "dissolved",
				}),
				notice: null,
			},
			{
				label: "queued with one native notice for a live pull target",
				target: descriptor({
					state: "idle",
					pid: process.pid,
					harness: "copilot",
					deliveryMode: "pull",
					lifecycle: "bound",
				}),
				notice: "queued",
			},
		],
	)("$label", { timeout: 30_000 }, async ({ target, notice }) => {
		const root = realpathSync(mkdtempSync(join(tmpdir(), "pij-baton-notice-")));
		const legacyHome = join(root, "legacy");
		const stateDir = join(root, "rs");
		mkdirSync(legacyHome);
		mkdirSync(stateDir);
		writeFileSync(join(stateDir, "daemon.key"), "private-notice-key");
		// Old filesystem state is a decoy, never the source of an rs notice verdict.
		if (target) writeFileSync(join(legacyHome, "pij-target.json"), JSON.stringify(target));
		const before = snapshot(legacyHome);
		const defineArgs = [
			"orchestration",
			"baton",
			"define",
			"git-index",
			"--resource",
			"shared git index",
			"--json",
		];
		const requestArgs = [
			"orchestration",
			"baton",
			"request",
			"git-index",
			"--purpose",
			"stage the commit",
			"--json",
		];
		const defined = JSON.stringify({
			ok: true,
			command: "pij orchestration",
			v: 2,
			data: {
				baton: {
					name: "git-index",
					description: "shared git index",
					resource: "shared git index",
					probe: null,
					repo: root,
					created_by: "pij-target",
					created_at: AT,
				},
				seq: 1,
			},
		});
		const requested = ` ${JSON.stringify({
			ok: true,
			command: "pij orchestration",
			v: 2,
			data: {
				request: {
					id: "request-notice-proof",
					baton: "git-index",
					requester: "pij-requester",
					purpose: "stage the commit",
					pin: null,
					evidence: null,
					requested_at: AT,
					state: "requested",
				},
				seq: 2,
				notice,
			},
			meta: "native notice result",
			future: { retained: true },
		})}\n`;
		const requests: { method?: string; authorization?: string; body: unknown }[] = [];
		const server = createServer((request, response) => {
			void (async () => {
				response.setHeader("Content-Type", "application/json");
				if (request.url === "/health") {
					response.end(
						JSON.stringify({
							ok: true,
							command: "health",
							v: 2,
							data: { status: "ok", build: "fixture", offline: true, machine: "private" },
						}),
					);
					return;
				}
				if (request.url !== "/v1/orchestration") {
					response.writeHead(404).end();
					return;
				}
				let body = "";
				for await (const chunk of request) body += String(chunk);
				requests.push({
					method: request.method,
					authorization: request.headers.authorization,
					body: JSON.parse(body),
				});
				response.end(requests.length === 1 ? defined : requested);
			})().catch((error: Error) => response.destroy(error));
		});
		try {
			await new Promise<void>((resolve, reject) => {
				server.once("error", reject);
				server.listen(0, "127.0.0.1", resolve);
			});
			const address = server.address();
			if (address === null || typeof address === "string") throw new Error("missing private port");
			const run = (args: readonly string[], actor: string) =>
				execute(process.execPath, ["--import", TSX, CLI, ...args], {
					cwd: root,
					timeout: 10_000,
					killSignal: "SIGKILL",
					env: {
						PATH: process.env.PATH,
						HOME: root,
						USERPROFILE: root,
						CLAUDE_CONFIG_DIR: join(root, "claude"),
						XDG_CONFIG_HOME: join(root, "config"),
						TSX_DISABLE_CACHE: "1",
						PIJ_HOME: legacyHome,
						PIJ_SESSION_ID: actor,
						PIJ_RS_STATE_DIR: stateDir,
						PIJ_RS_ADDR: `127.0.0.1:${address.port}`,
					},
				});
			const definition = await run(defineArgs, "pij-target");
			expect(definition.stderr).toBe("");
			expect(definition.stdout).toBe(`${defined}\n`);
			const request = await run(requestArgs, "pij-requester");
			expect(request.stderr).toBe("");
			expect(request.stdout).toBe(requested);
			expect(JSON.parse(request.stdout)).toMatchObject({
				ok: true,
				v: 2,
				data: { request: { requester: "pij-requester", state: "requested" }, seq: 2, notice },
			});
			expect(requests).toEqual([
				{
					method: "POST",
					authorization: "Bearer private-notice-key",
					body: {
						argv: defineArgs,
						caller: expect.objectContaining({ pijSessionId: "pij-target", cwd: root }),
					},
				},
				{
					method: "POST",
					authorization: "Bearer private-notice-key",
					body: {
						argv: requestArgs,
						caller: expect.objectContaining({ pijSessionId: "pij-requester", cwd: root }),
					},
				},
			]);
			expect(snapshot(legacyHome)).toEqual(before);
		} finally {
			if (server.listening)
				await new Promise<void>((resolve, reject) => {
					server.close((error) => (error ? reject(error) : resolve()));
					server.closeAllConnections();
				});
			rmSync(root, { recursive: true, force: true });
		}
	});
});
