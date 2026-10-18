import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, copyFile, writeFile, chmod, rm } from 'node:fs/promises';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { resolveTestPi } from './rpc-client.mjs';

test('native RPC requires an explicit absolute pinned Pi staging root', () => {
  assert.throws(() => resolveTestPi(''), /absolute/);
  assert.throws(() => resolveTestPi('relative'), /absolute/);
  assert.equal(resolveTestPi(), join(process.env.KOTA_LOCAL_TEST_PI_RUNTIME_ROOT, 'pi'));
});

test('wrong manifests and missing adjacent resources fail without running a CLI', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kota-pi-test-resolver-'));
  try {
    await writeFile(join(root, 'VENDOR-MANIFEST.json'), '{}');
    assert.throws(() => resolveTestPi(root), /pin/);
    await copyFile(join(dirname(resolveTestPi()), 'VENDOR-MANIFEST.json'), join(root, 'VENDOR-MANIFEST.json'));
    assert.throws(() => resolveTestPi(root), /ENOENT/);
    for (const name of ['pi', 'photon_rs_bg.wasm', 'native/darwin/prebuilds/darwin-arm64/darwin-platform.node', 'package.json']) {
      const path = join(root, name);
      await mkdir(dirname(path), { recursive: true });
      await writeFile(path, 'not executed');
    }
    assert.throws(() => resolveTestPi(root), /entrypoint/);
    await chmod(join(root, 'pi'), 0o700);
    assert.equal(resolveTestPi(root), join(root, 'pi'));
  } finally { await rm(root, { recursive: true, force: true }); }
});
