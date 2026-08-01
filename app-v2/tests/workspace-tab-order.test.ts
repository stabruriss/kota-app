import { describe, expect, it } from 'vitest';

import {
  appendWorkspaceTabOrder,
  reconcileDiscoveredWorkspaceTabOrder,
  removeWorkspaceTabOrder,
  reorderVisibleWorkspaceTabOrder,
} from '../src/lib/workspace-tab-order';

describe('workspace tab order', () => {
  it('keeps missing project ids persisted while ordering the discovered projects', () => {
    expect(reconcileDiscoveredWorkspaceTabOrder(
      ['project-a', 'project-b', 'project-c'],
      ['project-c', 'project-a'],
    )).toEqual({
      persistedOrder: ['project-a', 'project-b', 'project-c'],
      visibleOrder: ['project-a', 'project-c'],
    });
  });

  it('appends newly discovered projects without pruning missing projects', () => {
    expect(reconcileDiscoveredWorkspaceTabOrder(
      ['project-a', 'project-b', 'project-c'],
      ['project-c', 'project-d', 'project-a'],
    )).toEqual({
      persistedOrder: ['project-a', 'project-b', 'project-c', 'project-d'],
      visibleOrder: ['project-a', 'project-c', 'project-d'],
    });
  });

  it('appends one project idempotently', () => {
    expect(appendWorkspaceTabOrder(['project-a', 'project-b'], 'project-c'))
      .toEqual(['project-a', 'project-b', 'project-c']);
    expect(appendWorkspaceTabOrder(['project-a', 'project-b'], 'project-a'))
      .toEqual(['project-a', 'project-b']);
  });

  it('removes only the explicitly removed project', () => {
    expect(removeWorkspaceTabOrder(
      ['project-a', 'temporarily-hidden', 'project-b'],
      'project-a',
    )).toEqual(['temporarily-hidden', 'project-b']);
  });

  it('reorders visible ids in their existing slots without moving hidden ids', () => {
    expect(reorderVisibleWorkspaceTabOrder(
      ['project-a', 'temporarily-hidden', 'project-b', 'project-c'],
      ['project-c', 'project-a', 'project-b'],
    )).toEqual([
      'project-c',
      'temporarily-hidden',
      'project-a',
      'project-b',
    ]);
  });

  it('appends an unpersisted visible project during reorder', () => {
    expect(reorderVisibleWorkspaceTabOrder(
      ['project-a', 'temporarily-hidden', 'project-b'],
      ['project-b', 'project-a', 'project-c'],
    )).toEqual([
      'project-b',
      'temporarily-hidden',
      'project-a',
      'project-c',
    ]);
  });
});
