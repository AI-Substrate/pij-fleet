// The installed Copilot CLI, as pij's harness scripts read it.
import { execFile } from "node:child_process";
import { promisify } from "node:util";

/**
 * The CLI version a pij seat runs, or undefined when Copilot is not installed.
 * Offline, and with auto-update off exactly as every seat launches (F271c), so
 * it reports the version built into the installed binary, never a newer
 * package another Copilot left in the shared cache.
 * @returns {Promise<string | undefined>}
 */
export async function installedVersion() {
	const binary = process.env.COPILOT_BIN || "copilot";
	try {
		const { stdout } = await promisify(execFile)(binary, ["--version"], {
			env: { ...process.env, COPILOT_OFFLINE: "true", COPILOT_AUTO_UPDATE: "false" },
			encoding: "utf8",
			timeout: 10_000,
		});
		const version = /^GitHub Copilot CLI (\S+)\.$/m.exec(stdout)?.[1];
		if (!version) throw new Error(`${binary} --version printed no version`);
		return version;
	} catch (error) {
		if (error.code === "ENOENT") return undefined;
		throw error;
	}
}
