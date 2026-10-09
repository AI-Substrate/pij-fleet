#!/usr/bin/env tsx
// npm run smoke -- [name] — runs each .omp/extensions/<name>/smoke.ts via the Driver SDK.

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
	mkdirSync,
	readdirSync,
	readFileSync,
	realpathSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { exerciseExternalRoundtrip } from "../driver/governance-external.js";
import { type CommandResult, PrivateGovernanceFixture } from "../driver/governance-fixture.js";
import { loadScenario, runScenario, type Scenario } from "../driver/index.js";

const PIJ_ROOT = join(import.meta.dirname, "..", "..");
const EXTENSIONS_ROOT = join(PIJ_ROOT, ".omp", "extensions");

function findTopLevelFiles(root: string, filename: string, filter?: string): string[] {
	const found: string[] = [];
	let entries: string[];
	try {
		entries = readdirSync(root);
	} catch {
		return found; // an absent extensions dir is fine
	}
	for (const entry of entries) {
		if (filter && entry !== filter) continue;
		const file = join(root, entry, filename);
		try {
			if (statSync(file).isFile()) found.push(file);
		} catch {
			/* none */
		}
	}
	return found.sort();
}

function findScenarios(filter?: string): string[] {
	return findTopLevelFiles(EXTENSIONS_ROOT, "smoke.ts", filter);
}

export function findProjectExtensionEntries(root: string): string[] {
	return findTopLevelFiles(root, "index.ts");
}

function quoteShellArg(value: string): string {
	return `'${value.replaceAll("'", "'\"'\"'")}'`;
}

export function resolveSmokeCommand(
	scenario: Pick<Scenario, "cmd">,
	extensionEntries: readonly string[],
): string {
	if (scenario.cmd !== undefined) return scenario.cmd;
	const extensions = [...extensionEntries]
		.sort()
		.map((extension) => ` --extension ${quoteShellArg(extension)}`)
		.join("");
	return `omp --auto-approve --no-extensions${extensions}`;
}

interface SmokeResult {
	readonly verdict: "PASS" | "FAIL";
	readonly reason?: string;
}

function object(value: unknown): Record<string, unknown> {
	assert(value !== null && typeof value === "object" && !Array.isArray(value));
	return value as Record<string, unknown>;
}

function text(value: unknown): string {
	assert(typeof value === "string" && value.length > 0);
	return value;
}

function sequence(value: unknown): number {
	assert(typeof value === "number" && Number.isSafeInteger(value) && value >= 0);
	return value;
}

/** Keep the entire wire response: never turn a data projection into an envelope. */
export function parseSmokeEnvelope(
	result: CommandResult,
	expectedOk = true,
	refusalCode?: string,
): Record<string, unknown> {
	const envelope = object(JSON.parse(result.stdout));
	assert.equal(envelope.v, 2, `wire version: ${result.stdout}`);
	text(envelope.command);
	assert.equal(envelope.ok, expectedOk, `response: ${result.stdout}\n${result.stderr}`);
	assert.notEqual(result.status, null, `command terminated by signal: ${result.stderr}`);
	if (expectedOk) assert.equal(result.status, 0, result.stderr);
	else assert.notEqual(result.status, 0, "a refusal must exit unsuccessfully");
	if (refusalCode !== undefined) assert.equal(object(envelope.details).code, refusalCode);
	return envelope;
}

function smokeFailure(error: unknown): string {
	if (error instanceof AggregateError) {
		return [error.message, ...error.errors.map(smokeFailure)].join("\n");
	}
	if (error instanceof Error) {
		return [
			error.stack ?? error.message,
			...(error.cause === undefined ? [] : [smokeFailure(error.cause)]),
		].join("\n");
	}
	return String(error);
}

async function privateSmoke(
	exercise: (fixture: PrivateGovernanceFixture) => Promise<void>,
): Promise<SmokeResult> {
	let fixture: PrivateGovernanceFixture | undefined;
	const failures: unknown[] = [];
	try {
		fixture = await PrivateGovernanceFixture.open(PIJ_ROOT);
		await exercise(fixture);
	} catch (error) {
		failures.push(error);
	} finally {
		try {
			await fixture?.close();
		} catch (error) {
			failures.push(error);
		}
	}
	return failures.length === 0
		? { verdict: "PASS" }
		: {
				verdict: "FAIL",
				reason: failures.map(smokeFailure).join("\n"),
			};
}

