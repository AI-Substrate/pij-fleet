import { spawnSync } from "node:child_process";
import {
	assertQuarantineEnforceableOrExit,
	npmResolutionEnvironment,
	rootLockReplayEnvironment,
} from "./release-age-policy.js";

const ROOT_LOCK_MODE = "--root-lock";
const input = process.argv.slice(2);
const rootLockMode = input[0] === ROOT_LOCK_MODE;
const [command, ...args] = rootLockMode ? input.slice(1) : input;

if (!command) {
	console.error("usage: npm-resolution-run.ts [--root-lock] <command> [args...]");
	process.exit(2);
}

// Governed installs enforce the seven-day age gate via npm-native
// min-release-age, available from npm 11.10.0. Older npm releases are refused.
// Root-lock replay explicitly clears the age and is exempt.
if (!rootLockMode) {
	assertQuarantineEnforceableOrExit({
		fail: (message) => {
			console.error(`npm-resolution-run: ${message}`);
			process.exit(1);
		},
	});
}

const result = spawnSync(command, args, {
	env: rootLockMode ? rootLockReplayEnvironment() : npmResolutionEnvironment(),
	stdio: "inherit",
});

if (result.error) {
	console.error(`npm-resolution-run: ${result.error.message}`);
	process.exit(1);
}
if (result.signal) {
	console.error(`npm-resolution-run: child terminated by ${result.signal}`);
	process.exit(1);
}

process.exit(result.status ?? 1);
