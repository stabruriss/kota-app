import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import * as ptyClient from '../src/pty-client';
import {
  localPendingDeliveryKeySet,
  unconfirmedAgentBusTargetAgentIds,
  VioletRoomPanel,
} from '../src/chrome/VioletRoomPanel';
import {
  __clearVioletComposerSentHistoryForTests,
  emitVioletComposerSent,
} from '../src/chrome/violet-room-events';

// Kota Local (v6.2): while the Local runtime still owns a delivery, the room
// must not reach the legacy 180 s "unconfirmed" verdict for that target; once
// the delivery settles it falls back to the existing path.

const RETRY = 'Queued message — try sending now';
const LOCAL = 'agent-fixture-local';
const PTY = 'agent-fixture-pty';
const START = '2026-10-02T00:00:00.000Z';

type RoomState = ptyClient.VioletRoomState;

function roomState(projectRoot: string, overrides: Partial<RoomState> = {}): RoomState {
  return {
    messages: [],
    sources: [],
    workEvents: [],
    agentBusReceipts: [],
    rawLogDir: `${projectRoot}/project-memory/raw_logs`,
    chathistoryDir: `${projectRoot}/project-memory/chathistory`,
    syncedAt: new Date().toISOString(),
    ...overrides,
  };
}

function serveRoom(read: () => RoomState) {
  vi.spyOn(ptyClient, 'readVioletRoomCache').mockImplementation(async () => read());
  vi.spyOn(ptyClient, 'syncVioletRoom').mockImplementation(async () => read());
}

