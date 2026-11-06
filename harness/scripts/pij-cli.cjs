#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const { constants } = require("node:os");
const path = require("node:path");

const args = process.argv.slice(2);

// Shim send flags that take a value, so a value is never read as the recipient.
const SEND_VALUE_FLAGS = new Set([
	"--body-file",
	"--reason",
	"--command",
	"--in-reply-to",
	"--reply",
	"--limit",
	"--project",
	"--file",
	"--caption",
	"--to",
]);

/**
 * Plan 213: pij-rs's community verbs, and sends it can name a sender for, go
 * straight to pij-rs before tsx loads: argv, stdin, stdout, stderr and the exit
 * code are pij-rs's own. Everything else, and any explicit non-rs generation,
 * keeps the routed path below.
 */
function forwardsToPijRs(argv, env) {
	const generation = env.PIJ_DAEMON_GENERATION;
	if (generation !== undefined && generation !== "" && generation !== "rs") return false;
	const [verb, ...rest] = argv;
	if (require("./pij-native-verbs.json").verbs.includes(verb)) return true;
	if (verb !== "send") return false;
	if (rest.includes("--help") || rest.includes("-h")) return true;
	// pij-rs names the sender from PIJ_SESSION_ID or TMUX_PANE; a paneless host
	// keeps the routed path, which registers it first.
	if (!env.PIJ_SESSION_ID && !env.TMUX_PANE) return false;
	for (let index = 0; index < rest.length; index++) {
		const token = rest[index];
		if (token === "--to" || token.startsWith("--to=")) return true;
		if (SEND_VALUE_FLAGS.has(token)) index++;
		else if (!token.startsWith("-")) return token.startsWith("role:");
	}
	return false;
}

function exitCode(result) {
	if (result.status !== null) return result.status;
	const signal = result.signal === null ? undefined : constants.signals[result.signal];
	return signal === undefined ? 1 : 128 + signal;
}

if (forwardsToPijRs(args, process.env)) {
	const result = spawnSync("pij-rs", args, { stdio: "inherit" });
	if (result.error) {
		const why =
			result.error.code === "ENOENT"
				? "pij-rs is not on PATH (run `just rs-install`)"
				: result.error.message;
		console.error(`pij: cannot run pij-rs: ${why}`);
		process.exit(127);
	}
	process.exit(exitCode(result));
}

const tsxCli = require.resolve("tsx/cli");
const pijCli = path.resolve(__dirname, "..", "..", ".omp", "extensions", "pij", "cli.ts");
const result = spawnSync(process.execPath, [tsxCli, pijCli, ...args], {
	stdio: "inherit",
	env: { ...process.env, NODE_NO_WARNINGS: "1" },
});

if (result.error) {
	console.error(`pij: ${result.error.message}`);
	process.exit(1);
}

process.exit(result.status ?? 1);
