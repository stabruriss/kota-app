import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, readFile, rm } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { resolveTestPi, TestRpcClient } from './fake-provider/rpc-client.mjs';
import { createFakeProvider } from './fake-provider/server.mjs';
import { AUTH_CODEC_PREFIX } from '../auth-protocol.ts';
import { extensionPath } from './fixtures.mjs';
const cost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
const model = id => ({ id, name: 'Synthetic owned overlay', reasoning: true, input: ['text'], contextWindow: 16000, maxTokens: 1000, cost });
function authEvent(record) {
  if (record.type !== 'extension_ui_request') return;
  const text = record.method === 'notify' ? record.message : record.title;
  if (typeof text !== 'string' || !text.startsWith(AUTH_CODEC_PREFIX)) return;
  return JSON.parse(text.slice(AUTH_CODEC_PREFIX.length));
}
async function fixture(t, helper = true) {
  const dir = realpathSync(await mkdtemp('/tmp/kota-auth-rpc-'));
  const home = join(dir, 'home'), agent = join(dir, 'agent'), cwd = join(dir, 'cwd');
  for (const path of [home, agent, cwd]) await mkdir(path, { mode: 0o700 });
  const provider = await createFakeProvider([]);
  const modelsPath = join(agent, 'models.json');
  const models = { providers: {
    'kota-fake': { baseUrl: provider.baseUrl, api: 'openai-completions', apiKey: 'fake-model-config', models: [model('fixture')] },
    'azure': { baseUrl: 'http://127.0.0.1:1/owned-overlay', apiKey: 'fake-overlay-auth', models: [model('owned-overlay-model')] },
  } };
  await writeFile(modelsPath, JSON.stringify(models), { mode: 0o600 });
  const authPath = join(agent, 'auth.json');
  await writeFile(authPath, JSON.stringify({ deepseek: { type: 'api_key', key: 'fake-other-key' } }), { mode: 0o600 });
  await writeFile(join(agent, 'settings.json'), JSON.stringify({ steeringMode: 'all', followUpMode: 'all',
    cacheWarming: 'off', enableInstallTelemetry: false, enableAnalytics: false,
    compaction: { enabled: false }, retry: { enabled: false } }), { mode: 0o600 });
  const sentinel = join(home, '.pi/agent/auth.json');
  await mkdir(join(home, '.pi/agent'), { recursive: true, mode: 0o700 });
  await writeFile(sentinel, 'synthetic-personal-sentinel\n');
  const env = { PATH: '/usr/bin:/bin:/opt/homebrew/bin', HOME: home, USERPROFILE: home, TMPDIR: dir,
    PI_CODING_AGENT_DIR: agent, PI_OFFLINE: '1', PI_SKIP_VERSION_CHECK: '1', NO_COLOR: '1',
    // Preserved SDK override: only the owned Resource URL must enter the credential.
    AZURE_OPENAI_BASE_URL: 'http://127.0.0.1:1/ambient-not-owned' };
  if (helper) Object.assign(env, { KOTA_LOCAL_AUTH_HELPER: '1', KOTA_LOCAL_INSTALLATION_UUID: '11111111-2222-4333-8444-555555555555' });
  const product = fileURLToPath(new URL('../../../', import.meta.url));
  const policy = `(version 1)(allow default)(deny network*)
(deny file-read* file-write* (subpath ${JSON.stringify(join(homedir(), '.pi'))}))
(deny file-read* file-write* (subpath ${JSON.stringify(join(home, '.pi'))}))
(deny file-write* (subpath ${JSON.stringify(product)}))`;
  const client = new TestRpcClient('/usr/bin/sandbox-exec', ['-p', policy, resolveTestPi(),
    '--mode', 'rpc', '--no-approve', '-ne', '-ns', '-np', '--no-themes', '--no-context-files', '--offline',
    '-e', extensionPath, '--provider', 'kota-fake', '--model', 'fixture', '--no-session'], { cwd, env });
  t.after(async () => { await client.close(); assert.equal(provider.requests.length, 0);
    assert.equal(await readFile(sentinel, 'utf8'), 'synthetic-personal-sentinel\n'); await provider.close(); await rm(dir, { recursive: true, force: true }); });
  const commands = await client.command('get_commands');
  assert.equal(commands.success, true); assert(commands.data.commands.some(command => command.name === 'kota-auth'));
  let serial = 0;
  function start(operation, fields = {}) {
    const operationId = `actual-${++serial}`;
    const sent = client.send('prompt', { message: `/kota-auth ${JSON.stringify({ schema: 1, operationId, operation, ...fields })}` });
    return { operationId, ...sent };
  }
  const result = id => client.waitFor(record => authEvent(record)?.operationId === id && authEvent(record)?.event === 'result').then(authEvent);
  return { client, start, result, authPath, modelsPath };
}

