#!/usr/bin/env -S NODE_NO_WARNINGS=1 npm_config_loglevel=error npm_config_yes=true npx tsx
// The executable shim has one authority: the explicit rs routing inventory.
// Identity, authorization, command grammar and state belong to the daemon.
import { readFileSync } from "node:fs";
import { defaultRouterDeps, probeDecisionOnly, routeVerb } from "./adapters/generation-router.js";
import {
	describeOutcome,
	findRoute,
	type RouteOutcome,
	RS_ROUTE_TABLE,
	RS_UNSUPPORTED_STATUS,
	type RsRouteRow,
	refusalText,
	renderRsAnswer,
	routingRefusalEnvelope,
} from "./core/generation-routing.js";

const SEND_USAGE = `pij send <id> <text> [--json]
  pij send <id> --body-file <path|-> [--json]
  pij send <id> --force --reason "<why>" <text> [--json]
  --in-reply-to <msg-id> preserves reply correlation without changing the body.
  --fyi holds a message until the recipient's next turn instead of waking them; the receipt
    reads held (fyi), and adds a warning if the body looks like a question. Refused with a
    control command.
    Use \`--fyi\` only when the recipient's next action doesn't depend on it. If they'd be
    stuck, wrong or waiting without it, it's a normal send. If they'd be fine not reading it
    until their next turn, it's \`--fyi\`. If they'd be fine never reading it, don't send it.
    Always normal sends: work done or a phase complete, review verdicts, hand-offs, blockers,
    questions, decisions needed.
    FYI examples: "merged #452, nothing needed from you"; "heads-up: main moved, rebase when
    you next touch it"; progress notes nobody is waiting on.
  A cold recipient (large context, idle past its prompt-cache TTL, not working) refuses a
    waking send with E-RS-COLD-WAKE and the price; nothing is sent. Resend with --fyi if
    their next action doesn't depend on it, or add --force --reason "<why>" when the wake is
    worth that price (audited on the spine).
  The receipt shows cold-check: clear | busy | forced | unknown: <why> (unknown allows the send).
  --body-file reads literal UTF-8 text from a path or stdin, without shell evaluation.
  Backticks and $(...) substitute in YOUR shell before pij starts: quoted relay text is UNSAFE.
  Use a file or a quoted heredoc: pij send <id> --body-file - <<'PIJ'
  --file <path> (file attachment) is unsupported and refused; use --body-file for literal text.
  The daemon validates targets, flags, body conflicts and controls.`;

const LIST_USAGE = `pij list [--here] [--harness <h>] [--folder <path>] [--parent <id>] [--scope local] [--json]
  Forwards declared filters to GET /v1/seats without client-side projection.
  --here scopes to caller cwd; it is boolean, not a path argument.
  Native pij-rs list supports --here; the other query flags belong to the shim.
  --role, --prime and --archived remain unsupported and refused.`;

const INBOX_USAGE = `pij inbox register [--json]
  Register a verified external Claude/Copilot/Codex host through rs; existing panes are read back.
  pij inbox [check] [--wait [ms]] [--json]
  Paneless pull only for --wait; omit ms to wait for mail, or set a finite timeout.
  JSON is the complete v2 envelope; received claims are acknowledged only after output.`;

function routeDescription(row: RsRouteRow): string {
	const label = `pij ${row.verb}${row.leaf === undefined ? "" : ` ${row.leaf}`}`;
	if ("unported" in row) return `${label}: E-RS-UNPORTED — ${row.unported}`;
	if ("nativeCommand" in row)
		return `${label}: native pij-rs ${row.nativeCommand} (stdout, stderr and exit forwarded)`;
	return `${label}: ${row.method} ${row.rsPath} — ${row.why}`;
}

function usage(): string {
	return [
		"pij — rs-only command shim",
		"Usage: pij <verb> [arguments] [--json]",
		"HTTP --json preserves the complete validated v2 response envelope.",
		"commit-trailers preserves native output even with --json.",
		LIST_USAGE,
		INBOX_USAGE,
		"sessions accepts only bare invocation or --json; all filters are refused.",
		"pij generation [verb] [--json] — read-only routing diagnostic",
		"pij --version",
		"",
		...RS_ROUTE_TABLE.map(routeDescription),
		"",
		SEND_USAGE,
		"",
		`Unsupported status: ${RS_UNSUPPORTED_STATUS}`,
		"No command falls back to the legacy daemon or registry.",
	].join("\n");
}

function pijVersion(): string {
	try {
		const pkg = JSON.parse(readFileSync(new URL("../../../package.json", import.meta.url), "utf8"));
		return typeof pkg.version === "string" ? pkg.version : "unknown";
	} catch {
		return "unknown";
	}
}

