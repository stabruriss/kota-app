import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {fileURLToPath} from 'node:url';
import {list, extract} from 'tar';

export const appRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const piMachOFiles = ['pi', 'native/darwin/prebuilds/darwin-arm64/darwin-platform.node'];
const sha256 = data => createHash('sha256').update(data).digest('hex');
const json = file => JSON.parse(fs.readFileSync(file, 'utf8'));
const requireCondition = (condition, message) => { if (!condition) throw new Error(message); };

function fileBytes(file) {
  requireCondition(fs.lstatSync(file).isFile(), `Pi runtime input is not a regular file: ${file}`);
  return fs.readFileSync(file);
}

function safeRelative(value) {
  return typeof value === 'string' && value.length > 0 && !path.posix.isAbsolute(value)
    && !/[\\\x00-\x1f\x7f]/.test(value)
    && value.split('/').every(part => part && part !== '.' && part !== '..');
}

export function readPiRuntimePin(root = appRoot) {
  const vendor = path.join(root, 'vendor/pi-runtime');
  const pin = json(path.join(vendor, 'pi-runtime.lock.json'));
  requireCondition(pin.schema_version === 1 && pin.runtime === 'Pi' && pin.platform === 'darwin-arm64', 'Unsupported Pi runtime pin.');
  requireCondition(safeRelative(pin.archive) && !pin.archive.includes('/') && pin.archive_root === 'kota-pi', 'Invalid Pi archive path/root.');
  requireCondition(safeRelative(pin.provenance) && !pin.provenance.includes('/'), 'Invalid Pi provenance path.');
  for (const field of ['sha256', 'vendor_manifest_sha256']) {
    requireCondition(/^[a-f0-9]{64}$/.test(pin[field]), `Invalid Pi pin ${field}.`);
  }
  for (const field of ['size_bytes', 'payload_files', 'payload_size_bytes']) {
    requireCondition(Number.isSafeInteger(pin[field]) && pin[field] > 0, `Invalid Pi pin ${field}.`);
  }
  requireCondition(pin.notices && Object.keys(pin.notices).length > 0, 'Pi notices are missing from the pin.');
  return {vendor, pin};
}

function runtimeFiles(root, relative = '') {
  const files = [];
  requireCondition(fs.lstatSync(path.join(root, relative)).isDirectory(), 'Pi runtime root is not a real directory.');
  for (const entry of fs.readdirSync(path.join(root, relative), {withFileTypes: true})) {
    const name = path.posix.join(relative, entry.name);
    if (entry.isDirectory()) files.push(...runtimeFiles(root, name));
    else {
      requireCondition(entry.isFile(), `Unsupported Pi runtime file type: ${name}`);
      files.push(name);
    }
  }
  return files;
}

// Signing changes only the two authorized Mach-O files. Data and file modes remain pinned.
export function validatePiRuntimeTree(root, pin, {signedMachO = false} = {}) {
  const manifestBytes = fileBytes(path.join(root, 'VENDOR-MANIFEST.json'));
  requireCondition(sha256(manifestBytes) === pin.vendor_manifest_sha256, 'Pi vendor manifest SHA-256 mismatch.');
  const manifest = JSON.parse(manifestBytes);
  requireCondition(manifest.version === pin.version && manifest.vendor_revision === pin.vendor_revision
    && manifest.platform === pin.platform, 'Pi vendor version/platform mismatch.');
  requireCondition(JSON.stringify(Object.entries(manifest.added_notices).sort()) === JSON.stringify(Object.entries(pin.notices).sort()), 'Pi notice inventory mismatch.');
  const expected = new Map([['VENDOR-MANIFEST.json', {sha256: pin.vendor_manifest_sha256, mode: '0o644'}]]);
  for (const entry of manifest.preserved_upstream_files) expected.set(entry.path, entry);
  for (const [name, hash] of Object.entries(pin.notices)) expected.set(name, {sha256: hash, mode: '0o644'});
  for (const name of expected.keys()) requireCondition(safeRelative(name), `Invalid Pi inventory path: ${name}`);
  const files = runtimeFiles(root);
  requireCondition(expected.size === pin.payload_files && files.length === expected.size, 'Pi runtime file count mismatch.');
  let totalBytes = 0;
  for (const name of files) {
    const entry = expected.get(name);
    requireCondition(entry, `Unexpected Pi runtime file: ${name}`);
    const file = path.join(root, name);
    const stat = fs.statSync(file);
    totalBytes += stat.size;
    requireCondition((stat.mode & 0o777) === Number(entry.mode), `Pi runtime file mode mismatch: ${name}`);
    if (!signedMachO || !piMachOFiles.includes(name)) {
      requireCondition(sha256(fileBytes(file)) === entry.sha256, `Pi runtime file SHA-256 mismatch: ${name}`);
    }
  }
  if (!signedMachO) requireCondition(totalBytes === pin.payload_size_bytes, 'Pi runtime payload size mismatch.');
  for (const name of [...piMachOFiles, 'photon_rs_bg.wasm']) requireCondition(expected.has(name), `Missing Pi resource: ${name}`);
  fs.accessSync(path.join(root, 'pi'), fs.constants.X_OK);
  return {files: files.length, bytes: totalBytes};
}

