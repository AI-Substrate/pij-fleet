import { type ExecFileSyncOptionsWithStringEncoding, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { globSync, readFileSync, realpathSync } from "node:fs";
import { join, sep } from "node:path";

export interface ExtensionBuildIdentity {
	readonly extension_build: string;
	readonly extension_path: string;
}

export function computeExtensionBuildIdentity(extensionDirectory: string): ExtensionBuildIdentity {
	const extension_path = realpathSync(extensionDirectory);
	try {
		const options: ExecFileSyncOptionsWithStringEncoding = {
			encoding: "utf8",
			stdio: ["ignore", "pipe", "pipe"],
			// Review F5: load-path spawn; a wedged git must not block extension boot.
			timeout: 2_000,
		};
		const sha = execFileSync(
			"git",
			["-C", extension_path, "rev-parse", "--short=10", "HEAD"],
			options,
		).trim();
		const dirty = execFileSync(
			"git",
			["-C", extension_path, "status", "--porcelain", "--", extension_path],
			options,
		).trim();
		return { extension_build: `${sha}${dirty ? "+dirty" : ""}`, extension_path };
	} catch {
		// Standalone installs (or unavailable git metadata) identify their source bytes instead.
	}

	const paths = globSync(["index.ts", "adapters/*.ts", "core/*.ts"], { cwd: extension_path })
		.map((path) => path.split(sep).join("/"))
		.sort();
	const hash = createHash("sha256");
	for (const path of paths) {
		const bytes = readFileSync(join(extension_path, path));
		// Byte-length framing prevents filenames or source contents from blurring record boundaries.
		hash.update(`${Buffer.byteLength(path)}:`);
		hash.update(path);
		hash.update(`${bytes.length}:`);
		hash.update(bytes);
	}
	return { extension_build: `hash:${hash.digest("hex").slice(0, 12)}`, extension_path };
}
