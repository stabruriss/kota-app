import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';

// Loaded only by the fixture recorder, never by the kota-ext package manifest.
export default function fixtureFault(pi: ExtensionAPI) {
  pi.registerCommand('fixture-fail', {
    handler: async () => { throw new Error('Synthetic fixture extension failure.'); },
  });
}
