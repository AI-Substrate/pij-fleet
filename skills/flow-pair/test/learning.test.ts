// skills/flow-pair/test/learning.test.ts
// Phase 7: Prompt-learning notes + cluster lifecycle.
// T001/T002/T004: AC-07 cluster isolation and P9 ledger-before-candidate guard.

import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	Learning,
	type LearningDeps,
	nodeLearningDeps,
	PROMPT_CLUSTERS,
	type PromptCluster,
	type RecordLearningOpts,
} from "../lib/learning.js";

const RUN_ID = "2026-06-23T00-00-00Z-learning-t007";
const DELEGATION_ID = "dlg-0001";

interface SnapshotEntry {
	kind: "dir" | "file";
	value: string;
}

type TreeSnapshot = Map<string, SnapshotEntry>;

function scaffoldRunDir(ledgerRoot: string, runId: string): string {
	const runDir = join(ledgerRoot, "runs", runId);
	for (const sub of [
		"",
		"reviews",
		"fix-packets",
		"delegations",
		"prompt-trials",
		"learnings",
		"prompts",
		"worker-reports",
		"diffs",
	]) {
		mkdirSync(join(runDir, sub), { recursive: true });
	}
	writeFileSync(
		join(runDir, "run.json"),
		JSON.stringify({ runId, repoId: "test", status: "open", createdAt: new Date().toISOString() }),
	);
	writeFileSync(join(runDir, "events.jsonl"), "");
	return runDir;
}

function makePromptLabFixture(promptLabRoot: string): void {
	for (const cluster of PROMPT_CLUSTERS) {
		const clusterDir = join(promptLabRoot, "clusters", cluster);
		mkdirSync(join(clusterDir, "candidates"), { recursive: true });
		writeFileSync(
			join(clusterDir, "active.md"),
			`# ${cluster} active\n\nCurrent prompt guidance.\n`,
		);
		writeFileSync(join(clusterDir, "candidates", ".gitkeep"), "");
		writeFileSync(
			join(clusterDir, "changelog.md"),
			`# ${cluster} changelog\n\nNo promotions yet.\n`,
		);
	}
}

function snapshotDir(root: string, rel = ""): TreeSnapshot {
	const snapshot: TreeSnapshot = new Map();
	const abs = rel === "" ? root : join(root, rel);
	const entries = readdirSync(abs, { withFileTypes: true }).sort((a, b) =>
		a.name.localeCompare(b.name),
	);
	snapshot.set(rel, { kind: "dir", value: entries.map((entry) => entry.name).join("\n") });
	for (const entry of entries) {
		const childRel = rel === "" ? entry.name : join(rel, entry.name);
		const childAbs = join(root, childRel);
		if (entry.isDirectory()) {
			for (const [key, value] of snapshotDir(root, childRel)) {
				snapshot.set(key, value);
			}
		} else {
			snapshot.set(childRel, { kind: "file", value: readFileSync(childAbs, "utf8") });
		}
	}
	return snapshot;
}

function expectSnapshotsEqual(actual: TreeSnapshot, expected: TreeSnapshot): void {
	expect([...actual.keys()].sort()).toEqual([...expected.keys()].sort());
	for (const [key, value] of expected) {
		expect(actual.get(key)).toEqual(value);
	}
}

function clusterSnapshot(promptLabRoot: string, cluster: PromptCluster): TreeSnapshot {
	return snapshotDir(join(promptLabRoot, "clusters", cluster));
}

function candidateMdFiles(promptLabRoot: string, cluster: PromptCluster): string[] {
	return readdirSync(join(promptLabRoot, "clusters", cluster, "candidates"))
		.filter((name) => name.endsWith(".md"))
		.sort();
}

function eventsContent(ledgerRoot: string): string {
	return readFileSync(join(ledgerRoot, "runs", RUN_ID, "events.jsonl"), "utf8");
}

function learningJsonFiles(ledgerRoot: string): string[] {
	return readdirSync(join(ledgerRoot, "runs", RUN_ID, "learnings"))
		.filter((name) => name.endsWith(".json"))
		.sort();
}

