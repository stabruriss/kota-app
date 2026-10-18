/** Node-only SDK witnesses. Native RPC tests use resolveTestPi instead. */
import { readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';

export function registerPinnedSdkHooks() {
  const pin = JSON.parse(readFileSync(new URL('../../vendor/pi-runtime/pi-runtime.lock.json', import.meta.url), 'utf8'));
  const localPackages = new URL('../../node_modules/', import.meta.url).href;
  const entries = new Map();
  for (const name of ['@earendil-works/pi-coding-agent', '@earendil-works/pi-ai']) {
    const entry = new URL(import.meta.resolve(name));
    if (!entry.href.startsWith(localPackages)) throw new Error(`Test SDK ${name} must be installed in app-v2; run npm ci there.`);
    const manifest = JSON.parse(readFileSync(new URL('../package.json', entry), 'utf8'));
    if (manifest.version !== pin.version) throw new Error(`Test SDK ${name} must match the bundled Pi ${pin.version}; run npm ci in app-v2.`);
    entries.set(name, entry.href);
  }
  registerHooks({ resolve(specifier, context, next) {
    const url = entries.get(specifier);
    return url ? { url, shortCircuit: true } : next(specifier, context);
  } });
}
