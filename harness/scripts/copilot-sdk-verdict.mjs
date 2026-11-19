// D-F208-2: what one installed-SDK contract run means for the fleet, and the
// words that say so. Pure, so CI (which has no Copilot CLI) can pin them.
//
// Copilot updates itself (17 Nov's 1.0.113 arrived that way), so the version
// mismatch below is usually the first anyone hears of an update: it names the
// fix in the words seats and doctor readers act on.

/** The step before reloading any Copilot seat onto a new CLI version. */
export const RELOAD_FIX = "run just copilot-sdk-contract before reloading seats";
/** What a failing contract means for seats: a reload loads the broken shape. */
export const DO_NOT_RELOAD = "do not reload: the SDK would drop pij_send/hooks";
const RECORD_FIX =
	"after a pass, run just copilot-sdk-contract --record and commit harness/copilot-sdk-verified.json";

/**
 * The status and summary of a contract run on `version`, given its `checks`,
 * the last recorded `verified` version and whether this run `recorded`.
 * @param {{ version: string, verified?: string, checks: Array<{ name: string, ok: boolean }>, recorded?: boolean }} run
 * @returns {{ status: "pass" | "warn" | "fail", summary: string }}
 */
export function verdict({ version, verified, checks, recorded = false }) {
	const failed = checks.filter((check) => !check.ok).map((check) => check.name);
	if (failed.length > 0)
		return {
			status: "fail",
			summary: `${DO_NOT_RELOAD}. Copilot ${version}'s SDK would not register pij's ${failed.join(", ")} (last verified ${verified ?? "never"}); compare its copilot-sdk/extension.js resumeSessionForExtension with ${verified ?? "the last good version"}'s`,
		};
	if (recorded) return { status: "pass", summary: `passed on Copilot ${version}; recorded` };
	if (verified !== version)
		return {
			status: "warn",
			summary: `Copilot is ${version}, but the last verified version is ${verified ?? "none"}: ${RELOAD_FIX}; ${RECORD_FIX}. This run passed on ${version}.`,
		};
	return { status: "pass", summary: `passed on Copilot ${version}` };
}