export async function runTeamScaffoldSmoke(): Promise<SmokeResult> {
	return privateSmoke(async (fixture) => {
		const parent = object(
			parseSmokeEnvelope(
				await fixture.native(["adopt", fixture.parentPane, "--harness", "omp", "--role", "prime"]),
			).data,
		);
		const parentId = text(parent.id);
		const workerEnv = { ...fixture.env, TMUX_PANE: fixture.workerPane };
		const worker = object(
			parseSmokeEnvelope(
				await fixture.native(
					[
						"adopt",
						fixture.workerPane,
						"--harness",
						"omp",
						"--parent",
						parentId,
						"--role",
						"worker",
					],
					workerEnv,
				),
			).data,
		);
		const workerId = text(worker.id);
		assert.equal(worker.parent, parentId);
		assert.equal(worker.pane, fixture.workerPane);
		assert(sequence(object(worker.proc).proc_start) > 0);
		assert.equal(worker.session, null, "inert pane is not a model session");

		for (const args of [
			["init", "-b", "main", fixture.repo],
			[
				"-c",
				"user.name=Private smoke fixture",
				"-c",
				"user.email=fixture@example.invalid",
				"commit",
				"--allow-empty",
				"-m",
				"private smoke",
			],
		]) {
			const result = await fixture.run("git", args);
			assert.equal(result.status, 0, `git: ${result.stderr}`);
		}
		const projectReply = parseSmokeEnvelope(
			await fixture.shim(["project", "create", "Private team smoke", "--repo", fixture.repo]),
		);
		const projectData = object(projectReply.data);
		const project = text(object(projectData.project).slug);
		const projectSeq = sequence(projectData.seq);
		const worktrees = join(fixture.root, "worktrees");
		mkdirSync(worktrees);
		const streamData = object(
			parseSmokeEnvelope(
				await fixture.shim([
					"stream",
					"create",
					"--project",
					project,
					"--slug",
					"workflow",
					"--base",
					"main",
					"--root",
					worktrees,
				]),
			).data,
		);
		const stream = object(streamData.stream);
		const streamId = text(stream.id);
		assert.equal(stream.project, project);
		assert.equal(stream.state, "created");
		assert(sequence(stream.ordinal) > 0);
		const worktree = realpathSync(text(stream.worktree));
		assert(worktree.startsWith(`${realpathSync(worktrees)}/`), "worktree escaped private root");
		assert(statSync(join(worktree, ".git")).isFile(), "real linked worktree");
		const branch = await fixture.run("git", ["-C", worktree, "branch", "--show-current"]);
		assert.equal(branch.status, 0, branch.stderr);
		assert.equal(branch.stdout.trim(), stream.branch);

		const fence = object(
			object(
				parseSmokeEnvelope(await fixture.shim(["fence", "set", streamId, "--paths", "src/api/**"]))
					.data,
			).fence,
		);
		const shownFence = object(
			parseSmokeEnvelope(await fixture.shim(["fence", "show", "--stream", streamId])).data,
		);
		assert(Array.isArray(shownFence.fences));
		assert.deepEqual(shownFence.fences, [fence]);
		assert.equal(fence.stream, streamId);
		assert.deepEqual(fence.paths, ["src/api/**"]);

		const packet = join(fixture.root, "packet.txt");
		writeFileSync(packet, "Read these exact private fixture bytes. No live model is running.\n");
		const dispatched = object(
			parseSmokeEnvelope(await fixture.shim(["dispatch", workerId, "--packet", packet])).data,
		);
		const dispatch = object(dispatched.dispatch);
		const dispatchId = text(dispatch.id);
		const claims = parseSmokeEnvelope(await fixture.shim(["inbox"], workerEnv)).data;
		assert(Array.isArray(claims));
		const messages = claims.map((claim) => object(object(claim).message));
		const packetMessage = object(messages.find((message) => message.msg_id === dispatch.msg_id));
		assert.equal(packetMessage.from, parentId);
		assert.equal(packetMessage.to, workerId);
		const packetSha = createHash("sha256").update(readFileSync(packet)).digest("hex");
		assert.equal(dispatch.packet_sha256, packetSha);
		assert(text(packetMessage.body).includes(packet));
		assert(text(packetMessage.body).includes(packetSha));
		assert.deepEqual(parseSmokeEnvelope(await fixture.shim(["inbox"], workerEnv)).data, []);
		const ack = object(
			parseSmokeEnvelope(
				await fixture.shim(["ack", dispatchId, "--packet-sha", packetSha], workerEnv),
			).data,
		);
		const acknowledged = object(ack.dispatch);
		assert.equal(acknowledged.state, "acked");
		assert.equal(object(acknowledged.ack).seat, workerId);
		assert.equal(object(acknowledged.ack).packet_sha256, packetSha);
		assert(sequence(ack.seq) > sequence(dispatched.seq));

		const taskData = object(
			parseSmokeEnvelope(
				await fixture.shim([
					"task",
					"set",
					workerId,
					"Exercise shipped governance",
					"--project",
					project,
				]),
			).data,
		);
		const task = object(taskData.task);
		assert.equal(task.node_id, workerId);
		assert.equal(task.project, project);
		const node = object(
			object(parseSmokeEnvelope(await fixture.shim(["node", "show", workerId])).data).node,
		);
		assert(Array.isArray(node.assignments));
		assert(node.assignments.some((assignment) => object(assignment).id === task.id));
		assert(Array.isArray(node.dispatches));
		assert(node.dispatches.some((record) => object(record).id === dispatchId));

		const canary = parseSmokeEnvelope(
			await fixture.shim(["canary", workerId, "--wait=20"]),
			false,
			"E-RS-CANARY-PENDING",
		);
		const pendingCanary = object(canary.details);
		text(pendingCanary.nonce);
		const challengeId = text(pendingCanary.dispatch);
		// Inert hosts cannot answer a model challenge. Never inject a positive ACK/footer.
		const challenged = object(
			object(parseSmokeEnvelope(await fixture.shim(["node", "show", workerId])).data).node,
		);
		assert(Array.isArray(challenged.dispatches));
		const challenge = object(
			challenged.dispatches.find((record) => object(record).id === challengeId),
		);
		assert(!Object.hasOwn(challenge, "canary"), "inert host has no proven canary");
		assert.notEqual(challenge.state, "acked");
		const closedData = object(
			parseSmokeEnvelope(await fixture.shim(["stream", "close", streamId])).data,
		);
		assert.equal(object(closedData.stream).state, "closed");
		const spine = object(
			parseSmokeEnvelope(await fixture.shim(["spine", "events", "--since", "0"])).data,
		);
		assert(Array.isArray(spine.events));
		for (const expected of [
			projectSeq,
			sequence(streamData.seq),
			sequence(dispatched.seq),
			sequence(ack.seq),
			sequence(taskData.seq),
			sequence(closedData.seq),
		]) {
			assert(
				spine.events.some((event) => object(event).seq === expected),
				`spine omitted actual effect ${expected}`,
			);
		}
		assert(sequence(spine.cursor) >= sequence(closedData.seq));
		await exerciseExternalRoundtrip(fixture, parseSmokeEnvelope);
		console.error(
			JSON.stringify({
				smoke: "private-team",
				parent: parentId,
				worker: workerId,
				project,
				stream: streamId,
				dispatch: dispatchId,
				packetSha,
				cursor: spine.cursor,
				modelCanary: "unproven-inert-hosts",
			}),
		);
	});
}

