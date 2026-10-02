import {
	NAME_ADJECTIVES as adjectives,
	NAME_NOUNS as nouns,
	SHIP_NAMES as ships,
} from "./name-corpus.js";
import { err, ok, type Result, type SessionId } from "./types.js";

// The seat-name corpus is vendored + curated in ./name-corpus.ts (edit the word
// lists there). We keep the lib's seeded generator, but feed it OUR dictionaries.
export { EXCLUDED_NAME_WORDS } from "./name-corpus.js";

// The id space is the adjective×noun grid PLUS the standalone ship names appended
// after it: index < PAIR_SPACE ⇒ `pij-<adjective>-<noun>`, otherwise ⇒ `pij-<ship>`.
const PAIR_SPACE = adjectives.length * nouns.length;
export const MEMORABLE_PIJ_ID_SPACE = PAIR_SPACE + ships.length;

// Ships are far too few to surface from a flat uniform draw over ~500k pairs, so
// we weight the seed's STARTING slot: roughly 1 seat in SHIP_NAME_EVERY starts on
// a ship name, the rest start on a pair. Strided probing still walks the whole
// space, so every name stays reachable and collision-free.
const SHIP_NAME_EVERY = 6;

/** Deterministic 32-bit FNV-1a hash of a seed (no Date/Math.random — resume-safe). */
function fnv1a(seed: string): number {
	let hash = 0x811c9dc5;
	for (let i = 0; i < seed.length; i++) {
		hash ^= seed.charCodeAt(i);
		hash = Math.imul(hash, 0x01000193);
	}
	return hash >>> 0;
}

// FNV-1a mixes its LOW bits weakly, so `hash % n` for small n is badly biased on the
// structured seeds we actually generate (`s<epoch>-<pid>`, `adopt\0…\0<panePid>`).
// Measured: 10 ship starts in 405 real-shaped seeds against a designed 1-in-6 (~68).
// Avalanche the hash before taking any modulus.
function mix32(hash: number): number {
	let h = hash >>> 0;
	h ^= h >>> 16;
	h = Math.imul(h, 0x7feb352d);
	h ^= h >>> 15;
	h = Math.imul(h, 0x846ca68b);
	h ^= h >>> 16;
	return h >>> 0;
}

/** 53-bit hash, so `% PAIR_SPACE` (~5e5) carries no measurable modulo bias. */
function hash53(seed: string): number {
	const high = mix32(fnv1a(`${seed}#hi`));
	const low = mix32(fnv1a(`${seed}#lo`));
	return high * 0x200000 + (low >>> 11); // 2^32 * 2^21 = 2^53
}

/** The ship this seed opens on, or null if it opens in the adjective×noun grid. */
function shipSlotFor(seed: string): number | null {
	if (ships.length === 0) return null;
	if (mix32(fnv1a(`${seed}#ship`)) % SHIP_NAME_EVERY !== 0) return null;
	return mix32(fnv1a(`${seed}#shipslot`)) % ships.length;
}

/**
 * The seed's starting row/column in the adjective×noun grid.
 *
 * This used to ask `unique-names-generator` for a word pair and look its indices up.
 * That library's seeded RNG has a tiny effective range: 5000 distinct seeds reached
 * only 66 of our 1177 adjectives, so starts piled onto a few rows. Hashing the seed
 * straight to an index draws uniformly over the whole grid instead.
 */
function pairStart(seed: string): number {
	return hash53(seed) % PAIR_SPACE;
}

function gcd(a: number, b: number): number {
	let [x, y] = [a, b];
	while (y > 0) [x, y] = [y, x % y];
	return x;
}

/**
 * Probe stride. Stepping by 1 walked a single adjective's entire row on collision —
 * `able-aardvark`, `able-aardwolf`, `able-albatross`, … — which is how one adjective
 * came to hold 79 of 402 live seats. ANY stride coprime with the space still visits
 * every slot exactly once, so the no-repeat/full-coverage contract is unchanged; it
 * just lands the next candidate in a different row. Falls back to 1 (previous
 * behaviour) if the corpus is ever resized such that no candidate is coprime.
 */
const PROBE_STRIDE: number = (() => {
	for (const prime of [15_485_863, 104_729, 65_537, 7919, 257, 31, 7, 3]) {
		const stride = prime % PAIR_SPACE;
		if (stride > 1 && gcd(stride, PAIR_SPACE) === 1) return stride;
	}
	return 1;
})();

/**
 * The slot this seed offers at `attempt`, over the whole space and without repeats.
 *
 * Order is: the seed's ship (if it drew one) → the entire adjective×noun grid, strided
 * from the seed's own hashed start → every remaining ship. Ships sit at the END rather
 * than interleaved because ships-then-grid is a FUNNEL: only 40 ships exist but ~1 seat
 * in 6 opens on one, so every loser used to probe onward from a 40-slot-wide window and
 * land in a single shared adjective row. Measured: one row took 21-24 of 405 seats where
 * ~3 is the expected maximum. Walking the grid from the seed's own hash decorrelates them.
 */
function slotAt(seed: string, attempt: number): number {
	const ship = shipSlotFor(seed);
	if (ship !== null && attempt === 0) return PAIR_SPACE + ship;
	const step = ship === null ? attempt : attempt - 1;
	if (step < PAIR_SPACE) return (pairStart(seed) + step * PROBE_STRIDE) % PAIR_SPACE;
	// Trailing ships, skipping the one already offered at attempt 0.
	let index = step - PAIR_SPACE;
	if (ship !== null && index >= ship) index += 1;
	return PAIR_SPACE + index;
}

function idAt(index: number): SessionId | null {
	if (index >= PAIR_SPACE) {
		const ship = ships[index - PAIR_SPACE];
		return ship ? `pij-${ship}` : null;
	}
	const adjective = adjectives[Math.floor(index / nouns.length)];
	const noun = nouns[index % nouns.length];
	return adjective && noun ? `pij-${adjective}-${noun}` : null;
}

/** One deterministic, non-repeating candidate from the pinned name space. */
export function memorablePijIdCandidate(seed: string, attempt: number): Result<SessionId> {
	if (!Number.isInteger(attempt) || attempt < 0) {
		return err("E-ARG", `memorable id attempt must be a non-negative integer (got ${attempt})`);
	}
	if (attempt >= MEMORABLE_PIJ_ID_SPACE) {
		return err("E-FULL", `memorable id space exhausted after ${MEMORABLE_PIJ_ID_SPACE} attempts`);
	}
	const id = idAt(slotAt(seed, attempt));
	return id ? ok(id) : err("E-NOREG", "pinned memorable-id corpus index is invalid");
}

/** Full deterministic candidate sequence. Linear probing guarantees no repeats before exhaustion. */
export function* memorablePijIdCandidates(seed: string): Generator<SessionId> {
	for (let attempt = 0; attempt < MEMORABLE_PIJ_ID_SPACE; attempt++) {
		const id = idAt(slotAt(seed, attempt));
		if (id) yield id;
	}
}
