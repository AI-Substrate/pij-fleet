import { describe, expect, it } from "vitest";

import {
	ADOPTION_HINT,
	effectiveParent,
	isUnadopted,
	planLink,
	projectSessionForest,
} from "./tree.js";
import type {
	LivenessVerdict,
	SessionDescriptor,
	SessionLifecycle,
	TreeActivity,
	TreeSession,
} from "./types.js";

function desc(id: string, overrides: Partial<SessionDescriptor> = {}): SessionDescriptor {
	return {
		id,
		folder: "/repo",
		dataDir: `/home/.pij/${id}`,
		eventsPath: `/home/.pij/${id}/events.ndjson`,
		pid: 100,
		startedAt: "2026-07-13T00:00:00.000Z",
		...overrides,
	};
}

function entry(
	descriptor: SessionDescriptor,
	activity: TreeActivity = "idle",
	liveness: LivenessVerdict = "active",
): TreeSession {
	return { descriptor, activity, liveness };
}

function project(
	sessions: readonly TreeSession[],
	options: Parameters<typeof projectSessionForest>[1] = {},
) {
	const result = projectSessionForest(sessions, options);
	if (!result.ok) throw new Error(`${result.code}: ${result.message}`);
	return result.value;
}

function flattenIds(nodes: ReturnType<typeof project>["roots"]): string[] {
	const ids: string[] = [];
	const visit = (node: (typeof nodes)[number]): void => {
		ids.push(node.id);
		for (const child of node.children) visit(child);
	};
	for (const node of nodes) visit(node);
	return ids;
}

describe("effectiveParent", () => {
	it("uses explicit parent, explicit root, then legacy spawnedBy fallback", () => {
		expect(effectiveParent(desc("child", { parentId: "explicit", spawnedBy: "owner" }))).toBe(
			"explicit",
		);
		expect(effectiveParent(desc("root", { parentId: null, spawnedBy: "owner" }))).toBeNull();
		expect(effectiveParent(desc("legacy", { spawnedBy: "owner" }))).toBe("owner");
		expect(effectiveParent(desc("unlinked"))).toBeNull();
	});
});

describe("planLink", () => {
	const descriptors = [
		desc("root", { parentId: null }),
		desc("new-parent", { parentId: null }),
		desc("child", { parentId: "root", spawnedBy: "owner" }),
		desc("grandchild", { parentId: "child" }),
		desc("legacy", { spawnedBy: "grandchild" }),
	];

	it("returns a reparented descriptor without mutating ownership or inputs", () => {
		const before = structuredClone(descriptors);
		const result = planLink(descriptors, "child", "new-parent");
		expect(result).toMatchObject({
			ok: true,
			value: { id: "child", parentId: "new-parent", spawnedBy: "owner" },
		});
		expect(descriptors).toEqual(before);
	});

	it("persists null as an explicit root that suppresses spawnedBy fallback", () => {
		const result = planLink(descriptors, "child", null);
		expect(result).toMatchObject({
			ok: true,
			value: { id: "child", parentId: null, spawnedBy: "owner" },
		});
		if (result.ok) expect(effectiveParent(result.value)).toBeNull();
	});

	it.each([
		["unknown child", "missing", "root", "E-NOID"],
		["unknown parent", "child", "missing", "E-NOID"],
		["self parent", "child", "child", "E-SELF"],
	] as const)("rejects %s without mutating inputs", (_name, child, parent, code) => {
		const before = structuredClone(descriptors);
		const result = planLink(descriptors, child, parent);
		expect(result).toMatchObject({ ok: false, code });
		expect(descriptors).toEqual(before);
	});

	it("rejects cycles across mixed explicit and spawnedBy fallback edges", () => {
		const mixed = [
			desc("a", { parentId: null }),
			desc("b", { spawnedBy: "a" }),
			desc("c", { parentId: "b" }),
		];
		const before = structuredClone(mixed);
		const result = planLink(mixed, "a", "c");
		expect(result).toMatchObject({ ok: false, code: "E-ARG" });
		expect(mixed).toEqual(before);
	});
});