function privateHomeSnapshot(root: string): Record<string, string> {
	const snapshot: Record<string, string> = {};
	const visit = (directory: string): void => {
		for (const entry of readdirSync(directory, { withFileTypes: true })) {
			const path = join(directory, entry.name);
			if (entry.isDirectory()) {
				snapshot[`${path.slice(root.length)}/`] = "directory";
				visit(path);
			} else {
				assert(entry.isFile(), `unexpected private-home entry: ${path}`);
				snapshot[path.slice(root.length)] = createHash("sha256")
					.update(readFileSync(path))
					.digest("hex");
			}
		}
	};
	visit(root);
	return snapshot;
}

export async function runWatchdogSmoke(): Promise<SmokeResult> {
	return privateSmoke(async (fixture) => {
		parseSmokeEnvelope(await fixture.native(["adopt", fixture.parentPane, "--harness", "omp"]));
		const before = object(
			parseSmokeEnvelope(await fixture.native(["spine", "events", "--since", "0"])).data,
		);
		const home = join(fixture.root, "home");
		const files = privateHomeSnapshot(home);
		// Refusals first: forced legacy routing is retired, and rs refuses the
		// old TS administrative leaves. Neither may write anything.
		const legacy = parseSmokeEnvelope(
			await fixture.shim(["watchdog", "on"], { ...fixture.env, PIJ_DAEMON_GENERATION: "legacy" }),
			false,
			"E-RS-UNPORTED",
		);
		assert.equal(object(legacy.details).verb, "watchdog");
		parseSmokeEnvelope(await fixture.shim(["watchdog", "disable-all"]), false, "E-RS-ARG");
		const after = object(
			parseSmokeEnvelope(
				await fixture.native(["spine", "events", "--since", String(sequence(before.cursor))]),
			).data,
		);
		assert.equal(after.cursor, before.cursor, "a refused watchdog call must not append");
		assert.deepEqual(privateHomeSnapshot(home), files, "no legacy file authority was mutated");

		// The wire, end to end through the shim: on → status → off for the caller.
		const on = object(
			parseSmokeEnvelope(await fixture.shim(["watchdog", "on", "--every", "30m"])).data,
		);
		assert.equal(on.enabled, true);
		assert.equal(on.interval_secs, 1_800);
		const status = object(parseSmokeEnvelope(await fixture.shim(["watchdog", "status"])).data);
		assert.equal((status.optins as unknown[]).length, 1, "status lists the opted-in seat");
		const off = object(parseSmokeEnvelope(await fixture.shim(["watchdog", "off"])).data);
		assert.equal(off.was_on, true);
		console.error(
			JSON.stringify({
				smoke: "watchdog",
				forcedLegacy: "refused",
				oldLeaf: "E-RS-ARG",
				roundTrip: "on→status→off",
			}),
		);
	});
}

