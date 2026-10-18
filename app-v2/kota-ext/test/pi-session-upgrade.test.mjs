import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { createFakeProvider, MODEL, PROVIDER } from './fake-provider/server.mjs';
import { createFakeJev } from './fake-provider/jev.mjs';
import { createTestSession, TestRpcClient } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand } from './fixtures.mjs';

const fixture = new URL('./fixtures/pi-1.1-session/', import.meta.url);
const mcpServer = fileURLToPath(new URL('./fixtures/mcp-server.mjs', import.meta.url));
const routeFact = record => record.type === 'extension_ui_request' && record.method === 'notify'
  && record.message?.startsWith('kota.route.v1:') ? JSON.parse(record.message.slice('kota.route.v1:'.length)) : undefined;

test('Pi 1.3 resumes an actual 1.1 transcript, MCP resources and the persisted Auto String selection', { timeout: 60_000 }, async t => {
  const source = JSON.parse(await readFile(new URL('SOURCE.json', fixture), 'utf8'));
  const transcript = await readFile(new URL('session.jsonl', fixture), 'utf8');
  assert.equal(createHash('sha256').update(transcript).digest('hex'), source.sha256);
  const oldEntries = transcript.trim().split('\n').map(JSON.parse).slice(1);
  let provider, jev, session, client;
  t.after(async () => {
    try { await client?.close(); await session?.close(); }
    finally { await provider?.close(); await jev?.close(); }
  });
  provider = await createFakeProvider({ steps: [
    { type: 'text', text: 'Resumed selected model B.' },
    { type: 'tool', name: 'codemode', arguments: { code: 'const result = await tools.mcp__legacy__echo({value:"upgraded MCP result"}); text(result.content); text(await tools.list_mcp_resources({server:"legacy"})); text(await tools.read_mcp_resource({server:"legacy",uri:"fixture://one"}));' } },
    { type: 'text', text: 'New model A completed its MCP tool.' },
  ] });
  jev = await createFakeJev({ steps: [{ serious: .95, casual: .05 }] });
  session = await createTestSession({ baseUrl: provider.baseUrl, extensionPath, persistent: true,
    sessionId: source.sessionId, model: 'kota/auto', ownedLayout: true,
    extraArgs: [`--kota-auto-route=${JSON.stringify(source.group)}`],
    extraProviders: { typesafe: { baseUrl: jev.baseUrl } } });
  await session.client.close();
  const sessions = join(session.directory, 'sessions');
  for (const name of await readdir(sessions)) if (name.endsWith('.jsonl')) await rm(join(sessions, name));
  const sessionFile = join(sessions, source.fileName);
  // Pi matches session-id inside the same project cwd. Rebind only that portable header;
  // every persisted message, entry ID, parent link and routing state remains byte-for-byte old.
  const newline = transcript.indexOf('\n');
  const header = { ...JSON.parse(transcript.slice(0, newline)), cwd: session.cwd };
  const seeded = JSON.stringify(header) + transcript.slice(newline);
  await writeFile(sessionFile, seeded);
  await mkdir(join(session.directory, 'jev'), { recursive: true });
  await writeFile(join(session.directory, 'jev/keys.json'), JSON.stringify({ schema: 1, provider: 'typesafe',
    keys: { typesafe: { key: 'synthetic-upgrade-key', keyId: 'upgrade-key' } } }));
  await writeFile(join(session.configDirectory, 'mcp.json'), JSON.stringify({ mcpServers: {
    legacy: { command: process.execPath, args: [mcpServer] },
  } }));
  client = new TestRpcClient('pi', session.args, { cwd: session.cwd, env: session.env });
  assert.equal((await client.command('get_state')).success, true);
  const restored = await client.command('get_entries');
  assert.equal(restored.success, true);
  const byId = new Map(restored.data.entries.map(entry => [entry.id, entry]));
  for (const entry of oldEntries) assert.deepEqual(byId.get(entry.id), entry, `Preserve old entry ${entry.id}`);
  assert.deepEqual(oldEntries.filter(e => e.customType === 'kota.user').map(e => e.details.kotaId), source.expectedKotaIds);

  let health;
  for (let attempt = 0; attempt < 100; attempt++) {
    const requestId = `upgrade-${attempt}`;
    const after = client.records.length;
    assert.equal((await client.command('prompt', { message: `/kota-mcp ${JSON.stringify({ schema: 1, requestId })}` })).success, true);
    const record = await client.waitFor(r => r.type === 'extension_ui_request' && r.method === 'notify'
      && r.message?.startsWith('kota.mcp.v1:') && JSON.parse(r.message.slice('kota.mcp.v1:'.length)).requestId === requestId, { after });
    health = JSON.parse(record.message.slice('kota.mcp.v1:'.length));
    if (health.servers?.some(server => server.name === 'legacy' && server.status === 'connected')) break;
    await new Promise(resolve => setTimeout(resolve, 25));
  }
  assert.equal(health.available, true);
  assert.deepEqual(health.servers, [{ name: 'legacy', status: 'connected' }]);

  async function send(id, text) {
    const after = client.records.length;
    assert.equal((await client.command('prompt', injectCommand(envelope(id, 'kota.user', text)))).success, true);
    await client.waitFor(r => r.type === 'agent_settled', { after, timeoutMs: 30_000 });
    return client.records.slice(after);
  }
  const retried = await send('legacy-b', 'Synthetic legacy-b input.');
  assert.equal(jev.requests.length, 0, 'Same delivery after upgrade reuses the 1.1 selection');
  assert.equal(retried.some(r => routeFact(r)?.event === 'selection'), false);
  assert.equal(provider.requests[0].body.model, 'fixture-alternate');
  assert.equal(retried.findLast(r => r.type === 'message_end' && r.message.role === 'assistant').message.stopReason, 'stop');
  const next = await send('upgraded-c', 'Synthetic new input after upgrade.');
  assert.equal(jev.requests.length, 1);
  assert.equal(jev.requests[0].body.state.current_model, 'model-2');
  assert.equal(jev.requests[0].body.state.previous_reply, 'Resumed selected model B.');
  assert.deepEqual(provider.requests.map(r => r.body.model), ['fixture-alternate', MODEL, MODEL]);
  assert(next.some(r => r.type === 'tool_execution_end' && r.toolName === 'codemode' && r.isError === false));
  assert.match(JSON.stringify(provider.requests[0].body.messages), /legacy MCP result/);
  assert.match(JSON.stringify(provider.requests[0].body.messages), /synthetic resource/);
  assert.match(JSON.stringify(provider.requests.at(-1).body.messages), /upgraded MCP result/);
  assert(next.some(r => r.type === 'tool_execution_end' && JSON.stringify(r.result).includes('synthetic resource')));
  await client.close();
  assert((await readFile(sessionFile, 'utf8')).startsWith(seeded), 'Upgrade appends in place without rewriting the old transcript');
  assert.deepEqual((await readdir(sessions)).filter(name => name.endsWith('.jsonl')), [source.fileName]);
});

