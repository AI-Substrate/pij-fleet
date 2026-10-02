import { posix, win32 } from "node:path";

export interface CliInvocation {
	file: string;
	args: string[];
}

interface InvocationEnvironment {
	platform?: NodeJS.Platform;
	execPath?: string;
	npmExecPath?: string;
	piCliPath?: string;
}

function environment(overrides: InvocationEnvironment): Required<InvocationEnvironment> {
	const platform = overrides.platform ?? process.platform;
	const paths = platform === "win32" ? win32 : posix;
	const execPath = overrides.execPath ?? process.execPath;
	const npmExecPath =
		overrides.npmExecPath ??
		process.env.npm_execpath ??
		paths.join(paths.dirname(execPath), "node_modules", "npm", "bin", "npm-cli.js");
	return {
		platform,
		execPath,
		npmExecPath,
		piCliPath:
			overrides.piCliPath ??
			paths.resolve(
				import.meta.dirname,
				"..",
				"..",
				"node_modules",
				"@earendil-works",
				"pi-coding-agent",
				"dist",
				"cli.js",
			),
	};
}

export function piInvocation(
	args: readonly string[],
	overrides: InvocationEnvironment = {},
): CliInvocation {
	const env = environment(overrides);
	if (env.platform === "win32") {
		return { file: env.execPath, args: [env.piCliPath, ...args] };
	}
	return { file: "pi", args: [...args] };
}

export function npmInvocation(
	args: readonly string[],
	overrides: InvocationEnvironment = {},
): CliInvocation {
	const env = environment(overrides);
	if (env.platform === "win32") {
		return { file: env.execPath, args: [env.npmExecPath, ...args] };
	}
	return { file: "npm", args: [...args] };
}

export function npxInvocation(
	args: readonly string[],
	overrides: InvocationEnvironment = {},
): CliInvocation {
	const env = environment(overrides);
	if (env.platform === "win32") {
		const npxCli = win32.join(win32.dirname(env.npmExecPath), "npx-cli.js");
		return {
			file: env.execPath,
			args: [npxCli, ...args],
		};
	}
	return { file: "npx", args: [...args] };
}
