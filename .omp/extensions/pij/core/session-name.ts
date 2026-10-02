export interface SessionNamePublisherDeps {
	setSessionName(name: string): Promise<unknown>;
	getSessionName?(): string | undefined;
	setStatus(key: string, text: string): void;
	notice(text: string): void;
	sleep(ms: number): Promise<void>;
}

export const PIJ_STATUS_KEY = "pij";

export const DEFAULT_SESSION_NAME_SCHEDULE_MS = [0, 250, 1_000, 3_000, 10_000] as const;

export async function publishSeatName(
	deps: SessionNamePublisherDeps,
	self: string,
	generation: "rs" | "legacy",
	schedule: readonly number[] = DEFAULT_SESSION_NAME_SCHEDULE_MS,
	extensionBuild?: string,
): Promise<{ took: boolean; attempts: number }> {
	const extension = generation === "rs" && extensionBuild ? ` · ext ${extensionBuild}` : "";
	const nativeName = `${generation}·${self}${extension}`;
	const statusText =
		generation === "rs"
			? `\u001b[33mrs\u001b[0m ${self}${extension}`
			: `\u001b[2mlegacy\u001b[0m ${self}`;
	try {
		deps.setStatus(PIJ_STATUS_KEY, statusText);
	} catch {
		// Native publication must still proceed when the fallback surface is unavailable.
	}

	let attempts = 0;
	let previousAt = 0;
	let publishedWithoutReadback = false;
	for (const scheduledAt of schedule) {
		const waitMs = Math.max(0, scheduledAt - previousAt);
		previousAt = scheduledAt;
		if (waitMs > 0) {
			try {
				await deps.sleep(waitMs);
			} catch {
				break;
			}
		}

		attempts += 1;
		try {
			await deps.setSessionName(nativeName);
			publishedWithoutReadback = true;
		} catch {
			continue;
		}

		if (deps.getSessionName) {
			try {
				if (deps.getSessionName() === nativeName) return { took: true, attempts };
			} catch {
				// A stale session manager is equivalent to a failed readback; retry.
			}
		}
	}

	if (!deps.getSessionName && publishedWithoutReadback) return { took: true, attempts };
	try {
		deps.notice(`pij: could not set omp session name — id is ${self}`);
	} catch {
		// The tagged result remains the caller's authoritative failure signal.
	}
	return { took: false, attempts };
}
