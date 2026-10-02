// pij-messaging — monotonic seq counter with crash-safe recovery.
//
// Pure. After a /reload the in-memory counter is gone; we recover it from the
// event log's lastSeq() so the stream stays strictly monotonic (finding 04).

/** A strictly-increasing sequence allocator. */
export class SeqCounter {
	private current: number;

	/** Recover from the last persisted seq (0 for a fresh log). */
	constructor(lastSeq = 0) {
		this.current = lastSeq;
	}

	/** Allocate the next seq (strictly greater than every prior). */
	next(): number {
		this.current += 1;
		return this.current;
	}

	/** The most recently allocated seq (0 before any allocation). */
	peek(): number {
		return this.current;
	}
}
