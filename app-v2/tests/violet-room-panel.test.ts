import { afterEach, describe, expect, it, vi } from 'vitest';
import { createElement } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import * as ptyClient from '../src/pty-client';
import codexIcon from '../src/assets/tavern/icons/providers/openai.svg';

import {
  VioletRoomPanel,
  VioletMessageBubble,
  mergeOlderRoomMessages,
  mergeRoomMessages,
  mergeSyncedNativeMessages,
  normalizeAttachmentInsensitive,
  normalizeForDedupe,
  reconcileAgentBusDeliveries,
  nextAgentBusDeliveryDeadline,
} from '../src/chrome/VioletRoomPanel';
import type { VioletChatMessage } from '../src/pty-client';
import * as dedupeText from '../src/lib/violet-message-dedupe';
import { splitLeadingEnvelopePrefix, stripLeadingTemporalGapForDisplay } from '../src/lib/violet-message-dedupe';

it('uses the agent header for Puppeteer without changing ordinary user or agent headers', () => {
  const agentMeta = {
    'agent-a': { name: 'Ada', emoji: 'A', role: 'Engineer', hue: '#fff', avatarClass: 'provider-codex' },
    'agent-b': { name: '座山雕', emoji: 'B', role: 'Reviewer', hue: '#fff', avatarClass: 'provider-claude' },
  };
  const message = {
    ...roomMessage({
      agentId: 'user', shell: 'composer', messageOrigin: 'puppeteer', targetAgentIds: ['agent-b'],
    }),
    deliveryStatus: 'failed' as const,
  };
  const onRetryComposerMessage = vi.fn();
  const props = { message, agentMeta, humanTargetName: 'Human Tester', onRetryComposerMessage };
  const view = render(createElement(VioletMessageBubble, props));
  const expectAgentHeader = (name: string) => {
    const header = view.container.querySelector<HTMLDivElement>('.violet-msg-meta')!;
    expect(header).toBeInTheDocument();
    const names = header.querySelector('.violet-agent-name-group');
    const target = within(header).getByText('@座山雕');
    expect(names).toHaveTextContent(name);
    expect(target.previousElementSibling).toBe(names);
    expect(target.nextElementSibling).toBe(header.querySelector('time'));
    expect(header.querySelector('time')).not.toBeEmptyDOMElement();
    expect(view.container.querySelectorAll('time')).toHaveLength(1);
    expect(view.container.querySelector('.violet-msg-targets')).toBeNull();
  };
  expectAgentHeader('Puppeteer');
  expect(view.container.querySelector('article')).toHaveClass('agent', 'puppeteer-origin');
  expect(view.container.querySelector('.violet-msg-avatar')).toHaveClass('system-puppeteer');
  fireEvent.click(screen.getByRole('button', { name: 'Queued message — try sending now' }));
  expect(onRetryComposerMessage).toHaveBeenCalledExactlyOnceWith(message);

  view.rerender(createElement(VioletMessageBubble, {
    ...props, agent: agentMeta['agent-a'], message: roomMessage({ role: 'assistant', targetAgentIds: ['agent-b'] }),
  }));
  expectAgentHeader('Ada');
  expect(view.container.querySelector('article')).not.toHaveClass('puppeteer-origin');

  view.rerender(createElement(VioletMessageBubble, {
    ...props, message: { ...message, messageOrigin: undefined, privacy: true },
  }));
  const targets = view.container.querySelector<HTMLDivElement>('.violet-msg-targets')!;
  expect(view.container.querySelector('article')).toHaveClass('user');
  expect(view.container.querySelector('.violet-msg-meta')).toBeNull();
  expect(targets.querySelector('.project-agent-name')).toBeNull();
  expect(within(targets).getByText('@座山雕').nextElementSibling).toHaveTextContent('private');
  expect(targets.lastElementChild).toBe(targets.querySelector('time'));
  expect(view.container.querySelectorAll('time')).toHaveLength(1);

  view.rerender(createElement(VioletMessageBubble, {
    ...props, message: roomMessage({ agentId: 'user', targetAgentIds: [] }),
  }));
  expect(view.container.querySelector('.violet-msg-meta, .violet-msg-targets')).toBeNull();
});

