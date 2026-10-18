import assert from 'node:assert/strict';
import test from 'node:test';
import { CUSTOM_TYPES } from '../inject.ts';
import { createFakeProvider } from './fake-provider/server.mjs';
import { createTestSession } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand, isConsumed, tinyPng } from './fixtures.mjs';

async function fixture(t, steps, options = {}) {
  const provider = await createFakeProvider({ steps });
  t.after(() => provider.close());
  const session = await createTestSession({ baseUrl: provider.baseUrl, extensionPath, ...options });
  t.after(() => session.close());
  return { provider, client: session.client, session };
}
const delta = record => record.type === 'message_update' && record.assistantMessageEvent.type === 'text_delta';
const settled = record => record.type === 'agent_settled';
const controlled = record => record.type === 'extension_error' && record.extensionPath === 'command:kota-inject' && record.event === 'command';

async function sendAndSettle(client, input) {
  const after = client.records.length;
  const response = await client.command('prompt', injectCommand(input));
  assert.equal(response.success, true);
  assert.equal(response.data.disposition, 'handled');
  await client.waitFor(isConsumed(input.kotaId), { after });
  await client.waitFor(settled, { after });
  assert.equal(client.records.filter(isConsumed(input.kotaId)).length, 1);
}

test('idle injection starts a real Pi run; all sources and structured input keep one identity', { timeout: 30_000 }, async t => {
  const inputs = CUSTOM_TYPES.map((type, index) => envelope(`msg-source-${index}`, type));
  inputs.push(envelope('msg-image', 'kota.user', [
    { type: 'text', text: '  synthetic image input\n' }, { type: 'image', data: tinyPng, mimeType: 'image/png' },
  ]));
  const { provider, client } = await fixture(t, inputs.map(() => ({ type: 'text', text: 'fixture response' })));
  for (const input of inputs) {
    await sendAndSettle(client, input);
    const receipt = client.records.find(isConsumed(input.kotaId));
    assert.deepEqual(receipt.message.content, input.content);
    assert.deepEqual(receipt.message.details, { ...input.details, kotaId: input.kotaId });
    assert.equal(receipt.message.customType, input.customType);
  }
  assert.equal(client.records.filter(record => record.type === 'agent_start').length, inputs.length);
  assert.equal(provider.requests.length, inputs.length);
  const lastUser = provider.requests.at(-1).body.messages.findLast(message => message.role === 'user');
  assert.equal(lastUser.content.some(block => block.type === 'image_url'), true);
});

test('streaming injection is consumed in the same run without interrupting its response', { timeout: 20_000 }, async t => {
  const { provider, client } = await fixture(t, [
    { type: 'long', text: 'fixture stream. ', repeat: 12, chunkSize: 16, holdAfterChunks: 1 },
    { type: 'text', text: 'fixture steering response' },
  ]);
  const first = envelope('msg-running');
  const queued = envelope('msg-queued', 'kota.bus', 'queued original envelope');
  await client.command('prompt', injectCommand(first));
  await client.waitFor(delta);
  await client.command('prompt', injectCommand(queued));
  assert.equal(client.records.some(isConsumed(queued.kotaId)), false);
  assert.equal((await client.command('get_state')).data.isStreaming, true);
  provider.release(1);
  await client.waitFor(settled);
  assert.equal(client.records.filter(record => record.type === 'agent_start').length, 1);
  assert.equal(client.records.filter(isConsumed(queued.kotaId)).length, 1);
  assert.equal(provider.requests.length, 2);
  assert.equal(client.records.some(record => record.type === 'message_end' && record.message.stopReason === 'aborted'), false);
});

test('bad JSON is unknown; parsed failures carry an encoded ID and handled success is not an ACK', { timeout: 20_000 }, async t => {
  const { provider, client } = await fixture(t, [{ type: 'text', text: 'must not be requested' }]);
  const bad = envelope('msg-invalid');
  bad.customType = 'kota.not-a-source';
  for (const [message, expected] of [
    ['/kota-inject {"kotaId":"msg-invalid#agent-fixture", secret-body', 'kota-inject-failed kotaId=unknown code=invalid-json'],
    [`/kota-inject ${JSON.stringify(bad)}`, 'kota-inject-failed kotaId=msg-invalid%23agent-fixture code=invalid-envelope'],
  ]) {
    const after = client.records.length;
    const response = await client.command('prompt', { message });
    assert.equal(response.success, true);
    assert.equal(response.data.disposition, 'handled');
    const error = await client.waitFor(controlled, { after });
    assert.equal(error.error, expected);
    assert.equal(client.records.indexOf(error) < client.records.indexOf(response), true);
  }
  assert.equal(provider.requests.length, 0);
  assert.equal(client.records.some(record => record.type === 'agent_start'), false);
});

