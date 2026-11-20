import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

// F220: nice19.sh runs a command at max(19, the caller's niceness). A plain
// `nice -n 19` under a caller already niced stacked to 20 on macOS (R257-001).

const NICE19 = join(import.meta.dirname, "nice19.sh");

/** The caller's niceness, then the wrapped command's, under `parentNice`. */
function niceness(parentNice: readonly number[]): { caller: number; command: number } {
	const niced = parentNice.flatMap((n) => ["/usr/bin/nice", "-n", String(n)]);
	const script = `/bin/ps -o nice= -p $$; exec "${NICE19}" /bin/sh -c '/bin/ps -o nice= -p $$'`;
	const argv = [...niced, "/bin/sh", "-c", script];
	const result = spawnSync(argv[0] ?? "", argv.slice(1), { encoding: "utf8" });
	expect(result.status, result.stderr).toBe(0);
	const [caller, command] = result.stdout.trim().split(/\s+/).map(Number);
	return { caller: caller ?? Number.NaN, command: command ?? Number.NaN };
}

describe("nice19.sh", () => {
	it.each([
		[[]],
		[[5]],
		[[19]],
		[[19, 19]],
	] as const)("runs at max(19, caller) under a caller niced by %j", (parentNice) => {
		const { caller, command } = niceness(parentNice);
		expect(command).toBe(Math.max(19, caller));
	});

	it("passes the command's arguments through unchanged", () => {
		const result = spawnSync(NICE19, ["/usr/bin/printf", "%s|", "a b", "", "c"], {
			encoding: "utf8",
		});
		expect(result.stdout).toBe("a b||c|");
	});

	// R257-002: a niceness it cannot read is refused, never read as 0.
	describe("refuses when the caller's niceness cannot be read", () => {
		const roots: string[] = [];
		afterEach(() => {
			for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
		});
		/** nice19.sh asking a fake ps that runs `body`, wrapping `echo ran`. */
		function withPs(body: string) {
			const root = mkdtempSync(join(tmpdir(), "pij-nice19-"));
			roots.push(root);
			const ps = join(root, "ps");
			writeFileSync(ps, `#!/bin/sh\n${body}\n`);
			chmodSync(ps, 0o755);
			return spawnSync(NICE19, ["/bin/echo", "ran"], {
				encoding: "utf8",
				env: { ...process.env, NICE19_PS: ps },
			});
		}

		it.each([
			["fails", "exit 1"],
			["prints nothing", "exit 0"],
			["prints a bare minus", "echo ' -'"],
			["prints a range", "echo ' 1-2'"],
			["prints a word", "echo ' high'"],
		])("when ps %s", (_case, body) => {
			const result = withPs(body);
			expect(result.status).toBe(1);
			expect(result.stderr).toContain("nothing was run");
			expect(result.stdout).toBe("");
		});

		it("reads a padded or negative niceness and runs the command", () => {
			for (const body of ["echo '  5'", "echo ' -3'"]) {
				const result = withPs(body);
				expect(result.status, result.stderr).toBe(0);
				expect(result.stdout).toBe("ran\n");
			}
		});
	});
});
