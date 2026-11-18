import { spawnSync } from "node:child_process";
import {
	chmodSync,
	copyFileSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

// The gate reads ../../.github/audit-allowlist.json beside itself and runs `npm audit --json`, so
// each case runs a copy of it in a scratch tree, with its own allowlist and a fake `npm`.
const GATE = resolve(dirname(fileURLToPath(import.meta.url)), "audit-gate.mjs");
const scratch: string[] = [];

afterEach(() => {
	for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true });
});

const CLEAN = { auditReportVersion: 2, vulnerabilities: {}, metadata: {} };
const HIGH = {
	auditReportVersion: 2,
	vulnerabilities: {
		picomatch: {
			name: "picomatch",
			severity: "high",
			via: [
				{
					name: "picomatch",
					title: "ReDoS",
					url: "https://github.com/advisories/GHSA-test-0000-0000",
					severity: "high",
				},
			],
		},
	},
	metadata: {},
};
// The shape npm printed when the registry proxy rate-limited the bulk advisory call.
const RATE_LIMITED = {
	message:
		"429 Too Many Requests - POST https://registry.invalid/-/npm/v1/security/advisories/bulk",
	method: "POST",
	uri: "https://registry.invalid/-/npm/v1/security/advisories/bulk",
	headers: {},
	statusCode: 429,
	body: "Request was blocked",
	error: {},
};

interface Gate {
	readonly status: number | null;
	readonly stdout: string;
	readonly stderr: string;
	readonly audits: number;
}

/** Run the gate with `npm audit` answering `reports` in turn (the last one repeats). */
function gate(reports: readonly object[], allowlist: readonly object[] = []): Gate {
	const root = mkdtempSync(join(tmpdir(), "pij-audit-gate-"));
	scratch.push(root);
	mkdirSync(join(root, "harness", "scripts"), { recursive: true });
	mkdirSync(join(root, ".github"));
	mkdirSync(join(root, "bin"));
	copyFileSync(GATE, join(root, "harness", "scripts", "audit-gate.mjs"));
	writeFileSync(join(root, ".github", "audit-allowlist.json"), JSON.stringify(allowlist));
	const calls = join(root, "calls");
	// Shell builtins only, so PATH can hold nothing but this fake.
	const answers = reports
		.map((report, i) =>
			i === reports.length - 1
				? `printf '%s' '${JSON.stringify(report)}'`
				: `if [ ! -e "${calls}.${i}" ]; then : > "${calls}.${i}"; printf '%s' '${JSON.stringify(report)}'; exit 1; fi`,
		)
		.join("\n");
	const npm = join(root, "bin", "npm");
	writeFileSync(npm, `#!/bin/sh\nprintf x >> "${calls}"\n${answers}\nexit 1\n`);
	chmodSync(npm, 0o755);
	const r = spawnSync(process.execPath, [join(root, "harness", "scripts", "audit-gate.mjs")], {
		encoding: "utf8",
		env: { ...process.env, PATH: join(root, "bin") },
	});
	let audits = 0;
	try {
		audits = readFileSync(calls, "utf8").length;
	} catch {
		audits = 0;
	}
	return { status: r.status, stdout: r.stdout, stderr: r.stderr, audits };
}

describe("audit gate", () => {
	it("a clean report passes", () => {
		const r = gate([CLEAN]);
		expect(r.status, r.stderr).toBe(0);
		expect(r.stdout).toContain("audit-gate: no unallowed high findings");
		expect(r.audits).toBe(1);
	});

	it("an allowlisted high passes until its expiry, and fails once expired", () => {
		const entry = { id: "GHSA-test-0000-0000", package: "picomatch", reason: "test" };
		const allowed = gate([HIGH], [{ ...entry, expires: "2999-01-01T00:00:00Z" }]);
		expect(allowed.status, allowed.stderr).toBe(0);
		expect(allowed.stdout).toContain("audit-gate: allowed GHSA-test-0000-0000 (picomatch)");

		const expired = gate([HIGH], [{ ...entry, expires: "2000-01-01T00:00:00Z" }]);
		expect(expired.status).toBe(1);
		expect(expired.stderr).toContain("allowlist entry GHSA-test-0000-0000 expired");
		expect(gate([HIGH]).status).toBe(1);
	});

	it("a rate-limited audit is retried once, then fails closed and says why", () => {
		const r = gate([RATE_LIMITED]);
		expect(r.status).toBe(1);
		expect(r.audits).toBe(2);
		expect(r.stderr).toContain(
			"audit-gate: audit did not run: 429 429 Too Many Requests - POST https://registry.invalid/",
		);
		expect(r.stdout).not.toContain("no unallowed high findings");
	});

	it("a report without vulnerabilities fails closed", () => {
		const r = gate([{ auditReportVersion: 2 }]);
		expect(r.status).toBe(1);
		expect(r.stderr).toContain("audit-gate: audit did not run: no status");
		expect(r.stdout).not.toContain("no unallowed high findings");
	});

	it("one retry recovers from a single rate-limited answer", () => {
		const r = gate([RATE_LIMITED, CLEAN]);
		expect(r.status, r.stderr).toBe(0);
		expect(r.audits).toBe(2);
		expect(r.stdout).toContain("audit-gate: no unallowed high findings");
	});
});