async function pushRoom(projectRoot: string, agentIds: string[], state: RoomState) {
  await act(async () => {
    window.dispatchEvent(new CustomEvent('violet://room/synced', {
      detail: { request: { projectRoot, agentIds }, state },
    }));
  });
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

function bubbleFor(text: string): HTMLElement {
  const bubble = screen.getByText(text).closest('.violet-msg');
  expect(bubble).not.toBeNull();
  return bubble as HTMLElement;
}

function localUserRecord(id: string, agentId: string, timestamp: string, text: string): ptyClient.VioletChatMessage {
  return {
    id,
    sessionId: 'sess-fixture-local',
    agentId,
    shell: 'kota-local',
    role: 'user',
    kind: 'message',
    timestamp,
    text,
    sourcePath: null,
    nativeEventId: null,
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
  __clearVioletComposerSentHistoryForTests();
});

describe('Kota Local pending deliveries in the room', () => {
  it('keeps tracking a pending Local composer message past 180 s, then falls back when it settles', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(START));
      const projectRoot = '/tmp/kota-local-pending-settles';
      const text = 'run the long build';
      const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL], privacy: false })!;
      let current = roomState(projectRoot, {
        localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
      });
      serveRoom(() => current);
      const onRetry = vi.fn();

      render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} onRetryComposerMessage={onRetry} />);
      await act(async () => {});
      await advance(181_000);

      expect(within(bubbleFor(text)).queryByRole('button', { name: RETRY })).not.toBeInTheDocument();
      expect(bubbleFor(text)).not.toHaveClass('delivery-issue');

      // Cleared / failed / abandoned: the pending entry disappears with no
      // native evidence, so the existing unconfirmed + retry path applies.
      current = roomState(projectRoot);
      await pushRoom(projectRoot, [LOCAL], current);

      fireEvent.click(within(bubbleFor(text)).getByRole('button', { name: RETRY }));
      expect(onRetry).toHaveBeenCalledWith(expect.objectContaining({ targetAgentIds: [LOCAL] }));
    } finally {
      vi.useRealTimers();
    }
  });

  it('settles back onto the existing 180 s deadline when the runtime finishes early', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(START));
      const projectRoot = '/tmp/kota-local-pending-early';
      const text = 'quick question';
      const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL], privacy: false })!;
      let current = roomState(projectRoot, {
        localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
      });
      serveRoom(() => current);

      render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} onRetryComposerMessage={vi.fn()} />);
      await act(async () => {});
      await advance(60_000);
      current = roomState(projectRoot);
      await pushRoom(projectRoot, [LOCAL], current);

      await advance(119_000);
      expect(within(bubbleFor(text)).queryByRole('button', { name: RETRY })).not.toBeInTheDocument();
      await advance(1_000);
      expect(within(bubbleFor(text)).getByRole('button', { name: RETRY })).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('completes normally once the consumed input is projected with its createdAt time', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(START));
      const projectRoot = '/tmp/kota-local-pending-consumed';
      const text = 'summarize the failing tests';
      const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL], privacy: false })!;
      let current = roomState(projectRoot, {
        localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
      });
      serveRoom(() => current);

      render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} onRetryComposerMessage={vi.fn()} />);
      await act(async () => {});
      await advance(200_000);

      // Consumed after a long tool: the projected record keeps the original
      // input time, so the existing text/time confirmation still matches.
      current = roomState(projectRoot, {
        messages: [localUserRecord('local-user-record', LOCAL, sent.timestamp, text)],
      });
      await pushRoom(projectRoot, [LOCAL], current);
      await advance(200_000);

      expect(screen.getAllByText(text)).toHaveLength(1);
      expect(screen.queryByRole('button', { name: RETRY })).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('leaves a mixed broadcast PTY target on the existing unconfirmed path', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(START));
      const projectRoot = '/tmp/kota-local-pending-mixed-unconfirmed';
      const text = 'both of you check the release notes';
      const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL, PTY], privacy: false })!;
      const current = roomState(projectRoot, {
        localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
      });
      serveRoom(() => current);
      const onRetry = vi.fn();

      render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PTY]} onRetryComposerMessage={onRetry} />);
      await act(async () => {});
      await advance(179_000);
      expect(within(bubbleFor(text)).queryByRole('button', { name: RETRY })).not.toBeInTheDocument();
      await advance(1_000);

      fireEvent.click(within(bubbleFor(text)).getByRole('button', { name: RETRY }));
      expect(onRetry).toHaveBeenCalledWith(expect.objectContaining({ targetAgentIds: [PTY] }));
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not complete a mixed broadcast while the Local target is pending, even after the PTY target confirms', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(START));
      const projectRoot = '/tmp/kota-local-pending-mixed-confirmed';
      const text = 'both of you rerun the smoke suite';
      const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL, PTY], privacy: false })!;
      const ptyEcho = {
        ...localUserRecord('pty-native-user', PTY, new Date(Date.parse(sent.timestamp) + 1_000).toISOString(), text),
        sessionId: 'pty-session',
        shell: 'codex',
      };
      let current = roomState(projectRoot, {
        messages: [ptyEcho],
        localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
      });
      serveRoom(() => current);
      const onRetry = vi.fn();

      render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PTY]} onRetryComposerMessage={onRetry} />);
      await act(async () => {});
      await advance(181_000);
      expect(screen.queryByRole('button', { name: RETRY })).not.toBeInTheDocument();

      // The Local delivery settles unconsumed; only that target is retried.
      // A subset sync exercises the external merge path.
      current = roomState(projectRoot, { messages: [ptyEcho] });
      await pushRoom(projectRoot, [LOCAL], current);

      fireEvent.click(screen.getByRole('button', { name: RETRY }));
      expect(onRetry).toHaveBeenCalledWith(expect.objectContaining({ targetAgentIds: [LOCAL] }));
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows one bubble for a cross-day composer record and its projected Local echo', async () => {
    const projectRoot = '/tmp/kota-local-cross-day';
    const original = 'pick up where we left off';
    const createdAt = '2026-10-02T00:00:00.000Z';
    // The Local input carries the composer's prepared text, so its projection
    // is shaped like a PTY native echo of a cross-day send.
    const echoed = [
      '<KOTA_TEMPORAL_GAP v="1" current_time="2026-10-02T00:00:00+00:00">',
      'It has been over 24 hours since your last completed response in this room.',
      '</KOTA_TEMPORAL_GAP>',
      original,
    ].join('\n');
    const current = roomState(projectRoot, {
      messages: [
        {
          id: 'msg-cross-day',
          sessionId: 'composer',
          agentId: 'user',
          shell: 'composer',
          role: 'user',
          kind: 'message',
          timestamp: createdAt,
          text: original,
          sourcePath: null,
          nativeEventId: 'msg-cross-day',
          targetAgentIds: [LOCAL],
          temporalGap: { currentTime: createdAt, prompt: 'gap', targetAgentIds: [LOCAL] },
        },
        localUserRecord('local-cross-day-echo', LOCAL, createdAt, echoed),
      ],
    });
    serveRoom(() => current);

    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} />);

    expect(await screen.findAllByText(original)).toHaveLength(1);
  });
});

