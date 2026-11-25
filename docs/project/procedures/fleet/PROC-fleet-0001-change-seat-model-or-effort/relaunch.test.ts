import { spawnSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const SCRIPT = join(dirname(fileURLToPath(import.meta.url)), "relaunch.sh");
const SECRET = "sk-do-not-leak";
const PS_COMMAND =
	"/opt/bin/copilot --model claude-opus-5.5 --context long_context --effort medium --resume=old --allow-all-tools *";
const PS_ENV = [
	"PIJ_SESSION_ID=pij-x",
	"COPILOT_PROVIDER_TYPE=anthropic",
	"COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:9/v1",
	"COPILOT_PROVIDER_MODEL_ID=claude-opus-5.5",
	"COPILOT_PROVIDER_MAX_PROMPT_TOKENS=1000000",
	`COPILOT_PROVIDER_API_KEY=${SECRET}`,
	`PIJ_BG_TOKEN=${SECRET}`,
	"PIJ_SPAWN_ID=spawn-1",
	"PIJ_QUOTED=it's;$(touch${IFS}pwned)",
	"HOME=/elsewhere",
].join(" ");

let dir: string;
let out: string;
let bin: string;

function stub(name: string, body: string): string {
	const path = join(bin, name);
	writeFileSync(path, `#!/bin/sh\n${body}\n`);
	chmodSync(path, 0o755);
	return path;
}

function relaunch(mode: string, effort: string, model = "") {
	return spawnSync("/bin/sh", [SCRIPT, mode], {
		cwd: dir,
		encoding: "utf8",
		env: {
			PATH: `${bin}:/usr/bin:/bin`,
			TMPDIR: dir,
			RELAUNCH_PS: join(bin, "ps"),
			RELAUNCH_PIJ_RS: join(bin, "pij-rs"),
			PROC_IN_SEAT: "pij-x",
			PROC_IN_EFFORT: effort,
			PROC_IN_MODEL: model,
		},
	});
}

/** Run the pane's command as tmux would, and return the copilot stub's argv and environment. */
function runPane(): { args: string[]; env: string } {
	const argv = readFileSync(join(out, "tmux.args"), "utf8");
	const command = argv.trimEnd().split("\n").at(-1) ?? "";
	const pane = spawnSync("/bin/sh", ["-c", command], {
		cwd: dir,
		encoding: "utf8",
		env: { PATH: `${bin}:/usr/bin:/bin` },
	});
	expect(pane.status).toBe(0);
	return {
		args: readFileSync(join(out, "copilot.args"), "utf8").split("\n"),
		env: readFileSync(join(out, "copilot.env"), "utf8"),
	};
}

beforeEach(() => {
	dir = mkdtempSync(join(tmpdir(), "proc-fleet-0001-"));
	out = join(dir, "out");
	bin = join(dir, "bin");
	mkdirSync(out);
	mkdirSync(bin);
	// pij list: the old process first, then the relaunched one.
	stub(
		"pij",
		`n=$(cat "${out}/pij.n" 2>/dev/null || echo 0); echo $((n + 1)) > "${out}/pij.n"
pid=100; [ "$n" -ge 1 ] && pid=200
printf '{"data":{"seats":[{"id":"pij-x","pane":"%%7","session":"sess-1","folder":"${dir}","proc":{"pid":%s}}]}}' "$pid"`,
	);
	// ps prints data files verbatim, so nothing in them is ever expanded by the stub itself.
	writeFileSync(join(out, "ps.cmd"), `${PS_COMMAND}\n`);
	writeFileSync(join(out, "ps.all"), `${PS_COMMAND} ${PS_ENV}\n`);
	stub(
		"ps",
		`case "$1" in -Eww) cat "${out}/ps.all" ;; *) if [ -n "\${PS_LINE:-}" ]; then printf '%s\\n' "$PS_LINE"; else cat "${out}/ps.cmd"; fi ;; esac`,
	);
	stub("copilot", `printf '%s\\n' "$@" > "${out}/copilot.args"; env > "${out}/copilot.env"`);
	stub(
		"tmux",
		`case "$1" in display-message) echo 100 ;; *) printf '%s\\n' "$@" > "${out}/tmux.args" ;; esac`,
	);
	stub(
		"pij-rs",
		`printf '{"data":{"status":{"ready":true,"state":"ready","models":{"claude-sonnet-5.5":{"max_prompt_tokens":936000}}}}}'`,
	);
});

afterEach(() => rmSync(dir, { recursive: true, force: true }));

describe("PROC-fleet-0001 relaunch.sh", () => {
	it("refuses an effort outside Copilot's list before touching anything", () => {
		for (const effort of ["high; touch pwned", ".*", "high high", ""]) {
			const result = relaunch("relaunch", effort);
			expect(result.status).not.toBe(0);
			expect(existsSync(join(out, "tmux.args"))).toBe(false);
		}
		expect(existsSync(join(dir, "pwned"))).toBe(false);
	});

	it("puts no environment value on the respawn command line, and the pane gets them from a 0600 file", () => {
		const result = relaunch("relaunch", "xhigh");
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(
			"carrying: PIJ_SESSION_ID COPILOT_PROVIDER_TYPE COPILOT_PROVIDER_BASE_URL COPILOT_PROVIDER_MODEL_ID COPILOT_PROVIDER_MAX_PROMPT_TOKENS PIJ_QUOTED",
		);
		expect(result.stdout).toContain(
			"not carrying (spawn id or secret-like): COPILOT_PROVIDER_API_KEY PIJ_BG_TOKEN PIJ_SPAWN_ID",
		);
		const argv = readFileSync(join(out, "tmux.args"), "utf8");
		for (const value of [SECRET, "127.0.0.1", "pij-x", "spawn-1", "it's"])
			expect(argv).not.toContain(value);
		const command = argv.trimEnd().split("\n").at(-1) ?? "";
		const envFile = /^\. '([^']+)'/.exec(command)?.[1] ?? "";
		expect(statSync(envFile).mode & 0o777).toBe(0o600);
		expect(readFileSync(envFile, "utf8")).not.toContain(SECRET);

		// Run the pane's command as tmux would: the values arrive, quoted words stay words, and the file is gone.
		const pane = spawnSync("/bin/sh", ["-c", command], {
			cwd: dir,
			encoding: "utf8",
			env: { PATH: `${bin}:/usr/bin:/bin` },
		});
		expect(pane.status).toBe(0);
		expect(existsSync(envFile)).toBe(false);
		expect(existsSync(join(dir, "pwned"))).toBe(false);
		expect(readFileSync(join(out, "copilot.args"), "utf8").split("\n")).toEqual([
			"--model",
			"claude-opus-5.5",
			"--context",
			"long_context",
			"--allow-all-tools",
			"*",
			"--effort",
			"xhigh",
			"--resume=sess-1",
			"",
		]);
		const env = readFileSync(join(out, "copilot.env"), "utf8");
		expect(env).toContain("COPILOT_PROVIDER_BASE_URL=http://127.0.0.1:9/v1\n");
		expect(env).toContain("PIJ_QUOTED=it's;$(touch${IFS}pwned)\n");
		expect(env).not.toContain(SECRET);
		expect(env).not.toContain("PIJ_SPAWN_ID");
	});

	it("checks the effort as an exact word, never a pattern", () => {
		const check = (effort: string, line = PS_COMMAND) =>
			spawnSync("/bin/sh", [SCRIPT, "flags-are"], {
				encoding: "utf8",
				env: {
					PATH: `${bin}:/usr/bin:/bin`,
					RELAUNCH_PS: join(bin, "ps"),
					PS_LINE: line,
					PROC_IN_SEAT: "pij-x",
					PROC_IN_EFFORT: effort,
				},
			}).status;
		expect(check("medium")).toBe(0);
		expect(check(".*")).not.toBe(0);
		expect(check("med")).not.toBe(0);
		expect(check("high")).not.toBe(0);
		expect(check("high", "copilot --reasoning-effort=high")).toBe(0);
		expect(check("high", "copilot --reasoning-effort high --x")).toBe(0);
		expect(check("high", "copilot --x high")).not.toBe(0);
		// An allowed effort that a pattern would find inside another word.
		expect(check("high", "copilot --effort xhigh")).not.toBe(0);
		expect(check("high", "copilot ---effort=high")).not.toBe(0);
	});
	it("refuses a bad model, a fusion model with an effort, or nothing to change, before touching anything", () => {
		for (const [effort, model] of [
			["", "gpt-5; touch pwned"],
			["", "-x"],
			["", "a b"],
			["high", "hydrafusion"],
			["", ""],
		]) {
			const result = relaunch("relaunch", effort, model);
			expect(result.status).not.toBe(0);
			expect(existsSync(join(out, "tmux.args"))).toBe(false);
		}
		expect(existsSync(join(dir, "pwned"))).toBe(false);
	});

	it("moves a proxied seat to a GPT model as plain Copilot: drops COPILOT_PROVIDER_*, keeps its effort", () => {
		const result = relaunch("relaunch", "", "gpt-5.5");
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(
			"dropped (gpt-5.5 is not Claude, so it runs plain Copilot): COPILOT_PROVIDER_TYPE COPILOT_PROVIDER_BASE_URL COPILOT_PROVIDER_MODEL_ID COPILOT_PROVIDER_MAX_PROMPT_TOKENS",
		);
		const { args, env } = runPane();
		expect(args).toEqual([
			"--effort",
			"medium",
			"--allow-all-tools",
			"*",
			"--model",
			"gpt-5.5",
			"--context",
			"long_context",
			"--resume=sess-1",
			"",
		]);
		expect(env).not.toContain("COPILOT_PROVIDER_");
		expect(env).toContain("PIJ_SESSION_ID=pij-x\n");
	});

	it("moves a proxied seat to another Claude model through the proxy, with the proxy's id and limit", () => {
		const result = relaunch("relaunch", "high", "claude-sonnet-5.5-1m");
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const { args, env } = runPane();
		expect(args.slice(-8)).toEqual([
			"--model",
			"claude-sonnet-5.5-1m",
			"--context",
			"long_context",
			"--effort",
			"high",
			"--resume=sess-1",
			"",
		]);
		expect(env).toContain("COPILOT_PROVIDER_TYPE=anthropic\n");
		expect(env).toContain("COPILOT_PROVIDER_MODEL_ID=claude-sonnet-5.5\n");
		expect(env).toContain("COPILOT_PROVIDER_MAX_PROMPT_TOKENS=936000\n");
	});

	it("refuses a Claude model the proxy does not serve on a proxied seat", () => {
		const result = relaunch("relaunch", "", "claude-unlisted-9");
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("does not serve claude-unlisted-9");
		expect(existsSync(join(out, "tmux.args"))).toBe(false);
	});

	it("gives a fusion model the default context tier and no effort", () => {
		expect(relaunch("relaunch", "", "hydrafusion").status).toBe(0);
		const { args } = runPane();
		expect(args).toEqual([
			"--allow-all-tools",
			"*",
			"--model",
			"hydrafusion",
			"--context",
			"default",
			"--resume=sess-1",
			"",
		]);
	});

	it("checks the model and its cache-proxy environment on the live process", () => {
		const check = (model: string, cmd: string, env: string) => {
			writeFileSync(join(out, "ps.all"), `${cmd} ${env}\n`);
			return spawnSync("/bin/sh", [SCRIPT, "flags-are"], {
				encoding: "utf8",
				env: {
					PATH: `${bin}:/usr/bin:/bin`,
					RELAUNCH_PS: join(bin, "ps"),
					PS_LINE: cmd,
					PROC_IN_SEAT: "pij-x",
					PROC_IN_MODEL: model,
				},
			}).status;
		};
		const proxied = "COPILOT_PROVIDER_TYPE=anthropic COPILOT_PROVIDER_MODEL_ID=claude-opus-5.5";
		expect(check("gpt-5.5", "copilot --model gpt-5.5", "PIJ_SESSION_ID=pij-x")).toBe(0);
		expect(check("gpt-5.5", "copilot --model gpt-5.5", proxied)).not.toBe(0);
		expect(check("gpt-5", "copilot --model gpt-5.5", "")).not.toBe(0);
		expect(check("claude-opus-5.5-1m", "copilot --model=claude-opus-5.5-1m", proxied)).toBe(0);
		expect(check("claude-sonnet-5.5", "copilot --model claude-sonnet-5.5", proxied)).not.toBe(0);
		expect(check("hydrafusion", "copilot --model hydrafusion --effort high", "")).not.toBe(0);
	});
	it("reads the launch from the pane's own process, as `node <path>/copilot`, and refuses one that is not copilot", () => {
		writeFileSync(
			join(out, "ps.cmd"),
			"node /opt/lib/bin/copilot --yolo --model claude-opus-5.5 --context long_context --effort xhigh --resume=old\n",
		);
		expect(relaunch("relaunch", "xhigh").status).toBe(0);
		expect(runPane().args).toEqual([
			"--yolo",
			"--model",
			"claude-opus-5.5",
			"--context",
			"long_context",
			"--effort",
			"xhigh",
			"--resume=sess-1",
			"",
		]);
		rmSync(join(out, "tmux.args"));
		writeFileSync(join(out, "ps.cmd"), "/bin/zsh -l\n");
		const result = relaunch("relaunch", "xhigh");
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("is not copilot");
		expect(existsSync(join(out, "tmux.args"))).toBe(false);
	});
});
