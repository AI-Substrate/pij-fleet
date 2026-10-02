// pij platform — checked ISO timestamp construction (review 001 F7).
//
// The ONE place a clock becomes an ISO string. `new Date(ms).toISOString()`
// throws RangeError on NaN or any |ms| beyond the ECMA TimeClip bound
// (8.64e15), bypassing the tagged-union error contract — so every fallible
// constructor funnels its nowMs through here and propagates the Result
// (Pattern P4: no throws in core). Lives beside types.ts, never inside it:
// types.ts imports NOTHING (zero-import law) and this module needs Result.

import { err, ok, type Result } from "../types.js";

/** ECMA TimeClip bound: a Date time value must satisfy |ms| ≤ 8.64e15. */
const MAX_TIME_MS = 8.64e15;

/** Stamp epoch milliseconds as ISO-8601, or E-ARG naming nowMs. */
export function isoTimestamp(nowMs: number): Result<string> {
	if (!Number.isFinite(nowMs) || Math.abs(nowMs) > MAX_TIME_MS) {
		return err(
			"E-ARG",
			`nowMs must be finite epoch milliseconds within ±8.64e15 (got ${String(nowMs)})`,
		);
	}
	return ok(new Date(nowMs).toISOString());
}
