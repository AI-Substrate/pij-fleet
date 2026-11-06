// Execute only the unchanged production selector, without its launch/router
// branches. This is branch-choice proof, not a daemon sender-attribution test.
"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const root = process.argv[2];
const source = fs.readFileSync(path.join(root, "harness/scripts/pij-cli.cjs"), "utf8");
const start = source.indexOf("const SEND_VALUE_FLAGS");
const end = source.indexOf("\nfunction exitCode", start);
assert(start >= 0 && end > start);
const context = {
	require(name) {
		assert.equal(name, "./pij-native-verbs.json");
		return JSON.parse(fs.readFileSync(path.join(root, "harness/scripts/pij-native-verbs.json"), "utf8"));
	},
};
vm.runInNewContext(`${source.slice(start, end)}\nthis.check = forwardsToPijRs;`, context);
const pane = { TMUX_PANE: "%fixture" };
assert.equal(context.check(["send", "role:probe", "hello"], pane), true);
assert.equal(context.check(["send", "--to", "role:probe", "--body", "hello"], pane), true);
assert.equal(context.check(["send", "pij-peer", "hello"], pane), false);
assert.equal(context.check(["directory"], { PIJ_DAEMON_GENERATION: "legacy" }), false);
console.log("CONTROLS: ordinary role/native-to forwards; seat and legacy remain routed");

for (const args of [
	["send", "--body", "hello", "--to", "role:probe"],
	["send", "--from", "pij-sender", "role:probe", "hello"],
	["send", "--msg-id", "id-147", "role:probe", "hello"],
]) {
	assert.equal(context.check(args, pane), false);
	console.log(`REPRODUCED F001: valid native flag order is not forwarded: ${JSON.stringify(args)}`);
}
assert.equal(context.check(["send", "--msg-id", "role:fake", "pij-peer", "hello"], pane), true);
console.log("REPRODUCED F001: a role-looking --msg-id value misclassifies a seat send");

const native = { COPILOT_AGENT_SESSION_ID: "00000000-0000-4000-8000-000000000147" };
assert.equal(context.check(["send", "role:probe", "hello"], native), false);
assert.equal(context.check(["send", "role:probe", "hello"], {
	...native,
	PIJ_SESSION_ID: "pij-parent-fixture",
}), true);
console.log("REPRODUCED F002: inherited PIJ_SESSION_ID bypasses the kept paneless path");
