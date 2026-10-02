import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	appendFileSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { createServer, type Server } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { npmInvocation } from "./cli-invocation.js";
import {
	npmResolutionEnvironment,
	quarantineSupportError,
	rootLockReplayEnvironment,
	rootLockReplayNpmArgs,
} from "./release-age-policy.js";

const RUNNING_NPM_VERSION = spawnSync("npm", ["--version"], { encoding: "utf8" }).stdout ?? "";
const PIJ_ROOT_DIR = resolve(import.meta.dirname, "..", "..");

const PACKAGE_NAME = "pij-npm-resolution-fixture";
const OLD_PUBLISHED_AT = "2020-01-01T00:00:00.000Z";
const YOUNG_PUBLISHED_AT = new Date(Date.now() - 86_400_000).toISOString();
const temporaryRoots: string[] = [];

interface PackedVersion {
	integrity: string;
	path: string;
	publishedAt: string;
	shasum: string;
	version: string;
}

interface RegistryVersion extends PackedVersion {
	tarball: "ok" | "missing" | "corrupt";
}

interface RegistryState {
	latest: string | null;
	versions: Record<string, RegistryVersion>;
}

interface RegistryRequest {
	method: string;
	url: string;
}

interface RegistryFixture {
	close(): Promise<void>;
	requests(): RegistryRequest[];
	resetRequests(): void;
	statePath: string;
	url: string;
}

interface CommandResult {
	status: number | null;
	stdout: string;
	stderr: string;
}

interface FixtureRoot {
	cache: string;
	globalConfig: string;
	home: string;
	project: string;
	proxyLog: string;
	proxyState: string;
	root: string;
	tarballs: string;
	upstreamLog: string;
	upstreamState: string;
	userConfig: string;
}

function createFixtureRoot(): FixtureRoot {
	const root = mkdtempSync(join(tmpdir(), "pij-npm-resolution-policy-"));
	temporaryRoots.push(root);
	const fixture = {
		cache: join(root, "cache"),
		globalConfig: join(root, "global.npmrc"),
		home: join(root, "home"),
		project: join(root, "project"),
		proxyLog: join(root, "proxy-requests.ndjson"),
		proxyState: join(root, "proxy-state.json"),
		root,
		tarballs: join(root, "tarballs"),
		upstreamLog: join(root, "upstream-requests.ndjson"),
		upstreamState: join(root, "upstream-state.json"),
		userConfig: join(root, "user.npmrc"),
	};
	for (const directory of [fixture.cache, fixture.home, fixture.project, fixture.tarballs]) {
		mkdirSync(directory);
	}
	for (const file of [
		fixture.globalConfig,
		fixture.proxyLog,
		fixture.upstreamLog,
		fixture.userConfig,
	]) {
		writeFileSync(file, "");
	}
	return fixture;
}

function baseEnvironment(fixture: FixtureRoot): NodeJS.ProcessEnv {
	const withoutNpmConfig = Object.fromEntries(
		Object.entries(process.env).filter(([key]) => !key.toLowerCase().startsWith("npm_config_")),
	);
	return {
		...withoutNpmConfig,
		HOME: fixture.home,
		USERPROFILE: fixture.home,
		npm_config_audit: "false",
		npm_config_cache: fixture.cache,
		npm_config_fetch_retries: "0",
		npm_config_fund: "false",
		npm_config_globalconfig: fixture.globalConfig,
		npm_config_logs_max: "0",
		npm_config_update_notifier: "false",
		npm_config_userconfig: fixture.userConfig,
	};
}

function fixtureResolutionEnvironment(
	fixture: FixtureRoot,
	registryUrl: string,
): NodeJS.ProcessEnv {
	return {
		...npmResolutionEnvironment(baseEnvironment(fixture)),
		npm_config_registry: registryUrl,
	};
}

function fixtureRootLockEnvironment(fixture: FixtureRoot, registryUrl: string): NodeJS.ProcessEnv {
	return {
		...rootLockReplayEnvironment({
			...baseEnvironment(fixture),
			NPM_CONFIG_REPLACE_REGISTRY_HOST: "never",
		}),
		npm_config_registry: registryUrl,
	};
}

function writeState(path: string, state: RegistryState): void {
	writeFileSync(path, `${JSON.stringify(state, null, 2)}\n`);
}

function readState(path: string): RegistryState {
	return JSON.parse(readFileSync(path, "utf8")) as RegistryState;
}