describe("projectSessionForest", () => {
	it("orders roots and children deterministically and supports arbitrary subtrees", () => {
		const sessions = [
			entry(desc("z-root", { parentId: null })),
			entry(desc("b-child", { parentId: "z-root" })),
			entry(desc("a-child", { parentId: "z-root" })),
			entry(desc("a-root", { parentId: null })),
			entry(desc("grandchild", { parentId: "a-child" })),
		];
		const forest = project(sessions);
		expect(forest.roots.map((node) => node.id)).toEqual(["a-root", "z-root"]);
		expect(forest.roots[1]?.children.map((node) => node.id)).toEqual(["a-child", "b-child"]);
		const subtree = project(sessions, { rootId: "a-child" });
		expect(flattenIds(subtree.roots)).toEqual(["a-child", "grandchild"]);
	});

	it("returns E-NOID for an unknown subtree root", () => {
		expect(projectSessionForest([entry(desc("root"))], { rootId: "missing" })).toMatchObject({
			ok: false,
			code: "E-NOID",
		});
	});

	it("preserves problem metadata on selected subtree roots", () => {
		const sessions = [
			entry(desc("known-parent", { parentId: null })),
			entry(desc("child", { parentId: "known-parent" })),
			entry(desc("orphan", { parentId: "missing" })),
		];

		expect(project(sessions, { rootId: "orphan" }).roots[0]).toMatchObject({
			id: "orphan",
			effectiveParentId: "missing",
			problem: "orphan",
		});
		expect(project(sessions, { rootId: "child" }).roots[0]).toMatchObject({
			id: "child",
			effectiveParentId: "known-parent",
			problem: "filtered-parent",
		});
	});

	it("marks missing parents as orphan and excluded parents as filtered-parent", () => {
		const sessions = [
			entry(desc("known-parent", { parentId: null })),
			entry(desc("filtered-child", { parentId: "known-parent" }), "working"),
			entry(desc("orphan", { parentId: "missing" })),
		];
		const filtered = project(sessions, { filters: { activity: ["working", "idle"] } });
		const roots = Object.fromEntries(filtered.roots.map((node) => [node.id, node]));
		expect(roots.orphan?.problem).toBe("orphan");
		expect(roots["filtered-child"]?.problem).toBeUndefined();

		const parentFiltered = project(sessions, { filters: { activity: ["working"] } });
		expect(parentFiltered.roots).toHaveLength(1);
		expect(parentFiltered.roots[0]).toMatchObject({
			id: "filtered-child",
			effectiveParentId: "known-parent",
			problem: "filtered-parent",
		});
	});

	it("composes repository-style preselection with filters", () => {
		const sessions = [
			entry(desc("repo-parent", { parentId: null }), "idle"),
			entry(desc("repo-child", { parentId: "repo-parent" }), "working"),
			entry(desc("other-repo", { parentId: null }), "working"),
		];
		const forest = project(sessions, {
			selectedIds: ["repo-parent", "repo-child"],
			filters: { activity: ["working"] },
		});
		expect(forest.roots).toHaveLength(1);
		expect(forest.roots[0]).toMatchObject({
			id: "repo-child",
			problem: "filtered-parent",
		});
	});

	it("composes repeated values OR within axes and AND across axes", () => {
		const sessions = [
			entry(desc("working-bound", { lifecycle: "bound" }), "working", "active"),
			entry(desc("done-bound", { lifecycle: "bound" }), "done", "active"),
			entry(desc("idle-pending", { lifecycle: "pending" }), "idle", "active"),
			entry(desc("working-failed", { lifecycle: "failed" }), "working", "dead"),
		];
		const forest = project(sessions, {
			filters: {
				activity: ["working", "done"],
				liveness: ["active"],
				lifecycle: ["bound", "failed"],
			},
		});
		expect(forest.roots.map((node) => node.id)).toEqual(["done-bound", "working-bound"]);
	});

	it("hides dead/dissolved history by default; --all exposes the dead but never the buried", () => {
		const sessions = [
			entry(desc("active")),
			entry(desc("dead"), "done", "dead"),
			entry(desc("closed", { lifecycle: "dissolved" }), "done", "dissolved"),
		];
		expect(project(sessions).roots.map((node) => node.id)).toEqual(["active"]);
		// `--all` means "include the DEAD". A dead seat is membership evidence; a
		// dissolved one is a closed seat still on disk for its 48h post-mortem
		// window, and emitting it renders a card with no `list` row behind it.
		expect(project(sessions, { filters: { all: true } }).roots.map((node) => node.id)).toEqual([
			"active",
			"dead",
		]);
		// Escape hatch: burial is hidden from `--all`, never made unreachable.
		expect(
			project(sessions, { filters: { lifecycle: ["dissolved" as SessionLifecycle] } }).roots.map(
				(node) => node.id,
			),
		).toEqual(["closed"]);
		// …and still reachable alongside `--all`, which must not veto an explicit axis.
		expect(
			project(sessions, {
				filters: { all: true, lifecycle: ["dissolved" as SessionLifecycle] },
			}).roots.map((node) => node.id),
		).toEqual(["closed"]);
		expect(
			project(sessions, { filters: { liveness: ["dead"] } }).roots.map((node) => node.id),
		).toEqual(["dead"]);
	});

	it("keeps raw descriptor fields and effective parent metadata in stable JSON nodes", () => {
		const legacy = desc("legacy", {
			spawnedBy: "owner",
			gitCommonDir: "/repo/.git",
			lifecycle: "bound",
			prime: true,
		});
		const node = project([entry(desc("owner")), entry(legacy)]).roots[0]?.children[0];
		expect(node).toMatchObject({
			id: "legacy",
			spawnedBy: "owner",
			gitCommonDir: "/repo/.git",
			lifecycle: "bound",
			prime: true,
			effectiveParentId: "owner",
			activity: "idle",
			liveness: "active",
			children: [],
		});
		expect(node).not.toHaveProperty("parentId");
	});

	it("terminates corrupt legacy cycles with explicit cycle markers", () => {
		const sessions = [
			entry(desc("a", { spawnedBy: "b" })),
			entry(desc("b", { parentId: "a" })),
			entry(desc("child", { parentId: "a" })),
		];
		const forest = project(sessions);
		const json = JSON.stringify(forest);
		expect(json.length).toBeLessThan(5000);
		expect(json).toContain('"problem":"cycle"');
		expect(flattenIds(forest.roots)).toContain("child");
	});

	it("projects an 8,000-node corrupt cycle without relying on the call stack", () => {
		const count = 8_000;
		const id = (index: number): string => `node-${index.toString().padStart(4, "0")}`;
		const sessions = Array.from({ length: count }, (_, index) =>
			entry(desc(id(index), { parentId: id((index + 1) % count) })),
		);

		const forest = project(sessions);
		const pending = [...forest.roots];
		let projectedCount = 0;
		let cycleCount = 0;
		while (pending.length > 0) {
			const node = pending.pop();
			if (!node) continue;
			projectedCount += 1;
			if (node.problem === "cycle") cycleCount += 1;
			pending.push(...node.children);
		}

		expect(projectedCount).toBeLessThanOrEqual(count + 1);
		expect(cycleCount).toBeGreaterThan(0);
	});
});

