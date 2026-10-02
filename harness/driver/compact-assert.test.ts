// harness/driver/compact-assert.test.ts
//
// Unit tests for compactAndAssert + its pure helpers. Coverage:
//   (a) field-equality success → ok: true, divergences: []
//   (b) field-divergence post-compact → divergences[] populated with phase: "compact"
//   (c) opt-in reload check → postReload set only when includeReloadCheck: true (default)
//                              and absent when false
//   (d) compactTimeoutMs / reloadTimeoutMs honoured (waitIdle invoked with passed value)
//   (e) F004 mitigation: stale JSON in scrollback does NOT cause a false pass —
//                         the helper throws when no FRESH JSON envelope is emitted.
//
// Pure helpers (extractLatestJsonObject, extractFreshJsonObject, compareFields)
// cover most of the surface area without a tmux mock. End-to-end tests use a
// scripted tmux mock where each scripted pane is a strict EXTENSION of the
// prior one (simulating real tmux scrollback semantics: append-only after
// rendering settles).

import { beforeEach, describe, expect, it, vi } from "vitest";

const { calls, paneScript, paneCursor } = vi.hoisted(() => ({
	calls: [] as string[][],
	paneScript: [] as string[],
	paneCursor: { value: 0 },
}));

vi.mock("node:child_process", () => ({
	execFileSync: vi.fn((file: string, args: string[]) => {
		calls.push([file, ...args]);
		if (args[0] === "display-message") return "%5\n";
		if (args[0] === "list-panes") return "%5\t12345\tpi\t0\t200\t50\n";
		if (args[0] === "capture-pane") {
			return paneScript[paneCursor.value] ?? "";
		}
		// send-keys with Enter advances to the next scripted pane (each
		// session.execute({press:"Enter"}) sends one Enter after the text).
		if (args[0] === "send-keys" && args.includes("Enter")) {
			paneCursor.value = Math.min(paneCursor.value + 1, paneScript.length - 1);
		}
		return "";
	}),
}));

beforeEach(() => {
	calls.length = 0;
	paneScript.length = 0;
	paneCursor.value = 0;
});

const FOOTER = "claude-sonnet-4.5 • medium · 12.3%/100%";

/** Footer + prompt suffix that satisfies waitIdle's defaults. */
function withChrome(body: string): string {
	return `${body}\n${FOOTER}\n> `;
}

/**
 * Build a chain of pane snapshots where each one is a strict EXTENSION of
 * the previous. Mirrors tmux append-only scrollback. `bodies[i]` is the new
 * body emitted at step i.
 */
function appendChain(bodies: string[]): string[] {
	const out: string[] = [];
	let accumulated = "";
	for (const body of bodies) {
		accumulated = accumulated ? `${accumulated}\n${body}` : body;
		out.push(withChrome(accumulated));
	}
	return out;
}