describe('Codex sandbox notice', () => {
  const notice = roomMessage({
    id: 'sandbox-downgrade:session-fixture:turn-fixture',
    role: 'system',
    shell: 'codex',
    actorIntent: 'sandbox-downgrade',
    agentDisplayName: 'Former name',
    text: 'Codex session lost room access after a Codex update — refresh session to restore.',
  });
  const agentMeta = {
    'agent-a': { name: 'Ada', emoji: 'A', role: 'Engineer', hue: '#fff', avatarClass: 'provider-codex' },
  };

  it('keeps the notice through room merging and filters, with a wrapping non-quoteable refresh action', async () => {
    const merged = mergeRoomMessages([notice], []);
    expect(merged).toHaveLength(1);
    expect(merged[0]?.ghostSasayaki).not.toBe(true);
    const projectRoot = '/tmp/fable-sandbox-notice';
    const readCache = vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [notice], sources: [], workEvents: [], agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: notice.timestamp,
    });
    const onRefreshAgentSession = vi.fn();
    const onOpenAgentTerminal = vi.fn();
    const onQuoteMessage = vi.fn(() => 'inserted' as const);
    const view = render(createElement(VioletRoomPanel, {
      projectRoot, agentIds: ['agent-a'], agentMeta,
      chatFilterActive: true, chatFilterAgentIds: ['agent-a'],
      showAgentToAgentMessages: false,
      onRefreshAgentSession, onOpenAgentTerminal, onQuoteMessage,
    }));
    try {
      view.container.style.width = '520px';
      const refresh = await screen.findByRole('button', { name: 'Refresh session' });
      const line = refresh.closest('article')!;
      expect(line).toHaveClass('violet-msg', 'system-line', 'sandbox-downgrade');
      expect(refresh.parentElement).toHaveClass('violet-system-line-text');
      expect(refresh).toHaveClass('violet-sandbox-refresh');
      expect(line).toHaveTextContent('Ada lost room access after a Codex update — Refresh session to restore.');
      expect(line).toHaveAttribute('data-violet-message-id', notice.id);
      expect(line.querySelector('.violet-msg-avatar-host, .provider-badge, .ghost-sasayaki')).toBeNull();
      expect(within(line).queryByRole('button', { name: /quote/i })).toBeNull();
      expect(refresh).toBeEnabled();
      fireEvent.click(refresh);
      expect(onRefreshAgentSession).toHaveBeenCalledExactlyOnceWith('agent-a');
      expect(onOpenAgentTerminal).not.toHaveBeenCalled();
      expect(onQuoteMessage).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      readCache.mockRestore();
    }
  });

  it('uses the current name, then the event snapshot, then the agent id', () => {
    const props = { message: notice, humanTargetName: 'Human Tester' };
    const view = render(createElement(VioletMessageBubble, { ...props, agentMeta }));
    expect(view.container.querySelector('strong')).toHaveTextContent('Ada');
    view.rerender(createElement(VioletMessageBubble, props));
    expect(view.container.querySelector('strong')).toHaveTextContent('Former name');
    view.rerender(createElement(VioletMessageBubble, {
      ...props, message: { ...notice, agentDisplayName: null },
    }));
    expect(view.container.querySelector('strong')).toHaveTextContent('agent-a');
  });
});

it('splits envelope prefixes without dropping or normalizing any bytes', () => {
  for (const prefix of ['', ' \u0015\n', '\r\n[Image #1] \t[Attachment #2]\r\n']) {
    const body = '<KOTA_QUOTE_META v="1">\r\nbody';
    expect(splitLeadingEnvelopePrefix(prefix + body)).toEqual({ prefix, rest: body });
  }
  expect(splitLeadingEnvelopePrefix('[not an attachment] body')).toEqual({ prefix: '', rest: '[not an attachment] body' });
});

describe('temporal context display', () => {
  const gap = [
    '<KOTA_TEMPORAL_GAP v="1" current_time="2026-09-01T12:00:00Z">',
    'It has been over 24 hours since your last completed response in this room.',
    '</KOTA_TEMPORAL_GAP>',
  ].join('\n');

  it('leaves text without a valid leading block unchanged', () => {
    for (const text of ['', ' \u0015hello world\n', '[Image #1] hi', `quoted:\n${gap}\nhello`, `${gap.replace('v="1"', 'v="2"')}\nhello`]) {
      expect(stripLeadingTemporalGapForDisplay(text)).toBe(text);
    }
  });

  it('removes the block but preserves leading whitespace, controls and the remaining body', () => {
    expect(stripLeadingTemporalGapForDisplay(`${gap}\nhello world`)).toBe('hello world');
    const prefix = '\u0015 \t\n\u0000';
    expect(stripLeadingTemporalGapForDisplay(`${prefix}${gap}\n\nhello world\n`)).toBe(`${prefix}\nhello world\n`);
  });

  it('retains provider attachment markers before the block', () => {
    const prefix = ' \u0015[Image #1] \n[Image #2]\t';
    expect(stripLeadingTemporalGapForDisplay(`${prefix}${gap}\nhi`)).toBe(`${prefix}hi`);
  });

  it('peels the host marks envelope ahead of the gap, never a quoted one', () => {
    const marks = '<KOTA_MARKS v="1" revision="3">\n- [m-a] Note.\n</KOTA_MARKS>\n';
    expect(stripLeadingTemporalGapForDisplay(`${marks}${gap}\nhello`)).toBe('hello');
    expect(stripLeadingTemporalGapForDisplay(`${marks}[Image #1] hi`)).toBe('[Image #1] hi');
    expect(stripLeadingTemporalGapForDisplay(`quoted:\n${marks}hello`)).toBe(`quoted:\n${marks}hello`);
  });

  it('handles CRLF without normalizing the preserved prefix or body', () => {
    const prefix = '\r\n[Image #1] \r\n';
    const body = '\r\nhello\r\nworld\r\n';
    expect(stripLeadingTemporalGapForDisplay(`${prefix}${gap.replaceAll('\n', '\r\n')}\r\n${body}`)).toBe(prefix + body);
  });
});