function checkAdjacentMetadata(vendor, pin) {
  const checksum = fileBytes(path.join(vendor, pin.archive + '.sha256')).toString('utf8').trim();
  requireCondition(checksum === `${pin.sha256}  ${pin.archive}`, 'Pi archive checksum file mismatch.');
  const provenance = json(path.join(vendor, pin.provenance));
  requireCondition(provenance.sha256 === pin.sha256 && provenance.size_bytes === pin.size_bytes
    && provenance.asset === pin.archive && provenance.vendor_revision === pin.vendor_revision
    && provenance.vendor_manifest_sha256 === pin.vendor_manifest_sha256, 'Pi archive provenance mismatch.');
  for (const [name, hash] of Object.entries(pin.notices)) {
    requireCondition(safeRelative(name) && name.startsWith('third-party/'), 'Invalid Pi notice path.');
    requireCondition(sha256(fileBytes(path.join(vendor, name))) === hash, `Adjacent Pi notice mismatch: ${name}`);
  }
}

async function unpackRuntime(archive, destination, pin) {
  const seen = new Set();
  let invalid;
  let files = 0;
  let bytes = 0;
  await list({file: archive, strict: true, onReadEntry(entry) {
    const name = entry.path.replace(/\/$/, '');
    if (!safeRelative(name) || (name !== pin.archive_root && !name.startsWith(pin.archive_root + '/'))
      || seen.has(name) || !['File', 'Directory'].includes(entry.type)
      || !Number.isSafeInteger(entry.size) || entry.size < 0) invalid = `Unsafe Pi tar member: ${entry.path}`;
    seen.add(name);
    if (entry.type === 'File') { files++; bytes += entry.size; }
  }});
  requireCondition(!invalid, invalid);
  requireCondition(files === pin.payload_files && bytes === pin.payload_size_bytes, 'Pi archive inventory size mismatch.');
  await extract({file: archive, cwd: destination, strip: 1, strict: true, chmod: true,
    preserveOwner: false, noMtime: true, filter: (_name, entry) => ['File', 'Directory'].includes(entry.type)});
}

export async function preparePiRuntime({root = appRoot, platform = process.platform, arch = process.arch} = {}) {
  requireCondition(platform === 'darwin' && arch === 'arm64', 'Pinned Pi runtime supports macOS arm64 only.');
  const {vendor, pin} = readPiRuntimePin(root);
  const archive = fileBytes(path.join(vendor, pin.archive));
  requireCondition(archive.length === pin.size_bytes && sha256(archive) === pin.sha256, 'Pi archive SHA-256/size mismatch.');
  checkAdjacentMetadata(vendor, pin);
  const generated = path.join(root, 'src-tauri/pi-runtime');
  fs.mkdirSync(generated, {recursive: true});
  requireCondition(fs.lstatSync(generated).isDirectory(), 'Pi staging root must be a real directory.');
  const temporary = fs.mkdtempSync(path.join(generated, '.prepare-'));
  const staging = path.join(generated, pin.platform);
  const backup = path.join(temporary, 'previous');
  try {
    const archiveCopy = path.join(temporary, 'input.tar.gz');
    fs.writeFileSync(archiveCopy, archive, {flag: 'wx'});
    const payload = path.join(temporary, 'payload');
    fs.mkdirSync(payload);
    await unpackRuntime(archiveCopy, payload, pin);
    const inventory = validatePiRuntimeTree(payload, pin);
    if (fs.existsSync(staging)) fs.renameSync(staging, backup);
    try { fs.renameSync(payload, staging); }
    catch (error) { if (fs.existsSync(backup)) fs.renameSync(backup, staging); throw error; }
    return {version: pin.version, revision: pin.vendor_revision, platform: pin.platform, archiveSha256: pin.sha256, staging, ...inventory};
  } finally { fs.rmSync(temporary, {recursive: true, force: true}); }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    requireCondition(process.argv.length === 2, 'Usage: node scripts/prepare-pi-runtime.mjs');
    console.log(JSON.stringify(await preparePiRuntime(), null, 2));
  } catch (error) { console.error(`Pi runtime preparation failed: ${error.message}`); process.exitCode = 1; }
}
