import { EventEmitter } from 'node:events';
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import { lstatSync, readFileSync } from 'node:fs';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';
import { StringDecoder } from 'node:string_decoder';
import { fileURLToPath, pathToFileURL } from 'node:url';
import assert from 'node:assert/strict';
import { MODEL, PROVIDER } from './server.mjs';

const MAX_RECORD_BYTES = 64 * 1024 * 1024;
const MAX_RECORDS = 20_000;

/** Explicit pinned staging only. Native RPC tests never discover a PATH installation. */
export function resolveTestPi(root = process.env.KOTA_LOCAL_TEST_PI_RUNTIME_ROOT) {
  if (!root || !isAbsolute(root)) throw new Error('Pass an absolute KOTA_LOCAL_TEST_PI_RUNTIME_ROOT');
  const pin = JSON.parse(readFileSync(new URL('../../../vendor/pi-runtime/pi-runtime.lock.json', import.meta.url), 'utf8'));
  const manifest = join(root, 'VENDOR-MANIFEST.json');
  const metadata = lstatSync(manifest);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size > 1024 * 1024 || pin.version !== '1.3.0' ||
      createHash('sha256').update(readFileSync(manifest)).digest('hex') !== pin.vendor_manifest_sha256) {
    throw new Error('Pi test runtime does not match the checked-in pin');
  }
  for (const file of ['pi', 'photon_rs_bg.wasm', 'native/darwin/prebuilds/darwin-arm64/darwin-platform.node', 'package.json']) {
    const stat = lstatSync(join(root, file));
    if (!stat.isFile() || stat.isSymbolicLink() || (file === 'pi' && !(stat.mode & 0o111))) {
      throw new Error('Pi test runtime entrypoint is missing or invalid');
    }
  }
  return join(root, 'pi');
}

/** Test-only RPC transport. LF is the only record delimiter, including inside Unicode text. */
export class TestRpcClient {
  records = [];
  lines = [];
  stderr = '';
  events = new EventEmitter();
  nextId = 0;
  failure;

  constructor(executable, args, { cwd, env }) {
    if (executable === 'pi') executable = resolveTestPi();
    this.child = spawn(executable, args, { cwd, env, stdio: ['pipe', 'pipe', 'pipe'] });
    this.exited = new Promise(resolveExit => {
      this.child.once('exit', (code, signal) => {
        this.exit = { code, signal };
        this.fail(new Error(`Pi exited (${code ?? signal})${this.stderr ? `: ${this.stderr}` : ''}`));
        resolveExit(this.exit);
      });
      this.child.once('error', error => { this.fail(error); resolveExit({ code: null, signal: null }); });
    });
    const decoder = new StringDecoder('utf8');
    let buffer = '';
    this.child.stdout.on('data', chunk => {
      buffer += decoder.write(chunk);
      let newline;
      while ((newline = buffer.indexOf('\n')) !== -1) {
        const line = buffer.slice(0, newline).replace(/\r$/, '');
        buffer = buffer.slice(newline + 1);
        if (!line) continue;
        if (Buffer.byteLength(line) > MAX_RECORD_BYTES) { this.fail(new Error('RPC record limit exceeded')); return; }
        try {
          const record = JSON.parse(line);
          if (this.records.length >= MAX_RECORDS) throw new Error('RPC test record count limit exceeded');
          this.records.push(record);
          this.lines.push(line);
          this.events.emit('record', record);
        } catch (error) { this.fail(error); return; }
      }
      if (Buffer.byteLength(buffer) > MAX_RECORD_BYTES) this.fail(new Error('RPC unterminated record limit exceeded'));
    });
    this.child.stdout.on('end', () => {
      buffer += decoder.end();
      if (buffer.trim()) this.fail(new Error('RPC stdout ended with an incomplete record'));
    });
    this.child.stdin.on('error', error => this.fail(error));
    this.child.stderr.on('data', chunk => { this.stderr = (this.stderr + chunk.toString('utf8')).slice(-32_768); });
  }

