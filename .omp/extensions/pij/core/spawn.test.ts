// pij-messaging — spawn builder + ready-body codec tests (AC-02 / AC-09).
//
// Covers: model ±, task ± (incl. special chars / quotes / newlines),
// paneId ±, required env vars, argv array shape, ready-body round-trip.

import { describe, expect, it } from "vitest";
import { FakeRegistry } from "../adapters/fakes.js";
import { type ModelEntry, parseModelsJson } from "./models/registry.js";
import {
	aliasAgentSpawnArgs,
	buildControlSpawnCommand,
	buildEffortWarning,
	buildPendingDescriptor,
	buildPiFocusSpawnCommand,
	buildPlanIdWarning,
	buildSpawnCommand,
	buildSpawnOutput,
	deriveCallerParent,
	livePeerPanes,
	markCompactingSelf,
	parseAdoptArgs,
	parseCompactSelfArgs,
	parseReadyBody,
	parseSpawnArgs,
	planBranch,
	planControlSplit,
	planPlacement,
	readyBody,
	renderSpawnReceipt,
	resolvePiModelBinding,
	spawnIdentitySeed,
} from "./spawn.js";
import type { HarnessKind, SessionDescriptor } from "./types.js";

// ─── buildSpawnCommand ───────────────────────────────────────────────────────

describe("buildSpawnCommand", () => {
	const base = {
		harness: "pi" as const,
		spawnId: "abc123",
		announceTo: "pij-parent01",
		cwd: "/home/user/project",
		role: "worker" as const,
	};

	it("launches the explicitly selected pi executable", () => {
		const result = buildSpawnCommand(base);
		expect(result.cmd).toBe("pi");
	});

	it("args is empty when no model given", () => {
		const result = buildSpawnCommand(base);
		expect(result.args).toEqual([]);
	});

	it("emits --model arg when model is provided", () => {
		const result = buildSpawnCommand({ ...base, model: "anthropic/claude-3-5-sonnet" });
		expect(result.args).toEqual(["--model", "anthropic/claude-3-5-sonnet"]);
	});

	it("model with slash and dots produces correct discrete args (AC-09: no shell string)", () => {
		const result = buildSpawnCommand({ ...base, model: "openai/gpt-4o-mini" });
		// Two discrete array elements — not one joined string
		expect(result.args).toHaveLength(2);
		expect(result.args[0]).toBe("--model");
		expect(result.args[1]).toBe("openai/gpt-4o-mini");
	});

	it("args is an array of strings (no shell string — AC-09)", () => {
		const result = buildSpawnCommand({ ...base, model: "some/model" });
		expect(Array.isArray(result.args)).toBe(true);
		for (const arg of result.args) {
			expect(typeof arg).toBe("string");
		}
	});

	describe("explicit harness selection", () => {
		it("omp launches its own executable and translates effort without a branch flag", () => {
			const result = buildSpawnCommand({
				...base,
				harness: "omp",
				model: "github-copilot/gpt-6-astra",
				effort: "high",
			});
			expect(result.cmd).toBe("omp");
			expect(result.args).toEqual([
				"--auto-approve",
				"--model",
				"github-copilot/gpt-6-astra",
				"--thinking",
				"high",
			]);
			expect(result.env.PIJ_SPAWN_MODEL).toBe("github-copilot/gpt-6-astra");
			expect(result.env.PIJ_SPAWN_EFFORT).toBe("high");
		});

		it("an executable path does not change the selected harness flag translation", () => {
			const result = buildSpawnCommand({
				...base,
				harness: "omp",
				bin: "/opt/omp",
				effort: "high",
			});
			expect(result.cmd).toBe("/opt/omp");
			expect(result.args).toEqual(["--auto-approve", "--thinking", "high"]);
		});
	});

	it("exposes the spawner id as PIJ_PARENT_ID (who spawned me)", () => {
		const result = buildSpawnCommand(base);
		expect(result.env.PIJ_PARENT_ID).toBe("pij-parent01");
	});

	it("exports an explicit plan id under both harness join names", () => {
		const result = buildSpawnCommand({ ...base, planId: "073-pij-first-class-ui" });
		expect(result.env).toMatchObject({
			HARNESS_PLAN_ID: "073-pij-first-class-ui",
			PIJ_PLAN_ID: "073-pij-first-class-ui",
		});
	});

	it("omits PIJ_SPAWN_TASK when no task given", () => {
		const result = buildSpawnCommand(base);
		expect(result.env).not.toHaveProperty("PIJ_SPAWN_TASK");
	});

	it("sets PIJ_SPAWN_TASK when task given", () => {
		const result = buildSpawnCommand({ ...base, task: "do something useful" });
		expect(result.env.PIJ_SPAWN_TASK).toBe("do something useful");
	});

	it("task with double quotes survives env round-trip (no shell interpretation)", () => {
		const task = 'Run: echo "hello world"';
		const result = buildSpawnCommand({ ...base, task });
		expect(result.env.PIJ_SPAWN_TASK).toBe(task);
	});

	it("task with single quotes survives env round-trip", () => {
		const task = "it's a task with 'single quotes'";
		const result = buildSpawnCommand({ ...base, task });
		expect(result.env.PIJ_SPAWN_TASK).toBe(task);
	});

	it("task with newlines and tabs survives env round-trip", () => {
		const task = "line one\nnewline here\ttab here";
		const result = buildSpawnCommand({ ...base, task });
		expect(result.env.PIJ_SPAWN_TASK).toBe(task);
	});

	it("task with shell metacharacters survives env round-trip", () => {
		const task = "do $(whoami) && rm -rf /; echo `id`";
		const result = buildSpawnCommand({ ...base, task });
		expect(result.env.PIJ_SPAWN_TASK).toBe(task);
	});

	it("omits PIJ_PANE_ID when paneId not given", () => {
		const result = buildSpawnCommand(base);
		expect(result.env).not.toHaveProperty("PIJ_PANE_ID");
	});

	it("sets PIJ_PANE_ID when paneId is provided (optional pass-through)", () => {
		const result = buildSpawnCommand({ ...base, paneId: "%42" });
		expect(result.env.PIJ_PANE_ID).toBe("%42");
	});

	it("omits PIJ_SPAWN_MODEL when model is not given (§H2)", () => {
		const result = buildSpawnCommand(base);
		expect(result.env).not.toHaveProperty("PIJ_SPAWN_MODEL");
	});

	it("sets PIJ_SPAWN_MODEL in env when model is given (§H2)", () => {
		const result = buildSpawnCommand({ ...base, model: "anthropic/claude-3-5-sonnet" });
		expect(result.env.PIJ_SPAWN_MODEL).toBe("anthropic/claude-3-5-sonnet");
	});

	it("PIJ_SPAWN_MODEL matches the --model argv value (both emitted together)", () => {
		const model = "openai/gpt-4o";
		const result = buildSpawnCommand({ ...base, model });
		expect(result.args).toContain(model); // --model argv
		expect(result.env.PIJ_SPAWN_MODEL).toBe(model); // env var
	});

	it("threads the resolved provider through PIJ_SPAWN_PROVIDER", () => {
		const result = buildSpawnCommand({
			...base,
			model: "github-copilot/gpt-5.6-sol",
			provider: "github-copilot",
		});
		expect(result.env.PIJ_SPAWN_PROVIDER).toBe("github-copilot");
	});

	it("role 'parent' is passed through to PIJ_ROLE", () => {
		const result = buildSpawnCommand({ ...base, role: "parent" });
		expect(result.env.PIJ_ROLE).toBe("parent");
	});

	// ── --effort translation for pi: `:<level>` suffix on the model id (#3) ──────
	it("--effort with a model rides as a :<level> suffix on the pi model id", () => {
		const r = buildSpawnCommand({ ...base, model: "github-copilot/gpt-5.5", effort: "xhigh" });
		expect(r.args).toEqual(["--model", "github-copilot/gpt-5.5:xhigh"]);
		expect(r.env.PIJ_SPAWN_MODEL).toBe("github-copilot/gpt-5.5:xhigh");
		expect(r.env.PIJ_SPAWN_EFFORT).toBe("xhigh");
	});

	it("keeps the Pi provider prefix unchanged when suffixing GPT-5.6 effort", () => {
		const r = buildSpawnCommand({
			...base,
			model: "github-copilot/gpt-5.6-sol",
			effort: "max",
		});
		expect(r.args).toEqual(["--model", "github-copilot/gpt-5.6-sol:max"]);
		expect(r.env.PIJ_SPAWN_MODEL).toBe("github-copilot/gpt-5.6-sol:max");
		expect(r.env.PIJ_SPAWN_EFFORT).toBe("max");
	});

	it("--effort without a model is a no-op for pi (nothing to suffix)", () => {
		const r = buildSpawnCommand({ ...base, effort: "high" });
		expect(r.args).toEqual([]);
		expect(r.env).not.toHaveProperty("PIJ_SPAWN_MODEL");
		expect(r.env.PIJ_SPAWN_EFFORT).toBe("high");
	});

	it("no --effort → model rides unchanged (default-to-agent)", () => {
		const r = buildSpawnCommand({ ...base, model: "fugu" });
		expect(r.args).toEqual(["--model", "fugu"]);
		expect(r.env.PIJ_SPAWN_MODEL).toBe("fugu");
		expect(r.env).not.toHaveProperty("PIJ_SPAWN_EFFORT");
	});
});

