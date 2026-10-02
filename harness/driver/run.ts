#!/usr/bin/env tsx
// harness/driver/run.ts
//
// Agent-facing CLI. Minih agents (notably extension-validator) shell into
// this with a JSON scenario; the runner prints a JSON RunReport.
//
// Usage:
//   npx tsx harness/driver/run.ts --scenario <path-to-json>
//   echo '{...}' | npx tsx harness/driver/run.ts --stdin
//
// Exit codes:
//   0 — RunReport.ok === true
//   1 — RunReport.ok === false (assertion / boot / idle / preflight)
//   2 — bad invocation (missing args, malformed JSON, unrecognised flag)

import { readFileSync } from "node:fs";

import { runScenario, type Scenario } from "./index.js";

interface CliArgs {
	scenario?: string;
	stdin?: boolean;
}

function parseArgs(argv: string[]): CliArgs {
	const out: CliArgs = {};
	for (let i = 0; i < argv.length; i++) {
		const arg = argv[i];
		if (arg === "--scenario") {
			const next = argv[++i];
			if (typeof next === "string") out.scenario = next;
		} else if (arg === "--stdin") {
			out.stdin = true;
		}
	}
	return out;
}

async function main(): Promise<void> {
	const args = parseArgs(process.argv.slice(2));
	let raw: string;
	if (args.stdin) {
		raw = readFileSync(0, "utf8");
	} else if (args.scenario) {
		raw = readFileSync(args.scenario, "utf8");
	} else {
		console.error("usage: --scenario <path> | --stdin");
		process.exit(2);
	}

	let scenario: Scenario;
	try {
		scenario = JSON.parse(raw) as Scenario;
	} catch (err) {
		console.error(`bad JSON: ${(err as Error).message}`);
		process.exit(2);
	}

	// runScenario() hydrates JSON-regex {source, flags?} → RegExp internally,
	// so JSON consumers don't need to pre-hydrate. Native RegExp would also
	// pass through unchanged for in-process callers.
	const report = await runScenario(scenario, { cwd: process.cwd() });
	console.log(JSON.stringify(report, null, 2));
	process.exit(report.ok ? 0 : 1);
}

main().catch((err: Error) => {
	console.error(err.message);
	process.exit(2);
});
