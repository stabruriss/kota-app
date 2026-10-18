import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createFakeProvider } from './server.mjs';
import { createTestSession } from './rpc-client.mjs';
import { envelope, extensionPath, injectCommand, isConsumed, tinyPng } from '../fixtures.mjs';

const faultExtension = fileURLToPath(new URL('../fixtures/fault-extension.ts', import.meta.url));
const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--out') throw new Error('Usage: record-fixtures.mjs --out <artifact-directory>');
const destination = resolve(args[1]);
const text = value => ({ type: 'text', text: value });
const provider = await createFakeProvider({ steps: [
  text('Synthetic plain reply.'), text('Synthetic image reply.'), text('Synthetic private reply.'),
  text('Synthetic bus reply.'), text('Synthetic Telegram reply.'), text('Synthetic reminder reply.'),
  { type: 'tool', name: 'read', arguments: { path: 'fixture.txt' } },
  { ...text('Synthetic read completed.'), thinking: 'Synthetic reasoning fixture; no private data.' },
  text('Synthetic compaction summary.'),
  text('Synthetic turn-prefix summary.'),
  { type: 'error', status: 400, message: 'Synthetic permanent fixture error.' },
  { type: 'error', status: 500, message: '500 synthetic transient fixture failure.' },
  { type: 'error', status: 500, message: '500 synthetic transient fixture failure.' },
  text('Synthetic incremental reply.'),
] });
let session;
try {
  session = await createTestSession({
    baseUrl: provider.baseUrl, extensionPath, extraExtensions: [faultExtension], reasoning: true,
    settings: {
      compaction: { enabled: false, keepRecentTokens: 0, reserveTokens: 1_024 },
      retry: { enabled: false, maxRetries: 1, baseDelayMs: 1, maxAgentDelayMs: 1 },
    },
  });
  const { client } = session;
  async function run(input) {
    const after = client.records.length;
    const response = await client.command('prompt', injectCommand(input));
    assert.equal(response.success, true);
    await client.waitFor(isConsumed(input.kotaId), { after });
    await client.waitFor(record => record.type === 'agent_settled', { after });
  }
  await run(envelope('msg-user', 'kota.user', 'Synthetic plain user text.'));
  await run(envelope('msg-user-image', 'kota.user', [
    { type: 'text', text: 'Synthetic image input.' }, { type: 'image', data: tinyPng, mimeType: 'image/png' },
  ]));
  const privateInput = envelope('msg-user-private', 'kota.user', 'Synthetic private text.');
  privateInput.details.private = true;
  await run(privateInput);
  // These use the unchanged KOTA_MESSAGE renderer's shape with synthetic IDs and body text.
  for (const [messageId, customType, actor, intent, timing] of [
    ['msg-bus', 'kota.bus', 'agent-fixture-sender', 'handoff', ''],
    ['msg-lm', 'kota.lm', 'laughing-man', 'telegram', ''],
    ['msg-ember', 'kota.ember', 'ember', 'reminder', ' trigger="in" scheduled_for="2026-10-02T00:00:00+00:00" current_time="2026-10-02T00:00:01+00:00"'],
  ]) {
    const body = `<KOTA_MESSAGE id="${messageId}" from="${actor}" to="agent-fixture" intent="${intent}"${timing}>\nSynthetic ${intent} body.\n</KOTA_MESSAGE>`;
    const input = envelope(messageId, customType, body);
    input.details.sourceActorId = actor;
    input.details.sourceIntent = intent;
    await run(input);
  }
  await run(envelope('msg-read', 'kota.user', 'Synthetic read task.'));
  assert.equal((await client.command('set_model', { provider: 'kota-fake', modelId: 'fixture-alternate' })).success, true);
  assert.equal((await client.command('set_thinking_level', { level: 'high' })).success, true);
  const compact = await client.command('compact');
  assert.equal(compact.success, true, compact.error);
  await run(envelope('msg-error', 'kota.user', 'Synthetic permanent provider error task.'));
  assert.equal((await client.command('set_auto_retry', { enabled: true })).success, true);
  await run(envelope('msg-retry', 'kota.user', 'Synthetic retry exhaustion task.'));
  assert.equal((await client.command('prompt', { message: '/fixture-fail' })).success, true);
  const full = await client.command('get_entries');
  assert.equal(full.success, true);
  const cursor = full.data.entries.at(-1).id;
  await run(envelope('msg-since', 'kota.user', 'Synthetic incremental task.'));
  const since = await client.command('get_entries', { since: cursor });
  assert.equal(since.success, true);

  const paths = [
    [session.cwd, '/fixture/workspace'], [extensionPath, '/fixture/workspace/kota-ext/index.ts'],
    [faultExtension, '/fixture/workspace/fault-extension.ts'], [session.directory, '/fixture'],
    [provider.baseUrl, 'http://127.0.0.1:3000/v1'],
  ].sort((left, right) => right[0].length - left[0].length);
  const sanitize = value => {
    let result = value;
    for (const [original, replacement] of paths) result = result.replaceAll(original, replacement);
    assert.equal(result.includes('/Users/'), false, 'Fixture still contains a user path');
    assert.equal(result.includes('stabruriss'), false, 'Fixture still contains a private project ID');
    return result;
  };
  const eventLines = client.lines.filter((_, index) => !['message_update', 'response'].includes(client.records[index].type));
  const files = {
    'full.json': sanitize(`${JSON.stringify(full.data, null, 2)}\n`),
    'since.json': sanitize(`${JSON.stringify(since.data, null, 2)}\n`),
    'events.jsonl': sanitize(`${eventLines.join('\n')}\n`),
  };
  const messages = full.data.entries.filter(entry => entry.type === 'message').map(entry => entry.message);
  const coverage = {
    thinking: messages.some(message => message.role === 'assistant' && message.content.some(block => block.type === 'thinking')),
    modelChange: full.data.entries.some(entry => entry.type === 'model_change' && entry.modelId === 'fixture-alternate'),
    thinkingChange: full.data.entries.some(entry => entry.type === 'thinking_level_change' && entry.thinkingLevel === 'high'),
    compaction: full.data.entries.some(entry => entry.type === 'compaction'),
    providerError: messages.some(message => message.role === 'assistant' && message.stopReason === 'error' && message.errorMessage),
    exhaustedRetry: client.records.some(record => record.type === 'auto_retry_end' && record.success === false),
    extensionError: client.records.some(record => record.type === 'extension_error' && record.extensionPath === 'command:fixture-fail'),
  };
  for (const [key, value] of Object.entries(coverage)) assert.equal(value, true, `Missing ${key} fixture`);
  await mkdir(destination, { recursive: true });
  const hashes = {};
  for (const [name, data] of Object.entries(files)) {
    await writeFile(resolve(destination, name), data);
    hashes[name] = createHash('sha256').update(data).digest('hex');
  }
  const receipt = {
    source: 'Real Pi RPC stdout/get_entries against the loopback fake provider; no real model or credentials.',
    transform: 'Only known temporary filesystem paths and the ephemeral loopback port were replaced. IDs, timestamps, contents, messages and entries are otherwise unchanged. message_update deltas and command responses were omitted from events.jsonl.',
    coverage, cursor, fullEntries: full.data.entries.length, sinceEntries: since.data.entries.length, hashes,
  };
  await writeFile(resolve(destination, 'RECEIPT.json'), `${JSON.stringify(receipt, null, 2)}\n`);
  process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
} finally {
  if (session) await session.close();
  await provider.close();
}
