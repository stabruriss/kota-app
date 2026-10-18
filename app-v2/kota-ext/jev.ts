import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { JEV_MODELS, JEV_PROVIDERS, JEV_INSTRUCTIONS, DEFAULT_INTENTS, isObject, type JevProvider } from './auto-route-core.ts';

/**
 * Save-time Jev key check, shared by Auto Route and Puppeteer. Only the launcher-marked helper
 * runs it. Rust answers the private key prompt; the key is one request option and is never
 * stored, logged or returned. The input is fixed and carries no user content.
 */
export const JEV_CODEC = 'kota.jev.v1:';
export const JEV_VERIFY_COMMAND = 'kota-jev-verify';
export const JEV_VERIFY_TIMEOUT_MS = 10_000;
const VERIFY_INPUT = 'ok';
type Outcome = 'verified' | 'rejected' | 'unavailable';

export function parseVerifyCommand(raw: string): { operationId: string; providerId: JevProvider } | undefined {
  let value: unknown;
  try { value = JSON.parse(raw); } catch { return undefined; }
  if (!isObject(value) || Object.keys(value).length !== 3 || value.schema !== 1) return undefined;
  if (typeof value.operationId !== 'string' || !/^[A-Za-z0-9-]{1,128}$/u.test(value.operationId)) return undefined;
  if (!(JEV_PROVIDERS as readonly unknown[]).includes(value.providerId)) return undefined;
  return { operationId: value.operationId, providerId: value.providerId as JevProvider };
}

export function registerJevVerify(pi: ExtensionAPI, dependencies: { timeoutMs?: number } = {}) {
  pi.registerCommand(JEV_VERIFY_COMMAND, {
    description: 'Verify a Jev key for Kota.',
    handler: async (raw, ctx) => {
      const request = parseVerifyCommand(raw);
      if (!request || ctx.mode !== 'rpc' || process.env.KOTA_LOCAL_AUTH_HELPER !== '1') throw new Error('kota-jev-verify-invalid-request');
      const report = (outcome: Outcome) => ctx.ui.notify(JEV_CODEC + JSON.stringify({ schema: 1, operationId: request.operationId, event: 'result', outcome }), 'info');
      const key = await ctx.ui.input(JEV_CODEC + JSON.stringify({ schema: 1, operationId: request.operationId, event: 'key' }), '');
      if (typeof key !== 'string' || !key) { report('unavailable'); return; }
      const registry = ctx.modelRegistry as unknown as {
        findOfType(type: 'classifier', provider: string, id: string): unknown;
        classify(model: unknown, context: unknown, options: Record<string, unknown>): Promise<{ stopReason: string; answers?: Record<string, unknown> }>;
      };
      const model = registry.findOfType('classifier', request.providerId, JEV_MODELS[request.providerId]);
      if (!model) { report('unavailable'); return; }
      let status: number | undefined;
      const recordingFetch: typeof fetch = async (input, init) => {
        const response = await globalThis.fetch(input, init);
        status = response.status;
        return response;
      };
      const result = await registry.classify(model, { state: { new_message: VERIFY_INPUT }, questions: { model: {
        type: 'choice', instructions: JEV_INSTRUCTIONS.first,
        criteria: Object.fromEntries(DEFAULT_INTENTS.map((intent, i) => [`model-${i + 1}`, intent])),
      } } },
        { apiKey: key, signal: AbortSignal.timeout(dependencies.timeoutMs ?? JEV_VERIFY_TIMEOUT_MS), maxRetries: 0, fetch: recordingFetch });
      const answer = result.answers?.model;
      if (result.stopReason === 'stop' && isObject(answer) && answer.type === 'choice') report('verified');
      else report(status === 401 || status === 403 ? 'rejected' : 'unavailable');
    },
  });
}
