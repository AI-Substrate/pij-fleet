import {
	err,
	ok,
	type Result,
	type SessionDescriptor,
	type SessionForest,
	type SessionId,
	type SessionTreeNode,
	type TreeFilters,
	type TreeProblem,
	type TreeProjectionOptions,
	type TreeSession,
} from "./types.js";

export function effectiveParent(descriptor: SessionDescriptor): SessionId | null {
	return descriptor.parentId !== undefined ? descriptor.parentId : (descriptor.spawnedBy ?? null);
}

/** The ADOPTION axis (plan 054 P3 — AC-08/WS-1, carp's split): a non-prime
 *  node with no effective parent has nobody to report or escalate to. Prime
 *  is the LEGAL root, never unadopted. Deliberately independent of the
 *  structural axis (`TreeProblem` — an orphan HAS a parent pointer) and the
 *  runtime axis (`systemState`). One predicate feeds tree nodes, list rows,
 *  and the adoption guidance surface, so "unadopted" means ONE thing. */
export function isUnadopted(descriptor: SessionDescriptor): boolean {
	return descriptor.prime !== true && effectiveParent(descriptor) === null;
}

/** Skill-facing adoption guidance (plan 054 P3 T005 — CONTENT only; the pij
 *  skill route that carries it lands in P4 4.3). One authored text so the
 *  route, future UI, and any nudge all say the same thing. Consumption
 *  contract: enumerate via `pij tree --global --json` filtering
 *  `unadopted === true` (or `pij list --json` rows' boolean); this hint is
 *  the remedy text to show alongside. */
export const ADOPTION_HINT =
	"Unadopted nodes (non-prime, no effective parent) have nobody to report or escalate to — " +
	"anomaly alerts and completion reports go nowhere. Enumerate them with " +
	"`pij tree --global --json` (filter `unadopted === true`; `pij list --json` carries the same " +
	"boolean per row). Remedy: adopt each under its real operator with " +
	"`pij link <child> --parent <parent-id>` (audited on the spine as a node-linked event). " +
	"Prevention beats repair: spawn from an identified session — export PIJ_SESSION_ID (or run " +
	"from your registered pane) so children record the true caller as parent. Prime seats are " +
	"legal roots and are never flagged.";

export function planLink(
	descriptors: readonly SessionDescriptor[],
	childId: SessionId,
	parentId: SessionId | null,
): Result<SessionDescriptor> {
	const byId = new Map(descriptors.map((descriptor) => [descriptor.id, descriptor]));
	const child = byId.get(childId);
	if (!child) return err("E-NOID", `no session '${childId}' in registry`);
	if (parentId === childId) return err("E-SELF", `session '${childId}' cannot parent itself`);
	if (parentId !== null && !byId.has(parentId)) {
		return err("E-NOID", `no parent session '${parentId}' in registry`);
	}

	const updated = { ...child, parentId };
	if (parentId !== null) {
		const visited = new Set<SessionId>();
		let cursor: SessionId | null = parentId;
		while (cursor !== null) {
			if (cursor === childId) {
				return err("E-ARG", `link '${childId}' → '${parentId}' would create a cycle`);
			}
			if (visited.has(cursor)) {
				return err("E-ARG", `parent chain for '${parentId}' already contains a cycle`);
			}
			visited.add(cursor);
			const descriptor = byId.get(cursor);
			if (!descriptor) break;
			cursor = effectiveParent(descriptor);
		}
	}

	return ok(updated);
}

