import assert from "node:assert/strict";
import test from "node:test";
import * as store from "./store.mjs";
import { resolveNativeHost } from "./store.mjs";

const processes = new Map([
	[300, { pid: 300, ppid: 200, command: "/runtime/extension-loader", proc_start: 20260905120002 }],
	[
		200,
		{ pid: 200, ppid: 100, command: "/Users/test/.local/bin/copilot", proc_start: 20260905120001 },
	],
	[100, { pid: 100, ppid: 1, command: "/bin/zsh", proc_start: 20260905120000 }],
]);
const inspectProcess = async (pid) => processes.get(pid);

test("host is verified Copilot ancestor, not extension loader or pane shell", async () => {
	assert.deepEqual(
		await resolveNativeHost({ parentPid: 300, inspectProcess, pane: "%137", paneProcess: 100 }),
		{ pid: 200, proc_start: 20260905120001, pane: "%137" },
	);
});

test("paneless native host remains truthful without inventing a pane", async () => {
	assert.deepEqual(await resolveNativeHost({ parentPid: 300, inspectProcess }), {
		pid: 200,
		proc_start: 20260905120001,
	});
});

test("a pane not in actual ancestry refuses instead of hiding pane metadata", async () => {
	await assert.rejects(
		resolveNativeHost({ parentPid: 300, inspectProcess, pane: "%other", paneProcess: 100 }),
		/pane/i,
	);
	await assert.rejects(
		resolveNativeHost({ parentPid: 300, inspectProcess, pane: "%138", paneProcess: 999 }),
		/exact pane/i,
	);
});

test("absence of native ancestor is explicit unsupported identity", async () => {
	await assert.rejects(
		resolveNativeHost({ parentPid: 100, inspectProcess }),
		/Copilot host ancestor/i,
	);
});

test("PID reuse during observation refuses stale host incarnation", async () => {
	let hostReads = 0;
	await assert.rejects(
		resolveNativeHost({
			parentPid: 300,
			inspectProcess: async (pid) => {
				const row = processes.get(pid);
				return pid === 200 && ++hostReads > 1 ? { ...row, proc_start: row.proc_start + 1 } : row;
			},
		}),
		/incarnation changed/i,
	);
});

test("kernel executable normalization strips only the exact trailing deletion marker", () => {
	for (const [raw, command, replaced] of [
		["/opt/copilot (deleted)", "/opt/copilot", true],
		["/opt/copilot", "/opt/copilot", false],
		["/opt/copilot (deleted) ", "/opt/copilot (deleted) ", false],
		["(deleted)", "(deleted)", false],
		["C:\\Tools\\copilot.exe", "C:\\Tools\\copilot.exe", false],
		["/opt/copilot.exe", "/opt/copilot.exe", false],
	]) {
		assert.deepEqual(store.normalizeExecutable(raw), { command, replaced });
	}
});

test("replaced Copilot host resolves even when the kernel marker changes between observations", async () => {
	let hostReads = 0;
	const host = await resolveNativeHost({
		parentPid: 300,
		pane: "%137",
		paneProcess: 100,
		inspectProcess: async (pid) => {
			const row = processes.get(pid);
			const raw =
				pid === 200 ? `/opt/copilot${++hostReads === 1 ? " (deleted)" : ""}` : row.command;
			return { ...row, ...store.normalizeExecutable(raw) };
		},
	});
	assert.deepEqual(host, { pid: 200, proc_start: 20260905120001, pane: "%137" });
});

test("a replaced executable with a lookalike basename still refuses", async () => {
	await assert.rejects(
		resolveNativeHost({
			parentPid: 300,
			inspectProcess: async (pid) => {
				const row = processes.get(pid);
				return {
					...row,
					...store.normalizeExecutable(pid === 200 ? "/opt/copilot-evil (deleted)" : row.command),
				};
			},
		}),
		/Copilot host ancestor/i,
	);
});

for (const field of ["ppid", "proc_start"]) {
	test(`a replaced host with a changed ${field} still refuses incarnation reuse`, async () => {
		let hostReads = 0;
		await assert.rejects(
			resolveNativeHost({
				parentPid: 300,
				inspectProcess: async (pid) => {
					const row = processes.get(pid);
					return {
						...row,
						...(pid === 200 && ++hostReads > 1 ? { [field]: row[field] + 1 } : {}),
						...store.normalizeExecutable(pid === 200 ? "/opt/copilot (deleted)" : row.command),
					};
				},
			}),
			/incarnation changed/i,
		);
	});
}