// ─── readyBody + parseReadyBody ──────────────────────────────────────────────

describe("readyBody + parseReadyBody round-trip", () => {
	it("round-trips a basic payload", () => {
		const body = readyBody("spawn-1", "gpt-4", "/tmp/project", "xhigh");
		const parsed = parseReadyBody(body);
		expect(parsed).toEqual({
			spawnId: "spawn-1",
			model: "gpt-4",
			effort: "xhigh",
			cwd: "/tmp/project",
		});
	});

	it("round-trips payload with slashes/dots in model name", () => {
		const body = readyBody("sid", "openai/gpt-4o-mini", "/path/with spaces/project");
		const parsed = parseReadyBody(body);
		expect(parsed).toEqual({
			spawnId: "sid",
			model: "openai/gpt-4o-mini",
			cwd: "/path/with spaces/project",
		});
	});

	it("keeps effort additive when parsing a legacy ready payload", () => {
		expect(
			parseReadyBody(JSON.stringify({ spawnId: "legacy", model: "gpt-4", cwd: "/repo" })),
		).toEqual({
			spawnId: "legacy",
			model: "gpt-4",
			cwd: "/repo",
		});
	});

	it("round-trips payload with unicode in cwd", () => {
		const body = readyBody("u1", "m", "/Users/café/proj");
		const parsed = parseReadyBody(body);
		expect(parsed?.cwd).toBe("/Users/café/proj");
	});

	it("parseReadyBody returns null for invalid JSON", () => {
		expect(parseReadyBody("not json")).toBeNull();
	});

	it("parseReadyBody returns null for empty string", () => {
		expect(parseReadyBody("")).toBeNull();
	});

	it("parseReadyBody returns null when spawnId missing", () => {
		expect(parseReadyBody(JSON.stringify({ model: "m", cwd: "/c" }))).toBeNull();
	});

	it("parseReadyBody returns null when model missing", () => {
		expect(parseReadyBody(JSON.stringify({ spawnId: "x", cwd: "/c" }))).toBeNull();
	});

	it("parseReadyBody returns null when cwd missing", () => {
		expect(parseReadyBody(JSON.stringify({ spawnId: "x", model: "m" }))).toBeNull();
	});

	it("parseReadyBody returns null when spawnId is not a string", () => {
		expect(parseReadyBody(JSON.stringify({ spawnId: 42, model: "m", cwd: "/c" }))).toBeNull();
	});

	it("parseReadyBody returns null for non-object JSON", () => {
		expect(parseReadyBody(JSON.stringify([1, 2, 3]))).toBeNull();
	});

	it("parseReadyBody returns null for null JSON", () => {
		expect(parseReadyBody("null")).toBeNull();
	});
});

// ─── Control plane (Plan 019) ────────────────────────────────────────────────

describe("spawnIdentitySeed", () => {
	it("is known before launch and deterministic for the same token+pid", () => {
		const a = spawnIdentitySeed("s1700000000000-0", 4242);
		const b = spawnIdentitySeed("s1700000000000-0", 4242);
		expect(a).toBe(b);
		expect(a).toContain("s1700000000000-0");
	});

	it("distinct spawn tokens yield distinct allocation seeds", () => {
		expect(spawnIdentitySeed("s1-0", 4242)).not.toBe(spawnIdentitySeed("s1-1", 4242));
	});
});

