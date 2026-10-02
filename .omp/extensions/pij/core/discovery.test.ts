import { describe, expect, it } from "vitest";

import {
	deriveHarnessPijId,
	deriveSelfId,
	detectPiRuntime,
	excludeSelf,
	filterByFolder,
	filterPrime,
	isSubagentChild,
	memorableIdentitySeed,
	resolveSelf,
	selectByRepository,
} from "./discovery.js";
import type { RepositoryIdentityPort } from "./ports.js";
import type { SessionDescriptor } from "./types.js";

function desc(id: string, folder: string): SessionDescriptor {
	return {
		id,
		folder,
		dataDir: `/home/u/.pij/${id}`,
		eventsPath: `/home/u/.pij/${id}/events.ndjson`,
		pid: 100,
		startedAt: "2026-06-16T00:00:00.000Z",
	};
}

const a = desc("a", "/work/proj");
const b = desc("b", "/work/proj");
const c = desc("c", "/work/other");

describe("detectPiRuntime", () => {
	it("recognises OMP's Bun launcher even when argv names an internal CLI file", () => {
		expect(detectPiRuntime({ bun: true, executableNames: ["bun", "cli.js"] })).toBe("omp");
	});

	it("keeps plain Node-hosted Pi distinct", () => {
		expect(detectPiRuntime({ bun: false, executableNames: ["node", "pi"] })).toBe("pi");
	});
});

describe("filterByFolder", () => {
	it("keeps only descriptors in the folder", () => {
		expect(filterByFolder([a, b, c], "/work/proj").map((d) => d.id)).toEqual(["a", "b"]);
	});

	describe("selectByRepository", () => {
		class FakeRepository implements RepositoryIdentityPort {
			readonly calls: string[] = [];

			constructor(private readonly keys: Readonly<Record<string, string | null>>) {}

			gitCommonDir(folder: string): string | null {
				this.calls.push(folder);
				return this.keys[folder] ?? null;
			}
		}

		it("groups persisted and legacy descriptors across linked worktrees", () => {
			const repo = new FakeRepository({
				"/repo/main": "/repo/main/.git",
				"/repo/worktree": "/repo/main/.git",
				"/other": "/other/.git",
			});
			const persisted = { ...desc("persisted", "/repo/main"), gitCommonDir: "/repo/main/.git" };
			const legacyWorktree = desc("legacy-worktree", "/repo/worktree");
			const unrelated = desc("unrelated", "/other");

			const selection = selectByRepository(
				[persisted, legacyWorktree, unrelated],
				"/repo/worktree",
				repo,
			);

			expect(selection.gitCommonDir).toBe("/repo/main/.git");
			expect(selection.descriptors.map((descriptor) => descriptor.id)).toEqual([
				"persisted",
				"legacy-worktree",
			]);
			expect(selection.unresolved).toEqual([]);
			expect(repo.calls).not.toContain("/repo/main");
		});

		it("reports a non-git query path explicitly", () => {
			const repo = new FakeRepository({});
			expect(selectByRepository([a], "/tmp/not-a-repo", repo)).toEqual({
				gitCommonDir: null,
				descriptors: [],
				unresolved: [],
			});
		});

		it("reports missing legacy folders without treating them as repository matches", () => {
			const missing = desc("missing", "/deleted/worktree");
			const repo = new FakeRepository({ "/repo/main": "/repo/main/.git" });
			const selection = selectByRepository([a, missing], "/repo/main", repo);
			expect(selection.descriptors).toEqual([]);
			expect(selection.unresolved).toEqual([a, missing]);
		});
	});

	describe("filterPrime", () => {
		it("keeps only explicit prime=true descriptors and composes with folder filtering", () => {
			const primeHere = { ...a, prime: true };
			const notPrime = { ...b, prime: false };
			const primeElsewhere = { ...c, prime: true };
			expect(filterPrime([primeHere, notPrime, primeElsewhere]).map((d) => d.id)).toEqual([
				"a",
				"c",
			]);
			expect(
				filterPrime(filterByFolder([primeHere, notPrime, primeElsewhere], "/work/proj")),
			).toEqual([primeHere]);
		});
	});
});

describe("excludeSelf", () => {
	it("drops the self id", () => {
		expect(excludeSelf([a, b], "a").map((d) => d.id)).toEqual(["b"]);
	});
});