  fail(error) {
    this.failure ??= error;
    this.events.emit('failure', this.failure);
  }

  waitFor(predicate, { after = 0, timeoutMs = 10_000 } = {}) {
    const found = this.records.slice(after).find(predicate);
    if (found) return Promise.resolve(found);
    if (this.failure) return Promise.reject(this.failure);
    return new Promise((resolveRecord, reject) => {
      const cleanup = () => { clearTimeout(timer); this.events.removeListener('record', onRecord); this.events.removeListener('failure', onFailure); };
      const onRecord = record => { if (predicate(record)) { cleanup(); resolveRecord(record); } };
      const onFailure = error => { cleanup(); reject(error); };
      const timer = setTimeout(() => { cleanup(); reject(new Error(`RPC record timeout${this.stderr ? `: ${this.stderr}` : ''}`)); }, timeoutMs);
      this.events.on('record', onRecord);
      this.events.on('failure', onFailure);
    });
  }

  send(type, fields = {}) {
    const id = `fixture-request-${++this.nextId}`;
    const response = this.waitFor(record => record.type === 'response' && record.id === id);
    // Attach immediately so an early process failure cannot become an unhandled rejection.
    void response.catch(() => {});
    this.child.stdin.write(`${JSON.stringify({ ...fields, type, id })}\n`, error => { if (error) this.fail(error); });
    return { id, response };
  }

  async command(type, fields = {}) {
    return this.send(type, fields).response;
  }

  async close() {
    if (this.exit) return;
    this.child.stdin.end();
    const terminate = setTimeout(() => this.child.kill('SIGTERM'), 2_000);
    const kill = setTimeout(() => this.child.kill('SIGKILL'), 3_000);
    try { await this.exited; }
    finally { clearTimeout(terminate); clearTimeout(kill); }
  }
}

