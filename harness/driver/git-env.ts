// harness/driver/git-env.ts
//
// The git environment a test never inherits (incident #354). A seat exported
// GIT_DIR/GIT_WORK_TREE and ran the tests; every fixture's `git init`,
// `git config` and `git worktree add` went to the real shared repository.
//
// Entry points (vitest.config.ts, the smoke runner) refuse while GIT_DIR or
// GIT_WORK_TREE is set and clear the rest from their own process.env before
// anything launches; test helpers that launch git build the child's env with
// gitEnv(). The Rust side is pij_testkit::git_env and
// harness/scripts/cargo-test-runner.sh.

/** The variables that move git's repository, index, objects or ref namespace. */
export const GIT_LOCATION_ENV = [
	"GIT_DIR",
	"GIT_WORK_TREE",
	"GIT_INDEX_FILE",
	"GIT_OBJECT_DIRECTORY",
	"GIT_ALTERNATE_OBJECT_DIRECTORIES",
	"GIT_COMMON_DIR",
	"GIT_CEILING_DIRECTORIES",
	"GIT_NAMESPACE",
] as const;

/** Every GIT_CONFIG* variable (GLOBAL, SYSTEM, NOSYSTEM, COUNT, KEY_n, VALUE_n, PARAMETERS). */
export const GIT_CONFIG_PREFIX = "GIT_CONFIG";

/** Author and committer variables, cleared where a test sets its own identity. */
export const GIT_IDENTITY_PREFIXES = ["GIT_AUTHOR_", "GIT_COMMITTER_"] as const;

/** The two variables whose presence refuses a test run. */
export const GIT_REFUSED_ENV = ["GIT_DIR", "GIT_WORK_TREE"] as const;

export interface GitEnvOptions {
	/** The environment to start from (default process.env). */
	readonly base?: NodeJS.ProcessEnv;
	/** Also clear GIT_AUTHOR_* and GIT_COMMITTER_*: the test sets its own identity. */
	readonly ownIdentity?: boolean;
}

function inherited(name: string, ownIdentity: boolean): boolean {
	if ((GIT_LOCATION_ENV as readonly string[]).includes(name)) return true;
	if (name.startsWith(GIT_CONFIG_PREFIX)) return true;
	return ownIdentity && GIT_IDENTITY_PREFIXES.some((prefix) => name.startsWith(prefix));
}

/** A copy of the base environment without the inherited git variables, with `extra` on top. */
export function gitEnv(
	extra: NodeJS.ProcessEnv = {},
	options: GitEnvOptions = {},
): NodeJS.ProcessEnv {
	const env: NodeJS.ProcessEnv = {};
	for (const [name, value] of Object.entries(options.base ?? process.env)) {
		if (!inherited(name, options.ownIdentity ?? false)) env[name] = value;
	}
	return { ...env, ...extra };
}

/** Why a test run must not start in `env`, or undefined when it may. */
export function inheritedGitDirRefusal(env: NodeJS.ProcessEnv, runner: string): string | undefined {
	const set = GIT_REFUSED_ENV.filter((name) => env[name] !== undefined);
	if (set.length === 0) return undefined;
	return [
		`${runner}: refusing to run: this environment sets ${set.join(" and ")}, so git in every test fixture would write into that repository (incident #354).`,
		"  fix: unset GIT_DIR GIT_WORK_TREE",
	].join("\n");
}

/** Deletes the inherited git location and config variables from `env` in place. For entry points only. */
export function scrubGitEnvInPlace(env: NodeJS.ProcessEnv): void {
	for (const name of Object.keys(env)) {
		if (inherited(name, false)) delete env[name];
	}
}