describe("driver/compactAndAssert: pure helpers", () => {
	it("extractLatestJsonObject returns the most-recent JSON envelope from a pane", async () => {
		const { extractLatestJsonObject } = await import("./index.js");
		const sample = [
			"chrome before",
			'{"iterations":1,"lastTaskTitle":"old"}',
			"more chrome",
			'{"iterations":2,"lastTaskTitle":"latest","runActive":true}',
			"trailing > prompt",
		].join("\n");
		expect(extractLatestJsonObject(sample)).toEqual({
			iterations: 2,
			lastTaskTitle: "latest",
			runActive: true,
		});
	});

	it("extractLatestJsonObject returns {} when no valid JSON is present", async () => {
		const { extractLatestJsonObject } = await import("./index.js");
		expect(extractLatestJsonObject("nothing here\n> ")).toEqual({});
	});

	it("extractLatestJsonObject ignores braces inside string literals", async () => {
		const { extractLatestJsonObject } = await import("./index.js");
		const sample = '{"note":"contains } literal","iterations":5}';
		expect(extractLatestJsonObject(sample)).toEqual({
			note: "contains } literal",
			iterations: 5,
		});
	});

	it("findJsonBlocks reports positions for every balanced block", async () => {
		const { findJsonBlocks } = await import("./index.js");
		const sample = 'pre {"a":1} mid {"b":2} tail';
		const blocks = findJsonBlocks(sample);
		expect(blocks.map((b) => b.text)).toEqual(['{"a":1}', '{"b":2}']);
		expect(blocks[0]?.start).toBe(4);
		expect(blocks[1]?.start).toBe(16);
	});

	it("extractFreshJsonObject returns the LATEST JSON past the common prefix", async () => {
		const { extractFreshJsonObject } = await import("./index.js");
		const before = 'header {"old":1}';
		const after = `${before} divider {"fresh":2}`;
		expect(extractFreshJsonObject(before, after)).toEqual({ fresh: 2 });
	});

	it("extractFreshJsonObject returns null when no JSON appears past baseline (F004)", async () => {
		const { extractFreshJsonObject } = await import("./index.js");
		const before = 'header {"only":1}';
		const after = `${before} no new json here`;
		expect(extractFreshJsonObject(before, after)).toBeNull();
	});

	it("extractFreshJsonObject treats identical-content same-position JSON as stale", async () => {
		const { extractFreshJsonObject } = await import("./index.js");
		const before = '{"iters":3}';
		const after = before; // pane never advanced
		expect(extractFreshJsonObject(before, after)).toBeNull();
	});

	it("compareFields returns [] when fields match (case a)", async () => {
		const { compareFields } = await import("./index.js");
		const result = compareFields(
			{ iterations: 3, lastTaskTitle: "x" },
			{ iterations: 3, lastTaskTitle: "x" },
			["iterations", "lastTaskTitle"],
			"compact",
		);
		expect(result).toEqual([]);
	});

	it("compareFields reports divergence with phase (case b)", async () => {
		const { compareFields } = await import("./index.js");
		const result = compareFields(
			{ iterations: 3, lastTaskTitle: "x" },
			{ iterations: 0, lastTaskTitle: "x" },
			["iterations", "lastTaskTitle"],
			"compact",
		);
		expect(result).toEqual([{ field: "iterations", pre: 3, post: 0, phase: "compact" }]);
	});

	it("compareFields treats missing field on one side as divergent", async () => {
		const { compareFields } = await import("./index.js");
		const result = compareFields({ iterations: 3 }, {}, ["iterations"], "reload");
		expect(result).toEqual([{ field: "iterations", pre: 3, post: undefined, phase: "reload" }]);
	});

	it("compareFields recurses into arrays + nested objects via deep equality", async () => {
		const { compareFields } = await import("./index.js");
		expect(
			compareFields(
				{ history: [{ id: 1 }, { id: 2 }] },
				{ history: [{ id: 1 }, { id: 2 }] },
				["history"],
				"compact",
			),
		).toEqual([]);
		expect(
			compareFields(
				{ history: [{ id: 1 }, { id: 2 }] },
				{ history: [{ id: 1 }] },
				["history"],
				"compact",
			),
		).toEqual([
			{
				field: "history",
				pre: [{ id: 1 }, { id: 2 }],
				post: [{ id: 1 }],
				phase: "compact",
			},
		]);
	});
});

