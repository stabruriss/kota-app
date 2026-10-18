/** TEST ONLY. Runs exclusively inside the parent-created sandbox/temporary HOME. */
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import http from 'node:http';
import { EventEmitter } from 'node:events';
import { syncBuiltinESMExports } from 'node:module';
import { registerPinnedSdkHooks } from '../pinned-sdk.mjs';
const dir = process.env.AUTH_UNIT_DIR;
assert(dir && process.env.HOME === join(dir, 'home'));
registerPinnedSdkHooks();
const { registerAuth } = await import('../../auth.ts');
const { AUTH_CODEC_PREFIX } = await import('../../auth-protocol.ts');
const { ModelRuntime } = await import('@earendil-works/pi-coding-agent');
const authPath = join(process.env.PI_CODING_AGENT_DIR, 'auth.json');
const command = (operationId, operation, fields = {}) => JSON.stringify({ schema: 1, operationId, operation, ...fields });
const decode = text => JSON.parse(text.slice(AUTH_CODEC_PREFIX.length));
const fixtureModel = id => ({ provider: id, id: 'model', name: 'Synthetic model', api: 'openai-completions',
  baseUrl: 'http://127.0.0.1:1', reasoning: true, thinkingLevelMap: { xhigh: null, max: null },
  input: ['text'], contextWindow: 1000, maxTokens: 100, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 } });
function provider(id, kind = 'key') {
  const result = { id, name: `Synthetic ${id}`, getModels: () => [fixtureModel(id)],
    stream() { throw new Error('No model calls'); }, streamSimple() { throw new Error('No model calls'); } };
  if (kind === 'oauth' || kind === 'step' || kind === 'device') result.auth = { oauth: {
    name: 'Synthetic OAuth',
    async login(ui, options) {
      assert.equal(options.getDeviceId(), '11111111-2222-4333-8444-555555555555');
      ui.notify({ type: 'info', message: 'Synthetic info', links: [{ url: 'https://example.invalid' }] });
      ui.notify({ type: 'auth_url', url: 'https://example.invalid/authorize' });
      ui.notify({ type: 'device_code', userCode: 'FAKE', verificationUri: 'https://example.invalid/device', expiresInSeconds: 60 });
      ui.notify({ type: 'progress', message: 'Synthetic progress' });
      if (kind === 'device') {
        await new Promise((_resolve, reject) => ui.signal.addEventListener('abort', () => reject(ui.signal.reason), { once: true }));
      } else if (kind === 'step') {
        const step = new AbortController(); const timer = setTimeout(() => step.abort(), 15);
        try { await ui.prompt({ type: 'manual_code', message: 'Callback race', signal: step.signal }); }
        catch { assert(step.signal.aborted); } finally { clearTimeout(timer); }
      } else {
        assert.equal(await ui.prompt({ type: 'select', message: 'Method', options: [{ id: 'a', label: 'Same' }, { id: 'b', label: 'Same' }] }), 'b');
        await ui.prompt({ type: 'text', message: 'Tenant' });
        assert.equal(await ui.prompt({ type: 'secret', message: 'OAuth PIN' }), '$FAKE_OAUTH_TOKEN');
        await ui.prompt({ type: 'manual_code', message: 'Code' });
      }
      ui.signal.throwIfAborted();
      return { type: 'oauth', access: 'fake-access', refresh: 'fake-refresh', expires: Date.now() + 3600000 };
    },
    refresh: async credential => credential, toAuth: async credential => ({ apiKey: credential.access }),
  } };
  else result.auth = { apiKey: {
    name: 'Synthetic key',
    async login(ui) {
      if (kind === 'ignore') {
        await new Promise(resolve => setTimeout(resolve, 150));
        ui.notify({ type: 'progress', message: 'late synthetic progress' });
        return { type: 'api_key', key: 'fake-late-key' };
      }
      const key = await ui.prompt({ type: 'secret', message: 'Synthetic key' });
      if (kind === 'fail') throw new Error('fake-secret-error-must-not-leak');
      return { type: 'api_key', key };
    },
    async check({ credential }) {
      if (kind === 'sync' && credential) throw new Error('fake-secret-sync-must-not-leak');
      return credential ? { type: 'api_key', source: 'synthetic' } : undefined;
    },
    resolve: async ({ credential }) => credential ? { auth: { apiKey: credential.key } } : undefined,
  } };
  return result;
}
const fixtures = [provider('fake-auth'), provider('fake-oauth', 'oauth'), provider('fake-step', 'step'),
  provider('fake-fail', 'fail'), provider('fake-sync', 'sync'), provider('fake-ignore', 'ignore'), provider('fake-device','device')];
