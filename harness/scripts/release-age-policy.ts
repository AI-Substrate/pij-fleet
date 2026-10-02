import { spawnSync } from "node:child_process";

export const NPM_REGISTRY_URL = "https://registry.npmjs.org/";
/** Opt-in registry override for machines that must resolve through a mirror or
 *  proxy (e.g. when registry.npmjs.org is unreachable). Unset → public npm. */
export const NPM_REGISTRY_OVERRIDE_ENV = "PIJ_NPM_REGISTRY";

export function npmRegistryUrl(environment: NodeJS.ProcessEnv = process.env): string {
	const override = environment[NPM_REGISTRY_OVERRIDE_ENV]?.trim();
	return override ? override : NPM_REGISTRY_URL;
}
export const NPM_REPLACE_REGISTRY_HOST = "npmjs";
export const NPM_PREFER_ONLINE = true;
export const MIN_RELEASE_AGE_DAYS = 7;
export const ROOT_LOCK_REPLAY_MIN_RELEASE_AGE = "null";

// ─── quarantine-support preflight (PR#25 adopt, dove ruling: fail-closed) ─────
//
// The `min-release-age` quarantine is enforced natively starting in npm 11.10.0.
// Older npm releases accept the setting but silently install a too-young package,
// so the governed path must refuse before resolution rather than run unprotected.
export const MIN_NPM_VERSION_FOR_QUARANTINE = "11.10.0";

function npmVersionAtLeast(version: string, minimum: string): boolean {
	const pattern = /^\s*v?(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\s*$/;
	const current = pattern.exec(version);
	const required = pattern.exec(minimum);
	if (!current || !required) return false;
	for (let index = 1; index <= 3; index += 1) {
		const currentPart = Number(current[index]);
		const requiredPart = Number(required[index]);
		if (currentPart !== requiredPart) return currentPart > requiredPart;
	}
	return current[4] === undefined;
}

/** Pure guard: the named refusal error when npm cannot enforce the quarantine,
 *  or null when it can. */
export function quarantineSupportError(version: string): string | null {
	if (npmVersionAtLeast(version, MIN_NPM_VERSION_FOR_QUARANTINE)) return null;
	return (
		`min-release-age requires npm>=${MIN_NPM_VERSION_FOR_QUARANTINE}; quarantine cannot be enforced ` +
		`— refusing rather than silently skipping it (found npm ${version.trim() || "unknown"}).`
	);
}

/** Fail-closed preflight for EVERY governed install path (dove ruling: no
 *  governed install may silently skip the quarantine on any npm — unreachability
 *  is a mitigation, not an invariant). Probes the npm version under the governed
 *  environment (so a hostile caller config can't trip the probe) and, on
 *  npm < 11.10.0, prints the named refusal and exits nonzero rather than proceeding
 *  unprotected. Injectable for tests. */
export function assertQuarantineEnforceableOrExit(
	deps: {
		probeNpmVersion?: () => { status: number | null; stdout: string };
		fail?: (message: string) => never;
	} = {},
): void {
	const probe =
		deps.probeNpmVersion ??
		(() => {
			const result = spawnSync("npm", ["--version"], {
				encoding: "utf8",
				env: npmResolutionEnvironment(),
			});
			return { status: result.status, stdout: result.stdout ?? "" };
		});
	const fail =
		deps.fail ??
		((message: string): never => {
			console.error(message);
			process.exit(1);
		});
	const { status, stdout } = probe();
	if (status !== 0) {
		fail("quarantine preflight: could not determine npm version");
		return;
	}
	const refusal = quarantineSupportError(stdout);
	if (refusal) fail(refusal);
}

const NPM_REGISTRY_ENV = "npm_config_registry";
const NPM_REPLACE_REGISTRY_HOST_ENV = "npm_config_replace_registry_host";
const NPM_PREFER_ONLINE_ENV = "npm_config_prefer_online";
const NPM_MIN_RELEASE_AGE_ENV = "npm_config_min_release_age";
const CONFLICTING_NPM_ENV_KEYS = new Set([
	NPM_REGISTRY_ENV,
	NPM_REPLACE_REGISTRY_HOST_ENV,
	NPM_PREFER_ONLINE_ENV,
	NPM_MIN_RELEASE_AGE_ENV,
	"npm_config_before",
]);

function withoutNpmResolutionOverrides(environment: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
	return Object.fromEntries(
		Object.entries(environment).filter(([key]) => !CONFLICTING_NPM_ENV_KEYS.has(key.toLowerCase())),
	);
}

export function npmResolutionEnvironment(
	environment: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
	return {
		...withoutNpmResolutionOverrides(environment),
		[NPM_REGISTRY_ENV]: npmRegistryUrl(environment),
		[NPM_REPLACE_REGISTRY_HOST_ENV]: NPM_REPLACE_REGISTRY_HOST,
		[NPM_PREFER_ONLINE_ENV]: String(NPM_PREFER_ONLINE),
		[NPM_MIN_RELEASE_AGE_ENV]: String(MIN_RELEASE_AGE_DAYS),
	};
}

export function rootLockReplayEnvironment(
	environment: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
	return {
		...withoutNpmResolutionOverrides(environment),
		[NPM_REGISTRY_ENV]: npmRegistryUrl(environment),
		[NPM_REPLACE_REGISTRY_HOST_ENV]: NPM_REPLACE_REGISTRY_HOST,
		[NPM_PREFER_ONLINE_ENV]: String(NPM_PREFER_ONLINE),
	};
}

export function rootLockReplayNpmArgs(): string[] {
	return ["ci", `--min-release-age=${ROOT_LOCK_REPLAY_MIN_RELEASE_AGE}`];
}