function readRequests(path: string): RegistryRequest[] {
	const content = readFileSync(path, "utf8").trim();
	if (!content) return [];
	return content.split("\n").map((line) => JSON.parse(line) as RegistryRequest);
}

function packageRequests(requests: RegistryRequest[]): RegistryRequest[] {
	return requests.filter(
		({ url }) => decodeURIComponent(new URL(url, "http://fixture").pathname) === `/${PACKAGE_NAME}`,
	);
}

function tarballRequests(requests: RegistryRequest[], version?: string): RegistryRequest[] {
	return requests.filter(({ url }) => {
		const pathname = decodeURIComponent(new URL(url, "http://fixture").pathname);
		if (!pathname.includes(`/${PACKAGE_NAME}/-/`)) return false;
		return version ? pathname.endsWith(`-${version}.tgz`) : true;
	});
}

async function startRegistry(statePath: string, requestLogPath: string): Promise<RegistryFixture> {
	const server: Server = createServer((request, response) => {
		try {
			const requestUrl = request.url ?? "/";
			appendFileSync(
				requestLogPath,
				`${JSON.stringify({ method: request.method ?? "GET", url: requestUrl })}\n`,
			);
			const pathname = decodeURIComponent(new URL(requestUrl, "http://fixture").pathname);
			const state = readState(statePath);
			if (pathname === `/${PACKAGE_NAME}`) {
				const address = server.address();
				if (!address || typeof address === "string")
					throw new Error("registry address unavailable");
				const origin = `http://127.0.0.1:${address.port}`;
				const versions = Object.fromEntries(
					Object.values(state.versions).map((entry) => [
						entry.version,
						{
							name: PACKAGE_NAME,
							version: entry.version,
							dist: {
								integrity: entry.integrity,
								shasum: entry.shasum,
								tarball: `${origin}/${PACKAGE_NAME}/-/${PACKAGE_NAME}-${entry.version}.tgz`,
							},
						},
					]),
				);
				const times = Object.fromEntries(
					Object.values(state.versions).map((entry) => [entry.version, entry.publishedAt]),
				);
				const body = JSON.stringify({
					name: PACKAGE_NAME,
					"dist-tags": state.latest ? { latest: state.latest } : {},
					versions,
					time: {
						created: OLD_PUBLISHED_AT,
						modified: new Date().toISOString(),
						...times,
					},
				});
				response.writeHead(200, {
					"cache-control": "public, max-age=3600",
					"content-length": Buffer.byteLength(body),
					"content-type": "application/json",
				});
				response.end(body);
				return;
			}
			const tarballMatch = pathname.match(
				new RegExp(`^/${PACKAGE_NAME}/-/${PACKAGE_NAME}-(.+)\\.tgz$`),
			);
			if (tarballMatch?.[1]) {
				const entry = state.versions[tarballMatch[1]];
				if (!entry || entry.tarball === "missing") {
					response.writeHead(404, { "content-type": "text/plain" });
					response.end("tarball unavailable");
					return;
				}
				const body =
					entry.tarball === "corrupt" ? Buffer.from("corrupt tarball") : readFileSync(entry.path);
				response.writeHead(200, {
					"content-length": body.length,
					"content-type": "application/octet-stream",
				});
				response.end(body);
				return;
			}
			response.writeHead(404, { "content-type": "text/plain" });
			response.end("not found");
		} catch (error) {
			response.writeHead(500, { "content-type": "text/plain" });
			response.end(error instanceof Error ? error.message : String(error));
		}
	});
	await new Promise<void>((resolveListen, reject) => {
		server.once("error", reject);
		server.listen(0, "127.0.0.1", () => {
			server.off("error", reject);
			resolveListen();
		});
	});
	const address = server.address();
	if (!address || typeof address === "string") throw new Error("registry did not bind a TCP port");
	return {
		close: () =>
			new Promise<void>((resolveClose, reject) => {
				server.close((error) => (error ? reject(error) : resolveClose()));
			}),
		requests: () => readRequests(requestLogPath),
		resetRequests: () => writeFileSync(requestLogPath, ""),
		statePath,
		url: `http://127.0.0.1:${address.port}/`,
	};
}

