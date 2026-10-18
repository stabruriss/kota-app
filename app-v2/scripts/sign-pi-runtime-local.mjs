import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {readPiRuntimePin, validatePiRuntimeTree, piMachOFiles} from './prepare-pi-runtime.mjs';

const sha256 = file => createHash('sha256').update(fs.readFileSync(file)).digest('hex');

// Call after bundling and before the existing outer App seal. Never signs with Developer ID.
export function signPiRuntimeLocal(bundle, {run = execFileSync} = {}) {
  if (process.platform !== 'darwin') throw new Error('Local Pi signing requires macOS.');
  const app = path.resolve(bundle);
  if (!app.endsWith('.app') || !fs.lstatSync(app).isDirectory()
    || !fs.existsSync(path.join(app, 'Contents/Info.plist'))) throw new Error('Expected a local debug .app bundle.');
  const runtime = path.join(app, 'Contents/Resources/kota-pi');
  const {pin} = readPiRuntimePin();
  validatePiRuntimeTree(runtime, pin);
  const signatures = [];
  for (const relative of piMachOFiles) {
    const file = path.join(runtime, relative);
    const inputSha256 = sha256(file);
    run('/usr/bin/codesign', ['--force', '--sign', '-', '--timestamp=none', file], {stdio: 'pipe'});
    run('/usr/bin/codesign', ['--verify', '--strict', '--verbose=2', file], {stdio: 'pipe'});
    signatures.push({file: relative, inputSha256, signedSha256: sha256(file)});
  }
  validatePiRuntimeTree(runtime, pin, {signedMachO: true});
  return {app, identity: 'adhoc', entitlementsAdded: false, signatures, outerAppSeal: 'required after this step'};
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3) throw new Error('Usage: node scripts/sign-pi-runtime-local.mjs /path/to/Local.app');
    console.log(JSON.stringify(signPiRuntimeLocal(process.argv[2]), null, 2));
  } catch (error) { console.error(`Local Pi signing failed: ${error.message}`); process.exitCode = 1; }
}
