// Test-only: evaluates the fixed Auto Route texts with the bundled pi-ai retry classifier.
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { isRetryableAssistantError } from '@earendil-works/pi-ai';
import { ROUTE_ERROR_TEXT } from '../../auto-route-core.ts';

const retryable = (text: string) => isRetryableAssistantError({ role: 'assistant', stopReason: 'error', errorMessage: text } as never);

export default function (pi: ExtensionAPI) {
  pi.registerCommand('route-copy-probe', {
    handler: async (_args, ctx) => {
      const errors = Object.values(ROUTE_ERROR_TEXT).map(text => ({ text, retryable: retryable(text) }));
      ctx.ui.notify(`route-copy-probe:${JSON.stringify({ errors })}`, 'info');
    },
  });
}
