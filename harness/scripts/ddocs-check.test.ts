import { describe, expect, it } from "vitest";

import {
	type DirListing,
	findDocuments,
	judgeBuildCheck,
	judgeValidate,
	resolveDdocs,
} from "./ddocs-check.js";

describe("ddocs-check", () => {
	describe("resolveDdocs (ruling C10: PIJ_DDOCS, else PATH)", () => {
		const executables = new Set(["/opt/dd/bin/ddocs.js", "/usr/local/bin/ddocs"]);
		const isExecutable = (path: string) => executables.has(path);

		it("prefers PIJ_DDOCS over PATH", () => {
			expect(
				resolveDdocs({ PIJ_DDOCS: "/opt/dd/bin/ddocs.js", PATH: "/usr/local/bin" }, isExecutable),
			).toEqual({ ok: true, path: "/opt/dd/bin/ddocs.js", via: "PIJ_DDOCS" });
		});

		it("refuses an unusable PIJ_DDOCS instead of falling back to PATH", () => {
			const missing = resolveDdocs(
				{ PIJ_DDOCS: "/nope/ddocs", PATH: "/usr/local/bin" },
				isExecutable,
			);
			expect(missing.ok).toBe(false);
			const relative = resolveDdocs({ PIJ_DDOCS: "ddocs", PATH: "/usr/local/bin" }, isExecutable);
			expect(relative).toMatchObject({ ok: false, reason: expect.stringContaining("absolute") });
		});

		it("finds ddocs on PATH, and reports unavailable when it is nowhere", () => {
			expect(resolveDdocs({ PATH: "/bin:/usr/local/bin" }, isExecutable)).toEqual({
				ok: true,
				path: "/usr/local/bin/ddocs",
				via: "PATH",
			});
			expect(resolveDdocs({ PATH: "/bin" }, isExecutable).ok).toBe(false);
		});

		it("skips relative PATH entries, which would resolve against the cwd", () => {
			const anywhere = () => true;
			expect(resolveDdocs({ PATH: "node_modules/.bin:/usr/local/bin" }, anywhere)).toEqual({
				ok: true,
				path: "/usr/local/bin/ddocs",
				via: "PATH",
			});
			expect(resolveDdocs({ PATH: "bin:." }, anywhere).ok).toBe(false);
		});
	});

	describe("findDocuments", () => {
		const tree: Record<string, Array<{ name: string; isDir: boolean }>> = {
			"docs/project": [
				{ name: "roadmaps", isDir: true },
				{ name: "README.md", isDir: false },
				{ name: "prds", isDir: true },
			],
			"docs/project/prds": [
				{ name: "001-x.dd.json", isDir: false },
				{ name: "001-x.dd.md", isDir: false },
			],
			"docs/project/roadmaps": [
				{ name: "001-x.dd.json", isDir: false },
				{ name: "node_modules", isDir: true },
			],
			"docs/project/roadmaps/node_modules": [{ name: "junk.dd.json", isDir: false }],
			skills: [{ name: "a.dd.json", isDir: false }],
		};
		const listDir = (dir: string): DirListing => {
			const entries = tree[dir];
			return entries
				? { ok: true, entries }
				: { ok: false, error: `EACCES: permission denied, scandir '${dir}'` };
		};

		it("collects .dd.json files under the roots, sorted, skipping node_modules", () => {
			expect(findDocuments(["docs/project"], listDir)).toEqual({
				documents: ["docs/project/prds/001-x.dd.json", "docs/project/roadmaps/001-x.dd.json"],
				errors: [],
			});
		});

		it("reports an unreadable root as an error, never as an empty tree", () => {
			const found = findDocuments(["skills", "docs/locked"], listDir);
			expect(found.documents).toEqual(["skills/a.dd.json"]);
			expect(found.errors).toEqual([
				{ path: "docs/locked", error: expect.stringContaining("permission denied") },
			]);
		});
	});

	describe("judgeValidate", () => {
		const envelope = (status: string, error: number, warn: number) =>
			JSON.stringify({ status, data: { counts: { error, warn } } });

		it("passes only ok with 0 errors, 0 warnings and exit 0", () => {
			expect(judgeValidate("a", 0, envelope("ok", 0, 0)).ok).toBe(true);
			expect(judgeValidate("a", 0, envelope("degraded", 0, 1)).ok).toBe(false);
			expect(judgeValidate("a", 1, envelope("ok", 0, 0)).ok).toBe(false);
		});

		it("fails with ddocs' own message on an error envelope, and on output that is not JSON", () => {
			const error = JSON.stringify({
				status: "error",
				error: { message: 'no entry with id "req-09"' },
			});
			expect(judgeValidate("a", 1, error)).toMatchObject({
				ok: false,
				detail: expect.stringContaining("req-09"),
			});
			expect(judgeValidate("a", 0, "").ok).toBe(false);
		});
	});

	it("judgeBuildCheck fails on drift and passes when the render matches", () => {
		expect(
			judgeBuildCheck("a", 0, JSON.stringify({ status: "ok", data: { drift: false } })).ok,
		).toBe(true);
		const drift = JSON.stringify({
			status: "error",
			error: { message: "a.dd.md drifted from the render" },
		});
		expect(judgeBuildCheck("a", 1, drift)).toMatchObject({
			ok: false,
			detail: expect.stringContaining("drifted"),
		});
	});
});
