import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { copyFileSync, mkdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath, pathToFileURL } from "node:url";

import type {
	CommandResult,
	OwnedProcess,
	PrivateGovernanceFixture,
} from "./governance-fixture.js";

type EnvelopeReader = (result: CommandResult) => Record<string, unknown>;
const REQUEST = 15_000;

function object(value: unknown): Record<string, unknown> {
	assert(value !== null && typeof value === "object" && !Array.isArray(value));
	return value as Record<string, unknown>;
}

async function frame(
	host: OwnedProcess,
	key: string,
	value: number,
): Promise<Record<string, unknown>> {
	const deadline = Date.now() + REQUEST;
	for (;;) {
		const { stdout, stderr } = host.output();
		for (const line of stdout.split("\n").slice(0, -1)) {
			const candidate = object(JSON.parse(line));
			if (candidate[key] === value) return candidate;
		}
		assert(
			host.child.exitCode === null && host.child.signalCode === null,
			`protocol host exited: ${stdout}\n${stderr}`,
		);
		assert(
			Date.now() < deadline,
			`protocol host did not emit ${key}=${value}: ${stdout}\n${stderr}`,
		);
		await delay(20);
	}
}

class ExternalHost {
	private ticket = 0;

	constructor(
		readonly process: OwnedProcess,
		readonly session: string,
	) {}

	async run(args: readonly string[]): Promise<CommandResult & { pid: number }> {
		const id = ++this.ticket;
		const input = this.process.child.stdin;
		assert(input);
		await new Promise<void>((resolve, reject) => {
			input.write(`${JSON.stringify({ id, args })}\n`, (error) =>
				error ? reject(error) : resolve(),
			);
		});
		const response = await frame(this.process, "id", id);
		assert.equal(response.failure, undefined, JSON.stringify(response));
		assert.equal(response.timedOut, false, JSON.stringify(response));
		assert(typeof response.stdout === "string" && typeof response.stderr === "string");
		assert(response.status === null || typeof response.status === "number");
		assert(typeof response.pid === "number" && response.pid > 0);
		assert.notEqual(response.pid, this.process.child.pid, "CLI wrapper is not the native host");
		return {
			status: response.status,
			stdout: response.stdout,
			stderr: response.stderr,
			pid: response.pid,
		};
	}

	started(): Promise<Record<string, unknown>> {
		return frame(this.process, "started", this.ticket);
	}
}

async function openHost(fixture: PrivateGovernanceFixture, name: string): Promise<ExternalHost> {
	const directory = join(fixture.root, "hosts", name, "@anthropic-ai", "claude-code");
	mkdirSync(directory, { recursive: true });
	writeFileSync(join(directory, "package.json"), JSON.stringify({ private: true, type: "module" }));
	const entry = join(directory, "cli.js");
	copyFileSync(new URL("./governance-host.mjs", import.meta.url), entry);
	const session = randomUUID();
	const env: NodeJS.ProcessEnv = { ...fixture.env, CLAUDE_CODE_SESSION_ID: session };
	delete env.TMUX;
	delete env.TMUX_PANE;
	delete env.PIJ_SESSION_ID;
	const loader = pathToFileURL(createRequire(import.meta.url).resolve("tsx")).href;
	const shim = new URL("../../.omp/extensions/pij/cli.ts", import.meta.url);
	const host = fixture.start(
		process.execPath,
		[entry, loader, fileURLToPath(shim), "--session-id", session],
		env,
	);
	assert(host.child.pid);
	const ready = await frame(host, "ready", host.child.pid);
	assert.equal(ready.fixture, "protocol-only-no-model");
	const observed = await fixture.run(
		"ps",
		["-ww", "-p", String(host.child.pid), "-o", "command="],
		env,
	);
	assert.equal(observed.status, 0, observed.stderr);
	assert(
		observed.stdout.trim().startsWith(`${process.execPath} ${entry} `),
		`actual host argv: ${observed.stdout}`,
	);
	return new ExternalHost(host, session);
}

