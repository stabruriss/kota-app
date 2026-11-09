import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {create} from 'tar';
import ts from 'typescript';
import {appRoot, preparePiRuntime, readPiRuntimePin, validatePiRuntimeTree, piMachOFiles} from '../prepare-pi-runtime.mjs';
import {signPiRuntimeLocal} from '../sign-pi-runtime-local.mjs';

const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const writeJson = (file, value) => fs.writeFileSync(file, JSON.stringify(value, null, 2) + '\n');
const mac = {platform: 'darwin', arch: 'arm64'};

async function fixture(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pi-package-test-'));
  t.after(() => fs.rmSync(root, {recursive: true, force: true}));
  const vendor = path.join(root, 'vendor/pi-runtime');
  const inputs = path.join(root, 'input');
  const payload = path.join(inputs, 'kota-pi');
  fs.mkdirSync(vendor, {recursive: true});
  fs.mkdirSync(payload, {recursive: true});
  const data = new Map([
    ['pi', 'fixture executable'],
    ['photon_rs_bg.wasm', 'fixture wasm'],
    [piMachOFiles[1], 'fixture native module'],
    ['third-party/NOTICE.txt', 'fixture notice'],
  ]);
  const preserved = [];
  for (const [name, bytes] of data) {
    const file = path.join(payload, name);
    fs.mkdirSync(path.dirname(file), {recursive: true});
    fs.writeFileSync(file, bytes);
    fs.chmodSync(file, name === 'pi' ? 0o755 : 0o644);
    if (!name.startsWith('third-party/')) preserved.push({path: name, sha256: hash(bytes), bytes: Buffer.byteLength(bytes), mode: name === 'pi' ? '0o755' : '0o644'});
  }
  const manifest = {version: '9.9.9', vendor_revision: 'fixture', platform: 'darwin-arm64', preserved_upstream_files: preserved, added_notices: {'third-party/NOTICE.txt': hash('fixture notice')}};
  writeJson(path.join(payload, 'VENDOR-MANIFEST.json'), manifest);
  fs.chmodSync(path.join(payload, 'VENDOR-MANIFEST.json'), 0o644);
  fs.cpSync(path.join(payload, 'third-party'), path.join(vendor, 'third-party'), {recursive: true});
  const pin = {schema_version: 1, runtime: 'Pi', version: manifest.version, vendor_revision: manifest.vendor_revision,
    platform: manifest.platform, archive: 'pi-runtime-fixture.tar.gz', archive_root: 'kota-pi',
    vendor_manifest_sha256: hash(fs.readFileSync(path.join(payload, 'VENDOR-MANIFEST.json'))),
    payload_files: data.size + 1, payload_size_bytes: [...data.values()].reduce((sum, value) => sum + Buffer.byteLength(value), fs.statSync(path.join(payload, 'VENDOR-MANIFEST.json')).size),
    notices: manifest.added_notices, provenance: 'pi-runtime-fixture.provenance.json'};
  async function pack(options = {}, files = ['kota-pi']) {
    await create({cwd: inputs, file: path.join(vendor, pin.archive), gzip: true, portable: true, ...options}, files);
    const archive = fs.readFileSync(path.join(vendor, pin.archive));
    pin.sha256 = hash(archive); pin.size_bytes = archive.length;
    writeJson(path.join(vendor, 'pi-runtime.lock.json'), pin);
    fs.writeFileSync(path.join(vendor, pin.archive + '.sha256'), `${pin.sha256}  ${pin.archive}\n`);
    writeJson(path.join(vendor, pin.provenance), {asset: pin.archive, sha256: pin.sha256, size_bytes: pin.size_bytes,
      vendor_revision: pin.vendor_revision, vendor_manifest_sha256: pin.vendor_manifest_sha256});
  }
  await pack();
  return {root, vendor, inputs, payload, pin, pack, staging: path.join(root, 'src-tauri/pi-runtime/darwin-arm64')};
}

test('preparation stages local pinned files and repairs stale generated output', async t => {
  const f = await fixture(t);
  const result = await preparePiRuntime({root: f.root, ...mac});
  assert.equal(result.files, 5);
  assert.equal(fs.readFileSync(path.join(result.staging, 'pi'), 'utf8'), 'fixture executable');
  assert.equal(fs.statSync(path.join(result.staging, 'pi')).mode & 0o777, 0o755);
  fs.writeFileSync(path.join(result.staging, 'stale'), 'old generated file');
  await preparePiRuntime({root: f.root, ...mac});
  assert.equal(fs.existsSync(path.join(result.staging, 'stale')), false);
  assert.deepEqual(fs.readdirSync(path.dirname(result.staging)), ['darwin-arm64']);
});