export function projectSessionForest(
	sessions: readonly TreeSession[],
	options: TreeProjectionOptions = {},
): Result<SessionForest> {
	const byId = new Map<SessionId, TreeSession>();
	for (const session of sessions) byId.set(session.descriptor.id, session);

	const selected = options.selectedIds
		? new Set(options.selectedIds.filter((id) => byId.has(id)))
		: new Set(byId.keys());

	if (options.rootId !== undefined) {
		if (!byId.has(options.rootId) || !selected.has(options.rootId)) {
			return err("E-NOID", `no session '${options.rootId}' in selected tree`);
		}
		const descendants = descendantsOf(options.rootId, selected, byId);
		selected.clear();
		for (const id of descendants) selected.add(id);
	}

	const visible = new Set<SessionId>();
	for (const id of selected) {
		const session = byId.get(id);
		if (session && matchesFilters(session, options.filters)) visible.add(id);
	}

	const children = new Map<SessionId, SessionId[]>();
	const roots = new Map<SessionId, TreeProblem | undefined>();
	for (const id of visible) {
		const session = byId.get(id);
		if (!session) continue;
		const parentId = effectiveParent(session.descriptor);
		if (parentId === null) {
			roots.set(id, undefined);
			continue;
		}
		if (!byId.has(parentId)) {
			roots.set(id, "orphan");
			continue;
		}
		if (!selected.has(parentId) || !visible.has(parentId)) {
			roots.set(id, "filtered-parent");
			continue;
		}
		const siblings = children.get(parentId) ?? [];
		siblings.push(id);
		children.set(parentId, siblings);
	}
	for (const siblings of children.values()) siblings.sort(compareIds);

	const emitted = new Set<SessionId>();
	const projectedRoots: SessionTreeNode[] = [];
	for (const id of [...roots.keys()].sort(compareIds)) {
		const node = renderNode(id, roots.get(id), undefined, byId, children, emitted);
		if (node) projectedRoots.push(node);
	}

	for (const id of [...visible].sort(compareIds)) {
		if (emitted.has(id)) continue;
		const session = byId.get(id);
		if (!session) continue;
		const node = renderNode(
			id,
			"cycle",
			effectiveParent(session.descriptor) ?? undefined,
			byId,
			children,
			emitted,
		);
		if (node) projectedRoots.push(node);
	}

	return ok({ roots: projectedRoots });
}

function compareIds(a: SessionId, b: SessionId): number {
	return a.localeCompare(b);
}

function descendantsOf(
	rootId: SessionId,
	selected: ReadonlySet<SessionId>,
	byId: ReadonlyMap<SessionId, TreeSession>,
): Set<SessionId> {
	const children = new Map<SessionId, SessionId[]>();
	for (const id of selected) {
		const session = byId.get(id);
		if (!session) continue;
		const parentId = effectiveParent(session.descriptor);
		if (parentId === null || !selected.has(parentId)) continue;
		const siblings = children.get(parentId) ?? [];
		siblings.push(id);
		children.set(parentId, siblings);
	}

	const descendants = new Set<SessionId>();
	const pending = [rootId];
	while (pending.length > 0) {
		const id = pending.shift();
		if (id === undefined || descendants.has(id)) continue;
		descendants.add(id);
		for (const childId of children.get(id) ?? []) pending.push(childId);
	}
	return descendants;
}

function matchesFilters(session: TreeSession, filters: TreeFilters | undefined): boolean {
	if (!filters) return !isHistorical(session);
	const explicitHistoryFilter =
		(filters.liveness?.length ?? 0) > 0 || (filters.lifecycle?.length ?? 0) > 0;
	if (!explicitHistoryFilter) {
		if (filters.all !== true && isHistorical(session)) return false;
		// `--all` means "include the DEAD", never "include the BURIED".
		if (filters.all === true && isBuried(session)) return false;
	}
	if (filters.activity && !filters.activity.includes(session.activity)) return false;
	if (filters.liveness && !filters.liveness.includes(session.liveness)) return false;
	if (
		filters.lifecycle &&
		(session.descriptor.lifecycle === undefined ||
			!filters.lifecycle.includes(session.descriptor.lifecycle))
	) {
		return false;
	}
	return true;
}

