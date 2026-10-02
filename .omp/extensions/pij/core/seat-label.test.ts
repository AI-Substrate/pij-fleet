import { describe, expect, it } from "vitest";
import { buildSeatLabel } from "./seat-label.js";

describe("buildSeatLabel", () => {
	it("names a stream revive window by work and puts peer/model in the pane title", () => {
		expect(
			buildSeatLabel({
				cwd: "/repo/pij-worktrees/s066-session-revive",
				job: "revive",
				peerId: "pij-finished-fox",
				model: "claude-sonnet-5",
			}),
		).toEqual({
			windowName: "s066-revive",
			paneTitle: "s066 revive · pij-finished-fox · claude-sonnet-5",
		});
	});

	it("uses a readable repo label outside stream worktrees and sanitizes the job", () => {
		expect(
			buildSeatLabel({
				cwd: "/repo/pij",
				job: "agent reviewer",
				peerId: "pij-calm-owl",
			}),
		).toEqual({
			windowName: "pij-agent-reviewer",
			paneTitle: "pij agent-reviewer · pij-calm-owl · default",
		});
	});
});
