import assert from 'node:assert/strict';
import { chmod, copyFile, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { FAILURE_COPY, OUTPUT_LIMIT_BYTES } from '../cli.ts';
import { createFakeProvider } from './fake-provider/server.mjs';
import { createTestSession } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand } from './fixtures.mjs';

const temporaryRoot = fileURLToPath(new URL('.tmp/', import.meta.url));
const probePath = fileURLToPath(new URL('fixtures/tool-probe.ts', import.meta.url));
const bundleConfigUrl = new URL('../../src-tauri/tauri.conf.json', import.meta.url);
// Copy exactly what Tauri packages, so omitted runtime imports fail this fixture.
const runtimeFiles = Object.entries(JSON.parse(await readFile(bundleConfigUrl, 'utf8')).bundle.resources)
  .filter(([, destination]) => destination.startsWith('kota-ext/'))
  .map(([source, destination]) => ({
    source: fileURLToPath(new URL(source, bundleConfigUrl)),
    name: destination.slice('kota-ext/'.length),
  }));
const names = ['kota_agent_bus', 'kota_bbs', 'kota_ember', 'kota_bartender', 'kota_my_soul'];
const goldenCopy = [
  ['kota_agent_bus', 'Agent Bus', "Send a message to a teammate in the current project. Use the teammate's AKA or agent ID in to. The body is sent unchanged. A successful result means the local dispatch was recorded, not that the teammate has replied."],
  ['kota_bbs', 'Bulletin Board', 'Look up projects and agents, read a BBS thread, or publish a new post or reply. Use action agents to find project IDs and the targetRef for at. Set explicit projects or broadcast for a new thread. Use at for an explicit mention and attachments for files to copy into the post. Successful publication does not confirm delivery to another device.'],
  ['kota_ember', 'Ember', 'Create, list, read, update, or delete scheduled prompts and draft notes for the current project. Schedule targets use agent names, agent IDs, or the configured human name. A draft is a note without delivery timing. Timing: in and every take a duration such as 10m, 2h, or 1d (every at least 1m); at takes a local datetime such as "2026-06-18 14:00"; cron takes a cron expression; idle delivers when the target is idle.'],
  ['kota_bartender', 'Bartender', "Request synchronization of this project's agent worktrees through the running Kota app. Use only when the user has asked to sync. Concurrent sync requests are rejected by the existing service."],
  ['kota_my_soul', 'My Soul', 'kota_my_soul — Your marks (lessons you keep across projects) and episodes (past room messages). Actions: add-mark, edit-mark, delete-mark, read-mark, search-episode, clear-heat. Same arguments and output as the kota-my-soul CLI.'],
];
const settled = record => record.type === 'agent_settled';
const toolStep = (name, args) => ({ type: 'tool', name, arguments: args });
const finalStep = { type: 'text', text: 'Synthetic tools completed.' };

// These five executables exist only on the test Pi child's PATH; no real Kota CLI is invoked.
const mockSource = `#!${process.execPath}
import { appendFileSync } from 'node:fs';
import { basename } from 'node:path';
let body='';
process.stdin.setEncoding('utf8');
process.stdin.on('data',chunk=>body+=chunk);
process.stdin.on('end',()=>{
  const result={binary:basename(process.argv[1]),args:process.argv.slice(2),body,cwd:process.cwd()};
  appendFileSync('mock-calls.jsonl',JSON.stringify(result)+'\\n');
  if(result.args.includes('stdout-limit')||result.args.includes('stderr-limit')) {
    const stream=result.args.includes('stdout-limit')?process.stdout:process.stderr;
    stream.write('x'.repeat(${OUTPUT_LIMIT_BYTES}+1024));setInterval(()=>{},1000);return;
  }
  if(result.args.includes('hold')) {setInterval(()=>{},1000);return;}
  process.stdout.write(JSON.stringify(result));process.stderr.write('fixture stderr');
  if(result.args.includes('nonzero'))process.exitCode=7;
});
`;

