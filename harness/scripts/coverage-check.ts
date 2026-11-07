#!/usr/bin/env tsx
/**
 * Plan 210 (ruling C29, req-31): the workstream coverage sensor. It fails,
 * naming the row, when
 * - a PRD requirement is delivered by no workstream step (unless `dropped`);
 * - a blueprint component is built by no step (unless `dropped`);
 * - an umbrella step is `complete` while a step that is `part_of` it is not
 *   complete or abandoned (req-30; ddocs can't express this one).
 * Known gaps are listed in `coverage-exemptions.json`, each with an owner. The
 * file may only shrink: every row must be one of `APPROVED_EXEMPTIONS` below,
 * so adding or swapping an address needs a reviewed change to this file, and an
 * exemption for a row that is now covered, or that names no row, fails.
 * Inputs fail closed: a required folder, document, section or the exemptions
 * file that can't be read or has the wrong shape is a finding, never an empty
 * list. Only the old `roadmaps` folder may be absent. Reads files once; no
 * loop, poll or launch.
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

/** The filesystem reads the sensor makes, injectable for tests. */
export interface Reader {
	readdir(path: string): string[];
	readFile(path: string): string;
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
const WORKSTREAMS = "docs/project/workstreams";
/** The workstreams folder's old name (C29); the only folder allowed to be absent. */
const LEGACY_WORKSTREAMS = "docs/project/roadmaps";

/** The exemptions file next to this script. */
export const EXEMPTIONS = "harness/scripts/coverage-exemptions.json";

/**
 * The approved gaps (pm-001, plan 210 AC6; owner docs-system, plan 210 AC4).
 * `coverage-exemptions.json` may list only these. When a gap is closed, delete
 * its row here and in the JSON; never add one to hide new work.
 */
export const APPROVED_EXEMPTIONS: readonly string[] = [
	"docs/project/prds/001-pij-community.dd.json#requirements/req-26",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-01",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-02",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-03",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-04",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-05",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-06",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-07",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-08",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-09",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-10",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-11",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-12",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-13",
	"docs/project/prds/002-pij-base.dd.json#requirements/req-14",
];

const NODE_READER: Reader = {
	readdir: (path) => readdirSync(path),
	readFile: (path) => readFileSync(path, "utf8"),
};

function reason(error: unknown): string {
	if (typeof error === "object" && error !== null && "code" in error) return String(error.code);
	return error instanceof Error ? error.message : String(error);
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The rows of a required section; a missing or malformed one is a finding. */
function rows(doc: Doc, section: string, findings: string[]): readonly Row[] {
	const sections = isRecord(doc.body) ? doc.body.sections : undefined;
	if (!Array.isArray(sections)) {
		findings.push(`${doc.path}: has no sections array`);
		return [];
	}
	const entry: unknown = sections.find((item) => isRecord(item) && item.name === section);
	const value = isRecord(entry) ? entry.value : undefined;
	if (!Array.isArray(value)) {
		findings.push(
			`${doc.path}: required section ${section} is ${entry === undefined ? "missing" : "not an array"}`,
		);
		return [];
	}
	return value.filter((row, index): row is Row => {
		if (isRecord(row) && typeof row.id === "string" && row.id !== "") return true;
		findings.push(`${doc.path}#${section}[${index}]: row has no id`);
		return false;
	});
}

/** A link cell (`../prds/x.dd.json#requirements/req-01`, `#steps/s04`) as a repo-relative address. */
export function resolveLink(from: string, link: string): string {
	const hash = link.indexOf("#");
	const file = hash === -1 ? link : link.slice(0, hash);
	const fragment = hash === -1 ? "" : link.slice(hash);
	return (file === "" ? from : posix.normalize(posix.join(posix.dirname(from), file))) + fragment;
}

/**
 * The exemptions file's rows that may be applied, and a finding for each one
 * that may not: a malformed file or row, a duplicate, or an address outside
 * `approved`. A rejected row is not applied, so its gap is reported too.
 */
export function validExemptions(
	file: unknown,
	approved: readonly string[],
): { exemptions: Exemption[]; findings: string[] } {
	const findings: string[] = [];
	const list = isRecord(file) ? file.rows : undefined;
	if (!Array.isArray(list))
		return { exemptions: [], findings: [`${EXEMPTIONS}: rows is not an array`] };
	const allowed = new Set(approved);
	const seen = new Set<string>();
	const exemptions: Exemption[] = [];
	list.forEach((row: unknown, index) => {
		const field = (name: string) =>
			isRecord(row) && typeof row[name] === "string" ? row[name].trim() : "";
		const [address, owner, why] = [field("address"), field("owner"), field("why")];
		const label = address === "" ? `${EXEMPTIONS} rows[${index}]` : address;
		if (address === "" || owner === "" || why === "")
			findings.push(`${label}: exemption needs a non-empty address, owner and why`);
		else if (seen.has(address)) findings.push(`${address}: exempted twice`);
		else if (!allowed.has(address))
			findings.push(
				`${address}: exemption not in the approved baseline (APPROVED_EXEMPTIONS in coverage-check.ts); deliver it or set it dropped`,
			);
		else exemptions.push({ address, owner, why });
		if (address !== "") seen.add(address);
	});
	return { exemptions, findings };
}

/** Every coverage finding, one line each, naming the row. */
export function coverageFindings(
	prds: readonly Doc[],
	blueprints: readonly Doc[],
	workstreams: readonly Doc[],
	exemptionsFile: unknown,
	approved: readonly string[] = APPROVED_EXEMPTIONS,
): string[] {
	const findings: string[] = [];
	const delivered = new Set<string>();
	const built = new Set<string>();
	const steps = new Map<string, Row>();
	const children = new Map<string, string[]>();
	for (const doc of workstreams) {
		for (const step of rows(doc, "steps", findings)) {
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
	const obligations = [
		...prds.flatMap((doc) =>
			rows(doc, "requirements", findings).map((row) => ({
				address: `${doc.path}#requirements/${row.id}`,
				row,
				covered: delivered,
				gap: "no workstream step delivers this requirement (deliver it, or set it dropped)",
			})),
		),
		...blueprints.flatMap((doc) =>
			rows(doc, "components", findings).map((row) => ({
				address: `${doc.path}#components/${row.id}`,
				row,
				covered: built,
				gap: "no workstream step builds this component (build it, or set it dropped)",
			})),
		),
	];
	const gaps = new Set(
		obligations
			.filter(({ address, row, covered }) => row.state !== "dropped" && !covered.has(address))
			.map(({ address }) => address),
	);
	const valid = validExemptions(exemptionsFile, approved);
	findings.push(...valid.findings);
	const exempt = new Set(valid.exemptions.map((entry) => entry.address));
	for (const { address, gap } of obligations)
		if (gaps.has(address) && !exempt.has(address)) findings.push(`${address}: ${gap}`);
	const known = new Set(obligations.map(({ address }) => address));
	for (const entry of valid.exemptions) {
		if (!known.has(entry.address))
			findings.push(`${entry.address}: exempted, but no such row; remove the exemption`);
		else if (!gaps.has(entry.address))
			findings.push(
				`${entry.address}: exempted, but now covered; remove the exemption here and in APPROVED_EXEMPTIONS (owner ${entry.owner})`,
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

/**
 * Every `.dd.json` in `folder` under `root`, parsed. A folder that can't be
 * listed, a required folder with no documents, or a file that can't be read
 * or parsed is a finding. `mayBeAbsent` excuses only a missing folder (ENOENT).
 */
export function readDocs(
	root: string,
	folder: string,
	reader: Reader = NODE_READER,
	mayBeAbsent = false,
): { docs: Doc[]; findings: string[] } {
	let names: string[];
	try {
		names = reader.readdir(posix.join(root, folder));
	} catch (error) {
		if (mayBeAbsent && reason(error) === "ENOENT") return { docs: [], findings: [] };
		return { docs: [], findings: [`${folder}: cannot read this folder (${reason(error)})`] };
	}
	const docs: Doc[] = [];
	const findings: string[] = [];
	for (const name of names.filter((entry) => entry.endsWith(".dd.json")).sort()) {
		const path = `${folder}/${name}`;
		try {
			docs.push({ path, body: JSON.parse(reader.readFile(posix.join(root, path))) });
		} catch (error) {
			findings.push(`${path}: cannot read or parse (${reason(error)})`);
		}
	}
	if (!mayBeAbsent && names.every((entry) => !entry.endsWith(".dd.json")))
		findings.push(`${folder}: required folder has no .dd.json documents`);
	return { docs, findings };
}

/** The repo's findings, and how many rows it exempts. */
export function repoFindings(
	root: string,
	reader: Reader = NODE_READER,
): { findings: string[]; exempted: number } {
	const prds = readDocs(root, PRDS, reader);
	const blueprints = readDocs(root, BLUEPRINTS, reader);
	const workstreams = readDocs(root, WORKSTREAMS, reader);
	const legacy = readDocs(root, LEGACY_WORKSTREAMS, reader, true);
	let file: unknown;
	const findings = [prds, blueprints, workstreams, legacy].flatMap((read) => read.findings);
	try {
		file = JSON.parse(reader.readFile(posix.join(root, EXEMPTIONS)));
	} catch (error) {
		findings.push(`${EXEMPTIONS}: cannot read or parse (${reason(error)})`);
	}
	findings.push(
		...coverageFindings(prds.docs, blueprints.docs, [...workstreams.docs, ...legacy.docs], file),
	);
	return { findings, exempted: validExemptions(file, APPROVED_EXEMPTIONS).exemptions.length };
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