it('shows the actual turn provider badge only on known-provider end-turn bubbles', async () => {
  const projectRoot = '/tmp/fable-provider-badges';
  const readCache = vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
    messages: [
      roomMessage({ id: 'final', role: 'assistant', shell: 'codex', agentProvider: 'claude', text: 'Finished the draft.' }),
      roomMessage({ id: 'system', role: 'assistant', shell: 'system', text: 'System notice.' }),
      roomMessage({ id: 'unknown', role: 'assistant', shell: 'future-cli', text: 'Unknown provider reply.' }),
      roomMessage({ id: 'progress', role: 'assistant', kind: 'commentary', shell: 'codex', text: 'Checking the draft.' }),
      roomMessage({ id: 'user', role: 'user', shell: 'codex', text: 'Please check the draft.' }),
    ],
    sources: [], workEvents: [], agentBusReceipts: [],
    rawLogDir: `${projectRoot}/project-memory/raw_logs`,
    chathistoryDir: `${projectRoot}/project-memory/chathistory`,
    syncedAt: '2026-05-18T08:51:27.000Z',
  });
  const view = render(createElement(VioletRoomPanel, { projectRoot, agentIds: ['agent-a'] }));
  try {
    const bubble = (await screen.findByText('Finished the draft.')).closest('.violet-msg') as HTMLElement;
    const badge = within(bubble).getByLabelText('Codex');
    expect(badge.parentElement).toHaveClass('violet-msg-model');
    expect(bubble.querySelector('.violet-msg-avatar-host .provider-badge')).toBeNull();
    expect(bubble.querySelector('.with-provider-badge')).toBeNull();
    expect(badge.querySelector('img')).toHaveAttribute('src', codexIcon);
    expect(badge.querySelector('img')).toHaveAttribute('width', '10');
    expect(badge.querySelector('img')).toHaveAttribute('height', '10');
    const tooltip = within(badge).getByRole('tooltip', { hidden: true });
    expect(tooltip).toHaveTextContent('Codex');
    expect(badge).toHaveAttribute('aria-describedby', tooltip.id);
    expect(badge).toHaveAttribute('tabindex', '0');
    for (const id of ['system', 'unknown', 'progress', 'user']) {
      const other = view.container.querySelector(`[data-violet-message-id="${id}"]`);
      expect(other).not.toBeNull();
      expect(other?.querySelector('.provider-badge')).toBeNull();
      expect(other?.querySelector('.violet-msg-model')).toBeNull();
    }
    expect(view.container.querySelectorAll('.provider-badge')).toHaveLength(1);
  } finally {
    view.unmount();
    readCache.mockRestore();
  }
});

it.each([false, true])('keeps each turn’s actual model and thinking after the Auto String prefix (search=%s)', search => {
  const snapshots = [
    { model: 'gpt-6.1-sol', effort: 'high', route: 'auto' as const },
    { model: 'gpt-6-luna', effort: 'low', route: 'auto' as const },
    { model: 'claude-sonnet-4-6', effort: 'high', route: 'auto' as const },
    { model: 'gpt-6.1-sol', effort: 'medium' },
  ];
  const view = render(createElement('div', null, snapshots.map((snapshot, index) => createElement(VioletMessageBubble, {
    key: index,
    message: roomMessage({ id: `route-${index}`, role: 'assistant', shell: 'kota-local', text: `Reply ${index}`, ...snapshot }),
    humanTargetName: 'Human Tester',
    ...(search ? { searchResult: { human: { name: 'Human Tester' }, now: new Date('2026-05-18'), terms: [], selected: false, onOpen: vi.fn() } } : {}),
  }))));
  const captions = [...view.container.querySelectorAll('.violet-msg-model')];
  expect(captions).toHaveLength(4);
  captions.forEach((caption, index) => {
    expect(caption.firstElementChild).toHaveClass('provider-badge');
    expect(within(caption as HTMLElement).getByLabelText('Kota Local')).toBeInTheDocument();
    expect(caption.querySelector('.violet-msg-model-text')?.textContent).toBe(`${index < 3 ? 'Auto String · ' : ''}${snapshots[index].model} • ${snapshots[index].effort}`);
  });
});

