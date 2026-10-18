import assert from 'node:assert/strict';
import { once } from 'node:events';
import test from 'node:test';
import { createFakeProvider } from './server.mjs';
import { createTestSession } from './rpc-client.mjs';

const prompt = { model: 'fixture', messages: [{ role: 'user', content: 'synthetic test input' }] };
const request = (provider, body, options = {}) => fetch(`${provider.baseUrl}/chat/completions`, {
  method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body), ...options,
});

function sseRecords(text) {
  return text.split('\n\n').filter(Boolean).map(record => record.slice('data: '.length)).filter(value => value !== '[DONE]').map(JSON.parse);
}

test('scripted non-stream reply and explicit script exhaustion', async t => {
  const provider = await createFakeProvider({ steps: [{ type: 'text', text: 'deterministic reply' }] });
  t.after(() => provider.close());
  const response = await request(provider, prompt);
  assert.equal(response.status, 200);
  assert.equal((await response.json()).choices[0].message.content, 'deterministic reply');
  assert.equal((await request(provider, prompt)).status, 409);
  assert.equal(provider.requests.length, 1);
});

test('SSE reconstructs Unicode text and deterministic usage', async t => {
  const text = 'fixture 中文\nline separator:\u2028 paragraph separator:\u2029';
  const provider = await createFakeProvider({ steps: [{ type: 'text', text, chunkSize: 3 }] });
  t.after(() => provider.close());
  const records = sseRecords(await (await request(provider, { ...prompt, stream: true })).text());
  assert.equal(records.map(record => record.choices[0].delta.content ?? '').join(''), text);
  assert.equal(records.at(-1).choices[0].finish_reason, 'stop');
  assert.equal(records.at(-1).usage.prompt_tokens, 128);
});

test('SSE tool arguments survive chunk boundaries', async t => {
  const args = { path: 'fixture.txt' };
  const provider = await createFakeProvider({ steps: [{ type: 'tool', name: 'read', arguments: args, chunkSize: 2 }] });
  t.after(() => provider.close());
  const records = sseRecords(await (await request(provider, { ...prompt, stream: true })).text());
  const calls = records.flatMap(record => record.choices[0].delta.tool_calls ?? []);
  assert.equal(calls[0].function.name, 'read');
  assert.deepEqual(JSON.parse(calls.map(call => call.function.arguments).join('')), args);
  assert.equal(records.at(-1).choices[0].finish_reason, 'tool_calls');
});

test('held response is released through a deterministic test barrier', async t => {
  const provider = await createFakeProvider({ steps: [{ type: 'long', text: 'fixture.', repeat: 8, holdAfterChunks: 1, chunkSize: 8 }] });
  t.after(() => provider.close());
  const response = await request(provider, { ...prompt, stream: true });
  const record = await provider.waitForRequest(1);
  assert.equal(record.completed, false);
  const state = await (await fetch(`${provider.baseUrl.replace('/v1', '')}/__test/state`)).json();
  assert.equal(state.requests[0].type, 'long');
  const release = await fetch(`${provider.baseUrl.replace('/v1', '')}/__test/release/1`, { method: 'POST' });
  assert.deepEqual(await release.json(), { released: true });
  assert.equal(sseRecords(await response.text()).at(-1).choices[0].finish_reason, 'stop');
});

test('aborting a held stream releases its server-side work', async t => {
  const provider = await createFakeProvider({ steps: [{ type: 'long', text: 'fixture.', repeat: 8, holdAfterChunks: 1, chunkSize: 8 }] });
  t.after(() => provider.close());
  const aborted = once(provider.events, 'aborted', { signal: AbortSignal.timeout(5_000) });
  const response = await request(provider, { ...prompt, stream: true });
  await response.body.cancel();
  await aborted;
  assert.equal(provider.requests[0].aborted, true);
});

test('rejects malformed requests and unsupported tool fixture names', async t => {
  const provider = await createFakeProvider();
  t.after(() => provider.close());
  const response = await fetch(`${provider.baseUrl}/chat/completions`, { method: 'POST', body: '{' });
  assert.equal(response.status, 400);
  assert.equal(provider.requests.length, 0);
  await assert.rejects(createFakeProvider({ steps: [{ type: 'tool', name: 'write' }] }), /Fake tool response names must be native or Kota fixture tools/);
});

test('scripted errors do not forward requests, and reasoning uses the compatibility wire field', async t => {
  const provider = await createFakeProvider({ steps: [
    { type: 'error', status: 500, message: 'Synthetic test error.' },
    { type: 'text', text: 'fixture answer', thinking: 'fixture thinking' },
  ] });
  t.after(() => provider.close());
  const failure = await request(provider, prompt);
  assert.equal(failure.status, 500);
  assert.equal((await failure.json()).error.message, 'Synthetic test error.');
  const records = sseRecords(await (await request(provider, { ...prompt, stream: true })).text());
  assert.equal(records.find(record => record.choices[0].delta.reasoning_content).choices[0].delta.reasoning_content, 'fixture thinking');
});

test('real Pi talks only to the fake endpoint in an isolated temporary home', { timeout: 20_000 }, async t => {
  const provider = await createFakeProvider({ steps: [{ type: 'text', text: 'Pi fixture \u2028 still one RPC record \u2029.' }] });
  t.after(() => provider.close());
  const session = await createTestSession({ baseUrl: provider.baseUrl });
  t.after(() => session.close());
  assert.equal(session.env.HOME.startsWith(session.directory), true);
  assert.equal(session.env.PI_CODING_AGENT_DIR.startsWith(session.directory), true);
  assert.equal(Object.keys(session.env).some(key => key.endsWith('_API_KEY') || key.startsWith('KOTA_')), false);
  const after = session.client.records.length;
  const response = await session.client.command('prompt', { message: 'Synthetic transport smoke test.' });
  assert.equal(response.success, true);
  await session.client.waitFor(record => record.type === 'agent_settled', { after });
  const final = session.client.records.find(record => record.type === 'message_end' && record.message.role === 'assistant');
  assert.equal(final.message.content[0].text, 'Pi fixture \u2028 still one RPC record \u2029.');
  assert.equal(provider.requests.length, 1);
});