async function main(): Promise<void> {
	const filter = process.argv[2];
	const files = findScenarios(filter);
	const includeWatchdog = filter === undefined || filter === "pij" || filter === "watchdog";
	const includeTeamScaffold =
		filter === undefined || filter === "pij" || filter === "team-scaffold";
	if (files.length === 0 && !includeWatchdog && !includeTeamScaffold) {
		console.log(filter ? `no smoke.ts in ${filter}` : "no smoke scenarios");
		process.exit(0);
	}
	const extensionEntries = findProjectExtensionEntries(EXTENSIONS_ROOT);
	let failed = 0;
	if (includeTeamScaffold) {
		process.stdout.write("smoke: pij-team-scaffold ... ");
		const teamScaffold = await runTeamScaffoldSmoke();
		if (teamScaffold.verdict === "PASS") console.log("✓");
		else {
			failed++;
			console.log("✗");
			console.error(teamScaffold.reason ?? "team-scaffold smoke failed");
		}
	}
	if (includeWatchdog) {
		process.stdout.write("smoke: pij-watchdog ... ");
		const watchdog = await runWatchdogSmoke();
		if (watchdog.verdict === "PASS") console.log("✓");
		else {
			failed++;
			console.log("✗");
			console.error(watchdog.reason ?? "watchdog smoke failed");
		}
	}
	for (const file of files) {
		const scenario = await loadScenario(file);
		process.stdout.write(`smoke: ${scenario.name} ... `);
		const report = await runScenario(scenario, {
			cwd: PIJ_ROOT,
			cmd: resolveSmokeCommand(scenario, extensionEntries),
		});
		if (report.ok) console.log("✓");
		else {
			failed++;
			console.log("✗");
			console.error(JSON.stringify(report.failure, null, 2));
		}
	}
	process.exit(failed > 0 ? 1 : 0);
}

const isMainModule =
	process.argv[1] !== undefined && resolve(process.argv[1]) === import.meta.filename;
if (isMainModule) {
	main().catch((err: Error) => {
		console.error(err.message);
		process.exit(2);
	});
}