it('shows an inline provider icon with optional model and effort only on known-provider end turns', async () => {
  const projectRoot = '/tmp/fable-model-captions';
  const readCache = vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
    messages: [
      roomMessage({ id: 'caption-both', role: 'assistant', shell: 'codex', text: 'Both fields.', model: 'gpt-fable-6', effort: 'max' }),
      roomMessage({ id: 'caption-model', role: 'assistant', shell: 'opencode', text: 'Model only.', model: 'gpt-fable-open' }),
      roomMessage({ id: 'caption-neither', role: 'assistant', shell: 'antigravity', text: 'Unknown fields.' }),
      roomMessage({ id: 'caption-effort-only', role: 'assistant', shell: 'antigravity', text: 'Unknown model.', effort: 'High' }),
      roomMessage({ id: 'caption-progress', role: 'assistant', kind: 'commentary', shell: 'codex', text: 'Checking.', model: 'gpt-fable-6', effort: 'max' }),
      roomMessage({ id: 'caption-system', role: 'assistant', shell: 'system', text: 'System notice.', model: 'ignored', effort: 'ignored' }),
      roomMessage({ id: 'caption-unknown', role: 'assistant', shell: 'future-cli', text: 'Unknown provider.', model: 'ignored', effort: 'ignored' }),
      roomMessage({ id: 'caption-user', role: 'user', shell: 'codex', text: 'A request.', model: 'ignored', effort: 'ignored' }),
    ],
    sources: [], workEvents: [], agentBusReceipts: [],
    rawLogDir: `${projectRoot}/project-memory/raw_logs`,
    chathistoryDir: `${projectRoot}/project-memory/chathistory`,
    syncedAt: '2026-05-18T08:51:27.000Z',
  });
  const view = render(createElement(VioletRoomPanel, { projectRoot, agentIds: ['agent-a'] }));
  try {
    const both = (await screen.findByText('Both fields.')).closest('.violet-msg') as HTMLElement;
    const caption = both.querySelector('.violet-msg-model');
    expect(caption).toHaveTextContent('gpt-fable-6 • max');
    expect(caption?.previousElementSibling).toHaveClass('violet-msg-body');
    expect(caption?.parentElement).toHaveClass('violet-msg-content');
    expect(caption?.firstElementChild).toHaveClass('provider-badge');
    expect(caption?.querySelector('.provider-badge img')).toHaveAttribute('width', '10');
    expect(caption?.querySelector('.violet-msg-model-text')).toHaveTextContent(/^gpt-fable-6 • max$/);
    const modelOnly = (await screen.findByText('Model only.')).closest('.violet-msg') as HTMLElement;
    expect(modelOnly.querySelector('.violet-msg-model-text')).toHaveTextContent(/^gpt-fable-open$/);
    expect(modelOnly.querySelector('.violet-msg-effort')).toBeNull();
    for (const id of ['caption-neither', 'caption-effort-only']) {
      const iconOnly = view.container.querySelector(`[data-violet-message-id="${id}"] .violet-msg-model`) as HTMLElement;
      expect(iconOnly).not.toBeNull();
      expect(iconOnly.children).toHaveLength(1);
      expect(within(iconOnly).getByLabelText('Antigravity CLI')).toHaveClass('provider-badge');
      expect(iconOnly.querySelector('.violet-msg-model-text')).toBeNull();
    }
    for (const id of ['caption-progress', 'caption-system', 'caption-unknown', 'caption-user']) {
      const bubble = view.container.querySelector(`[data-violet-message-id="${id}"]`);
      expect(bubble).not.toBeNull();
      expect(bubble?.querySelector('.violet-msg-model')).toBeNull();
    }
    expect(view.container.querySelectorAll('.violet-msg-model')).toHaveLength(4);
  } finally {
    view.unmount();
    readCache.mockRestore();
  }
});

