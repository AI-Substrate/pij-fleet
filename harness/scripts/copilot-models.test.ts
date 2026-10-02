import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const SCRIPT = resolve(import.meta.dirname, "copilot-models.py");

function runPython(source: string): ReturnType<typeof spawnSync> {
	return spawnSync("python3", ["-c", source], {
		cwd: resolve(import.meta.dirname, "..", ".."),
		encoding: "utf8",
	});
}

describe("copilot-models", () => {
	it("retries a stale Pi credential with a fresh OMP token", () => {
		const result = runPython(`
import importlib.util
import io
import sys
import urllib.error

spec = importlib.util.spec_from_file_location("copilot_models", ${JSON.stringify(SCRIPT)})
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

stale = "proxy-ep=https://api.individual.githubcopilot.com;stale"
fresh = "proxy-ep=https://api.enterprise.githubcopilot.com;fresh"
calls = []
module.load_pi_token = lambda: stale
module.load_omp_token = lambda: fresh

def fetch(token):
    calls.append(token)
    if token == stale:
        raise urllib.error.HTTPError("https://example.invalid/models", 401, "expired", None, io.BytesIO(b"expired"))
    return {"host": module.host_for(token), "payload": {"data": [{"id": "claude-opus-5"}]}}

module.fetch_models = fetch
sys.argv = ["copilot-models.py", "opus-5"]
module.main()
assert calls == [stale, fresh]
`);

		expect(result.status, String(result.stderr)).toBe(0);
		expect(result.stdout).toContain("host: api.enterprise.githubcopilot.com");
		expect(result.stdout).toContain("claude-opus-5");
	});
});
