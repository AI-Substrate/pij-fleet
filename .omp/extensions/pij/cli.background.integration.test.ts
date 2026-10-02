import { execFile } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";
import { describe, expect, it } from "vitest";

const execute = promisify(execFile);
const CLI = join(import.meta.dirname, "cli.ts");
const TSX = join(import.meta.dirname, "..", "..", "..", "node_modules", ".bin", "tsx");

async function withDaemon(
	rawEnvelope: string,
	status: number,
	run: (env: NodeJS.ProcessEnv, requests: Record<string, unknown>[]) => Promise<void>,
): Promise<void> {
	const home = mkdtempSync(join(tmpdir(), "pij-bg-shim-"));
	writeFileSync(join(home, "daemon.key"), "bg-test-key");
	const requests: Record<string, unknown>[] = [];
	const server = createServer(async (request, response) => {
		response.setHeader("Content-Type", "application/json");
		if (request.url === "/health") {
			response.end(
				JSON.stringify({
					ok: true,
					command: "pij ping",
					v: 2,
					data: { status: "healthy", build: "pij-rs test", offline: false, machine: "test" },
				}),
			);
		} else if (request.url === "/v1/seats") {
			response.end(
				JSON.stringify({
					ok: true,
					command: "pij list",
					v: 2,
					data: { seats: [{ id: "pij-bg-owner" }] },
				}),
			);
		} else if (request.url === "/v1/bg" && request.method === "POST") {
			let body = "";
			for await (const chunk of request) body += String(chunk);
			requests.push(JSON.parse(body) as Record<string, unknown>);
			response.writeHead(status);
			response.end(rawEnvelope);
		} else {
			response.writeHead(404);
			response.end();
		}
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	const address = server.address();
	if (address === null || typeof address === "string") throw new Error("missing listener address");
	try {
		await run(
			{
				...process.env,
				HOME: home,
				PIJ_HOME: join(home, "legacy"),
				CLAUDE_CONFIG_DIR: join(home, "claude"),
				PIJ_RS_STATE_DIR: home,
				PIJ_RS_ADDR: `127.0.0.1:${address.port}`,
				PIJ_DAEMON_GENERATION: "rs",
				PIJ_SESSION_ID: "pij-bg-owner",
				TMUX_PANE: "",
				PIJ_ROUTE_DIAGNOSTIC: "0",
			},
			requests,
		);
	} finally {
		await new Promise<void>((resolve, reject) =>
			server.close((error) => (error ? reject(error) : resolve())),
		);
		rmSync(home, { recursive: true, force: true });
	}
}

describe("bg shipped shim JSON output", () => {
	it("prints the shared native envelope with --json before or after the leaf, and data.line for humans", async () => {
		const rawEnvelope = readFileSync(
			new URL("../../../crates/testkit/fixtures/golden/cli/bg-list-envelope.json", import.meta.url),
			"utf8",
		);
		await withDaemon(rawEnvelope, 200, async (env, requests) => {
			for (const argv of [
				["--json", "bg", "list", "--all"],
				["bg", "--json", "list", "--all"],
				["bg", "list", "--all", "--json"],
				["bg", "list", "--all"],
			]) {
				const { stdout } = await execute(TSX, [CLI, ...argv], { env, timeout: 10_000 });
				expect(stdout).toBe(
					`${argv.includes("--json") ? rawEnvelope : JSON.parse(rawEnvelope).data.line}\n`,
				);
				const request = requests.pop();
				expect(request).toMatchObject({ caller: { pijSessionId: "pij-bg-owner" } });
				expect(request?.owner).toBeUndefined();
				expect((request?.argv as string[]).filter((arg) => arg !== "--json")).toEqual([
					"bg",
					"list",
					"--all",
				]);
			}
		});
	}, 60_000);

	it("prints an unchanged refusal envelope to stdout and exits unsuccessfully", async () => {
		const rawEnvelope =
			'{"ok":false,"command":"pij bg kill","v":2,"meta":"not the owner or recorded parent","error":"refused"}';
		await withDaemon(rawEnvelope, 403, async (env) => {
			const result = await execute(TSX, [CLI, "bg", "kill", "bg-other", "--json"], {
				env,
				timeout: 10_000,
			}).then(
				(output) => ({ ...output, code: 0 }),
				(error: { stdout: string; stderr: string; code: number }) => error,
			);
			expect(result.code).not.toBe(0);
			expect(result.stdout).toBe(`${rawEnvelope}\n`);
		});
	}, 20_000);
});
