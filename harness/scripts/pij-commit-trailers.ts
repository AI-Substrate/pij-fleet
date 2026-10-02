#!/usr/bin/env tsx

import { execFileSync } from "node:child_process";

interface Seat {
	id: string;
	session?: string | null;
	role?: string | null;
	parent?: string | null;
}

export interface AttributionContext {
	seats: Seat[];
	/** Fresh CLI output; guidance is only attached to that same committing seat. */
	trailers: string;
	seatId?: string;
}

interface Finding {
	sha: string;
	missing: string[];
	add: string[];
	unresolved: string[];
}

export interface AttributionReport {
	status: "pass" | "warn" | "skip";
	checked: number;
	findings: Finding[];
	unverified: string[];
	reason?: string;
}

const HARNESS = /\b(?:claude|omp|copilot|codex)\b/i;
const SESSION_KEY = /^(?:claude|omp|copilot|codex)-session(?:-id)?$/i;

function trailers(text: string): Map<string, string> {
	const values = new Map<string, string>();
	for (const line of text.split("\n")) {
		const match = /^([\w-]+):\s*(\S.*?)\s*$/.exec(line);
		if (match?.[1] && match[2]) values.set(match[1].toLowerCase(), match[2]);
	}
	return values;
}

/** Only real Git trailers count: a body example is not commit attribution. */
export function scanCommit(
	sha: string,
	author: string,
	trailerText: string,
	context: AttributionContext,
): { harness: boolean; finding?: Finding; unverified?: string } {
	const values = trailers(trailerText);
	const harness =
		HARNESS.test(author) ||
		trailerText.split("\n").some((line) => {
			const match = /^Co-Authored-By:\s*(.*)$/i.exec(line);
			return match?.[1] !== undefined && HARNESS.test(match[1]);
		}) ||
		[...values.keys()].some((key) => SESSION_KEY.test(key));
	if (!harness) return { harness: false };

	const explicitSeat = values.get("pij-seat");
	const sessions = [...values].filter(([key]) => SESSION_KEY.test(key)).map(([, value]) => value);
	const matches = context.seats.filter((seat) =>
		explicitSeat
			? seat.id === explicitSeat
			: seat.session != null && sessions.includes(seat.session),
	);
	const seat = matches.length === 1 ? matches[0] : undefined;
	const prime = values.get("pij-prime");
	const current = trailers(context.trailers);
	const isCurrent =
		context.seatId !== undefined &&
		(seat?.id === context.seatId || explicitSeat === context.seatId);
	// Prefer recorded commit attribution; absent that, registry role/root and
	// the current helper describe applicability under the interim root ruling.
	const isPrime =
		seat !== undefined &&
		(seat.id === prime ||
			(!prime && (seat.role === "prime" || seat.parent === null)) ||
			(isCurrent && current.get("pij-prime") === seat.id && !current.has("pij-seat")));
	const nonPrime = seat !== undefined && !isPrime;
	const missing = [
		...(!prime ? ["Pij-Prime"] : []),
		...(!explicitSeat && nonPrime ? ["Pij-Seat"] : []),
	];
	const add: string[] = [];
	const unresolved: string[] = [];
	for (const key of missing) {
		const value =
			key === "Pij-Seat"
				? seat?.id
				: isPrime
					? seat?.id
					: isCurrent
						? current.get("pij-prime")
						: undefined;
		if (value) add.push(`${key}: ${value}`);
		else
			unresolved.push(
				`${key}: cannot derive commit-time identity; do not substitute the current seat`,
			);
	}
	const unverified =
		!explicitSeat && !seat
			? `${sha}: Pij-Seat applicability unresolved (no unique registered native session); confirm whether the committer was the prime`
			: undefined;
	return {
		harness,
		...(missing.length ? { finding: { sha, missing, add, unresolved } } : {}),
		...(unverified ? { unverified } : {}),
	};
}

function git(root: string, args: string[]): string {
	return execFileSync("git", args, {
		cwd: root,
		encoding: "utf8",
		stdio: ["ignore", "pipe", "pipe"],
	});
}

export function scanBranch(root: string, context: AttributionContext): AttributionReport {
	let base: string;
	try {
		base = git(root, ["merge-base", "main", "HEAD"]).trim();
	} catch {
		return {
			status: "skip",
			checked: 0,
			findings: [],
			unverified: [],
			reason: "No merge-base with main; attribution was not checked",
		};
	}
	const report: AttributionReport = { status: "pass", checked: 0, findings: [], unverified: [] };
	const hashes = git(root, ["rev-list", `${base}..HEAD`]).trim();
	for (const sha of hashes ? hashes.split("\n") : []) {
		const [author = "", trailerText = ""] = git(root, [
			"show",
			"-s",
			"--format=%an <%ae>%x00%(trailers:only,unfold)",
			sha,
		]).split("\0");
		const result = scanCommit(sha, author, trailerText, context);
		if (result.harness) report.checked++;
		if (result.finding) report.findings.push(result.finding);
		if (result.unverified) report.unverified.push(result.unverified);
	}
	if (report.findings.length || report.unverified.length) report.status = "warn";
	return report;
}

function readContext(root: string): AttributionContext {
	const context: AttributionContext = { seats: [], trailers: "" };
	try {
		const registry = JSON.parse(
			execFileSync("pij-rs", ["list", "--json"], {
				cwd: root,
				encoding: "utf8",
				stdio: ["ignore", "pipe", "pipe"],
				timeout: 5000,
			}),
		) as { data?: { seats?: Seat[] } };
		context.seats = registry.data?.seats ?? [];
		context.trailers = execFileSync("pij-rs", ["commit-trailers"], {
			cwd: root,
			encoding: "utf8",
			stdio: ["ignore", "pipe", "pipe"],
			timeout: 5000,
		});
		const current = trailers(context.trailers);
		context.seatId = current.get("pij-seat") ?? current.get("pij-prime");
	} catch {
		// Registry/CLI absence cannot turn an advisory history scan into a gate.
	}
	return context;
}

function main(): void {
	let report: AttributionReport;
	try {
		report = scanBranch(process.cwd(), readContext(process.cwd()));
	} catch (error) {
		report = {
			status: "skip",
			checked: 0,
			findings: [],
			unverified: [],
			reason: `Git scan unavailable: ${String(error)}`,
		};
	}
	if (process.argv.includes("--json")) {
		console.log(JSON.stringify(report));
		return;
	}
	console.log(
		`pij-commit-trailers: ${report.status} — ${report.findings.length} missing across ${report.checked} harness commit(s) since merge-base with main`,
	);
	if (report.reason) console.log(report.reason);
	for (const finding of report.findings) {
		console.log(`${finding.sha}: missing ${finding.missing.join(", ")}`);
		for (const line of finding.add) console.log(`  ${line}`);
		for (const line of finding.unresolved) console.log(`  ${line}`);
	}
	for (const line of report.unverified) console.log(line);
	if (report.status === "warn")
		console.log(
			"Advisory only: add derived trailers after a blank line alongside harness attribution; do not rewrite shared history automatically.",
		);
}

if (process.argv[1]?.endsWith("pij-commit-trailers.ts")) main();