function validOpts(
	promptLabRoot: string,
	overrides: Partial<RecordLearningOpts> = {},
): RecordLearningOpts {
	return {
		runId: RUN_ID,
		delegationId: DELEGATION_ID,
		cluster: "implement-code",
		missType: "implement-code",
		summary: "Worker implemented without updating the validation contract.",
		evidence: ["review found missing CLI coverage", "mutation gate caught the gap"],
		candidateDelta: "When changing CLI behavior, add a subprocess test for the stdout contract.",
		promptLabRoot,
		...overrides,
	};
}

class TrackingDeps implements LearningDeps {
	callLog: Array<{ method: "appendFileSync" | "writeFileSync"; path: string }> = [];

	constructor(private readonly real: LearningDeps) {}

	mkdirSync(path: string, opts?: { recursive?: boolean }): void {
		this.real.mkdirSync(path, opts);
	}
	writeFileSync(path: string, data: string): void {
		this.callLog.push({ method: "writeFileSync", path });
		this.real.writeFileSync(path, data);
	}
	appendFileSync(path: string, data: string): void {
		this.callLog.push({ method: "appendFileSync", path });
		this.real.appendFileSync(path, data);
	}
	readFileSync(path: string, enc: "utf8"): string {
		return this.real.readFileSync(path, enc);
	}
	existsSync(path: string): boolean {
		return this.real.existsSync(path);
	}
	readdirSync(path: string): string[] {
		return this.real.readdirSync(path);
	}
}

class FailAppendDeps implements LearningDeps {
	candidateWriteWasCalled = false;
	learningRecordWriteWasCalled = false;

	constructor(private readonly real: LearningDeps) {}

	mkdirSync(path: string, opts?: { recursive?: boolean }): void {
		this.real.mkdirSync(path, opts);
	}
	writeFileSync(path: string, data: string): void {
		if (path.includes(`${join("clusters", "implement-code", "candidates")}`)) {
			this.candidateWriteWasCalled = true;
		}
		if (path.includes(`${join("learnings", "learn-")}`)) {
			this.learningRecordWriteWasCalled = true;
		}
		this.real.writeFileSync(path, data);
	}
	appendFileSync(_path: string, _data: string): void {
		throw new Error("injected appendFileSync failure");
	}
	readFileSync(path: string, enc: "utf8"): string {
		return this.real.readFileSync(path, enc);
	}
	existsSync(path: string): boolean {
		return this.real.existsSync(path);
	}
	readdirSync(path: string): string[] {
		return this.real.readdirSync(path);
	}
}

