#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const { accessSync, constants: fsConstants, statSync } = require("node:fs");
const { constants } = require("node:os");
const path = require("node:path");

const args = process.argv.slice(2);

/** `--name=value` splits into the name and its inline value. */
function splitFlag(token) {
	const equals = token.indexOf("=");
	return equals === -1 ? [token, undefined] : [token.slice(0, equals), token.slice(equals + 1)];
}

/** The verb and what follows it, past pij-rs's global options. */
function findVerb(argv, native) {
	for (let index = 0; index < argv.length; index++) {
		const [name, inline] = splitFlag(argv[index]);
		if (!name.startsWith("-")) return { verb: name, rest: argv.slice(index + 1) };
		if (name === "--json" && inline === undefined) continue;
		if (!native.global_value_flags.includes(name)) return undefined;
		if (inline === undefined) index++;
	}
	return undefined;
}

/** A send's recipient positionals, --to, help and shim-only flags, with each flag's value consumed. */
function scanSend(rest, native) {
	const scan = { positionals: [], to: undefined, help: false, routed: false };
	for (let index = 0; index < rest.length; index++) {
		const token = rest[index];
		if (token === "--") {
			scan.positionals.push(...rest.slice(index + 1));
			break;
		}
		if (token === "--help" || token === "-h") {
			scan.help = true;
			continue;
		}
		if (!token.startsWith("-") || token === "-") {
			scan.positionals.push(token);
			continue;
		}
		const [name, inline] = splitFlag(token);
		const routed = native.routed_send_value_flags.includes(name);
		if (routed) scan.routed = true;
		const takesValue =
			routed || native.send_value_flags.includes(name) || native.global_value_flags.includes(name);
		const value = inline ?? (takesValue ? rest[++index] : undefined);
		if (name === "--to") scan.to = value ?? "";
	}
	return scan;
}

/**
 * Plan 213: pij-rs's community verbs, and sends from a tmux pane to a role: or
 * with --to, go straight to pij-rs before tsx loads: argv, stdin, stdout,
 * stderr and the exit code are pij-rs's own. Everything else, and any explicit
 * non-rs generation, keeps the routed path below.
 */
function forwardsToPijRs(argv, env, native) {
	const generation = env.PIJ_DAEMON_GENERATION;
	if (generation !== undefined && generation !== "" && generation !== "rs") return false;
	const found = findVerb(argv, native);
	if (found === undefined) return false;
	if (native.verbs.includes(found.verb)) return true;
	if (found.verb !== "send") return false;
	const scan = scanSend(found.rest, native);
	if (scan.help) return true;
	// Only a pane is an identity the daemon observes. A paneless send keeps the
	// routed path, which registers the observed host first and never trusts an
	// inherited PIJ_SESSION_ID (review of #147, F002).
	if (!env.TMUX_PANE || scan.routed) return false;
	return scan.to !== undefined || (scan.positionals[0] ?? "").startsWith("role:");
}

/** pij-rs from the first absolute PATH entry that holds it; a relative entry is skipped. */
function resolvePijRs(env) {
	for (const dir of (env.PATH ?? "").split(path.delimiter)) {
		if (!path.isAbsolute(dir)) continue;
		const candidate = path.join(dir, "pij-rs");
		try {
			if (!statSync(candidate).isFile()) continue;
			accessSync(candidate, fsConstants.X_OK);
			return candidate;
		} catch {
			/* not here */
		}
	}
	return undefined;
}

function exitCode(result) {
	if (result.status !== null) return result.status;
	const signal = result.signal === null ? undefined : constants.signals[result.signal];
	return signal === undefined ? 1 : 128 + signal;
}

if (forwardsToPijRs(args, process.env, require("./pij-native-verbs.json"))) {
	const pijRs = resolvePijRs(process.env);
	if (pijRs === undefined) {
		console.error("pij: cannot run pij-rs: it is not on PATH (run `just rs-install`)");
		process.exit(127);
	}
	const result = spawnSync(pijRs, args, { stdio: "inherit" });
	if (result.error) {
		console.error(`pij: cannot run ${pijRs}: ${result.error.message}`);
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
