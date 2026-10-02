#!/usr/bin/env tsx

import { type ChildProcess, spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { relative, resolve } from "node:path";
import { npmInvocation } from "./cli-invocation.js";
import {
	MIN_RELEASE_AGE_DAYS,
	NPM_REPLACE_REGISTRY_HOST,
	npmResolutionEnvironment,
	ROOT_LOCK_REPLAY_MIN_RELEASE_AGE,
	rootLockReplayEnvironment,
	rootLockReplayNpmArgs,
} from "./release-age-policy.js";

const PIJ_ROOT = resolve(import.meta.dirname, "..", "..");
const TEMP_PARENT = resolve(PIJ_ROOT, ".harness", "temp", "s052");
const PACKAGE_NAME = "pij-release-age-fixture";
const OLD_VERSION = "1.0.0";
const YOUNG_VERSION = "2.0.0";
const OLD_PUBLISHED_AT = "2020-01-01T00:00:00.000Z";

interface CommandEvidence {
	command: string[];
	cwd: string;
	exitCode: number | null;
	name: string;
	stderr: string;
	stdout: string;
}

interface FixtureVersion {
	integrity: string;
	path: string;
	publishedAt: string;
	shasum: string;
	version: string;
}

interface RegistryFixture {
	child: ChildProcess;
	requestLogPath: string;
	statePath: string;
	url: string;
}

interface ProbeResult {
	audit: {
		exitCode: number | null;
		metadata: unknown;
	};
	checks: Record<string, boolean>;
	commands: CommandEvidence[];
	fixtureRegistry: string;
	productionMinReleaseAgeDays: number;
	requestCount: number;
	tempCleaned: boolean;
	tempRoot: string;
}

function normalizeOutput(output: string): string {
	const home = process.env.HOME;
	const withoutRepo = output.replaceAll(PIJ_ROOT, "<repo>");
	return home ? withoutRepo.replaceAll(home, "~") : withoutRepo;
}

function runNpm(
	name: string,
	args: string[],
	cwd: string,
	env: NodeJS.ProcessEnv,
): CommandEvidence {
	const invocation = npmInvocation(args);
	const result = spawnSync(invocation.file, invocation.args, {
		cwd,
		encoding: "utf8",
		env,
		maxBuffer: 20 * 1024 * 1024,
	});
	if (result.error) {
		throw new Error(`${name} failed to start: ${result.error.message}`);
	}
	return {
		name,
		command: [invocation.file, ...invocation.args],
		cwd: relative(PIJ_ROOT, cwd) || ".",
		exitCode: result.status,
		stdout: normalizeOutput(result.stdout),
		stderr: normalizeOutput(result.stderr),
	};
}

function sha256(path: string): string {
	return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function sleep(milliseconds: number): void {
	Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

function isolatedBaseEnvironment(
	tempRoot: string,
	cachePath: string,
	userConfigPath: string,
	globalConfigPath: string,
): NodeJS.ProcessEnv {
	const withoutNpmConfig = Object.fromEntries(
		Object.entries(process.env).filter(([key]) => !key.toLowerCase().startsWith("npm_config_")),
	);
	const home = resolve(tempRoot, "home");
	return {
		...withoutNpmConfig,
		HOME: home,
		USERPROFILE: home,
		npm_config_audit: "false",
		npm_config_cache: cachePath,
		npm_config_fund: "false",
		npm_config_globalconfig: globalConfigPath,
		npm_config_logs_max: "0",
		npm_config_update_notifier: "false",
		npm_config_userconfig: userConfigPath,
	};
}

function fixtureResolutionEnvironment(
	base: NodeJS.ProcessEnv,
	registryUrl: string,
): NodeJS.ProcessEnv {
	return {
		...npmResolutionEnvironment(base),
		npm_config_registry: registryUrl,
	};
}

function fixtureRootLockEnvironment(
	base: NodeJS.ProcessEnv,
	registryUrl: string,
): NodeJS.ProcessEnv {
	return {
		...rootLockReplayEnvironment(base),
		npm_config_registry: registryUrl,
	};
}

function createTarball(
	tempRoot: string,
	tarballs: string,
	baseEnvironment: NodeJS.ProcessEnv,
	version: string,
	publishedAt: string,
): FixtureVersion {
	const source = resolve(tempRoot, "package-source", version);
	const packCache = resolve(tempRoot, "pack-cache");
	mkdirSync(source, { recursive: true });
	mkdirSync(packCache, { recursive: true });
	writeFileSync(
		resolve(source, "package.json"),
		`${JSON.stringify({ name: PACKAGE_NAME, version, main: "index.js" }, null, 2)}\n`,
	);
	writeFileSync(resolve(source, "index.js"), `module.exports = ${JSON.stringify(version)};\n`);
	const packed = runNpm(
		`pack fixture ${version}`,
		["pack", source, "--ignore-scripts", "--json", "--pack-destination", tarballs],
		tempRoot,
		{ ...baseEnvironment, npm_config_cache: packCache },
	);
	if (packed.exitCode !== 0) {
		throw new Error(`failed to pack fixture ${version}: ${packed.stderr}`);
	}
	const filename = (JSON.parse(packed.stdout) as Array<{ filename?: string }>)[0]?.filename;
	if (!filename) throw new Error(`npm pack did not report a filename for ${version}`);
	const path = resolve(tarballs, filename);
	const bytes = readFileSync(path);
	return {
		integrity: `sha512-${createHash("sha512").update(bytes).digest("base64")}`,
		path,
		publishedAt,
		shasum: createHash("sha1").update(bytes).digest("hex"),
		version,
	};
}

function startRegistryFixture(tempRoot: string): RegistryFixture {
	const serverPath = resolve(tempRoot, "registry.cjs");
	const readyPath = resolve(tempRoot, "registry-port.txt");
	const statePath = resolve(tempRoot, "proxy-state.json");
	const requestLogPath = resolve(tempRoot, "proxy-requests.ndjson");
	writeFileSync(requestLogPath, "");
	writeFileSync(
		serverPath,
		`const fs = require("node:fs");
const http = require("node:http");
const [readyPath, statePath, requestLogPath, packageName] = process.argv.slice(2);
const server = http.createServer((request, response) => {
  fs.appendFileSync(requestLogPath, JSON.stringify({ method: request.method, url: request.url }) + "\\n");
  const pathname = decodeURIComponent(new URL(request.url, "http://fixture").pathname);
  if (request.method === "POST" && pathname === "/-/npm/v1/security/advisories/bulk") {
    const body = "{}";
    response.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(body) });
    response.end(body);
    return;
  }
  if (request.method === "POST" && pathname === "/-/npm/v1/security/audits/quick") {
    const body = JSON.stringify({
      auditReportVersion: 2,
      vulnerabilities: {},
      metadata: {
        vulnerabilities: { info: 0, low: 0, moderate: 0, high: 0, critical: 0, total: 0 },
        dependencies: { prod: 1, dev: 0, optional: 0, peer: 0, peerOptional: 0, total: 1 }
      }
    });
    response.writeHead(200, { "content-type": "application/json", "content-length": Buffer.byteLength(body) });
    response.end(body);
    return;
  }
  const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
  if (pathname === "/" + packageName) {
    const origin = "http://127.0.0.1:" + server.address().port;
    const versions = {};
    const time = { created: "${OLD_PUBLISHED_AT}", modified: new Date().toISOString() };
    for (const entry of Object.values(state.versions)) {
      versions[entry.version] = {
        name: packageName,
        version: entry.version,
        dist: {
          integrity: entry.integrity,
          shasum: entry.shasum,
          tarball: origin + "/" + packageName + "/-/" + packageName + "-" + entry.version + ".tgz"
        }
      };
      time[entry.version] = entry.publishedAt;
    }
    const body = JSON.stringify({
      name: packageName,
      "dist-tags": state.latest ? { latest: state.latest } : {},
      versions,
      time
    });
    response.writeHead(200, {
      "cache-control": "public, max-age=3600",
      "content-type": "application/json",
      "content-length": Buffer.byteLength(body)
    });
    response.end(body);
    return;
  }
  const prefix = "/" + packageName + "/-/" + packageName + "-";
  if (pathname.startsWith(prefix) && pathname.endsWith(".tgz")) {
    const version = pathname.slice(prefix.length, -4);
    const entry = state.versions[version];
    if (!entry) {
      response.writeHead(404);
      response.end("tarball unavailable");
      return;
    }
    const body = fs.readFileSync(entry.path);
    response.writeHead(200, {
      "content-type": "application/octet-stream",
      "content-length": body.length
    });
    response.end(body);
    return;
  }
  response.writeHead(404);
  response.end("not found");
});
server.listen(0, "127.0.0.1", () => {
  fs.writeFileSync(readyPath, String(server.address().port));
});
process.on("SIGTERM", () => server.close(() => process.exit(0)));
`,
	);
	const child = spawn(
		process.execPath,
		[serverPath, readyPath, statePath, requestLogPath, PACKAGE_NAME],
		{ stdio: "ignore" },
	);
	for (let attempt = 0; attempt < 200; attempt++) {
		if (existsSync(readyPath)) {
			const port = Number(readFileSync(readyPath, "utf8"));
			if (Number.isInteger(port) && port > 0) {
				return {
					child,
					requestLogPath,
					statePath,
					url: `http://127.0.0.1:${port}/`,
				};
			}
		}
		if (child.exitCode !== null) break;
		sleep(25);
	}
	child.kill("SIGTERM");
	throw new Error("local npm registry fixture did not become ready");
}

function parseJsonOutput(command: CommandEvidence): unknown {
	try {
		return JSON.parse(command.stdout);
	} catch (error) {
		const detail = error instanceof Error ? error.message : String(error);
		throw new Error(`${command.name} did not emit parseable JSON: ${detail}`);
	}
}

function main(): void {
	mkdirSync(TEMP_PARENT, { recursive: true });
	const tempRoot = mkdtempSync(resolve(TEMP_PARENT, "release-age-probe-"));
	const packageJsonPath = resolve(PIJ_ROOT, "package.json");
	const packageLockPath = resolve(PIJ_ROOT, "package-lock.json");
	const beforeHashes = {
		packageJson: sha256(packageJsonPath),
		packageLock: sha256(packageLockPath),
	};
	const cache = resolve(tempRoot, "cache");
	const home = resolve(tempRoot, "home");
	const project = resolve(tempRoot, "project");
	const tarballs = resolve(tempRoot, "tarballs");
	const userConfig = resolve(tempRoot, "user.npmrc");
	const globalConfig = resolve(tempRoot, "global.npmrc");
	for (const directory of [cache, home, project, tarballs]) mkdirSync(directory);
	writeFileSync(userConfig, "");
	writeFileSync(globalConfig, "");
	const baseEnvironment = isolatedBaseEnvironment(tempRoot, cache, userConfig, globalConfig);
	const commands: CommandEvidence[] = [];
	let registryFixture: RegistryFixture | null = null;
	let tempCleaned = false;

	try {
		const oldVersion = createTarball(
			tempRoot,
			tarballs,
			baseEnvironment,
			OLD_VERSION,
			OLD_PUBLISHED_AT,
		);
		const youngVersion = createTarball(
			tempRoot,
			tarballs,
			baseEnvironment,
			YOUNG_VERSION,
			new Date().toISOString(),
		);
		registryFixture = startRegistryFixture(tempRoot);
		writeFileSync(
			registryFixture.statePath,
			`${JSON.stringify(
				{
					latest: OLD_VERSION,
					versions: { [OLD_VERSION]: oldVersion, [YOUNG_VERSION]: youngVersion },
				},
				null,
				2,
			)}\n`,
		);
		const governed = fixtureResolutionEnvironment(baseEnvironment, registryFixture.url);

		const config = runNpm(
			"npm resolution config",
			["config", "list", "-l", "--json"],
			project,
			governed,
		);
		commands.push(config);
		const configJson = parseJsonOutput(config) as Record<string, unknown>;

		const youngProject = resolve(project, "young");
		mkdirSync(youngProject);
		writeFileSync(
			resolve(youngProject, "package.json"),
			`${JSON.stringify(
				{
					name: "pij-release-age-young",
					private: true,
					version: "0.0.0",
					dependencies: { [PACKAGE_NAME]: YOUNG_VERSION },
				},
				null,
				2,
			)}\n`,
		);
		const refusal = runNpm(
			"fresh resolution refusal",
			["install", "--package-lock-only", "--ignore-scripts", "--audit=false", "--fund=false"],
			youngProject,
			governed,
		);
		commands.push(refusal);

		const lockedProject = resolve(project, "locked");
		mkdirSync(lockedProject);
		writeFileSync(
			resolve(lockedProject, "package.json"),
			`${JSON.stringify(
				{
					name: "pij-release-age-locked",
					private: true,
					version: "0.0.0",
					dependencies: { [PACKAGE_NAME]: OLD_VERSION },
				},
				null,
				2,
			)}\n`,
		);
		const lockCreation = runNpm(
			"fixture lock creation",
			["install", "--package-lock-only", "--ignore-scripts", "--audit=false", "--fund=false"],
			lockedProject,
			governed,
		);
		commands.push(lockCreation);
		if (lockCreation.exitCode !== 0) {
			throw new Error(`fixture lock creation failed: ${lockCreation.stderr}`);
		}
		const fixtureLockPath = resolve(lockedProject, "package-lock.json");
		const fixtureLockHash = sha256(fixtureLockPath);
		writeFileSync(
			registryFixture.statePath,
			`${JSON.stringify({ latest: YOUNG_VERSION, versions: { [YOUNG_VERSION]: youngVersion } }, null, 2)}\n`,
		);
		rmSync(cache, { recursive: true, force: true });
		mkdirSync(cache);
		const lockReplay = runNpm(
			"exact lock replay after proxy removal",
			rootLockReplayNpmArgs(),
			lockedProject,
			fixtureRootLockEnvironment(baseEnvironment, registryFixture.url),
		);
		commands.push(lockReplay);

		writeFileSync(
			registryFixture.statePath,
			`${JSON.stringify({ latest: OLD_VERSION, versions: { [OLD_VERSION]: oldVersion } }, null, 2)}\n`,
		);
		const audit = runNpm("fixture audit", ["audit", "--json"], lockedProject, governed);
		commands.push(audit);
		const auditJson = parseJsonOutput(audit) as Record<string, unknown>;
		const auditMetadata = auditJson.metadata ?? null;

		const afterHashes = {
			packageJson: sha256(packageJsonPath),
			packageLock: sha256(packageLockPath),
		};
		const configuredBeforeMs = Date.parse(String(configJson.before));
		const expectedBeforeMs = Date.now() - MIN_RELEASE_AGE_DAYS * 86_400_000;
		const refusalOutput = `${refusal.stdout}\n${refusal.stderr}`;
		const checks = {
			auditJsonWasObserved: auditMetadata !== null,
			exactLockAbsenceFailedClosed: lockReplay.exitCode !== 0,
			fixtureLockUnchanged: sha256(fixtureLockPath) === fixtureLockHash,
			freshResolutionWasRefused:
				refusal.exitCode !== 0 &&
				/ETARGET|No matching version/i.test(refusalOutput) &&
				/with a date before/i.test(refusalOutput),
			npmSupportsMinReleaseAge: Object.hasOwn(configJson, "min-release-age"),
			productionPolicyIsSevenDays:
				MIN_RELEASE_AGE_DAYS === 7 &&
				Number.isFinite(configuredBeforeMs) &&
				Math.abs(configuredBeforeMs - expectedBeforeMs) < 5 * 60_000,
			registryHostReplacementIsGoverned:
				configJson["replace-registry-host"] === NPM_REPLACE_REGISTRY_HOST,
			proxyOnly: commands.every((command) => !command.command.some((arg) => /npmjs/i.test(arg))),
			repositoryManifestsUnchanged:
				beforeHashes.packageJson === afterHashes.packageJson &&
				beforeHashes.packageLock === afterHashes.packageLock,
			rootLockUsesOnlyApprovedOverride:
				lockReplay.command.at(-1) === `--min-release-age=${ROOT_LOCK_REPLAY_MIN_RELEASE_AGE}` &&
				!lockReplay.command.includes("install"),
		};
		const requestContent = readFileSync(registryFixture.requestLogPath, "utf8").trim();
		const requestCount = requestContent ? requestContent.split("\n").length : 0;

		registryFixture.child.kill("SIGTERM");
		rmSync(tempRoot, { recursive: true, force: true });
		tempCleaned = !existsSync(tempRoot);
		const result: ProbeResult = {
			productionMinReleaseAgeDays: MIN_RELEASE_AGE_DAYS,
			fixtureRegistry: registryFixture.url,
			commands,
			checks,
			audit: {
				exitCode: audit.exitCode,
				metadata: auditMetadata,
			},
			requestCount,
			tempRoot: relative(PIJ_ROOT, tempRoot),
			tempCleaned,
		};
		console.log(JSON.stringify(result, null, 2));
		if (!Object.values(checks).every(Boolean) || !tempCleaned) {
			process.exitCode = 1;
		}
	} finally {
		if (registryFixture?.child.exitCode === null) {
			registryFixture.child.kill("SIGTERM");
		}
		if (existsSync(tempRoot)) {
			rmSync(tempRoot, { recursive: true, force: true });
		}
	}
}

main();
