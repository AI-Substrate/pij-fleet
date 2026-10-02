// THE WRITE-LAW ENFORCER (plan 071 review round 1, §1.2).
//
// Five lost-updates on the same law in three plans. Two of them — `stampSenderActivity`
// and `releaseIdentity` — were written by the very branch that was fixing the other
// three, while the law sat documented in a daemon module behind a docstring that
// said "every DAEMON descriptor write".
//
// The conclusion is not "document it better". It is that a rule with no mechanical
// enforcement is a rule that gets rediscovered by shipping the bug. This test is the
// enforcement: a NEW raw descriptor write outside the allowlist fails the build, and
// the failure message tells the author what to do instead.
//
// This is deliberately a source-shaped test. A behavioural test cannot see a writer
// that has not been written yet, and "has not been written yet" is exactly the case
// that has bitten five times.
//
// ── READ THIS BEFORE TRUSTING THIS FILE ───────────────────────────────────────
//
// THIS TEST IS A TRIPWIRE, NOT A PROOF. Everything below is a TEXT PROXY for the
// real question — "does this write set a contested field?" — and a text proxy will
// always have holes. Two were found by mutating the guard itself (review round 3):
// ES6 shorthand (`write({ ...latest, closeIntent })`) sailed through a `field:`
// pattern, and an authority matched ANYWHERE in the call text meant a PAYLOAD VALUE
// (`{ currentTask: "cli" }`) satisfied it. Both are closed. That does NOT make this
// complete, and a green run here is not evidence that no lost-update exists.
//
// THE LOAD-BEARING PROTECTION IS THAT `write()` MERGES BY DEFAULT
// (`core/registry-write.ts`, applied inside the adapter). A writer that reads none
// of this and declares nothing still cannot clobber another writer's field. This
// file only catches the careless case earlier and with a better error message.
//
// A second limit, worth knowing precisely: the raw-write allowlist above is
// FILE-LEVEL, and it allowlists `cli.ts`, `core/cli.ts`, `daemon.ts`,
// `core/session.ts` and `core/daemon/loop.ts`. NONE of the five historical
// incidents would have been caught by it — every one of them lived in an
// allowlisted file. The SET-side test at the bottom is the one doing real work,
// because it is file-agnostic: it asks what a write DOES, not where it lives.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { DESCRIPTOR_FIELD_OWNER } from "./registry-write.js";

const EXTENSION_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Files permitted to write a descriptor WITHOUT the merge law, each with the
 *  reason exact semantics are correct there. Adding a row is a deliberate act and
 *  should be argued in review; that is the entire point of the list.
 *
 *  A row here does NOT mean "this file is exempt forever" — it means the writes in
 *  it as of plan 071 were reviewed. */
const RAW_WRITE_ALLOWLIST = new Map<string, string>([
	[
		"adapters/rust-runtime.ts",
		"implements an in-memory scratch RegistryPort; its writes never touch a shared descriptor",
	],
	["core/registry-write.ts", "this IS the law — it performs the merged write"],
	["core/session.ts", "in-process seat owns its own descriptor lifecycle (publish + self-persist)"],
	[
		"core/orchestration/role.ts",
		'RoleService owns orchestrationRole and must declare "cli" so the write law keeps its computed value; mirrors prime.ts',
	],
	["cli.ts", "spawn/adopt/revive publish brand-new descriptors; reviewed in plan 071"],
]);

/** `registry.write(` / `this.write(` on a registry, and the adapter's raw
 *  `writeAtomic(`. Deliberately textual: it must catch a writer that does not
 *  exist yet. */