describe('Agent Bus delivery filter for Kota Local targets', () => {
  const BUS_EVENT = 'agentbus-fixture-0001';
  const busMessage: ptyClient.VioletChatMessage = {
    id: 'bus-bubble',
    sessionId: 'actor-agent-fixture-peer',
    agentId: 'agent-fixture-peer',
    shell: 'system',
    role: 'assistant',
    kind: 'message',
    timestamp: START,
    text: 'please review the gate',
    sourcePath: null,
    nativeEventId: BUS_EVENT,
    targetAgentIds: [LOCAL, PTY],
  };
  const pendingOn = (messageId: string, targetAgentId: string) => (
    localPendingDeliveryKeySet([{ messageId, targetAgentId }])
  );

  it('keeps the existing result when nothing is pending', () => {
    expect(unconfirmedAgentBusTargetAgentIds(busMessage, [], [], [BUS_EVENT])).toEqual([LOCAL, PTY]);
    expect(unconfirmedAgentBusTargetAgentIds(
      busMessage, [], [], [BUS_EVENT], localPendingDeliveryKeySet(undefined),
    )).toEqual([LOCAL, PTY]);
  });

  it('holds back only the Local target whose delivery is still pending', () => {
    expect(unconfirmedAgentBusTargetAgentIds(
      busMessage, [], [], [BUS_EVENT], pendingOn(BUS_EVENT, LOCAL),
    )).toEqual([PTY]);
  });

  it('matches by the original nativeEventId, not by retry attempt ids', () => {
    const attempts = [BUS_EVENT, `${BUS_EVENT}-retry`];
    expect(unconfirmedAgentBusTargetAgentIds(
      busMessage, [], [], attempts, pendingOn(BUS_EVENT, LOCAL),
    )).toEqual([PTY]);
    expect(unconfirmedAgentBusTargetAgentIds(
      busMessage, [], [], attempts, pendingOn(`${BUS_EVENT}-retry`, LOCAL),
    )).toEqual([LOCAL, PTY]);
  });

  it('ignores pending entries of other messages or other targets', () => {
    const pending = localPendingDeliveryKeySet([
      { messageId: 'agentbus-fixture-other', targetAgentId: LOCAL },
      { messageId: BUS_EVENT, targetAgentId: 'agent-fixture-elsewhere' },
    ]);
    expect(unconfirmedAgentBusTargetAgentIds(busMessage, [], [], [BUS_EVENT], pending)).toEqual([LOCAL, PTY]);
  });

  it('keeps receipt confirmation, and a settled Local target falls back to unconfirmed', () => {
    const receipts = [{ eventId: BUS_EVENT, agentId: PTY, timestamp: START }];
    expect(unconfirmedAgentBusTargetAgentIds(
      busMessage, [], receipts, [BUS_EVENT], pendingOn(BUS_EVENT, LOCAL),
    )).toEqual([]);
    // Settled without consumption: the entry leaves the pending list.
    expect(unconfirmedAgentBusTargetAgentIds(
      busMessage, [], receipts, [BUS_EVENT], localPendingDeliveryKeySet([]),
    )).toEqual([LOCAL]);
  });
});
