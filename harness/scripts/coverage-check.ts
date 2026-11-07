#!/usr/bin/env tsx
/**
 * Plan 210 (ruling C29, req-31): the workstream coverage sensor. It fails,
 * naming the row, when
 * - a PRD requirement is delivered by no workstream step (unless `dropped`);
 * - a blueprint component is built by no step (unless `dropped`);
 * - an umbrella step is `complete` while a step that is `part_of` it is not
 *   complete or abandoned (req-30; ddocs can't express this one).
 * Known gaps are listed in `coverage-exemptions.json`, each with an owner; an
 * exemption for a row that is now covered, or that names no row, also fails,
 * so the list only shrinks. Reads files once; no loop, poll or launch.
 */

import { readdirSync, readFileSync } from "node:fs";
import { posix } from "node:path";

/** A parsed `.dd.json` document and its repo-relative path. */
export interface Doc {
	readonly path: string;
	readonly body: unknown;
}

/** A known gap, kept until its owner closes it. */
export interface Exemption {
	readonly address: string;
	readonly owner: string;
	readonly why: string;
}

interface Section {
	readonly name: string;
	readonly value: unknown;
}

interface Row {
	readonly id: string;
	readonly state?: string;
	readonly delivers?: readonly string[];
	readonly builds?: readonly string[];
	readonly part_of?: string;
}

const PRDS = "docs/project/prds";
const BLUEPRINTS = "docs/project/blueprints";
/** Where workstreams live; `roadmaps` is the folder's old name (C29). */
const WORKSTREAMS = ["docs/project/workstreams", "docs/project/roadmaps"];

function rows(doc: Doc, section: string): readonly Row[] {
	const sections = (doc.body as { sections?: readonly Section[] }).sections ?? [];
	const value = sections.find((entry) => entry.name === section)?.value;
	return Array.isArray(value) ? (value as Row[]) : [];
}

/** A link cell (`../prds/x.dd.json#requirements/req-01`, `#steps/s04`) as a repo-relative address. */
export function resolveLink(from: string, link: string): string {
	const hash = link.indexOf("#");
	const file = hash === -1 ? link : link.slice(0, hash);
	const fragment = hash === -1 ? "" : link.slice(hash);
	return (file === "" ? from : posix.normalize(posix.join(posix.dirname(from), file))) + fragment;
}

/** Every coverage finding, one line each, naming the row. */
export function coverageFindings(
	prds: readonly Doc[],
	blueprints: readonly Doc[],
	workstreams: readonly Doc[],
	exemptions: readonly Exemption[],
): string[] {
	const delivered = new Set<string>();
	const built = new Set<string>();
	const steps = new Map<string, Row>();
	const children = new Map<string, string[]>();
	for (const doc of workstreams) {
		for (const step of rows(doc, "steps")) {
			const address = `${doc.path}#steps/${step.id}`;
			steps.set(address, step);
			for (const link of step.delivers ?? []) delivered.add(resolveLink(doc.path, link));
			for (const link of step.builds ?? []) built.add(resolveLink(doc.path, link));
			if (step.part_of) {
				const parent = resolveLink(doc.path, step.part_of);
				children.set(parent, [...(children.get(parent) ?? []), address]);
			}
		}
	}
	const gaps: string[] = [];
	for (const doc of prds) {
		for (const row of rows(doc, "requirements")) {
			const address = `${doc.path}#requirements/${row.id}`;
			if (row.state !== "dropped" && !delivered.has(address)) gaps.push(address);
		}
	}
	for (const doc of blueprints) {
		for (const row of rows(doc, "components")) {
			const address = `${doc.path}#components/${row.id}`;
			if (row.state !== "dropped" && !built.has(address)) gaps.push(address);
		}
	}
	const exempt = new Map(exemptions.map((entry) => [entry.address, entry]));
	const findings: string[] = [];
	for (const address of gaps) {
		if (exempt.has(address)) continue;
		findings.push(
			address.includes("#requirements/")
				? `${address}: no workstream step delivers this requirement (deliver it, or set it dropped)`
				: `${address}: no workstream step builds this component (build it, or set it dropped)`,
		);
	}
	const known = new Set([
		...prds.flatMap((doc) =>
			rows(doc, "requirements").map((row) => `${doc.path}#requirements/${row.id}`),
		),
		...blueprints.flatMap((doc) =>
			rows(doc, "components").map((row) => `${doc.path}#components/${row.id}`),
		),
	]);
	for (const entry of exemptions) {
		if (!known.has(entry.address))
			findings.push(`${entry.address}: exempted, but no such row; remove the exemption`);
		else if (!gaps.includes(entry.address))
			findings.push(
				`${entry.address}: exempted, but now covered; remove the exemption (owner ${entry.owner})`,
			);
	}
	for (const [parent, kids] of children) {
		const umbrella = steps.get(parent);
		if (umbrella === undefined) {
			for (const kid of kids) findings.push(`${kid}: part_of ${parent}, which is no step`);
			continue;
		}
		if (umbrella.state !== "complete") continue;
		for (const kid of kids) {
			const state = steps.get(kid)?.state;
			if (state !== "complete" && state !== "abandoned")
				findings.push(`${parent}: complete while its child ${kid} is ${state ?? "unknown"}`);
		}
	}
	return findings;
}

/** Every `.dd.json` in `folders` under `root`, parsed. */
export function readDocs(root: string, folders: readonly string[]): Doc[] {
	return folders.flatMap((folder) => {
		let names: string[];
		try {
			names = readdirSync(posix.join(root, folder));
		} catch {
			return [];
		}
		return names
			.filter((name) => name.endsWith(".dd.json"))
			.sort()
			.map((name) => {
				const path = `${folder}/${name}`;
				return { path, body: JSON.parse(readFileSync(posix.join(root, path), "utf8")) };
			});
	});
}

/** The exemptions file next to this script. */
export const EXEMPTIONS = "harness/scripts/coverage-exemptions.json";

/** The repo's findings, and how many rows it exempts. */
export function repoFindings(root: string): { findings: string[]; exempted: number } {
	const exemptions = (
		JSON.parse(readFileSync(posix.join(root, EXEMPTIONS), "utf8")) as { rows: Exemption[] }
	).rows;
	const findings = coverageFindings(
		readDocs(root, [PRDS]),
		readDocs(root, [BLUEPRINTS]),
		readDocs(root, WORKSTREAMS),
		exemptions,
	);
	return { findings, exempted: exemptions.length };
}

function main(): void {
	const { findings, exempted } = repoFindings(process.cwd());
	if (findings.length > 0) {
		process.stderr.write(`coverage-check: ${findings.length} finding(s)\n`);
		for (const finding of findings) process.stderr.write(`  ${finding}\n`);
		process.exitCode = 1;
		return;
	}
	process.stdout.write(
		`coverage-check: every requirement and component is covered (${exempted} exempted, each with an owner)\n`,
	);
}

if (process.argv[1]?.endsWith("coverage-check.ts")) main();
