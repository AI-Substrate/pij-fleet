// harness/driver.it.test.ts
//
// Integration tests for the Driver SDK against live tmux. Gated behind
// PIJ_DRIVER_IT=1 so CI without tmux skips cleanly. Drives `bash` (not
// `pi`) for determinism — pi requires API keys and renders unpredictably.
// The validator pilot (T012) is the end-to-end test against real pi.
//
// Per workshop 001 § Testing strategy § Integration.

import { describe, expect, it } from "vitest";

const IT_ENABLED = process.env.PIJ_DRIVER_IT === "1";

describe.skipIf(!IT_ENABLED)("driver integration (live tmux + bash)", () => {
	it("Session.start → waitIdle → run → teardown round-trips against bash", async () => {
		const { Session } = await import("./driver/session.js");
		const { hasSession } = await import("./driver/tmux.js");

		const sessionName = `pij-it-${process.pid}-${Date.now()}`;
		const session = await Session.start({
			session: sessionName,
			cwd: process.cwd(),
			cmd: "bash --noprofile --norc",
			cols: 80,
			rows: 24,
		});

		try {
			// bash prompt is "$" by default with --noprofile --norc
			await session.waitIdle({
				promptRe: /\$\s*$/m,
				contextRe: /./, // anything — bash has no context% footer
				timeoutMs: 5000,
			});

			const pane = await session.run("echo hello-pij", /hello-pij/, {
				timeoutMs: 5000,
			});
			expect(pane).toMatch(/hello-pij/);
		} finally {
			session.teardown();
			expect(hasSession(sessionName)).toBe(false);
		}
	}, 15_000);

	it("hasSession() distinguishes live vs dead sessions", async () => {
		const { Session } = await import("./driver/session.js");
		const { hasSession } = await import("./driver/tmux.js");

		const sessionName = `pij-it-has-${process.pid}-${Date.now()}`;
		expect(hasSession(sessionName)).toBe(false);

		const session = await Session.start({
			session: sessionName,
			cwd: process.cwd(),
			cmd: "bash --noprofile --norc",
			cols: 80,
			rows: 24,
		});

		try {
			expect(hasSession(sessionName)).toBe(true);
		} finally {
			session.teardown();
		}

		expect(hasSession(sessionName)).toBe(false);
	}, 10_000);
});