async function fixture(t, steps, { probe = false, readonly = false } = {}) {
  await mkdir(temporaryRoot, { recursive: true });
  const directory = await mkdtemp(join(temporaryRoot, 'tools-'));
  const bin = join(directory, 'bin');
  await mkdir(bin);
  for (const name of ['kota-agent-bus', 'kota-bbs', 'kota-ember', 'kota-bartender', 'kota-my-soul']) {
    await writeFile(join(bin, name), mockSource, { mode: 0o700 });
  }
  let resourceDirectory;
  let before;
  const snapshot = async () => Object.fromEntries(await Promise.all((await readdir(resourceDirectory)).sort().map(async name => {
    const path = join(resourceDirectory, name);
    const metadata = await stat(path);
    return [name, { sha256: createHash('sha256').update(await readFile(path)).digest('hex'), size: metadata.size, mtimeMs: metadata.mtimeMs, mode: metadata.mode & 0o777 }];
  })));
  if (readonly) {
    resourceDirectory = join(directory, 'kota-ext');
    await mkdir(resourceDirectory);
    for (const {source, name} of runtimeFiles) {
      await copyFile(source, join(resourceDirectory, name));
      await chmod(join(resourceDirectory, name), 0o444);
    }
    await chmod(resourceDirectory, 0o555);
    before = await snapshot();
  }
  let provider;
  let session;
  t.after(async () => {
    try {
      if (session) await session.close();
      if (provider) await provider.close();
      if (resourceDirectory) assert.deepEqual(await snapshot(), before);
    } finally {
      if (resourceDirectory) {
        await chmod(resourceDirectory, 0o700);
        for (const {name} of runtimeFiles) await chmod(join(resourceDirectory, name), 0o600);
      }
      await rm(directory, { recursive: true, force: true });
    }
  });
  provider = await createFakeProvider({ steps });
  session = await createTestSession({ baseUrl: provider.baseUrl, extensionPath: probe ? probePath : readonly ? resourceDirectory : extensionPath, pathPrefix: bin });
  const calls = async () => (await readFile(join(session.cwd, 'mock-calls.jsonl'), 'utf8')).trim().split('\n').map(line => JSON.parse(line));
  return { provider, session, client: session.client, calls, async assertReadonly() {
    assert.deepEqual(await snapshot(), before);
    assert.equal((await stat(resourceDirectory)).mode & 0o777, 0o555);
  } };
}

async function run(client) {
  const after = client.records.length;
  assert.equal((await client.command('prompt', injectCommand(envelope('msg-tools')))).success, true);
  await client.waitFor(settled, { after });
  return client.records.slice(after).filter(record => record.type === 'tool_execution_end');
}

function parsedResult(event) {
  const result = JSON.parse(event.result.content[0].text);
  assert.equal(event.result.isError, result.exitCode !== 0 || result.error !== undefined);
  assert.equal(event.isError, event.result.isError);
  assert.deepEqual(event.result.details, result.error === 'output_limit' ? { ...result, truncated: true } : result);
  return result;
}

function noExtraSchemaCopy(value) {
  if (!value || typeof value !== 'object') return;
  assert.equal(Object.hasOwn(value, 'description'), false);
  assert.equal(Object.hasOwn(value, 'default'), false);
  for (const child of Object.values(value)) noExtraSchemaCopy(child);
}

