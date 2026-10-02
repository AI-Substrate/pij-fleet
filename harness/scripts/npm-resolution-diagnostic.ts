import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { npmInvocation } from "./cli-invocation.js";
import {
	MIN_RELEASE_AGE_DAYS,
	NPM_PREFER_ONLINE,
	NPM_REGISTRY_URL,
	NPM_REPLACE_REGISTRY_HOST,
	npmResolutionEnvironment,
} from "./release-age-policy.js";

type DiagnosticVerdict =
	| "PROXY_OK"
	| "PROXY_ABSENT"
	| "PROXY_INCONSISTENT"
	| "POLICY_TOO_YOUNG"
	| "DIAGNOSTIC_ERROR";

interface ExactTarget {
	name: string;
	target: string;
	version: string;
}

interface CommandResult {
	status: number | null;
	stdout: string;
	stderr: string;
}

interface DiagnosticOutcome {
	target: string;
	verdict: DiagnosticVerdict;
	detail?: string;
	failingUrl?: string;
}

const EXIT_CODES: Record<DiagnosticVerdict, number> = {
	PROXY_OK: 0,
	PROXY_ABSENT: 2,
	PROXY_INCONSISTENT: 3,
	POLICY_TOO_YOUNG: 4,
	DIAGNOSTIC_ERROR: 5,
};

