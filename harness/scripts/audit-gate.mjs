#!/usr/bin/env node
// CI dependency gate: `npm audit` at high+, minus time-boxed allowlist entries.
// An entry must name the advisory, why it is safe to defer, and an expiry; an
// expired entry fails the gate, so an exception can never become permanent.
// An audit that did not run fails the gate too: a registry error (a 429 from
// the proxy) is retried once, then refused, never read as "no findings".
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { setTimeout as sleep } from "node:timers/promises";

const RETRY_MS = 5000;

const allow = JSON.parse(
	readFileSync(new URL("../../.github/audit-allowlist.json", import.meta.url), "utf8"),
);
const now = Date.now();

/** The audit report, or why there is none. */
function audit() {
	let raw;
	try {
		raw = execFileSync("npm", ["audit", "--json"], { encoding: "utf8" });
	} catch (err) {
		raw = err.stdout; // npm audit exits non-zero when it finds anything
	}
	let report;
	try {
		report = JSON.parse(raw);
	} catch {
		return { why: "npm audit printed no JSON report" };
	}
	const failed = report?.error !== undefined || report?.statusCode !== undefined;
	if (failed || typeof report?.vulnerabilities !== "object" || report.vulnerabilities === null) {
		const status = report?.statusCode ?? report?.error?.code ?? "no status";
		const message = report?.message || report?.error?.summary || "no vulnerabilities in the report";
		return { why: `${status} ${String(message).split("\n")[0]}` };
	}
	return { report };
}

let { report, why } = audit();
if (!report) {
	console.error(`audit-gate: audit did not run: ${why}; retrying once in ${RETRY_MS / 1000} s`);
	await sleep(RETRY_MS);
	({ report, why } = audit());
}
if (!report) {
	console.error(`audit-gate: audit did not run: ${why}`);
	process.exit(1);
}
// Judge advisories, not packages: a package flagged only because it depends on
// a vulnerable one lists that dependency's name in `via`, not an advisory.
const advisories = new Map();
for (const vuln of Object.values(report.vulnerabilities)) {
	for (const via of vuln.via) {
		if (typeof via !== "object" || !["high", "critical"].includes(via.severity)) continue;
		advisories.set(via.url?.split("/").pop() ?? via.title, via);
	}
}
const failures = [];
for (const [id, via] of advisories) {
	const entry = allow.find((a) => a.id === id);
	if (entry && Date.parse(entry.expires) > now) {
		console.log(`audit-gate: allowed ${id} (${via.name}) until ${entry.expires}: ${entry.reason}`);
		continue;
	}
	if (entry) console.error(`audit-gate: allowlist entry ${id} expired ${entry.expires}`);
	failures.push(`${id} ${via.name} (${via.severity}): ${via.title}`);
}
if (failures.length > 0) {
	console.error(`audit-gate: high findings:\n  ${failures.join("\n  ")}`);
	process.exit(1);
}
console.log("audit-gate: no unallowed high findings");
