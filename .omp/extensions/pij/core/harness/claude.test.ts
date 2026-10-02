import { describe, expect, it } from "vitest";

import {
	buildInitInjection,
	discoverNewTranscript,
	mangleCwd,
	summarizeTranscriptLine,
	transcriptDir,
	transcriptPathFor,
	transcriptSessionId,
} from "./claude.js";

describe("mangleCwd", () => {
	it("matches the live telemetry convention (every non-alnum → '-')", () => {
		// Verified against the real ~/.claude/projects tree.
		expect(mangleCwd("/Users/jordanknight/pi-hacking/pij")).toBe(
			"-Users-jordanknight-pi-hacking-pij",
		);
		expect(mangleCwd("/Users/jordanknight/github/jk-claw")).toBe(
			"-Users-jordanknight-github-jk-claw",
		);
	});

	it("collapses dots and underscores to '-' too (non-alnum is non-alnum)", () => {
		expect(mangleCwd("/a/b.c_d")).toBe("-a-b-c-d");
	});
});

describe("transcriptDir", () => {
	it("is ~/.claude/projects/<mangled cwd>", () => {
		expect(transcriptDir("/Users/jo", "/Users/jo/pi-hacking/pij")).toBe(
			"/Users/jo/.claude/projects/-Users-jo-pi-hacking-pij",
		);
	});
});

describe("transcriptSessionId", () => {
	it("is the basename stem without the .jsonl suffix", () => {
		expect(
			transcriptSessionId("/Users/jo/.claude/projects/-Users-jo-x/a83382a0-aed1-4264-9573.jsonl"),
		).toBe("a83382a0-aed1-4264-9573");
	});
});

describe("discoverNewTranscript (new path appearance, NOT mtime)", () => {
	const DIR = "/Users/jo/.claude/projects/-Users-jo-pi-hacking-pij";
	const PREEXISTING = `${DIR}/old-active-session.jsonl`;
	const FRESH = `${DIR}/new-spawned-session.jsonl`;

	it("binds the path that did NOT exist at spawn", () => {
		const r = discoverNewTranscript([PREEXISTING], [PREEXISTING, FRESH]);
		expect(r).toEqual({
			status: "found",
			path: FRESH,
			sessionId: "new-spawned-session",
		});
	});

	it("NEVER chooses a pre-existing active transcript in the same cwd (the load-bearing case)", () => {
		// The pre-existing session's mtime would advance as it works — discovery
		// must ignore that and only consider path appearance.
		const r = discoverNewTranscript([PREEXISTING], [PREEXISTING]);
		expect(r.status).toBe("pending");
	});

	it("returns pending until the new file appears", () => {
		expect(discoverNewTranscript([], []).status).toBe("pending");
	});

	it("flags concurrent boots (two new paths) as ambiguous", () => {
		const a = `${DIR}/boot-a.jsonl`;
		const b = `${DIR}/boot-b.jsonl`;
		const r = discoverNewTranscript([], [a, b]);
		expect(r.status).toBe("ambiguous");
		if (r.status === "ambiguous") expect(r.paths).toEqual([a, b]);
	});

	it("ignores non-jsonl noise in the directory listing", () => {
		const r = discoverNewTranscript([], [`${DIR}/.DS_Store`, FRESH]);
		expect(r).toMatchObject({ status: "found", path: FRESH });
	});
});

describe("transcriptPathFor", () => {
	it("is <dir>/<harnessSessionId>.jsonl", () => {
		expect(transcriptPathFor("/Users/jo", "/Users/jo/proj", "abc-123")).toBe(
			"/Users/jo/.claude/projects/-Users-jo-proj/abc-123.jsonl",
		);
	});
});

describe("summarizeTranscriptLine (pij tail of a bound claude session, AC-09)", () => {
	it("extracts user text (string content)", () => {
		expect(
			summarizeTranscriptLine(JSON.stringify({ type: "user", message: { content: "hi there" } })),
		).toEqual({ role: "user", text: "hi there" });
	});

	it("extracts assistant text + tool_use names from array content", () => {
		const line = JSON.stringify({
			type: "assistant",
			message: {
				content: [
					{ type: "text", text: "On it." },
					{ type: "tool_use", name: "Bash" },
				],
			},
		});
		expect(summarizeTranscriptLine(line)).toEqual({ role: "assistant", text: "On it. ⚙ Bash" });
	});

	it("returns null for non-conversational lines (mode/system/snapshot) and bad JSON", () => {
		expect(summarizeTranscriptLine(JSON.stringify({ type: "mode", mode: "x" }))).toBeNull();
		expect(summarizeTranscriptLine(JSON.stringify({ type: "system" }))).toBeNull();
		expect(summarizeTranscriptLine("not json")).toBeNull();
	});
});

describe("buildInitInjection", () => {
	it("carries the pij-id and the confirmatory phonehome line", () => {
		const init = buildInitInjection("pij-abc");
		expect(init.pijId).toBe("pij-abc");
		expect(init.phonehomeLine).toBe("pij phonehome");
		expect(init.body).toContain("pij-abc");
		expect(init.body).toContain("pij phonehome");
	});

	it("a normal (non-branch) init carries NO fork reframe", () => {
		const init = buildInitInjection("pij-abc");
		expect(init.body).not.toMatch(/FORK/i);
		expect(init.body.startsWith("You are now a pij peer")).toBe(true);
	});

	it("a branched init reframes the fork (context-only, don't resume/spawn) — Finding 08, T016", () => {
		const init = buildInitInjection("pij-fork", true);
		expect(init.body).toMatch(/FORK/);
		expect(init.body).toMatch(/CONTEXT only|NOT a task/i);
		expect(init.body).toMatch(/do not (continue|spawn)/i);
		// still a valid peer init: keeps the id + phonehome confirmatory line
		expect(init.body).toContain("pij-fork");
		expect(init.body).toContain("pij phonehome");
		expect(init.phonehomeLine).toBe("pij phonehome");
	});

	it("a revived init restores context without continuing prior work", () => {
		const init = buildInitInjection("pij-revived", false, "pij-parent", true);
		expect(init.body).toMatch(/REVIVED/);
		expect(init.body).toMatch(/context only/i);
		expect(init.body).toMatch(/do not continue the old work/i);
		expect(init.body).toContain("pij-revived");
		expect(init.body).toContain("pij phonehome");
	});

	it("names the spawner and gives the concrete reply form when spawnedBy is set", () => {
		const init = buildInitInjection("pij-child", false, "pij-parent");
		expect(init.body).toContain("spawned by pij-parent");
		// the peer boots pij-blind, so it must be handed the exact reply command
		expect(init.body).toContain('pij send pij-parent "<text>"');
		// self id + phonehome confirmatory line still present
		expect(init.body).toContain("pij-child");
		expect(init.body).toContain("pij phonehome");
	});

	it("omits the spawner clause for an adopted/root peer (no spawnedBy)", () => {
		const init = buildInitInjection("pij-root");
		expect(init.body).not.toMatch(/spawned by/i);
		expect(init.body).not.toMatch(/reply to your spawner/i);
		expect(init.body.startsWith("You are now a pij peer")).toBe(true);
	});
});
