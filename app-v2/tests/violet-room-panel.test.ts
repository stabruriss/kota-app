import { describe, expect, it } from 'vitest';

import {
  mergeOlderRoomMessages,
  mergeRoomMessages,
  mergeSyncedNativeMessages,
  normalizeAttachmentInsensitive,
  normalizeForDedupe,
} from '../src/chrome/VioletRoomPanel';
import type { VioletChatMessage } from '../src/pty-client';

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
      text: 'project-memory/attachments/composer/att_1/original.png same prompt',
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
      timestamp: '2026-06-07T20:00:00.000Z',
    });
    const local = localComposerMessage({
      id: 'local-image-prompt',
      text: 'project-memory/attachments/composer/att_1/original.png button spacing',
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
