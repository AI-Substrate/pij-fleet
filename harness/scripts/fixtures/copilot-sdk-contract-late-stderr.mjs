// R259-001 regression seam, preloaded into the copilot-sdk-contract PARENT by
// copilot-sdk-contract.test.ts only. For every child the parent spawns, each
// stderr event (data, end, close) reaches the parent 50 ms late, in its
// original order, and each data chunk arrives cut in two mid-record. Both are
// legal for a pipe: stdout and stderr are independent, and a chunk boundary is
// not a line boundary. No byte, SDK behavior or JSON-RPC answer changes.
import { ChildProcess } from "node:child_process";

const DELAY_MS = 50;
const spawn = ChildProcess.prototype.spawn;
ChildProcess.prototype.spawn = function (...args) {
	const result = spawn.apply(this, args);
	const stderr = this.stderr;
	if (stderr) {
		const emit = stderr.emit.bind(stderr);
		stderr.emit = (event, ...rest) => {
			if (event !== "data" && event !== "end" && event !== "close") return emit(event, ...rest);
			if (event === "data" && rest[0]?.length > 1) {
				const chunk = Buffer.from(rest[0]);
				const cut = Math.floor(chunk.length / 2);
				setTimeout(() => emit("data", chunk.subarray(0, cut)), DELAY_MS);
				setTimeout(() => emit("data", chunk.subarray(cut)), DELAY_MS);
			} else setTimeout(() => emit(event, ...rest), DELAY_MS);
			return true;
		};
	}
	return result;
};