test('missing archive fails even when old staging exists', async t => {
  const f = await fixture(t);
  await preparePiRuntime({root: f.root, ...mac});
  fs.unlinkSync(path.join(f.vendor, f.pin.archive));
  await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /ENOENT/);
});

test('wrong archive bytes fail without replacing old staging', async t => {
  const f = await fixture(t);
  await preparePiRuntime({root: f.root, ...mac});
  fs.writeFileSync(path.join(f.staging, 'old-marker'), 'keep on failed build');
  const archive = path.join(f.vendor, f.pin.archive);
  const bytes = fs.readFileSync(archive); bytes[bytes.length - 1] ^= 1; fs.writeFileSync(archive, bytes);
  await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /SHA-256\/size mismatch/);
  assert.equal(fs.readFileSync(path.join(f.staging, 'old-marker'), 'utf8'), 'keep on failed build');
});

for (const target of ['checksum', 'provenance', 'notice']) {
  test(`adjacent ${target} drift blocks preparation`, async t => {
    const f = await fixture(t);
    const file = target === 'checksum' ? f.pin.archive + '.sha256' : target === 'provenance' ? f.pin.provenance : 'third-party/NOTICE.txt';
    fs.writeFileSync(path.join(f.vendor, file), target === 'provenance' ? '{}' : 'changed');
    await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /checksum file mismatch|provenance mismatch|Adjacent Pi notice mismatch/);
    assert.equal(fs.existsSync(f.staging), false);
  });
}

test('unsafe tar path is rejected before any file extraction', async t => {
  const f = await fixture(t);
  await f.pack({onWriteEntry(entry) { if (entry.path === 'kota-pi/pi') entry.path = 'kota-pi/../../outside.txt'; }});
  await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /Unsafe Pi tar member/);
  assert.equal(fs.existsSync(path.join(f.root, 'outside.txt')), false);
  assert.deepEqual(fs.readdirSync(path.dirname(f.staging)), []);
});

test('duplicate tar paths and symlinks are rejected', async t => {
  const f = await fixture(t);
  await f.pack({}, ['kota-pi', 'kota-pi/pi']);
  await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /Unsafe Pi tar member/);
  fs.unlinkSync(path.join(f.payload, 'pi'));
  fs.symlinkSync('../../outside', path.join(f.payload, 'pi'));
  await f.pack();
  await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /Unsafe Pi tar member/);
});

test('payload hash mismatch blocks replacement and removes temporary files', async t => {
  const f = await fixture(t);
  fs.writeFileSync(path.join(f.payload, 'photon_rs_bg.wasm'), 'bad  wasm---');
  await f.pack();
  f.pin.payload_size_bytes += Buffer.byteLength('bad  wasm---') - Buffer.byteLength('fixture wasm');
  writeJson(path.join(f.vendor, 'pi-runtime.lock.json'), f.pin);
  await assert.rejects(preparePiRuntime({root: f.root, ...mac}), /file SHA-256 mismatch/);
  assert.equal(fs.existsSync(f.staging), false);
  assert.deepEqual(fs.readdirSync(path.dirname(f.staging)), []);
});

test('unsupported build architecture fails explicitly', async t => {
  const f = await fixture(t);
  await assert.rejects(preparePiRuntime({root: f.root, platform: 'darwin', arch: 'x64'}), /macOS arm64 only/);
});

