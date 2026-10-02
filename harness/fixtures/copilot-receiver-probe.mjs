// Test-only companion for the isolated receiver smoke: real SDK registry and connection controls.
import { existsSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { joinSession } from "@github/copilot-sdk/extension";

async function startProbe() {
	const session = await joinSession();
	const home = process.env.COPILOT_HOME ?? join(process.env.HOME, ".copilot");
	const prefix = join(home, `receiver-probe-${session.sessionId}`);
	const endpoint = new URL(readFileSync(join(home, "receiver-probe-provider.txt"), "utf8").trim());
	if (endpoint.hostname !== "127.0.0.1")
		throw Error("receiver probe requires isolated local provider");
	const models = await session.rpc.model.list();
	writeFileSync(`${prefix}.ready.json`, JSON.stringify({ session: session.sessionId, models }), {
		mode: 0o600,
	});
	for (;;) {
		await delay(200);
		if (!existsSync(`${prefix}.command.json`)) continue;
		const command = JSON.parse(readFileSync(`${prefix}.command.json`, "utf8"));
		if (!["disable-pij", "enable-pij"].includes(command.op))
			throw Error("unsupported receiver probe command");
		const list = await session.rpc.extensions.list();
		const target = list.extensions.find((extension) => extension.name === "pij");
		if (!target) throw Error("isolated native pij extension is absent");
		unlinkSync(`${prefix}.command.json`);
		writeFileSync(
			`${prefix}.requested.json`,
			JSON.stringify({ op: command.op, target, at: new Date().toISOString() }),
			{ mode: 0o600 },
		);
		if (command.op === "disable-pij") await session.rpc.extensions.disable({ id: target.id });
		else await session.rpc.extensions.enable({ id: target.id });
		writeFileSync(
			`${prefix}.result.json`,
			JSON.stringify({ op: command.op, target, at: new Date().toISOString() }),
			{ mode: 0o600 },
		);
		break;
	}
}

// Like the real extension, never hold host refresh waiting for module evaluation.
void startProbe().catch((error) => console.error(`[pij-smoke-probe] ${error.message}`));
