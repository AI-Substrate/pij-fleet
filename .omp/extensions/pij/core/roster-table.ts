/**
 * `pij list`'s human table (plan 160): the same layout as the Rust CLI's
 * `pij_cli::roster::render_table`. The daemon derives and renders the size
 * columns (`sizeColumns`), so this only lays them out. The golden
 * `crates/testkit/fixtures/golden/cli/list-sized.{json,txt}` pins both.
 */

const HEADERS = ["SEAT", "HARNESS", "STATE", "CTX", "IDLE", "CACHE"] as const;

function record(value: unknown): Record<string, unknown> {
	return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

function str(value: unknown): string | undefined {
	return typeof value === "string" ? value : undefined;
}

/** Character count, matching Rust's `chars().count()` for padding. */
function width(text: string): number {
	return [...text].length;
}

function pad(text: string, to: number): string {
	return text + " ".repeat(Math.max(0, to - width(text)));
}

export function renderRosterTable(payload: unknown): string {
	const data = record(payload);
	const seats = Array.isArray(data.seats) ? data.seats : [];
	const rows = seats.map((entry) => {
		const seat = record(entry);
		const columns = Array.isArray(seat.sizeColumns) ? seat.sizeColumns : undefined;
		const id = str(seat.id) ?? "-";
		const machine = str(seat.machine);
		const column = (index: number): string => str(columns?.[index]) ?? "-";
		return {
			cold: columns?.[3] === "❄",
			cells: [
				machine !== undefined && columns === undefined ? `${id}@${machine}` : id,
				str(seat.harness) ?? "-",
				str(seat.state) ?? "-",
				column(0),
				column(1),
				column(2),
			],
		};
	});
	const widths = HEADERS.map((header, index) =>
		Math.max(width(header), ...rows.map((row) => width(row.cells[index] ?? ""))),
	);
	const line = (mark: string, cells: readonly string[]): string =>
		(
			mark +
			cells
				.map((cell, index) =>
					index + 1 === cells.length ? cell : `${pad(cell, widths[index] ?? 0)}  `,
				)
				.join("")
		).trimEnd();
	const lines = [
		line("  ", HEADERS),
		...rows.map((row) => line(row.cold ? "❄ " : "  ", row.cells)),
	];
	const unavailable = Array.isArray(data.unavailable) ? data.unavailable : [];
	for (const peer of unavailable) {
		const entry = record(peer);
		lines.push(`unavailable: ${str(entry.machine) ?? "?"} (${str(entry.reason) ?? "?"})`);
	}
	return lines.join("\n");
}
