// pij-messaging — human seat addresses: `<seat>` or `<seat>@<machine-alias>` (pure).
//
// Mirrors crates/core/src/address.rs exactly; both sides are pinned to the shared
// golden crates/testkit/fixtures/golden/address/cases.json. Within the seat id,
// `@@` is a literal `@`; the first unescaped `@` separates the machine alias, and
// an alias cannot contain `@`.

import { err, ok, type Result } from "./types.js";

/** A daemon send destination (`SendRequest.to`); no `machine` means local. */
export interface Destination {
	readonly seat: string;
	readonly machine?: string;
}

/** Parse a human seat address (Rust `parse_destination`). */
export function parseDestination(input: string): Result<Destination> {
	let seat = "";
	let machine: string | undefined;
	for (let i = 0; i < input.length; i++) {
		const character = input.charAt(i);
		if (machine !== undefined) {
			if (character === "@") {
				return err(
					"E-ARG",
					"machine aliases cannot contain '@' — escape '@' only inside the seat as '@@'",
				);
			}
			machine += character;
		} else if (character !== "@") {
			seat += character;
		} else if (input.charAt(i + 1) === "@") {
			i++;
			seat += "@";
		} else {
			machine = "";
		}
	}
	if (seat === "") return err("E-ARG", "a seat address needs a seat id");
	if (machine === "") {
		return err("E-ARG", "a qualified seat address needs a machine alias after '@'");
	}
	return ok(machine === undefined ? { seat } : { seat, machine });
}

/** Render a destination in the exact grammar `parseDestination` accepts (Rust `render_destination`). */
export function renderDestination(destination: Destination): string {
	const escapedSeat = destination.seat.replaceAll("@", "@@");
	return destination.machine === undefined ? escapedSeat : `${escapedSeat}@${destination.machine}`;
}
