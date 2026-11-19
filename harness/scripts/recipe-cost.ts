// F217, F220: every justfile recipe that runs a test runner or a cargo build
// does it through `harness/scripts/nice19.sh` (or through shared-build, which
// nices every cargo command the same way), and never into a per-worktree
// target. Load reached 198 from one un-niced `node --test`, and per-worktree
// targets filled the disk. A bare `nice -n` in a recipe is refused: under a
// caller that already niced, it stacks to 20 on macOS (R257-001).

/** A test runner or a cargo build on a recipe line. */
const RUNNER =
	/\bvitest\b|\bnode --test\b|\bnpm (?:run )?test\b|\bnpm run smoke\b|\bcargo (?:build|test|clippy|check|nextest|bench|run)\b|flow-pair-mutate\.sh/;
const NICE = /harness\/scripts\/nice19\.sh\b/;
const STACKING_NICE = /\bnice -n\b/;
const SHARED_BUILD = /shared-build\.sh\b/;
/**
 * Bare cargo builds into the checkout's default target, a fresh one per
 * worktree. Only the release deploy builds (`rs-install`, `bounce-rs`) do that,
 * niced; everything else goes through shared-build.
 */
const BARE_CARGO = /\bcargo (?!build --release\b)(?:build|test|clippy|check|nextest|bench|run)\b/;
const TARGET_ASSIGNMENT = /\b(CARGO_TARGET_DIR|PIJ_SHARED_TARGET)=("[^"]*"|\S*)/g;
/** The target may only come from cargo-target.sh, directly or through `$target`. */
const RESOLVED_TARGET = /^"?(?:\$\(bash harness\/scripts\/cargo-target\.sh\)|\$\{?target\}?)"?$/;

export interface RecipeFinding {
	readonly line: number;
	readonly recipe: string;
	readonly text: string;
	readonly problem: string;
}

/** Findings for a justfile's text; none means every recipe complies. */
export function recipeCostFindings(justfile: string): RecipeFinding[] {
	const findings: RecipeFinding[] = [];
	let recipe = "";
	let resolves = false;
	const lines = justfile.split("\n");
	lines.forEach((text, index) => {
		const header = /^@?([A-Za-z_][\w-]*)[^:=]*:(?!=)/.exec(text);
		if (header) {
			recipe = header[1] ?? "";
			resolves = false;
		}
		if (!/^\s/.test(text) || text.trim().startsWith("#")) return;
		if (text.includes("harness/scripts/cargo-target.sh")) resolves = true;
		const at = { line: index + 1, recipe, text: text.trim() };
		const runner = RUNNER.exec(text);
		if (STACKING_NICE.test(text))
			findings.push({
				...at,
				problem:
					"nices with nice -n, which stacks past 19 under a caller that already niced; use harness/scripts/nice19.sh",
			});
		else if (runner) {
			const before = text.slice(0, runner.index);
			if (!NICE.test(before) && !SHARED_BUILD.test(before))
				findings.push({
					...at,
					problem: "runs a test or cargo build without nice19.sh or shared-build",
				});
			else if (BARE_CARGO.test(text) && !SHARED_BUILD.test(text))
				findings.push({
					...at,
					problem: "runs cargo outside shared-build, into the checkout's default target",
				});
		}
		for (const [, name, value] of text.matchAll(TARGET_ASSIGNMENT)) {
			if (!resolves || !RESOLVED_TARGET.test(value ?? ""))
				findings.push({
					...at,
					problem: `sets ${name} to ${value}, not the checkout's one target from harness/scripts/cargo-target.sh`,
				});
		}
	});
	return findings;
}
