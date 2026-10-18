import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { Stage } from '../src/chrome/Stage';
import * as ptyClient from '../src/pty-client';
import type { VioletChatMessage, VioletRoomState } from '../src/pty-client';

afterEach(() => vi.restoreAllMocks());

function messages(room: string, start = 0, end = 230): VioletChatMessage[] {
  return Array.from({ length: end - start }, (_, offset) => {
    const index = start + offset;
    return {
      id: `${room}-${index}`, sessionId: room, agentId: 'alice', shell: 'claude',
      role: 'assistant', kind: 'message', text: `${room} message ${index}`,
      timestamp: new Date(Date.UTC(2026, 5, 6) + index * 1000).toISOString(),
      sourcePath: null, nativeEventId: null,
    };
  });
}

function roomState(root: string, items: VioletChatMessage[]): VioletRoomState {
  return {
    messages: items, sources: [], workEvents: [], agentBusReceipts: [],
    rawLogDir: `${root}/project-memory/raw_logs`,
    chathistoryDir: `${root}/project-memory/chathistory`,
    syncedAt: '2026-06-06T12:00:00.000Z',
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function fixture(name: string, historyA = messages('A')) {
  const a = `/tmp/fable-history-${name}-a`;
  const b = `/tmp/fable-history-${name}-b`;
  const histories = new Map([[a, historyA], [b, messages('B')]]);
  const readPage = (request: ptyClient.VioletRoomRequest = {}) => {
    const items = histories.get(request.projectRoot!) ?? [];
    return roomState(request.projectRoot!, items.filter(message => (
      !request.before || message.timestamp < request.before
    )).slice(-(request.limit ?? 30)));
  };
  const read = vi.spyOn(ptyClient, 'readVioletRoomCache').mockImplementation(async request => readPage(request));
  const props: ComponentProps<typeof Stage> = {
    sceneKey: 'conversation', liveAgents: new Set(), tableSlots: ['alice', 'bob'],
    targetAgent: null, onOpenAgent: vi.fn(), centerpiece: 'fire',
    roomColor: '#2b2c2f', deskColor: '#6d5241', roomTheme: 'classic', deskTheme: 'warm',
    onChangeCenter: vi.fn(), onChangeRoom: vi.fn(), onChangeDesk: vi.fn(),
    onChangeRoomTheme: vi.fn(), onChangeDeskTheme: vi.fn(),
    projectRoot: a, groupChatOpen: true,
  };
  const view = render(<Stage {...props} />);
  const show = async (root: string, extra: Partial<ComponentProps<typeof Stage>> = {}) => {
    await act(async () => view.rerender(<Stage {...props} projectRoot={root} {...extra} />));
  };
  const sync = async (root: string, items: VioletChatMessage[]) => {
    await act(async () => window.dispatchEvent(new CustomEvent('violet://room/synced', {
      detail: { request: { projectRoot: root }, state: roomState(root, items) },
    })));
  };
  const count = () => view.container.querySelectorAll('[data-violet-message-id]').length;
  const expand = async (root: string, label: string) => {
    await screen.findByText(`${label} message 229`);
    await sync(root, histories.get(root)!.slice(-200));
    expect(count()).toBe(200);
    fireEvent.click(screen.getByRole('button', { name: 'Load previous 30 messages' }));
    await screen.findByText(`${label} message 0`);
    expect(count()).toBe(230);
  };
  return { a, b, histories, read, readPage, view, props, show, sync, count, expand };
}

describe('Violet room history on project changes', () => {
  it('returns to 200 messages with older history available and keeps two rooms separate', async () => {
    const f = fixture('roundtrip');
    await f.expand(f.a, 'A');
    await f.show(f.b);
    await f.expand(f.b, 'B');
    expect(screen.queryByText('A message 229')).not.toBeInTheDocument();
    await f.show(f.a);
    expect(f.count()).toBe(200);
    expect(screen.getByText('A message 30')).toBeInTheDocument();
    expect(screen.queryByText('A message 0')).not.toBeInTheDocument();
    expect(screen.queryByText('B message 229')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Load previous 30 messages' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Load previous 30 messages' }));
    await screen.findByText('A message 0');
    expect(f.count()).toBe(230);
    await f.show(f.b);
    expect(f.count()).toBe(200);
    expect(screen.getByText('B message 30')).toBeInTheDocument();
    expect(screen.queryByText('A message 0')).not.toBeInTheDocument();
  });

  it('preserves loaded history during live updates, filter changes and Latest', async () => {
    const f = fixture('same-project');
    await f.expand(f.a, 'A');
    await f.show(f.a, { chatFilterActive: true, chatFilterTargetAgents: ['alice'] });
    expect(f.count()).toBe(230);
    const scroller = f.view.container.querySelector('.violet-room-scroll') as HTMLDivElement;
    Object.defineProperty(scroller, 'clientHeight', { configurable: true, value: 300 });
    Object.defineProperty(scroller, 'scrollHeight', { configurable: true, value: 1200 });
    await act(async () => {
      for (let i = 0; i < 12; i++) await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
    });
    scroller.scrollTop = 100;
    fireEvent.scroll(scroller);
    await screen.findByRole('button', { name: /latest/i });
    await f.sync(f.a, messages('A', 230, 231));
    expect(f.count()).toBe(231);
    expect(screen.getByText('A message 0')).toBeInTheDocument();
    expect(scroller.scrollTop).toBe(100);
    fireEvent.click(screen.getByRole('button', { name: /latest/i }));
    expect(f.count()).toBe(231);
    expect(screen.getByText('A message 0')).toBeInTheDocument();
  });

  it('keeps history when minimizing the same room but releases it when switching projects while hidden', async () => {
    const f = fixture('minimized');
    await f.expand(f.a, 'A');
    await f.show(f.a, { groupChatOpen: false });
    await f.show(f.a);
    expect(f.count()).toBe(230);
    expect(screen.getByText('A message 0')).toBeInTheDocument();
    await f.show(f.a, { groupChatOpen: false });
    await f.show(f.b, { groupChatOpen: false });
    await f.show(f.a);
    expect(f.count()).toBe(200);
    expect(screen.queryByText('A message 0')).not.toBeInTheDocument();
    await f.sync(f.a, messages('A', 230, 231));
    // The expanded flag was also cleared, including across a panel remount.
    expect(f.count()).toBe(200);
    expect(screen.getByText('A message 31')).toBeInTheDocument();
  });

  it.each(['resolve', 'reject'] as const)('ignores an older-page %s after leaving the room', async outcome => {
    const f = fixture(`late-${outcome}`);
    await f.expand(f.a, 'A');
    const pending = deferred<VioletRoomState>();
    f.read.mockImplementation(async request => (
      request?.projectRoot === f.a && request.before ? pending.promise : f.readPage(request)
    ));
    fireEvent.click(screen.getByRole('button', { name: 'Load previous 30 messages' }));
    await screen.findByText('Loading previous messages');
    await f.show(f.b);
    await act(async () => {
      if (outcome === 'resolve') pending.resolve(roomState(f.a, messages('A', -30, 0)));
      else pending.reject(new Error('Old room read failed'));
    });
    expect(f.count()).toBe(30);
    expect(screen.getByText('B message 229')).toBeInTheDocument();
    expect(screen.queryByText('A message -30')).not.toBeInTheDocument();
    expect(screen.queryByText('Could not load older Violet messages.')).not.toBeInTheDocument();
    await f.show(f.a);
    expect(f.count()).toBe(200);
    expect(screen.queryByText('A message -30')).not.toBeInTheDocument();
    expect(screen.queryByText('Could not load older Violet messages.')).not.toBeInTheDocument();
  });

  it('does not let an old request change a new load after A to B to A', async () => {
    const f = fixture('late-roundtrip');
    await f.expand(f.a, 'A');
    const oldRead = deferred<VioletRoomState>();
    const newRead = deferred<VioletRoomState>();
    let requested = 0;
    f.read.mockImplementation(async request => {
      if (request?.projectRoot === f.a && request.before) return requested++ === 0 ? oldRead.promise : newRead.promise;
      return f.readPage(request);
    });
    fireEvent.click(screen.getByRole('button', { name: 'Load previous 30 messages' }));
    await f.show(f.b);
    await f.show(f.a);
    fireEvent.click(screen.getByRole('button', { name: 'Load previous 30 messages' }));
    await screen.findByText('Loading previous messages');
    await act(async () => oldRead.resolve(roomState(f.a, messages('A', -30, 0))));
    expect(screen.getByRole('button', { name: 'Loading previous messages' })).toBeDisabled();
    expect(f.count()).toBe(200);
    expect(screen.queryByText('A message -30')).not.toBeInTheDocument();
    await act(async () => newRead.resolve(roomState(f.a, messages('A', 0, 30))));
    expect(f.count()).toBe(230);
    expect(screen.getByText('A message 0')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Load previous 30 messages' })).toBeEnabled();
  });

  it('ignores a latest-page read from a room that is no longer open', async () => {
    const f = fixture('late-latest');
    await f.expand(f.a, 'A');
    await f.show(f.b);
    const pending = deferred<VioletRoomState>();
    f.read.mockImplementationOnce(async () => pending.promise);
    await f.show(f.a);
    await f.show(f.b);
    await act(async () => pending.resolve(roomState(f.a, messages('late-A', 0, 230))));
    expect(f.count()).toBe(30);
    expect(screen.queryByText('late-A message 229')).not.toBeInTheDocument();
    await f.show(f.a);
    expect(f.count()).toBe(200);
    expect(screen.queryByText('late-A message 229')).not.toBeInTheDocument();
  });

  it('lets an empty filter autofill older matching messages again after a project roundtrip', async () => {
    const f = fixture('filter-autofill', messages('A').map((message, i) => ({
      ...message, agentId: i === 0 ? 'alice' : 'bob',
    })));
    await f.expand(f.a, 'A');
    const filter = { chatFilterActive: true, chatFilterTargetAgents: ['alice'] };
    await f.show(f.a, filter);
    expect(screen.getByText('A message 0')).toBeInTheDocument();
    await f.show(f.b, filter);
    f.read.mockClear();
    await f.show(f.a, filter);
    await screen.findByText('A message 0');
    await waitFor(() => expect(f.read).toHaveBeenCalledWith(expect.objectContaining({
      projectRoot: f.a, before: messages('A', 30, 31)[0].timestamp, agentIds: ['alice'],
    })));
    expect(screen.queryByText('B message 229')).not.toBeInTheDocument();
  });
});