function parseExactTarget(input: string | undefined): ExactTarget | null {
	if (!input) return null;
	const separator = input.lastIndexOf("@");
	if (separator <= 0) return null;
	const name = input.slice(0, separator);
	const version = input.slice(separator + 1);
	const validName = name.startsWith("@")
		? /^@[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._-]*$/i.test(name)
		: /^[a-z0-9][a-z0-9._-]*$/i.test(name);
	const exactVersion =
		/^\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?(?:\+[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?$/.test(
			version,
		);
	return validName && exactVersion ? { name, target: input, version } : null;
}

function isolatedEnvironment(tempRoot: string): NodeJS.ProcessEnv {
	const withoutNpmConfig = Object.fromEntries(
		Object.entries(process.env).filter(([key]) => !key.toLowerCase().startsWith("npm_config_")),
	);
	const home = join(tempRoot, "home");
	const cache = join(tempRoot, "cache");
	const userConfig = join(tempRoot, "user.npmrc");
	const globalConfig = join(tempRoot, "global.npmrc");
	mkdirSync(home);
	mkdirSync(cache);
	writeFileSync(userConfig, "");
	writeFileSync(globalConfig, "");
	return npmResolutionEnvironment({
		...withoutNpmConfig,
		HOME: home,
		USERPROFILE: home,
		npm_config_cache: cache,
		npm_config_userconfig: userConfig,
		npm_config_globalconfig: globalConfig,
		npm_config_update_notifier: "false",
		npm_config_logs_max: "0",
	});
}

function runNpm(args: string[], cwd: string, env: NodeJS.ProcessEnv): CommandResult {
	const invocation = npmInvocation(args);
	const result = spawnSync(invocation.file, invocation.args, {
		cwd,
		encoding: "utf8",
		env,
		maxBuffer: 20 * 1024 * 1024,
	});
	if (result.error) {
		return {
			status: null,
			stdout: result.stdout,
			stderr: `${result.stderr}\n${result.error.message}`,
		};
	}
	return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function failingProxyUrl(output: string): string | undefined {
	const escaped = NPM_REGISTRY_URL.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
	return output.match(new RegExp(`${escaped}[^\\s"'<>]*`, "i"))?.[0];
}

function outcome(
	target: string,
	verdict: DiagnosticVerdict,
	detail?: string,
	failingUrl?: string,
): DiagnosticOutcome {
	return {
		target,
		verdict,
		...(detail ? { detail: detail.replaceAll(/\s+/g, " ").trim() } : {}),
		...(failingUrl ? { failingUrl } : {}),
	};
}

function printOutcome(result: DiagnosticOutcome): void {
	console.log(`registry=${NPM_REGISTRY_URL}`);
	console.log(`replace-registry-host=${NPM_REPLACE_REGISTRY_HOST}`);
	console.log(`prefer-online=${String(NPM_PREFER_ONLINE)}`);
	console.log(`min-release-age=${MIN_RELEASE_AGE_DAYS}`);
	console.log(`target=${result.target}`);
	console.log(`verdict=${result.verdict}`);
	if (result.failingUrl) console.log(`proxy-url=${result.failingUrl}`);
	if (result.detail) console.log(`detail=${result.detail}`);
}

function main(): DiagnosticOutcome {
	const rawTarget = process.argv[2];
	const target = parseExactTarget(rawTarget);
	if (!target) {
		return outcome(
			rawTarget ?? "<missing>",
			"DIAGNOSTIC_ERROR",
			"usage: npm-resolution-diagnostic.ts <exact-package-version>",
		);
	}

	const tempRoot = mkdtempSync(join(tmpdir(), "pij-npm-resolution-diagnostic-"));
	const project = join(tempRoot, "project");
	const downloads = join(tempRoot, "downloads");
	try {
		mkdirSync(project);
		mkdirSync(downloads);
		const env = isolatedEnvironment(tempRoot);
		writeFileSync(
			join(project, "package.json"),
			`${JSON.stringify({ name: "pij-npm-diagnostic", private: true, version: "0.0.0" }, null, 2)}\n`,
		);

		const metadata = runNpm(["view", target.target, "version", "time", "--json"], project, env);
		const metadataOutput = `${metadata.stdout}\n${metadata.stderr}`;
		if (metadata.status !== 0) {
			const verdict = /\bE404\b|\bETARGET\b|no match found|not found in this registry/i.test(
				metadataOutput,
			)
				? "PROXY_ABSENT"
				: "DIAGNOSTIC_ERROR";
			return outcome(
				target.target,
				verdict,
				metadata.stderr || metadata.stdout,
				failingProxyUrl(metadataOutput),
			);
		}

		let publishedAt: string;
		try {
			const parsed = JSON.parse(metadata.stdout) as {
				time?: Record<string, string>;
				version?: string;
			};
			publishedAt = parsed.time?.[target.version] ?? "";
			if (parsed.version !== target.version || !Number.isFinite(Date.parse(publishedAt))) {
				return outcome(
					target.target,
					"DIAGNOSTIC_ERROR",
					"proxy metadata omitted exact publication time",
				);
			}
		} catch (error) {
			const detail = error instanceof Error ? error.message : String(error);
			return outcome(
				target.target,
				"DIAGNOSTIC_ERROR",
				`proxy metadata was not parseable: ${detail}`,
			);
		}

		writeFileSync(
			join(project, "package.json"),
			`${JSON.stringify(
				{
					name: "pij-npm-diagnostic",
					private: true,
					version: "0.0.0",
					dependencies: { [target.name]: target.version },
				},
				null,
				2,
			)}\n`,
		);
		const resolution = runNpm(
			["install", "--package-lock-only", "--ignore-scripts", "--audit=false", "--fund=false"],
			project,
			env,
		);
		if (resolution.status !== 0) {
			const publishedMs = Date.parse(publishedAt);
			const cutoffMs = Date.now() - MIN_RELEASE_AGE_DAYS * 86_400_000;
			const verdict = publishedMs > cutoffMs ? "POLICY_TOO_YOUNG" : "DIAGNOSTIC_ERROR";
			const output = `${resolution.stdout}\n${resolution.stderr}`;
			return outcome(
				target.target,
				verdict,
				resolution.stderr || resolution.stdout,
				failingProxyUrl(output),
			);
		}

		const packed = runNpm(
			["pack", target.target, "--ignore-scripts", "--json", "--pack-destination", downloads],
			project,
			env,
		);
		if (packed.status !== 0) {
			const output = `${packed.stdout}\n${packed.stderr}`;
			return outcome(
				target.target,
				"PROXY_INCONSISTENT",
				packed.stderr || packed.stdout,
				failingProxyUrl(output),
			);
		}

		return outcome(target.target, "PROXY_OK");
	} finally {
		rmSync(tempRoot, { recursive: true, force: true });
	}
}

const result = main();
printOutcome(result);
process.exitCode = EXIT_CODES[result.verdict];