export async function exerciseExternalRoundtrip(
	fixture: PrivateGovernanceFixture,
	wire: EnvelopeReader,
): Promise<void> {
	// Both are real, long-lived OS processes, but neither runs a model. Their
	// package-shaped entry paths exercise u2's native ancestry admission law.
	const sender = await openHost(fixture, "sender");
	const recipient = await openHost(fixture, "recipient");
	assert.notEqual(sender.process.child.pid, recipient.process.child.pid);
	const register = async (host: ExternalHost): Promise<Record<string, unknown>> => {
		const result = await host.run(["inbox", "register"]);
		const data = object(wire(result).data);
		assert.equal(data.harness, "claude");
		assert.equal(data.session, host.session);
		assert.equal(data.pane, null);
		assert.equal(data.native_extension_delivery, false);
		const identity = object(data.proc);
		assert.equal(identity.pid, host.process.child.pid);
		assert.notEqual(
			identity.pid,
			result.pid,
			"registration must retain the host, not the exited wrapper",
		);
		assert(typeof identity.proc_start === "number" && identity.proc_start > 0);
		assert(typeof data.id === "string" && data.id.length > 0);
		return data;
	};
	const from = await register(sender);
	const to = await register(recipient);
	assert.equal(from.binding, "created");
	assert.equal(to.binding, "created");
	assert.notEqual(from.id, to.id);
	const repeat = await register(recipient);
	assert.equal(repeat.binding, "same");
	assert.equal(repeat.id, to.id);
	assert.deepEqual(repeat.proc, to.proc);
	assert.deepEqual(wire(await recipient.run(["inbox", "--wait", "20"])).data, []);

	const waiting = recipient.run(["inbox", "--wait", "5000"]);
	// Avoid an unhandled rejection while the independent sender is running;
	// awaiting waiting below still propagates the original failure.
	void waiting.catch(() => {});
	const started = await recipient.started();
	assert(typeof started.pid === "number", "actual recipient CLI child started before send");
	const body = `private external roundtrip ${randomUUID()}`;
	const sent = object(wire(await sender.run(["send", String(to.id), body])).data);
	assert(typeof sent.msg_id === "string");
	const claims = wire(await waiting).data;
	assert(Array.isArray(claims) && claims.length === 1);
	const claim = object(claims[0]);
	assert(typeof claim.job_id === "number" && claim.job_id > 0);
	const message = object(claim.message);
	assert.equal(message.msg_id, sent.msg_id);
	assert.equal(message.from, from.id);
	assert.equal(message.to, to.id);
	assert.equal(message.body, body);
	assert.deepEqual(
		wire(await recipient.run(["inbox", "--wait", "20"])).data,
		[],
		"stdout-complete read acknowledged the real Rust claim",
	);

	const replyBody = `reply to ${sent.msg_id}`;
	const reply = object(
		wire(await recipient.run(["send", String(from.id), replyBody, "--in-reply-to", sent.msg_id]))
			.data,
	);
	const replies = wire(await sender.run(["inbox", "--wait", "5000"])).data;
	assert(Array.isArray(replies) && replies.length === 1);
	const received = object(object(replies[0]).message);
	assert.equal(received.msg_id, reply.msg_id);
	assert.equal(received.in_reply_to, sent.msg_id);
	assert.equal(received.from, to.id);
	assert.equal(received.to, from.id);
	assert.equal(received.body, replyBody);
	assert.deepEqual(wire(await sender.run(["inbox", "--wait", "20"])).data, []);
	const final = await register(sender);
	assert.equal(final.binding, "same");
	assert.equal(final.id, from.id);
	assert.deepEqual(final.proc, from.proc);
	await Promise.all([sender.process.stop(), recipient.process.stop()]);
	console.error(
		JSON.stringify({
			smoke: "paneless-external",
			from: from.id,
			to: to.id,
			senderHost: from.proc,
			recipientHost: to.proc,
			sent: sent.msg_id,
			reply: reply.msg_id,
			modelCanary: "unproven-protocol-hosts",
		}),
	);
}