let lastRuntime;
let originalHandlers;
const createRuntime = async options => {
  assert.equal(options.modelsPath, null); // All ordinary provider logins stay native-only.
  const runtime = await ModelRuntime.create(options);
  for (const fixture of fixtures) runtime.registerNativeProvider(fixture);
  await runtime.refresh({ allowNetwork: false });
  originalHandlers = new Map(runtime.getProviders().map(entry => [entry.id, entry.auth.apiKey?.login ?? entry.auth.oauth?.login]));
  lastRuntime = runtime;
  return runtime;
};
let handler;
const hooks = new Map();
const records = [];
let inputMode = 'answer';
let ownerRefreshes = 0;
let refreshError;
const ui = {
  notify(text) { records.push(decode(text)); },
  async input(title, _placeholder, { signal }) {
    const event = decode(title); records.push(event);
    if (inputMode === 'cancel') return undefined;
    if (inputMode === 'pending') return new Promise(resolve => {
      if (signal.aborted) resolve(undefined); else signal.addEventListener('abort', () => resolve(undefined), { once: true });
    });
    return event.authType === 'oauth' && event.prompt.type === 'secret' ? '$FAKE_OAUTH_TOKEN' : 'fake-unit-key';
  },
  async select(title, options) { const event = decode(title); records.push(event); assert.deepEqual(options, ['a', 'b']); return 'b'; },
};
const context = { mode: 'rpc', ui, isIdle: () => true, modelRegistry: {
  refresh: async () => { ownerRefreshes++; return { aborted: false, errors: new Map() }; }, getError: () => refreshError,
} };
registerAuth({ registerCommand(name, value) { assert.equal(name, 'kota-auth'); handler = value.handler; }, on(name, callback) { hooks.set(name, callback); } },
  { createRuntime, loginTimeoutMs: 3000, controlTimeoutMs: 3000 });
