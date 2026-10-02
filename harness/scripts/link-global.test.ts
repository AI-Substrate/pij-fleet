import { execFileSync, spawnSync } from "node:child_process";
import {
	existsSync,
	fstatSync,
	fsyncSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	rmSync,
	symlinkSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { runLinkGlobal } from "./link-global.js";

const scratch: string[] = [];

function tempDir(prefix: string): string {
	const path = mkdtempSync(join(tmpdir(), prefix));
	scratch.push(path);
	return path;
}

function checkout(): string {
	const root = tempDir("pij-link-source-");
	mkdirSync(join(root, ".git"), { recursive: true });
	writeFileSync(join(root, "package.json"), JSON.stringify({ name: "pij" }));
	mkdirSync(join(root, ".omp", "extensions", "pij"), { recursive: true });
	writeFileSync(join(root, ".omp", "models.yml"), "providers: {}\n");
	writeFileSync(join(root, ".omp", "mcp.json"), "{}\n");
	mkdirSync(join(root, ".copilot", "extensions", "pij"), { recursive: true });
	for (const name of ["extension.mjs", "store.mjs"]) {
		writeFileSync(join(root, ".copilot", "extensions", "pij", name), "export {};\n");
	}
	return root;
}

function run(
	root: string,
	home: string,
	args: readonly string[] = [],
	fsync?: (fd: number) => void,
): {
	code: number;
	stdout: string[];
	stderr: string[];
} {
	const stdout: string[] = [];
	const stderr: string[] = [];
	const code = runLinkGlobal({
		pijRoot: root,
		home,
		fsync,
		args,
		stdout: (line) => stdout.push(line),
		stderr: (line) => stderr.push(line),
	});
	return { code, stdout, stderr };
}

afterEach(() => {
	for (const path of scratch.splice(0)) rmSync(path, { recursive: true, force: true });
});

describe("link-global", () => {
	it("links only pij into OMP with the curated MCP + model config", () => {
		const root = checkout();
		const home = tempDir("pij-link-home-");

		expect(run(root, home)).toMatchObject({ code: 0, stderr: [] });
		expect(readdirSync(join(home, ".omp", "agent", "extensions"))).toEqual(["pij"]);
		expect(readlinkSync(join(home, ".omp", "agent", "extensions", "pij"))).toBe(
			join(root, ".omp", "extensions", "pij"),
		);
		expect(readlinkSync(join(home, ".omp", "agent", "mcp.json"))).toBe(
			join(root, ".omp", "mcp.json"),
		);
		expect(readlinkSync(join(home, ".omp", "agent", "models.yml"))).toBe(
			join(root, ".omp", "models.yml"),
		);
		expect(existsSync(join(home, ".pi"))).toBe(false);
		expect(run(root, home, ["--doctor-omp"])).toMatchObject({ code: 0, stderr: [] });
	});

	it("migrates links made through the retired Pi layout", () => {
		const root = checkout();
		const home = tempDir("pij-link-home-");
		const ompAgent = join(home, ".omp", "agent");
		const piMcp = join(home, ".pi", "agent", "mcp.json");
		mkdirSync(join(ompAgent, "extensions"), { recursive: true });
		mkdirSync(join(home, ".pi", "agent"), { recursive: true });
		writeFileSync(piMcp, "{}\n");
		// Before pi was retired: OMP reached pij via <repo>/.pi/extensions/pij (now
		// gone) and shared Pi's live MCP file.
		symlinkSync(join(root, ".pi", "extensions", "pij"), join(ompAgent, "extensions", "pij"));
		symlinkSync(piMcp, join(ompAgent, "mcp.json"));

		const result = run(root, home);

		expect(result).toMatchObject({ code: 0, stderr: [] });
		expect(readlinkSync(join(ompAgent, "extensions", "pij"))).toBe(
			join(root, ".omp", "extensions", "pij"),
		);
		expect(readlinkSync(join(ompAgent, "mcp.json"))).toBe(join(root, ".omp", "mcp.json"));
		expect(readFileSync(piMcp, "utf8")).toBe("{}\n");
		expect(run(root, home, ["--doctor-omp"])).toMatchObject({ code: 0, stderr: [] });
	});

	it("prunes only pij-owned non-pij OMP extension links", () => {
		const root = checkout();
		const oldRoot = checkout();
		const home = tempDir("pij-link-home-");
		const ompExtensions = join(home, ".omp", "agent", "extensions");
		mkdirSync(ompExtensions, { recursive: true });
		symlinkSync(join(oldRoot, ".pi", "extensions", "todo"), join(ompExtensions, "todo"));

		expect(run(root, home).code).toBe(0);
		expect(existsSync(join(ompExtensions, "todo"))).toBe(false);
		expect(readlinkSync(join(ompExtensions, "pij"))).toBe(join(root, ".omp", "extensions", "pij"));
	});

	it("never clobbers a foreign symlink or real directory", () => {
		const root = checkout();
		const home = tempDir("pij-link-home-");
		const foreign = tempDir("foreign-extension-");
		const ompExtensions = join(home, ".omp", "agent", "extensions");
		mkdirSync(join(ompExtensions, "todo"), { recursive: true });
		writeFileSync(join(home, ".omp", "agent", "models.yml"), "providers: {}\n");
		symlinkSync(foreign, join(ompExtensions, "pij"));

		const result = run(root, home);

		expect(result.code).toBe(1);
		expect(readlinkSync(join(ompExtensions, "pij"))).toBe(foreign);
		expect(lstatSync(join(ompExtensions, "todo")).isDirectory()).toBe(true);
		expect(result.stderr.join("\n")).toContain("refusing to replace foreign symlink");
		expect(readFileSync(join(home, ".omp", "agent", "models.yml"), "utf8")).toBe("providers: {}\n");
		expect(result.stderr.join("\n")).toContain("models.yml: real file; refusing to clobber");
		expect(result.stderr.join("\n")).toContain("real directory");
	});

	it("reclaims a dangling OMP symlink but still refuses a live foreign one", () => {
		const root = checkout();
		const home = tempDir("pij-link-home-");
		const gone = tempDir("deleted-worktree-");
		const dead = join(gone, ".omp", "models.yml");
		mkdirSync(join(home, ".omp", "agent"), { recursive: true });
		symlinkSync(dead, join(home, ".omp", "agent", "models.yml"));
		expect(existsSync(join(home, ".omp", "agent", "models.yml"))).toBe(false);

		const reclaimed = run(root, home);
		expect(reclaimed.stderr.join("\n")).toContain("reclaiming dangling symlink");
		expect(readlinkSync(join(home, ".omp", "agent", "models.yml"))).toBe(
			join(root, ".omp", "models.yml"),
		);

		const live = tempDir("other-checkout-");
		mkdirSync(join(live, ".omp"), { recursive: true });
		writeFileSync(join(live, ".omp", "models.yml"), "providers: {}\n");
		rmSync(join(home, ".omp", "agent", "models.yml"));
		symlinkSync(join(live, ".omp", "models.yml"), join(home, ".omp", "agent", "models.yml"));

		const refused = run(root, home);
		expect(refused.stderr.join("\n")).toContain("refusing to replace foreign symlink");
		expect(readlinkSync(join(home, ".omp", "agent", "models.yml"))).toBe(
			join(live, ".omp", "models.yml"),
		);
	});

	it("worktree invocation exits 1 before either machine home changes", () => {
		const canonicalRoot = tempDir("pij-link-canonical-");
		const linkedRoot = tempDir("pij-link-worktree-");
		const gitDir = join(canonicalRoot, ".git", "worktrees", "fixture");
		mkdirSync(gitDir, { recursive: true });
		writeFileSync(join(linkedRoot, ".git"), `gitdir: ${gitDir}\n`);
		const home = tempDir("pij-link-home-");
		const marker = join(home, "marker.txt");
		writeFileSync(marker, "unchanged\n");

		const result = run(linkedRoot, home);

		expect(result.code).toBe(1);
		expect(result.stderr.join("\n")).toContain("linked worktree");
		expect(result.stderr.join("\n")).toContain(`run \`just link\` from ${canonicalRoot}`);
		expect(readFileSync(marker, "utf8")).toBe("unchanged\n");
		expect(existsSync(join(home, ".pi"))).toBe(false);
		expect(existsSync(join(home, ".omp"))).toBe(false);
		expect(existsSync(join(home, ".copilot"))).toBe(false);
	});

	it("check-only validates a canonical checkout without touching HOME", () => {
		const root = checkout();
		const home = tempDir("pij-link-home-");
		const result = run(root, home, ["--check-only"]);

		expect(result).toMatchObject({ code: 0, stderr: [] });
		expect(existsSync(join(home, ".pi"))).toBe(false);
		expect(existsSync(join(home, ".omp"))).toBe(false);
		expect(existsSync(join(home, ".copilot"))).toBe(false);
	});
});

describe("Copilot managed installation", () => {
	it("syncs complete settings before replacement and the directory afterward", () => {
		const root = checkout();
		const home = tempDir("pij-copilot-sync-");
		const config = join(home, ".copilot");
		mkdirSync(config);
		const settings = join(config, "settings.json");
		const raw = '{"experimental":false,"theme":"light"}';
		writeFileSync(settings, raw);
		const stages: string[] = [];
		const descriptors: number[] = [];
		const result = run(root, home, [], (fd) => {
			const directory = fstatSync(fd).isDirectory();
			stages.push(directory ? "directory" : "file");
			descriptors.push(fd);
			if (directory) expect(JSON.parse(readFileSync(settings, "utf8")).experimental).toBe(true);
			else expect(readFileSync(settings, "utf8")).toBe(raw);
			fsyncSync(fd);
		});
		expect(result.code).toBe(0);
		expect(stages).toEqual(["file", "directory"]);
		for (const fd of descriptors) expect(() => fstatSync(fd)).toThrow();
		expect(JSON.parse(readFileSync(settings, "utf8")).theme).toBe("light");
		expect(readdirSync(config).some((name) => name.endsWith(".tmp"))).toBe(false);
		expect(
			run(root, home, [], () => {
				throw new Error("already enabled must not rewrite");
			}).code,
		).toBe(0);
	});

	it.each([
		{ stage: "file", code: "EIO", status: 1, enabled: false },
		{ stage: "file", code: "EINVAL", status: 1, enabled: false },
		{ stage: "directory", code: "EIO", status: 1, enabled: true },
		{ stage: "directory", code: "EPERM", status: 1, enabled: true },
		{ stage: "directory", code: "EINVAL", status: 0, enabled: true },
		{ stage: "directory", code: "ENOTSUP", status: 0, enabled: true },
	])("handles $stage fsync $code without hiding real errors", ({
		stage,
		code,
		status,
		enabled,
	}) => {
		const root = checkout();
		const home = tempDir("pij-copilot-sync-error-");
		const config = join(home, ".copilot");
		mkdirSync(config);
		const settings = join(config, "settings.json");
		writeFileSync(settings, '{"experimental":false,"keep":1}');
		let failedFd: number | undefined;
		const result = run(root, home, [], (fd) => {
			if ((fstatSync(fd).isDirectory() ? "directory" : "file") === stage) {
				failedFd = fd;
				throw Object.assign(new Error(`injected fsync ${code}`), { code });
			}
			fsyncSync(fd);
		});
		expect(result.code).toBe(status);
		expect(failedFd).toBeDefined();
		expect(() => fstatSync(failedFd as number)).toThrow();
		expect(JSON.parse(readFileSync(settings, "utf8"))).toEqual({ experimental: enabled, keep: 1 });
		expect(readdirSync(config).some((name) => name.endsWith(".tmp"))).toBe(false);
		if (status) expect(result.stderr.join("\n")).toContain(`injected fsync ${code}`);
		else expect(result.stderr).toEqual([]);
	});

	it("does not report everything linked when only Copilot link or settings change", () => {
		const root = checkout();
		const home = tempDir("pij-copilot-report-");
		expect(run(root, home).code).toBe(0);
		expect(run(root, home).stdout).toContain("everything already linked");
		const config = join(home, ".copilot");
		unlinkSync(join(config, "extensions", "pij"));
		const relinked = run(root, home);
		expect(relinked.code).toBe(0);
		expect(relinked.stdout).not.toContain("everything already linked");
		writeFileSync(join(config, "settings.json"), '{"experimental":false}');
		const enabled = run(root, home);
		expect(enabled.code).toBe(0);
		expect(enabled.stdout).not.toContain("everything already linked");
		expect(run(root, home).stdout).toContain("everything already linked");
	});

	it("links reviewed modules and enables experimental without replacing unknown settings", () => {
		const root = checkout();
		const home = tempDir("pij-copilot-home-");
		const config = join(home, ".copilot");
		mkdirSync(config);
		const settings = join(config, "settings.json");
		writeFileSync(
			settings,
			JSON.stringify({ experimental: false, theme: "light", nested: { keep: 1 } }),
		);
		expect(run(root, home).code).toBe(0);
		expect(readlinkSync(join(config, "extensions", "pij"))).toBe(
			join(root, ".copilot", "extensions", "pij"),
		);
		expect(JSON.parse(readFileSync(settings, "utf8"))).toEqual({
			experimental: true,
			theme: "light",
			nested: { keep: 1 },
		});
		const first = readFileSync(settings, "utf8");
		const inode = lstatSync(settings).ino;
		expect(run(root, home).code).toBe(0);
		expect(readFileSync(settings, "utf8")).toBe(first);
		expect(lstatSync(settings).ino).toBe(inode);
		expect(run(root, home, ["--doctor-copilot"]).code).toBe(0);
	});

	it("treats empty COPILOT_HOME as the default, matching shell :- semantics", () => {
		const root = checkout();
		const home = tempDir("pij-copilot-empty-");
		expect(
			runLinkGlobal({
				pijRoot: root,
				home,
				copilotHome: "",
				args: [],
				stdout: () => {},
				stderr: () => {},
			}),
		).toBe(0);
		expect(JSON.parse(readFileSync(join(home, ".copilot", "settings.json"), "utf8"))).toEqual({
			experimental: true,
		});
	});

	it("honors explicit COPILOT_HOME without touching the default root", () => {
		const root = checkout();
		const home = tempDir("pij-copilot-home-");
		const copilotHome = join(tempDir("pij-copilot-alt-"), "config");
		const stderr: string[] = [];
		expect(
			runLinkGlobal({
				pijRoot: root,
				home,
				copilotHome,
				args: [],
				stdout: () => {},
				stderr: (line) => stderr.push(line),
			}),
		).toBe(0);
		expect(stderr).toEqual([]);
		expect(existsSync(join(home, ".copilot"))).toBe(false);
		expect(JSON.parse(readFileSync(join(copilotHome, "settings.json"), "utf8")).experimental).toBe(
			true,
		);
	});

	it.each([
		"foreign-link",
		"dangling-link",
		"directory",
		"file",
	])("preserves a %s at the install destination", (kind) => {
		const root = checkout();
		const home = tempDir("pij-copilot-home-");
		const config = join(home, ".copilot");
		const target = join(config, "extensions", "pij");
		const foreign = tempDir("foreign-copilot-");
		mkdirSync(join(config, "extensions"), { recursive: true });
		if (kind === "directory") mkdirSync(target);
		else if (kind === "file") writeFileSync(target, "preserve me");
		else symlinkSync(kind === "dangling-link" ? join(foreign, "absent") : foreign, target);
		const before = lstatSync(target).ino;
		const result = run(root, home);
		expect(result.code).toBe(1);
		expect(result.stderr.join("\n")).toContain("refusing");
		expect(lstatSync(target).ino).toBe(before);
		expect(existsSync(join(config, "settings.json"))).toBe(false);
	});

	it.each([
		"{broken",
		"null",
		"[]",
		"true",
	])("preserves malformed/non-object settings %s without linking", (raw) => {
		const root = checkout();
		const home = tempDir("pij-copilot-home-");
		const config = join(home, ".copilot");
		mkdirSync(config);
		writeFileSync(join(config, "settings.json"), raw);
		expect(run(root, home).code).toBe(1);
		expect(readFileSync(join(config, "settings.json"), "utf8")).toBe(raw);
		expect(existsSync(join(config, "extensions", "pij"))).toBe(false);
	});

	it.each([
		"root",
		"extensions",
		"settings",
	])("refuses symlinked %s without touching its referent", (kind) => {
		const root = checkout();
		const home = tempDir("pij-copilot-home-");
		const config = join(home, ".copilot");
		const foreign = tempDir("foreign-copilot-");
		const marker = join(foreign, "settings.json");
		writeFileSync(marker, "{}");
		if (kind === "root") symlinkSync(foreign, config);
		else {
			mkdirSync(config);
			symlinkSync(
				kind === "settings" ? marker : foreign,
				join(config, kind === "settings" ? "settings.json" : "extensions"),
			);
		}
		expect(run(root, home).code).toBe(1);
		expect(readFileSync(marker, "utf8")).toBe("{}");
		expect(readdirSync(foreign)).toEqual(["settings.json"]);
	});

	it("migrates only a repo-owned link and unlink preserves experimental settings", () => {
		const root = checkout();
		const oldRoot = checkout();
		const home = tempDir("pij-copilot-home-");
		const target = join(home, ".copilot", "extensions", "pij");
		mkdirSync(join(home, ".copilot", "extensions"), { recursive: true });
		symlinkSync(join(oldRoot, ".copilot", "extensions", "pij"), target);
		expect(run(root, home).code).toBe(0);
		expect(readlinkSync(target)).toBe(join(root, ".copilot", "extensions", "pij"));
		const settings = readFileSync(join(home, ".copilot", "settings.json"), "utf8");
		expect(run(root, home, ["--remove"]).code).toBe(0);
		expect(existsSync(target)).toBe(false);
		expect(readFileSync(join(home, ".copilot", "settings.json"), "utf8")).toBe(settings);
		expect(run(root, home, ["--remove"]).code).toBe(0);
	});

	it("doctor is read-only and diagnoses disabled settings and missing modules", () => {
		const root = checkout();
		const home = tempDir("pij-copilot-home-");
		expect(run(root, home, ["--doctor-copilot"]).code).toBe(1);
		expect(readdirSync(home)).toEqual([]);
		expect(run(root, home).code).toBe(0);
		writeFileSync(join(home, ".copilot", "settings.json"), '{"experimental":false}');
		expect(run(root, home, ["--doctor-copilot"]).stderr.join("\n")).toContain("experimental");
		writeFileSync(join(home, ".copilot", "settings.json"), '{"experimental":true}');
		rmSync(join(root, ".copilot", "extensions", "pij", "store.mjs"));
		expect(run(root, home, ["--doctor-copilot"]).code).toBe(1);
	});
});

describe("Copilot optional bootstrap check", () => {
	it.each([
		"absent",
		"present",
		"broken",
	] as const)("keeps the %s CLI outcome distinct from strict doctor", (kind) => {
		const root = tempDir("pij-copilot-recipe-");
		const home = join(root, "home");
		const bin = join(root, "bin");
		mkdirSync(home);
		mkdirSync(bin);
		const just = execFileSync("which", ["just"], { encoding: "utf8" }).trim();
		symlinkSync(just, join(bin, "just"));
		symlinkSync("/bin/sh", join(bin, "sh"));
		writeFileSync(
			join(root, "justfile"),
			readFileSync(join(import.meta.dirname, "../../justfile")),
		);
		writeFileSync(join(bin, "npm"), '#!/bin/sh\nprintf "%s\\n" "$@" > "$HOME/npm-args"\n', {
			mode: 0o700,
		});
		if (kind !== "absent")
			writeFileSync(
				join(bin, "copilot"),
				`#!/bin/sh\necho fixture-copilot\nexit ${kind === "broken" ? 9 : 0}\n`,
				{ mode: 0o700 },
			);
		const invoke = (recipe: string) =>
			spawnSync(just, ["--no-dotenv", recipe], {
				cwd: root,
				env: { HOME: home, PATH: bin },
				encoding: "utf8",
				timeout: 10_000,
			});
		const optional = invoke("_copilot-native-install-check");
		expect(optional.error).toBeUndefined();
		expect(optional.status).toBe(kind === "broken" ? 9 : 0);
		if (kind === "absent") expect(optional.stdout).toContain("skipping optional native doctor");
		else expect(optional.stdout).not.toContain("skipping");
		const strict = invoke("copilot-native-doctor");
		expect(strict.error).toBeUndefined();
		if (kind === "present") {
			expect(strict.status).toBe(0);
			expect(readFileSync(join(home, "npm-args"), "utf8")).toBe(
				"run\nlink\n--\n--doctor-copilot\n",
			);
		} else {
			expect(strict.status).not.toBe(0);
			expect(existsSync(join(home, "npm-args"))).toBe(false);
		}
	});
});