describe('Violet room message dedupe', () => {
  it('matches absolute and project-memory-relative attachment prompts', () => {
    expect(
      normalizeForDedupe(
        '/Users/example/Kota/Workspaces/demo-project/project-memory/attachments/composer/att_1/original.png 图上是什么',
      ),
    ).toBe('project-memory/attachments/composer/att_1/original.png 图上是什么');
    expect(
      normalizeForDedupe('project-memory/attachments/composer/att_1/original.png 图上是什么'),
    ).toBe('project-memory/attachments/composer/att_1/original.png 图上是什么');
  });

  it('ignores terminal control padding when deduping provider echoes', () => {
    expect(normalizeForDedupe('\x15same prompt\x0b')).toBe('same prompt');
  });

  it('normalizes a very long non-attachment token without attachment regex backtracking', () => {
    const prompt = `https://example.test/${'x'.repeat(154_000)} final instruction`;

    expect(normalizeAttachmentInsensitive(prompt)).toBe(prompt);
  });

  it('strips attachment paths and provider markers without stripping user text', () => {
    expect(normalizeAttachmentInsensitive(
      '/Users/example/Kota/project-memory/attachments/composer/att_1/original.png explain this',
    )).toBe('explain this');
    expect(normalizeAttachmentInsensitive('[Image #12]explain this')).toBe('explain this');
    expect(normalizeAttachmentInsensitive('[Image: source: /tmp/input.png] explain this')).toBe('explain this');
    expect(normalizeAttachmentInsensitive('a user literally wrote project-memory/attachment')).toBe(
      'a user literally wrote project-memory/attachment',
    );
  });

  it('collapses broadcast user echoes from native and raw cache into one bubble', () => {
    const messages = ['agent-a', 'agent-b', 'agent-c'].flatMap((agentId, index) => [
      roomMessage({
        id: `native-${agentId}`,
        agentId,
        timestamp: `2026-05-18T08:51:26.${530 + index}Z`,
        text: '/Users/example/Kota/Workspaces/demo-project/project-memory/attachments/composer/att_1/original.png 图上是什么',
      }),
      roomMessage({
        id: `cache-${agentId}`,
        agentId,
        timestamp: `2026-05-18T08:51:26.${530 + index}Z`,
        text: 'project-memory/attachments/composer/att_1/original.png 图上是什么',
      }),
    ]);

    const merged = mergeRoomMessages(messages, []);

    expect(merged).toHaveLength(1);
    expect(merged[0].targetAgentIds).toEqual(['agent-a', 'agent-b', 'agent-c']);
  });

  it('keeps original exact composer echo dedupe before ghost folding', () => {
    const native = roomMessage({
      id: 'native-exact-echo',
      agentId: 'agent-a',
      text: 'same prompt',
      timestamp: '2026-06-07T20:00:02.000Z',
    });
    const local = localComposerMessage({
      id: 'local-exact-prompt',
      text: 'same prompt',
      targetAgentIds: ['agent-a'],
      timestamp: '2026-06-07T20:00:00.000Z',
    });

    const merged = mergeRoomMessages([native], [local]);

    expect(merged).toHaveLength(1);
    expect(merged[0]?.id).toBe('local-exact-prompt');
    expect((merged[0] as { ghostSasayaki?: boolean } | undefined)?.ghostSasayaki).toBeUndefined();
  });

  it('matches a native echo led by the host marks envelope to its composer prompt', () => {
    const marks = '<KOTA_MARKS v="1" revision="3">\nYour own marks that may apply to this message.\n- [m-a] Note.\n</KOTA_MARKS>\n';
    const local = localComposerMessage({ id: 'local-marked', text: 'same prompt', targetAgentIds: ['agent-a'], timestamp: '2026-06-07T20:00:00.000Z' });
    const echo = roomMessage({ id: 'native-marked', text: `${marks}same prompt`, timestamp: '2026-06-07T20:00:02.000Z' });
    expect(mergeRoomMessages([echo], [local]).map(message => message.id)).toEqual(['local-marked']);
  });

  it('keeps a matched local composer prompt before a coarse same-second reply', () => {
    const nativeUser = roomMessage({
      id: 'native-coarse-user',
      agentId: 'agent-a',
      role: 'user',
      text: 'fast prompt',
      timestamp: '2026-08-20T18:43:56+00:00',
      violetSeq: 313,
    });
    const nativeReply = roomMessage({
      id: 'native-coarse-reply',
      agentId: 'agent-a',
      role: 'assistant',
      text: 'fast reply',
      timestamp: '2026-08-20T18:43:56+00:00',
      violetSeq: 314,
    });
    const local = localComposerMessage({
      id: 'local-precise-prompt',
      text: 'fast prompt',
      targetAgentIds: ['agent-a'],
      timestamp: '2026-08-20T18:43:56.412Z',
    });

    const merged = mergeRoomMessages([nativeUser, nativeReply], [local]);

    expect(merged.map((message) => message.id)).toEqual([
      'local-precise-prompt',
      'native-coarse-reply',
    ]);
    expect(merged[0]?.violetSeq).toBe(313);
    expect((merged[0] as { quoteRefId?: string } | undefined)?.quoteRefId).toBe('native-coarse-user');
  });

  it('marks only the first near native user echo as Ghost Sasayaki', () => {
    const nativeFirst = roomMessage({
      id: 'native-near-first',
      agentId: 'agent-a',
      text: '[Image #4]same prompt',
      timestamp: '2026-06-07T20:00:02.000Z',
    });
    const nativeSecond = roomMessage({
      id: 'native-near-second',
      agentId: 'agent-a',
      text: '[Image #5]same prompt',
      timestamp: '2026-06-07T20:00:04.000Z',
    });
    const local = localComposerMessage({
      id: 'local-image-prompt',
      text: 'project-memory/attachments/composer/att_1/original.png composer prompt',
      targetAgentIds: ['agent-a'],
      timestamp: '2026-06-07T20:00:00.000Z',
    });

    const merged = mergeRoomMessages([nativeFirst, nativeSecond], [local]);
    const first = merged.find((message) => message.id === 'native-near-first') as { ghostSasayaki?: boolean } | undefined;
    const second = merged.find((message) => message.id === 'native-near-second') as { ghostSasayaki?: boolean } | undefined;

    expect(first?.ghostSasayaki).toBe(true);
    expect(second?.ghostSasayaki).toBeUndefined();
  });

  it('does not mark native user echoes outside the Ghost Sasayaki window', () => {
    const native = roomMessage({
      id: 'native-late-echo',
      agentId: 'agent-a',
      text: '[Image #4]same prompt',
      timestamp: '2026-06-07T20:00:09.000Z',
    });
    const local = localComposerMessage({
      id: 'local-image-prompt',
      text: 'project-memory/attachments/composer/att_1/original.png same prompt',
      targetAgentIds: ['agent-a'],
      timestamp: '2026-06-07T20:00:00.000Z',
    });

    const merged = mergeRoomMessages([native], [local]);
    const late = merged.find((message) => message.id === 'native-late-echo') as { ghostSasayaki?: boolean } | undefined;

    expect(late?.ghostSasayaki).toBeUndefined();
  });

  it('marks native echoes that arrive just before the local composer bubble', () => {
    const native = roomMessage({
      id: 'native-early-echo',
      agentId: 'agent-a',
      text: '[Image #15]button spacing',
      timestamp: '2026-06-07T20:00:00Z',
    });
    const local = localComposerMessage({
      id: 'local-image-prompt',
      text: 'project-memory/attachments/composer/att_1/original.png composer button spacing',
      targetAgentIds: ['agent-a'],
      timestamp: '2026-06-07T20:00:02.000Z',
    });

    const merged = mergeRoomMessages([native], [local]);
    const early = merged.find((message) => message.id === 'native-early-echo') as { ghostSasayaki?: boolean } | undefined;

    expect(merged.map((message) => message.id)).toEqual(['local-image-prompt', 'native-early-echo']);
    expect(early?.ghostSasayaki).toBe(true);
  });

  it('marks internal KOTA_MESSAGE native echoes with provider attachment markers as Ghost Sasayaki', () => {
    const native = roomMessage({
      id: 'native-kota-message-echo',
      agentId: 'agent-a',
      text: '[Image #13]<KOTA_MESSAGE id="ember-reminder-1" from="ember" to="agent-a" intent="reminder">\nhello\n</KOTA_MESSAGE>',
      timestamp: '2026-06-07T20:00:00.000Z',
    });

    const merged = mergeRoomMessages([native], []);
    const echo = merged[0] as { ghostSasayaki?: boolean } | undefined;

    expect(echo?.ghostSasayaki).toBe(true);
  });

  it('marks internal KOTA_MESSAGE native echoes with trailing terminal controls as Ghost Sasayaki', () => {
    const native = roomMessage({
      id: 'native-kota-message-control-echo',
      agentId: 'agent-a',
      text: '<KOTA_MESSAGE id="ember-reminder-1" from="ember" to="agent-a" intent="reminder">\nhello\n</KOTA_MESSAGE>\u0015',
      timestamp: '2026-06-07T20:00:00.000Z',
    });

    const merged = mergeRoomMessages([native], []);
    const echo = merged[0] as { ghostSasayaki?: boolean } | undefined;

    expect(echo?.ghostSasayaki).toBe(true);
  });

});

