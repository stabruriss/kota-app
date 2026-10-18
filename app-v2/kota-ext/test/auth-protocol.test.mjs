import assert from 'node:assert/strict';
import test from 'node:test';
import { AUTH_CODEC_PREFIX, AUTH_COMMAND_SCHEMA, AUTH_LOGIN_TIMEOUT_MS, AUTH_CONTROL_TIMEOUT_MS,
  AUTH_CANCEL_GRACE_MS, THINKING_LEVELS, parseAuthCommand, encodeAuthEvent } from '../auth-protocol.ts';
const command = fields => ({ schema: 1, operationId: 'op-1', ...fields });
const parse = value => parseAuthCommand(JSON.stringify(value));
const base = { schema: 1, channel: 'kota.auth', operationId: 'op-1' };
const decode = value => JSON.parse(encodeAuthEvent(value).slice(AUTH_CODEC_PREFIX.length));

test('auth uses one strict root object and the approved independent deadlines', () => {
  assert.equal(AUTH_COMMAND_SCHEMA.type, 'object');
  assert.equal(AUTH_COMMAND_SCHEMA.additionalProperties, false);
  assert.equal('anyOf' in AUTH_COMMAND_SCHEMA, false);
  assert.equal(AUTH_LOGIN_TIMEOUT_MS, 600_000);
  assert.equal(AUTH_CONTROL_TIMEOUT_MS, 30_000);
  assert.equal(AUTH_CANCEL_GRACE_MS, 5_000);
});

test('all seven operations accept only their own fields', () => {
  const cases = [
    { operation: 'login', providerId: 'openai-codex', authType: 'oauth', uiSchema: 1 },
    { operation: 'status' }, { operation: 'status', providerId: 'anthropic' },
    { operation: 'logout', providerId: 'anthropic' },
    { operation: 'cancel', targetOperationId: 'op-0' }, { operation: 'refresh' },
    { operation: 'catalog' }, { operation: 'catalog', providerId: 'openai' },
    { operation: 'resolve', providerId: 'openai', modelId: 'exact/model:id', thinkingLevel: 'max' },
  ];
  for (const fields of cases) {
    const value = command(fields); assert.deepEqual(parse(value), value);
    assert.equal(parse({ ...value, key: 'fake-value' }), undefined);
  }
  for (const field of ['headers', 'env', 'authPath', 'cwd', 'binary', 'credential', 'access', 'refreshToken']) {
    assert.equal(parse(command({ operation: 'status', [field]: 'fake-value' })), undefined);
  }
});

test('malformed, unknown, partial and oversized commands cannot recover operation IDs', () => {
  for (const text of ['{', '{"operationId":"op-1",', 'null', '[]', 'true', '"value"'])
    assert.equal(parseAuthCommand(text), undefined);
  for (const value of [command({ operation: 'other' }), { schema: 1, operation: 'status' },
    command({ operation: 'status', operationId: '' }), command({ operation: 'status', schema: 2 }),
    command({ operation: 'status', providerId: null }), command({ operation: 'status', operationId: 'a\n' }),
    command({ operation: 'status', operationId: 'a'.repeat(129) })]) assert.equal(parse(value), undefined);
  assert.equal(parseAuthCommand(' '.repeat(16_385)), undefined);
});

test('Azure resourceUrl is required only for Azure api_key and is not an env override', () => {
  const azure = command({ operation: 'login', providerId: 'azure', authType: 'api_key', uiSchema: 1 });
  assert.equal(parse(azure), undefined);
  for (const url of ['https://example.openai.azure.com', 'http://127.0.0.1:1234/openai/v1'])
    assert.deepEqual(parse({ ...azure, resourceUrl: url }), { ...azure, resourceUrl: url });
  for (const url of ['', ' ', '!command', '$FAKE_VAR', 'file:///tmp/fixture', 'javascript:0',
    'https://fake-user:fake-password@example.test', 'https://example.test/' + 'x'.repeat(4096)])
    assert.equal(parse({ ...azure, resourceUrl: url }), undefined);
  for (const [providerId, authType] of [['openai', 'api_key'], ['azure', 'oauth']])
    assert.equal(parse({ ...azure, providerId, authType, resourceUrl: 'https://example.test' }), undefined);
});

test('cross-operation fields and incomplete mutations fail closed', () => {
  for (const value of [
    command({ operation: 'login', providerId: 'openai', authType: 'api_key' }),
    command({ operation: 'login', providerId: 'openai', authType: 'api_key', uiSchema: 2 }),
    command({ operation: 'logout' }), command({ operation: 'logout', providerId: 'openai', authType: 'oauth' }),
    command({ operation: 'cancel' }), command({ operation: 'cancel', targetOperationId: '' }),
    command({ operation: 'refresh', providerId: 'openai' }),
    command({ operation: 'catalog', modelId: 'anything' }),
    command({ operation: 'resolve', providerId: 'openai', modelId: 'gpt', thinkingLevel: 'auto' }),
  ]) assert.equal(parse(value), undefined);
  for (const thinkingLevel of THINKING_LEVELS)
    assert(parse(command({ operation: 'resolve', providerId: 'openai', modelId: 'gpt', thinkingLevel })));
});

