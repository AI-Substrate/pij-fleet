// Smoke scenario for pij. Runs via `just smoke` (or `npm run smoke -- pij`).
//
// The Driver points fixture children at a closed daemon address, never the
// live pij-rs daemon, so this proves two things against a real OMP binary:
// OMP loads this repository's extension, and the extension refuses to boot
// loudly (no fallback generation) when no daemon answers. A full live boot
// against a private daemon is a follow-up.

import type { Scenario } from "../../../harness/driver/index.js";

const scenario: Scenario = {
	name: "pij",
	steps: [
		{
			kind: "type",
			text: "/pij",
			press: "Enter",
			expect: /No pij daemon answered at 127\.0\.0\.1:1[\s\S]*pij: not booted yet/,
			expectTimeoutMs: 15000,
		},
	],
};

export default scenario;