const result = id => records.find(event => event.operationId === id && event.event === 'result');
const login = (id, providerId, authType = 'api_key') => handler(command(id, 'login', { providerId, authType, uiSchema: 1 }), context);
const results = [];
await login('key', 'fake-auth');
assert.equal(result('key').commit, 'confirmed'); assert.equal(result('key').outcome, 'succeeded');
assert.equal(records.find(event => event.operationId === 'key' && event.event === 'prompt').credentialField, 'api_key.key');
assert.equal(JSON.parse(await readFile(authPath, 'utf8'))['fake-auth'].key, 'fake-unit-key');
results.push('key-persistence-and-role');
assert.notEqual(lastRuntime.getProvider('fake-auth').auth.apiKey.login, originalHandlers.get('fake-auth'));
assert.equal(lastRuntime.getProvider('deepseek').auth.apiKey.login, originalHandlers.get('deepseek'));
assert.equal(fixtures[0].auth.apiKey.login, originalHandlers.get('fake-auth'));
results.push('helper-target-only-clone-preserves-other-and-original-handlers');
await login('oauth', 'fake-oauth', 'oauth');
assert.equal(result('oauth').outcome, 'succeeded');
assert(records.filter(event => event.operationId === 'oauth' && event.event === 'prompt').every(event => !event.credentialField));
assert.equal(records.filter(event => event.operationId === 'oauth' && event.event === 'auth_event').length, 4);
results.push('oauth-all-variants-and-no-key-role');
inputMode = 'pending'; await login('step', 'fake-step', 'oauth'); inputMode = 'answer';
assert.equal(result('step').outcome, 'succeeded');
assert(records.some(event => event.operationId === 'step' && event.event === 'prompt_closed' && event.reason === 'step-aborted'));
results.push('step-abort-not-flow-cancel');
const devicePending = login('device-cancelled','fake-device','oauth');
while (!records.some(event => event.operationId === 'device-cancelled' && event.event === 'auth_event' && event.notification.type === 'device_code')) await new Promise(resolve => setImmediate(resolve));
await handler(command('device-cancel-command','cancel',{targetOperationId:'device-cancelled'}), context);
await devicePending;
assert.equal(result('device-cancelled').outcome, 'cancelled'); assert.equal(result('device-cancelled').commit, 'not-written');
results.push('oauth-device-wait-cancel-before-native-return');
inputMode = 'cancel'; await login('cancelled', 'fake-auth'); inputMode = 'answer';
assert.equal(result('cancelled').outcome, 'cancelled'); assert.equal(result('cancelled').commit, 'not-written');
results.push('key-precommit-cancel-preserves-old');
await login('failure', 'fake-fail');
assert.equal(result('failure').outcome, 'failed'); assert.equal(result('failure').commit, 'not-written');
assert(!JSON.stringify(records).includes('fake-secret-error-must-not-leak'));
results.push('native-pre-return-failure-whitelist-and-not-written');
await login('sync', 'fake-sync');
assert.equal(result('sync').outcome, 'committed-unsynchronized'); assert.equal(result('sync').commit, 'confirmed');
assert(!JSON.stringify(records).includes('fake-secret-sync-must-not-leak'));
results.push('committed-sync-failure-no-secret');
// Use a fresh controller/runtime after the deliberately faulting provider snapshot.
let cleanHandler;
registerAuth({ registerCommand(_name, value) { cleanHandler = value.handler; }, on() {} }, { createRuntime: async options => {
  const runtime = await ModelRuntime.create(options); runtime.registerNativeProvider(provider('fake-auth')); await runtime.refresh({ allowNetwork: false }); return runtime;
} });
await cleanHandler(command('status', 'status', { providerId: 'fake-auth' }), context);
assert.equal(result('status').status.storedType, 'api_key'); assert.equal(result('status').status.canApiKey, true);
assert.equal('source' in result('status').status, false);
results.push('fresh-status-handler-metadata-no-source');
await cleanHandler(command('catalog', 'catalog', { providerId: 'fake-auth' }), context);
assert.deepEqual(result('catalog').catalog.providers[0].models[0].thinkingLevels, ['off','minimal','low','medium','high']);
await cleanHandler(command('resolve', 'resolve', { providerId: 'fake-auth', modelId: 'model', thinkingLevel: 'max' }), context);
assert.equal(result('resolve').resolved.thinkingLevel, 'high');
results.push('catalog-and-exact-native-clamp');
await cleanHandler(command('missing', 'resolve', { providerId: 'fake-auth', modelId: 'unknown-id', thinkingLevel: 'high' }), context);
assert.equal(result('missing').code, 'model-not-found');
results.push('unknown-model-does-not-fallback');
const native = await ModelRuntime.create({ authPath, modelsPath: null, allowModelNetwork: false });
const special = new Set(['amazon-bedrock','google-vertex','cloudflare-workers-ai','cloudflare-ai-gateway','azure','llama.cpp']);
let standardCount = 0;
for (const entry of native.getProviders()) {
  const key = entry.auth.apiKey;
  if (!key?.login || !key.resolve || special.has(entry.id)) continue;
  let prompts = 0;
  const literal = '  !FAKE_LITERAL ${FAKE_TEMPLATE}';
  const credential = await key.login({ signal: new AbortController().signal,
    prompt: async prompt => { assert.equal(prompt.type, 'secret', entry.id); prompts++; return literal; }, notify() {} });
  assert.equal(prompts, 1, entry.id); assert.equal(credential.key, literal, entry.id); standardCount++;
}
assert(standardCount >= 30);
results.push('all-pinned-standard-key-handlers-verbatim-single-secret');
let nativeHandler;
registerAuth({ registerCommand(_name, value) { nativeHandler = value.handler; }, on() {} }, { createRuntime: async () => native });
let choice;
const nativeContext = { ...context, ui: { ...ui,
  async select(title, options) { const event = decode(title); records.push(event); assert(options.includes(choice)); return choice; },
  async input(title) { const event = decode(title); records.push(event);
    return event.credentialField ? 'fake-cloud-key' : '!fake-cloud-config $CONFIG'; },
} };
for (const [providerId, selections] of [['amazon-bedrock', ['bearer-token','aws-profile','credential-chain']],
    ['google-vertex', ['api-key','adc','service-account']], ['cloudflare-workers-ai', [undefined]], ['cloudflare-ai-gateway', [undefined]]]) {
  for (choice of selections) {
    const id = `native-${providerId}-${choice ?? 'key'}`;
    await nativeHandler(command(id, 'login', { providerId, authType: 'api_key', uiSchema: 1 }), nativeContext);
    assert.equal(result(id).outcome, 'succeeded', id);
    const prompts = records.filter(event => event.operationId === id && event.event === 'prompt');
    const keyPrompts = prompts.filter(event => event.credentialField);
    assert.equal(keyPrompts.length, providerId.startsWith('cloudflare-') || choice === 'bearer-token' || choice === 'api-key' ? 1 : 0, id);
    assert(prompts.every(event => !event.credentialField || event.prompt.type === 'secret'));
  }
}
results.push('native-bedrock-vertex-cloudflare-stable-branches-and-key-roles');
for (const providerId of ['anthropic','openai','openai-codex','github-copilot','openrouter','kimi-coding','meta','xai','radius']) {
  const id = `native-cap-${providerId}`;
  await nativeHandler(command(id, 'status', { providerId }), nativeContext);
  assert.equal(result(id).status.canOAuth, true, providerId);
}
results.push('nine-native-oauth-capabilities-no-authorization-requests');
const brandingUrls = [
  ['https://example.invalid/authorize?agent_name_hint=Pi&client_id=dynamic_agent_client&state=fake-state&code_challenge=fake-challenge&redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback',
    'https://example.invalid/authorize?agent_name_hint=Kota&client_id=dynamic_agent_client&state=fake-state&code_challenge=fake-challenge&redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fauth%2Fcallback'],
  ['https://EXAMPLE.invalid:443/a%2fb?state=a%2fb+%20c&&agent_name_hint=%50i&code_challenge=FAKE%2Dchallenge&redirect_uri=http%3a%2f%2f127.0.0.1%3a1455%2fauth%2fcallback&scope=one%20two+three&nonce=&opaque=%26agent_name_hint%3DPi#keep%2f',
    'https://EXAMPLE.invalid:443/a%2fb?state=a%2fb+%20c&&agent_name_hint=Kota&code_challenge=FAKE%2Dchallenge&redirect_uri=http%3a%2f%2f127.0.0.1%3a1455%2fauth%2fcallback&scope=one%20two+three&nonce=&opaque=%26agent_name_hint%3DPi#keep%2f'],
  ['https://example.invalid/?%61gent_name_hint=Pi&state=fake', 'https://example.invalid/?%61gent_name_hint=Kota&state=fake'],
  ['https://example.invalid/?state=fake', 'https://example.invalid/?state=fake'],
  ['https://example.invalid/?agent_name_hint=Kota', 'https://example.invalid/?agent_name_hint=Kota'],
  ['https://example.invalid/?agent_name_hint=Other', 'https://example.invalid/?agent_name_hint=Other'],
  ['https://example.invalid/#?agent_name_hint=Pi', 'https://example.invalid/#?agent_name_hint=Pi'],
  ['not-an-absolute-url?agent_name_hint=Pi', 'not-an-absolute-url?agent_name_hint=Pi'],
];
for (const providerId of ['openai','openai-codex']) {
  let brandingHandler;
  const id = `branding-${providerId}`;
  registerAuth({ registerCommand(_name, value) { brandingHandler = value.handler; }, on() {} }, { createRuntime: async options => {
    const runtime = await ModelRuntime.create(options);
    const original = runtime.getProvider(providerId);
    runtime.registerNativeProvider({ ...original, auth:{ ...original.auth, oauth:{ ...original.auth.oauth,
      async login(interaction) {
        for (const [url] of brandingUrls) {
          const event = Object.freeze({ type:'auth_url', url, instructions:'Synthetic instructions' });
          interaction.notify(event); assert.equal(event.url, url);
        }
        return { type:'oauth', access:'fake-branding-access', refresh:'fake-branding-refresh', expires:Date.now()+3600000,
          clientId:'fake-issued-client', scopes:['chatgpt.tokens.use.direct'] };
      },
    } } });
    await runtime.refresh({ allowNetwork:false }); return runtime;
  } });
  await brandingHandler(command(id,'login',{providerId,authType:'oauth',uiSchema:1}), context);
  assert.equal(result(id).outcome, 'succeeded');
  const links = records.filter(event => event.operationId === id && event.event === 'auth_event');
  assert.deepEqual(links.map(event => event.notification.url), brandingUrls.map(pair => pair[providerId === 'openai' ? 1 : 0]));
  assert(links.every(event => event.notification.instructions === 'Synthetic instructions'));
}
results.push('openai-auth-url-branding-preserves-all-other-url-bytes');
results.push('other-provider-auth-urls-remain-byte-identical');
// Exercise the real public OpenAI handler's manual callback parser/PKCE with an
// in-memory fetch response. Pi 1.1 requires callback setup to succeed before
// offering manual input; a fake public Node HTTP server supplies that setup.
// The sandbox continues to deny every real listener and all network.
const previousFetch = globalThis.fetch;
const previousCreateServer = http.createServer;
let originalAuthUrl;
let manualCallback;
let interceptedTokenRequests = 0;
let manualHandler;
try {
  http.createServer = () => {
    const server = new EventEmitter();
    server.listen = (...args) => { queueMicrotask(args.at(-1)); return server; };
    server.close = () => server;
    server.closeAllConnections = () => {};
    return server;
  };
  syncBuiltinESMExports();
  globalThis.fetch = async (url, options) => {
    assert.equal(String(url), 'https://auth.openai.com/api/accounts/oauth/token');
    interceptedTokenRequests++;
    const body = new URLSearchParams(options.body);
    assert.equal(body.get('grant_type'), 'authorization_code'); assert.equal(body.get('client_id'), 'fake-manual-issued-client');
    assert.equal(body.get('code'), 'fake-manual-code'); assert.equal(body.get('redirect_uri'), 'http://127.0.0.1:1455/auth/callback');
    const challenge = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(body.get('code_verifier')));
    assert.equal(Buffer.from(challenge).toString('base64url'), new URL(originalAuthUrl).searchParams.get('code_challenge'));
    return new Response(JSON.stringify({ access_token:'fake-manual-access', refresh_token:'fake-manual-refresh', id_token:'fake-id',
      expires_in:3600, scope:'openid profile email offline_access resource.invoke chatgpt.tokens.use.direct' }));
  };
  registerAuth({ registerCommand(_name, value) { manualHandler = value.handler; }, on() {} }, { createRuntime: async options => {
    const runtime = await ModelRuntime.create(options);
    const original = runtime.getProvider('openai'); const nativeLogin = original.auth.oauth.login;
    runtime.registerNativeProvider({ ...original, auth:{ ...original.auth, oauth:{ ...original.auth.oauth,
      login(interaction, options) { return nativeLogin.call(original.auth.oauth, { ...interaction, notify(event) {
        if (event.type === 'auth_url') originalAuthUrl = event.url;
        interaction.notify(event);
      } }, options); },
    } } });
    await runtime.refresh({ allowNetwork:false }); return runtime;
  } });
  await manualHandler(command('native-manual-branding','login',{providerId:'openai',authType:'oauth',uiSchema:1}), { ...context, ui:{ ...ui,
    async input(title) {
      const event = decode(title); records.push(event); assert.equal(event.prompt.type, 'manual_code');
      assert.equal(event.credentialField, undefined);
      const link = records.find(event => event.operationId === 'native-manual-branding' && event.event === 'auth_event' && event.notification.type === 'auth_url').notification.url;
      assert.equal(link, originalAuthUrl.replace('agent_name_hint=Pi', 'agent_name_hint=Kota'));
      const callback = new URL('http://127.0.0.1:1455/auth/callback');
      callback.search = new URLSearchParams({ code:'fake-manual-code',state:new URL(link).searchParams.get('state'),client_id:'fake-manual-issued-client' }).toString();
      manualCallback = callback.href; return manualCallback;
    },
  } });
  assert.equal(result('native-manual-branding').outcome, 'succeeded'); assert.equal(interceptedTokenRequests, 1);
  const saved = JSON.parse(await readFile(authPath, 'utf8')).openai;
  assert.equal(saved.clientId, 'fake-manual-issued-client'); assert.equal(saved.access, 'fake-manual-access');
  assert.equal(new URL(manualCallback).searchParams.get('state'), new URL(originalAuthUrl).searchParams.get('state'));
} finally { globalThis.fetch = previousFetch; http.createServer = previousCreateServer; syncBuiltinESMExports(); }
results.push('real-openai-manual-callback-state-pkce-and-issued-client-unchanged');
const warmed = await ModelRuntime.create({ authPath, modelsPath: null, allowModelNetwork: false });
for (const entry of [provider('fake-auth'), provider('fake-ignore','ignore')]) warmed.registerNativeProvider(entry);
await warmed.refresh({ allowNetwork: false });
let fastHandler;
registerAuth({ registerCommand(_name, value) { fastHandler = value.handler; }, on() {} },
  { createRuntime: async () => warmed, loginTimeoutMs: 250 });
