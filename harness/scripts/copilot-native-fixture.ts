import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

export function object(value: unknown): Record<string, unknown> {
	if (value === null || typeof value !== "object" || Array.isArray(value)) {
		throw new Error("expected JSON object");
	}
	return value as Record<string, unknown>;
}

/** Never inherit seat identity, SDK attachment, global config or local-provider credentials. */
export function smokeEnvironment(
	inherited: NodeJS.ProcessEnv,
	home: string,
	copilotHome: string,
	state: string,
	addr: string,
	provider: "local" | "real",
): NodeJS.ProcessEnv {
	const env: NodeJS.ProcessEnv = {};
	for (const key of [
		"PATH",
		"LANG",
		"LC_ALL",
		"TERM",
		"TMPDIR",
		"SHELL",
		"USER",
		"LOGNAME",
		"SSL_CERT_FILE",
		"NODE_EXTRA_CA_CERTS",
	]) {
		if (inherited[key] !== undefined) env[key] = inherited[key];
	}
	if (provider === "real") {
		for (const key of [
			"COPILOT_GITHUB_TOKEN",
			"GH_TOKEN",
			"GITHUB_TOKEN",
			"GH_HOST",
			"COPILOT_GH_HOST",
			"HTTP_PROXY",
			"HTTPS_PROXY",
			"NO_PROXY",
		]) {
			if (inherited[key] !== undefined) env[key] = inherited[key];
		}
	}
	return {
		...env,
		HOME: home,
		XDG_CONFIG_HOME: `${home}/.config`,
		COPILOT_HOME: copilotHome,
		PIJ_RS_STATE_DIR: state,
		PIJ_RS_ADDR: addr,
		COPILOT_AUTO_UPDATE: "false",
		COPILOT_OFFLINE: provider === "local" ? "true" : "false",
		NO_PROXY: "127.0.0.1,localhost",
	};
}

export function sendPrompt(to: string, message: string): string {
	return `Native transport smoke. Call the pij_send tool exactly once with the following JSON, then say PIJ_NATIVE_DONE. Do not call other tools.\nPIJ_NATIVE_SEND:${JSON.stringify({ to, message })}`;
}

function content(message: Record<string, unknown>): string {
	if (typeof message.content === "string") return message.content;
	if (!Array.isArray(message.content)) return "";
	return message.content
		.map((part: unknown) => {
			const row = object(part);
			return typeof row.text === "string" ? row.text : "";
		})
		.join("\n");
}

/** Deliberately deterministic: it follows only our fixture marker, never performs inference. */
export function fixtureReply(input: unknown): Record<string, unknown> {
	const request = object(input);
	if (!Array.isArray(request.messages)) throw new Error("messages must be an array");
	const messages = request.messages.map(object);
	let lastUser = messages.length - 1;
	while (lastUser >= 0 && messages[lastUser]?.role !== "user") lastUser--;
	const instruction = lastUser < 0 ? "" : content(messages[lastUser] as Record<string, unknown>);
	const marker = /PIJ_NATIVE_SEND:(\{[^\n]+\})/.exec(instruction);
	if (marker && !messages.slice(lastUser + 1).some((message) => message.role === "tool")) {
		const args = object(JSON.parse(marker[1] as string));
		if (typeof args.to !== "string" || typeof args.message !== "string")
			throw new Error("invalid fixture send marker");
		const tools = Array.isArray(request.tools) ? request.tools.map(object) : [];
		const tool = tools.map((row) => object(row.function)).find((row) => row.name === "pij_send");
		if (!tool) throw new Error("PIJ_NATIVE_TOOL_MISSING: actual pij_send tool was not advertised");
		return {
			role: "assistant",
			content: null,
			tool_calls: [
				{
					id: `call_${randomUUID()}`,
					type: "function",
					function: { name: "pij_send", arguments: JSON.stringify(args) },
				},
			],
		};
	}
	return { role: "assistant", content: marker ? "PIJ_NATIVE_DONE" : "PIJ_NATIVE_OBSERVED" };
}

export async function startFixtureProvider(
	options: { beforeReply?: (body: Record<string, unknown>) => Promise<void> } = {},
): Promise<{
	url: string;
	requests: string[];
	close: () => Promise<void>;
}> {
	const requests: string[] = [];
	const server = createServer(async (request, response) => {
		try {
			requests.push(`${request.method} ${request.url}`);
			if (request.method === "GET" && request.url?.endsWith("/models")) {
				response.setHeader("Content-Type", "application/json");
				response.end(
					JSON.stringify({
						object: "list",
						data: [
							{
								id: "pij-native-fixture",
								object: "model",
								owned_by: "deterministic-local-fixture",
							},
						],
					}),
				);
				return;
			}
			if (request.method !== "POST" || !request.url?.endsWith("/chat/completions")) {
				response.writeHead(404).end("fixture supports only /v1/chat/completions and /v1/models");
				return;
			}
			const chunks: Buffer[] = [];
			let size = 0;
			for await (const chunk of request) {
				const bytes = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
				size += bytes.length;
				if (size > 4 * 1024 * 1024) throw new Error("fixture request exceeds 4 MiB");
				chunks.push(bytes);
			}
			const body = object(JSON.parse(Buffer.concat(chunks).toString("utf8")));
			const message = fixtureReply(body);
			const finish = message.tool_calls ? "tool_calls" : "stop";
			const base = {
				id: `chatcmpl-${randomUUID()}`,
				created: Math.floor(Date.now() / 1000),
				model: "pij-native-fixture",
			};
			if (body.stream) {
				response.writeHead(200, {
					"Content-Type": "text/event-stream",
					"Cache-Control": "no-cache",
				});
				const delta = { ...message };
				if (Array.isArray(delta.tool_calls))
					delta.tool_calls = delta.tool_calls.map((call, index) => ({ ...object(call), index }));
				response.write(
					`data: ${JSON.stringify({ ...base, object: "chat.completion.chunk", choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`,
				);
				await options.beforeReply?.(body);
				response.end(
					`data: ${JSON.stringify({ ...base, object: "chat.completion.chunk", choices: [{ index: 0, delta: {}, finish_reason: finish }] })}\n\ndata: [DONE]\n\n`,
				);
			} else {
				await options.beforeReply?.(body);
				response.setHeader("Content-Type", "application/json");
				response.end(
					JSON.stringify({
						...base,
						object: "chat.completion",
						choices: [{ index: 0, message, finish_reason: finish }],
						usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
					}),
				);
			}
		} catch (error) {
			response.writeHead(400, { "Content-Type": "application/json" }).end(
				JSON.stringify({
					error: { message: error instanceof Error ? error.message : String(error) },
				}),
			);
		}
	});
	await new Promise<void>((resolve, reject) => {
		server.once("error", reject);
		server.listen(0, "127.0.0.1", resolve);
	});
	return {
		url: `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1`,
		requests,
		close: () =>
			new Promise<void>((resolve, reject) => {
				server.closeAllConnections();
				server.close((error) => (error ? reject(error) : resolve()));
			}),
	};
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
	const fixture = await startFixtureProvider();
	console.log(JSON.stringify({ provider: "deterministic-local-fixture", url: fixture.url }));
	process.once("SIGINT", () => {
		void fixture.close();
	});
	process.once("SIGTERM", () => {
		void fixture.close();
	});
}
