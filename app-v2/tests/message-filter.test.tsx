import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Stage } from '../src/chrome/Stage';
import { VioletRoomPanel } from '../src/chrome/VioletRoomPanel';
import { filterAgentBusMessages } from '../src/lib/violet-message-filter';
import * as client from '../src/pty-client';

beforeEach(() => vi.restoreAllMocks());
afterEach(() => vi.restoreAllMocks());

function message(id: string, fields: Partial<client.VioletChatMessage> = {}): client.VioletChatMessage {
  return {
    id, sessionId: 'message-filter-test', agentId: 'alice', shell: 'codex', role: 'assistant',
    kind: 'message', timestamp: '2026-09-13T10:00:00Z', text: id, ...fields,
  };
}

function bus(id: string, fields: Partial<client.VioletChatMessage> = {}) {
  return message(id, {
    shell: 'system', nativeEventId: `agentbus-${id}`, actorIntent: 'handoff',
    targetAgentIds: ['bob'], ...fields,
  });
}

function room(messages: client.VioletChatMessage[]): client.VioletRoomState {
  return {
    messages, sources: [], workEvents: [], agentBusReceipts: [],
    rawLogDir: '/tmp/message-filter/raw_logs', chathistoryDir: '/tmp/message-filter/chathistory',
    syncedAt: '2026-09-13T10:10:00Z',
  };
}

function preview(projectRoot: string, open = true) {
  return <Stage
    sceneKey="conversation" liveAgents={new Set()} tableSlots={['alice', 'bob']} shortcutAgentsOrdered={['alice', 'bob']}
    targetAgent="alice" chatFilterTargetAgents={['alice']} groupChatOpen={open}
    projectRoot={projectRoot} onOpenAgent={vi.fn()} onToggleGroupChat={vi.fn()}
    centerpiece="fire" roomColor="#2b2c2f" deskColor="#6d5241" roomTheme="classic" deskTheme="warm"
    onChangeCenter={vi.fn()} onChangeRoom={vi.fn()} onChangeDesk={vi.fn()}
    onChangeRoomTheme={vi.fn()} onChangeDeskTheme={vi.fn()}
  />;
}

describe('Agent Bus display selector', () => {
  it('hides actor-side bus messages and skipped notices without matching intent or text', () => {
    const kept = [
      message('ordinary reply', { actorIntent: 'handoff' }),
      message('literal agentbus-quoted and <KOTA_MESSAGE> text', { role: 'user' }),
      message('provider-native echo', { nativeEventId: 'agentbus-provider-echo', role: 'user' }),
      message('subagent progress', { messageOrigin: 'subagent', kind: 'commentary' }),
      message('telegram prompt', { shell: 'system', agentId: 'laughing-man', nativeEventId: 'lm-update-test', actorIntent: 'telegram' }),
      message('ember reminder', { shell: 'system', agentId: 'ember', nativeEventId: 'ember-reminder-test', actorIntent: 'reminder' }),
      message('bbs mention', { shell: 'system', agentId: 'bbs', nativeEventId: 'bbs-mention:test', actorIntent: 'handoff' }),
      message('unknown system message', { shell: 'system', nativeEventId: null, actorIntent: 'handoff' }),
    ].map((entry) => Object.freeze(entry));
    const messages = Object.freeze([
      ...kept, Object.freeze(bus('handoff')), Object.freeze(bus('status', { actorIntent: 'status' })),
      Object.freeze(bus('skipped', { nativeEventId: 'agentbus-test:skipped', actorIntent: 'delivery-skipped' })),
    ]);
    expect(filterAgentBusMessages(messages, false)).toEqual(kept);
    expect(filterAgentBusMessages(messages, true)).toBe(messages);
    expect(messages).toHaveLength(11);
  });
});