test('Tauri hooks preserve CLI order and explicit runtime resource mappings', () => {
  const config = JSON.parse(fs.readFileSync(path.join(appRoot, 'src-tauri/tauri.conf.json'), 'utf8'));
  assert.equal(config.build.beforeDevCommand, 'npm run prepare:pi-runtime && npm run build:sidecars:dev && npm run dev');
  assert.equal(config.build.beforeBuildCommand, 'npm run prepare:pi-runtime && npm run build && npm run build:sidecars');
  assert.equal(config.bundle.resources['pi-runtime/darwin-arm64/'], 'kota-pi/');
  assert.deepEqual(config.bundle.externalBin, ['bin/kota-bbs', 'bin/kota-agent-bus', 'bin/kota-bartender', 'bin/kota-ember', 'bin/kota-my-soul', 'bin/kota-dream']);
  const extensionResources = Object.fromEntries(Object.entries(config.bundle.resources)
    .filter(([source, destination]) => source.startsWith('../kota-ext') || destination.startsWith('kota-ext')));
  assert.deepEqual(extensionResources, {
    '../kota-ext/index.ts': 'kota-ext/index.ts',
    '../kota-ext/history.ts': 'kota-ext/history.ts',
    '../kota-ext/bridge.ts': 'kota-ext/bridge.ts',
    '../kota-ext/inject.ts': 'kota-ext/inject.ts',
    '../kota-ext/tools.ts': 'kota-ext/tools.ts',
    '../kota-ext/tool-command.ts': 'kota-ext/tool-command.ts',
    '../kota-ext/cli.ts': 'kota-ext/cli.ts',
    '../kota-ext/auth.ts': 'kota-ext/auth.ts',
    '../kota-ext/auth-protocol.ts': 'kota-ext/auth-protocol.ts',
    '../kota-ext/auto-route.ts': 'kota-ext/auto-route.ts',
    '../kota-ext/auto-route-core.ts': 'kota-ext/auto-route-core.ts',
    '../kota-ext/jev.ts': 'kota-ext/jev.ts',
    '../kota-ext/puppeteer-prompts.json': 'kota-ext/puppeteer-prompts.json',
    '../kota-ext/puppeteer-prompts-core.ts': 'kota-ext/puppeteer-prompts-core.ts',
    '../kota-ext/puppeteer-prompts.ts': 'kota-ext/puppeteer-prompts.ts',
    '../kota-ext/capabilities.ts': 'kota-ext/capabilities.ts',
    '../kota-ext/mcp-status.ts': 'kota-ext/mcp-status.ts',
    '../kota-ext/shadow-clone.ts': 'kota-ext/shadow-clone.ts',
    '../kota-ext/package.json': 'kota-ext/package.json',
  });
});

test('every relative extension import is included at its bundled path', () => {
  const configDirectory = path.join(appRoot, 'src-tauri');
  const config = JSON.parse(fs.readFileSync(path.join(configDirectory, 'tauri.conf.json'), 'utf8'));
  const resources = new Map(Object.entries(config.bundle.resources)
    .map(([source, destination]) => [path.resolve(configDirectory, source), destination]));
  const seen = new Set();
  function visit(file, bundledPath) {
    assert.equal(resources.get(file), bundledPath,
      'Missing or misplaced extension resource: ' + path.relative(appRoot, file));
    if (seen.has(file)) return;
    seen.add(file);
    if (path.extname(file) === '.json') return;
    for (const {fileName} of ts.preProcessFile(fs.readFileSync(file, 'utf8'), true, true).importedFiles) {
      if (!fileName.startsWith('.')) continue;
      visit(path.resolve(path.dirname(file), fileName),
        path.posix.normalize(path.posix.join(path.posix.dirname(bundledPath), fileName)));
    }
  }
  visit(path.join(appRoot, 'kota-ext/index.ts'), 'kota-ext/index.ts');
});

test('real Pi 1.3 r1 stages identically; local signing selects only two inner files', {skip: process.platform !== 'darwin' || process.arch !== 'arm64'}, async t => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'pi-real-bundle-test-'));
  t.after(() => fs.rmSync(temp, {recursive: true, force: true}));
  const result = await preparePiRuntime();
  assert.equal(result.files, 209);
  assert.equal(result.archiveSha256, '81daeb9443e37844393c4a1e5b33322aa81528e6498740eedba4681b980592ce');
  const bundle = path.join(temp, 'Local.app');
  fs.mkdirSync(path.join(bundle, 'Contents/Resources'), {recursive: true});
  fs.writeFileSync(path.join(bundle, 'Contents/Info.plist'), 'fixture');
  fs.cpSync(result.staging, path.join(bundle, 'Contents/Resources/kota-pi'), {recursive: true});
  const commands = [];
  const signed = signPiRuntimeLocal(bundle, {run(command, args) { commands.push({command, args}); }});
  assert.equal(commands.length, 4);
  for (const [index, relative] of piMachOFiles.entries()) {
    const file = path.join(bundle, 'Contents/Resources/kota-pi', relative);
    assert.deepEqual(commands[index * 2], {command: '/usr/bin/codesign', args: ['--force', '--sign', '-', '--timestamp=none', file]});
    assert.deepEqual(commands[index * 2 + 1].args, ['--verify', '--strict', '--verbose=2', file]);
  }
  assert.equal(signed.entitlementsAdded, false);
  assert.equal(signed.outerAppSeal, 'required after this step');
  const {pin} = readPiRuntimePin();
  validatePiRuntimeTree(result.staging, pin);
  const ignored = execFileSync('git', ['-C', path.dirname(appRoot), 'check-ignore', '--', path.relative(path.dirname(appRoot), path.join(result.staging, 'pi'))], {encoding: 'utf8'});
  assert.match(ignored, /src-tauri\/pi-runtime\/darwin-arm64\/pi/);
});
