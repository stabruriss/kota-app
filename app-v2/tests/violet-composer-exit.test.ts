import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  syncVioletProjectAgentsNow: vi.fn(),
}));

vi.mock('../src/lib/violet-sync-engine', () => ({
  syncVioletProjectAgentsNow: mocks.syncVioletProjectAgentsNow,
}));

import type { VioletChatMessage, VioletRoomState } from '../src/pty-client';
import { reconcileVioletComposerAfterAgentExit } from '../src/chrome/violet-composer-exit';
import {
  __clearVioletComposerSentHistoryForTests,
  emitVioletComposerSent,
  VIOLET_COMPOSER_AGENT_EXIT_REASON,
  violetComposerSentHistory,
} from '../src/chrome/violet-room-events';

describe('Violet Composer delivery after an agent exits', () => {
  beforeEach(() => {
    __clearVioletComposerSentHistoryForTests();
    mocks.syncVioletProjectAgentsNow.mockReset();
  });

  it('waits for the final native-log sync before queuing an unconfirmed prompt', async () => {
    const projectRoot = '/tmp/kota-exit-unconfirmed';
    let resolveSync!: (state: VioletRoomState) => void;
    mocks.syncVioletProjectAgentsNow.mockReturnValue(new Promise((resolve) => {
      resolveSync = resolve;
    }));
    const sent = emitVioletComposerSent({
      projectRoot,
      text: 'keep this prompt safe',
      targetAgentIds: ['alice'],
      privacy: false,
    });

    const reconciliation = reconcileVioletComposerAfterAgentExit({
      projectRoot,
      agentId: 'alice',
    });
    expect(violetComposerSentHistory(projectRoot)[0]?.delivery).toBeUndefined();

    resolveSync(roomState([]));
    await reconciliation;

    expect(mocks.syncVioletProjectAgentsNow).toHaveBeenCalledWith(projectRoot, ['alice']);
    expect(violetComposerSentHistory(projectRoot)).toHaveLength(1);
    expect(violetComposerSentHistory(projectRoot)[0]?.delivery).toEqual({
      status: 'unconfirmed',
      reason: VIOLET_COMPOSER_AGENT_EXIT_REASON,
      retryTargetAgentIds: ['alice'],
    });
    expect(violetComposerSentHistory(projectRoot)[0]?.id).toBe(sent?.id);
  });

  it('retires the prompt when the final sync finds native user evidence', async () => {
    const projectRoot = '/tmp/kota-exit-confirmed';
    const sent = emitVioletComposerSent({
      projectRoot,
      text: 'native evidence exists',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    mocks.syncVioletProjectAgentsNow.mockResolvedValue(roomState([
      nativeUserMessage('alice', 'native evidence exists', offsetTimestamp(sent!.timestamp, 1_000)),
    ]));

    await reconcileVioletComposerAfterAgentExit({ projectRoot, agentId: 'alice' });

    expect(violetComposerSentHistory(projectRoot)[0]?.delivery).toEqual({ status: 'clear' });
  });

  it('queues only the exited group target after every peer is natively confirmed', async () => {
    const projectRoot = '/tmp/kota-exit-group-confirmed-peer';
    const sent = emitVioletComposerSent({
      projectRoot,
      text: 'group prompt',
      targetAgentIds: ['alice', 'bob'],
      privacy: false,
    });
    mocks.syncVioletProjectAgentsNow.mockResolvedValue(roomState([
      nativeUserMessage('bob', 'group prompt', offsetTimestamp(sent!.timestamp, 1_000)),
    ]));

    await reconcileVioletComposerAfterAgentExit({ projectRoot, agentId: 'alice' });

    expect(mocks.syncVioletProjectAgentsNow).toHaveBeenCalledWith(
      projectRoot,
      ['alice', 'bob'],
    );
    expect(violetComposerSentHistory(projectRoot)[0]?.delivery?.retryTargetAgentIds)
      .toEqual(['alice']);
  });

  it('leaves a group pending when another target still needs the ordinary timeout', async () => {
    const projectRoot = '/tmp/kota-exit-group-pending-peer';
    emitVioletComposerSent({
      projectRoot,
      text: 'group still pending',
      targetAgentIds: ['alice', 'bob'],
      privacy: false,
    });
    mocks.syncVioletProjectAgentsNow.mockResolvedValue(roomState([]));

    await reconcileVioletComposerAfterAgentExit({ projectRoot, agentId: 'alice' });

    expect(violetComposerSentHistory(projectRoot)[0]?.delivery).toBeUndefined();
  });

  it('keeps the 180-second fallback when the final sync fails', async () => {
    const projectRoot = '/tmp/kota-exit-sync-failed';
    emitVioletComposerSent({
      projectRoot,
      text: 'sync failed safely',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    mocks.syncVioletProjectAgentsNow.mockRejectedValue(new Error('Violet unavailable'));

    await expect(reconcileVioletComposerAfterAgentExit({
      projectRoot,
      agentId: 'alice',
    })).rejects.toThrow('Violet unavailable');

    expect(violetComposerSentHistory(projectRoot)[0]?.delivery).toBeUndefined();
  });
});

function roomState(messages: VioletChatMessage[]): VioletRoomState {
  return {
    messages,
    sources: [],
    workEvents: [],
    agentBusReceipts: [],
    rawLogDir: '/tmp/raw',
    chathistoryDir: '/tmp/history',
    syncedAt: '2026-07-23T00:00:00.000Z',
  };
}

function nativeUserMessage(
  agentId: string,
  text: string,
  timestamp: string,
): VioletChatMessage {
  return {
    id: `native-${agentId}`,
    sessionId: `session-${agentId}`,
    agentId,
    shell: 'codex',
    role: 'user',
    kind: 'message',
    timestamp,
    text,
    sourcePath: null,
    nativeEventId: null,
  };
}

function offsetTimestamp(timestamp: string, offsetMs: number): string {
  return new Date(Date.parse(timestamp) + offsetMs).toISOString();
}