test('codec explicitly whitelists error/result/status/catalog/resolve properties', () => {
  const extra = { credential: { key: 'fake-do-not-leak' }, cause: 'fake-do-not-leak', stack: 'fake-do-not-leak' };
  const status = { providerId: 'openai', providerLabel: 'OpenAI', canApiKey: true, canOAuth: true,
    storedType: 'api_key', configured: true, policy: 'unknown', validity: 'not-checked', ...extra };
  const wire = encodeAuthEvent({ ...base, ...extra, event: 'result', operation: 'catalog',
    outcome: 'succeeded', commit: 'not-written', status, statuses: [status],
    catalog: { ...extra, providers: [{ ...status, models: [{ modelId: 'gpt', name: 'GPT', thinkingLevels: ['off'], ...extra }] }] },
    resolved: { providerId: 'openai', modelId: 'gpt', requestedThinkingLevel: 'high', thinkingLevel: 'off', ...extra },
  });
  assert(wire.startsWith(AUTH_CODEC_PREFIX)); assert(!wire.includes('fake-do-not-leak'));
  const value = JSON.parse(wire.slice(AUTH_CODEC_PREFIX.length));
  assert.equal(value.status.canOAuth, true);
  assert.deepEqual(value.catalog.providers[0].models[0], { modelId: 'gpt', name: 'GPT', thinkingLevels: ['off'] });
  assert.equal(value.resolved.thinkingLevel, 'off');
});

test('prompt codec preserves credential role and duplicate option IDs without signals/replies', () => {
  const value = decode({ ...base, event: 'prompt', providerId: 'cloudflare-workers-ai', authType: 'api_key',
    promptId: 'step-1',
    prompt: { type: 'select', message: 'Method', signal: 'fake-do-not-leak', value: 'fake-do-not-leak',
      options: [{ id: 'first', label: 'Same', key: 'fake-do-not-leak' }, { id: 'second', label: 'Same' }] },
  });
  assert.equal('credentialField' in value, false);
  const key = decode({ ...base, event: 'prompt', providerId: 'openai', authType: 'api_key',
    promptId: 'key-1', credentialField: 'api_key.key', prompt: { type: 'secret', message: 'Key' } });
  assert.equal(key.credentialField, 'api_key.key');
  assert.deepEqual(value.prompt.options, [{ id: 'first', label: 'Same' }, { id: 'second', label: 'Same' }]);
  assert(!JSON.stringify(value).includes('fake-do-not-leak'));
  const secret = decode({ ...base, event: 'prompt', providerId: 'openai', authType: 'oauth', promptId: 'oauth-1',
    prompt: { type: 'secret', message: 'PIN', placeholder: 'Fake', signal: 'fake-do-not-leak' } });
  assert.equal('credentialField' in secret, false);
  assert.deepEqual(secret.prompt, { type: 'secret', message: 'PIN', placeholder: 'Fake' });
});

test('contradictory credential roles fail closed before producing a title', () => {
  for (const fields of [
    { authType: 'oauth', credentialField: 'api_key.key', prompt: { type: 'secret', message: 'Code' } },
    { authType: 'api_key', credentialField: 'api_key.key', prompt: { type: 'text', message: 'URL' } },
    { authType: 'api_key', credentialField: 'other', prompt: { type: 'secret', message: 'Key' } },
  ]) assert.throws(() => encodeAuthEvent({ ...base, event: 'prompt', providerId: 'openai', promptId: 'step', ...fields }), /invalid-auth-metadata/);
});

test('notifications, started and prompt_closed whitelist every variant', () => {
  const notifications = [
    { type: 'info', message: 'Info', links: [{ url: 'https://example.test', label: 'Help', key: 'fake-do-not-leak' }] },
    { type: 'auth_url', url: 'https://example.test', instructions: 'Open' },
    { type: 'device_code', userCode: 'FAKE', verificationUri: 'https://example.test', intervalSeconds: 1, expiresInSeconds: 60 },
    { type: 'progress', message: 'Pending' },
  ];
  for (const notification of notifications) {
    const value = decode({ ...base, event: 'auth_event', providerId: 'openai-codex',
      notification: { ...notification, credential: 'fake-do-not-leak' } });
    assert.equal(value.notification.type, notification.type);
    assert(!JSON.stringify(value).includes('fake-do-not-leak'));
  }
  const started = decode({ ...base, event: 'started', operation: 'login', providerId: 'openai', credential: 'fake-do-not-leak' });
  assert(!JSON.stringify(started).includes('fake-do-not-leak'));
  const closed = decode({ ...base, event: 'prompt_closed', providerId: 'openai', promptId: 'step-1', reason: 'step-aborted', value: 'fake-do-not-leak' });
  assert.equal(closed.reason, 'step-aborted'); assert(!JSON.stringify(closed).includes('fake-do-not-leak'));
});
