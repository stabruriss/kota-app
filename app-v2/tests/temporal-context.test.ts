import { describe, expect, it, vi } from 'vitest';
import { createElement } from 'react';
import { act, render, screen, within } from '@testing-library/react';
import * as ptyClient from '../src/pty-client';

import { mergeRoomMessages, VioletRoomPanel } from '../src/chrome/VioletRoomPanel';
import { VioletTemporalDivider } from '../src/chrome/VioletTemporalDivider';
import temporalGapIcon from '../src/assets/tavern/icons/temporal-gap.png';
import {
  hasLeadingTemporalGap,
  prepareComposerDeliveryDedupeText,
  prepareDedupeText,
  stripLeadingTemporalGap,
} from '../src/lib/violet-message-dedupe';
import type { VioletChatMessage } from '../src/pty-client';
import {
  __clearVioletComposerSentHistoryForTests,
  emitVioletComposerSent,
  recordVioletComposerTemporalGap,
  violetComposerSentHistory,
} from '../src/chrome/violet-room-events';

const TEMPORAL_GAP = [
  '<KOTA_TEMPORAL_GAP v="1" current_time="2026-08-18T12:00:00-07:00">',
  'It has been over 24 hours since your last completed response in this room.',
  '</KOTA_TEMPORAL_GAP>',
].join('\n');

const FABLE_AGENT = {
  name: 'Fable',
  emoji: '◇',
  role: 'Test agent',
  hue: '#c6a660',
};