describe('Message Filter at the Stage entry', () => {
  it('keeps scope and bus visibility independent in all four combinations', async () => {
    const data = room([
      message('Alice ordinary reply'), message('Bob ordinary reply', { agentId: 'bob' }),
      message('User literally says agentbus-', { role: 'user' }),
      bus('Alice to Bob bus'), bus('Bob to Alice bus', { agentId: 'bob', targetAgentIds: ['alice'] }),
      bus('Bob to Charlie bus', { agentId: 'bob', targetAgentIds: ['charlie'] }),
      message('Telegram still visible', { shell: 'system', agentId: 'laughing-man', nativeEventId: 'lm-update-keep', targetAgentIds: ['alice'] }),
      message('Ember still visible', { shell: 'system', agentId: 'ember', nativeEventId: 'ember-reminder-keep', targetAgentIds: ['alice'] }),
    ]);
    const original = structuredClone(data);
    const read = vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue(data);
    const send = vi.spyOn(client, 'agentBusSend');
    const retry = vi.spyOn(client, 'agentBusRetryDelivery');
    const view = render(preview('/tmp/message-filter-combinations'));
    expect(await screen.findByText('Bob to Charlie bus')).toBeInTheDocument();
    const trigger = screen.getByTestId('ribbon-filter-clear');
    await userEvent.hover(trigger);
    const panel = screen.getByRole('dialog', { name: 'Message Filter' });
    const toggle = within(panel).getByRole('button', { name: 'Show Agent to Agent Msg' });
    expect(toggle).toHaveAttribute('aria-pressed', 'true');
    expect(panel.closest('button')).toBeNull();
    expect(within(panel).queryByText(/current|Click to toggle|Every agent/)).not.toBeInTheDocument();

    await userEvent.click(toggle);
    expect(trigger).toHaveAttribute('data-chat-filter-mode', 'all');
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    expect(screen.queryByText('Alice to Bob bus')).not.toBeInTheDocument();
    expect(screen.queryByText('Bob to Alice bus')).not.toBeInTheDocument();
    expect(screen.queryByText('Bob to Charlie bus')).not.toBeInTheDocument();
    expect(screen.getByText('Bob ordinary reply')).toBeInTheDocument();

    await userEvent.click(within(panel).getByRole('radio', { name: 'Selected Agent' }));
    expect(trigger).toHaveAttribute('data-chat-filter-mode', 'filter');
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    expect(screen.queryByText('Bob ordinary reply')).not.toBeInTheDocument();
    expect(screen.getByText('Alice ordinary reply')).toBeInTheDocument();
    expect(screen.getByText('User literally says agentbus-')).toBeInTheDocument();
    expect(screen.getByText('Telegram still visible')).toBeInTheDocument();
    expect(screen.getByText('Ember still visible')).toBeInTheDocument();

    await userEvent.click(toggle);
    expect(trigger).toHaveAttribute('data-chat-filter-mode', 'filter');
    expect(await screen.findByText('Alice to Bob bus')).toBeInTheDocument();
    expect(screen.getByText('Bob to Alice bus')).toBeInTheDocument();
    expect(screen.queryByText('Bob to Charlie bus')).not.toBeInTheDocument();
    await userEvent.click(within(panel).getByRole('radio', { name: 'All Agents' }));
    expect(await screen.findByText('Bob to Charlie bus')).toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-pressed', 'true');

    await userEvent.click(toggle);
    view.rerender(preview('/tmp/message-filter-combinations', false));
    view.rerender(preview('/tmp/message-filter-combinations', true));
    expect(trigger).toHaveAttribute('data-show-agent-to-agent-messages', 'false');
    expect(await screen.findByText('Alice ordinary reply')).toBeInTheDocument();
    expect(screen.queryByText('Alice to Bob bus')).not.toBeInTheDocument();
    expect(data).toEqual(original);
    expect(send).not.toHaveBeenCalled();
    expect(retry).not.toHaveBeenCalled();
    expect(read.mock.calls.every(([request]) => !request || !('showAgentToAgentMessages' in request))).toBe(true);
  });

  it('retains live bus arrivals while hidden and restores them on reopening the display filter', async () => {
    const projectRoot = '/tmp/message-filter-live';
    vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue(room([message('ordinary live reply')]));
    render(preview(projectRoot));
    await screen.findByText('ordinary live reply');
    await userEvent.hover(screen.getByTestId('ribbon-filter-clear'));
    const toggle = screen.getByRole('button', { name: 'Show Agent to Agent Msg' });
    await userEvent.click(toggle);
    act(() => {
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot, agentIds: ['alice', 'bob'] },
          state: room([message('ordinary live reply'), bus('arrived while hidden')]),
        },
      }));
    });
    expect(screen.queryByText('arrived while hidden')).not.toBeInTheDocument();
    await userEvent.click(toggle);
    expect(await screen.findByText('arrived while hidden')).toBeInTheDocument();
    expect(screen.getAllByText('arrived while hidden')).toHaveLength(1);
  });

  it('supports hover entry, keyboard scope selection, Escape and outside dismissal', async () => {
    vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue(room([]));
    const view = render(preview('/tmp/message-filter-keyboard'));
    const trigger = screen.getByTestId('ribbon-filter-clear');
    await userEvent.hover(trigger);
    expect(screen.getByRole('dialog', { name: 'Message Filter' })).toBeInTheDocument();
    const all = screen.getByRole('radio', { name: 'All Agents' });
    act(() => all.focus());
    fireEvent.keyDown(all, { key: 'ArrowLeft' });
    expect(screen.getByRole('radio', { name: 'Selected Agent' })).toHaveFocus();
    expect(trigger).toHaveAttribute('data-chat-filter-mode', 'filter');
    await userEvent.tab();
    expect(screen.getByRole('button', { name: 'Show Agent to Agent Msg' })).toHaveFocus();
    await userEvent.keyboard(' ');
    expect(trigger).toHaveAttribute('data-show-agent-to-agent-messages', 'false');
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Message Filter' })).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    fireEvent.mouseEnter(trigger.parentElement!);
    fireEvent.pointerDown(document.body);
    expect(screen.queryByRole('dialog', { name: 'Message Filter' })).not.toBeInTheDocument();
    fireEvent.mouseEnter(trigger.parentElement!);
    fireEvent.mouseLeave(trigger.parentElement!);
    view.unmount();
  });

  it('keeps pointer clicks available when WebKit blurs a mode button to null', async () => {
    vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue(room([]));
    render(preview('/tmp/message-filter-webkit-blur'));
    const trigger = screen.getByTestId('ribbon-filter-clear');
    await userEvent.hover(trigger);
    const all = screen.getByRole('radio', { name: 'All Agents' });
    act(() => all.focus());
    fireEvent.blur(all, { relatedTarget: null });
    const toggle = screen.getByRole('button', { name: 'Show Agent to Agent Msg' });
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    expect(trigger).toHaveAttribute('data-chat-filter-mode', 'all');
  });
});