const RAW_WRITE = /(?:registry|reg0|this)\s*\.\s*write\s*\(|writeAtomic\s*\(/;

function sourceFiles(dir: string, out: string[] = []): string[] {
	for (const entry of readdirSync(dir)) {
		if (entry === "node_modules" || entry === "__fixtures__") continue;
		const full = join(dir, entry);
		if (statSync(full).isDirectory()) {
			sourceFiles(full, out);
			continue;
		}
		if (!entry.endsWith(".ts") || entry.endsWith(".test.ts")) continue;
		out.push(full);
	}
	return out;
}

describe("registry write law", () => {
	it("has no raw descriptor writer outside the reviewed allowlist", () => {
		const offenders: string[] = [];
		for (const file of sourceFiles(EXTENSION_ROOT)) {
			const rel = relative(EXTENSION_ROOT, file);
			if (RAW_WRITE_ALLOWLIST.has(rel)) continue;
			if (RAW_WRITE.test(readFileSync(file, "utf8"))) offenders.push(rel);
		}

		expect(
			offenders,
			`New raw descriptor write(s) found in: ${offenders.join(", ")}.\n\n` +
				"A descriptor write that does not go through `persistDaemonWrite` can replay a stale\n" +
				"snapshot over a field a concurrent writer just stamped. That has happened FIVE\n" +
				"times (see core/registry-write.ts for the incident list), twice inside the branch\n" +
				"that was fixing the earlier ones.\n\n" +
				"Use `persistDaemonWrite(registry, descriptor)` from `core/registry-write.js`.\n" +
				"If exact last-write-wins semantics are genuinely correct here (publishing a\n" +
				"brand-new descriptor, or deliberately clearing a mutable externally-owned\n" +
				"field), add this file to RAW_WRITE_ALLOWLIST with the reason.",
		).toEqual([]);
	});

	it("keeps the allowlist honest — every entry names a real file and a reason", () => {
		for (const [rel, reason] of RAW_WRITE_ALLOWLIST) {
			expect(
				() => statSync(join(EXTENSION_ROOT, rel)),
				`${rel} is allowlisted but missing`,
			).not.toThrow();
			expect(reason.length, `${rel} needs a real reason`).toBeGreaterThan(20);
		}
	});

	it("the law itself is stated where a writer will find it — next to the port, not in the daemon", () => {
		const law = readFileSync(join(EXTENSION_ROOT, "core", "registry-write.ts"), "utf8");
		expect(law).toContain("DESCRIPTOR_FIELD_OWNER");
		expect(law).toContain("applyWriteLaw");
		expect(law).toContain("PER-FIELD OWNERSHIP");
		// The wording that let CLI writers opt out must not come back.
		expect(law).not.toContain("EVERY daemon descriptor write");
	});
});

// Review round 2 §MED-a — the SET side of the law. A write that deliberately
// carries a contested field but declares no authority is silently lossy for its
// own data: the field is discarded whenever disk already holds a value, with no
// error and no log line. It is invisible on a fresh record, which is exactly why
// it hid in spawn's second-phase write until a reviewer reproduced it on
// adopt-into-pending.
//
// A behavioural test cannot reach every such call site (many need tmux), so this
// is enforced at the source, like the raw-write rule above.
describe("contested-field writes declare their authority", () => {
	const AUTHORITY = /"(daemon|cli|seat|close)"/;
	// `field:` OR ES6 shorthand (`field,` / `field}`). The `:`-only version missed
	// `write({ ...latest, closeIntent })` — both the idiomatic way to write the
	// guarded thing and a style already used in this codebase (review round 3).
	const CONTESTED_SETTER = new RegExp(
		`\\b(${Object.keys(DESCRIPTOR_FIELD_OWNER).join("|")})\\s*[:,}]`,
	);

	/** The call text AFTER its first argument — i.e. real argument position.
	 *  Matching the authority ANYWHERE let a PAYLOAD VALUE satisfy it:
	 *  `write({ ...latest, currentTask: "cli" })` passed while declaring nothing
	 *  (review round 3). Skips string literals so a comma inside one cannot split. */
	function argumentsAfterFirst(call: string): string {
		const open = call.indexOf("(");
		if (open === -1) return "";
		let depth = 0;
		let quote: string | undefined;
		for (let index = open; index < call.length; index++) {
			const ch = call[index];
			if (quote !== undefined) {
				if (ch === "\\") index += 1;
				else if (ch === quote) quote = undefined;
				continue;
			}
			if (ch === '"' || ch === "'" || ch === "`") {
				quote = ch;
				continue;
			}
			if (ch === "(" || ch === "{" || ch === "[") depth += 1;
			else if (ch === ")" || ch === "}" || ch === "]") depth -= 1;
			else if (ch === "," && depth === 1) return call.slice(index + 1);
		}
		return ""; // single-argument call — nothing was declared
	}

	/** The text of each `.write(...)` call, crudely but deterministically bounded
	 *  by paren depth. Deliberately textual — it must catch a call not yet written. */
	function writeCalls(source: string): string[] {
		const calls: string[] = [];
		const marker = ".write(";
		let from = source.indexOf(marker);
		while (from !== -1) {
			// REGISTRY writes only. Without this, `process.stderr.write("… parentId …")`
			// trips the shorthand pattern on its MESSAGE TEXT — a false positive the
			// reviewer hit too. The receiver is the identifier immediately before `.write`.
			const receiver = /([A-Za-z0-9_$]+)\s*$/.exec(source.slice(0, from));
			if (!receiver || !/^(registry|reg|reg0)$/.test(receiver[1] ?? "")) {
				from = source.indexOf(marker, from + marker.length);
				continue;
			}
			let depth = 0;
			let index = from + marker.length - 1;
			for (; index < source.length; index++) {
				const ch = source[index];
				if (ch === "(") depth += 1;
				else if (ch === ")") {
					depth -= 1;
					if (depth === 0) break;
				}
			}
			calls.push(source.slice(from, index + 1));
			from = source.indexOf(marker, index + 1);
		}
		return calls;
	}

	it("no write sets a contested field without naming the authority that owns it", () => {
		const offenders: string[] = [];
		for (const file of sourceFiles(EXTENSION_ROOT)) {
			const rel = relative(EXTENSION_ROOT, file);
			// The adapter implements the law; the law module IS the law.
			if (rel === "adapters/fs-registry.ts" || rel === "core/registry-write.ts") continue;
			for (const call of writeCalls(readFileSync(file, "utf8"))) {
				// `writeExact` is the reviewed escape hatch and never merges.
				if (call.startsWith(".writeExact(")) continue;
				if (!CONTESTED_SETTER.test(call)) continue;
				if (AUTHORITY.test(argumentsAfterFirst(call))) continue;
				offenders.push(`${rel}: ${call.split("\n")[0]?.trim()}`);
			}
		}

		expect(
			offenders,
			`Write(s) setting a contested field with no authority declared:\n  ${offenders.join("\n  ")}\n\n` +
				"Omitting the authority is NOT 'safe' for the field you are trying to SET —\n" +
				"it is discarded whenever disk already holds a value, silently. Pass the\n" +
				"owning authority (see DESCRIPTOR_FIELD_OWNER), or use writeExact if you are\n" +
				"deliberately CLEARING the field.",
		).toEqual([]);
	});
});
