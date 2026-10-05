import { spawnSync } from "node:child_process";
import {
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	assertQuarantineEnforceableOrExit,
	MIN_NPM_VERSION_FOR_QUARANTINE,
	MIN_RELEASE_AGE_DAYS,
	NPM_PREFER_ONLINE,
	NPM_REGISTRY_OVERRIDE_ENV,
	NPM_REGISTRY_URL,
	NPM_REPLACE_REGISTRY_HOST,
	npmRegistryUrl,
	npmResolutionEnvironment,
	quarantineSupportError,
	ROOT_LOCK_REPLAY_MIN_RELEASE_AGE,
	rootLockReplayEnvironment,
	rootLockReplayNpmArgs,
} from "./release-age-policy.js";

const PIJ_ROOT = resolve(import.meta.dirname, "..", "..");
const RUNNING_NPM_VERSION = spawnSync("npm", ["--version"], { encoding: "utf8" }).stdout ?? "";
const temporaryRoots: string[] = [];

afterEach(() => {
	for (const root of temporaryRoots.splice(0)) {
		rmSync(root, { recursive: true, force: true });
	}
});

describe("npm resolution policy", () => {
	it("freezes proxy authority, lock-host replacement, online revalidation, age, and audit", () => {
		expect(NPM_REGISTRY_URL).toBe("https://registry.npmjs.org/");
		expect(npmRegistryUrl({})).toBe("https://registry.npmjs.org/");
		expect(npmRegistryUrl({ PIJ_NPM_REGISTRY: "https://mirror.example/npm/" })).toBe(
			"https://mirror.example/npm/",
		);
		expect(
			npmResolutionEnvironment({ PIJ_NPM_REGISTRY: "https://mirror.example/npm/" })
				.npm_config_registry,
		).toBe("https://mirror.example/npm/");
		// npmjs-scoped host replacement (PR#25 adopt, dove ruling s052-npm-hardening-pr25).
		// npm's `replace-registry-host=always` rewrites the host of EVERY resolved URL —
		// including the `minih` git+ssh github.com dependency, which the proxy cannot serve
		// (npm ci then dies 128/E404 on every platform). `npmjs` scopes the rewrite to the
		// default npmjs registry host only, so registry reads still route through a configured mirror
		// while git deps resolve from their origin. A registry proxy can't serve git, so
		// `always` bought zero security on git deps — only breakage.
		expect(NPM_REPLACE_REGISTRY_HOST).toBe("npmjs");
		expect(NPM_PREFER_ONLINE).toBe(true);
		expect(MIN_RELEASE_AGE_DAYS).toBe(7);
		expect(readFileSync(resolve(PIJ_ROOT, ".npmrc"), "utf8")).toBe(
			"replace-registry-host=npmjs\n" +
				"prefer-online=true\n" +
				"min-release-age=7\n" +
				"audit=true\n",
		);
	});

	it("fails closed below npm 11.10.0 and allows supported releases", () => {
		expect(MIN_NPM_VERSION_FOR_QUARANTINE).toBe("11.10.0");
		for (const supported of ["11.10.0", "11.10.0+build.1", "11.10.1", "12.0.0"]) {
			expect(quarantineSupportError(supported)).toBeNull();
		}

		for (const unsupported of ["11.10.0-rc.0", "11.9.9", "10.9.2", "9.0.0", "garbage"]) {
			const error = quarantineSupportError(unsupported);
			expect(error).toContain("min-release-age requires npm>=11.10.0");
			expect(error).toContain("refusing rather than silently skipping");
		}
	});

	it("refuses npm 11.5.1 because min-release-age starts at 11.10.0", () => {
		const error = quarantineSupportError("11.5.1");
		expect(error).toContain("min-release-age requires npm>=11.10.0");
		expect(error).toContain("refusing rather than silently skipping");
	});

	it("the shared preflight refuses below npm 11.10.0 and passes at the floor", () => {
		// The shared assert is what BOTH governed install paths (npm-resolution-run
		// and packages.ts) call, so the fail-closed guarantee is comprehensive.
		let failed: string | null = null;
		const failing: never = undefined as never;
		const fail = (m: string): never => {
			failed = m;
			return failing;
		};

		// npm >= 11 → no refusal, fail never called.
		assertQuarantineEnforceableOrExit({
			probeNpmVersion: () => ({ status: 0, stdout: "11.10.0\n" }),
			fail,
		});
		expect(failed).toBeNull();

		// npm < 11 → the named refusal.
		assertQuarantineEnforceableOrExit({
			probeNpmVersion: () => ({ status: 0, stdout: "10.9.2\n" }),
			fail,
		});
		expect(failed).toContain("min-release-age requires npm>=11.10.0");

		// npm probe itself fails → refuse (can't prove enforceability).
		failed = null;
		assertQuarantineEnforceableOrExit({
			probeNpmVersion: () => ({ status: 1, stdout: "" }),
			fail,
		});
		expect(failed).toContain("could not determine npm version");
	});

	it("scopes host replacement to npmjs so git deps resolve from their origin, not the proxy", () => {
		// Positive semantics assertion (dove condition 1): the contract must ROUTE
		// npmjs registry reads through the proxy AND leave github.com git deps alone —
		// not merely be "loosened" from always. `npmjs` is npm's mode for exactly that.
		expect(NPM_REPLACE_REGISTRY_HOST).toBe("npmjs");
		expect(NPM_REPLACE_REGISTRY_HOST).not.toBe("always"); // over-broad: rewrites git hosts too

		// The probe/daemon resolution environment inherits the scoped mode, so no code
		// path re-broadens it to `always` and re-breaks the git dependency.
		for (const env of [npmResolutionEnvironment({}), rootLockReplayEnvironment({})]) {
			expect(env.npm_config_replace_registry_host).toBe("npmjs");
			// Registry authority is preserved: npmjs reads still go through the proxy.
			expect(env.npm_config_registry).toBe(NPM_REGISTRY_URL);
		}
	});

	it("strips mixed-case caller policy overrides without mutating the caller", () => {
		const callerEnvironment = {
			PATH: process.env.PATH,
			NPM_CONFIG_REGISTRY: "https://caller.invalid/",
			Npm_Config_Replace_Registry_Host: "never",
			Npm_Config_Prefer_Online: "false",
			NPM_CONFIG_MIN_RELEASE_AGE: "0",
			Npm_Config_Before: "2000-01-01T00:00:00.000Z",
		};

		const childEnvironment = npmResolutionEnvironment(callerEnvironment);

		expect(childEnvironment).toMatchObject({
			npm_config_registry: NPM_REGISTRY_URL,
			npm_config_replace_registry_host: NPM_REPLACE_REGISTRY_HOST,
			npm_config_prefer_online: "true",
			npm_config_min_release_age: "7",
		});
		expect(childEnvironment.NPM_CONFIG_REGISTRY).toBeUndefined();
		expect(childEnvironment.Npm_Config_Replace_Registry_Host).toBeUndefined();
		expect(childEnvironment.Npm_Config_Prefer_Online).toBeUndefined();
		expect(childEnvironment.NPM_CONFIG_MIN_RELEASE_AGE).toBeUndefined();
		expect(childEnvironment.Npm_Config_Before).toBeUndefined();
		expect(childEnvironment.npm_config_before).toBeUndefined();
		expect(callerEnvironment).toEqual({
			PATH: process.env.PATH,
			NPM_CONFIG_REGISTRY: "https://caller.invalid/",
			Npm_Config_Replace_Registry_Host: "never",
			Npm_Config_Prefer_Online: "false",
			NPM_CONFIG_MIN_RELEASE_AGE: "0",
			Npm_Config_Before: "2000-01-01T00:00:00.000Z",
		});
	});

	it("keeps proxy, lock-host replacement, and online policy while clearing root age", () => {
		const childEnvironment = rootLockReplayEnvironment({
			PATH: process.env.PATH,
			NPM_CONFIG_REGISTRY: "https://caller.invalid/",
			NPM_CONFIG_REPLACE_REGISTRY_HOST: "never",
			npm_config_prefer_online: "false",
			Npm_Config_Min_Release_Age: "1",
			NPM_CONFIG_BEFORE: "2000-01-01T00:00:00.000Z",
		});

		expect(childEnvironment).toMatchObject({
			npm_config_registry: NPM_REGISTRY_URL,
			npm_config_replace_registry_host: NPM_REPLACE_REGISTRY_HOST,
			npm_config_prefer_online: "true",
		});
		expect(childEnvironment.npm_config_min_release_age).toBeUndefined();
		expect(childEnvironment.npm_config_before).toBeUndefined();
		expect(rootLockReplayNpmArgs()).toEqual(["ci", "--min-release-age=null"]);
		expect(ROOT_LOCK_REPLAY_MIN_RELEASE_AGE).toBe("null");
		expect(rootLockReplayNpmArgs()).not.toContain("install");
	});

	it("propagates governed values through the fail-closed runner and preserves child status", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-npm-resolution-runner-"));
		temporaryRoots.push(root);
		const recorder = join(root, "record.cjs");
		const output = join(root, "environment.json");
		writeFileSync(
			recorder,
			[
				'const fs = require("node:fs");',
				"fs.writeFileSync(process.argv[2], JSON.stringify({",
				"  registry: process.env.npm_config_registry,",
				"  replaceRegistryHost: process.env.npm_config_replace_registry_host,",
				"  online: process.env.npm_config_prefer_online,",
				"  age: process.env.npm_config_min_release_age,",
				"  before: process.env.npm_config_before,",
				"}));",
				"process.exit(7);",
				"",
			].join("\n"),
		);
		const result = spawnSync(
			resolve(PIJ_ROOT, "node_modules", ".bin", "tsx"),
			[
				resolve(PIJ_ROOT, "harness", "scripts", "npm-resolution-run.ts"),
				process.execPath,
				recorder,
				output,
			],
			{
				cwd: PIJ_ROOT,
				encoding: "utf8",
				env: {
					// The machine's own registry override would leak into the expectation below.
					...withoutRegistryOverride(process.env),
					NPM_CONFIG_REGISTRY: "https://caller.invalid/",
					NPM_CONFIG_REPLACE_REGISTRY_HOST: "never",
					NPM_CONFIG_PREFER_ONLINE: "false",
					NPM_CONFIG_MIN_RELEASE_AGE: "0",
					NPM_CONFIG_BEFORE: "2000-01-01T00:00:00.000Z",
				},
			},
		);

		// Fail-closed preflight (dove ruling): on npm >= 11 the runner propagates
		// the governed env and runs the command (recorder exits 7). On npm < 11 it
		// REFUSES up front — the recorder never runs — so the governed-env
		// propagation cannot be observed; the refusal is the correct behaviour.
		if (quarantineSupportError(RUNNING_NPM_VERSION) === null) {
			expect(result.status).toBe(7);
			expect(JSON.parse(readFileSync(output, "utf8"))).toEqual({
				registry: NPM_REGISTRY_URL,
				replaceRegistryHost: NPM_REPLACE_REGISTRY_HOST,
				online: "true",
				age: "7",
			});
		} else {
			expect(result.status).toBe(1);
			expect(result.stderr).toContain("min-release-age requires npm>=11.10.0");
		}

		const missing = spawnSync(
			resolve(PIJ_ROOT, "node_modules", ".bin", "tsx"),
			[
				resolve(PIJ_ROOT, "harness", "scripts", "npm-resolution-run.ts"),
				"pij-command-that-does-not-exist",
			],
			{ cwd: PIJ_ROOT, encoding: "utf8" },
		);
		expect(missing.status).toBe(1);
		expect(missing.stderr).toContain("npm-resolution-run:");
	});

	it("wires every pij-owned resolver through the shared policy", () => {
		const justfile = readFileSync(resolve(PIJ_ROOT, "justfile"), "utf8");
		const ciWorkflow = readFileSync(resolve(PIJ_ROOT, ".github/workflows/ci.yml"), "utf8");

		expect(justfile).toContain("just _root-lock-npm-ci");
		expect(justfile).toContain("npm ci --min-release-age=null");
		// "$" + "{" keeps biome's template-literal lint off a shell default-expansion.
		const governedRegistry =
			'npm_config_registry="$' + '{PIJ_NPM_REGISTRY:-https://registry.npmjs.org/}"';
		expect(justfile).toContain(governedRegistry);
		expect(justfile).toContain('npm_config_replace_registry_host="npmjs"');
		expect(justfile).toContain('npm_config_prefer_online="true"');
		// Every fresh resolution (OMP and its bun runtime) goes through the governed runner.
		expect(justfile).toContain(
			"just _npm-resolution npm install -g --ignore-scripts @oh-my-pi/pi-coding-agent@latest",
		);
		expect(justfile).toContain("just _npm-resolution npm install -g bun");
		expect(justfile).toContain("node harness/scripts/pij-cli.cjs");
		expect(
			readFileSync(resolve(PIJ_ROOT, "harness/scripts/npm-resolution-run.ts"), "utf8"),
		).not.toMatch(/^#!.*tsx/m);
		expect(
			readFileSync(resolve(PIJ_ROOT, "harness/scripts/npm-resolution-diagnostic.ts"), "utf8"),
		).not.toMatch(/^#!.*tsx/m);
		expect(justfile).not.toMatch(/^\s+npx\s/m);
		expect(justfile).not.toMatch(/npm_config_min_release_age=.*npm install/);

		expect(ciWorkflow.match(/run: npm ci --min-release-age=null/g)).toHaveLength(1);
		expect(`${justfile}\n${ciWorkflow}`).not.toMatch(
			/npm install[^\n]*--min-release-age=(?:0|null)/,
		);
	});

	it("rejects a stale global pij bin while accepting the wrapper target", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-bin-shape-"));
		temporaryRoots.push(root);
		const globalRoot = join(root, "lib", "node_modules");
		const binDir = join(root, "bin");
		const expected = join(globalRoot, "pij", "harness", "scripts", "pij-cli.cjs");
		const stale = join(globalRoot, "pij", ".pi", "extensions", "pij", "cli.ts");
		const bin = join(binDir, "pij");
		mkdirSync(resolve(expected, ".."), { recursive: true });
		mkdirSync(resolve(stale, ".."), { recursive: true });
		mkdirSync(binDir, { recursive: true });
		writeFileSync(expected, "#!/usr/bin/env node\n");
		writeFileSync(stale, "#!/usr/bin/env -S npx tsx\n");
		symlinkSync(relative(binDir, expected), bin);

		const valid = spawnSync("just", ["_pij-bin-shape-check", globalRoot, bin], {
			cwd: PIJ_ROOT,
			encoding: "utf8",
		});
		expect(valid.status).toBe(0);
		expect(valid.stdout).toContain(expected);

		unlinkSync(bin);
		symlinkSync(relative(binDir, stale), bin);
		const invalid = spawnSync("just", ["_pij-bin-shape-check", globalRoot, bin], {
			cwd: PIJ_ROOT,
			encoding: "utf8",
		});
		expect(invalid.status).toBe(1);
		expect(invalid.stdout).toContain("stale global pij bin");
		expect(invalid.stdout).toContain("run npm link from the local main checkout");
	});
});

function withoutRegistryOverride(env: NodeJS.ProcessEnv): NodeJS.ProcessEnv {
	const { [NPM_REGISTRY_OVERRIDE_ENV]: _override, ...rest } = env;
	return rest;
}
