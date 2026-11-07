// The one test-only wall-clock scale (plan 209, F28): a budget on real child
// processes is multiplied by PIJ_TEST_TIME_SCALE (default 1), so a loaded or
// niced machine stretches it instead of failing. Production never reads it.
// The Rust side is `pij_testkit::time`.

export const TIME_SCALE_VAR = "PIJ_TEST_TIME_SCALE";

/** Unset or blank is 1; otherwise a finite number of at least 1, else throws. */
export function parseTimeScale(raw: string | undefined): number {
	const text = raw?.trim() ?? "";
	if (text === "") return 1;
	const scale = Number(text);
	if (!Number.isFinite(scale) || scale < 1)
		throw new Error(`${TIME_SCALE_VAR} must be a number >= 1, not ${JSON.stringify(raw)}`);
	return scale;
}

/** `ms` stretched by the scale in `env` (the process's own by default). */
export function scaled(ms: number, env: NodeJS.ProcessEnv = process.env): number {
	return Math.ceil(ms * parseTimeScale(env[TIME_SCALE_VAR]));
}
