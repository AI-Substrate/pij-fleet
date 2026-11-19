// D-F208-1: preloaded into the extension child by copilot-sdk-contract.mjs.
//
// The extension imports `@microsoft/copilot-sdk/extension`, which the Copilot
// CLI resolves to its own installed SDK. This hook resolves it to that same
// file (PIJ_SDK_CONTRACT_SDK) through a shim that changes no shape: it copies
// the session config, wrapping each function in place so a call reports the
// config path it was found at, then hands the copy to the REAL joinSession.
// Every lookup (tools.definitions, hooks.handlers, ...) is the SDK's own, so a
// shape the SDK ignores is a callback that is never called.
//
// Facts travel on stderr, a pipe independent of the JSON-RPC stdout, so the
// parent cannot take a stdout answer as proof that earlier facts arrived. Once
// joined, the shim answers a `contract.barrier` session event with a barrier
// fact on the same stderr, after every fact written before it (R259-001).
import { registerHooks } from "node:module";
import { pathToFileURL } from "node:url";

const sdk = process.env.PIJ_SDK_CONTRACT_SDK;
if (!sdk) throw new Error("PIJ_SDK_CONTRACT_SDK is not set");
const real = JSON.stringify(pathToFileURL(sdk).href);

const shim = `
import * as real from ${real};
export * from ${real};
const say = (fact) => process.stderr.write("SDK_CONTRACT " + JSON.stringify(fact) + "\\n");
function spy(value, path) {
  if (typeof value === "function") {
    return function (...args) { say({ called: path }); return value.apply(this, args); };
  }
  if (Array.isArray(value)) {
    return value.map((item, index) => spy(item, path + "." + (typeof item?.name === "string" ? item.name : index)));
  }
  if (value && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, spy(item, path ? path + "." + key : key)]));
  }
  return value;
}
export async function joinSession(config) {
  const session = await real.joinSession(spy(config, ""));
  const on = session.on.bind(session);
  on((event) => { if (event?.type === "contract.barrier") say({ barrier: event.id }); });
  const heard = (label, handler) => (event) => { say({ event: event?.type, listener: label }); return handler(event); };
  session.on = (type, handler) => typeof type === "function" ? on(heard("*", type)) : on(type, heard(type, handler));
  // The extension registers its own listeners as soon as this returns, before it
  // can read another message.
  say({ joined: true });
  return session;
}
`;

registerHooks({
	resolve(specifier, context, next) {
		return specifier === "@microsoft/copilot-sdk/extension"
			? { url: `data:text/javascript,${encodeURIComponent(shim)}`, shortCircuit: true }
			: next(specifier, context);
	},
});
