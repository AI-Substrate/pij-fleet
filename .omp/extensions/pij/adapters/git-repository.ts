import { type ExecFileSyncOptions, execFileSync } from "node:child_process";
import { resolve } from "node:path";

import type { RepositoryIdentityPort } from "../core/ports.js";

export type GitRepositoryRunner = (command: string, args: readonly string[]) => string;

const EXEC_OPTS: ExecFileSyncOptions = {
	encoding: "utf8",
	stdio: ["ignore", "pipe", "pipe"],
};

function execFileRunner(command: string, args: readonly string[]): string {
	const output = execFileSync(command, args, EXEC_OPTS);
	return typeof output === "string" ? output : output.toString("utf8");
}

export class GitRepositoryAdapter implements RepositoryIdentityPort {
	constructor(private readonly run: GitRepositoryRunner = execFileRunner) {}

	gitCommonDir(folder: string): string | null {
		try {
			const output = this.run("git", [
				"-C",
				folder,
				"rev-parse",
				"--path-format=absolute",
				"--git-common-dir",
			]);
			const gitCommonDir = output.trim();
			return gitCommonDir === "" ? null : resolve(folder, gitCommonDir);
		} catch {
			return null;
		}
	}
}