// ─── P3 (plan 054 — AC-08): prime is a LEGAL root ────────────────────────────
describe("prime parentless is a legal root (AC-08)", () => {
	it("projects a parentless prime as a root with no problem", () => {
		const forest = project([
			entry(desc("pij-prime", { prime: true })),
			entry(desc("pij-child", { spawnedBy: "pij-prime" })),
		]);
		const root = forest.roots[0];
		expect(root?.id).toBe("pij-prime");
		expect(root?.problem).toBeUndefined();
		expect(root?.effectiveParentId).toBeNull();
		expect(root?.children.map((c) => c.id)).toEqual(["pij-child"]);
	});
});

// ─── P3 (plan 054 — AC-08/WS-1): the ADOPTION axis, carp's split ─────────────
// unadopted ≠ problem ≠ runtime state: three independently assertable axes.
// unadopted = non-prime with NO effective parent (nothing to escalate to);
// an orphan (parent named but MISSING) is a structural problem, not an
// adoption gap; runtime state lives on neither.
describe("unadopted adoption-axis projection (AC-08 / WS-1)", () => {
	it("flags a parentless non-prime root as unadopted", () => {
		const forest = project([entry(desc("pij-loner"))]);
		expect(forest.roots[0]?.unadopted).toBe(true);
		expect(forest.roots[0]?.problem).toBeUndefined();
	});

	it("flags an explicit root override (parentId null) on a non-prime", () => {
		const forest = project([entry(desc("pij-rooted", { parentId: null, spawnedBy: "gone" }))]);
		expect(forest.roots[0]?.unadopted).toBe(true);
	});

	it("never flags prime — parentless prime is the legal root", () => {
		const forest = project([entry(desc("pij-prime", { prime: true }))]);
		expect(forest.roots[0]?.unadopted).toBeUndefined();
	});

	it("still flags a designated PM when prime is not true (Phase 2 T008 non-change)", () => {
		const pm = desc("pij-pm", { orchestrationRole: "pm" });
		expect(isUnadopted(pm)).toBe(true);
		expect(project([entry(pm)]).roots[0]?.unadopted).toBe(true);
	});

	it("never flags a parented child, and an orphan is a problem — NOT unadopted", () => {
		const forest = project([
			entry(desc("pij-parent")),
			entry(desc("pij-child", { spawnedBy: "pij-parent" })),
			entry(desc("pij-orphan", { parentId: "pij-vanished" })),
		]);
		const byId = new Map(forest.roots.map((r) => [r.id, r]));
		const child = byId.get("pij-parent")?.children[0];
		expect(child?.unadopted).toBeUndefined();
		// The orphan HAS a parent pointer (adoption axis satisfied); the
		// registry gap is a structural problem on a different axis.
		expect(byId.get("pij-orphan")?.problem).toBe("orphan");
		expect(byId.get("pij-orphan")?.unadopted).toBeUndefined();
		// And the runtime axis is untouched by either flag: the descriptor's
		// systemState flows through the node verbatim.
		expect(byId.get("pij-orphan")?.systemState).toBeUndefined();
	});
});

// ─── P3 T005: adoption guidance surface (content contract for the P4 route) ──
describe("ADOPTION_HINT (P3 T005 — skill-facing content, route lands P4 4.3)", () => {
	it("names the flag, the enumeration recipe, and the remedy", () => {
		expect(ADOPTION_HINT).toContain("unadopted");
		expect(ADOPTION_HINT).toContain("pij tree --global --json");
		expect(ADOPTION_HINT).toContain("pij link");
		expect(ADOPTION_HINT).toContain("--parent");
		// Spawn-time prevention beats post-hoc adoption: the hint teaches the
		// caller-truth rule, not just the repair verb.
		expect(ADOPTION_HINT).toContain("PIJ_SESSION_ID");
		// Prime is the legal root — the hint must not nag prime seats.
		expect(ADOPTION_HINT.toLowerCase()).toContain("prime");
	});
});
