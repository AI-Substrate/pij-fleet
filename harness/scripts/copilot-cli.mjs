// The installed Copilot CLI, as pij's harness scripts read it.
import { execFile } from "node:child_process";
import { promisify } from "node:util";

/**
 * The CLI version `copilot` (or COPILOT_BIN) runs now, or undefined when it is
 * not installed. Offline: it never checks for or downloads an update.
 * @returns {Promise<string | undefined>}
 */
export async function installedVersion() {
	const binary = process.env.COPILOT_BIN || "copilot";
	try {
		const { stdout } = await promisify(execFile)(binary, ["--version"], {
			env: { ...process.env, COPILOT_OFFLINE: "true" },
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
