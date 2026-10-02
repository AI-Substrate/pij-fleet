// pij-control-plane — pure fuzzy closest-id match (T002).
//
// Normalise query → lowercase + spaces/underscores → hyphens, then:
//   1. Exact match on normalised id (fastest path)
//   2. Prefix match (normalised query is a prefix of normalised id)
//   3. Levenshtein distance (closest, capped to reject totally unlike queries)
// Returns null when nothing is close enough.

import type { ModelEntry } from "./registry.js";

/** Normalise a model query or id: lowercase, collapse whitespace/underscores to
 *  hyphens, trim. "Fugu Ultra" → "fugu-ultra"; "claude_sonnet_4_6" → "claude-sonnet-4-6". */
export function normalizeModelQuery(query: string): string {
	return query
		.trim()
		.toLowerCase()
		.replace(/[\s_]+/g, "-");
}

/** Character-level Levenshtein distance (standard DP). */
function levenshtein(a: string, b: string): number {
	const la = a.length;
	const lb = b.length;
	if (la === 0) return lb;
	if (lb === 0) return la;
	const row: number[] = Array.from({ length: lb + 1 }, (_, i) => i);
	for (let i = 1; i <= la; i++) {
		let prev = i;
		for (let j = 1; j <= lb; j++) {
			const cost = a[i - 1] === b[j - 1] ? 0 : 1;
			const cur = Math.min(prev + 1, (row[j] ?? 0) + 1, (row[j - 1] ?? 0) + cost);
			row[j - 1] = prev;
			prev = cur;
		}
		row[lb] = prev;
	}
	return row[lb] ?? 0;
}

/**
 * Find the closest model for `query` in `models`. Returns null when the list is
 * empty or no candidate is close enough (distance > half the query length, capped
 * at 5 so short queries don't accept junk).
 *
 * Priority: exact normalised-id match > prefix match > minimum Levenshtein.
 */
export function closestModel(query: string, models: readonly ModelEntry[]): ModelEntry | null {
	if (models.length === 0) return null;
	const norm = normalizeModelQuery(query);
	if (!norm) return null;

	// 1. Exact match on normalised id
	const exact = models.find((m) => normalizeModelQuery(m.id) === norm);
	if (exact) return exact;

	// 2. Prefix match (query is a prefix of the normalised model id)
	const prefixed = models.filter((m) => normalizeModelQuery(m.id).startsWith(norm));
	if (prefixed.length === 1) return prefixed[0] ?? null;
	if (prefixed.length > 1) {
		// Shortest prefix match wins (most specific without being too broad)
		return prefixed.reduce((a, b) => (a.id.length <= b.id.length ? a : b));
	}

	// 3. Levenshtein — accept only if close enough
	const maxDist = Math.min(5, Math.floor(norm.length / 2));
	let best: ModelEntry | null = null;
	let bestDist = Number.POSITIVE_INFINITY;
	for (const m of models) {
		const d = levenshtein(norm, normalizeModelQuery(m.id));
		if (d < bestDist) {
			bestDist = d;
			best = m;
		}
	}
	return bestDist <= maxDist ? best : null;
}
