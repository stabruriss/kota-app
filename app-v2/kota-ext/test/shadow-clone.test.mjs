import test from 'node:test';
import assert from 'node:assert/strict';
import { registerPinnedSdkHooks } from './pinned-sdk.mjs';
// Node SDK witness only, like auth-unit-worker. Real transport and tools are
// separately exercised with the pinned standalone Pi, not this module mapping.
registerPinnedSdkHooks();
const { registerShadowClones, CLONE_CODEC, CLONE_DESCRIPTION } = await import('../shadow-clone.ts');

const register = () => {
  let tool;
  registerShadowClones({ registerTool(value) { tool = value; } });
  return tool;
};

test('the thin clone tool keeps the exact approved copy and one root-object schema for all five actions', () => {
  const tool = register();
  assert.equal(CLONE_DESCRIPTION, `Start copies of yourself. Each clone is a
separate agent in this workspace. Clones
do not have context, so give each one a complete task. create
returns with the clone ids; each clone's final
reply arrives as a message when it finishes. ask sends a follow-up to a
clone that has finished; it continues with its own history. status lists
your clones and what each one last said. stop ends a clone and keeps its
records in under-table/clone-<id>/; destroy removes them. Unless read only, clones could edit your files. At most 7 clones run at once, and a run is limited to max 90
minutes unless you set another limit.`);
  assert.equal(tool.name, 'kota_shadow_clone');
  assert.equal(tool.parameters.type, 'object');
  assert.deepEqual(tool.parameters.required, ['action']);
  assert.deepEqual(tool.parameters.properties.action.anyOf.map(item => item.const), ['create', 'ask', 'status', 'stop', 'destroy']);
  assert.deepEqual(Object.keys(tool.parameters.properties), ['action', 'tasks', 'clone_id', 'message', 'timeout_minutes']);
  assert.equal(tool.parameters.properties.tasks.items.properties.task.description, 'What the clone should do, with the context it needs.');
  assert.equal(tool.parameters.properties.tasks.items.properties.timeout_minutes.description, 'Run time limit. Default 90. Use 0 for no limit.');
});

test('all five actions are unchanged private UI requests, not prompts or CLI calls, and return immediately on the Rust reply', async () => {
  const tool = register(), signal = new AbortController().signal;
  const calls = [];
  const ctx = { mode: 'rpc', ui: { async input(title, placeholder, options) {
    calls.push({ title, placeholder, signal: options.signal });
    return JSON.stringify({ success: true, data: { fixture: calls.length } });
  } } };
  const requests = [
    { action: 'create', tasks: [{ task: 'Synthetic task', read_only: true }] },
    { action: 'ask', clone_id: 1, message: 'Synthetic follow-up', timeout_minutes: 0 },
    { action: 'status' }, { action: 'stop', clone_id: 1 }, { action: 'destroy', clone_id: 1 },
  ];
  for (const args of requests) {
    const result = await tool.execute('fixture', args, signal, undefined, ctx);
    assert.equal(result.isError, false);
    assert.deepEqual(JSON.parse(result.content[0].text), result.details);
  }
  assert.deepEqual(calls.map(call => JSON.parse(call.title.slice(CLONE_CODEC.length))), requests);
  assert(calls.every(call => call.title.startsWith(CLONE_CODEC) && call.placeholder === '' && call.signal === signal));
});

test('cancellation and malformed replies become structured failures; TUI does not invoke the private UI', async () => {
  const tool = register();
  for (const [reply, error] of [[undefined, 'clone_cancelled'], ['not-json', 'clone_response_invalid'], ['null', 'clone_response_invalid'], [JSON.stringify({ success: false, error: 'clone_running' }), 'clone_running']]) {
    const result = await tool.execute('fixture', { action: 'status' }, undefined, undefined, { mode: 'rpc', ui: { input: async () => reply } });
    assert.equal(result.isError, true); assert.deepEqual(result.details, { error });
  }
  const result = await tool.execute('fixture', { action: 'status' }, undefined, undefined, { mode: 'interactive', ui: { input() { assert.fail('TUI cannot use a Kota owner'); } } });
  assert.deepEqual(result.details, { error: 'clone_rpc_required' });
});

test('auth and catalog helpers never register the clone tool', () => {
  const previous = process.env.KOTA_LOCAL_AUTH_HELPER;
  process.env.KOTA_LOCAL_AUTH_HELPER = '1';
  try { assert.equal(register(), undefined); }
  finally { if (previous === undefined) delete process.env.KOTA_LOCAL_AUTH_HELPER; else process.env.KOTA_LOCAL_AUTH_HELPER = previous; }
});
