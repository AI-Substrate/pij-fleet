// pij-control-plane — pure model validation (T006).
//
// validateModel: ok when model is known (exact or fuzzy match), unknown when not.
// Never blocks — callers warn and continue; the spawn id is always returned.

import { closestModel, normalizeModelQuery } from "./match.js";
import type { ModelEntry } from "./registry.js";

export type ValidationResult =
	| { readonly ok: true }
	| { readonly ok: false; readonly unknown: true; readonly suggestion: string | null };

function findKnownModel(model: string, known: readonly ModelEntry[]): ModelEntry | undefined {
	const norm = normalizeModelQuery(model);
	return known.find((entry) => {
		const candidates = entry.selector
			? [entry.id, entry.selector]
			: [entry.id, `${entry.provider}/${entry.id}`];
		return candidates.some((candidate) => normalizeModelQuery(candidate) === norm);
	});
}

/**
 * Validate a model id against the known model list. Returns `ok` when:
 *   - model is empty/undefined (no --model flag — nothing to validate)
 *   - known list is empty (cannot validate, so never block)
 *   - model matches an entry by exact normalised bare or provider-qualified id (case-insensitive)
 * Returns `{ ok: false, unknown: true, suggestion }` otherwise. Suggestion is
 * the closest known id, or null if nothing is close.
 */
export function validateModel(model: string, known: readonly ModelEntry[]): ValidationResult {
	if (!model || known.length === 0) return { ok: true };
	const exactMatch = findKnownModel(model, known);
	if (exactMatch) return { ok: true };
	const closest = closestModel(model, known);
	return { ok: false, unknown: true, suggestion: closest?.id ?? null };
}

export type EffortValidation =
	| { readonly ok: true }
	| { readonly ok: false; readonly unsupported: true; readonly levels: readonly string[] };

/**
 * Validate a requested `--effort` level against the chosen model's known levels
 * (#3, task 2.6). Warn-don't-block — like {@link validateModel}, it only reports
 * `unsupported` when the registry can POSITIVELY contradict the request:
 *   - no effort requested, or no model → ok (nothing to validate)
 *   - model not found in `known`, or the entry carries no level data → ok (cannot
 *     validate, so never block — mirrors the empty-known-list rule)
 *   - effort matches one of the model's levels (case-insensitive) → ok
 * Otherwise `{ ok: false, unsupported: true, levels }` (the supported levels, so
 * the caller can list them). NEVER throws/blocks — the spawn always proceeds.
 */
export function validateEffort(
	effort: string,
	model: string | undefined,
	known: readonly ModelEntry[],
): EffortValidation {
	if (!effort || !model) return { ok: true };
	const entry = findKnownModel(model, known);
	const levels = entry?.levels ?? [];
	if (levels.length === 0) return { ok: true }; // unknown model / no level data → can't validate
	const want = effort.toLowerCase();
	if (levels.some((l) => l.toLowerCase() === want)) return { ok: true };
	return { ok: false, unsupported: true, levels };
}