describe("buildControlSpawnCommand", () => {
	const base = { harness: "claude" as const, pijId: "pij-bright-otter", cwd: "/repo" };

	it("claude: cmd is 'claude', --dangerously-skip-permissions always (driven pane, no human to approve)", () => {
		const r = buildControlSpawnCommand(base);
		expect(r.cmd).toBe("claude");
		expect(r.args).toEqual(["--dangerously-skip-permissions"]);
	});

	// s071 D4 — the deterministic-bind pin for a PLAIN (non-branched) claude.
	it("claude: a bare forkSessionId pins the session id WITHOUT --resume/--fork-session", () => {
		const r = buildControlSpawnCommand({
			...base,
			forkSessionId: "11111111-2222-3333-4444-555555555555",
		});
		expect(r.args).toEqual([
			"--dangerously-skip-permissions",
			"--session-id",
			"11111111-2222-3333-4444-555555555555",
		]);
		expect(r.args).not.toContain("--fork-session");
		expect(r.args).not.toContain("--resume");
	});

	// CONTROL: with a branchFrom present the SAME field still means "fork", so the
	// new branch above did not cannibalise branch-from-self.
	it("control — forkSessionId WITH branchFrom still emits the full fork form", () => {
		const r = buildControlSpawnCommand({
			...base,
			branchFrom: "src-session",
			forkSessionId: "11111111-2222-3333-4444-555555555555",
		});
		expect(r.args).toEqual([
			"--dangerously-skip-permissions",
			"--resume",
			"src-session",
			"--fork-session",
			"--session-id",
			"11111111-2222-3333-4444-555555555555",
		]);
	});

	// CONTROL: no id supplied ⇒ no flag invented.
	it("control — without a forkSessionId claude gets no --session-id at all", () => {
		expect(buildControlSpawnCommand(base).args).not.toContain("--session-id");
	});

	it("emits --model after --skip-permissions as discrete argv (AC-09: no shell string)", () => {
		const r = buildControlSpawnCommand({ ...base, model: "claude-sonnet-4-6" });
		expect(r.args).toEqual(["--dangerously-skip-permissions", "--model", "claude-sonnet-4-6"]);
	});

	it("threads the pre-allocated id via PIJ_SESSION_ID + harness via PIJ_HARNESS", () => {
		const r = buildControlSpawnCommand(base);
		expect(r.env.PIJ_SESSION_ID).toBe("pij-bright-otter");
		expect(r.env.PIJ_HARNESS).toBe("claude");
	});

	it("exports an explicit plan id under both harness join names", () => {
		const r = buildControlSpawnCommand({ ...base, planId: "073-pij-first-class-ui" });
		expect(r.env).toMatchObject({
			HARNESS_PLAN_ID: "073-pij-first-class-ui",
			PIJ_PLAN_ID: "073-pij-first-class-ui",
		});
	});

	it("omits PIJ_PARENT_ID when no parentId given (caller unresolved)", () => {
		expect(buildControlSpawnCommand(base).env).not.toHaveProperty("PIJ_PARENT_ID");
	});

	it("sets PIJ_PARENT_ID to the spawner id when parentId given (who spawned me)", () => {
		const r = buildControlSpawnCommand({ ...base, parentId: "pij-boss01" });
		expect(r.env.PIJ_PARENT_ID).toBe("pij-boss01");
		expect(r.env.PIJ_SESSION_ID).toBe("pij-bright-otter"); // self id unchanged
	});

	it("sets PIJ_SPAWN_TASK only when a task is given", () => {
		expect(buildControlSpawnCommand(base).env).not.toHaveProperty("PIJ_SPAWN_TASK");
		expect(buildControlSpawnCommand({ ...base, task: "review the diff" }).env.PIJ_SPAWN_TASK).toBe(
			"review the diff",
		);
	});

	it("copilot: cmd is 'copilot' with --yolo + --session-id (deterministic bind)", () => {
		const r = buildControlSpawnCommand({
			harness: "copilot",
			pijId: "pij-cop",
			cwd: "/repo",
			copilotSessionId: "9a8f8be6-3670-4e5c-b43e-09f46fe4dfad",
		});
		expect(r.cmd).toBe("copilot");
		expect(r.args).toEqual(["--yolo", "--session-id", "9a8f8be6-3670-4e5c-b43e-09f46fe4dfad"]);
		expect(r.env.PIJ_HARNESS).toBe("copilot");
		expect(r.env.PIJ_SESSION_ID).toBe("pij-cop");
	});

	it("copilot: --model rides after --session-id as discrete argv", () => {
		const r = buildControlSpawnCommand({
			harness: "copilot",
			pijId: "pij-cop",
			cwd: "/repo",
			copilotSessionId: "uuid-1",
			model: "gpt-5.5",
		});
		expect(r.args).toEqual([
			"--yolo",
			"--session-id",
			"uuid-1",
			"--model",
			"gpt-5.5",
			"--context",
			"long_context",
		]);
	});

	it("copilot: a pinned model always selects the long-context tier", () => {
		const r = buildControlSpawnCommand({
			harness: "copilot",
			pijId: "pij-cop",
			cwd: "/repo",
			model: "claude-opus-5",
		});
		expect(r.args).toContain("claude-opus-5");
		expect(r.args).toContain("long_context");
	});

	it("claude --branch: --resume <from> --fork-session --session-id <new> after skip-permissions (AC-01)", () => {
		const r = buildControlSpawnCommand({
			...base,
			branchFrom: "claude-src",
			forkSessionId: "fork-uuid",
		});
		expect(r.args).toEqual([
			"--dangerously-skip-permissions",
			"--resume",
			"claude-src",
			"--fork-session",
			"--session-id",
			"fork-uuid",
		]);
	});

	it("claude --branch + --model: model rides last (fork onto another model)", () => {
		const r = buildControlSpawnCommand({
			...base,
			branchFrom: "claude-src",
			forkSessionId: "fork-uuid",
			model: "sonnet",
		});
		expect(r.args).toEqual([
			"--dangerously-skip-permissions",
			"--resume",
			"claude-src",
			"--fork-session",
			"--session-id",
			"fork-uuid",
			"--model",
			"sonnet",
		]);
	});

	it("pi focus fork uses the self-registering spawn env without a caller-owned pij id", () => {
		const r = buildPiFocusSpawnCommand({
			harness: "pi",
			spawnId: "focus-spawn",
			announceTo: "pij-parent",
			cwd: "/repo",
			role: "worker",
			snapshotPath: "/home/.pij/focus/golden/snapshot.jsonl",
			forkSessionId: "019f-focus-native",
			sessionDir: "/home/.pij/focus-launches/019f-focus-native/pi-sessions",
			model: "github-copilot/gpt-5.6-sol",
			effort: "xhigh",
		});
		expect(r.cmd).toBe("pi");
		expect(r.args).toEqual([
			"--fork",
			"/home/.pij/focus/golden/snapshot.jsonl",
			"--session-dir",
			"/home/.pij/focus-launches/019f-focus-native/pi-sessions",
			"--session-id",
			"019f-focus-native",
			"--model",
			"github-copilot/gpt-5.6-sol:xhigh",
		]);
		expect(r.env).toMatchObject({
			PIJ_ANNOUNCE_TO: "pij-parent",
			PIJ_PARENT_ID: "pij-parent",
			PIJ_SPAWN_ID: "focus-spawn",
			PIJ_ROLE: "worker",
		});
		expect(r.env).not.toHaveProperty("PIJ_SESSION_ID");
	});

	it("no branchFrom → today's claude args are byte-identical (AC-05 regression)", () => {
		expect(buildControlSpawnCommand(base).args).toEqual(["--dangerously-skip-permissions"]);
	});

	// ─── codex arm (Plan 022, AC-01/05/07; Finding 05) ──────────────────────────
	it("codex: cmd is 'codex' with --dangerously-bypass-approvals-and-sandbox (driven pane, no human to approve)", () => {
		const r = buildControlSpawnCommand({ harness: "codex", pijId: "pij-cdx", cwd: "/repo" });
		expect(r.cmd).toBe("codex");
		expect(r.args).toEqual(["--dangerously-bypass-approvals-and-sandbox"]);
		expect(r.env.PIJ_HARNESS).toBe("codex");
		expect(r.env.PIJ_SESSION_ID).toBe("pij-cdx");
	});

	it("codex: --model rides after the bypass flag as discrete argv (AC-09: no shell string)", () => {
		const r = buildControlSpawnCommand({
			harness: "codex",
			pijId: "pij-cdx",
			cwd: "/repo",
			model: "gpt-5.5",
		});
		expect(r.args).toEqual(["--dangerously-bypass-approvals-and-sandbox", "--model", "gpt-5.5"]);
	});

	it("codex: sets PIJ_PARENT_ID when given; NEVER a deterministic --session-id (discovery-bound, AC-02/05)", () => {
		const r = buildControlSpawnCommand({
			harness: "codex",
			pijId: "pij-cdx",
			cwd: "/repo",
			parentId: "pij-boss",
			model: "o3",
		});
		expect(r.env.PIJ_PARENT_ID).toBe("pij-boss");
		expect(r.env.PIJ_SESSION_ID).toBe("pij-cdx");
		// codex auto-generates its UUID (F-01) — no --session-id like copilot.
		expect(r.args).not.toContain("--session-id");
	});

	it("codex: sets PIJ_SPAWN_TASK only when a task is given", () => {
		expect(
			buildControlSpawnCommand({ harness: "codex", pijId: "p", cwd: "/r" }).env,
		).not.toHaveProperty("PIJ_SPAWN_TASK");
		expect(
			buildControlSpawnCommand({ harness: "codex", pijId: "p", cwd: "/r", task: "go" }).env
				.PIJ_SPAWN_TASK,
		).toBe("go");
	});

	// ── --effort translation (#3) — per-harness flag drift pinned by exact argv ──
	it("claude: appends --effort <level> after the model (flag-drift pin)", () => {
		const r = buildControlSpawnCommand({ ...base, model: "sonnet", effort: "high" });
		expect(r.args).toEqual([
			"--dangerously-skip-permissions",
			"--model",
			"sonnet",
			"--effort",
			"high",
		]);
	});

	it("copilot: appends --effort <level> (same flag as claude)", () => {
		const r = buildControlSpawnCommand({
			harness: "copilot",
			pijId: "pij-cop",
			cwd: "/repo",
			copilotSessionId: "u",
			effort: "medium",
		});
		expect(r.args).toEqual(["--yolo", "--session-id", "u", "--effort", "medium"]);
	});

	it("codex: translates effort to -c model_reasoning_effort=<level> (no --effort flag)", () => {
		const r = buildControlSpawnCommand({
			harness: "codex",
			pijId: "pij-cdx",
			cwd: "/repo",
			effort: "xhigh",
		});
		expect(r.args).toEqual([
			"--dangerously-bypass-approvals-and-sandbox",
			"-c",
			"model_reasoning_effort=xhigh",
		]);
		expect(r.args).not.toContain("--effort");
	});

	it("unset effort → NO effort flag for any harness (default-to-agent)", () => {
		expect(buildControlSpawnCommand(base).args).not.toContain("--effort");
		expect(
			buildControlSpawnCommand({ harness: "codex", pijId: "p", cwd: "/r" }).args,
		).not.toContain("-c");
	});
});