inputMode = 'pending';
const pending = fastHandler(command('pending-flow','login',{providerId:'fake-auth',authType:'api_key',uiSchema:1}), context);
while (!records.some(event => event.operationId === 'pending-flow' && event.event === 'prompt')) await new Promise(resolve => setImmediate(resolve));
await fastHandler(command('busy-other','login',{providerId:'fake-auth',authType:'api_key',uiSchema:1}), context);
assert.equal(result('busy-other').code, 'busy-auth');
await fastHandler(command('cancel-command','cancel',{targetOperationId:'pending-flow'}), context);
await pending;
assert.equal(result('pending-flow').commit, 'not-written'); assert.equal(result('pending-flow').outcome, 'cancelled');
results.push('independent-correlated-cancel-and-busy-auth');
await fastHandler(command('key-timeout','login',{providerId:'fake-auth',authType:'api_key',uiSchema:1}), context);
assert.equal(result('key-timeout').outcome, 'timeout'); assert.equal(result('key-timeout').commit, 'not-written');
inputMode = 'answer';
results.push('prompt-timeout-precommit-closure');
let shortHandler;
registerAuth({ registerCommand(_name, value) { shortHandler = value.handler; }, on() {} },
  { createRuntime: async () => warmed, loginTimeoutMs: 30 });
