"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");

const root = process.argv[2];
const source = fs.readFileSync(path.join(root, "harness/scripts/pij-cli.cjs"), "utf8");
const start = source.indexOf("function splitFlag(");
const end = source.indexOf("\nfunction resolvePijRs(", start);
assert(start >= 0 && end > start);
const context = {};
vm.runInNewContext(`${source.slice(start, end)}\nthis.choose = forwardsToPijRs;`, context);
const native = JSON.parse(fs.readFileSync(path.join(root, "harness/scripts/pij-native-verbs.json"), "utf8"));
const pane = { TMUX_PANE: "%fixture" };
const inherited = {
	PIJ_SESSION_ID: "pij-parent-fixture",
	COPILOT_AGENT_SESSION_ID: "00000000-0000-4000-8000-000000000147",
};
const cases = [
	["role", ["send", "role:probe", "hello"], pane, true],
	["native to", ["send", "--to", "role:probe", "--body", "hello"], pane, true],
	["body first", ["send", "--body", "hello", "--to", "role:probe"], pane, true],
	["from first", ["send", "--from", "pij-sender", "role:probe", "hello"], pane, true],
	["msg-id first", ["send", "--msg-id", "id-147", "role:probe", "hello"], pane, true],
	["opaque msg-id", ["send", "--msg-id", "role:fake", "pij-peer", "hello"], pane, false],
	["seat", ["send", "pij-peer", "hello"], pane, false],
	["inherited role", ["send", "role:probe", "hello"], inherited, false],
	["inherited to", ["send", "--to", "role:probe", "--body", "hello"], inherited, false],
	["json prefix", ["--json", "claims"], {}, true],
	["global values", ["--state-dir", "/state", "--addr=127.0.0.1:9", "claims"], {}, true],
	["json send", ["--json", "send", "role:probe", "hello"], pane, true],
	["global help", ["--addr", "127.0.0.1:9", "--json", "send", "--help"], {}, true],
	["inline help data", ["send", "--to=role:probe", "--body=-h"], pane, true],
	["separate help data paneless", ["send", "--body", "--help", "role:probe"], inherited, false],
	["terminator role", ["send", "--", "role:probe", "--help"], pane, true],
	["terminator paneless", ["send", "--", "role:probe", "--help"], inherited, false],
	["terminator seat", ["send", "--", "pij-peer", "role:fake"], pane, false],
	["terminator literal to", ["send", "--", "pij-peer", "--to"], pane, false],
	["routed file", ["send", "role:probe", "hello", "--file", "/fixture.png"], pane, false],
	["legacy", ["claims"], { PIJ_DAEMON_GENERATION: "legacy" }, false],
	["root help", ["--help"], {}, false],
];
for (const [label, argv, env, expected] of cases) {
	assert.equal(context.choose(argv, env, native), expected, label);
}
console.log(`PASS: ${cases.length} repaired production-selector cases, including all original findings, -- boundaries and opaque help data`);
