/** Pinned Pi, isolated HOME, loopback fake provider, synthetic session files only. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { appendFile, writeFile, stat } from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { createTestSession, TestRpcClient, resolveTestPi } from './fake-provider/rpc-client.mjs';
import { createFakeProvider } from './fake-provider/server.mjs';
import { extensionPath } from './fixtures.mjs';
import { HISTORY_CODEC } from '../history.ts';

const SESSION = '11111111-2222-4333-8444-555555555555';
const stamp = '2026-10-26T00:00:00.000Z';
async function digest(path) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest('hex');
}
async function fixture(t) {
  const provider = await createFakeProvider({ steps: [{ type: 'text', text: 'Unexpected model request' }] });
  t.after(() => provider.close());
  // Bootstrap only the isolated configuration here; load the product extension
  // on the actual restore below, instead of loading it twice per fixture.
  const f = await createTestSession({ baseUrl: provider.baseUrl, persistent: true, sessionId: SESSION });
  t.after(() => f.close());
  const initial = (await f.client.command('get_entries')).data.entries;
  await f.client.close();
  const file = join(f.directory, 'sessions', `2026-10-26T00-00-00-000Z_${SESSION}.jsonl`);
  await writeFile(file, JSON.stringify({ type: 'session', version: 3, id: SESSION, timestamp: stamp, cwd: f.cwd }) + '\n');
  const append = async entry => appendFile(file, JSON.stringify({ timestamp: stamp, ...entry }) + '\n');
  for (const entry of initial) await append(entry);
  const open = async (id = SESSION) => {
    const args = [...f.args, '-e', extensionPath]; args[args.indexOf('--session-id') + 1] = id;
    const policy = '(version 1)(allow default)(deny network*)(allow network-outbound (remote ip "localhost:*"))(allow network-inbound (local ip "localhost:*"))';
    const client = new TestRpcClient('/usr/bin/sandbox-exec', ['-p', policy, resolveTestPi(), ...args], { cwd: f.cwd, env: f.env });
    t.after(() => client.close());
    const state = await client.command('get_state').catch(error => { throw new Error('restore get_state: ' + error.message); });
    assert.equal(state.data.sessionId, id);
    return client;
  };
  return { file, append, open, provider, initial };
}
async function history(client, requestId) {
  const read = r => r.type === 'extension_ui_request' && r.method === 'notify' && r.message?.startsWith(HISTORY_CODEC)
    ? JSON.parse(r.message.slice(HISTORY_CODEC.length)) : undefined;
  const pending = client.send('prompt', { message: `/kota-history ${JSON.stringify({ schema: 1, requestId })}` });
  const result = read(await client.waitFor(r => read(r)?.requestId === requestId)
    .catch(error => { throw new Error('history notification: ' + error.message); }));
  const response = await pending.response;
  assert.equal(response.success, true); assert.equal(response.data.disposition, 'handled');
  return result.result;
}

test('real Pi restores >64 MiB image history, preserves its file and native incremental images', { timeout: 60000 }, async t => {
  const f = await fixture(t);
  const image = { type: 'image', mimeType: 'image/png', data: 'A'.repeat(1024 * 1024) };
  for (let i = 0; i < 65; i++) await f.append({ type: 'message', id: `tool-${i}`, parentId: i ? `tool-${i - 1}` : f.initial.at(-1).id,
    message: { role: 'toolResult', toolName: 'read', toolCallId: `call-${i}`, content: [image], isError: false, timestamp: Date.parse(stamp) } });
  const custom = { type: 'custom_message', id: 'receipt', parentId: 'tool-64', customType: 'kota.user', content: 'Receipt kept',
    display: false, details: { schema: 1, kotaId: 'msg#agent-fixture', messageId: 'msg', targetAgentId: 'agent-fixture', createdAt: stamp, private: false } };
  await f.append(custom);
  const before = await digest(f.file); assert((await stat(f.file)).size > 64 * 1024 * 1024);
  const client = await f.open(); const result = await history(client, 'history-large-images');
  assert.equal(result.status, 'ready'); assert.equal(result.entries.length, f.initial.length + 66); assert.equal(result.leafId, 'receipt');
  assert.deepEqual(result.entries.slice(0, f.initial.length), f.initial);
  for (const row of result.entries.slice(f.initial.length, f.initial.length + 65)) assert.deepEqual(row.message.content, []);
  assert.deepEqual(result.entries.at(-1), { timestamp: stamp, ...custom });
  const delta = await client.command('get_entries', { since: 'tool-63' });
  assert.equal(delta.success, true); assert.equal(delta.data.entries.length, 2);
  assert.deepEqual(delta.data.entries[0].message.content, [image], 'native history still owns the image');
  assert.equal(client.records.some(r => r.type === 'agent_start'), false, 'private command never prompts the model');
  assert(client.lines.every(line => Buffer.byteLength(line) < 64 * 1024 * 1024));
  await client.close(); assert.equal(await digest(f.file), before);
  assert.equal(f.provider.requests.length, 0);
});

test('real Pi text oversize gives a short error; a fresh session works and leaves the old file intact', { timeout: 60000 }, async t => {
  const f = await fixture(t);
  await f.append({ type: 'message', id: 'long-text', parentId: f.initial.at(-1).id,
    message: { role: 'user', content: [{ type: 'text', text: 'x'.repeat(33 * 1024 * 1024) }], timestamp: Date.parse(stamp) } });
  const before = await digest(f.file); const client = await f.open();
  assert.deepEqual(await history(client, 'history-large-text'), { status: 'too_large' });
  assert(client.lines.every(line => Buffer.byteLength(line) < 65536));
  assert.equal(client.records.some(r => r.type === 'agent_start'), false);
  await client.close();
  const fresh = await f.open('22222222-2222-4333-8444-555555555555');
  const result = await history(fresh, 'history-fresh');
  assert.equal(result.status, 'ready');
  assert.deepEqual(result.entries.map(entry => entry.type), ['model_change', 'thinking_level_change']);
  assert.equal(result.leafId, result.entries.at(-1).id);
  await fresh.close(); assert.equal(await digest(f.file), before);
  assert.equal(f.provider.requests.length, 0);
});