describe("resolveSelf", () => {
	it("env id wins", () => {
		const r = resolveSelf("a", [a, b]);
		expect(r.ok).toBe(true);
		if (r.ok) expect(r.value).toBe("a");
	});
	it("falls back to the lone local descriptor", () => {
		const r = resolveSelf(undefined, [b]);
		expect(r.ok).toBe(true);
		if (r.ok) expect(r.value).toBe("b");
	});
	it("E-AMBIG when env unset + multiple local", () => {
		const r = resolveSelf("", [a, b]);
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.code).toBe("E-AMBIG");
	});
	it("E-AMBIG when env unset + no local", () => {
		const r = resolveSelf(undefined, []);
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.code).toBe("E-AMBIG");
	});
	it("disambiguates by $TMUX_PANE when env unset + multiple local (feedback #1)", () => {
		const ap = { ...a, paneId: "%1" };
		const bp = { ...b, paneId: "%2" };
		const r = resolveSelf(undefined, [ap, bp], "%2");
		expect(r.ok).toBe(true);
		if (r.ok) expect(r.value).toBe("b");
	});
	it("pane hint that matches nothing stays E-AMBIG", () => {
		const ap = { ...a, paneId: "%1" };
		const bp = { ...b, paneId: "%2" };
		const r = resolveSelf(undefined, [ap, bp], "%9");
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.code).toBe("E-AMBIG");
	});
	it("env still wins over a pane hint", () => {
		const ap = { ...a, paneId: "%1" };
		const bp = { ...b, paneId: "%2" };
		const r = resolveSelf("a", [ap, bp], "%2");
		expect(r.ok).toBe(true);
		if (r.ok) expect(r.value).toBe("a");
	});
	it("ambiguous error lists candidate ids and the export hint", () => {
		const r = resolveSelf("", [a, b]);
		expect(r.ok).toBe(false);
		if (!r.ok) {
			expect(r.message).toContain("a, b");
			expect(r.message).toContain("export PIJ_SESSION_ID=a");
			expect(r.message).toContain("pij adopt");
		}
	});
});

describe("deriveSelfId", () => {
	it("derives a stable pij id from a pi session id", () => {
		const id = deriveSelfId("sess-ABC123", 4242);
		expect(id).toMatch(/^pij-[a-z0-9]+$/);
		// deterministic across /reload: same input => same id
		expect(deriveSelfId("sess-ABC123", 4242)).toBe(id);
		// independent of pid (a /new in the same process keeps the pid but not the id)
		expect(deriveSelfId("sess-ABC123", 9999)).toBe(id);
	});

	it("gives different ids for different session ids (/new => new peer)", () => {
		expect(deriveSelfId("sess-OLD", 4242)).not.toBe(deriveSelfId("sess-NEW", 4242));
	});

	it("falls back to pij-<pid> when no session id is available", () => {
		expect(deriveSelfId(undefined, 4242)).toBe("pij-4242");
		expect(deriveSelfId("", 4242)).toBe("pij-4242");
		expect(deriveSelfId("   ", 4242)).toBe("pij-4242");
	});
});

describe("deriveHarnessPijId", () => {
	it("is deterministic for an exact harness-native identity tuple", () => {
		const id = deriveHarnessPijId("claude", "native-session-1");
		expect(id).toMatch(/^pij-[a-z0-9]+$/);
		expect(deriveHarnessPijId("claude", "native-session-1")).toBe(id);
	});

	it("namespaces the same native id by harness", () => {
		expect(deriveHarnessPijId("claude", "shared-id")).not.toBe(
			deriveHarnessPijId("copilot", "shared-id"),
		);
	});
});

describe("memorableIdentitySeed", () => {
	it("is stable and namespaces the same native id by harness", () => {
		expect(memorableIdentitySeed("claude", "native-1")).toBe(
			memorableIdentitySeed("claude", "native-1"),
		);
		expect(memorableIdentitySeed("claude", "shared")).not.toBe(
			memorableIdentitySeed("copilot", "shared"),
		);
	});

	it("keeps the legacy FNV id available only as a migration lookup candidate", () => {
		expect(deriveHarnessPijId("claude", "native-1")).toMatch(/^pij-[a-z0-9]+$/);
		expect(memorableIdentitySeed("claude", "native-1")).not.toBe(
			deriveHarnessPijId("claude", "native-1"),
		);
	});
});

describe("isSubagentChild", () => {
	it("recognises OMP in-process task sessions without process-global child flags", () => {
		expect(
			isSubagentChild(
				{},
				{
					runtimeBin: "omp",
					mode: "print",
					entries: [{ type: "session_init" }],
				},
			),
		).toBe(true);
	});
	it.each([
		{ runtimeBin: "omp", mode: "tui", entries: [{ type: "session_init" }] },
		{ runtimeBin: "omp", mode: "print", entries: [{ type: "session" }] },
		{ runtimeBin: "omp", mode: "rpc", entries: [{ type: "session_init" }] },
		{ runtimeBin: "pi", mode: "print", entries: [{ type: "session_init" }] },
	] as const)("preserves independently hosted sessions: %j", (context) => {
		expect(isSubagentChild({}, context)).toBe(false);
	});
	it("false for a top-level session (neither env set)", () => {
		expect(isSubagentChild({})).toBe(false);
	});
	it("true when PI_SUBAGENT_CHILD=1", () => {
		expect(isSubagentChild({ PI_SUBAGENT_CHILD: "1" })).toBe(true);
	});
	it("true when PI_SUBAGENT_DEPTH>0", () => {
		expect(isSubagentChild({ PI_SUBAGENT_DEPTH: "1" })).toBe(true);
		expect(isSubagentChild({ PI_SUBAGENT_DEPTH: "3" })).toBe(true);
	});
	it("false at depth 0 / non-numeric", () => {
		expect(isSubagentChild({ PI_SUBAGENT_DEPTH: "0" })).toBe(false);
		expect(isSubagentChild({ PI_SUBAGENT_DEPTH: "nope" })).toBe(false);
	});
});