test('the real Pi factory preserves five approved Kota tools alongside native codemode/tool_search and the clone tool', { timeout: 20_000 }, async t => {
  const { client, provider } = await fixture(t, [finalStep], { probe: true });
  assert.equal((await client.command('prompt', { message: '/fixture-tools' })).success, true);
  const record = await client.waitFor(record => record.type === 'message_end' && record.message.customType === 'fixture.tools');
  const { tools: allTools, active } = record.message.details;
  const tools = allTools.filter(tool => names.includes(tool.name));
  assert.deepEqual(allTools.filter(tool => !names.includes(tool.name)).map(tool => tool.name).sort(), ['codemode','kota_shadow_clone','tool_search']);
  assert(active.includes('codemode'));
  assert.deepEqual(tools.map(tool => [tool.name, tool.label, tool.description]), goldenCopy);
  for (const tool of tools) {
    assert.equal(active.includes(tool.name), true);
    noExtraSchemaCopy(tool.parameters);
  }
  assert.equal(tools[0].parameters.required.includes('intent'), false);
  // Pi's non-strict conversion keeps only root type/properties/required (pi-ai anthropic-messages.js
  // convertTools), so every tool root must be one object with visible fields, never a union.
  const seenByModel = parameters => ({ type: 'object', properties: parameters.properties ?? {}, required: parameters.required ?? [] });
  for (const tool of tools) {
    assert.equal(tool.parameters.type, 'object');
    assert.notEqual(Object.keys(tool.parameters.properties ?? {}).length, 0);
  }
  const expectedFields = {
    kota_bbs: ['action', 'threadId', 'body', 'projects', 'broadcast', 'at', 'attachments'],
    kota_ember: ['action', 'id', 'draft', 'body', 'to', 'timing', 'endAfter', 'endAt'],
    kota_my_soul: ['action', 'text', 'id', 'query', 'project'],
  };
  for (const tool of tools) {
    const fields = expectedFields[tool.name];
    if (fields) assert.deepEqual(Object.keys(seenByModel(tool.parameters).properties).sort(), [...fields].sort());
  }
  assert.equal(provider.requests.length, 0);
});

test('all five tools execute only mock CLIs with fixed argv and untouched stdin/structured output', { timeout: 25_000 }, async t => {
  const body = '--project-root=/x\n$(not-a-command) "quoted" 中文\nsecond line\u2028\u2029\n';
  const { client, session, provider, calls } = await fixture(t, [
    toolStep(names[0], { to: 'agent-fixture', body }),
    toolStep(names[1], { action: 'agents' }),
    toolStep(names[2], { action: 'add', draft: true, body }),
    toolStep(names[3], { action: 'sync' }),
    toolStep(names[4], { action: 'add-mark', text: body }), finalStep,
  ]);
  const events = await run(client);
  assert.deepEqual(events.map(event => event.toolName), names);
  const expected = [
    { binary: 'kota-agent-bus', args: ['send', '--to', 'agent-fixture'], body, cwd: session.cwd },
    { binary: 'kota-bbs', args: ['agents'], body: '', cwd: session.cwd },
    { binary: 'kota-ember', args: ['add', '--draft'], body, cwd: session.cwd },
    { binary: 'kota-bartender', args: ['sync', '--json'], body: '', cwd: session.cwd },
    { binary: 'kota-my-soul', args: ['add-mark', '--', body], body: '', cwd: session.cwd },
  ];
  assert.deepEqual(await calls(), expected);
  assert.deepEqual(events.map(event => {
    const result = parsedResult(event);
    assert.equal(result.exitCode, 0);
    assert.equal(result.stderr, 'fixture stderr');
    assert.equal(result.error, undefined);
    return JSON.parse(result.stdout);
  }), expected);
  const declared = provider.requests[0].body.tools.filter(tool => names.includes(tool.function.name));
  assert.deepEqual(declared.map(tool => tool.function.name), names);
  // Pi native mode:'on' appends its own call declaration; Kota's approved text
  // and tool schema are unchanged. Pin the native suffix as well.
  assert.deepEqual(declared.map(tool => tool.function.description), goldenCopy.map(row =>
    `${row[2]}\n\nCodemode: \`tools.${row[0]}(args)\` resolves to a string.`));
});