describe('Violet room pagination merge', () => {
  it('keeps older loaded pages when the live window is already full', () => {
    const current = numberedMessages(200, 400);
    const older = numberedMessages(170, 200);

    const merged = mergeOlderRoomMessages(current, older);

    expect(merged).toHaveLength(230);
    expect(merged[0]?.id).toBe('m-170');
    expect(merged.at(-1)?.id).toBe('m-399');
  });

  it('does not recut loaded history on later live sync', () => {
    const current = mergeOlderRoomMessages(numberedMessages(200, 400), numberedMessages(170, 200));

    const merged = mergeSyncedNativeMessages(current, numberedMessages(400, 401), {
      preserveLoadedHistory: true,
    });

    expect(merged).toHaveLength(231);
    expect(merged[0]?.id).toBe('m-170');
    expect(merged.at(-1)?.id).toBe('m-400');
  });

  it('keeps normal live sync bounded before history is expanded', () => {
    const current = numberedMessages(0, 200);

    const merged = mergeSyncedNativeMessages(current, numberedMessages(200, 201));

    expect(merged).toHaveLength(200);
    expect(merged[0]?.id).toBe('m-1');
    expect(merged.at(-1)?.id).toBe('m-200');
  });

  it('keeps a loaded history page even when the merged count is exactly the live limit', () => {
    const current = mergeOlderRoomMessages(numberedMessages(30, 200), numberedMessages(0, 30));

    const merged = mergeSyncedNativeMessages(current, numberedMessages(200, 201), {
      preserveLoadedHistory: true,
    });

    expect(merged).toHaveLength(201);
    expect(merged[0]?.id).toBe('m-0');
    expect(merged.at(-1)?.id).toBe('m-200');
  });
});