describe("Learning.recordLearning — AC-07 cluster isolation", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let promptLabRoot: string;
	let runDir: string;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "learning-ac07-"));
		ledgerRoot = join(tmpRoot, "ledger");
		promptLabRoot = join(tmpRoot, "prompt-lab");
		runDir = scaffoldRunDir(ledgerRoot, RUN_ID);
		makePromptLabFixture(promptLabRoot);
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("implement-code miss writes exactly one implement-code candidate and leaves siblings byte-identical", () => {
		const beforeByCluster = new Map<PromptCluster, TreeSnapshot>();
		for (const cluster of PROMPT_CLUSTERS) {
			beforeByCluster.set(cluster, clusterSnapshot(promptLabRoot, cluster));
		}

		const learning = new Learning(ledgerRoot, nodeLearningDeps());
		const result = learning.recordLearning(validOpts(promptLabRoot));

		expect(result.ok).toBe(true);
		if (!result.ok) throw new Error(result.error);
		expect(result.candidate.learningId).toBe("learn-0001");
		expect(result.candidate.cluster).toBe("implement-code");
		expect(result.candidate.candidatePath).toBe(
			join(promptLabRoot, "clusters", "implement-code", "candidates", "learn-0001.md"),
		);

		expect(candidateMdFiles(promptLabRoot, "implement-code")).toEqual(["learn-0001.md"]);
		const candidateBody = readFileSync(result.candidate.candidatePath, "utf8");
		expect(candidateBody).toContain("# Learning Candidate — learn-0001");
		expect(candidateBody).toContain("Pending manual review");
		expect(candidateBody).not.toContain("fix-code");

		for (const cluster of PROMPT_CLUSTERS) {
			const before = beforeByCluster.get(cluster);
			if (before === undefined) throw new Error(`missing snapshot for ${cluster}`);
			if (cluster === "implement-code") {
				expect(readFileSync(join(promptLabRoot, "clusters", cluster, "active.md"), "utf8")).toBe(
					before.get("active.md")?.value,
				);
				expect(readFileSync(join(promptLabRoot, "clusters", cluster, "changelog.md"), "utf8")).toBe(
					before.get("changelog.md")?.value,
				);
			} else {
				expectSnapshotsEqual(clusterSnapshot(promptLabRoot, cluster), before);
				expect(candidateMdFiles(promptLabRoot, cluster)).toEqual([]);
			}
		}

		const rec = JSON.parse(readFileSync(join(runDir, "learnings", "learn-0001.json"), "utf8")) as {
			learningId: string;
			cluster: string;
			candidatePath: string;
		};
		expect(rec.learningId).toBe("learn-0001");
		expect(rec.cluster).toBe("implement-code");
		expect(rec.candidatePath).toBe(result.candidate.candidatePath);
	});

	it("mismatched missType/cluster reaches the isolation guard and writes nothing", () => {
		const beforePromptLab = snapshotDir(promptLabRoot);
		const beforeEvents = eventsContent(ledgerRoot);
		const beforeLearnings = learningJsonFiles(ledgerRoot);
		const learning = new Learning(ledgerRoot, nodeLearningDeps());

		const result = learning.recordLearning(
			validOpts(promptLabRoot, { cluster: "fix-code", missType: "implement-code" }),
		);

		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toMatch(/missType/);
		expectSnapshotsEqual(snapshotDir(promptLabRoot), beforePromptLab);
		expect(eventsContent(ledgerRoot)).toBe(beforeEvents);
		expect(learningJsonFiles(ledgerRoot)).toEqual(beforeLearnings);
	});

	it.each([
		"",
		"../fix-code",
		"/abs",
		"implement-code/../fix-code",
	])("invalid/traversal cluster %j returns {ok:false} with zero writes", (clusterValue) => {
		const beforePromptLab = snapshotDir(promptLabRoot);
		const beforeEvents = eventsContent(ledgerRoot);
		const beforeLearnings = learningJsonFiles(ledgerRoot);
		const learning = new Learning(ledgerRoot, nodeLearningDeps());

		const result = learning.recordLearning(
			validOpts(promptLabRoot, {
				cluster: clusterValue as RecordLearningOpts["cluster"],
				missType: "implement-code",
			}),
		);

		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.error).toMatch(/cluster/);
		expectSnapshotsEqual(snapshotDir(promptLabRoot), beforePromptLab);
		expect(eventsContent(ledgerRoot)).toBe(beforeEvents);
		expect(learningJsonFiles(ledgerRoot)).toEqual(beforeLearnings);
	});
});