test('switching physical models keeps the answer but does not replay another model\'s thinking', { timeout: 30_000 }, async t => {
  const thinking = 'SYNTHETIC_PRIVATE_THINKING_FROM_MODEL_A';
  const provider = await createFakeProvider({ steps: [
    { type: 'text', text: 'Public answer from A.', thinking },
    { type: 'text', text: 'Public answer from B.' },
  ] });
  let session;
  t.after(async () => { try { await session?.close(); } finally { await provider.close(); } });
  session = await createTestSession({ baseUrl: provider.baseUrl, extensionPath, reasoning: true });
  const client = session.client;
  async function send(text) {
    const after = client.records.length;
    assert.equal((await client.command('prompt', { message: text })).success, true);
    await client.waitFor(r => r.type === 'agent_settled', { after });
    return client.records.slice(after);
  }
  const first = await send('Synthetic model A task.');
  const answer = first.findLast(r => r.type === 'message_end' && r.message.role === 'assistant').message;
  assert(answer.content.some(block => block.type === 'thinking' && block.thinking === thinking));
  assert.equal((await client.command('set_model', { provider: PROVIDER, modelId: 'fixture-alternate' })).success, true);
  await send('Synthetic model B task.');
  const secondRequest = JSON.stringify(provider.requests[1].body.messages);
  assert.match(secondRequest, /Public answer from A/);
  assert(!secondRequest.includes(thinking));
});

test('cancelling a waiting codemode worker settles and allows the next Local message', { timeout: 20_000 }, async t => {
  const provider = await createFakeProvider({ steps: [
    { type: 'tool', name: 'codemode', arguments: { code: 'text("worker-ready"); await new Promise(resolve => setTimeout(resolve, 30000)); text("must-not-finish");' } },
    { type: 'text', text: 'After cancellation.' },
  ] });
  let session;
  t.after(async () => { try { await session?.close(); } finally { await provider.close(); } });
  session = await createTestSession({ baseUrl: provider.baseUrl, extensionPath });
  const client = session.client;
  assert.equal((await client.command('prompt', { message: 'Synthetic waiting script.' })).success, true);
  await client.waitFor(r => r.type === 'tool_execution_update' && r.toolName === 'codemode'
    && JSON.stringify(r).includes('worker-ready'));
  const afterAbort = client.records.length;
  const started = performance.now();
  assert.equal((await client.command('abort')).success, true);
  await client.waitFor(r => r.type === 'agent_settled', { after: afterAbort, timeoutMs: 5_000 });
  assert(performance.now() - started < 5_000, 'Cancel must not wait for the 30 second script');
  assert.equal(provider.requests.length, 1);
  const afterNext = client.records.length;
  assert.equal((await client.command('prompt', { message: 'Synthetic message after stop.' })).success, true);
  await client.waitFor(r => r.type === 'agent_settled', { after: afterNext });
  const answer = client.records.slice(afterNext).findLast(r => r.type === 'message_end' && r.message.role === 'assistant').message;
  assert.equal(answer.stopReason, 'stop');
  assert.equal(provider.requests.length, 2);
});
