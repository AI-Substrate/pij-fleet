#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const path = require("node:path");

const tsxCli = require.resolve("tsx/cli");
const pijCli = path.resolve(__dirname, "..", "..", ".omp", "extensions", "pij", "cli.ts");
const result = spawnSync(process.execPath, [tsxCli, pijCli, ...process.argv.slice(2)], {
	stdio: "inherit",
	env: { ...process.env, NODE_NO_WARNINGS: "1" },
});

if (result.error) {
	console.error(`pij: ${result.error.message}`);
	process.exit(1);
}

process.exit(result.status ?? 1);
