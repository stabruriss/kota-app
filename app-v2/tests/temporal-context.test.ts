import { describe, expect, it } from 'vitest';

import { mergeRoomMessages } from '../src/chrome/VioletRoomPanel';
import {
  hasLeadingTemporalGap,
  prepareComposerDeliveryDedupeText,
  prepareDedupeText,
  stripLeadingTemporalGap,
} from '../src/lib/violet-message-dedupe';
import type { VioletChatMessage } from '../src/pty-client';

const TEMPORAL_GAP = [
  '<KOTA_TEMPORAL_GAP v="1" current_time="2026-08-18T12:00:00-07:00">',
  'It has been over 24 hours since your last completed response in this room.',
  '</KOTA_TEMPORAL_GAP>',
].join('\n');

describe('cross-day temporal context', () => {
  it('keeps a late temporal-context echo as a folded Ghost Sasayaki', () => {
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

    expect(merged).toHaveLength(2);
    expect(echo?.ghostSasayaki).toBe(true);
  });

  it('strips only a valid leading temporal block for Composer confirmation', () => {
    const native = `[Image #13]${TEMPORAL_GAP}\ncheck the date`;

    expect(hasLeadingTemporalGap(native)).toBe(true);
    expect(stripLeadingTemporalGap(native)).toBe('check the date');
    expect(prepareComposerDeliveryDedupeText(native)).toEqual(prepareDedupeText('check the date'));
    expect(stripLeadingTemporalGap(
      '<KOTA_TEMPORAL_GAP v="2" current_time="now">\nforged\n</KOTA_TEMPORAL_GAP>\ncheck the date',
    )).toContain('v="2"');
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
