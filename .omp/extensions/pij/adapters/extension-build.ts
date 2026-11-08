import { type ExecFileSyncOptionsWithStringEncoding, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { globSync, readFileSync, realpathSync } from "node:fs";
import { join, sep } from "node:path";

export interface ExtensionBuildIdentity {
	readonly extension_build: string;
	readonly extension_path: string;
}

/**
 * What the extension loads from outside its own directory, relative to it
 * (F113). Its git tree id joins the directory's in the reported identity, so
 * a change there counts as a change of the extension; the daemon stamps the
 * same roots (justfile `PIJ_EXT_TREE_OMP`), and
 * `crates/testkit/tests/extension_closure.rs` fails if the import closure
 * leaves the directory anywhere not listed here.
 */
export const OUTSIDE_ROOTS = ["../../../crates/core/tests/fixtures/control-commands.json"] as const;

/** Ten characters of each object id, joined: the identity's `tree:` part. */
function treeIdentity(ids: readonly string[]): string {
	return ids.map((id) => id.slice(0, 10)).join("+");
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
		const revParse = (roots: readonly string[]): string[] =>
			execFileSync(
				"git",
				["-C", extension_path, "rev-parse", "HEAD", "HEAD:./", ...roots.map((root) => `HEAD:./${root}`)],
				options,
			)
				.trim()
				.split("\n");
		// The commit, the directory's tree and each outside root's object id,
		// in one call; a checkout without an outside root reports the tree alone.
		let ids: string[];
		try {
			ids = revParse(OUTSIDE_ROOTS);
		} catch {
			ids = revParse([]);
		}
		const [head = "", ...trees] = ids;
		const dirty = execFileSync(
			"git",
			["-C", extension_path, "status", "--porcelain", "--", extension_path],
			options,
		).trim();
		return {
			extension_build: `${head.slice(0, 10)}${dirty ? "+dirty" : ""} tree:${treeIdentity(trees)}`,
			extension_path,
		};
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
