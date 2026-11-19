import { describe, expect, it } from "vitest";
import checks from "../../.harness/extensions/checks/extension.ts";

interface Result {
	name: string;
	status: string;
	output?: string;
}

/**
 * Runs the real `harness checks --quick` verb with every sensor stubbed: each
 * command exits 0 and prints a passing report, except `copilot-sdk`, which
 * exits `code` and prints `stdout`. No sensor, test or build runs.
 */
async function copilotSdkResult(stdout: string, code = 0): Promise<Result | undefined> {
	const ctx = {
		options: { quick: true },
		exec: async (_cmd: string, args: string[]) =>
			args[0] === "copilot-sdk-contract"
				? { ok: code === 0, code, stdout, stderr: "" }
				: { ok: true, code: 0, stdout: '{"status":"pass"}', stderr: "" },
		ok: (data: { results: Result[] }) => data.results,
		error: (_code: string, _message: string, extra: { details: { results: Result[] } }) =>
			extra.details.results,
	};
	const results = (await checks.run(ctx as never)) as unknown as Result[];
	return results.find((result) => result.name === "copilot-sdk");
}

describe("harness checks: the copilot-sdk sensor's report (R259-002)", () => {
	it.each([
		['{"status":"pass"}', "pass"],
		['{"status":"warn"}', "warn"],
		['{"status":"skipped"}', "skipped"],
		["{}", "warn"],
		['{"status":"passed"}', "warn"],
		["not json", "warn"],
	])("a successful run printing %s is %s", async (stdout, status) => {
		const result = await copilotSdkResult(stdout);
		expect(result?.status).toBe(status);
		if (status !== "pass") expect(result?.output).toBe(stdout);
	});

	it("a failing run fails whatever it prints", async () => {
		expect((await copilotSdkResult('{"status":"pass"}', 1))?.status).toBe("fail");
	});
});