function createPackedVersion(
	fixture: FixtureRoot,
	version: string,
	publishedAt: string,
): PackedVersion {
	const source = join(fixture.root, "package-sources", version);
	const packCache = join(fixture.root, "pack-cache");
	mkdirSync(source, { recursive: true });
	mkdirSync(packCache, { recursive: true });
	writeFileSync(
		join(source, "package.json"),
		`${JSON.stringify({ name: PACKAGE_NAME, version, main: "index.js" }, null, 2)}\n`,
	);
	writeFileSync(join(source, "index.js"), `module.exports = ${JSON.stringify(version)};\n`);
	const invocation = npmInvocation([
		"pack",
		source,
		"--ignore-scripts",
		"--json",
		"--pack-destination",
		fixture.tarballs,
	]);
	const packed = spawnSync(invocation.file, invocation.args, {
		cwd: fixture.root,
		encoding: "utf8",
		env: { ...baseEnvironment(fixture), npm_config_cache: packCache },
	});
	if (packed.error || packed.status !== 0) {
		throw new Error(
			`failed to create fixture tarball ${version}: ${packed.error?.message ?? packed.stderr}`,
		);
	}
	const output = JSON.parse(packed.stdout) as Array<{ filename?: string }>;
	const filename = output[0]?.filename;
	if (!filename) throw new Error(`npm pack did not report a filename for ${version}`);
	const path = resolve(fixture.tarballs, filename);
	const bytes = readFileSync(path);
	return {
		integrity: `sha512-${createHash("sha512").update(bytes).digest("base64")}`,
		path,
		publishedAt,
		shasum: createHash("sha1").update(bytes).digest("hex"),
		version,
	};
}

function registryVersion(
	entry: PackedVersion,
	tarball: RegistryVersion["tarball"] = "ok",
): RegistryVersion {
	return { ...entry, tarball };
}

function createProject(fixture: FixtureRoot, name: string, version: string): string {
	const project = join(fixture.project, name);
	mkdirSync(project);
	writeFileSync(
		join(project, "package.json"),
		`${JSON.stringify(
			{
				name: `pij-${name}`,
				private: true,
				version: "0.0.0",
				dependencies: { [PACKAGE_NAME]: version },
			},
			null,
			2,
		)}\n`,
	);
	return project;
}

function runNpm(args: string[], cwd: string, env: NodeJS.ProcessEnv): Promise<CommandResult> {
	const invocation = npmInvocation(args);
	return new Promise((resolveRun) => {
		const child = spawn(invocation.file, invocation.args, {
			cwd,
			env,
			stdio: ["ignore", "pipe", "pipe"],
		});
		let stdout = "";
		let stderr = "";
		child.stdout.setEncoding("utf8");
		child.stderr.setEncoding("utf8");
		child.stdout.on("data", (chunk: string) => {
			stdout += chunk;
		});
		child.stderr.on("data", (chunk: string) => {
			stderr += chunk;
		});
		child.once("error", (error) => {
			resolveRun({ status: null, stdout, stderr: `${stderr}\n${error.message}` });
		});
		child.once("close", (status) => {
			resolveRun({ status, stdout, stderr });
		});
	});
}

function resetCache(fixture: FixtureRoot): void {
	rmSync(fixture.cache, { recursive: true, force: true });
	mkdirSync(fixture.cache);
}

function installArgs(): string[] {
	return ["install", "--ignore-scripts", "--audit=false", "--fund=false"];
}

afterEach(() => {
	for (const root of temporaryRoots.splice(0)) {
		rmSync(root, { recursive: true, force: true });
	}
});