describe('Filtered room history', () => {
  it('gives each room a fresh bounded scan without using the previous room cursor', async () => {
    const firstRoot = '/tmp/message-filter-history-first';
    const secondRoot = '/tmp/message-filter-history-second';
    let firstPages = 0;
    const batch = (time: number) => room(Array.from({ length: 30 }, (_, i) => bus(`bus ${time}-${i}`, {
      timestamp: new Date(time + i * 1000).toISOString(),
    })));
    const secondOldest = '2026-09-13T08:00:00.000Z';
    const read = vi.spyOn(client, 'readVioletRoomCache').mockImplementation(async request => {
      if (request?.projectRoot === firstRoot) {
        return batch(Date.parse('2026-09-13T10:00:00Z') - firstPages++ * 30_000);
      }
      return request?.before
        ? room([message('second room older message', { timestamp: '2026-09-13T07:59:00Z' })])
        : batch(Date.parse(secondOldest));
    });
    const panel = (projectRoot: string) => <VioletRoomPanel
      projectRoot={projectRoot} agentIds={['alice', 'bob']} showAgentToAgentMessages={false}
    />;
    const view = render(panel(firstRoot));
    await waitFor(() => expect(firstPages).toBe(5));
    expect(screen.getByText('Agent-to-agent messages are hidden.')).toBeInTheDocument();
    view.rerender(panel(secondRoot));
    expect(await screen.findByText('second room older message')).toBeInTheDocument();
    const olderRequests = read.mock.calls.map(([request]) => request)
      .filter(request => request?.projectRoot === secondRoot && request.before);
    expect(olderRequests).toEqual([{
      projectRoot: secondRoot, limit: 30, before: secondOldest, agentIds: null,
    }]);
    expect(firstPages).toBe(5);
  });

  it.each([false, true])('loads older visible messages when a full page is bus-only (Selected Agent: %s)', async (selected) => {
    const batch = Array.from({ length: 30 }, (_, i) => bus(`bus page ${i}`, {
      timestamp: `2026-09-13T10:00:${String(i).padStart(2, '0')}Z`,
    }));
    const read = vi.spyOn(client, 'readVioletRoomCache').mockImplementation(async request => (
      request?.before ? room([message('older ordinary message', { timestamp: '2026-09-13T09:59:00Z' })]) : room(batch)
    ));
    render(<VioletRoomPanel
      projectRoot={`/tmp/message-filter-history-${selected}`} agentIds={['alice', 'bob']}
      chatFilterActive={selected} chatFilterAgentIds={['alice']} showAgentToAgentMessages={false}
    />);
    expect(await screen.findByText('older ordinary message')).toBeInTheDocument();
    expect(screen.queryByText('bus page 0')).not.toBeInTheDocument();
    await waitFor(() => expect(read).toHaveBeenCalledWith({
      projectRoot: `/tmp/message-filter-history-${selected}`, limit: 30,
      before: '2026-09-13T10:00:00Z', agentIds: selected ? ['alice'] : null,
    }));
  });
});
