/** Upgrade witnesses executed by the pinned standalone, never a Node SDK. */
import assert from 'node:assert/strict';
import { createModels, InMemoryCredentialStore, type CredentialStore, type OAuthCredential } from '@earendil-works/pi-ai';
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';

export default function upgradeWitness(pi: ExtensionAPI) {
  pi.registerCommand('test-pi-upgrade', { description: 'Synthetic upgrade witness', handler: async (_args, ctx) => {
    assert.equal(process.env.KOTA_PI_TEST_DOTENV, undefined);
    const memory = new InMemoryCredentialStore();
    let committed!: () => void;
    const written = new Promise<void>(resolve => { committed = resolve; });
    const credentials: CredentialStore = {
      read: (id, options) => memory.read(id, options), list: options => memory.list(options),
      delete: (id, options) => memory.delete(id, options),
      async modify(id, update, options) {
        const result = await memory.modify(id, update, options);
        if (result?.type === 'oauth' && result.refresh === 'fake-new-refresh') committed();
        return result;
      },
    };
    const id = 'synthetic-refresh';
    const expired: OAuthCredential = { type: 'oauth', access: 'fake-old-access', refresh: 'fake-old-refresh', expires: 1 };
    await credentials.modify(id, async () => expired);
    let started!: () => void, release!: () => void;
    const entered = new Promise<void>(resolve => { started = resolve; });
    const proceed = new Promise<void>(resolve => { release = resolve; });
    const cancel = new AbortController();
    let refreshes = 0;
    const models = createModels({ credentials, authContext: { env: async () => 'fake-env-key', fileExists: async () => false } });
    models.setProvider({ id, name: 'Synthetic refresh', getModels: () => [],
      stream() { throw new Error('No model requests'); }, streamSimple() { throw new Error('No model requests'); },
      auth: { oauth: { name: 'Synthetic OAuth', async login() { throw new Error('No logins'); },
        async refresh(_credential, signal) {
          refreshes++;
          started();
          await proceed;
          assert(!signal?.aborted, 'Cancellation must not discard an already-rotated token.');
          return { type: 'oauth', access: 'fake-new-access', refresh: 'fake-new-refresh', expires: Date.now() + 3_600_000 };
        }, toAuth: async credential => ({ apiKey: credential.access }),
      } },
    });
    const resolving = models.getAuth(id, { signal: cancel.signal });
    await entered;
    cancel.abort();
    release();
    await resolving.catch(() => undefined); // A caller may be cancelled; persistence must finish.
    await written; // Cancellation returns before the detached native refresh finishes writing.
    const saved = await credentials.read(id);
    assert.equal(saved?.type, 'oauth');
    assert.equal((saved as OAuthCredential).refresh, 'fake-new-refresh');
    const next = await models.getAuth(id);
    assert.equal(next?.auth.apiKey, 'fake-new-access');
    assert.equal(refreshes, 1);
    ctx.ui.notify('test.pi-upgrade:' + JSON.stringify({ dotenvIgnored: true, rotatedTokenSaved: true,
      nextRequestUsesSavedToken: true, refreshes, networkRequests: 0 }), 'info');
  } });
}