describe("buildPendingDescriptor", () => {
	const input = {
		pijId: "pij-bright-otter",
		paneId: "%42",
		cwd: "/repo",
		harness: "claude" as const,
		dataDir: "/home/.pij/pij-bright-otter",
		eventsPath: "/home/.pij/pij-bright-otter/events.ndjson",
		pid: 4242,
		startedAtIso: "2026-06-27T00:00:00.000Z",
	};

	it("carries (id, paneId, cwd, harness) with lifecycle 'pending' (F2/AC-01)", () => {
		const d = buildPendingDescriptor(input);
		expect(d).toMatchObject({
			id: "pij-bright-otter",
			paneId: "%42",
			folder: "/repo",
			harness: "claude",
			lifecycle: "pending",
		});
	});

	it("is NOT yet bound — no harnessSessionId, no initInjectedAt", () => {
		const d = buildPendingDescriptor(input);
		expect(d.harnessSessionId).toBeUndefined();
		expect(d.initInjectedAt).toBeUndefined();
	});

	it("carries plannedHarnessSessionId for copilot (the daemon binds to it)", () => {
		const d = buildPendingDescriptor({
			...input,
			harness: "copilot",
			plannedHarnessSessionId: "uuid-9",
		});
		expect(d.plannedHarnessSessionId).toBe("uuid-9");
		// still pending until the daemon confirms ready + injects init.
		expect(d.lifecycle).toBe("pending");
		expect(d.harnessSessionId).toBeUndefined();
	});

	it("stamps daemon-bound Copilot descriptors with github-copilot provider", () => {
		const d = buildPendingDescriptor({
			...input,
			harness: "copilot",
			model: "gpt-5.6-sol",
		});
		expect(d.boundProvider).toBe("github-copilot");
	});

	it("omits plannedHarnessSessionId for claude (discovery-bound)", () => {
		expect(buildPendingDescriptor(input)).not.toHaveProperty("plannedHarnessSessionId");
	});

	it("branched claude: carries plannedHarnessSessionId + branchedFrom, no transcriptsAtSpawn (AC-02)", () => {
		const d = buildPendingDescriptor({
			...input,
			plannedHarnessSessionId: "fork-uuid",
			branchedFrom: "claude-src",
		});
		expect(d.plannedHarnessSessionId).toBe("fork-uuid");
		expect(d.branchedFrom).toBe("claude-src");
		expect(d).not.toHaveProperty("transcriptsAtSpawn");
		expect(d.lifecycle).toBe("pending");
	});

	it("omits branchedFrom for a normal (non-branch) spawn", () => {
		expect(buildPendingDescriptor(input)).not.toHaveProperty("branchedFrom");
	});

	it("persists the pinned model + effort before first inference (registry truth)", () => {
		expect(
			buildPendingDescriptor({
				...input,
				model: "gpt-5.6-sol",
				effort: "xhigh",
			}),
		).toMatchObject({
			boundModel: "gpt-5.6-sol",
			effort: "xhigh",
		});
	});

	it("persists the explicit plan id on a daemon-bound descriptor", () => {
		expect(
			buildPendingDescriptor({
				...input,
				planId: "073-pij-first-class-ui",
			}).planId,
		).toBe("073-pij-first-class-ui");
	});

	it("keeps model + effort additive for legacy/default spawns", () => {
		const descriptor = buildPendingDescriptor(input);
		expect(descriptor.boundModel).toBeUndefined();
		expect(descriptor.effort).toBeUndefined();
	});

	it.each([
		["pij-structural-parent", "pij-structural-parent"],
		[null, null],
	] as const)("persists structural parent %s and repository identity without changing spawn ownership", (parentId, expectedParentId) => {
		expect(
			buildPendingDescriptor({
				...input,
				parentId,
				gitCommonDir: "/repo/.git",
				spawnedBy: "pij-close-owner",
				model: "gpt-5.6-sol",
				effort: "xhigh",
				plannedHarnessSessionId: "fork-native",
				branchedFrom: "source-native",
			}),
		).toMatchObject({
			parentId: expectedParentId,
			gitCommonDir: "/repo/.git",
			spawnedBy: "pij-close-owner",
			boundModel: "gpt-5.6-sol",
			effort: "xhigh",
			plannedHarnessSessionId: "fork-native",
			branchedFrom: "source-native",
		});
	});
});

