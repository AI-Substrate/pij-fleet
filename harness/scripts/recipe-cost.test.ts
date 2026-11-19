import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { recipeCostFindings } from "./recipe-cost.js";

const JUSTFILE = join(import.meta.dirname, "../../justfile");

describe("justfile recipe cost (F217)", () => {
	it("runs every test and cargo build niced, into the checkout's one target", () => {
		expect(recipeCostFindings(readFileSync(JUSTFILE, "utf8"))).toEqual([]);
	});

	it.each([
		["an un-niced vitest", 'test *ARGS:\n    npm run test -- "$@"\n', "without nice"],
		["an un-niced node --test", "t:\n    node --test a/*.test.mjs\n", "without nice"],
		["a bare cargo test", "r:\n    cargo test --workspace\n", "without nice"],
		[
			"a niced cargo test into the default target",
			"r:\n    /usr/bin/nice -n 19 cargo test -p pij-daemon\n",
			"outside shared-build",
		],
		[
			"a per-worktree target",
			'r:\n    PIJ_SHARED_TARGET="{{justfile_directory()}}/target" bash harness/scripts/shared-build.sh -- test\n',
			"not the checkout's one target",
		],
		[
			"a target not from cargo-target.sh",
			'r:\n    PIJ_SHARED_TARGET="$PWD/target" bash harness/scripts/shared-build.sh -- test\n',
			"not the checkout's one target",
		],
	])("flags %s", (_, justfile, problem) => {
		const findings = recipeCostFindings(justfile);
		expect(findings).toHaveLength(1);
		expect(findings[0]?.problem).toContain(problem);
	});

	it("accepts nice, shared-build and the resolved target", () => {
		expect(
			recipeCostFindings(
				[
					"a:",
					"    /usr/bin/nice -n 19 npm run test",
					"b:",
					'    PIJ_SHARED_TARGET="$(bash harness/scripts/cargo-target.sh)" bash harness/scripts/shared-build.sh -- test',
					"c:",
					"    #!/usr/bin/env bash",
					'    target="$(bash harness/scripts/cargo-target.sh)"',
					'    CARGO_TARGET_DIR="$target" /usr/bin/nice -n 19 npm run smoke',
					"",
				].join("\n"),
			),
		).toEqual([]);
	});
});
