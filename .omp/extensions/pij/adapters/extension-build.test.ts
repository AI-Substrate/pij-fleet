import { execFileSync } from "node:child_process";
import {
	mkdirSync,
	mkdtempSync,
	realpathSync,
	renameSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { removeTemporaryTree } from "../../../../harness/test-utils.js";
import { computeExtensionBuildIdentity } from "./extension-build.js";

const roots: string[] = [];
const sources = [
	["index.ts", "export const extension = true;\n"],
	["adapters/z-last.ts", "export const last = 1;\n"],
	["adapters/a-first.ts", "export const first = 2;\n"],
	["core/state.ts", "export const state = 3;\n"],
] as const;

function tempRoot(): string {
	const root = mkdtempSync(join(tmpdir(), "pij-extension-build-"));
	roots.push(root);
	return root;
}

function writeSource(directory: string, path: string, content: string): void {
	const target = join(directory, path);
	mkdirSync(dirname(target), { recursive: true });
	writeFileSync(target, content);
}

function extensionTree(directory: string, reverse = false): string {
	for (const [path, content] of reverse ? [...sources].reverse() : sources) {
		writeSource(directory, path, content);
	}
	return directory;
}

function git(directory: string, ...args: string[]): string {
	return execFileSync("git", ["-C", directory, ...args], {
		encoding: "utf8",
		stdio: ["ignore", "pipe", "pipe"],
	}).trim();
}

function repository(): { root: string; extension: string; sha: string } {
	const root = tempRoot();
	const extension = extensionTree(join(root, "extension with spaces"));
	writeSource(root, "unrelated.txt", "unrelated\n");
	git(root, "init", "--quiet");
	git(root, "config", "user.email", "pij@example.test");
	git(root, "config", "user.name", "pij test");
	git(root, "add", "--", "extension with spaces", "unrelated.txt");
	git(root, "commit", "--quiet", "-m", "initial");
	return { root, extension, sha: git(root, "rev-parse", "--short=10", "HEAD") };
}

afterEach(() => {
	for (const root of roots.splice(0)) {
		removeTemporaryTree(root);
	}
});

describe("computeExtensionBuildIdentity", () => {
	it("reports the ten-character HEAD for a clean extension", () => {
		const { extension, sha } = repository();

		expect(computeExtensionBuildIdentity(extension)).toEqual({
			extension_build: sha,
			extension_path: realpathSync(extension),
		});
	});

	it.each([
		"unstaged",
		"staged",
		"untracked",
	])("marks the extension dirty for %s extension changes", (change) => {
		const { root, extension, sha } = repository();
		const path = change === "untracked" ? "core/new.ts" : "index.ts";
		writeSource(extension, path, "export const changed = true;\n");
		if (change === "staged") git(root, "add", "--", join(extension, path));

		expect(computeExtensionBuildIdentity(extension).extension_build).toBe(`${sha}+dirty`);
	});

	it("does not mark the extension dirty for unrelated repository changes", () => {
		const { root, extension, sha } = repository();
		writeSource(root, "unrelated.txt", "changed\n");
		git(root, "add", "unrelated.txt");
		writeSource(root, "untracked.txt", "new\n");

		expect(computeExtensionBuildIdentity(extension).extension_build).toBe(sha);
	});

	it("resolves symlinks before choosing the source repository and dirty scope", () => {
		const source = repository();
		const host = repository();
		const linked = join(host.root, "linked extension");
		symlinkSync(source.extension, linked, "dir");
		writeSource(source.extension, "index.ts", "export const changed = true;\n");

		expect(computeExtensionBuildIdentity(linked)).toEqual({
			extension_build: `${source.sha}+dirty`,
			extension_path: realpathSync(source.extension),
		});
	});

	it("falls back to a stable twelve-digit content hash outside git regardless of creation order", () => {
		const root = tempRoot();
		const first = extensionTree(join(root, "first"));
		const second = extensionTree(join(root, "second"), true);
		const identity = computeExtensionBuildIdentity(first);

		expect(identity.extension_build).toMatch(/^hash:[0-9a-f]{12}$/);
		expect(identity.extension_path).toBe(realpathSync(first));
		expect(computeExtensionBuildIdentity(second).extension_build).toBe(identity.extension_build);
	});

	it.each([
		"index.ts",
		"adapters/a-first.ts",
		"core/state.ts",
	])("changes the fallback hash when %s content changes", (path) => {
		const extension = extensionTree(tempRoot());
		const before = computeExtensionBuildIdentity(extension).extension_build;
		writeSource(extension, path, "export const changed = true;\n");

		expect(computeExtensionBuildIdentity(extension).extension_build).not.toBe(before);
	});

	it("includes relative source paths in the fallback hash", () => {
		const extension = extensionTree(tempRoot());
		const before = computeExtensionBuildIdentity(extension).extension_build;
		renameSync(join(extension, "adapters/a-first.ts"), join(extension, "adapters/renamed.ts"));

		expect(computeExtensionBuildIdentity(extension).extension_build).not.toBe(before);
	});

	it("hashes only index.ts and direct TypeScript children of adapters and core", () => {
		const extension = extensionTree(tempRoot());
		const before = computeExtensionBuildIdentity(extension).extension_build;
		writeSource(extension, "other.ts", "ignored\n");
		writeSource(extension, "adapters/data.json", "{}\n");
		writeSource(extension, "adapters/nested/child.ts", "ignored\n");
		writeSource(extension, "core/nested/child.ts", "ignored\n");

		expect(computeExtensionBuildIdentity(extension).extension_build).toBe(before);
	});

	it("frames paths and bytes so source boundaries cannot produce the same fallback hash", () => {
		const root = tempRoot();
		const first = extensionTree(join(root, "first"));
		const second = extensionTree(join(root, "second"));
		writeSource(first, "adapters/a-first.ts", "leftadapters/z-last.ts");
		writeSource(first, "adapters/z-last.ts", "right");
		writeSource(second, "adapters/a-first.ts", "left");
		writeSource(second, "adapters/z-last.ts", "adapters/z-last.tsright");

		expect(computeExtensionBuildIdentity(first).extension_build).not.toBe(
			computeExtensionBuildIdentity(second).extension_build,
		);
	});
});