describe("authoritative npm resolution", () => {
	it("revalidates stale metadata, fails closed on proxy gaps, enforces age, and never falls back", async () => {
		const fixture = createFixtureRoot();
		const one = createPackedVersion(fixture, "1.0.0", OLD_PUBLISHED_AT);
		const two = createPackedVersion(fixture, "2.0.0", OLD_PUBLISHED_AT);
		const young = createPackedVersion(fixture, "3.0.0", YOUNG_PUBLISHED_AT);
		const upstreamOnly = createPackedVersion(fixture, "9.0.0", OLD_PUBLISHED_AT);
		writeState(fixture.proxyState, {
			latest: "2.0.0",
			versions: { "2.0.0": registryVersion(two) },
		});
		writeState(fixture.upstreamState, {
			latest: "9.0.0",
			versions: {
				"2.0.0": registryVersion(two),
				"9.0.0": registryVersion(upstreamOnly),
			},
		});
		const proxy = await startRegistry(fixture.proxyState, fixture.proxyLog);
		const upstream = await startRegistry(fixture.upstreamState, fixture.upstreamLog);
		try {
			const governed = fixtureResolutionEnvironment(fixture, proxy.url);
			const prime = await runNpm(
				["view", PACKAGE_NAME, "version", "--json"],
				fixture.project,
				governed,
			);
			expect(prime.status).toBe(0);
			expect(JSON.parse(prime.stdout)).toBe("2.0.0");
			expect(packageRequests(proxy.requests())).not.toHaveLength(0);

			writeState(proxy.statePath, {
				latest: "1.0.0",
				versions: { "1.0.0": registryVersion(one) },
			});
			proxy.resetRequests();
			const withoutOnline = { ...governed };
			delete withoutOnline.npm_config_prefer_online;
			const staleControl = await runNpm(
				installArgs(),
				createProject(fixture, "stale-control", "*"),
				withoutOnline,
			);
			expect(
				staleControl.status,
				JSON.stringify(
					{
						requests: proxy.requests(),
						stderr: staleControl.stderr,
						stdout: staleControl.stdout,
					},
					null,
					2,
				),
			).not.toBe(0);
			expect(packageRequests(proxy.requests())).toHaveLength(0);
			expect(tarballRequests(proxy.requests(), "2.0.0")).not.toHaveLength(0);

			proxy.resetRequests();
			const recoveredProject = createProject(fixture, "recovered", "*");
			const recovered = await runNpm(installArgs(), recoveredProject, governed);
			expect(recovered.status).toBe(0);
			expect(packageRequests(proxy.requests())).not.toHaveLength(0);
			expect(
				JSON.parse(
					readFileSync(
						join(recoveredProject, "node_modules", PACKAGE_NAME, "package.json"),
						"utf8",
					),
				),
			).toMatchObject({ version: "1.0.0" });
			expect(upstream.requests()).toHaveLength(0);

			resetCache(fixture);
			writeState(proxy.statePath, {
				latest: "2.0.0",
				versions: { "2.0.0": registryVersion(two, "missing") },
			});
			proxy.resetRequests();
			const inconsistent = await runNpm(
				installArgs(),
				createProject(fixture, "inconsistent", "2.0.0"),
				fixtureResolutionEnvironment(fixture, proxy.url),
			);
			expect(inconsistent.status).not.toBe(0);
			expect(packageRequests(proxy.requests())).not.toHaveLength(0);
			expect(tarballRequests(proxy.requests(), "2.0.0")).not.toHaveLength(0);
			expect(upstream.requests()).toHaveLength(0);

			resetCache(fixture);
			writeState(proxy.statePath, {
				latest: "2.0.0",
				versions: { "2.0.0": registryVersion(two, "corrupt") },
			});
			proxy.resetRequests();
			upstream.resetRequests();
			const corrupt = await runNpm(
				installArgs(),
				createProject(fixture, "corrupt", "2.0.0"),
				fixtureResolutionEnvironment(fixture, proxy.url),
			);
			expect(corrupt.status).not.toBe(0);
			expect(tarballRequests(proxy.requests(), "2.0.0")).not.toHaveLength(0);
			expect(upstream.requests()).toHaveLength(0);

			resetCache(fixture);
			writeState(proxy.statePath, {
				latest: "2.0.0",
				versions: { "2.0.0": registryVersion(two) },
			});
			const lockedProject = createProject(fixture, "locked", "2.0.0");
			const lockCreation = await runNpm(
				["install", "--package-lock-only", "--ignore-scripts", "--audit=false", "--fund=false"],
				lockedProject,
				fixtureResolutionEnvironment(fixture, proxy.url),
			);
			expect(lockCreation.status).toBe(0);
			const lockPath = join(lockedProject, "package-lock.json");
			const lock = JSON.parse(readFileSync(lockPath, "utf8")) as {
				packages?: Record<string, { resolved?: string }>;
			};
			const lockedEntry = lock.packages?.[`node_modules/${PACKAGE_NAME}`];
			expect(lockedEntry).toBeDefined();
			if (!lockedEntry) throw new Error("fixture lock entry was not created");
			// npmjs-scoped host replacement (PR#25 adopt): npm's lockfile-replay path
			// replaces the host but preserves the URL scheme. Match this HTTP fixture's
			// scheme so the replay proves host routing rather than TLS setup.
			// Lockfile-source integrity for non-npmjs hosts remains owned by the
			// deterministic lockfile-allowlist check, not silent install-time redirection.
			lockedEntry.resolved = `http://registry.npmjs.org/${PACKAGE_NAME}/-/${PACKAGE_NAME}-2.0.0.tgz`;
			writeFileSync(lockPath, `${JSON.stringify(lock, null, 2)}\n`);
			const lockBefore = createHash("sha256").update(readFileSync(lockPath)).digest("hex");

			resetCache(fixture);
			rmSync(join(lockedProject, "node_modules"), { recursive: true, force: true });
			proxy.resetRequests();
			upstream.resetRequests();
			const governedLockHostReplay = await runNpm(
				rootLockReplayNpmArgs(),
				lockedProject,
				fixtureRootLockEnvironment(fixture, proxy.url),
			);
			expect(
				governedLockHostReplay.status,
				JSON.stringify(
					{ stderr: governedLockHostReplay.stderr, stdout: governedLockHostReplay.stdout },
					null,
					2,
				),
			).toBe(0);
			expect(tarballRequests(proxy.requests(), "2.0.0")).not.toHaveLength(0);
			expect(upstream.requests()).toHaveLength(0);
			expect(createHash("sha256").update(readFileSync(lockPath)).digest("hex")).toBe(lockBefore);

			writeState(proxy.statePath, { latest: null, versions: {} });
			resetCache(fixture);
			rmSync(join(lockedProject, "node_modules"), { recursive: true, force: true });
			proxy.resetRequests();
			upstream.resetRequests();
			const lockReplay = await runNpm(
				rootLockReplayNpmArgs(),
				lockedProject,
				fixtureRootLockEnvironment(fixture, proxy.url),
			);
			expect(lockReplay.status).not.toBe(0);
			expect(tarballRequests(proxy.requests(), "2.0.0")).not.toHaveLength(0);
			expect(upstream.requests()).toHaveLength(0);
			expect(createHash("sha256").update(readFileSync(lockPath)).digest("hex")).toBe(lockBefore);

			resetCache(fixture);
			writeState(proxy.statePath, {
				latest: "3.0.0",
				versions: { "3.0.0": registryVersion(young) },
			});
			const tooYoungProject = createProject(fixture, "too-young", "3.0.0");
			if (quarantineSupportError(RUNNING_NPM_VERSION) === null) {
				const tooYoung = await runNpm(
					installArgs(),
					tooYoungProject,
					fixtureResolutionEnvironment(fixture, proxy.url),
				);
				expect(tooYoung.status).not.toBe(0);
				expect(`${tooYoung.stdout}\n${tooYoung.stderr}`).toMatch(/ETARGET|No matching version/i);
			} else {
				proxy.resetRequests();
				const refused = spawnSync(
					join(PIJ_ROOT_DIR, "node_modules/.bin/tsx"),
					[join(PIJ_ROOT_DIR, "harness/scripts/npm-resolution-run.ts"), ...installArgs()],
					{
						cwd: tooYoungProject,
						encoding: "utf8",
						env: fixtureResolutionEnvironment(fixture, proxy.url),
					},
				);
				expect(refused.status).not.toBe(0);
				expect(`${refused.stdout}\n${refused.stderr}`).toContain(
					"min-release-age requires npm>=11.10.0",
				);
				expect(proxy.requests()).toHaveLength(0);
			}

			resetCache(fixture);
			const ageZeroEnvironment = fixtureResolutionEnvironment(fixture, proxy.url);
			ageZeroEnvironment.npm_config_min_release_age = "0";
			const ageZero = await runNpm(
				installArgs(),
				createProject(fixture, "age-zero-mutation", "3.0.0"),
				ageZeroEnvironment,
			);
			expect(ageZero.status).toBe(0);

			resetCache(fixture);
			const upstreamMutation = fixtureResolutionEnvironment(fixture, upstream.url);
			const mutationResult = await runNpm(
				["view", `${PACKAGE_NAME}@9.0.0`, "version", "--json"],
				fixture.project,
				upstreamMutation,
			);
			expect(mutationResult.status).toBe(0);
			expect(upstream.requests()).not.toHaveLength(0);
			upstream.resetRequests();

			writeState(proxy.statePath, {
				latest: "1.0.0",
				versions: { "1.0.0": registryVersion(one) },
			});
			proxy.resetRequests();
			const noFallback = await runNpm(
				installArgs(),
				createProject(fixture, "no-fallback", "9.0.0"),
				fixtureResolutionEnvironment(fixture, proxy.url),
			);
			expect(noFallback.status).not.toBe(0);
			expect(packageRequests(proxy.requests())).not.toHaveLength(0);
			expect(upstream.requests()).toHaveLength(0);
		} finally {
			await Promise.all([proxy.close(), upstream.close()]);
		}
	}, 120_000);
});
