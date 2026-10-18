import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import * as ptyClient from '../src/pty-client';
import { VioletRoomPanel } from '../src/chrome/VioletRoomPanel';
import {
  __clearVioletComposerSentHistoryForTests,
  emitVioletComposerSent,
  emitVioletComposerDelivery,
  violetComposerSentHistory,
} from '../src/chrome/violet-room-events';
import { localQueueGuardKey } from '../src/lib/local-queue';
import type { Agent } from '../src/types/scene';

// Counts bubble renders: every VioletMessageBubble render parses its text once.
const quoteParses = vi.hoisted(() => [] as string[]);
vi.mock('../src/lib/room-quote', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/lib/room-quote')>();
  return {
    ...actual,
    parseRoomQuotePrompt: (...args: Parameters<typeof actual.parseRoomQuotePrompt>) => {
      quoteParses.push(args[0]);
      return actual.parseRoomQuotePrompt(...args);
    },
  };
});

// Kota Local M1 D3: a message whose delivery a Local runtime still owns shows
// the v5 queue icon outside the bubble; it opens that agent's workbench queue.

const LOCAL = 'agent-fixture-local';
const OTHER_LOCAL = 'agent-fixture-local-peer';
const PTY = 'agent-fixture-pty';
const PEER = 'agent-fixture-peer';
const RETRY = 'Queued message — try sending now';
const QUEUE = 'Queued for Aster. Open message queue';
const START = '2026-10-03T00:00:00.000Z';

const agentMeta: Record<string, Agent> = {
  [LOCAL]: { name: 'Aster', emoji: '', role: 'agent', hue: '#8e897f', provider: 'kota-local' },
  [OTHER_LOCAL]: { name: 'Juniper', emoji: '', role: 'agent', hue: '#8e897f', provider: 'kota-local' },
  [PTY]: { name: 'Rowan', emoji: '', role: 'agent', hue: '#8e897f', provider: 'codex' },
};

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
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

function bubbleFor(text: string): HTMLElement {
  const bubble = screen.getByText(text).closest('.violet-msg');
  expect(bubble).not.toBeNull();
  return bubble as HTMLElement;
}

function busBubble(id: string, eventId: string, text: string): ptyClient.VioletChatMessage {
  return {
    id,
    sessionId: `actor-${PEER}`,
    agentId: PEER,
    shell: 'system',
    role: 'assistant',
    kind: 'message',
    timestamp: START,
    text,
    sourcePath: null,
    nativeEventId: eventId,
    targetAgentIds: [LOCAL],
  };
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] });
  vi.setSystemTime(new Date(START));
  __clearVioletComposerSentHistoryForTests();
});

afterEach(() => vi.useRealTimers());