/** All configuration, HOME, cwd, and session files live under this package's ignored test directory. */
export async function createTestSession({ baseUrl, extensionPath, extraExtensions = [], reasoning = false, settings = {}, persistent = false, sessionId = 'fixture-session', executable = 'pi', pathPrefix,
  model = `${PROVIDER}/${MODEL}`, extraArgs = [], extraProviders = {}, extraEnv = {}, modelIds = [MODEL, 'fixture-alternate'], ownedLayout = false }) {
  const url = new URL(baseUrl);
  if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || url.pathname !== '/v1') {
    throw new Error('Tests only accept a loopback fake-provider baseUrl');
  }
  const temporaryRoot = fileURLToPath(new URL('../.tmp/', import.meta.url));
  await mkdir(temporaryRoot, { recursive: true, mode: 0o700 });
  const sandbox = await mkdtemp(join(temporaryRoot, 'rpc-'));
  const home = join(sandbox, 'home');
  const directory = ownedLayout ? join(home, 'Kota/providers/kota-local') : sandbox;
  const configDirectory = join(directory, 'pi-agent');
  const cwd = join(sandbox, 'workspace');
  const temporary = join(sandbox, 'tmp');
  const sessions = join(directory, 'sessions');
  for (const path of [configDirectory, cwd, home, temporary, sessions]) await mkdir(path, { recursive: true, mode: 0o700 });
  await writeFile(join(cwd, 'fixture.txt'), 'Harmless Kota Local read fixture.\n');
  await writeFile(join(configDirectory, 'models.json'), JSON.stringify({
    providers: { [PROVIDER]: { baseUrl, api: 'openai-completions', apiKey: 'fixture-not-a-secret', models: modelIds.map(id => ({
      id, name: 'Kota test fixture', reasoning, input: ['text', 'image'],
      contextWindow: 16_384, maxTokens: 2_048, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    })) }, ...extraProviders },
  }), { mode: 0o600 });
  await writeFile(join(configDirectory, 'settings.json'), JSON.stringify({
    steeringMode: 'all', followUpMode: 'all', cacheWarming: 'off',
    enableInstallTelemetry: false, enableAnalytics: false, defaultTools: ['read', 'bash'],
    retry: { enabled: false }, compaction: { enabled: false, keepRecentTokens: 256, reserveTokens: 1_024 },
    ...settings,
  }), { mode: 0o600 });
  const env = {
    PATH: pathPrefix ? `${resolve(pathPrefix)}${process.platform === 'win32' ? ';' : ':'}${process.env.PATH}` : process.env.PATH, HOME: home, USERPROFILE: home, TMPDIR: temporary, TEMP: temporary, TMP: temporary,
    PI_CODING_AGENT_DIR: configDirectory, PI_CODING_AGENT_SESSION_DIR: sessions,
    PI_OFFLINE: '1', PI_SKIP_VERSION_CHECK: '1', PI_TELEMETRY: '0', NO_COLOR: '1', ...extraEnv,
  };
  for (const key of ['SystemRoot', 'WINDIR']) if (process.env[key]) env[key] = process.env[key];
  // Do not inherit provider keys, OAuth tokens, proxies, or the real project's KOTA_* environment.
  const args = ['--mode', 'rpc', '--no-approve', '-ne', '-ns', '-np', '--no-themes', '--no-context-files', '--offline'];
  if (extensionPath) args.push('-e', resolve(extensionPath));
  for (const path of extraExtensions) args.push('-e', resolve(path));
  if (persistent) args.push('--session-id', sessionId, '--session-dir', sessions);
  else args.push('--no-session');
  args.push('--model', model, '--thinking', 'off', ...extraArgs);
  const client = new TestRpcClient(executable, args, { cwd, env });
  try {
    const state = await client.command('get_state');
    if (!state.success || `${state.data.model?.provider}/${state.data.model?.id}` !== model) throw new Error('Fake-provider session did not become ready');
  } catch (error) {
    await client.close();
    await rm(sandbox, { recursive: true, force: true });
    throw error;
  }
  return {
    client, directory, configDirectory, cwd, env, args,
    async close() { try { await client.close(); } finally { await rm(sandbox, { recursive: true, force: true }); } },
  };
}

async function main() {
  let baseUrl;
  let extensionPath = fileURLToPath(new URL('../../index.ts', import.meta.url));
  for (let index = 2; index < process.argv.length; index++) {
    const option = process.argv[index];
    if (option === '--base-url') baseUrl = process.argv[++index];
    else if (option === '--extension') extensionPath = process.argv[++index];
    else throw new Error(`Unknown option: ${option}`);
  }
  if (!baseUrl) throw new Error('Pass --base-url http://127.0.0.1:<port>/v1');
  const session = await createTestSession({ baseUrl, extensionPath });
  try {
    const input = {
      kotaId: 'msg-smoke#agent-fixture', customType: 'kota.user', content: 'Synthetic smoke input.',
      details: { schema: 1, messageId: 'msg-smoke', targetAgentId: 'agent-fixture', createdAt: '2026-10-02T00:00:00Z', private: false },
    };
    const after = session.client.records.length;
    const response = await session.client.command('prompt', { message: `/kota-inject ${JSON.stringify(input)}` });
    assert.equal(response.success, true);
    await session.client.waitFor(record => record.type === 'agent_start', { after });
    await session.client.waitFor(record => record.type === 'message_end' && record.message.role === 'custom' && record.message.details?.kotaId === input.kotaId, { after });
    await session.client.waitFor(record => record.type === 'agent_settled', { after });
    assert.equal(session.client.records.some(record => record.type === 'message_end' && record.message.role === 'assistant' && record.message.stopReason === 'error'), false);
    process.stdout.write(`${JSON.stringify({ type: 'passed', kotaId: input.kotaId, records: session.client.records.length })}\n`);
  } finally { await session.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