test('actual product entry: Azure login bypasses owned overlay; metadata retains it; other providers stay unwrapped', { timeout: 60_000 }, async t => {
  const { client, start, result, authPath } = await fixture(t);
  for (const suffix of ['A', 'B']) {
    const flow = start('login', { providerId: 'azure', authType: 'api_key', uiSchema: 1,
      resourceUrl: `http://127.0.0.1:1/saved-${suffix}` });
    const dialog = await client.waitFor(record => authEvent(record)?.operationId === flow.operationId && authEvent(record)?.event === 'prompt');
    assert.equal(dialog.method, 'input');
    assert.equal(authEvent(dialog).credentialField, 'api_key.key'); assert.equal(authEvent(dialog).prompt.type, 'secret');
    client.child.stdin.write(`${JSON.stringify({ type: 'extension_ui_response', id: dialog.id, value: `fake-azure-key-${suffix}` })}\n`);
    const outcome = await result(flow.operationId); assert.equal(outcome.outcome, 'succeeded'); assert.equal(outcome.commit, 'confirmed');
    assert.equal((await flow.response).success, true);
    const saved = JSON.parse(await readFile(authPath, 'utf8'));
    assert.equal(saved['azure'].key, `fake-azure-key-${suffix}`);
    assert.equal(saved['azure'].env.AZURE_OPENAI_BASE_URL, `http://127.0.0.1:1/saved-${suffix}`);
    assert.equal(saved.deepseek.key, 'fake-other-key');
  }
  const catalog = start('catalog', { providerId: 'azure' });
  const catalogResult = await result(catalog.operationId); await catalog.response;
  assert(catalogResult.catalog.providers[0].models.some(model => model.modelId === 'owned-overlay-model'));
  const resolve = start('resolve', { providerId: 'azure', modelId: 'owned-overlay-model', thinkingLevel: 'max' });
  const resolution = await result(resolve.operationId); await resolve.response;
  assert.equal(resolution.resolved.modelId, 'owned-overlay-model');
  const deepseek = start('login', { providerId: 'deepseek', authType: 'api_key', uiSchema: 1 });
  const dialog = await client.waitFor(record => authEvent(record)?.operationId === deepseek.operationId && authEvent(record)?.event === 'prompt');
  client.child.stdin.write(`${JSON.stringify({ type: 'extension_ui_response', id: dialog.id, value: 'fake-deepseek-replacement' })}\n`);
  assert.equal((await result(deepseek.operationId)).outcome, 'succeeded'); await deepseek.response;
  const saved = JSON.parse(await readFile(authPath, 'utf8'));
  assert.deepEqual(saved.deepseek, { type: 'api_key', key: 'fake-deepseek-replacement' });
  assert.equal(saved['azure'].env.AZURE_OPENAI_BASE_URL, 'http://127.0.0.1:1/saved-B');
  assert(!client.lines.join('\n').includes('fake-azure-key-A')); assert(!client.lines.join('\n').includes('fake-deepseek-replacement'));
  const session = await client.command('get_messages');
  assert.equal(session.success, true); assert.deepEqual(session.data.messages, []);
});

test('actual helper cancellation is correlated and independent of model abort', { timeout: 30_000 }, async t => {
  const { client, start, result, authPath } = await fixture(t);
  const before = await readFile(authPath);
  const flow = start('login', { providerId: 'deepseek', authType: 'api_key', uiSchema: 1 });
  const dialog = await client.waitFor(record => authEvent(record)?.operationId === flow.operationId && authEvent(record)?.event === 'prompt');
  assert.equal(authEvent(dialog).credentialField, 'api_key.key');
  assert.equal((await client.command('abort')).success, true);
  assert(!client.records.some(record => authEvent(record)?.operationId === flow.operationId && authEvent(record)?.event === 'result'));
  const competing = start('login', { providerId: 'openai', authType: 'api_key', uiSchema: 1 });
  assert.equal((await result(competing.operationId)).code, 'busy-auth'); await competing.response;
  const cancelled = start('cancel', { targetOperationId: flow.operationId });
  assert.equal((await result(cancelled.operationId)).outcome, 'succeeded'); await cancelled.response;
  const outcome = await result(flow.operationId); await flow.response;
  assert.equal(outcome.outcome, 'cancelled'); assert.equal(outcome.commit, 'not-written');
  assert((await readFile(authPath)).equals(before));
  assert(client.records.some(record => authEvent(record)?.operationId === flow.operationId &&
    authEvent(record)?.event === 'prompt_closed' && authEvent(record)?.reason === 'cancelled'));
});

test('actual owner only permits asynchronous idle refresh, not helper authentication', { timeout: 30_000 }, async t => {
  const { client, start, result } = await fixture(t, false);
  const refresh = start('refresh');
  assert.equal((await result(refresh.operationId)).outcome, 'succeeded'); await refresh.response;
  for (const [operation, fields] of [
    ['status', {}], ['catalog', {}], ['logout', { providerId: 'deepseek' }],
    ['login', { providerId: 'deepseek', authType: 'api_key', uiSchema: 1 }],
    ['cancel', { targetOperationId: 'not-an-owner-flow' }],
    ['resolve', { providerId: 'azure', modelId: 'owned-overlay-model', thinkingLevel: 'high' }],
  ]) {
    const after = client.records.length;
    const forbidden = start(operation, fields);
    await client.waitFor(record => record.type === 'extension_error' && record.error?.includes('kota-auth-helper-required'), { after });
    await forbidden.response;
    assert(!client.records.some(record => authEvent(record)?.operationId === forbidden.operationId));
  }
});

test('actual fresh helper fails explicitly on damaged owned model overlay instead of an empty catalog', { timeout: 30_000 }, async t => {
  const { start, result, modelsPath } = await fixture(t);
  await writeFile(modelsPath, '{');
  const flow = start('catalog');
  const outcome = await result(flow.operationId); await flow.response;
  assert.equal(outcome.outcome, 'failed'); assert.equal(outcome.commit, 'not-written');
  assert.equal(outcome.code, 'storage-failed'); assert.equal(outcome.catalog, undefined);
});