describe("buildSpawnOutput", () => {
	it("reports pinned model + effort for a daemon-bound spawn", () => {
		expect(
			buildSpawnOutput({
				id: "pij-worker",
				paneId: "%42",
				harness: "copilot",
				lifecycle: "pending",
				model: "gpt-5.6-sol",
				effort: "xhigh",
				planId: "073-pij-first-class-ui",
				warnings: ["warning: unresolved plan"],
			}),
		).toEqual({
			id: "pij-worker",
			paneId: "%42",
			harness: "copilot",
			lifecycle: "pending",
			model: "gpt-5.6-sol",
			effort: "xhigh",
			planId: "073-pij-first-class-ui",
			warnings: ["warning: unresolved plan"],
		});
	});

	it("reports explicit nulls for a pi/default spawn whose id is assigned at boot", () => {
		expect(buildSpawnOutput({ paneId: "%43", harness: "pi" })).toEqual({
			id: null,
			paneId: "%43",
			harness: "pi",
			lifecycle: null,
			model: null,
			effort: null,
			planId: null,
			warnings: [],
		});
	});

	it("carries warnings in JSON and appends them as human receipt lines", () => {
		const output = buildSpawnOutput({
			paneId: "%43",
			harness: "pi",
			planId: "073-missing",
			warnings: ["warning: unresolved plan"],
		});
		expect(JSON.parse(renderSpawnReceipt(output, "spawned pi worker", true))).toMatchObject({
			warnings: ["warning: unresolved plan"],
		});
		expect(renderSpawnReceipt(output, "spawned pi worker", false)).toBe(
			"spawned pi worker\nwarning: unresolved plan",
		);
	});
});

describe("buildPlanIdWarning", () => {
	it("warns with the spawn cwd path when docs/plans/<id> is unresolved", () => {
		expect(buildPlanIdWarning("073-missing", "/repo", () => false)).toBe(
			"warning: plan id '073-missing' does not resolve to '/repo/docs/plans/073-missing' — spawn continues",
		);
	});

	it("reports opaque non-segment plan ids as not checked without probing the filesystem", () => {
		for (const planId of [".", "..", "../../opaque/value", String.raw`opaque\value`]) {
			expect(
				buildPlanIdWarning(planId, "/repo", () => {
					throw new Error("non-segment plan ids must not be probed as paths");
				}),
			).toBe(
				`warning: plan id '${planId}' was not checked against docs/plans (not a simple path segment) — spawn continues`,
			);
		}
	});

	it("is silent when the plan directory resolves or no plan id was supplied", () => {
		expect(
			buildPlanIdWarning("073-known", "/repo", (path) => path.endsWith("/073-known")),
		).toBeNull();
		expect(buildPlanIdWarning(undefined, "/repo", () => false)).toBeNull();
	});
});

describe("parseCompactSelfArgs", () => {
	it("no args → uses the env pane, default 1500ms, no instruction", () => {
		expect(parseCompactSelfArgs([], "%72")).toEqual({ pane: "%72", delayMs: 1500 });
	});

	it("joins non-flag tokens into the instruction (unquoted multi-word works)", () => {
		expect(parseCompactSelfArgs(["resume", "the", "build"], "%72")).toEqual({
			pane: "%72",
			delayMs: 1500,
			instruction: "resume the build",
		});
	});

	it("--pane and --delay-ms override (space and = forms), instruction is the rest", () => {
		expect(parseCompactSelfArgs(["--pane", "%9", "--delay-ms", "2000", "go", "now"])).toEqual({
			pane: "%9",
			delayMs: 2000,
			instruction: "go now",
		});
		expect(parseCompactSelfArgs(["--pane=%9", "--delay-ms=800", "carry on"], "%72")).toEqual({
			pane: "%9",
			delayMs: 800,
			instruction: "carry on",
		});
	});

	it("ignores a non-numeric/negative delay (keeps the default)", () => {
		expect(parseCompactSelfArgs(["--delay-ms", "nope"], "%72").delayMs).toBe(1500);
		expect(parseCompactSelfArgs(["--delay-ms=-5"], "%72").delayMs).toBe(1500);
	});
});

describe("planControlSplit (default side stack, parity with pi)", () => {
	it("worker #1 → split the orchestrator pane right (~1/3 column)", () => {
		expect(planControlSplit("%72", [])).toEqual({
			ok: true,
			target: "%72",
			direction: "h",
			percent: 33,
		});
	});

	it("worker #2 → split worker-1's pane vertically (stacked below) + even out + pin width", () => {
		expect(planControlSplit("%72", ["%80"])).toEqual({
			ok: true,
			target: "%80",
			direction: "v",
			evenOut: true,
			columnPercent: 33,
		});
	});

	it("worker #3+ → appends below the NEWEST peer (uncapped, evened)", () => {
		expect(planControlSplit("%72", ["%80", "%81"])).toEqual({
			ok: true,
			target: "%81",
			direction: "v",
			evenOut: true,
			columnPercent: 33,
		});
		expect(planControlSplit("%72", ["%80", "%81", "%82", "%83"])).toEqual({
			ok: true,
			target: "%83",
			direction: "v",
			evenOut: true,
			columnPercent: 33,
		});
	});
});

