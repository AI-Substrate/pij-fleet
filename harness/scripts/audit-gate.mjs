#!/usr/bin/env node
// CI dependency gate: `npm audit` at high+, minus time-boxed allowlist entries.
// An entry must name the advisory, why it is safe to defer, and an expiry; an
// expired entry fails the gate, so an exception can never become permanent.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const allow = JSON.parse(
	readFileSync(new URL("../../.github/audit-allowlist.json", import.meta.url), "utf8"),
);
const now = Date.now();
let raw;
try {
	raw = execFileSync("npm", ["audit", "--json"], { encoding: "utf8" });
} catch (err) {
	raw = err.stdout; // npm audit exits non-zero when it finds anything
}
const report = JSON.parse(raw);
// Judge advisories, not packages: a package flagged only because it depends on
// a vulnerable one lists that dependency's name in `via`, not an advisory.
const advisories = new Map();
for (const vuln of Object.values(report.vulnerabilities ?? {})) {
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
