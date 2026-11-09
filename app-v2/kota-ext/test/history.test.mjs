import test from 'node:test';
import assert from 'node:assert/strict';
import { HISTORY_BYTES, HISTORY_CODEC, historyReply, parseHistoryRequest } from '../history.ts';

const decode = text => JSON.parse(text.slice(HISTORY_CODEC.length));
const entry = (role, content) => ({ type: 'message', id: 'entry-1', parentId: 'parent', timestamp: '2026-10-26T00:00:00Z', message: { role, content, toolCallId: 'call-1', toolName: 'read', isError: false, details: { image: 'keep' } } });
const image = { type: 'image', mimeType: 'image/png', data: 'unchanged-original-image' };

test('full history removes only toolResult image blocks, retaining metadata, branches and receipts', () => {
  const entries = [entry('toolResult', [{ type: 'text', text: '原文\n"unchanged"' }, image]),
    entry('user', [image]), { type: 'custom_message', customType: 'kota-message', id: 'receipt', content: [image], details: { kotaId: 'receipt-id' } },
    { type: 'custom', id: 'custom', data: { content: [image] } },
    { type: 'compaction', id: 'compacted', parentId: 'entry-1', firstKeptEntryId: 'receipt', summary: 'retained' }];
  const before = structuredClone(entries);
  const expected = structuredClone(entries); expected[0].message.content.pop();
  const result = decode(historyReply('local-1-2', entries, 'receipt'));
  assert.deepEqual(result, { schema: 1, requestId: 'local-1-2', result: { status: 'ready', entries: expected, leafId: 'receipt' } });
  assert.deepEqual(entries, before, 'Pi original history remains unchanged');
});

test('an image-only tool result retains its entry with an empty content array', () => {
  const original = entry('toolResult', [image]);
  assert.deepEqual(decode(historyReply('local-1-2', [original], 'entry-1')).result.entries,
    [{ ...original, message: { ...original.message, content: [] } }]);
  assert.deepEqual(original.message.content, [image]);
  assert.deepEqual(decode(historyReply('local-1-3', [], null)).result, { status: 'ready', entries: [], leafId: null });
});

test('private full-history command rejects cursors, paths and malformed request IDs', () => {
  assert.deepEqual(parseHistoryRequest('{"schema":1,"requestId":"local-1-2"}'), { schema: 1, requestId: 'local-1-2' });
  for (const args of ['broken', 'null', '{"schema":2,"requestId":"a"}', '{"schema":1,"requestId":"a","since":"cursor"}',
    '{"schema":1,"requestId":"a","path":"/tmp"}', '{"schema":1,"requestId":"a\\nb"}']) assert.equal(parseHistoryRequest(args), undefined);
});

test('text-only oversize returns a short error and counts UTF-8 bytes rather than string length', () => {
  for (const text of ['a'.repeat(HISTORY_BYTES), '中'.repeat(Math.ceil(HISTORY_BYTES / 3))]) {
    const original = entry('toolResult', [{ type: 'text', text }]);
    const reply = historyReply('local-1-2', [original], 'entry-1');
    assert.deepEqual(decode(reply), { schema: 1, requestId: 'local-1-2', result: { status: 'too_large' } });
    assert(Buffer.byteLength(reply) < 256);
    assert.equal(original.message.content[0].text, text);
  }
});

test('a near-limit escaped payload still fits the existing 64 MiB outer record', () => {
  const text = '"'.repeat(Math.floor((HISTORY_BYTES - 1024) / 2));
  const reply = historyReply('local-1-2', [entry('toolResult', [{ type: 'text', text }])], 'entry-1');
  assert.equal(decode(reply).result.status, 'ready');
  const record = JSON.stringify({ type: 'extension_ui_request', method: 'notify', id: 'notification-id', message: reply, notifyType: 'info' }) + '\n';
  assert(Buffer.byteLength(record) < 64 * 1024 * 1024);
});
