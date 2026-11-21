#!/usr/bin/env node
// D-F208-2: `just update-copilot`, the one way to take a Copilot CLI update.
//
// F208: Copilot 1.0.113 changed its SDK's config shape, and every seat that
// reloaded onto it silently lost pij_send and its hooks. So an update is
// checked before any seat reloads: update, then run the installed-SDK contract
// on the new version (copilot-sdk-contract.mjs).
//
// 1. `copilot update` (seam: PIJ_COPILOT_UPDATE_CMD, an executable run with no
//    arguments and no shell; tests use it, so no test ever updates Copilot).
// 2. The contract with --json (seam: PIJ_COPILOT_SDK_CONTRACT_CMD, run the
//    same way with that argument). Its full pass records a local pass for the
//    version under pij-rs's state dir (F250); nothing is committed.
// 3. Pass: the fleet may reload. Anything else: exit 1, "do not reload", and why.
// With no update available it says so and still runs the contract.
import { spawn } from "node:child_process";
import { join } from "node:path";
import { installedVersion } from "./copilot-cli.mjs";
import { DO_NOT_RELOAD } from "./copilot-sdk-verdict.mjs";

const CONTRACT = join(import.meta.dirname, "copilot-sdk-contract.mjs");

function stop(message) {
	console.log(`update-copilot: ${message}`);
	process.exit(1);
}

/** Run `command args`, no shell; resolve its exit code and its stdout. */
function run(command, args, { capture = false } = {}) {
	return new Promise((resolve, reject) => {
		const child = spawn(command, args, {
			stdio: ["ignore", capture ? "pipe" : "inherit", "inherit"],
		});
		let stdout = "";
		child.stdout?.on("data", (chunk) => {
			stdout += chunk;
		});
		child.once("error", reject);
		child.once("close", (code) => resolve({ code, stdout }));
	});
}

const before = await installedVersion();
if (!before) stop("no Copilot CLI installed (COPILOT_BIN or `copilot` on PATH); nothing to update");

const [updateCommand, updateArgs] = process.env.PIJ_COPILOT_UPDATE_CMD
	? [process.env.PIJ_COPILOT_UPDATE_CMD, []]
	: [process.env.COPILOT_BIN || "copilot", ["update"]];
const updated = await run(updateCommand, updateArgs).catch((error) => ({ code: error.message }));
if (updated.code !== 0)
	stop(
		`copilot update failed (${updated.code}); seats keep Copilot ${before} and nothing was recorded`,
	);

const after = await installedVersion();
if (!after)
	stop(`Copilot no longer reports a version after the update (was ${before}); ${DO_NOT_RELOAD}`);
console.log(
	after === before
		? `update-copilot: no update available: Copilot is still ${after}; running the contract anyway`
		: `update-copilot: Copilot updated ${before} -> ${after}; running the contract before any seat reloads`,
);

const [contractCommand, contractArgs] = process.env.PIJ_COPILOT_SDK_CONTRACT_CMD
	? [process.env.PIJ_COPILOT_SDK_CONTRACT_CMD, ["--json"]]
	: [process.execPath, [CONTRACT, "--json"]];
const contract = await run(contractCommand, contractArgs, { capture: true }).catch((error) => ({
	code: error.message,
	stdout: "",
}));
let report;
try {
	report = JSON.parse(contract.stdout.trim().split("\n").at(-1) ?? "");
} catch {
	report = undefined;
}
if (report?.status !== "pass" || contract.code !== 0) {
	const failing = (report?.checks ?? []).filter((check) => !check.ok);
	for (const check of failing) console.log(`FAIL ${check.name}: ${check.detail}`);
	if (report?.diagnostic) console.log(`extension stderr:\n${report.diagnostic}`);
	stop(
		`${DO_NOT_RELOAD}. The contract on Copilot ${after} was ${report?.status ?? `unreadable (exit ${contract.code})`}${report?.summary ? `: ${report.summary}` : ""}`,
	);
}
// R261-001: a pass approves a reload only for the version it checked and that
// is verified (a local pass on this machine, or the committed floor), and only
// if that is the version installed now. The contract reads the version itself,
// a separate snapshot from `after`, and Copilot can replace itself in between.
const verifiedNow = report.local?.version === after || report.verified === after;
if (report.version !== after || !verifiedNow)
	stop(
		`${DO_NOT_RELOAD}. The contract passed on Copilot ${report.version ?? "(no version named)"}, verified ${report.local?.version ?? report.verified ?? "(nothing)"}, but Copilot ${after} is installed: run just update-copilot again`,
	);
console.log(
	`update-copilot: the contract passed on Copilot ${report.version}${report.local?.version === after ? ` and recorded a local pass for it (${report.local.passed_at})` : ", the committed floor"}; nothing to commit. The fleet may reload Copilot seats.`,
);
