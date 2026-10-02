import { describe, expect, it } from "vitest";
import { publishSeatName } from "./session-name.js";

const SELF = "pij-exceptional-planarian";

describe("publishSeatName", () => {
	it("retries until readback matches, then stops", async () => {
		let attempts = 0;
		let current: string | undefined;
		const sleeps: number[] = [];
		const statuses: Array<[string, string]> = [];
		const notices: string[] = [];

		const result = await publishSeatName(
			{
				setSessionName: async (name) => {
					attempts += 1;
					if (attempts === 3) current = name;
				},
				getSessionName: () => current,
				setStatus: (key, text) => statuses.push([key, text]),
				notice: (text) => notices.push(text),
				sleep: async (ms) => {
					sleeps.push(ms);
				},
			},
			SELF,
			"rs",
			[0, 250, 1_000, 3_000],
		);

		expect(result).toEqual({ took: true, attempts: 3 });
		expect(sleeps).toEqual([250, 750]);
		expect(statuses).toEqual([["pij", `\u001b[33mrs\u001b[0m ${SELF}`]]);
		expect(notices).toEqual([]);
	});

	it("still performs one publication when readback already matches", async () => {
		let attempts = 0;
		const sleeps: number[] = [];

		const result = await publishSeatName(
			{
				setSessionName: async () => {
					attempts += 1;
				},
				getSessionName: () => `rs·${SELF}`,
				setStatus: () => {},
				notice: () => {},
				sleep: async (ms) => {
					sleeps.push(ms);
				},
			},
			SELF,
			"rs",
			[0, 250],
		);

		expect(result).toEqual({ took: true, attempts: 1 });
		expect(attempts).toBe(1);
		expect(sleeps).toEqual([]);
	});

	it("sets fallback status and emits one honest notice after bounded failure", async () => {
		const statuses: Array<[string, string]> = [];
		const notices: string[] = [];
		const sleeps: number[] = [];

		const result = await publishSeatName(
			{
				setSessionName: async () => {},
				getSessionName: () => undefined,
				setStatus: (key, text) => statuses.push([key, text]),
				notice: (text) => notices.push(text),
				sleep: async (ms) => {
					sleeps.push(ms);
				},
			},
			SELF,
			"rs",
			[0, 250, 1_000],
		);

		expect(result).toEqual({ took: false, attempts: 3 });
		expect(sleeps).toEqual([250, 750]);
		expect(statuses).toEqual([["pij", `\u001b[33mrs\u001b[0m ${SELF}`]]);
		expect(notices).toEqual([`pij: could not set omp session name — id is ${SELF}`]);
	});

	it("treats a resolved blind publication as successful without a false notice", async () => {
		const notices: string[] = [];
		let attempts = 0;

		const result = await publishSeatName(
			{
				setSessionName: async () => {
					attempts += 1;
				},
				setStatus: () => {},
				notice: (text) => notices.push(text),
				sleep: async () => {},
			},
			SELF,
			"rs",
			[0, 0],
		);

		expect(result).toEqual({ took: true, attempts: 2 });
		expect(attempts).toBe(2);
		expect(notices).toEqual([]);
	});

	it("continues native publication when the fallback status surface throws", async () => {
		let current: string | undefined;
		const result = await publishSeatName(
			{
				setSessionName: async (name) => {
					current = name;
				},
				getSessionName: () => current,
				setStatus: () => {
					throw new Error("status unavailable");
				},
				notice: () => {},
				sleep: async () => {},
			},
			SELF,
			"rs",
			[0],
		);

		expect(result).toEqual({ took: true, attempts: 1 });
	});

	it("stops instead of collapsing the schedule when the retry timer fails", async () => {
		let attempts = 0;
		const notices: string[] = [];

		const result = await publishSeatName(
			{
				setSessionName: async () => {
					attempts += 1;
				},
				getSessionName: () => undefined,
				setStatus: () => {},
				notice: (text) => notices.push(text),
				sleep: async () => {
					throw new Error("timer unavailable");
				},
			},
			SELF,
			"rs",
			[0, 250, 1_000],
		);

		expect(result).toEqual({ took: false, attempts: 1 });
		expect(attempts).toBe(1);
		expect(notices).toHaveLength(1);
	});
	it("keeps the dirty extension build visible in the native name and fallback status", async () => {
		let current: string | undefined;
		const statuses: Array<[string, string]> = [];

		await publishSeatName(
			{
				setSessionName: async (name) => {
					current = name;
				},
				getSessionName: () => current,
				setStatus: (key, text) => statuses.push([key, text]),
				notice: () => {},
				sleep: async () => {},
			},
			SELF,
			"rs",
			[0],
			"0123456789+dirty",
		);

		expect(current).toBe(`rs·${SELF} · ext 0123456789+dirty`);
		expect(statuses).toEqual([["pij", `\u001b[33mrs\u001b[0m ${SELF} · ext 0123456789+dirty`]]);
	});
	it("renders the legacy generation honestly", async () => {
		let current: string | undefined;
		const statuses: Array<[string, string]> = [];

		await publishSeatName(
			{
				setSessionName: async (name) => {
					current = name;
				},
				getSessionName: () => current,
				setStatus: (key, text) => statuses.push([key, text]),
				notice: () => {},
				sleep: async () => {},
			},
			SELF,
			"legacy",
			[0],
		);

		expect(current).toBe(`legacy·${SELF}`);
		expect(statuses).toEqual([["pij", `\u001b[2mlegacy\u001b[0m ${SELF}`]]);
	});
});
