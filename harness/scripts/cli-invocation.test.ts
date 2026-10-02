import { describe, expect, it } from "vitest";
import { npmInvocation, npxInvocation, piInvocation } from "./cli-invocation.js";

const WINDOWS_ENV = {
	platform: "win32" as const,
	execPath: "C:\\node\\node.exe",
	npmExecPath: "C:\\node\\node_modules\\npm\\bin\\npm-cli.js",
	piCliPath: "C:\\repo\\node_modules\\@earendil-works\\pi-coding-agent\\dist\\cli.js",
};

describe("Windows CLI invocation", () => {
	it("runs Pi through Node without a cmd.exe shell", () => {
		expect(piInvocation(["install", "npm:pkg"], WINDOWS_ENV)).toEqual({
			file: WINDOWS_ENV.execPath,
			args: [WINDOWS_ENV.piCliPath, "install", "npm:pkg"],
		});
	});

	it("runs npm and npx through their JavaScript entry points", () => {
		expect(npmInvocation(["audit", "--json"], WINDOWS_ENV)).toEqual({
			file: WINDOWS_ENV.execPath,
			args: [WINDOWS_ENV.npmExecPath, "audit", "--json"],
		});
		expect(npxInvocation(["--yes", "lockfile-lint"], WINDOWS_ENV)).toEqual({
			file: WINDOWS_ENV.execPath,
			args: ["C:\\node\\node_modules\\npm\\bin\\npx-cli.js", "--yes", "lockfile-lint"],
		});
	});
});

describe("Unix CLI invocation", () => {
	it("keeps direct command execution", () => {
		const env = { platform: "linux" as const };
		expect(piInvocation(["list"], env)).toEqual({ file: "pi", args: ["list"] });
		expect(npmInvocation(["audit"], env)).toEqual({ file: "npm", args: ["audit"] });
		expect(npxInvocation(["tool"], env)).toEqual({ file: "npx", args: ["tool"] });
	});
});
