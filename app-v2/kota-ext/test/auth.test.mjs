import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, readFile, writeFile, rm } from 'node:fs/promises';
import { realpathSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

test('auth controller fake-provider suite is isolated, private and does not run models', { timeout: 75_000 }, async t => {
  const dir = realpathSync(await mkdtemp('/tmp/kota-auth-unit-'));
  const home = join(dir, 'home');
  const agent = join(dir, 'agent');
  const sentinelDir = join(dir, 'simulated-personal/.pi/agent');
  try {
    for (const path of [home, agent, sentinelDir]) await mkdir(path, { recursive: true, mode: 0o700 });
    const sentinel = join(sentinelDir, 'auth.json');
    await writeFile(sentinel, '{"synthetic-sentinel":true}\n', { mode: 0o600 });
    const before = await readFile(sentinel);
    const worker = fileURLToPath(new URL('./fixtures/auth-unit-worker.mjs', import.meta.url));
    const product = fileURLToPath(new URL('../../../', import.meta.url));
    const policy = `(version 1)(allow default)(deny network*)
(deny file-read* file-write* (subpath ${JSON.stringify(join(homedir(), '.pi'))}))
(deny file-read* file-write* (subpath ${JSON.stringify(join(dir, 'simulated-personal'))}))
(deny file-write* (subpath ${JSON.stringify(product)}))`;
    const env = { PATH: '/usr/bin:/bin:/opt/homebrew/bin', HOME: home, USERPROFILE: home, TMPDIR: dir,
      PI_CODING_AGENT_DIR: agent, PI_OFFLINE: '1', PI_SKIP_VERSION_CHECK: '1', AUTH_UNIT_DIR: dir,
      KOTA_LOCAL_AUTH_HELPER: '1', KOTA_LOCAL_INSTALLATION_UUID: '11111111-2222-4333-8444-555555555555' };
    const receipt = await new Promise((resolve, reject) => {
      const child = spawn('/usr/bin/sandbox-exec', ['-p', policy, process.execPath, worker], { cwd: dir, env, stdio: ['ignore', 'pipe', 'pipe'] });
      let stdout = '', stderr = '';
      const timer = setTimeout(() => { child.kill('SIGKILL'); reject(new Error('Synthetic auth suite timeout')); }, 60_000);
      child.stdout.on('data', data => { stdout += data; }); child.stderr.on('data', data => { stderr += data; });
      child.on('error', error => { clearTimeout(timer); reject(error); });
      child.on('close', code => {
        clearTimeout(timer);
        if (code !== 0 || stderr) reject(new Error(`Synthetic auth suite failed (${code}): ${stderr}`));
        else { try { resolve(JSON.parse(stdout)); } catch (error) { reject(error); } }
      });
    });
    assert(receipt.results.every(result => result.passed));
    assert.equal(receipt.results.length, 34);
    assert.equal(receipt.interceptedTokenRequests, 1);
    assert(receipt.standardNativeHandlers >= 30);
    assert.equal(receipt.realModelOrAuthRequests, 0);
    assert((await readFile(sentinel)).equals(before));
    t.diagnostic(JSON.stringify(receipt));
  } finally { await rm(dir, { recursive: true, force: true }); }
});
