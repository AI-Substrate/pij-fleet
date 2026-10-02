import { execFileSync } from "node:child_process";
import {
	accessSync,
	constants,
	lstatSync,
	mkdirSync,
	readdirSync,
	readlinkSync,
	realpathSync,
	statSync,
	symlinkSync,
	unlinkSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";

const REQUIRED = [".bin/vitest", ".bin/tsc", "@types/node"];

function entry(path) {
	try {
		return lstatSync(path);
	} catch (error) {
		if (error.code === "ENOENT") return undefined;
		throw error;
	}
}

function verify(root) {
	for (const name of REQUIRED) {
		const path = join(root, name);
		const stats = statSync(path);
		if (name.startsWith(".bin/")) {
			if (!stats.isFile()) throw new Error(`${path} is not a file`);
			accessSync(path, constants.X_OK);
		} else if (!stats.isDirectory()) {
			throw new Error(`${path} is not a package directory`);
		}
	}
}

// These containers must never alias main: writes below a scope/.bin symlink
// would otherwise mutate the shared installation instead of this worktree.
function checkContainer(path) {
	const stats = entry(path);
	if (stats && (stats.isSymbolicLink() || !stats.isDirectory())) {
		throw new Error(`Refusing non-directory or symlink container: ${path}`);
	}
}

function main() {
	const args = process.argv.slice(2);
	const check = args[0] === "--check";
	if (check) args.shift();
	if (args.length > 1 || args.some((arg) => arg.startsWith("-"))) {
		throw new Error("Usage: just worktree-deps [--check] [<main-checkout>]");
	}
	const git = (...args) => execFileSync("git", args, { encoding: "utf8" });
	const target = realpathSync(git("rev-parse", "--show-toplevel").replace(/\n$/, ""));
	// -z preserves spaces, newlines and quotes; Git lists the main entry first.
	const first = git("worktree", "list", "--porcelain", "-z").split("\0", 1)[0];
	if (!first.startsWith("worktree ")) throw new Error("Cannot resolve canonical checkout");
	const canonical = realpathSync(first.slice("worktree ".length));
	if (target === canonical) {
		if (!check)
			throw new Error("Refusing to modify the canonical checkout; run in a linked worktree");
		console.log("worktree-deps: READY (canonical checkout; linked-worktree check not applicable)");
		return;
	}

	const destination = join(target, "node_modules");
	checkContainer(destination);
	if (check) {
		checkContainer(join(destination, ".bin"));
		checkContainer(join(destination, "@types"));
		verify(destination);
		console.log(`worktree-deps: READY (${REQUIRED.join(", ")} resolve)`);
		return;
	}

	const sourceCheckout = realpathSync(resolve(args[0] ?? canonical));
	if (sourceCheckout === target) throw new Error("Source and target must be different checkouts");
	const source = realpathSync(join(sourceCheckout, "node_modules"));
	if (source === destination || source.startsWith(`${destination}/`)) {
		throw new Error("Source node_modules must not be inside the target node_modules");
	}
	verify(source);

	const containers = [destination];
	const links = [];
	for (const name of readdirSync(source)) {
		if (name === "node_modules" || (name.startsWith(".") && name !== ".bin")) continue;
		const from = join(source, name);
		if (!statSync(from).isDirectory()) continue;
		if (name === ".bin" || name.startsWith("@")) {
			containers.push(join(destination, name));
			for (const child of readdirSync(from)) {
				links.push([join(from, child), join(destination, name, child)]);
			}
		} else {
			links.push([from, join(destination, name)]);
		}
	}
	// Freeze real source leaves before changing any link: a source scope/bin or
	// package can itself alias this worktree's existing links.
	for (const link of links) {
		link[0] = realpathSync(link[0]);
		if (link[0] === destination || link[0].startsWith(`${destination}/`)) {
			throw new Error(`Source package must not be inside the target node_modules: ${link[0]}`);
		}
	}

	// Validate the entire plan before creating anything; never replace real data.
	for (const path of containers) checkContainer(path);
	const existing = [];
	if (entry(destination)) {
		for (const name of readdirSync(destination)) {
			if (name.startsWith(".") && name !== ".bin") continue;
			const path = join(destination, name);
			if (name === ".bin" || name.startsWith("@")) {
				checkContainer(path);
				for (const child of readdirSync(path)) existing.push(join(path, child));
			} else {
				existing.push(path);
			}
		}
	}
	for (const to of [...existing, ...links.map(([, to]) => to)]) {
		const stats = entry(to);
		if (stats && !stats.isSymbolicLink()) throw new Error(`Refusing real package or binary: ${to}`);
	}
	for (const path of containers) mkdirSync(path, { recursive: true });
	let created = 0;
	for (const [from, to] of links) {
		if (entry(to)) {
			if (resolve(dirname(to), readlinkSync(to)) === from) continue;
			unlinkSync(to);
		}
		symlinkSync(from, to);
		created++;
	}
	verify(destination);
	console.log(
		`worktree-deps: ${links.length} links (${created} created, ${links.length - created} unchanged) from ${source}`,
	);
	console.log(`worktree-deps: READY (${REQUIRED.join(", ")} resolve)`);
}

try {
	main();
} catch (error) {
	console.error(`worktree-deps: NOT-READY — ${error.message}`);
	console.error(
		"Run `just worktree-deps` in the linked worktree; resolve any refusal without overwriting real packages. Rust-only work can continue.",
	);
	process.exitCode = 1;
}