describe("Learning.recordLearning — P9 ledger before candidate", () => {
	let tmpRoot: string;
	let ledgerRoot: string;
	let promptLabRoot: string;

	beforeEach(() => {
		tmpRoot = mkdtempSync(join(tmpdir(), "learning-p9-"));
		ledgerRoot = join(tmpRoot, "ledger");
		promptLabRoot = join(tmpRoot, "prompt-lab");
		scaffoldRunDir(ledgerRoot, RUN_ID);
		makePromptLabFixture(promptLabRoot);
	});

	afterEach(() => {
		rmSync(tmpRoot, { recursive: true, force: true, maxRetries: 10, retryDelay: 50 });
	});

	it("appends learning event and writes ledger record before prompt-lab candidate", () => {
		const tracking = new TrackingDeps(nodeLearningDeps());
		const learning = new Learning(ledgerRoot, tracking);
		const result = learning.recordLearning(validOpts(promptLabRoot));

		expect(result.ok).toBe(true);
		const appendIdx = tracking.callLog.findIndex(
			(e) => e.method === "appendFileSync" && e.path.endsWith("events.jsonl"),
		);
		const ledgerWriteIdx = tracking.callLog.findIndex(
			(e) => e.method === "writeFileSync" && e.path.includes(join("learnings", "learn-0001.json")),
		);
		const candidateWriteIdx = tracking.callLog.findIndex(
			(e) =>
				e.method === "writeFileSync" &&
				e.path.includes(join("clusters", "implement-code", "candidates", "learn-0001.md")),
		);
		expect(appendIdx).toBeGreaterThanOrEqual(0);
		expect(ledgerWriteIdx).toBeGreaterThan(appendIdx);
		expect(candidateWriteIdx).toBeGreaterThan(ledgerWriteIdx);
	});

	it("ledger append failure returns {ok:false} and creates no candidate", () => {
		const failDeps = new FailAppendDeps(nodeLearningDeps());
		const learning = new Learning(ledgerRoot, failDeps);
		const result = learning.recordLearning(validOpts(promptLabRoot));

		expect(result.ok).toBe(false);
		expect(failDeps.learningRecordWriteWasCalled).toBe(false);
		// P9 load-bearing: mutation s/if (!ledgerResult.ok)/if (false)/ writes this candidate.
		expect(failDeps.candidateWriteWasCalled).toBe(false);
		expect(candidateMdFiles(promptLabRoot, "implement-code")).toEqual([]);
	});
});

describe("cluster-set single-source (Phase 7 MED guardrail)", () => {
	// P5: the cluster set must not drift between code (PROMPT_CLUSTERS) and the
	// declared taxonomy source (prompt-taxonomy.md), nor the committed scaffold
	// dirs. A future code-only cluster addition would otherwise leave the taxonomy
	// stale and tests building fixtures from the code constant silently.
	const taxonomyPath = join(__dirname, "..", "references", "prompt-taxonomy.md");
	const clustersRoot = join(__dirname, "..", "prompt-lab", "clusters");

	function parseTaxonomyClusters(): string[] {
		const md = readFileSync(taxonomyPath, "utf8");
		// Capture the `## Canonical clusters` table's first column (`slug` in backticks).
		const section = md.split(/^## /m).find((s) => s.startsWith("Canonical clusters"));
		expect(section).toBeDefined();
		const clusters: string[] = [];
		if (section === undefined) throw new Error("missing Canonical clusters section");
		for (const line of section.split("\n")) {
			const m = line.match(/^\|\s*`([a-z-]+)`/);
			if (m) clusters.push(m[1]);
		}
		return clusters;
	}

	it("PROMPT_CLUSTERS matches the taxonomy Canonical clusters table (order-insensitive)", () => {
		const fromTaxonomy = parseTaxonomyClusters();
		expect(fromTaxonomy.sort()).toEqual([...PROMPT_CLUSTERS].sort());
	});

	it("committed prompt-lab scaffold dirs are exactly PROMPT_CLUSTERS", () => {
		const dirs = readdirSync(clustersRoot, { withFileTypes: true })
			.filter((e) => e.isDirectory())
			.map((e) => e.name)
			.sort();
		expect(dirs).toEqual([...PROMPT_CLUSTERS].sort());
	});

	it("learning.schema.json cluster enum is exactly PROMPT_CLUSTERS (Phase 7 LOW)", () => {
		// The runtime isolation layer rejects invalid clusters, but the persisted
		// ledger schema must ALSO constrain the enum so a hand-edited or future-written
		// record can't carry an invalid cluster past schema validation. Tie the enum to
		// PROMPT_CLUSTERS so it can't drift from the code constant.
		const schemaPath = join(__dirname, "..", "schemas", "learning.schema.json");
		const schema = JSON.parse(readFileSync(schemaPath, "utf8")) as {
			properties: { cluster: { enum?: string[] } };
		};
		const enumValues = schema.properties.cluster.enum;
		expect(enumValues).toBeDefined();
		expect(enumValues?.slice().sort()).toEqual([...PROMPT_CLUSTERS].sort());
	});
});