describe('Agent Bus delivery reconciliation', () => {
  const now = Date.parse('2026-06-06T12:00:00.000Z');
  const busMessage = (id: string, ageMs: number) => roomMessage({
    id, nativeEventId: `agentbus-${id}`, shell: 'system', role: 'assistant',
    targetAgentIds: ['agent-b'], timestamp: new Date(now - ageMs).toISOString(), text: `Message ${id}`,
  });
  type Deliveries = ReturnType<typeof reconcileAgentBusDeliveries>;

  it('checks evidence only for the three eligible candidates among 745 messages', () => {
    const old = Array.from({ length: 742 }, (_, i) => busMessage(`old-${i}`, 196_000));
    const eligible = [busMessage('one', 180_000), busMessage('two', 185_000), busMessage('three', 195_000)];
    const readEvidence = vi.fn(() => '<KOTA_MESSAGE id="unrelated">body</KOTA_MESSAGE>');
    const evidence = roomMessage({ agentId: 'agent-b' });
    Object.defineProperty(evidence, 'text', { get: readEvidence });

    const next = reconcileAgentBusDeliveries({}, [...old, ...eligible, evidence], [], now);

    expect(Object.keys(next)).toEqual(['one', 'two', 'three']);
    // Each confirmation check visits this native user once. No old candidate
    // may enter that scan, even though all of them remain loaded in the room.
    expect(readEvidence).toHaveBeenCalledTimes(3);
  });

  it('does not check evidence before the deadline or for invalid and expired untracked messages', () => {
    const messages = [busMessage('young', 179_999), busMessage('expired', 195_001),
      { ...busMessage('invalid', 0), timestamp: 'invalid' }];
    const readEvidence = vi.fn(() => 'not an envelope');
    const evidence = roomMessage({ agentId: 'agent-b' });
    Object.defineProperty(evidence, 'text', { get: readEvidence });
    const previous = {};
    expect(reconcileAgentBusDeliveries(previous, [...messages, evidence], [], now)).toBe(previous);
    expect(readEvidence).not.toHaveBeenCalled();
    expect(nextAgentBusDeliveryDeadline(messages, {}, now)).toBe(now + 1);
  });

  it('combines native and receipt confirmations including retry ids for an old tracked message', () => {
    const message = { ...busMessage('old', 600_000), targetAgentIds: ['agent-b', 'agent-c', 'agent-d'] };
    const previous: Deliveries = { old: {
      status: 'unconfirmed', retryTargetAgentIds: message.targetAgentIds,
      attemptEventIds: ['agentbus-old:retry:1'], lastAttemptAt: now - 200_000,
    } };
    const native = roomMessage({ agentId: 'agent-c', text:
      '[Image #1]<KOTA_MESSAGE id="agentbus-old:retry:1">body</KOTA_MESSAGE>\u0015' });
    const receipts = [{ agentId: 'agent-b', eventId: 'agentbus-old', timestamp: message.timestamp }];
    const next = reconcileAgentBusDeliveries(previous, [message, native], receipts, now);
    expect(next.old.retryTargetAgentIds).toEqual(['agent-d']);
    const finalReceipts = [...receipts, { agentId: 'agent-d', eventId: 'agentbus-old:retry:1', timestamp: message.timestamp }];
    expect(reconcileAgentBusDeliveries(next, [message, native], finalReceipts, now)).toEqual({});
  });

  it('preserves Local pending guards and removes tracked messages that leave the room input', () => {
    const message = busMessage('held', 185_000);
    const keys = new Set(['agentbus-held\u001fagent-b']);
    expect(reconcileAgentBusDeliveries({}, [message], [], now, new Set(), keys)).toEqual({});
    const tracked = reconcileAgentBusDeliveries({}, [message], [], now);
    expect(tracked.held.status).toBe('unconfirmed');
    expect(reconcileAgentBusDeliveries(tracked, [message], [], now, keys)).toEqual({});
    expect(reconcileAgentBusDeliveries(tracked, [], [], now)).toEqual({});
  });

  it('schedules only unresolved or retrying deadlines and preserves failure reasons', () => {
    const messages = ['retry', 'failed', 'unconfirmed'].map(id => busMessage(id, 600_000));
    const previous: Deliveries = Object.fromEntries(messages.map(message => [message.id, {
      status: message.id === 'retry' ? 'retrying' : message.id as 'failed' | 'unconfirmed',
      reason: 'Existing reason', retryTargetAgentIds: ['agent-b'], attemptEventIds: [message.nativeEventId!],
      lastAttemptAt: now - 179_000,
    }]));
    expect(nextAgentBusDeliveryDeadline(messages, previous, now)).toBe(now + 1_000);
    expect(reconcileAgentBusDeliveries(previous, messages, [], now).retry.status).toBe('retrying');
    const next = reconcileAgentBusDeliveries(previous, messages, [], now + 1_000);
    expect(next.retry.status).toBe('unconfirmed');
    expect(next.failed.reason).toBe('Existing reason');
    expect(nextAgentBusDeliveryDeadline(messages, next, now + 1_000)).toBe(Infinity);
  });
});

