/** Pinned standalone Pi, expired synthetic OAuth and a loopback blackhole proxy.
 * No OAuth server, personal directory or model is reachable. */
import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, writeFile, readFile, rm, access } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { homedir } from 'node:os';
import { resolveTestPi, TestRpcClient } from './fake-provider/rpc-client.mjs';

const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
const exists = async path => { try { await access(path); return true; } catch { return false; } };

test('clone --offline prevents startup OAuth refresh; native requests retain their own refresh path', { timeout: 30000 }, async t => {
  const sockets = new Set(); let connections = 0;
  const proxy = createServer(socket => {
    connections++; sockets.add(socket); socket.on('close', () => sockets.delete(socket));
    // Deliberately do not parse the CONNECT headers or forward the tunnel.
  });
  await new Promise(resolve => proxy.listen(0, '127.0.0.1', resolve));
  t.after(async () => { for (const socket of sockets) socket.destroy(); await new Promise(resolve => proxy.close(resolve)); });
  const port = proxy.address().port;
  async function run(offline) {
    const root = realpathSync(await mkdtemp('/tmp/kota-clone-startup-'));
    const home = join(root, 'home'), agent = join(root, 'agent'), cwd = join(root, 'cwd'), sessions = join(cwd, 'under-table/clone-1');
    for (const path of [home, agent, cwd, sessions]) await mkdir(path, { recursive: true, mode: 0o700 });
    const sentinel = join(home, '.pi/agent/auth.json'); await mkdir(dirname(sentinel), { recursive: true });
    await writeFile(sentinel, 'synthetic personal sentinel\n');
    const auth = JSON.stringify({ openai: { type: 'oauth', access: 'synthetic-access', refresh: 'synthetic-refresh', expires: 1,
      clientId: 'synthetic-client', scopes: [] } }) + '\n';
    const authPath = join(agent, 'auth.json'); await writeFile(authPath, auth);
    await writeFile(join(agent, 'settings.json'), JSON.stringify({ httpProxy: `http://127.0.0.1:${port}`, cacheWarming: 'off',
      enableInstallTelemetry: false, enableAnalytics: false, retry: { enabled: false } }));
    const policy = `(version 1)(allow default)(deny network*)(allow network-outbound (remote ip "localhost:${port}"))
(deny file-read* file-write* (subpath ${JSON.stringify(join(homedir(), '.pi'))}))
(deny file-read* file-write* (subpath ${JSON.stringify(join(home, '.pi'))}))`;
    const start = connections;
    const client = new TestRpcClient('/usr/bin/sandbox-exec', ['-p', policy, resolveTestPi(), '--mode', 'rpc', '-nc', '-ns', '-ne',
      '--no-approve', '--no-themes', '--session-id', 'clone-1', '--session-dir', sessions, '--tools', 'read,grep,find,ls', ...(offline ? ['--offline'] : [])],
    { cwd, env: { PATH: '/usr/bin:/bin:/opt/homebrew/bin', HOME: home, PI_CODING_AGENT_DIR: agent,
      PI_CODING_AGENT_SESSION_DIR: sessions, TMPDIR: root, PI_SKIP_VERSION_CHECK: '1', NO_COLOR: '1' } });
    try {
      assert.equal((await client.command('get_state')).success, true);
      if (offline) await delay(1500);
      else for (let n = 0; n < 150 && connections === start; n++) await delay(20);
      const result = { connections: connections - start, lock: await exists(join(agent, 'auth.json.lock')) };
      assert.equal(await readFile(authPath, 'utf8'), auth);
      assert.equal(await readFile(sentinel, 'utf8'), 'synthetic personal sentinel\n');
      await client.close();
      assert.equal(await exists(join(agent, 'auth.json.lock')), false, 'SIGTERM removes the native refresh lock');
      return result;
    } finally { await client.close(); await rm(root, { recursive: true, force: true }); }
  }
  const normal = await run(false), clone = await run(true);
  assert(normal.connections > 0, 'positive control: native startup starts a refresh using only the blackhole proxy');
  assert.equal(normal.lock, true);
  assert.deepEqual(clone, { connections: 0, lock: false });
  // --offline only suppresses main.js startup refresh. No prompt is sent here;
  // auth/resolve.js still owns expired-token refresh on a subsequent request.
});
