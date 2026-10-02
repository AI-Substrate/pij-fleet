// pij-control-plane — tmux capture-format parsing (plan 054 P2 T006, AC-09).
//
// The spawn format extends `-P -F '#{pane_id}'` to '#{pane_id} #{window_id}'
// so terminal addressability (`tmux select-window -t <windowId>`) is captured
// at birth. The parse is pinned here against the REAL format shapes tmux
// emits; the adapter itself stays a thin argv wrapper.

import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { parsePaneAndWindow, TmuxAdapter } from "./tmux.js";

describe("parsePaneAndWindow (real capture-format pin)", () => {
	it("parses '%12 @3\\n' — the extended -F output", () => {
		expect(parsePaneAndWindow("%12 @3\n")).toEqual({ paneId: "%12", windowId: "@3" });
	});

	it("parses a bare pane id (legacy single-field format) with windowId absent", () => {
		expect(parsePaneAndWindow("%12\n")).toEqual({ paneId: "%12" });
	});

	it("a malformed window id degrades to paneId-only — never fails the spawn", () => {
		expect(parsePaneAndWindow("%12 win3\n")).toEqual({ paneId: "%12" });
	});

	it("a malformed pane id is null (the spawn must fail loudly, as today)", () => {
		expect(parsePaneAndWindow("pane12 @3\n")).toBeNull();
		expect(parsePaneAndWindow("")).toBeNull();
		expect(parsePaneAndWindow("@3\n")).toBeNull();
	});
});

describe("TmuxAdapter launch rollback", () => {
	let dir: string | undefined;
	let previousPath: string | undefined;
	afterEach(() => {
		if (previousPath === undefined) delete process.env.PATH;
		else process.env.PATH = previousPath;
		if (dir) rmSync(dir, { recursive: true, force: true });
	});

	it("kills a newly created pane when title assignment fails", () => {
		dir = mkdtempSync(join(tmpdir(), "pij-tmux-rollback-"));
		const log = join(dir, "tmux.log");
		const bin = join(dir, "tmux");
		writeFileSync(
			bin,
			`#!/bin/sh
printf '%s\\n' "$*" >> '${log}'
if [ "$1" = "new-window" ]; then printf '%%42 @9\\n'; exit 0; fi
if [ "$1" = "select-pane" ]; then exit 1; fi
exit 0
`,
		);
		chmodSync(bin, 0o755);
		previousPath = process.env.PATH;
		process.env.PATH = `${dir}:${previousPath ?? ""}`;
		const result = new TmuxAdapter().newWindow({
			cmd: "copilot",
			args: [],
			env: {},
			name: "revive",
			title: "revive title",
		});
		expect(result).toMatchObject({ ok: false });
		expect(readFileSync(log, "utf8")).toContain("kill-pane -t %42");
	});

	it("kills a newly split pane when title assignment fails", () => {
		dir = mkdtempSync(join(tmpdir(), "pij-tmux-split-rollback-"));
		const log = join(dir, "tmux.log");
		const bin = join(dir, "tmux");
		writeFileSync(
			bin,
			`#!/bin/sh
printf '%s\\n' "$*" >> '${log}'
if [ "$1" = "split-window" ]; then printf '%%43 @9\\n'; exit 0; fi
if [ "$1" = "select-pane" ]; then exit 1; fi
exit 0
`,
		);
		chmodSync(bin, 0o755);
		previousPath = process.env.PATH;
		process.env.PATH = `${dir}:${previousPath ?? ""}`;
		const result = new TmuxAdapter().splitWindow({
			cmd: "copilot",
			args: [],
			env: {},
			title: "revive title",
			target: "%1",
			direction: "h",
		});
		expect(result).toMatchObject({ ok: false });
		expect(readFileSync(log, "utf8")).toContain("kill-pane -t %43");
	});
});
