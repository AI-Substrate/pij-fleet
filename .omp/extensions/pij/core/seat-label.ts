import { basename } from "node:path";

export interface SeatLabelInput {
	readonly cwd: string;
	readonly job: string;
	readonly peerId: string;
	readonly model?: string;
}

export interface SeatLabel {
	readonly windowName: string;
	readonly paneTitle: string;
}

function slug(value: string): string {
	return (
		value
			.toLowerCase()
			.replace(/[^a-z0-9]+/g, "-")
			.replace(/^-+|-+$/g, "") || "seat"
	);
}

function streamLabel(cwd: string): string {
	const folder = slug(basename(cwd));
	return folder.match(/^s\d+/)?.[0] ?? folder;
}

export function buildSeatLabel(input: SeatLabelInput): SeatLabel {
	const stream = streamLabel(input.cwd);
	const job = slug(input.job);
	return {
		windowName: `${stream}-${job}`.slice(0, 40),
		paneTitle: `${stream} ${job} · ${input.peerId} · ${input.model ?? "default"}`.slice(0, 100),
	};
}