/** A write callback, not write()'s boolean, proves the output reached the stream.
 * On an error the listener remains until Node emits it; no inbox ack can follow. */
function writeOutput(text: string): Promise<void> {
	return new Promise((resolve, reject) => {
		process.stdout.once("error", reject);
		process.stdout.write(text.endsWith("\n") ? text : `${text}\n`, (error) => {
			if (error) {
				reject(error);
				return;
			}
			process.stdout.off("error", reject);
			resolve();
		});
	});
}

async function emitRefusal(
	outcome: RouteOutcome,
	argv: readonly string[],
	rawEnvelope: string,
): Promise<void> {
	process.exitCode = 4;
	if (argv.includes("--json")) await writeOutput(rawEnvelope);
	else process.stderr.write(`${refusalText(outcome)}\n`);
}

async function runGenerationDiagnostic(argv: readonly string[]): Promise<void> {
	const verbs = argv.filter((token) => !token.startsWith("-"));
	const decisions = await probeDecisionOnly(
		verbs.length > 0 ? verbs : undefined,
		defaultRouterDeps(),
	);
	await writeOutput(
		argv.includes("--json")
			? JSON.stringify(decisions, null, 2)
			: decisions.map((decision) => decision.line).join("\n"),
	);
	if (decisions.some((decision) => decision.generation === "rs-failed")) process.exitCode = 4;
}

async function bootThroughGenerationRouting(): Promise<void> {
	const typedArgv = process.argv.slice(2);
	const verbAt = typedArgv.findIndex((arg) => arg !== "--json");
	const argv = verbAt > 0 ? [...typedArgv.slice(verbAt), ...typedArgv.slice(0, verbAt)] : typedArgv;
	const verb = argv[0] ?? "";
	if (verb === "" || verb === "--help" || verb === "-h" || verb === "help") {
		await writeOutput(usage());
		return;
	}
	if (verb === "--version" || verb === "-v" || verb === "version") {
		await writeOutput(`pij ${pijVersion()}`);
		return;
	}
	const leaf = argv[1]?.startsWith("-") ? undefined : argv[1];
	const helpRow = findRoute(verb, leaf);
	const localHelp =
		argv[1] === "--help" && (argv.length === 2 || (argv.length === 3 && argv[2] === "--json"));
	if (localHelp && helpRow !== undefined && "rsPath" in helpRow) {
		await writeOutput(
			verb === "send"
				? SEND_USAGE
				: verb === "list"
					? LIST_USAGE
					: verb === "inbox"
						? INBOX_USAGE
						: routeDescription(helpRow),
		);
		return;
	}
	let result: Awaited<ReturnType<typeof routeVerb>>;
	try {
		if (verb === "generation") {
			await runGenerationDiagnostic(argv.slice(1));
			return;
		}
		result = await routeVerb(verb, leaf, argv, defaultRouterDeps());
	} catch (error) {
		const outcome: RouteOutcome = {
			kind: "rs-error",
			verb,
			addr: process.env.PIJ_RS_ADDR ?? "",
			detail: error instanceof Error ? error.message : String(error),
		};
		await emitRefusal(outcome, argv, routingRefusalEnvelope(outcome));
		return;
	}
	const { outcome, payload, row, acknowledge, rawEnvelope } = result;
	if (process.env.PIJ_ROUTE_DIAGNOSTIC === "1")
		process.stderr.write(`pij route: ${describeOutcome(outcome)}\n`);
	if (outcome.kind === "rs-native") {
		process.exitCode = outcome.exitCode;
		return;
	}
	if (outcome.kind !== "rs") {
		await emitRefusal(outcome, argv, rawEnvelope ?? routingRefusalEnvelope(outcome));
		return;
	}
	const rendered = renderRsAnswer(row, payload, argv, rawEnvelope);
	switch (rendered.kind) {
		case "json":
		case "rendered":
			await writeOutput(rendered.text);
			break;
		case "no-renderer":
		case "json-refused": {
			const refusal: RouteOutcome = {
				kind: "rs-error",
				verb,
				addr: outcome.addr,
				detail: rendered.message,
			};
			await emitRefusal(refusal, argv, routingRefusalEnvelope(refusal));
			return;
		}
	}
	if (acknowledge !== undefined) {
		const warning = await acknowledge();
		if (warning !== undefined) process.stderr.write(`${warning}\n`);
	}
}

bootThroughGenerationRouting().catch((error: unknown) => {
	process.stderr.write(
		`E-RS: CLI output failed: ${error instanceof Error ? error.message : String(error)}\n`,
	);
	process.exitCode = 4;
});
