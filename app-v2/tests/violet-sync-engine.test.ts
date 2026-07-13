import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => {
  const listeners: Array<(payload: { projectRoot: string; reason: string; paths: string[] }) => void> = [];
  return {
    listeners,
    syncVioletRoom: vi.fn(),
    onVioletRoomChanged: vi.fn(async (
      listener: (payload: { projectRoot: string; reason: string; paths: string[] }) => void,
    ) => {
      listeners.push(listener);
      return async () => {
        const index = listeners.indexOf(listener);
        if (index >= 0) listeners.splice(index, 1);
      };
    }),
  };
});

vi.mock('../src/pty-client', () => ({
  onVioletRoomChanged: mocks.onVioletRoomChanged,
  syncVioletRoom: mocks.syncVioletRoom,
}));

import {
  connectVioletProjectSyncEngine,
  requestVioletProjectAgentSync,
  type VioletProjectSyncHandle,
} from '../src/lib/violet-sync-engine';

describe('violet sync engine', () => {
  let handles: VioletProjectSyncHandle[] = [];

  beforeEach(() => {
    vi.useFakeTimers();
    handles = [];
    mocks.listeners.length = 0;
    mocks.onVioletRoomChanged.mockClear();
    mocks.syncVioletRoom.mockReset().mockResolvedValue({
      messages: [],
      sources: [],
      workEvents: [],
      rawLogDir: '',
      chathistoryDir: '',
      syncedAt: new Date().toISOString(),
    });
  });

  afterEach(() => {
    for (const handle of handles) handle.dispose();
    handles = [];
    vi.useRealTimers();
  });

  it('materializes the agent derived from a changed path instead of active working agents', async () => {
    const projectRoot = '/tmp/kota/project-a';
    const roomAgentIds = ['agent-a1b2c3d4e5', 'agent-f6g7h8i9j0'];
    const handle = connectVioletProjectSyncEngine({
      projectRoot,
      roomAgentIds,
      workingAgentIds: ['agent-z9y8x7w6v5'],
    });
    handles.push(handle);
    await vi.advanceTimersByTimeAsync(1);
    mocks.syncVioletRoom.mockClear();

    emitRoomChanged({
      projectRoot,
      reason: 'native-log-or-agent-manifest',
      paths: [`/tmp/kota/project-a/project-memory/.violet/claude-hooks/${roomAgentIds[1]}.jsonl`],
    });
    await vi.advanceTimersByTimeAsync(180);

    expect(mocks.syncVioletRoom).toHaveBeenCalledWith({
      projectRoot,
      limit: 100,
      agentIds: [roomAgentIds[1]],
      watchAgentIds: roomAgentIds,
    });
  });

  it('falls back to room agents when changed paths cannot identify an agent', async () => {
    const projectRoot = '/tmp/kota/project-b';
    const roomAgentIds = ['agent-a1b2c3d4e5', 'agent-f6g7h8i9j0'];
    const handle = connectVioletProjectSyncEngine({
      projectRoot,
      roomAgentIds,
      workingAgentIds: ['agent-z9y8x7w6v5'],
    });
    handles.push(handle);
    await vi.advanceTimersByTimeAsync(1);
    mocks.syncVioletRoom.mockClear();

    emitRoomChanged({
      projectRoot,
      reason: 'native-log-or-agent-manifest',
      paths: ['/Users/example/.codex/sessions/2026/06/05/rollout.jsonl'],
    });
    await vi.advanceTimersByTimeAsync(180);

    expect(mocks.syncVioletRoom).toHaveBeenCalledWith({
      projectRoot,
      limit: 100,
      agentIds: roomAgentIds,
      watchAgentIds: roomAgentIds,
    });
  });

  it('can request a one-shot sync for a relaunched agent', async () => {
    const projectRoot = '/tmp/kota/project-c';
    const roomAgentIds = ['agent-a1b2c3d4e5', 'agent-f6g7h8i9j0'];
    const handle = connectVioletProjectSyncEngine({
      projectRoot,
      roomAgentIds,
      workingAgentIds: [],
    });
    handles.push(handle);
    await vi.advanceTimersByTimeAsync(1);
    mocks.syncVioletRoom.mockClear();

    requestVioletProjectAgentSync(projectRoot, [roomAgentIds[0]], roomAgentIds);
    await vi.advanceTimersByTimeAsync(0);

    expect(mocks.syncVioletRoom).toHaveBeenCalledWith({
      projectRoot,
      limit: 100,
      agentIds: [roomAgentIds[0]],
      watchAgentIds: roomAgentIds,
    });
  });
});

function emitRoomChanged(payload: { projectRoot: string; reason: string; paths: string[] }) {
  for (const listener of [...mocks.listeners]) listener(payload);
}
