import type { RmOptions } from "node:fs";
import { describe, expect, it } from "vitest";
import { type RemoveTemporaryTree, removeTemporaryTree } from "./test-utils.js";

function enotempty(): NodeJS.ErrnoException {
	return Object.assign(new Error("injected directory repopulation"), { code: "ENOTEMPTY" });
}

function injectedNativeRemoval(failuresBeforeSuccess: number): {
	readonly remove: RemoveTemporaryTree;
	readonly attempts: () => number;
	readonly options: () => RmOptions | undefined;
} {
	let attempts = 0;
	let configuredOptions: RmOptions | undefined;
	return {
		remove: (_path, options) => {
			configuredOptions = options;
			const maxAttempts = (options.maxRetries ?? 0) + 1;
			for (let attempt = 0; attempt < maxAttempts; attempt++) {
				attempts++;
				if (attempts > failuresBeforeSuccess) return;
			}
			throw enotempty();
		},
		attempts: () => attempts,
		options: () => configuredOptions,
	};
}

describe("removeTemporaryTree", () => {
	it("configures native removal to recover after two injected ENOTEMPTY attempts", () => {
		const injected = injectedNativeRemoval(2);

		expect(() => removeTemporaryTree("/tmp/injected-transient", injected.remove)).not.toThrow();
		expect(injected.attempts()).toBe(3);
		expect(injected.options()).toMatchObject({
			recursive: true,
			force: true,
			maxRetries: 3,
			retryDelay: 100,
		});
	});

	it("does not swallow persistent ENOTEMPTY after the configured bound", () => {
		const injected = injectedNativeRemoval(Number.POSITIVE_INFINITY);

		expect(() => removeTemporaryTree("/tmp/injected-persistent", injected.remove)).toThrow(
			expect.objectContaining({ code: "ENOTEMPTY" }),
		);
		expect(injected.attempts()).toBe(4);
	});
});
