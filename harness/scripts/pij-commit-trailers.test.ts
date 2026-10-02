import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { type AttributionContext, scanBranch, scanCommit } from "./pij-commit-trailers.js";

const context: AttributionContext = {
	seatId: "pij-coder",
	seats: [
		{ id: "pij-coder", session: "native-coder" },
		{ id: "pij-prime", session: "native-prime", role: "prime" },
	],
	trailers: "Pij-Seat: pij-coder\nPij-Prime: pij-prime\n",
};
const harnessTrailers =
	"Co-Authored-By: Copilot <copilot@example.test>\nOMP-Session: native-coder\n";

describe("pij-commit-trailers", () => {
	it("prescribes exact derived lines for a known non-prime and omits an absent plan", () => {
		const result = scanCommit("sha", "Human <human@example.test>", harnessTrailers, context);
		expect(result.finding).toEqual({
			sha: "sha",
			missing: ["Pij-Prime", "Pij-Seat"],
			add: ["Pij-Prime: pij-prime", "Pij-Seat: pij-coder"],
			unresolved: [],
		});
	});

	it("accepts a coder's complete attribution", () => {
		expect(scanCommit("sha", "Human", `${harnessTrailers}${context.trailers}`, context)).toEqual({
			harness: true,
		});
	});

	it("does not require a seat trailer for a prime committer", () => {
		expect(
			scanCommit("sha", "Claude", "Claude-Session: native-prime\nPij-Prime: pij-prime", context),
		).toEqual({ harness: true });
	});

	it("identifies a prime before the prime trailer has been added", () => {
		const primeContext = { ...context, seatId: "pij-prime", trailers: "Pij-Prime: pij-prime\n" };
		expect(
			scanCommit("sha", "Claude", "Claude-Session: native-prime", primeContext).finding,
		).toEqual({
			sha: "sha",
			missing: ["Pij-Prime"],
			add: ["Pij-Prime: pij-prime"],
			unresolved: [],
		});
	});

	it.each([
		{ id: "pij-other-prime", session: "other", role: "prime", parent: "pij-parent" },
		{ id: "pij-other-prime", session: "other", role: null, parent: null },
	])("does not prescribe Seat for another registered prime or root", (seat) => {
		const otherContext = { ...context, seats: [...context.seats, seat] };
		expect(scanCommit("sha", "Claude", "Claude-Session: other", otherContext).finding).toEqual({
			sha: "sha",
			missing: ["Pij-Prime"],
			add: ["Pij-Prime: pij-other-prime"],
			unresolved: [],
		});
	});

	it.each([
		"Claude",
		"omp",
		"GitHub Copilot",
		"Codex",
	])("recognizes harness author %s", (author) => {
		expect(scanCommit("sha", author, "", { seats: [], trailers: "" }).finding?.missing).toEqual([
			"Pij-Prime",
		]);
	});

	it("never assigns today's identity to an unknown or different historical committer", () => {
		const unknown = scanCommit("sha", "Codex", "", context);
		expect(unknown.finding?.add).toEqual([]);
		expect(unknown.finding?.unresolved).toHaveLength(1);
		expect(unknown.unverified).toContain("applicability unresolved");
		const other = scanCommit("sha", "Codex", "Codex-Session: other-session", {
			...context,
			seats: [...context.seats, { id: "pij-other", session: "other-session" }],
		});
		expect(other.finding?.add).toEqual(["Pij-Seat: pij-other"]);
		expect(other.finding?.unresolved).toHaveLength(1);
	});

	it("reports unknown seat applicability rather than silently accepting missing Seat", () => {
		const result = scanCommit("sha", "Claude", "Pij-Prime: pij-prime", context);
		expect(result.finding).toBeUndefined();
		expect(result.unverified).toContain("no unique registered native session");
	});

	it("ignores ordinary human commits", () => {
		expect(
			scanCommit("sha", "Human <human@example.test>", "Signed-off-by: Human", context),
		).toEqual({ harness: false });
	});

	it("only checks real trailers after merge-base, catching one synthetic mutation", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-commit-trailers-"));
		const git = (args: string[]) =>
			execFileSync("git", args, {
				cwd: root,
				encoding: "utf8",
				env: {
					...process.env,
					GIT_AUTHOR_NAME: "Human",
					GIT_AUTHOR_EMAIL: "human@example.test",
					GIT_COMMITTER_NAME: "Human",
					GIT_COMMITTER_EMAIL: "human@example.test",
				},
			});
		try {
			git(["init", "--quiet", "--initial-branch=main"]);
			git([
				"-c",
				"commit.gpgsign=false",
				"commit",
				"--quiet",
				"--allow-empty",
				"-m",
				`legacy\n\n${harnessTrailers}`,
			]);
			git(["switch", "--quiet", "-c", "topic"]);
			git([
				"-c",
				"commit.gpgsign=false",
				"commit",
				"--quiet",
				"--allow-empty",
				"-m",
				`attributed\n\n${harnessTrailers}${context.trailers}`,
			]);
			expect(scanBranch(root, context)).toEqual({
				status: "pass",
				checked: 1,
				findings: [],
				unverified: [],
			});
			git([
				"-c",
				"commit.gpgsign=false",
				"commit",
				"--quiet",
				"--allow-empty",
				"-m",
				`synthetic missing\n\n${harnessTrailers}`,
			]);
			const report = scanBranch(root, context);
			expect(report.status).toBe("warn");
			expect(report.checked).toBe(2);
			expect(report.findings).toHaveLength(1);
			expect(report.findings[0]?.add).toEqual(["Pij-Prime: pij-prime", "Pij-Seat: pij-coder"]);
			// A body example cannot satisfy Git's trailer contract.
			git([
				"-c",
				"commit.gpgsign=false",
				"commit",
				"--quiet",
				"--allow-empty",
				"-m",
				`example\n\n${context.trailers}\nNot trailers: explanatory body.\n\n${harnessTrailers}`,
			]);
			expect(scanBranch(root, context).findings).toHaveLength(2);
		} finally {
			rmSync(root, { recursive: true, force: true, maxRetries: 3 });
		}
	});

	it("explicitly reports unavailable merge-base as advisory skip", () => {
		const root = mkdtempSync(join(tmpdir(), "pij-no-base-"));
		try {
			expect(scanBranch(root, context).status).toBe("skip");
		} finally {
			rmSync(root, { recursive: true, force: true, maxRetries: 3 });
		}
	});
});