describe('Violet room text memoization', () => {
  afterEach(() => vi.restoreAllMocks());

  it('normalizes only incoming message objects on repeated history merges', () => {
    const normalize = vi.spyOn(dedupeText, 'normalizeForDedupe');
    const messages = numberedMessages(0, 200);
    mergeSyncedNativeMessages([], messages);
    expect(normalize).toHaveBeenCalledTimes(200);
    normalize.mockClear();
    const incoming = numberedMessages(200, 201);
    const merged = mergeSyncedNativeMessages(messages, incoming, { preserveLoadedHistory: true });
    expect(merged).toHaveLength(201);
    expect(normalize).toHaveBeenCalledTimes(1);
    normalize.mockClear();
    expect(mergeSyncedNativeMessages(merged, incoming, { preserveLoadedHistory: true })).toBe(merged);
    expect(normalize).not.toHaveBeenCalled();
  });

  it('invalidates changed text and never caches recipients or the time bucket', () => {
    const first = roomMessage({ id: 'first', targetAgentIds: ['agent-b'] });
    const second = roomMessage({ id: 'second', targetAgentIds: ['agent-b'] });
    expect(mergeSyncedNativeMessages([], [first, second])).toHaveLength(1);
    second.targetAgentIds = ['agent-c'];
    expect(mergeSyncedNativeMessages([], [first, second])).toHaveLength(2);
    second.targetAgentIds = ['agent-b'];
    second.timestamp = '2026-05-18T08:54:00.000Z';
    expect(mergeSyncedNativeMessages([], [first, second])).toHaveLength(2);
    second.timestamp = first.timestamp;
    second.text = 'different text';
    expect(mergeSyncedNativeMessages([], [first, second])).toHaveLength(2);
    second.text = first.text;
    expect(mergeSyncedNativeMessages([], [first, second])).toHaveLength(1);
    // Reusing an id in a different source/project must not reuse cached text.
    expect(mergeSyncedNativeMessages([], [first, { ...second, text: 'another source' }])).toHaveLength(2);
  });

  it('reuses prepared composer text and keeps attachment and changed-text comparisons exact', () => {
    const prepare = vi.spyOn(dedupeText, 'prepareDedupeText');
    // A native echo uses the composer delivery preparation, which peels host envelopes.
    const echo = vi.spyOn(dedupeText, 'prepareComposerDeliveryDedupeText');
    const native = roomMessage({ id: 'native', text: '[Image #1]same prompt' });
    const local = localComposerMessage({
      id: 'local', targetAgentIds: ['agent-a'], text: 'project-memory/attachments/composer/att_1/original.png same prompt',
    });
    expect(mergeRoomMessages([native], [local])).toHaveLength(1);
    expect(prepare).toHaveBeenCalledTimes(1);
    expect(echo).toHaveBeenCalledTimes(1);
    prepare.mockClear(); echo.mockClear();
    expect(mergeRoomMessages([native], [local])).toHaveLength(1);
    expect(prepare).not.toHaveBeenCalled();
    expect(echo).not.toHaveBeenCalled();
    local.text = 'a different prompt';
    expect(mergeRoomMessages([native], [local])).toHaveLength(2);
    expect(prepare).toHaveBeenCalledTimes(1);
  });
});

function numberedMessages(start: number, end: number): VioletChatMessage[] {
  const base = Date.parse('2026-06-06T00:00:00.000Z');
  return Array.from({ length: Math.max(0, end - start) }, (_, offset) => {
    const index = start + offset;
    return roomMessage({
      id: `m-${index}`,
      timestamp: new Date(base + index * 1000).toISOString(),
      text: `message ${index}`,
    });
  });
}

function roomMessage(overrides: Partial<VioletChatMessage>): VioletChatMessage {
  return {
    id: 'm',
    sessionId: 's',
    agentId: 'agent-a',
    shell: 'claude',
    role: 'user',
    kind: 'message',
    timestamp: '2026-05-18T08:51:26.530Z',
    text: 'hello',
    sourcePath: null,
    nativeEventId: null,
    ...overrides,
  };
}

function localComposerMessage(
  overrides: Partial<VioletChatMessage> & { targetAgentIds: string[] },
): VioletChatMessage & {
  local: true;
  projectRoot: string;
  targetAgentIds: string[];
} {
  return {
    ...roomMessage({
      agentId: 'user',
      shell: 'composer',
      ...overrides,
    }),
    local: true,
    projectRoot: '/Users/example/Kota/Workspaces/demo-project',
    targetAgentIds: overrides.targetAgentIds,
  };
}


it('uses the explicit shell handoff origin for a whisper without turning the original input into one', () => {
  const base = { sessionId: 'new', agentId: 'agent-a', shell: 'claude', kind: 'message', timestamp: '2026-09-20T12:00:00Z' };
  const native: VioletChatMessage[] = [
    { ...base, id: 'handoff-id', role: 'system', text: 'Read project memory.', messageOrigin: 'shell_handoff' },
    { ...base, id: 'original-id', role: 'user', text: 'Continue the task.' },
  ];
  const merged = mergeRoomMessages(native, []);
  expect(merged.find((m) => m.id === 'handoff-id')).toMatchObject({ ghostSasayaki: true, text: 'Read project memory.' });
  expect(merged.find((m) => m.id === 'original-id')?.ghostSasayaki).toBeUndefined();
});