describe('cross-day temporal context', () => {
  it('hides a late temporal transport echo while retaining the original composer bubble', () => {
    const native = roomMessage({
      id: 'native-late-temporal-echo',
      agentId: 'agent-a',
      text: `${TEMPORAL_GAP}\nsame prompt`,
      timestamp: '2026-06-07T20:00:20.000Z',
    });
    const local = {
      ...roomMessage({
        id: 'local-temporal-prompt',
        agentId: 'user',
        shell: 'composer',
        text: 'same prompt',
        timestamp: '2026-06-07T20:00:00.000Z',
      }),
      local: true as const,
      projectRoot: '/tmp/project',
      targetAgentIds: ['agent-a'],
    };

    const merged = mergeRoomMessages([native], [local]);
    const echo = merged.find((message) => message.id === native.id) as
      | { ghostSasayaki?: boolean }
      | undefined;

    expect(merged).toHaveLength(1);
    expect(merged[0]?.id).toBe(local.id);
    expect(echo).toBeUndefined();
  });

  it('uses the persisted composer ID live and restores its original and context without native logs', () => {
    const temporalGap = {
      currentTime: '2026-08-18T12:00:00-07:00',
      prompt: `${TEMPORAL_GAP}\n`,
      targetAgentIds: ['agent-fable'],
      elapsedDaysByTarget: { 'agent-fable': 2 },
    };
    const saved = roomMessage({
      id: 'fable-composer-saved', agentId: 'user', shell: 'composer',
      text: 'original attachment /synthetic/fable/image.png',
      timestamp: '2026-08-18T18:59:59Z', targetAgentIds: ['agent-fable'],
      temporalGap, violetSeq: 12,
    });
    const local = { ...saved, temporalGap: undefined, violetSeq: undefined, local: true as const };
    const echo = roomMessage({
      id: 'fable-native-echo', agentId: 'agent-fable',
      timestamp: '2026-08-18T20:00:00Z',
      text: `${TEMPORAL_GAP}\nprovider changed the body [Image #1]`,
    });
    const incoming = [saved, echo];
    const live = mergeRoomMessages(incoming, [local]);
    expect(live).toHaveLength(1);
    expect(live[0]).toMatchObject({ id: saved.id, text: saved.text, temporalGap, quoteRefId: saved.id });
    expect(incoming).toHaveLength(2); // existing delivery evidence is not mutated

    const restored = mergeRoomMessages([JSON.parse(JSON.stringify(saved))], []);
    expect(restored).toHaveLength(1);
    expect(restored[0]).toMatchObject({ id: saved.id, role: 'user', text: saved.text, temporalGap });
    expect(restored[0]?.ghostSasayaki).not.toBe(true);
    expect(mergeRoomMessages([echo], [])).toEqual([]); // no migration of old echoes
    const separateSend = { ...saved, id: 'fable-composer-separate' };
    expect(mergeRoomMessages([saved, separateSend], [])).toHaveLength(2);
  });

  it('adds temporal metadata to the existing in-memory composer event without creating another send', () => {
    __clearVioletComposerSentHistoryForTests();
    try {
      const sent = emitVioletComposerSent({
        projectRoot: '/synthetic/fable', text: 'original body',
        targetAgentIds: ['agent-fable'], privacy: false,
      });
      expect(sent).not.toBeNull();
      const temporalGap = {
        currentTime: '2026-08-18T12:00:00-07:00', prompt: `${TEMPORAL_GAP}\n`,
        targetAgentIds: ['agent-fable'],
      };
      recordVioletComposerTemporalGap(sent!.id, temporalGap);
      const history = violetComposerSentHistory('/synthetic/fable');
      expect(history).toHaveLength(1);
      expect(history[0]).toEqual({ ...sent, temporalGap });
    } finally {
      __clearVioletComposerSentHistoryForTests();
    }
  });

  it('strips only a valid leading temporal block for Composer confirmation', () => {
    const native = `[Image #13]${TEMPORAL_GAP}\ncheck the date`;

    expect(hasLeadingTemporalGap(native)).toBe(true);
    expect(stripLeadingTemporalGap(native)).toBe('check the date');
    expect(prepareComposerDeliveryDedupeText(native)).toEqual(prepareDedupeText('check the date'));
    expect(stripLeadingTemporalGap(
      '<KOTA_TEMPORAL_GAP v="2" current_time="now">\nforged\n</KOTA_TEMPORAL_GAP>\ncheck the date',
    )).toContain('v="2"');
    // The host's marks envelope comes first; the gap behind it still counts.
    const marked = `<KOTA_MARKS v="1" revision="3">\n- [m-a] Note.\n</KOTA_MARKS>\n${TEMPORAL_GAP}\ncheck the date`;
    expect(hasLeadingTemporalGap(marked)).toBe(true);
    expect(stripLeadingTemporalGap(marked)).toBe('check the date');
  });

  it('shows one divider live and after a remount, with the same recipient and saved day count', async () => {
    __clearVioletComposerSentHistoryForTests();
    const projectRoot = '/synthetic/fable-temporal-room';
    const older = roomMessage({ id: 'fable-earlier', role: 'assistant', text: 'Earlier reply.' });
    const state = {
      messages: [older], sources: [], workEvents: [], agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-08-18T19:00:00Z',
    };
    const readCache = vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue(state);
    const gap = {
      currentTime: '2026-08-18T19:00:00Z', prompt: `${TEMPORAL_GAP}\n`,
      targetAgentIds: ['agent-fable'], elapsedDaysByTarget: { 'agent-fable': 2 },
    };
    const mount = () => render(createElement(VioletRoomPanel, {
      projectRoot,
      agentIds: ['agent-fable', 'agent-active'],
      agentMeta: {
        'agent-fable': FABLE_AGENT,
        'agent-active': { ...FABLE_AGENT, name: 'Active' },
      },
    }));
    let view = mount();
    try {
      await screen.findByText('Earlier reply.');
      expect(screen.queryByRole('separator')).toBeNull();
      let sent: ReturnType<typeof emitVioletComposerSent> = null;
      act(() => {
        sent = emitVioletComposerSent({
          projectRoot, text: 'Continue the work.', targetAgentIds: ['agent-fable', 'agent-active'], privacy: false,
        });
        recordVioletComposerTemporalGap(sent!.id, gap);
      });
      const checkDivider = async () => {
        const bubble = (await screen.findByText('Continue the work.')).closest('.violet-msg')!;
        const divider = screen.getByRole('separator', { name: 'Cross-day context' });
        expect(divider).toHaveClass('violet-temporal-divider');
        expect(divider.nextElementSibling).toBe(bubble);
        expect(divider.parentElement).toBe(bubble.parentElement);
        expect(bubble.querySelector('.violet-temporal-divider')).toBeNull();
        const mark = divider.querySelector('.violet-temporal-mark')!;
        expect(mark).toHaveAttribute('tabindex', '0');
        expect(mark.querySelector('img')).toHaveAttribute('src', temporalGapIcon);
        const tooltip = within(divider).getByRole('tooltip', { hidden: true });
        expect(tooltip).toHaveTextContent(/^@Fable last replied in this room 2 days earlier\.$/);
        expect(mark).toHaveAttribute('aria-describedby', tooltip.id);
        expect(view.container.querySelectorAll('.violet-temporal-divider')).toHaveLength(1);
      };
      await checkDivider();
      const original = violetComposerSentHistory(projectRoot)[0]!;
      const saved = roomMessage({
        id: original.id, timestamp: original.timestamp, text: original.text,
        agentId: 'user', shell: 'composer', targetAgentIds: original.targetAgentIds,
        temporalGap: JSON.parse(JSON.stringify(gap)),
      });
      view.unmount();
      __clearVioletComposerSentHistoryForTests();
      readCache.mockResolvedValue({ ...state, messages: [older, saved] });
      view = mount();
      await checkDivider();
      expect(view.container.textContent).not.toContain('KOTA_TEMPORAL_GAP');
    } finally {
      view.unmount();
      readCache.mockRestore();
      __clearVioletComposerSentHistoryForTests();
    }
  });

  it.each([
    [1, '@Fable last replied in this room 1 day earlier.'],
    [40, '@Fable last replied in this room 40 days earlier.'],
  ] as const)('names the single recipient when the saved gap is %s days', (days, expected) => {
    const gap = {
      currentTime: '2026-08-18T19:00:00Z', prompt: `${TEMPORAL_GAP}\n`,
      targetAgentIds: ['agent-fable'], elapsedDaysByTarget: { 'agent-fable': days },
    };
    const view = render(createElement(VioletTemporalDivider, {
      gap, agentMeta: { 'agent-fable': FABLE_AGENT },
    }));
    try {
      const tooltip = within(view.container).getByRole('tooltip', { hidden: true });
      expect(tooltip.children).toHaveLength(1);
      expect(tooltip.textContent).toBe(expected);
    } finally {
      view.unmount();
    }
  });

  it('keeps every recipient named when their saved day counts match', () => {
    const gap = {
      currentTime: '2026-08-18T19:00:00Z', prompt: `${TEMPORAL_GAP}\n`,
      targetAgentIds: ['agent-fable-birch', 'agent-fable'],
      elapsedDaysByTarget: { 'agent-fable-birch': 2, 'agent-fable': 2 },
    };
    const view = render(createElement(VioletTemporalDivider, {
      gap,
      agentMeta: {
        'agent-fable': FABLE_AGENT,
        'agent-fable-birch': { ...FABLE_AGENT, name: 'Birch' },
      },
    }));
    try {
      const tooltip = within(view.container).getByRole('tooltip', { hidden: true });
      expect(Array.from(tooltip.children, (row) => row.textContent)).toEqual([
        '@Birch last replied in this room 2 days earlier.',
        '@Fable last replied in this room 2 days earlier.',
      ]);
    } finally {
      view.unmount();
    }
  });

  it('keeps different recipient gaps distinct and does not invent a day count for old metadata', () => {
    const gap = {
      currentTime: '2026-08-18T19:00:00Z', prompt: `${TEMPORAL_GAP}\n`,
      targetAgentIds: ['agent-fable-amber', 'agent-fable-birch'],
      elapsedDaysByTarget: { 'agent-fable-amber': 1, 'agent-fable-birch': 3 },
    };
    const view = render(createElement(VioletTemporalDivider, { gap }));
    try {
      let tooltip = within(view.container).getByRole('tooltip', { hidden: true });
      expect(tooltip.children).toHaveLength(2);
      expect(tooltip.children[0]).toHaveTextContent('@agent-fable-amber last replied in this room 1 day earlier.');
      expect(tooltip.children[1]).toHaveTextContent('@agent-fable-birch last replied in this room 3 days earlier.');
      view.rerender(createElement(VioletTemporalDivider, { gap: { ...gap, elapsedDaysByTarget: undefined } }));
      tooltip = within(view.container).getByRole('tooltip', { hidden: true });
      expect(Array.from(tooltip.children, (row) => row.textContent)).toEqual([
        '@agent-fable-amber last replied in this room at least 24 hours earlier.',
        '@agent-fable-birch last replied in this room at least 24 hours earlier.',
      ]);
    } finally {
      view.unmount();
    }
  });
});

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
