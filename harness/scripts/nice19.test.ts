import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

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
});
