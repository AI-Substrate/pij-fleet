// Remote control contract: Rust owns the allowed set; both runtimes read its fixture.

import controlContract from "../../../../crates/core/tests/fixtures/control-commands.json" with {
	type: "json",
};
import { err, ok, type Result } from "./types.js";

/** Remote commands a session will honour. `compact` runs on the long-lived
 *  ExtensionContext (autonomous from the receive watcher); `new`/`reload` are
 *  CONTROL_COMMANDS that only exist on pi's ExtensionCommandContext, so they
 *  execute via a captured command context (see PiRuntimePort.control). */
export const ALLOWED_COMMANDS = controlContract.commands as readonly AllowedCommand[];
export const COPILOT_CONTROL_REFUSAL = controlContract.copilot_refusal;

export type AllowedCommand = "compact" | ControlCommand;

/** The subset that needs a command context (cannot run from the background
 *  watcher; routed onto the captured ExtensionCommandContext). */
export const CONTROL_COMMANDS = ALLOWED_COMMANDS.filter(
	(command): command is ControlCommand => command !== "compact",
);

export type ControlCommand = "new" | "reload";

export type ControlOutcome =
	| { readonly outcome: "executed" }
	| { readonly outcome: "refused"; readonly reason: string };

/** Narrow an allow-listed command to the command-context-only subset. */
export function isControlCommand(name: AllowedCommand): name is ControlCommand {
	return (CONTROL_COMMANDS as readonly string[]).includes(name);
}

/** Validate a remote command name against the allow-list. Unknown names are
 *  rejected with E-CMD rather than executed. */
export function validateCommand(name: string): Result<AllowedCommand> {
	if ((ALLOWED_COMMANDS as readonly string[]).includes(name)) {
		return ok(name as AllowedCommand);
	}
	return err("E-CMD", `unknown command '${name}'; allowed: ${ALLOWED_COMMANDS.join(", ")}`);
}
