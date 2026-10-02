import { describe, expect, it } from "vitest";

import {
	capturePane,
	pasteBuffer,
	pressKey,
	sendFocusIn,
	type TmuxRunner,
	typeLiteral,
} from "./tmux-keys.js";

/** A fake tmux runner that records argv and returns a canned stdout. */
function recorder(stdout = ""): { calls: string[][]; run: TmuxRunner } {
	const calls: string[][] = [];
	const run: TmuxRunner = (args) => {
		calls.push(args);
		return stdout;
	};
	return { calls, run };
}

describe("typeLiteral", () => {
	it("send-keys -l literal, target after -t (matches probe.sh + driver)", () => {
		const r = recorder();
		typeLiteral("%3", "hello world", r.run);
		expect(r.calls).toEqual([["send-keys", "-t", "%3", "-l", "hello world"]]);
	});

	it("passes payload as a single argv slot — never shell-split (TC-01)", () => {
		const r = recorder();
		typeLiteral("%3", "rm -rf / ; echo $(whoami)", r.run);
		expect(r.calls[0]?.[4]).toBe("rm -rf / ; echo $(whoami)");
	});
});

describe("pressKey", () => {
	it("Enter — submits, no -N for a single press (matches probe.sh)", () => {
		const r = recorder();
		pressKey("%3", "Enter", 1, r.run);
		expect(r.calls).toEqual([["send-keys", "-t", "%3", "Enter"]]);
	});

	it("Escape — used to dismiss interstitials", () => {
		const r = recorder();
		pressKey("%3", "Escape", 1, r.run);
		expect(r.calls).toEqual([["send-keys", "-t", "%3", "Escape"]]);
	});

	it("repeats with -N when n > 1", () => {
		const r = recorder();
		pressKey("%3", "BSpace", 4, r.run);
		expect(r.calls).toEqual([["send-keys", "-t", "%3", "-N", "4", "BSpace"]]);
	});
});

describe("sendFocusIn — CSI I focus-in (the backgrounded-copilot wedge fix)", () => {
	it("injects the raw ESC [ I bytes via -H (unblocks a focus-OUT'd copilot's Enter)", () => {
		// If these bytes drift, a backgrounded copilot swallows Enter again → wedge.
		const r = recorder();
		sendFocusIn("%3", r.run);
		expect(r.calls).toEqual([["send-keys", "-t", "%3", "-H", "1b", "5b", "49"]]);
	});
});

describe("pasteBuffer", () => {
	it("set-buffer then paste-buffer -d, plain (no -p)", () => {
		const r = recorder();
		pasteBuffer("%3", "line1\nline2", { bufferName: "buf-x" }, r.run);
		expect(r.calls).toEqual([
			["set-buffer", "-b", "buf-x", "line1\nline2"],
			["paste-buffer", "-d", "-b", "buf-x", "-t", "%3"],
		]);
	});

	it("bracketed insertion adds -p right after paste-buffer (R-04)", () => {
		const r = recorder();
		pasteBuffer("%3", "multi\nline\nbody", { bufferName: "buf-x", bracketed: true }, r.run);
		expect(r.calls[1]).toEqual(["paste-buffer", "-p", "-d", "-b", "buf-x", "-t", "%3"]);
	});
});

describe("capturePane", () => {
	it("capture-pane -p -J by default (matches driver capture)", () => {
		const r = recorder("pane text");
		const out = capturePane("%3", {}, r.run);
		expect(r.calls).toEqual([["capture-pane", "-t", "%3", "-p", "-J"]]);
		expect(out).toBe("pane text");
	});

	it("scrollback adds -S -<N> -E -; ansi adds -e", () => {
		const r = recorder();
		capturePane("%3", { scrollback: 2000, ansi: true }, r.run);
		expect(r.calls[0]).toEqual([
			"capture-pane",
			"-t",
			"%3",
			"-p",
			"-J",
			"-e",
			"-S",
			"-2000",
			"-E",
			"-",
		]);
	});

	it("join:false omits -J", () => {
		const r = recorder();
		capturePane("%3", { join: false }, r.run);
		expect(r.calls[0]).toEqual(["capture-pane", "-t", "%3", "-p"]);
	});
});
