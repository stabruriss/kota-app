// Test-only catalog parity receipt for the Rust direct Dance client; no classifier call.
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { JEV_MODELS, JEV_PROVIDERS } from '../../auto-route-core.ts';
export default function (pi: ExtensionAPI) {
  pi.registerCommand('jev-catalog-probe', {
    handler: async (_raw, ctx) => {
      const registry = ctx.modelRegistry as unknown as { findOfType(type: string, provider: string, id: string): unknown };
      ctx.ui.notify('jev-catalog-probe:' + JSON.stringify(JEV_PROVIDERS.map(provider => registry.findOfType('classifier', provider, JEV_MODELS[provider]))), 'info');
    },
  });
}
