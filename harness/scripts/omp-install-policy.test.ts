import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const PIJ_ROOT = resolve(import.meta.dirname, "..", "..");
const justfile = readFileSync(resolve(PIJ_ROOT, "justfile"), "utf8");

function escapeRegExp(value: string): string {
	return value.replace(/[.*+?^${}()|[\]\\-]/g, "\\$&");
}

/**
 * Body of a single recipe. Matches the header with or without parameters
 * (`_omp-binary-install *REF:`), and never an assignment (`name := value`).
 */
function recipeBody(name: string): string {
	const lines = justfile.split("\n");
	const header = new RegExp(`^${escapeRegExp(name)}(\\s+[^:]*?)?\\s*:(?!=)`);
	const start = lines.findIndex((line) => header.test(line));
	if (start < 0) throw new Error(`missing just recipe: ${name}`);
	const body: string[] = [];
	for (const line of lines.slice(start + 1)) {
		// Indented lines belong to the recipe; a blank line inside one is legal.
		// Anything else at column 0 (next recipe, comment, setting) ends it.
		if (line.trim() !== "" && !line.startsWith(" ") && !line.startsWith("\t")) break;
		body.push(line);
	}
	return body.join("\n");
}

function recipeExists(name: string): boolean {
	try {
		recipeBody(name);
		return true;
	} catch {
		return false;
	}
}

/**
 * A recipe's body plus the bodies of every recipe it delegates to, transitively.
 *
 * The OMP install/update logic is shared through `just _omp-*` helpers, so the
 * supply-chain assertions below must see through that delegation — otherwise
 * extracting a helper would silently retire the guard rather than move it.
 */
function resolvedRecipeBody(name: string, seen = new Set<string>()): string {
	if (seen.has(name)) return "";
	seen.add(name);
	const body = recipeBody(name);
	const parts = [body];
	// Not line-anchored: delegations also appear inside command substitutions,
	// e.g. `latest=$(just _omp-latest-release || true)`.
	for (const [, dep] of body.matchAll(/\bjust\s+([A-Za-z_][\w-]*)/g)) {
		if (recipeExists(dep)) parts.push(resolvedRecipeBody(dep, seen));
	}
	return parts.join("\n");
}

function hostsIn(body: string): string[] {
	const hosts = [...body.matchAll(/https:\/\/([^/\s"')]+)/g)].map(([, host]) => host);
	return [...new Set(hosts)].sort();
}

describe("OMP management recipes", () => {
	it("installs from the governed npm proxy when omp is absent", () => {
		const body = recipeBody("omp-install");
		const resolved = resolvedRecipeBody("omp-install");
		expect(body).toContain("command -v omp");
		// The install itself is delegated; the guard follows it through.
		expect(body).toContain("just _omp-npm-install");
		// `_npm-resolution` is what supplies the governed environment (configured
		// registry, replace-registry-host, `before`, min-release-age). Installing
		// with a bare `npm` here would silently escape all four.
		expect(resolved).toContain("just _npm-resolution");
		expect(resolved).toContain("@oh-my-pi/pi-coding-agent@latest");
		expect(body).toContain("just link");
		expect(body).toContain("just omp-doctor");
	});

	it("keeps the GitHub-releases escape hatch pinned to the official host", () => {
		// `_omp-binary-install` is no longer reached automatically, but it survives
		// for deliberate recovery — so repointing it at another host still fails here.
		const helper = recipeBody("_omp-binary-install");
		expect(helper).toContain("https://omp.sh/install");
		expect(hostsIn(helper)).toEqual(["omp.sh"]);

		// Every invocation of the downloaded installer must ask for the official
		// prebuilt binary. Asserting the flag merely appears somewhere would still
		// pass if one call site quietly dropped it and fell back to a source build.
		const invocations = [...helper.matchAll(/^\s*sh\s+"\$tmp".*$/gm)].map(([line]) => line);
		expect(invocations.length).toBeGreaterThan(0);
		for (const invocation of invocations) {
			expect(invocation).toContain("--binary");
		}
	});

	it("updates through the governed proxy, not omp's built-in updater", () => {
		const body = recipeBody("update-omp");
		const resolved = resolvedRecipeBody("update-omp");

		// omp's updater compiles registry.npmjs.org in, so it honours neither
		// .npmrc nor NPM_CONFIG_REGISTRY: it would fetch a version the quarantine
		// has not cleared, on a machine that cannot even reach that host.
		expect(body).not.toMatch(/\bomp update\b/);
		// The GitHub-releases installer is a different HOST, not a different
		// registry — reaching it proves the network works and proves nothing about
		// the supply chain. It must never be an automatic fallback again.
		expect(body).not.toContain("just _omp-binary-install");

		expect(resolved).toContain("just _npm-resolution");
		expect(resolved).toContain("@oh-my-pi/pi-coding-agent@latest");
		expect(body).toContain("just link");
		expect(body).toContain("just omp-doctor");
	});

	it("hardcodes no host in the update path — the registry comes from .npmrc", () => {
		// The teeth: the governed update must name no host at all. A URL appearing
		// in this recipe is a registry decision moved out of .npmrc and into the
		// justfile, where the proxy, `before` and min-release-age do not reach it.
		expect(hostsIn(recipeBody("update-omp"))).toEqual([]);

		// Resolved, the only host reachable is omp.sh, and only from the smoke
		// check's recovery HINT text (`just _omp-binary-install v17.1.2`), which
		// the delegation walker follows into. No install action fetches it.
		expect(hostsIn(resolvedRecipeBody("update-omp"))).toEqual(["omp.sh"]);
	});

	it("proves an installed omp actually runs before reporting success", () => {
		// A version delta alone is not proof of a good install: a byte-complete,
		// correctly signed binary can still be killed on launch. Both entry points
		// must reach the smoke check, or a broken install reports success.
		expect(recipeBody("_omp-binary-install")).toContain("just _omp-smoke-check");
		expect(resolvedRecipeBody("omp-install")).toContain("omp --version");
		expect(resolvedRecipeBody("update-omp")).toContain("just _omp-smoke-check");
	});

	it("does not relax npm supply-chain policy to complete an update", () => {
		const resolved = resolvedRecipeBody("update-omp");
		// Each of these would WIDEN what the install accepts: clearing the age
		// quarantine, repointing the registry, or silencing the audit.
		expect(resolved).not.toMatch(/min-release-age/);
		expect(resolved).not.toMatch(/npm_config_registry/i);
		expect(resolved).not.toMatch(/\baudit\s*=\s*false\b/);
	});

	it("installs omp with scripts disabled", () => {
		// `--ignore-scripts` is a BRAKE, not a relaxation: removing it lets an
		// install-time script run arbitrary code, so its absence is the risk and
		// its presence is the control. An earlier revision of this file asserted
		// the opposite by lumping it in with genuine policy relaxations above.
		expect(resolvedRecipeBody("update-omp")).toContain("--ignore-scripts");
	});

	it("doctor checks version, pij-only extension inventory, and shared MCP config", () => {
		const body = recipeBody("omp-doctor");
		expect(body).toContain("omp --version");
		expect(body).toContain("--doctor-omp");
	});
});
