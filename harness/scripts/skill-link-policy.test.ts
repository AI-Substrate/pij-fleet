import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const PIJ_ROOT = resolve(import.meta.dirname, "..", "..");
const justfile = readFileSync(resolve(PIJ_ROOT, "justfile"), "utf8");

function recipeBody(name: string): string {
	const lines = justfile.split("\n");
	const start = lines.indexOf(`${name}:`);
	if (start < 0) throw new Error(`missing just recipe: ${name}`);
	const body: string[] = [];
	for (const line of lines.slice(start + 1)) {
		if (!line.startsWith(" ") && !line.startsWith("\t")) break;
		body.push(line);
	}
	return body.join("\n");
}

describe("pij skill link policy", () => {
	it("wires the global skill link into fresh-machine install", () => {
		expect(recipeBody("install")).toContain("just pij-skill-link-global");
	});

	it("guards before mutating the machine-wide skill link", () => {
		const body = recipeBody("pij-skill-link-global");
		const guard = body.indexOf("npm run link -- --check-only");
		const remove = body.indexOf('rm -rf "$target"');
		const link = body.indexOf('ln -sfn "$source" "$target"');
		expect(guard).toBeGreaterThanOrEqual(0);
		expect(remove).toBeGreaterThan(guard);
		expect(link).toBeGreaterThan(remove);
		expect(body).toContain('source="$(realpath skills/pij)"');
		expect(body).toContain('target="$HOME/.agents/skills/pij"');
		expect(body).not.toContain("npx skills");
		expect(body).not.toMatch(/\bcp\b/);
	});

	it("keeps the old install recipe as a link-only alias", () => {
		const body = recipeBody("pij-skill-install");
		expect(body).toContain("just pij-skill-link-global");
		expect(body).not.toContain("npx skills");
		expect(body).not.toContain("rm -rf");
	});
});