describe('Kota Local queue icon in the room', () => {
  it('hides an existing protected clock by target while preserving its backing failure and Terminal retry', async () => {
    const projectRoot = '/tmp/fixture-queue-guard';
    const text = 'Both targets keep their own delivery status';
    const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL, PTY], privacy: false })!;
    emitVioletComposerDelivery({ id: sent.id, status: 'failed', reason: 'Original failure', retryTargetAgentIds: [LOCAL, PTY] });
    serveRoom(() => roomState(projectRoot));
    const onRetry = vi.fn().mockResolvedValue(false);
    const props = { projectRoot, agentIds: [LOCAL, PTY], agentMeta, onRetryComposerMessage: onRetry, onOpenLocalWorkbench: vi.fn() };
    const view = render(<VioletRoomPanel {...props} />);
    await act(async () => {});
    const backing = structuredClone(violetComposerSentHistory(projectRoot));
    const keys = new Set([localQueueGuardKey(projectRoot, sent.id, LOCAL)]);
    view.rerender(<VioletRoomPanel {...props} localQueueGuards={keys} />);
    await act(async () => {});
    expect(violetComposerSentHistory(projectRoot)).toEqual(backing);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await act(async () => fireEvent.click(screen.getByRole('button', { name: RETRY })));
    expect(onRetry).toHaveBeenCalledWith(expect.objectContaining({ targetAgentIds: [PTY] }));
    const protectedBoth = new Set([localQueueGuardKey(projectRoot, sent.id, LOCAL), localQueueGuardKey(projectRoot, sent.id, PTY)]);
    view.rerender(<VioletRoomPanel {...props} localQueueGuards={protectedBoth} />);
    await act(async () => {});
    expect(screen.queryByRole('button', { name: RETRY })).not.toBeInTheDocument();
    expect(bubbleFor(text)).toBeInTheDocument();
    view.rerender(<VioletRoomPanel {...props} localQueueGuards={new Set()} />);
    await act(async () => {});
    expect(screen.getByRole('button', { name: RETRY })).toBeInTheDocument();
  });

  it('waits 500 ms before showing the queue gutter and drops it immediately once consumed', async () => {
    const projectRoot = '/tmp/kota-d3-composer';
    const text = 'check the narrow layout';
    const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL], privacy: false })!;
    let current = roomState(projectRoot, {
      localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
    });
    serveRoom(() => current);

    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});

    await advance(499);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    expect(bubbleFor(text)).not.toHaveClass('local-pending');
    expect(bubbleFor(text).querySelector('.violet-msg-local-gutter')).toBeNull();
    await advance(1);

    const bubble = bubbleFor(text);
    const icon = within(bubble).getByRole('button', { name: QUEUE });
    expect(bubble).toHaveClass('local-pending');
    expect(icon.closest('.violet-msg-content')).toBeNull();
    expect(icon).toHaveTextContent('Queued for Aster');

    // Consumed: the projected kota.user record arrives and the entry leaves the list.
    current = roomState(projectRoot, {
      messages: [{
        id: 'local-user-record',
        sessionId: 'sess-fixture-local',
        agentId: LOCAL,
        shell: 'kota-local',
        role: 'user',
        kind: 'message',
        timestamp: sent.timestamp,
        text,
        sourcePath: null,
        nativeEventId: `${sent.id}#${LOCAL}`,
      }],
    });
    await pushRoom(projectRoot, [LOCAL], current);

    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    expect(bubbleFor(text)).not.toHaveClass('local-pending');
  });

  it('never flashes for a short pending delivery and restarts the delay for a later retry', async () => {
    const projectRoot = '/tmp/kota-d3-short';
    const message = busBubble('bus-short', 'agentbus-fixture-short', 'quickly consumed input');
    const pending = roomState(projectRoot, {
      messages: [message],
      localPendingDeliveries: [{ messageId: message.nativeEventId!, targetAgentId: LOCAL }],
    });
    serveRoom(() => pending);
    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});
    await advance(250);
    await pushRoom(projectRoot, [LOCAL, PEER], roomState(projectRoot, { messages: [message] }));
    await advance(500);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    expect(bubbleFor(message.text)).not.toHaveClass('local-pending');

    await pushRoom(projectRoot, [LOCAL, PEER], pending);
    await advance(499);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await advance(1);
    expect(screen.getByRole('button', { name: QUEUE })).toBeInTheDocument();
  });

  it('rechecks the current pending cache at 500 ms instead of flashing an already consumed delivery', async () => {
    const projectRoot = '/tmp/kota-d3-stale-cache';
    const message = busBubble('bus-fast', 'agentbus-fixture-fast', 'already consumed behind a stale room snapshot');
    let current = roomState(projectRoot, {
      messages: [message], localPendingDeliveries: [{ messageId: message.nativeEventId!, targetAgentId: LOCAL }],
    });
    serveRoom(() => current);
    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});
    // Native consumption is fast, but the throttled room/synced event has not arrived.
    await advance(40);
    current = roomState(projectRoot, { messages: [message] });
    await advance(460);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    expect(bubbleFor(message.text)).not.toHaveClass('local-pending');
  });

  it('shares one fresh check across simultaneous targets and fences its reply after removal', async () => {
    const projectRoot = '/tmp/kota-d3-delayed-proof';
    const message = { ...busBubble('bus-proof', 'agentbus-fixture-proof', 'waiting for fresh proof'), targetAgentIds: [LOCAL, OTHER_LOCAL] };
    const pending = roomState(projectRoot, { messages: [message], localPendingDeliveries: [
      { messageId: message.nativeEventId!, targetAgentId: LOCAL },
      { messageId: message.nativeEventId!, targetAgentId: OTHER_LOCAL },
    ] });
    serveRoom(() => pending);
    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, OTHER_LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});
    let resolve!: (value: RoomState) => void;
    const read = vi.mocked(ptyClient.readVioletRoomCache).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    await advance(500);
    expect(read).toHaveBeenCalledTimes(2); // Initial room + one shared limit-1 verification.
    expect(read).toHaveBeenLastCalledWith({ projectRoot, limit: 1 });
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await pushRoom(projectRoot, [LOCAL, OTHER_LOCAL, PEER], roomState(projectRoot, { messages: [message] }));
    await act(async () => resolve(pending));
    expect(screen.queryByRole('button', { name: /Queued for/ })).not.toBeInTheDocument();
  });

  it('discards a verification reply from the previous project', async () => {
    const firstProject = '/tmp/kota-d3-proof-one';
    const secondProject = '/tmp/kota-d3-proof-two';
    const message = busBubble('bus-proof-project', 'agentbus-fixture-project-proof', 'project-scoped proof');
    const state = (projectRoot: string) => roomState(projectRoot, { messages: [message],
      localPendingDeliveries: [{ messageId: message.nativeEventId!, targetAgentId: LOCAL }] });
    let current = state(firstProject);
    serveRoom(() => current);
    const panel = (projectRoot: string) => <VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />;
    const { rerender } = render(panel(firstProject));
    await act(async () => {});
    let resolve!: (value: RoomState) => void;
    vi.mocked(ptyClient.readVioletRoomCache).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    await advance(500);
    current = state(secondProject); rerender(panel(secondProject));
    await act(async () => {});
    await act(async () => resolve(state(firstProject)));
    await advance(499);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await advance(1);
    expect(screen.getByRole('button', { name: QUEUE })).toBeInTheDocument();
  });

  it('keeps a failed verification neutral without changing delivery tracking or a later retry', async () => {
    const projectRoot = '/tmp/kota-d3-proof-failure';
    const message = busBubble('bus-failed-proof', 'agentbus-fixture-failed-proof', 'a real pending input');
    const pending = roomState(projectRoot, { messages: [message],
      localPendingDeliveries: [{ messageId: message.nativeEventId!, targetAgentId: LOCAL }] });
    serveRoom(() => pending);
    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});
    vi.mocked(ptyClient.readVioletRoomCache).mockRejectedValueOnce(new Error('fixture cache unavailable'));
    await advance(500);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await pushRoom(projectRoot, [LOCAL, PEER], roomState(projectRoot, { messages: [message] }));
    await pushRoom(projectRoot, [LOCAL, PEER], pending);
    await advance(500);
    expect(screen.getByRole('button', { name: QUEUE })).toBeInTheDocument();
  });

  it('times each message and target independently without restarting on room refresh', async () => {
    const projectRoot = '/tmp/kota-d3-independent';
    const messages = [
      { ...busBubble('bus-first', 'agentbus-fixture-first', 'first input'), targetAgentIds: [LOCAL, OTHER_LOCAL] },
      busBubble('bus-second', 'agentbus-fixture-second', 'second input'),
    ];
    const agentIds = [LOCAL, OTHER_LOCAL, PEER];
    const first = { messageId: 'agentbus-fixture-first', targetAgentId: LOCAL };
    let current = roomState(projectRoot, { messages, localPendingDeliveries: [first] });
    serveRoom(() => current);
    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={agentIds} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});
    await advance(250);
    current = roomState(projectRoot, {
      messages,
      localPendingDeliveries: [first,
        { messageId: 'agentbus-fixture-first', targetAgentId: OTHER_LOCAL },
        { messageId: 'agentbus-fixture-second', targetAgentId: LOCAL },
      ],
    });
    await pushRoom(projectRoot, agentIds, current);
    await advance(250);
    expect(within(bubbleFor('first input')).getByRole('button', { name: QUEUE })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Queued for Juniper/ })).not.toBeInTheDocument();
    expect(within(bubbleFor('second input')).queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await advance(249);
    expect(screen.queryByRole('button', { name: /Queued for Juniper/ })).not.toBeInTheDocument();
    await advance(1);
    expect(within(bubbleFor('first input')).getByRole('button', { name: /Queued for Juniper/ })).toBeInTheDocument();
    expect(within(bubbleFor('second input')).getByRole('button', { name: QUEUE })).toBeInTheDocument();
  });

  it('discards the old project timer and visibility when the same identity appears in another project', async () => {
    const firstProject = '/tmp/kota-d3-project-one';
    const secondProject = '/tmp/kota-d3-project-two';
    const message = busBubble('bus-shared', 'agentbus-fixture-shared', 'project-scoped input');
    const state = (projectRoot: string) => roomState(projectRoot, {
      messages: [message],
      localPendingDeliveries: [{ messageId: message.nativeEventId!, targetAgentId: LOCAL }],
    });
    let current = state(firstProject);
    serveRoom(() => current);
    const panel = (projectRoot: string) => <VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />;
    const { rerender, unmount } = render(panel(firstProject));
    await act(async () => {});
    await advance(250);
    current = state(secondProject);
    rerender(panel(secondProject));
    await act(async () => {});
    await advance(499);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    await advance(1);
    expect(screen.getByRole('button', { name: QUEUE })).toBeInTheDocument();

    current = state(firstProject);
    rerender(panel(firstProject));
    await act(async () => {});
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    unmount();
    await advance(500);
    expect(screen.queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
  });

  it('opens that agent workbench at its queue', async () => {
    const projectRoot = '/tmp/kota-d3-click';
    const text = 'open my queue';
    const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL], privacy: false })!;
    serveRoom(() => roomState(projectRoot, {
      localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
    }));
    const onOpenLocalWorkbench = vi.fn();

    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} agentMeta={agentMeta} onOpenLocalWorkbench={onOpenLocalWorkbench} />);
    await act(async () => {});
    await advance(500);
    fireEvent.click(within(bubbleFor(text)).getByRole('button', { name: QUEUE }));

    expect(onOpenLocalWorkbench).toHaveBeenCalledTimes(1);
    expect(onOpenLocalWorkbench).toHaveBeenCalledWith(LOCAL, 'queue');
  });

  it('keeps a terminal target retry visible beside the Local queue icon', async () => {
    vi.useFakeTimers();
    try {
      vi.setSystemTime(new Date(START));
      const projectRoot = '/tmp/kota-d3-mixed';
      const text = 'both of you compare the builds';
      const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL, PTY], privacy: false })!;
      serveRoom(() => roomState(projectRoot, {
        localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
      }));
      const onRetry = vi.fn();

      render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PTY]} agentMeta={agentMeta} onRetryComposerMessage={onRetry} onOpenLocalWorkbench={vi.fn()} />);
      await act(async () => {});
      expect(within(bubbleFor(text)).queryByRole('button', { name: RETRY })).not.toBeInTheDocument();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(180_000);
      });

      const bubble = bubbleFor(text);
      const gutter = bubble.querySelector('.violet-msg-local-gutter') as HTMLElement;
      expect(gutter).not.toBeNull();
      const retry = within(gutter).getByRole('button', { name: RETRY });
      expect(within(gutter).getByRole('button', { name: QUEUE })).toBeInTheDocument();
      expect(bubble).toHaveClass('delivery-issue');
      await act(async () => { fireEvent.click(retry); });
      expect(onRetry).toHaveBeenCalledWith(expect.objectContaining({ targetAgentIds: [PTY] }));
    } finally {
      vi.useRealTimers();
    }
  });

  it('marks a pending Agent Bus bubble by its original event id only', async () => {
    const projectRoot = '/tmp/kota-d3-bus';
    serveRoom(() => roomState(projectRoot, {
      messages: [
        busBubble('bus-pending', 'agentbus-fixture-0001', 'please review the gate'),
        busBubble('bus-delivered', 'agentbus-fixture-0002', 'thanks for the review'),
      ],
      localPendingDeliveries: [{ messageId: 'agentbus-fixture-0001', targetAgentId: LOCAL }],
    }));

    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);

    await act(async () => {});
    await advance(500);
    const pending = bubbleFor('please review the gate');
    expect(within(pending).getByRole('button', { name: QUEUE })).toBeInTheDocument();
    expect(pending).toHaveClass('local-pending');
    const delivered = bubbleFor('thanks for the review');
    expect(within(delivered).queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
    expect(delivered).not.toHaveClass('local-pending');
  });

  it('shows a plain icon when no workbench callback is wired', async () => {
    const projectRoot = '/tmp/kota-d3-plain';
    const text = 'no workbench yet';
    const sent = emitVioletComposerSent({ projectRoot, text, targetAgentIds: [LOCAL], privacy: false })!;
    serveRoom(() => roomState(projectRoot, {
      localPendingDeliveries: [{ messageId: sent.id, targetAgentId: LOCAL }],
    }));

    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL]} agentMeta={agentMeta} />);
    await act(async () => {});
    await advance(500);

    const bubble = bubbleFor(text);
    expect(within(bubble).getByRole('img', { name: 'Queued for Aster' })).toBeInTheDocument();
    expect(within(bubble).queryByRole('button', { name: QUEUE })).not.toBeInTheDocument();
  });

  it('renders rooms without Local pending deliveries exactly as before', async () => {
    const projectRoot = '/tmp/kota-d3-unchanged';
    const messages = [busBubble('bus-terminal', 'agentbus-fixture-0003', 'terminal handoff')];
    serveRoom(() => roomState(projectRoot, { messages }));
    const { container, unmount } = render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[PTY, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={vi.fn()} />);
    await act(async () => {});
    expect(screen.getByText('terminal handoff')).toBeInTheDocument();
    const withoutField = container.innerHTML;
    unmount();

    serveRoom(() => roomState(projectRoot, { messages, localPendingDeliveries: [] }));
    const second = render(<VioletRoomPanel projectRoot={projectRoot} agentIds={[PTY, PEER]} agentMeta={agentMeta} />);
    await act(async () => {});
    expect(screen.getByText('terminal handoff')).toBeInTheDocument();

    expect(second.container.innerHTML).toBe(withoutField);
    expect(second.container.querySelector('.local-pending, .violet-msg-local-gutter, .violet-msg-local-queue')).toBeNull();
  });

  it('hands the workbench callback only to pending bubbles', async () => {
    const projectRoot = '/tmp/kota-d3-callback-identity';
    serveRoom(() => roomState(projectRoot, {
      messages: [
        busBubble('bus-terminal', 'agentbus-fixture-0003', 'terminal handoff'),
        busBubble('bus-pending', 'agentbus-fixture-0001', 'please review the gate'),
      ],
      localPendingDeliveries: [{ messageId: 'agentbus-fixture-0001', targetAgentId: LOCAL }],
    }));
    const panel = (onOpen: (agentId: string, section?: 'queue') => void) => (
      <VioletRoomPanel projectRoot={projectRoot} agentIds={[LOCAL, PEER]} agentMeta={agentMeta} onOpenLocalWorkbench={onOpen} />
    );
    const { rerender } = render(panel(vi.fn()));
    await act(async () => {});
    await advance(500);
    const renders = (text: string) => quoteParses.filter((parsed) => parsed === text).length;
    const terminalBefore = renders('terminal handoff');
    const pendingBefore = renders('please review the gate');

    // A caller that does not keep the callback stable must not re-render the room.
    const onOpen = vi.fn();
    rerender(panel(onOpen));

    expect(renders('terminal handoff')).toBe(terminalBefore);
    expect(renders('please review the gate')).toBeGreaterThan(pendingBefore);
    fireEvent.click(within(bubbleFor('please review the gate')).getByRole('button', { name: QUEUE }));
    expect(onOpen).toHaveBeenCalledWith(LOCAL, 'queue');
  });
});
