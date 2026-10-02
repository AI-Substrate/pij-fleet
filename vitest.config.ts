import { defineConfig } from "vitest/config";

const NODE_SQLITE_SHIM = "\0node-sqlite-shim";

export default defineConfig({
	plugins: [
		{
			name: "node-sqlite-shim",
			enforce: "pre",
			resolveId(id: string) {
				if (id === "node:sqlite" || id === "sqlite") return NODE_SQLITE_SHIM;
				return undefined;
			},
			load(id: string) {
				if (id !== NODE_SQLITE_SHIM) return undefined;
				return [
					"import { createRequire } from 'node:module';",
					"const require = createRequire(import.meta.url);",
					"const sqlite = require('node:sqlite');",
					"export const DatabaseSync = sqlite.DatabaseSync;",
				].join("\n");
			},
		},
	],
	test: {
		include: [".omp/extensions/**/*.test.ts", "harness/**/*.test.ts", "skills/**/*.test.ts"],
		exclude: ["node_modules"],
		// 30s, not 5s (D-035). MEASURED, not guessed: at 5s the full suite on a
		// loaded dev box failed a DIFFERENT set of tests every run — observed 4
		// failures across 3 files, then 8 across 5, with membership varying
		// (daemon-push, worktree, core/cli, packages-bootstrap, flow-pair
		// cli-observe). Every one of them passes when its file is run alone.
		//
		// It is a BUDGET problem, not shared-state contention: raising ONLY this
		// number turns the whole suite green (198 files / 3610 tests, 0 failures)
		// and costs nothing — 95.9s vs 98.1s wall-clock, because the ceiling is
		// never reached by a healthy test. Nothing was corrupting anything; tests
		// were losing a wall-clock race against 16 parallel workers on a box that
		// also runs the fleet. Same family as the fsync note below.
		//
		// A varying red set is worse than a slow suite: it launders a real
		// regression as expected noise, and it makes "only my guard flipped"
		// undecidable for anyone proving a guard by injection. 30s still catches
		// a genuine hang.
		testTimeout: 30000,
		// Bounded BY CONSTRUCTION (DL-018, 2026-09-05). On Vitest 4 neither
		// VITEST_MAX_THREADS nor VITEST_MAX_WORKERS caps anything (dist reads the
		// CLI flag/config only), so every "capped" env-var run today fanned out to
		// CPU count on a box that also runs the fleet — load 200-380 on 16 cores and
		// a varying red set of real-binary timing tests. The gate runs through
		// `npm run test` with no way to pass --maxWorkers, so the cap lives here.
		// The cap is for the fleet box only. Hosted CI (4 vCPU) ran at vitest's
		// default before this cap existed and went green; pinning 4 there doubled
		// contention (summed test time 455s→995s) and timed out the real-binary
		// tests (main 7c97c726, PR #369). Leave CI on the default.
		...(process.env.CI ? {} : { maxWorkers: 4 }),
		// Skip physical fsync barriers in tests (adapters/atomic-file.ts
		// maybeFsyncSync): 18 fsync sites x 16 parallel workers on one disk
		// starved boot-path tests into 20s+ timeouts. Ordering/content
		// assertions are unaffected; production always fsyncs.
		env: { PIJ_TEST_NO_FSYNC: "1" },
		reporters: process.env.CI ? ["default", "github-actions"] : ["default"],
		// A fresh clone with no extensions yet (and post-demo-teardown at
		// v1 ship) has no .test.ts files. Vitest's default is exit 1 in
		// that case. Treat empty-tests as success — the harness's value is
		// the framework, not a guarantee that tests exist.
		passWithNoTests: true,
	},
});