describe("driver/compactAndAssert: end-to-end (scripted tmux mock with append-only scrollback)", () => {
	it("case (a): pre + postCompact + postReload all match → ok: true, no divergences", async () => {
		// Each step appends a new body to the prior pane. mimics tmux scrollback.
		paneScript.push(
			...appendChain([
				"ralph-loop loaded",
				// step 1: pre status emits JSON
				'>>> /ralph status --json\n{"iterations":3,"lastTaskTitle":"impl X","runActive":true}',
				// step 2: /compact echoes + idle
				">>> /compact\ncompact complete",
				// step 3: post-compact status emits FRESH JSON (same values, but freshly written)
				'>>> /ralph status --json\n{"iterations":3,"lastTaskTitle":"impl X","runActive":true}',
				// step 4: /reload echoes + idle
				">>> /reload\nreload complete",
				// step 5: post-reload status emits another FRESH JSON
				'>>> /ralph status --json\n{"iterations":3,"lastTaskTitle":"impl X","runActive":true}',
			]),
		);

		const { Session, compactAndAssert } = await import("./index.js");
		const session = Session.fromTarget({ session: "test", paneId: "%5" });
		await session.waitIdle({ timeoutMs: 1_000 });

		const r = await compactAndAssert(session, {
			statusCommand: "/ralph status --json",
			fields: ["iterations", "lastTaskTitle"],
			compactTimeoutMs: 1_000,
			reloadTimeoutMs: 1_000,
			statusTimeoutMs: 1_000,
		});

		expect(r.ok).toBe(true);
		expect(r.divergences).toEqual([]);
		expect(r.pre).toMatchObject({ iterations: 3, lastTaskTitle: "impl X" });
		expect(r.postCompact).toMatchObject({ iterations: 3, lastTaskTitle: "impl X" });
		expect(r.postReload).toMatchObject({ iterations: 3, lastTaskTitle: "impl X" });
	});

	it("case (b): postCompact diverges (D-005 confirmed shape) → ok: false, phase=compact", async () => {
		paneScript.push(
			...appendChain([
				"ralph-loop loaded",
				'>>> /ralph status --json\n{"iterations":5,"lastTaskTitle":"impl X","runActive":true}',
				">>> /compact\ncompact complete",
				// Iterations dropped to 0 — D-005 confirmed shape.
				'>>> /ralph status --json\n{"iterations":0,"lastTaskTitle":null,"runActive":false}',
				">>> /reload\nreload complete",
				'>>> /ralph status --json\n{"iterations":0,"lastTaskTitle":null,"runActive":false}',
			]),
		);

		const { Session, compactAndAssert } = await import("./index.js");
		const session = Session.fromTarget({ session: "test", paneId: "%5" });
		await session.waitIdle({ timeoutMs: 1_000 });

		const r = await compactAndAssert(session, {
			statusCommand: "/ralph status --json",
			fields: ["iterations", "lastTaskTitle"],
			compactTimeoutMs: 1_000,
			reloadTimeoutMs: 1_000,
			statusTimeoutMs: 1_000,
		});

		expect(r.ok).toBe(false);
		const compactDivs = r.divergences.filter((d) => d.phase === "compact");
		expect(compactDivs).toContainEqual({
			field: "iterations",
			pre: 5,
			post: 0,
			phase: "compact",
		});
	});

	it("case (c): includeReloadCheck=false omits postReload entirely", async () => {
		paneScript.push(
			...appendChain([
				"ralph-loop loaded",
				'>>> /ralph status --json\n{"iterations":3}',
				">>> /compact\ncompact complete",
				'>>> /ralph status --json\n{"iterations":3}',
			]),
		);

		const { Session, compactAndAssert } = await import("./index.js");
		const session = Session.fromTarget({ session: "test", paneId: "%5" });
		await session.waitIdle({ timeoutMs: 1_000 });

		const r = await compactAndAssert(session, {
			statusCommand: "/ralph status --json",
			fields: ["iterations"],
			compactTimeoutMs: 1_000,
			statusTimeoutMs: 1_000,
			includeReloadCheck: false,
		});

		expect(r.ok).toBe(true);
		expect(r.postReload).toBeUndefined();
		expect(r.divergences).toEqual([]);
		// And we should NOT have sent /reload over tmux.
		const sentReload = calls.some(
			(c) => c[0] === "tmux" && c[1] === "send-keys" && c.includes("/reload"),
		);
		expect(sentReload).toBe(false);
	});

	it("case (d): timeouts plumb through (small compact/reload/status timeouts accepted on idle)", async () => {
		paneScript.push(
			...appendChain([
				"ralph-loop loaded",
				'>>> /ralph status --json\n{"iterations":1}',
				">>> /compact\ncompact complete",
				'>>> /ralph status --json\n{"iterations":1}',
				">>> /reload\nreload complete",
				'>>> /ralph status --json\n{"iterations":1}',
			]),
		);

		const { Session, compactAndAssert } = await import("./index.js");
		const session = Session.fromTarget({ session: "test", paneId: "%5" });
		await session.waitIdle({ timeoutMs: 1_000 });

		// Custom (small) timeouts accepted on an idle pane.
		const r = await compactAndAssert(session, {
			statusCommand: "/ralph status --json",
			fields: ["iterations"],
			compactTimeoutMs: 800,
			reloadTimeoutMs: 800,
			statusTimeoutMs: 800,
		});

		expect(r.ok).toBe(true);
	});

	it("case (e) — F004 regression: stale pre JSON in scrollback does NOT pass when post emits no fresh JSON", async () => {
		// Pre status emits {iterations:5}; /compact runs; but the post-status
		// command produces NO new JSON (simulating ralph-loop's status failing
		// to render after compact). The old impl would have matched the stale
		// {iterations:5} in scrollback and falsely returned ok:true. The fixed
		// impl throws because no fresh JSON appears past the baseline.
		paneScript.push(
			...appendChain([
				"ralph-loop loaded",
				'>>> /ralph status --json\n{"iterations":5,"runActive":true}',
				">>> /compact\ncompact complete",
				// Status command echoed but NO new JSON envelope rendered.
				">>> /ralph status --json\n(error: ralph-loop store hydration timed out)",
			]),
		);

		const { Session, compactAndAssert } = await import("./index.js");
		const session = Session.fromTarget({ session: "test", paneId: "%5" });
		await session.waitIdle({ timeoutMs: 1_000 });

		await expect(
			compactAndAssert(session, {
				statusCommand: "/ralph status --json",
				fields: ["iterations"],
				compactTimeoutMs: 800,
				reloadTimeoutMs: 800,
				statusTimeoutMs: 800,
				includeReloadCheck: false,
			}),
		).rejects.toThrow(/did not emit a fresh JSON envelope/);
	});

	it("case (e2) — F004 regression: malformed post JSON also throws (no silent {} pass)", async () => {
		paneScript.push(
			...appendChain([
				"ralph-loop loaded",
				'>>> /ralph status --json\n{"iterations":5}',
				">>> /compact\ncompact complete",
				// Malformed JSON in fresh region.
				'>>> /ralph status --json\n{"iterations": malformed,',
			]),
		);

		const { Session, compactAndAssert } = await import("./index.js");
		const session = Session.fromTarget({ session: "test", paneId: "%5" });
		await session.waitIdle({ timeoutMs: 1_000 });

		await expect(
			compactAndAssert(session, {
				statusCommand: "/ralph status --json",
				fields: ["iterations"],
				compactTimeoutMs: 800,
				reloadTimeoutMs: 800,
				statusTimeoutMs: 800,
				includeReloadCheck: false,
			}),
		).rejects.toThrow(/did not emit a fresh JSON envelope/);
	});
});
