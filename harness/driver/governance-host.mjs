// Executed only after copying beneath a PRIVATE @anthropic-ai/claude-code/cli.js.
// This is an active process/CLI protocol fixture, never a model or provider.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createInterface } from "node:readline";

const [loader, shim] = process.argv.slice(2);
assert(loader && shim);
const input = createInterface({ input: process.stdin });
let active;
let closing = false;
const emit = (frame) =>
	new Promise((resolve, reject) => {
		process.stdout.write(`${JSON.stringify(frame)}\n`, (error) =>
			error ? reject(error) : resolve(),
		);
	});
const stop = () => {
	closing = true;
	input.close();
	// Parent cleanup has its own bound; do not leave a CLI descendant behind.
	active?.kill("SIGKILL");
};
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
await emit({ ready: process.pid, fixture: "protocol-only-no-model" });
for await (const line of input) {
	const request = JSON.parse(line);
	assert(Number.isSafeInteger(request.id) && Array.isArray(request.args));
	assert(request.args.every((arg) => typeof arg === "string"));
	if (closing) break;
	active = spawn(process.execPath, ["--import", loader, shim, ...request.args, "--json"], {
		cwd: process.cwd(),
		env: process.env,
		stdio: ["ignore", "pipe", "pipe"],
	});
	const child = active;
	let stdout = "";
	let stderr = "";
	let failure;
	let timedOut = false;
	child.stdout.setEncoding("utf8").on("data", (chunk) => {
		stdout += chunk;
	});
	child.stderr.setEncoding("utf8").on("data", (chunk) => {
		stderr += chunk;
	});
	child.on("error", (error) => {
		failure = error.message;
	});
	const completion = new Promise((resolve) => child.once("close", resolve));
	const timer = setTimeout(() => {
		timedOut = true;
		child.kill("SIGKILL");
	}, 10_000);
	try {
		await emit({ started: request.id, pid: child.pid });
		const status = await completion;
		await emit({ id: request.id, pid: child.pid, status, stdout, stderr, failure, timedOut });
	} finally {
		clearTimeout(timer);
		if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
		await completion;
		active = undefined;
	}
	if (closing) break;
}
input.close();