test('automatic compaction accepts held input and the next boundary consumes it without refusal', {timeout:25000}, async t=>{
 const {provider,client}=await fixture(t,[
 {type:'tool',name:'read',arguments:{path:'fixture.txt'},holdAfterChunks:1},
 {type:'text',text:'summary',holdBeforeHeaders:true},
 {type:'text',text:'next response'},
 {type:'text',text:'second summary'},
 {type:'text',text:'held response'}], {settings:{compaction:{enabled:true,reserveTokens:16300,keepRecentTokens:0}}});
 await client.command('prompt',injectCommand(envelope('compact-seed')));await client.waitFor(r=>r.type==='message_update');
 await client.command('prompt',injectCommand(envelope('compact-a')));provider.release(1);await provider.waitForRequest(2);
 const input=envelope('during-compact');await client.command('prompt',injectCommand(input));
 assert(!client.records.some(controlled));assert(!client.records.some(isConsumed(input.kotaId)));
 provider.release(2);await client.waitFor(isConsumed(input.kotaId));await client.waitFor(settled);
});

test('aborted compaction also releases the injection guard', { timeout: 25_000 }, async t => {
  const { provider, client } = await fixture(t, [
    { type: 'text', text: 'synthetic history. '.repeat(300) },
    { type: 'text', text: 'Must not finish.', holdBeforeHeaders: true },
    { type: 'text', text: 'response after aborted compaction' },
  ]);
  await sendAndSettle(client, envelope('msg-seed-abort', 'kota.user', 'Synthetic task history. '.repeat(300)));
  const compact = client.send('compact');
  await provider.waitForRequest(2);
  const after = client.records.length;
  assert.equal((await client.command('abort')).success, true);
  assert.equal((await compact.response).success, false);
  const ended = await client.waitFor(record => record.type === 'compaction_end', { after });
  assert.equal(ended.aborted, true);
  await sendAndSettle(client, envelope('msg-after-compact-abort'));
});

test('a long response can be aborted without another fake model request', { timeout: 20_000 }, async t => {
  const { provider, client } = await fixture(t, [{ type: 'long', text: 'fixture stream. ', repeat: 100, chunkSize: 16, holdAfterChunks: 1 }]);
  await client.command('prompt', injectCommand(envelope('msg-stop')));
  await client.waitFor(delta);
  assert.equal((await client.command('abort')).success, true);
  await client.waitFor(settled);
  assert.equal(client.records.some(record => record.type === 'message_end' && record.message.role === 'assistant' && record.message.stopReason === 'aborted'), true);
  assert.equal(provider.requests.length, 1);
});

test('Clear during next-turn compaction cannot retract an already-drained custom message', { timeout: 25_000 }, async t => {
  const { provider, client } = await fixture(t, [
    { type: 'tool', name: 'read', arguments: { path: 'fixture.txt' }, chunkSize: 8, holdAfterChunks: 1 },
    { type: 'text', text: 'Synthetic next-turn compaction summary.', holdBeforeHeaders: true },
    { type: 'text', text: 'response to drained instruction' },
  ], { settings: { compaction: { enabled: true, keepRecentTokens: 0, reserveTokens: 16_300 } } });
  const queued = envelope('msg-clear-drained');
  await client.command('prompt', injectCommand(envelope('msg-clear-seed')));
  await client.waitFor(record => record.type === 'message_update' && record.assistantMessageEvent.type === 'toolcall_delta');
  await client.command('prompt', injectCommand(queued));
  provider.release(1);
  await provider.waitForRequest(2);
  assert.equal(client.records.some(isConsumed(queued.kotaId)), false);
  const afterClear = client.records.length;
  assert.equal((await client.command('clear_queue')).success, true);
  assert.equal((await client.command('get_state')).data.isStreaming, true);
  // Joint-ledger expectation: not terminal cleared. The Rust R2 fixture must retain this frozen ID.
  let state = 'clear_requested';
  const confirm = record => { if (isConsumed(queued.kotaId)(record)) state = 'consumed'; };
  client.events.on('record', confirm);
  t.after(() => client.events.removeListener('record', confirm));
  provider.release(2);
  await client.waitFor(settled, { after: afterClear });
  assert.equal(state, 'consumed');
  assert.equal(client.records.filter(isConsumed(queued.kotaId)).length, 1);
  assert.equal(client.records.some(record => record.type === 'tool_execution_end' && record.toolName === 'read' && !record.isError), true);
});
