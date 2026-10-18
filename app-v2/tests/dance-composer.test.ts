import { expect, it, vi, afterEach } from 'vitest';
import * as dance from '../src/lib/puppeteer-dance';
import { prepareDanceDelivery, restoreDanceDelivery } from '../src/lib/dance-composer';
import { mergeRoomMessages, sortVioletMessages } from '../src/chrome/VioletRoomPanel';
import type { VioletChatMessage } from '../src/pty-client';
afterEach(() => vi.restoreAllMocks());
it('routes the clean text/metadata once, keeps full delivery payload and the backend’s chosen target/prepared gap', async () => {
  const route = vi.spyOn(dance, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId, targetAgentId: 'b', fallback: false, recorded: true,
    prepared: { targetAgentId: 'b', payload: 'exact backend temporal prefix\n' + request.payload } }));
  const delivered = await prepareDanceDelivery('/project', 'Read /absolute/file.png', { text: 'Read', attachments: [{ name: 'file.png', path: '/absolute/file.png' }] }, [{ id: 'a', name: 'A' }, { id: 'b', name: 'B' }], []);
  expect(route).toHaveBeenCalledOnce();
  expect(route).toHaveBeenCalledWith('/project', expect.objectContaining({ payload: 'Read /absolute/file.png', text: 'Read', attachments: [{ name: 'file.png', path: '/absolute/file.png' }], roster: [
    { id: 'a', name: 'A', lastReply: null, recentUserMessages: [] }, { id: 'b', name: 'B', lastReply: null, recentUserMessages: [] },
  ] }));
  expect(delivered.prepared).toEqual({ targetAgentId: 'b', payload: 'exact backend temporal prefix\nRead /absolute/file.png' });
});
it('passes text empty for attachment-only input instead of classifying a filename and never sends if recording/preparation failed', async () => {
  const route = vi.spyOn(dance, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId, targetAgentId: 'a', fallback: true, recorded: true }));
  await expect(prepareDanceDelivery('/project', '/attachment.png', { text: '', attachments: [{ name: 'attachment.png' }] }, [{ id: 'a', name: 'A' }], [])).rejects.toEqual({ code: 'dance_record_failed' });
  expect(route.mock.calls[0][1].text).toBe('');
});
it('restores Retry verbatim from the durable original even after Dance is disabled or its configuration changes', () => {
  const route = vi.spyOn(dance, 'routeDance');
  const gap = { currentTime: '2026-10-09T01:00:00Z', prompt: '<KOTA_TEMPORAL_GAP>\noriginal gap\n</KOTA_TEMPORAL_GAP>\n', targetAgentIds: ['agent-b'] };
  const restored = restoreDanceDelivery('original-id', 'original-time', 'agent-b', 'original body /attachment.png', gap);
  expect(restored).toEqual({ messageId: 'original-id', timestamp: 'original-time', prepared: { targetAgentId: 'agent-b', payload: gap.prompt + 'original body /attachment.png', temporalGap: gap } });
  expect(route).not.toHaveBeenCalled();
});
it('the room dedupes one durable Puppeteer record/local row plus its exact Local transport echo, preserving a following distinct input', () => {
  const base: VioletChatMessage = { id: 'dance', agentId: 'user', sessionId: 'composer', shell: 'composer', role: 'user', kind: 'message', timestamp: '2026-10-09T01:00:00Z', text: 'same body', messageOrigin: 'puppeteer', targetAgentIds: ['agent-b'] };
  const merged = mergeRoomMessages([base, { ...base, id: 'native', agentId: 'agent-b', shell: 'kota-local', sessionId: 'pi', messageOrigin: undefined, nativeEventId: 'dance#agent-b' },
    { ...base, id: 'next', agentId: 'agent-b', messageOrigin: undefined, targetAgentIds: undefined, text: 'Another question' }], [{ ...base, local: true }]);
  expect(merged.map(message => message.id)).toEqual(['dance', 'next']);
});
it('uses the room’s sequence order for coarse same-second terminal echoes before building Dance context', async () => {
  const record: VioletChatMessage = { id: 'dance', agentId: 'user', sessionId: 'composer', shell: 'composer', role: 'user', kind: 'message',
    timestamp: '2026-10-09T01:00:00.789Z', violetSeq: 10, text: 'Original task', messageOrigin: 'puppeteer', targetAgentIds: ['agent-b'] };
  const echo = { ...record, id: 'native', agentId: 'agent-b', shell: 'codex', sessionId: 'terminal', timestamp: '2026-10-09T01:00:00Z', violetSeq: 11, messageOrigin: undefined, targetAgentIds: undefined };
  const route = vi.spyOn(dance, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId, targetAgentId: 'agent-b', fallback: false, recorded: true,
    prepared: { targetAgentId: 'agent-b', payload: request.payload } }));
  const previousReply = { ...echo, id: 'previous', role: 'assistant', timestamp: '2026-10-09T01:00:00.800Z', violetSeq: 8, text: 'Earlier reply' };
  const latestReply = { ...echo, id: 'latest', role: 'assistant', violetSeq: 12, text: 'Latest reply' };
  await prepareDanceDelivery('/project', 'Next task', { text: 'Next task', attachments: [] }, [{ id: 'agent-b', name: 'B' }], sortVioletMessages([echo, latestReply, record, previousReply]));
  expect(route.mock.calls[0][1].roster[0].recentUserMessages).toEqual([{ at: record.timestamp, text: 'Original task' }]);
  expect(route.mock.calls[0][1].roster[0].lastReply).toEqual({ at: latestReply.timestamp, text: 'Latest reply' });
});