function isHistorical(session: TreeSession): boolean {
	return (
		session.liveness === "dead" ||
		session.liveness === "dissolved" ||
		session.descriptor.lifecycle === "dissolved"
	);
}

/** BURIED — the seat was explicitly torn down. Distinct from merely DEAD: a dead
 *  seat is a corpse still lying where it fell and is real membership evidence
 *  (`--all` exists to show it, so a dead seat in a sibling worktree is not
 *  orphaned by both the tree and path containment). A buried one has been closed
 *  and is only still on disk because `archive.ts` holds terminal records for a
 *  48h post-mortem window before moving them out of the hot tier.
 *
 *  The line is drawn to AGREE WITH `list()`, which hides exactly `dissolved`
 *  (adapters/fs-registry.ts:148). That agreement is the point: when the two
 *  surfaces disagree a consumer renders a card with no row behind it, which is
 *  precisely the 2026-07-30 wall of ROLE-UNKNOWN cards — 245 of the 250 nodes
 *  `tree --all` emitted were dissolved and absent from `list`.
 *
 *  `failed` is deliberately NOT buried here even though `archive.ts` retires it
 *  on the same 48h clock: `list()` still shows failed seats, so hiding them from
 *  the tree would just invert the same defect into a row with no card. */
function isBuried(session: TreeSession): boolean {
	return session.liveness === "dissolved" || session.descriptor.lifecycle === "dissolved";
}

interface RenderFrame {
	id: SessionId;
	session: TreeSession;
	problem: TreeProblem | undefined;
	cycleTo: SessionId | undefined;
	childIds: readonly SessionId[];
	nextChildIndex: number;
	projectedChildren: SessionTreeNode[];
}

function renderNode(
	id: SessionId,
	problem: TreeProblem | undefined,
	cycleTo: SessionId | undefined,
	byId: ReadonlyMap<SessionId, TreeSession>,
	children: ReadonlyMap<SessionId, readonly SessionId[]>,
	emitted: Set<SessionId>,
): SessionTreeNode | null {
	const session = byId.get(id);
	if (!session) return null;
	emitted.add(id);
	const active = new Set<SessionId>([id]);
	const stack: RenderFrame[] = [
		{
			id,
			session,
			problem,
			cycleTo,
			childIds: children.get(id) ?? [],
			nextChildIndex: 0,
			projectedChildren: [],
		},
	];

	while (stack.length > 0) {
		const frame = stack.at(-1);
		if (!frame) break;
		const childId = frame.childIds[frame.nextChildIndex];
		if (childId !== undefined) {
			frame.nextChildIndex += 1;
			const childSession = byId.get(childId);
			if (!childSession) continue;
			if (active.has(childId)) {
				frame.projectedChildren.push(toNode(childSession, [], "cycle", childId));
				continue;
			}
			emitted.add(childId);
			active.add(childId);
			stack.push({
				id: childId,
				session: childSession,
				problem: undefined,
				cycleTo: undefined,
				childIds: children.get(childId) ?? [],
				nextChildIndex: 0,
				projectedChildren: [],
			});
			continue;
		}

		const node = toNode(frame.session, frame.projectedChildren, frame.problem, frame.cycleTo);
		stack.pop();
		active.delete(frame.id);
		const parent = stack.at(-1);
		if (!parent) return node;
		parent.projectedChildren.push(node);
	}

	return null;
}

function toNode(
	session: TreeSession,
	children: readonly SessionTreeNode[],
	problem?: TreeProblem,
	cycleTo?: SessionId,
): SessionTreeNode {
	return {
		...session.descriptor,
		planId: session.descriptor.planId ?? null,
		effectiveParentId: effectiveParent(session.descriptor),
		activity: session.activity,
		liveness: session.liveness,
		...(isUnadopted(session.descriptor) ? { unadopted: true as const } : {}),
		...(problem !== undefined ? { problem } : {}),
		...(cycleTo !== undefined ? { cycleTo } : {}),
		children,
	};
}