test('cross-field validation returns the approved fixed error without starting a CLI', { timeout: 20_000 }, async t => {
  const { client, session } = await fixture(t, [toolStep('kota_bbs', { action: 'new', body: 'Missing target.' }), finalStep]);
  const [event] = await run(client);
  assert.deepEqual(parsedResult(event), { exitCode: null, stdout: '', stderr: 'Invalid tool arguments.', error: 'invalid_arguments' });
  await assert.rejects(readFile(join(session.cwd, 'mock-calls.jsonl')), { code: 'ENOENT' });
});

for (const [name, field, args] of [
  ['kota_ember', 'update.id', { action: 'update', id: '--project-root=/x' }],
  ['kota_bbs', 'threadId', { action: 'show', threadId: '--project-root=/x' }],
  ['kota_ember', 'to', { action: 'add', to: ['Fixture', '--project-root=/x'], timing: { kind: 'idle' }, body: 'Synthetic blocked task.' }],
]) {
  test(`leading-dash ${name} ${field} is invalid_arguments and never starts a CLI`, { timeout: 20_000 }, async t => {
    const { client, session } = await fixture(t, [toolStep(name, args), finalStep]);
    const events = await run(client);
    assert.equal(events.length, 1);
    assert.deepEqual(parsedResult(events[0]), { exitCode: null, stdout: '', stderr: 'Invalid tool arguments.', error: 'invalid_arguments' });
    await assert.rejects(readFile(join(session.cwd, 'mock-calls.jsonl')), { code: 'ENOENT' });
  });
}

test('a nonzero CLI exit is preserved without invented success or wrapper failure', { timeout: 20_000 }, async t => {
  const { client } = await fixture(t, [toolStep('kota_bbs', { action: 'show', threadId: 'nonzero' }), finalStep]);
  const [event] = await run(client);
  const result = parsedResult(event);
  assert.equal(result.exitCode, 7);
  assert.equal(result.stderr, 'fixture stderr');
  assert.equal(result.error, undefined);
});

for (const stream of ['stdout', 'stderr']) {
  test(`registered tool enforces the ${stream} byte limit with approved error and explicit truncation`, { timeout: 20_000 }, async t => {
    const { client } = await fixture(t, [toolStep('kota_bbs', { action: 'show', threadId: `${stream}-limit` }), finalStep]);
    const [event] = await run(client);
    const result = parsedResult(event);
    assert.equal(result.error, 'output_limit');
    assert.equal(result.stderr.endsWith(FAILURE_COPY.output_limit), true);
    assert.equal(result.stdout.length, stream === 'stdout' ? OUTPUT_LIMIT_BYTES : 0);
    assert.notEqual(result.exitCode, 0);
  });
}

test('Pi abort reaches the registered tool AbortSignal and uses the approved cancellation copy', { timeout: 20_000 }, async t => {
  const { client, provider } = await fixture(t, [toolStep('kota_bbs', { action: 'show', threadId: 'hold' })]);
  assert.equal((await client.command('prompt', injectCommand(envelope('msg-tools-abort')))).success, true);
  await client.waitFor(record => record.type === 'tool_execution_start');
  assert.equal((await client.command('abort')).success, true);
  const event = await client.waitFor(record => record.type === 'tool_execution_end');
  const result = parsedResult(event);
  assert.equal(result.error, 'cancelled');
  assert.equal(result.stderr, FAILURE_COPY.cancelled);
  assert.equal(provider.requests.length, 1);
});

test('Pi loads the packaged runtime resources from a read-only directory without modifying them', { timeout: 25_000 }, async t => {
  const { client, calls, provider, assertReadonly } = await fixture(t, [toolStep('kota_bbs', { action: 'agents' }), finalStep], { readonly: true });
  const [event] = await run(client);
  assert.equal(parsedResult(event).exitCode, 0);
  assert.equal((await calls())[0].binary, 'kota-bbs');
  assert.deepEqual(provider.requests[0].body.tools.filter(tool => names.includes(tool.function.name)).map(tool => tool.function.name), names);
  await assertReadonly();
});