describe("parseSpawnArgs (T018)", () => {
	it("parses --harness with --task/--model/--json (space and = forms)", () => {
		expect(parseSpawnArgs(["--harness", "claude", "--task", "review the diff"])).toEqual({
			ok: true,
			value: {
				harness: "claude",
				task: "review the diff",
				model: undefined,
				planId: undefined,
				branch: false,
				json: false,
			},
		});
		expect(parseSpawnArgs(["--harness=claude", "--model=opus", "--json"])).toMatchObject({
			ok: true,
			value: { harness: "claude", model: "opus", json: true },
		});
	});

	it("parses --plan-id as an opaque identifier without path-shaped validation", () => {
		expect(
			parseSpawnArgs(["--harness", "claude", "--plan-id", "../../opaque/value"]),
		).toMatchObject({
			ok: true,
			value: { harness: "claude", planId: "../../opaque/value" },
		});
	});

	it("rejects missing, empty, and whitespace-only plan-id values", () => {
		expect(parseSpawnArgs(["--harness", "claude", "--plan-id"])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
		expect(parseSpawnArgs(["--harness", "claude", "--plan-id="])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
		expect(parseSpawnArgs(["--harness", "claude", "--plan-id", "   "])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
	});

	it("accepts --branch as a boolean flag (default false)", () => {
		const off = parseSpawnArgs(["--harness", "claude"]);
		expect(off).toMatchObject({ ok: true, value: { branch: false } });
		const on = parseSpawnArgs(["--harness", "claude", "--branch"]);
		expect(on).toMatchObject({ ok: true, value: { harness: "claude", branch: true } });
		// --branch composes with --model (fork onto another model)
		expect(parseSpawnArgs(["--harness", "claude", "--branch", "--model", "sonnet"])).toMatchObject({
			ok: true,
			value: { harness: "claude", branch: true, model: "sonnet" },
		});
	});

	it("accepts pi (Plan 021 — uniform spawn surface), requires --harness, rejects unknown", () => {
		expect(parseSpawnArgs([])).toMatchObject({ ok: false, code: "E-ARG" });
		// pi is now a first-class spawnable harness (was rejected pre-021).
		expect(parseSpawnArgs(["--harness", "pi"])).toMatchObject({
			ok: true,
			value: { harness: "pi", branch: false },
		});
		expect(
			parseSpawnArgs(["--harness", "pi", "--task", "review the diff", "--model", "@preset/glm-1m"]),
		).toMatchObject({
			ok: true,
			value: { harness: "pi", task: "review the diff", model: "@preset/glm-1m" },
		});
		expect(parseSpawnArgs(["--harness", "bogus"])).toMatchObject({ ok: false, code: "E-ARG" });
	});

	it("accepts codex (Plan 022 — 4th spawnable harness)", () => {
		expect(parseSpawnArgs(["--harness", "codex"])).toMatchObject({
			ok: true,
			value: { harness: "codex", branch: false },
		});
		expect(parseSpawnArgs(["--harness=codex", "--model=gpt-5.5"])).toMatchObject({
			ok: true,
			value: { harness: "codex", model: "gpt-5.5" },
		});
	});

	it("rejects unknown flags and missing values", () => {
		expect(parseSpawnArgs(["--harness", "claude", "--nope", "x"])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
		expect(parseSpawnArgs(["--harness"])).toMatchObject({ ok: false, code: "E-ARG" });
	});

	it("accepts omp as a harness, not a pi binary switch", () => {
		expect(parseSpawnArgs(["--harness", "omp"])).toMatchObject({
			ok: true,
			value: { harness: "omp" },
		});
	});

	it("refuses a binary harness selector with the explicit harness repair", () => {
		expect(parseSpawnArgs(["--harness", "pi", "--bin", "omp"])).toMatchObject({
			ok: false,
			code: "E-ARG",
			message: expect.stringContaining("--harness omp"),
		});
	});

	it("accepts an executable path without selecting a different harness", () => {
		expect(parseSpawnArgs(["--harness=omp", "--bin=/opt/omp"])).toMatchObject({
			ok: true,
			value: { harness: "omp", bin: "/opt/omp" },
		});
	});

	it("rejects an executable whose basename names a different harness", () => {
		expect(parseSpawnArgs(["--harness", "pi", "--bin", "/opt/omp"])).toMatchObject({
			ok: false,
			code: "E-ARG",
			message: expect.stringContaining("--harness omp"),
		});
	});

	it("refuses omp branching before a pane is created", () => {
		const result = parseSpawnArgs(["--harness", "omp", "--branch"]);
		expect(result).toMatchObject({
			ok: false,
			code: "E-ARG",
			message: expect.stringContaining("--branch"),
		});
	});

	it("parses --effort (space and = forms); unset → effort undefined (#3, AC-04)", () => {
		expect(parseSpawnArgs(["--harness", "claude", "--effort", "high"])).toMatchObject({
			ok: true,
			value: { harness: "claude", effort: "high" },
		});
		expect(parseSpawnArgs(["--harness=codex", "--effort=xhigh"])).toMatchObject({
			ok: true,
			value: { harness: "codex", effort: "xhigh" },
		});
		const unset = parseSpawnArgs(["--harness", "claude"]);
		expect(unset.ok).toBe(true);
		if (unset.ok) expect(unset.value.effort).toBeUndefined();
	});
});

describe("resolvePiModelBinding", () => {
	const model = (provider: string, id = "gpt-5.6-sol"): ModelEntry => ({
		id,
		name: id,
		provider,
		verified: true,
	});

	it("pins an ambiguous Copilot-family bare id to github-copilot with a notice", () => {
		const result = resolvePiModelBinding("gpt-5.6-sol", [
			model("github-copilot"),
			model("copilot"),
			model("codex"),
		]);
		expect(result).toMatchObject({
			ok: true,
			value: {
				model: "github-copilot/gpt-5.6-sol",
				provider: "github-copilot",
			},
		});
		if (result.ok) {
			expect(result.value.notice).toContain("github-copilot, copilot, codex");
			expect(result.value.notice).toContain("defaulting");
		}
	});

	it("preserves an explicit provider-qualified selection", () => {
		expect(
			resolvePiModelBinding("codex/gpt-5.6-sol", [model("github-copilot"), model("codex")]),
		).toEqual({
			ok: true,
			value: { model: "codex/gpt-5.6-sol", provider: "codex" },
		});
	});

	it("qualifies a unique exact provider and leaves an unknown id unchanged", () => {
		expect(resolvePiModelBinding("fugu", [model("sakana", "fugu")])).toEqual({
			ok: true,
			value: { model: "sakana/fugu", provider: "sakana" },
		});
		expect(resolvePiModelBinding("custom-model", [model("sakana", "fugu")])).toEqual({
			ok: true,
			value: { model: "custom-model" },
		});
	});

	it("fails loud on non-Copilot provider ambiguity and lists actionable candidates", () => {
		const result = resolvePiModelBinding("shared-model", [
			model("openrouter", "shared-model"),
			model("sakana", "shared-model"),
		]);
		expect(result).toMatchObject({ ok: false, code: "E-AMBIGUOUS" });
		if (!result.ok) {
			expect(result.message).toContain("openrouter/shared-model");
			expect(result.message).toContain("sakana/shared-model");
		}
	});
});

describe("buildEffortWarning (warn-don't-block vs a model's levels, #3)", () => {
	const KNOWN: ModelEntry[] = [
		{
			id: "fugu",
			name: "Sakana Fugu",
			provider: "sakana",
			verified: true,
			reasoning: true,
			levels: ["high", "xhigh"],
		},
	];

	it("returns null when effort is supported / unset / model unknown", () => {
		expect(buildEffortWarning("high", "fugu", KNOWN)).toBeNull();
		expect(buildEffortWarning(undefined, "fugu", KNOWN)).toBeNull();
		expect(buildEffortWarning("high", "unknown-model", KNOWN)).toBeNull();
	});

	it("warns (never throws) when effort is unsupported, listing the model's levels", () => {
		const w = buildEffortWarning("medium", "fugu", KNOWN);
		expect(w).toMatch(/medium/);
		expect(w).toMatch(/high/);
	});

	const GPT56_LEVELS = ["none", "low", "medium", "high", "xhigh", "max"];
	const GPT56 = parseModelsJson({
		providers: {
			"github-copilot": {
				models: [
					{
						id: "gpt-5.6-sol",
						reasoning: true,
						thinkingLevelMap: { xhigh: "xhigh", max: "max" },
					},
				],
			},
		},
	});

	it.each(GPT56_LEVELS)("does not warn for supported GPT-5.6 effort %s", (level) => {
		expect(buildEffortWarning(level, "gpt-5.6-sol", GPT56)).toBeNull();
	});

	it("warns and continues for unsupported GPT-5.6 minimal with the corrected levels", () => {
		expect(buildEffortWarning("minimal", "gpt-5.6-sol", GPT56)).toBe(
			"warning: effort 'minimal' may be unsupported for 'gpt-5.6-sol' — gpt-5.6-sol supports: none, low, medium, high, xhigh, max; spawn continues",
		);
	});
});

describe("livePeerPanes (shared split-cap input, Plan 021)", () => {
	const d = (paneId: string | undefined, harness: HarnessKind): SessionDescriptor =>
		({ paneId, harness }) as SessionDescriptor;

	it("counts EVERY live peer pane in the window regardless of harness (the asymmetry fix)", () => {
		// A pi pane and a claude pane both count toward the same main+2 cap. A mutation
		// re-introducing a `harness === claude|copilot` filter would drop the pi pane
		// and flip this assertion RED.
		const descriptors = [d("%2", "claude"), d("%3", "pi"), d("%4", "copilot")];
		const windowPanes = ["%1", "%2", "%3", "%4"];
		expect(livePeerPanes(descriptors, windowPanes, "%1")).toEqual(["%2", "%3", "%4"]);
	});

	it("excludes own pane, dead/closed panes (not in windowPanes), and paneless descriptors", () => {
		const descriptors = [
			d("%1", "pi"), // own pane → excluded
			d("%9", "claude"), // not in windowPanes (closed) → excluded
			d(undefined, "pi"), // no paneId → excluded
			d("%2", "pi"), // live peer → kept
		];
		expect(livePeerPanes(descriptors, ["%1", "%2"], "%1")).toEqual(["%2"]);
	});

	it("dedupes repeated pane ids", () => {
		expect(livePeerPanes([d("%2", "pi"), d("%2", "claude")], ["%1", "%2"], "%1")).toEqual(["%2"]);
	});
});

describe("planBranch (branch-from-self gating, Plan 020)", () => {
	const supports = (h: HarnessKind) => h === "claude";
	const boundSelf = (over: Partial<SessionDescriptor> = {}): SessionDescriptor => ({
		id: "pij-me",
		folder: "/repo",
		dataDir: "/d",
		eventsPath: "/e",
		pid: 1,
		startedAt: "2026-06-28T00:00:00.000Z",
		harness: "claude",
		harnessSessionId: "claude-self",
		lifecycle: "bound",
		...over,
	});

	it("bound same-harness claude → ok {from: self's harnessSessionId, newSessionId}", () => {
		expect(planBranch("claude", boundSelf(), supports, "new-uuid")).toEqual({
			ok: true,
			value: { from: "claude-self", newSessionId: "new-uuid" },
		});
	});

	it("unsupported harness (copilot today) → E-BRANCH (AC-04)", () => {
		expect(planBranch("copilot", boundSelf({ harness: "copilot" }), supports, "u")).toMatchObject({
			ok: false,
			code: "E-BRANCH",
		});
	});

	it("pi can spawn (Plan 021) but cannot fork → E-BRANCH (the bin mirrors this guard)", () => {
		expect(
			planBranch("pi", boundSelf({ harness: "pi", harnessSessionId: "pi-self" }), supports, "u"),
		).toMatchObject({ ok: false, code: "E-BRANCH" });
	});

	it("codex can spawn (Plan 022) but cannot fork-from-self → E-BRANCH (AC-07)", () => {
		expect(
			planBranch("codex", boundSelf({ harness: "codex", harnessSessionId: "x" }), supports, "u"),
		).toMatchObject({ ok: false, code: "E-BRANCH" });
	});

	it("caller unresolved (self null) → E-BRANCH (AC-04)", () => {
		expect(planBranch("claude", null, supports, "u")).toMatchObject({
			ok: false,
			code: "E-BRANCH",
		});
	});

	it("harness mismatch (caller copilot, spawning claude) → E-BRANCH (AC-04)", () => {
		expect(
			planBranch("claude", boundSelf({ harness: "copilot", harnessSessionId: "x" }), supports, "u"),
		).toMatchObject({ ok: false, code: "E-BRANCH" });
	});

	it("caller not bound (no harnessSessionId) → E-BRANCH (AC-04)", () => {
		const r = planBranch("claude", boundSelf({ harnessSessionId: undefined }), supports, "u");
		expect(r).toMatchObject({ ok: false, code: "E-BRANCH" });
		if (!r.ok) expect(r.message).toMatch(/not bound/i);
	});
});

describe("parseAdoptArgs (T023)", () => {
	it("parses <pane> + --harness + optional --id/--session-id/--json", () => {
		expect(parseAdoptArgs(["%72", "--harness", "claude"])).toEqual({
			ok: true,
			value: {
				pane: "%72",
				harness: "claude",
				id: undefined,
				sessionId: undefined,
				json: false,
			},
		});
		expect(
			parseAdoptArgs([
				"%5",
				"--harness=claude",
				"--id=pij-x",
				"--session-id=native-x",
				"--parent=pij-prime",
				"--json",
			]),
		).toMatchObject({
			ok: true,
			value: {
				pane: "%5",
				harness: "claude",
				id: "pij-x",
				sessionId: "native-x",
				parentId: "pij-prime",
				json: true,
			},
		});
	});

	it("rejects adopting a self-registering Pi seat", () => {
		expect(parseAdoptArgs(["%72", "--harness", "pi"])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
	});

	it("rejects missing --session-id/--parent values instead of consuming the next flag", () => {
		expect(parseAdoptArgs(["%7", "--harness", "claude", "--session-id", "--json"])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
		expect(parseAdoptArgs(["%7", "--harness", "claude", "--parent", "--json"])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
	});

	it("requires a %N pane and a harness", () => {
		expect(parseAdoptArgs(["--harness", "claude"])).toMatchObject({ ok: false, code: "E-ARG" });
		expect(parseAdoptArgs(["notapane", "--harness", "claude"])).toMatchObject({
			ok: false,
			code: "E-ARG",
		});
		expect(parseAdoptArgs(["%72"])).toMatchObject({ ok: false, code: "E-ARG" });
	});
});

describe("aliasAgentSpawnArgs — `pij spawn --agent` forwards to `pij agent spawn`", () => {
	it("returns null when --agent is absent (a normal colleague spawn)", () => {
		expect(aliasAgentSpawnArgs(["--harness", "claude", "--model", "sonnet"])).toBeNull();
	});

	it("puts the slug first and forwards every other flag verbatim", () => {
		expect(
			aliasAgentSpawnArgs([
				"--agent",
				"flowspace-search",
				"-p",
				"query=x",
				"--once",
				"--model",
				"sonnet",
			]),
		).toEqual(["spawn", "flowspace-search", "-p", "query=x", "--once", "--model", "sonnet"]);
	});

	it("supports the --agent=<slug> inline form", () => {
		expect(aliasAgentSpawnArgs(["--agent=my-pack", "-p", "q=1"])).toEqual([
			"spawn",
			"my-pack",
			"-p",
			"q=1",
		]);
	});
});

// ─── FX001-3 / SUGG-001: explicit placement ──────────────────────────────────
describe("planPlacement", () => {
	it("undefined layout defaults to the side stack (uncapped)", () => {
		expect(planPlacement(undefined, "%1", [])).toEqual({
			ok: true,
			target: "%1",
			direction: "h",
			percent: 33,
		});
		expect(planPlacement(undefined, "%1", ["%2"])).toEqual({
			ok: true,
			target: "%2",
			direction: "v",
			evenOut: true,
			columnPercent: 33,
		});
		expect(planPlacement(undefined, "%1", ["%2", "%3", "%4"])).toEqual({
			ok: true,
			target: "%4",
			direction: "v",
			evenOut: true,
			columnPercent: 33,
		});
	});
	it("explicit 'stack' is the same as the default", () => {
		expect(planPlacement("stack", "%1", ["%2", "%3"])).toEqual(
			planPlacement(undefined, "%1", ["%2", "%3"]),
		);
	});
	it("right/below split the CALLER's pane", () => {
		expect(planPlacement("right", "%1", ["%2"])).toEqual({
			ok: true,
			target: "%1",
			direction: "h",
			percent: 40,
		});
		expect(planPlacement("below", "%1", [])).toEqual({ ok: true, target: "%1", direction: "v" });
	});
	it("right/below still honour the main+2 cap", () => {
		const r = planPlacement("right", "%1", ["%2", "%3"]);
		expect(r.ok).toBe(false);
		if (!r.ok) expect(r.code).toBe("E-FULL");
	});
	it("window is cap-exempt", () => {
		expect(planPlacement("window", "%1", ["%2", "%3"])).toEqual({ ok: true, window: true });
	});
});

describe("parseSpawnArgs --layout", () => {
	it("accepts stack|right|below|window", () => {
		const r = parseSpawnArgs(["--harness", "claude", "--layout", "window"]);
		expect(r.ok).toBe(true);
		if (r.ok) expect(r.value.layout).toBe("window");
		const s = parseSpawnArgs(["--harness", "claude", "--layout", "stack"]);
		expect(s.ok).toBe(true);
		if (s.ok) expect(s.value.layout).toBe("stack");
	});
	it("rejects an unknown layout", () => {
		const r = parseSpawnArgs(["--harness", "claude", "--layout", "floating"]);
		expect(r.ok).toBe(false);
	});
});

// ─── plan 054 P2 T006 — windowId capture at spawn (AC-09) ───────────────────
describe("buildPendingDescriptor windowId (AC-09 terminal addressability)", () => {
	const base = {
		pijId: "pij-w",
		paneId: "%9",
		cwd: "/repo",
		harness: "claude" as const,
		dataDir: "/home/.pij/pij-w",
		eventsPath: "/home/.pij/pij-w/events.ndjson",
		pid: 42,
		startedAtIso: "2026-07-17T00:00:00.000Z",
	};

	it("carries the tmux window id captured beside the pane id", () => {
		const d = buildPendingDescriptor({ ...base, windowId: "@4" });
		expect(d.windowId).toBe("@4");
		expect(d.paneId).toBe("%9");
	});

	it("stays ABSENT (no key) when the capture did not yield one", () => {
		const d = buildPendingDescriptor(base);
		expect("windowId" in d).toBe(false);
	});
});

describe("buildPendingDescriptor systemState (AC-04 — starting at spawn/adopt)", () => {
	it("stamps the mechanical axis starting at birth; it holds until the bind verdict", () => {
		const d = buildPendingDescriptor({
			pijId: "pij-w",
			paneId: "%9",
			cwd: "/repo",
			harness: "claude",
			dataDir: "/home/.pij/pij-w",
			eventsPath: "/home/.pij/pij-w/events.ndjson",
			pid: 42,
			startedAtIso: "2026-07-17T00:00:00.000Z",
		});
		expect(d.systemState).toBe("starting");
	});
});

// ─── deriveCallerParent (plan 054 P3 T001/T002 — AC-08 caller truth) ─────────
// Behavior contracts ONLY (SW-7): outcomes of parent derivation, never the
// internals that produce them. The invoking session is the parent; cwd
// cohabitation NEVER makes a parent (issue #20's corruption mechanism).

describe("deriveCallerParent (AC-08 — parent is the invoking session, never cwd)", () => {
	const at = (id: string, over: Partial<SessionDescriptor> = {}): SessionDescriptor => ({
		id,
		folder: "/repo",
		dataDir: `/home/.pij/${id}`,
		eventsPath: `/home/.pij/${id}/events.ndjson`,
		pid: 100,
		startedAt: "2026-07-17T00:00:00.000Z",
		...over,
	});

	it("PIJ_SESSION_ID wins outright, even with cwd cohabitants and a matching pane", () => {
		const all = [at("pij-env"), at("pij-neighbor"), at("pij-paned", { paneId: "%7" })];
		expect(deriveCallerParent("pij-env", all, "%7")).toBe("pij-env");
	});

	it("env unset: a unique pane-exact match identifies the caller", () => {
		const all = [at("pij-a"), at("pij-caller", { paneId: "%7" })];
		expect(deriveCallerParent(undefined, all, "%7")).toBe("pij-caller");
		expect(deriveCallerParent("", all, "%7")).toBe("pij-caller");
		expect(deriveCallerParent("   ", all, "%7")).toBe("pij-caller");
	});

	it("pane matching runs against the FULL registry — a caller registered under a DIFFERENT folder still resolves (adopted-peer-in-worktree shape)", () => {
		const all = [
			at("pij-local-neighbor", { folder: "/worktree" }),
			at("pij-caller", { folder: "/elsewhere", paneId: "%7" }),
		];
		// Caller invokes from /worktree; its registered folder is /elsewhere.
		// cwd plays NO role: the pane identity alone resolves the parent.
		expect(deriveCallerParent(undefined, all, "%7")).toBe("pij-caller");
	});

	it("env unset + no pane match: cwd cohabitants NEVER become the parent (issue #20 kill)", () => {
		// The #20 shape: exactly one descriptor sharing the launch cwd. The old
		// lone-local inference made it the parent; caller truth says ABSENT.
		const loneCohabitant = [at("pij-neighbor", { folder: "/worktree" })];
		expect(deriveCallerParent(undefined, loneCohabitant, undefined)).toBeUndefined();
		expect(deriveCallerParent(undefined, loneCohabitant, "%none")).toBeUndefined();
	});

	it("ambiguous pane (duplicate registrations) yields NO parent, never a guess", () => {
		const all = [at("pij-x", { paneId: "%7" }), at("pij-y", { paneId: "%7" })];
		expect(deriveCallerParent(undefined, all, "%7")).toBeUndefined();
	});

	it("nothing to go on — empty registry, no env, no pane — yields no parent", () => {
		expect(deriveCallerParent(undefined, [], undefined)).toBeUndefined();
		expect(deriveCallerParent(undefined, [], "%7")).toBeUndefined();
	});
});

describe("markCompactingSelf (DL-004 — compact-self window mark)", () => {
	const NOW_ISO = "2026-07-18T00:00:00.000Z";
	const me: SessionDescriptor = {
		id: "pij-me",
		folder: "/repo",
		dataDir: "/home/.pij/pij-me",
		eventsPath: "/home/.pij/pij-me/events.ndjson",
		pid: 100,
		startedAt: "2026-07-17T00:00:00.000Z",
	};

	it("stamps compactingAt on the resolved descriptor (merge-law persist)", () => {
		const reg = new FakeRegistry([me]);
		const written = markCompactingSelf(reg, "pij-me", NOW_ISO);
		expect(written?.compactingAt).toBe(NOW_ISO);
		expect(reg.read("pij-me")?.compactingAt).toBe(NOW_ISO);
	});

	it("no id / unknown id → null, nothing written (unregistered pane compacts unchanged)", () => {
		const reg = new FakeRegistry([me]);
		expect(markCompactingSelf(reg, undefined, NOW_ISO)).toBeNull();
		expect(markCompactingSelf(reg, "pij-ghost", NOW_ISO)).toBeNull();
		expect(reg.read("pij-me")?.compactingAt).toBeUndefined();
	});
});
