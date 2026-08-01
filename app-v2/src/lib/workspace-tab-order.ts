import type { ProjectId } from '../types/project';

export interface ReconciledWorkspaceTabOrder {
  persistedOrder: ProjectId[];
  visibleOrder: ProjectId[];
}

export function normalizeWorkspaceTabOrder(ids: readonly unknown[]): ProjectId[] {
  const seen = new Set<ProjectId>();
  const order: ProjectId[] = [];
  for (const id of ids) {
    if (typeof id !== 'string' || id.length === 0 || seen.has(id)) continue;
    seen.add(id);
    order.push(id);
  }
  return order;
}

export function reconcileDiscoveredWorkspaceTabOrder(
  storedOrder: readonly unknown[],
  discoveredIds: readonly unknown[],
): ReconciledWorkspaceTabOrder {
  const persistedOrder = normalizeWorkspaceTabOrder(storedOrder);
  const persistedIds = new Set(persistedOrder);
  const discoveredOrder = normalizeWorkspaceTabOrder(discoveredIds);

  for (const id of discoveredOrder) {
    if (persistedIds.has(id)) continue;
    persistedIds.add(id);
    persistedOrder.push(id);
  }

  const discoveredIdSet = new Set(discoveredOrder);
  return {
    persistedOrder,
    visibleOrder: persistedOrder.filter((id) => discoveredIdSet.has(id)),
  };
}

export function appendWorkspaceTabOrder(
  storedOrder: readonly unknown[],
  projectId: unknown,
): ProjectId[] {
  return reconcileDiscoveredWorkspaceTabOrder(storedOrder, [projectId]).persistedOrder;
}

export function removeWorkspaceTabOrder(
  storedOrder: readonly unknown[],
  projectId: ProjectId,
): ProjectId[] {
  return normalizeWorkspaceTabOrder(storedOrder).filter((id) => id !== projectId);
}

export function reorderVisibleWorkspaceTabOrder(
  storedOrder: readonly unknown[],
  visibleIds: readonly unknown[],
): ProjectId[] {
  const order = normalizeWorkspaceTabOrder(storedOrder);
  const requestedVisibleOrder = normalizeWorkspaceTabOrder(visibleIds);
  const storedIds = new Set(order);
  const reorderedStoredIds = requestedVisibleOrder.filter((id) => storedIds.has(id));
  const visibleIdSet = new Set(reorderedStoredIds);
  let visibleIndex = 0;

  const nextOrder = order.map((id) => (
    visibleIdSet.has(id) ? reorderedStoredIds[visibleIndex++]! : id
  ));
  for (const id of requestedVisibleOrder) {
    if (!storedIds.has(id)) nextOrder.push(id);
  }
  return nextOrder;
}