const beforeIgnored = await readFile(authPath);
await shortHandler(command('ignore-timeout','login',{providerId:'fake-ignore',authType:'api_key',uiSchema:1}), context);
assert.equal(result('ignore-timeout').outcome, 'timeout'); assert.equal(result('ignore-timeout').commit, 'not-written');
await new Promise(resolve => setTimeout(resolve, 180));
assert((await readFile(authPath)).equals(beforeIgnored));
assert(!records.some(event => event.operationId === 'ignore-timeout' && event.event === 'auth_event'));
results.push('noncooperative-timeout-no-replay-or-late-events');
// Public caller-injected store witnesses exact stages; this is NOT a production store.
for (const phase of ['before','after','sync','throw','logout-before','logout-after']) {
  const map = new Map(Object.entries(JSON.parse(await readFile(authPath, 'utf8'))));
  map.set('fake-witness', { type:'api_key', key:'fake-existing-witness' });
  await writeFile(authPath, JSON.stringify(Object.fromEntries(map)));
  let witnessHandler;
  const id = `witness-${phase}`;
  const cancel = () => witnessHandler(command(`${id}-cancel`, 'cancel', {targetOperationId:id}), context);
  const save = () => writeFile(authPath, JSON.stringify(Object.fromEntries(map)));
  const store = {
    read: async providerId => map.get(providerId),
    list: async () => [...map].map(([providerId, credential]) => ({ providerId, type:credential.type })),
    async modify(providerId, update, { signal }) {
      const next = await update(map.get(providerId));
      assert.equal(next.key, 'fake-unit-key');
      if (phase === 'before') { await cancel(); signal.throwIfAborted(); }
      map.set(providerId, next); await save();
      if (phase === 'after') { await cancel(); signal.throwIfAborted(); }
      if (phase === 'throw') throw new Error('fake-write-completed-but-store-rejected');
      if (phase === 'sync') await cancel();
      return next;
    },
    async delete(providerId, { signal }) {
      if (phase === 'logout-before') { await cancel(); signal.throwIfAborted(); }
      map.delete(providerId); await save();
      await cancel(); signal.throwIfAborted();
    },
  };
  registerAuth({ registerCommand(_name, value) { witnessHandler = value.handler; }, on() {} }, { createRuntime: async options => {
    const runtime = await ModelRuntime.create({ ...options, credentials:store });
    runtime.registerNativeProvider(provider('fake-witness')); await runtime.refresh({ allowNetwork:false }); return runtime;
  } });
  await witnessHandler(command(id, phase.startsWith('logout') ? 'logout' : 'login', phase.startsWith('logout')
    ? {providerId:'fake-witness'} : {providerId:'fake-witness',authType:'api_key',uiSchema:1}), context);
  assert.equal(result(id).commit, phase === 'sync' ? 'confirmed' : 'unknown', phase);
  assert.equal(result(id).outcome, phase === 'sync' ? 'committed-unsynchronized' : 'indeterminate', phase);
  const saved = JSON.parse(await readFile(authPath, 'utf8'));
  assert.equal(saved['fake-witness']?.key, phase === 'before' || phase === 'logout-before' ? 'fake-existing-witness'
    : phase === 'logout-after' ? undefined : 'fake-unit-key', phase);
}
results.push('credential-returned-modify-before-and-after-abort-stay-unknown');
results.push('credential-returned-store-wrote-then-rejected-stays-unknown');
results.push('postcommit-sync-abort-is-confirmed');
results.push('logout-invoked-abort-before-and-after-delete-stays-unknown');
const originalFile = await readFile(authPath);
await writeFile(authPath, '{');
await cleanHandler(command('bad-file', 'status'), context);
assert.equal(result('bad-file').code, 'storage-failed'); assert.equal(result('bad-file').statuses, undefined);
await writeFile(authPath, originalFile);
results.push('bad-fresh-store-not-empty');
// Magpie is the only login synthesized from Kota's owned models.json.
const magpieModelsPath = join(process.env.PI_CODING_AGENT_DIR, 'models.json');
await writeFile(magpieModelsPath, JSON.stringify({ kotaMagpie:{mode:'local',ip:'',port:3425}, providers:{
  magpie:{api:'openai-completions',baseUrl:'http://127.0.0.1:1/v1',models:[{id:'upstream/with/slashes',name:'Synthetic Magpie'}]},
  deepseek:{models:[{id:'fixture-overlay-only',name:'Must not enter ordinary login'}]},
} }));
let magpieHandler, magpieRuntime, magpieExpectedPath = magpieModelsPath;
registerAuth({ registerCommand(_name,value) { magpieHandler=value.handler; }, on() {} }, { createRuntime:async options => {
  assert.equal(options.modelsPath,magpieExpectedPath); assert.equal(options.allowModelNetwork,false);
  magpieRuntime = await ModelRuntime.create(options); return magpieRuntime;
} });
const beforeMagpie = JSON.parse(await readFile(authPath,'utf8'));
await magpieHandler(command('magpie-owned-login','login',{providerId:'magpie',authType:'api_key',uiSchema:1}),context);
assert.equal(result('magpie-owned-login').outcome,'succeeded'); assert.equal(result('magpie-owned-login').commit,'confirmed');
assert.equal(magpieRuntime.getModel('magpie','upstream/with/slashes').id,'upstream/with/slashes');
assert.equal(records.find(e => e.operationId === 'magpie-owned-login' && e.event === 'prompt').credentialField,'api_key.key');
results.push('magpie-owned-models-synthesize-the-native-api-key-handler');
const afterMagpie = JSON.parse(await readFile(authPath,'utf8'));
assert.deepEqual(afterMagpie.magpie,{type:'api_key',key:'fake-unit-key'});
delete afterMagpie.magpie; assert.deepEqual(afterMagpie,beforeMagpie);
assert(!JSON.stringify(records).includes('fake-unit-key'));
results.push('magpie-key-uses-owned-auth-only-and-preserves-other-entries');
magpieExpectedPath=null;
await magpieHandler(command('ordinary-login-after-magpie','login',{providerId:'deepseek',authType:'api_key',uiSchema:1}),context);
assert.equal(result('ordinary-login-after-magpie').outcome,'succeeded');
assert.equal(magpieRuntime.getModel('deepseek','fixture-overlay-only'),undefined);
assert.equal(magpieRuntime.getProvider('magpie'),undefined);
results.push('other-provider-login-still-bypasses-all-models-overlays');
delete process.env.KOTA_LOCAL_AUTH_HELPER;
delete process.env.KOTA_LOCAL_INSTALLATION_UUID;
await assert.rejects(cleanHandler(command('not-helper', 'login', { providerId: 'fake-auth', authType: 'api_key', uiSchema: 1 }), context), /helper-required/);
await cleanHandler(command('owner-refresh', 'refresh'), context); assert.equal(ownerRefreshes, 1);
await cleanHandler(command('nonidle-refresh', 'refresh'), { ...context, isIdle: () => false });
assert.equal(result('nonidle-refresh').code, 'invalid-request'); assert.equal(ownerRefreshes, 1);
refreshError = 'fake-secret-error-must-not-leak';
await cleanHandler(command('refresh-failed', 'refresh'), context);
assert.equal(result('refresh-failed').code, 'credential-sync-failed');
assert(!JSON.stringify(records).includes('fake-secret-error-must-not-leak'));
results.push('owner-only-idle-refresh-with-get-error-check');
// Refresh keeps an existing Magpie selection on its new configuration, without
// changing its ID or touching another provider, a removed model, or a busy owner.
refreshError=undefined;
let magpieRefreshHandler; const selectedModels=[], restoredThinking=[];
registerAuth({ registerCommand(_name,value) { magpieRefreshHandler=value.handler; }, on() {},
  getThinkingLevel() { return 'low'; }, setThinkingLevel(level) { restoredThinking.push(level); },
  async setModel(model) { selectedModels.push(model); return true; },
}, { createRuntime:async () => { throw Error('Refresh must use the owner registry'); } });
const oldMagpie={...fixtureModel('magpie'),id:'upstream/with/slashes'};
const newMagpie={...oldMagpie,baseUrl:'http://127.0.0.1:2/v1'};
const ownerContext={...context,model:oldMagpie,modelRegistry:{...context.modelRegistry,find:() => newMagpie}};
await magpieRefreshHandler(command('magpie-new-target','refresh'),ownerContext);
assert.equal(result('magpie-new-target').outcome,'succeeded'); assert.deepEqual(selectedModels,[newMagpie]);
assert.equal(selectedModels[0].id,oldMagpie.id); assert.deepEqual(restoredThinking,['low']);
results.push('idle-magpie-same-id-refresh-updates-native-model-configuration');
await magpieRefreshHandler(command('magpie-unchanged','refresh'),{...ownerContext,model:newMagpie});
assert.equal(selectedModels.length,1); assert.equal(result('magpie-unchanged').outcome,'succeeded');
results.push('unchanged-magpie-refresh-does-not-repeat-model-selection');
await magpieRefreshHandler(command('magpie-removed','refresh'),{...ownerContext,modelRegistry:{...ownerContext.modelRegistry,find:() => undefined}});
assert.equal(selectedModels.length,1); assert.equal(result('magpie-removed').outcome,'succeeded');
results.push('removed-magpie-model-does-not-fall-back-to-another-model');
await magpieRefreshHandler(command('other-model-refresh','refresh'),{...ownerContext,model:fixtureModel('openai'),
  modelRegistry:{...ownerContext.modelRegistry,find:() => { throw Error('Do not reselect other providers'); }}});
assert.equal(selectedModels.length,1); assert.equal(result('other-model-refresh').outcome,'succeeded');
results.push('other-provider-refresh-keeps-its-original-selected-model');
await magpieRefreshHandler(command('busy-magpie-refresh','refresh'),{...ownerContext,isIdle:() => false});
assert.equal(selectedModels.length,1); assert.equal(result('busy-magpie-refresh').code,'invalid-request');
results.push('busy-magpie-owner-cannot-refresh-its-selected-model');
assert.equal(records.filter(event => event.event === 'result').length, 59);
console.log(JSON.stringify({ schema: 1, testOnly: true, results: results.map(name => ({ name, passed: true })),
  standardNativeHandlers: standardCount, interceptedTokenRequests, realModelOrAuthRequests: 0, rawSecretsRecorded: false }));
