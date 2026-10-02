// Explicit fault injection only: real native callbacks/read responses are hidden at one old cursor.
// Tail and backward recovery calls always execute against the actual Copilot SDK/history.
import { existsSync } from "node:fs";
import { join } from "node:path";

export function faultedSession(session) {
	const arm = join(process.env.COPILOT_HOME, `receiver-fault-${session.sessionId}.arm`);
	let cursor;
	let recovered = false;
	const active = () => !recovered && existsSync(arm);
	const eventLog = session.rpc.eventLog;
	return {
		send: (input) => session.send(input),
		on: (listener) =>
			session.on((event) => {
				if (!active() || event.type === "session.shutdown") listener(event);
			}),
		rpc: {
			...session.rpc,
			eventLog: {
				...eventLog,
				async read(input) {
					if (active() && input.direction !== "backward") {
						cursor ??= input.cursor;
						if (input.cursor === cursor) {
							console.error(
								`[pij-smoke-fault] ${JSON.stringify({ kind: "injected-empty-cursor", cursor })}`,
							);
							return { events: [], cursor, hasMore: false, cursorStatus: "ok" };
						}
					}
					const page = await eventLog.read(input);
					if (
						active() &&
						input.direction === "backward" &&
						page.events.some((event) => event.type === "abort")
					) {
						recovered = true;
						console.error(
							`[pij-smoke-fault] ${JSON.stringify({ kind: "real-backward-recovery", events: page.events.length })}`,
						);
					}
					return page;
				},
			},
		},
	};
}
