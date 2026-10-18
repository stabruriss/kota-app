import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { performance } from 'node:perf_hooks';
import { CredentialSynchronizationError, getAgentDir, ModelRuntime,
  type ExtensionAPI, type ExtensionCommandContext } from '@earendil-works/pi-coding-agent';
import { clampThinkingLevel, getSupportedThinkingLevels,
  type AuthPrompt as NativePrompt, type Provider } from '@earendil-works/pi-ai';
import { AUTH_CONTROL_TIMEOUT_MS, AUTH_LOGIN_TIMEOUT_MS, encodeAuthEvent, parseAuthCommand,
  type AuthCode, type AuthCommand, type AuthCommit, type AuthEvent, type AuthOutcome,
  type AuthStatus, type PromptCloseReason, type ProviderCapabilities } from './auth-protocol.ts';

type UI = ExtensionCommandContext['ui'];
type EventFields<T = AuthEvent> = T extends unknown ? Omit<T, 'schema' | 'channel' | 'operationId'> : never;
export type AuthDependencies = {
  createRuntime?: typeof ModelRuntime.create;
  loginTimeoutMs?: number;
  controlTimeoutMs?: number;
};
type Flow = {
  request: AuthCommand; controller: AbortController; ui: UI; deadline: number;
  timer?: ReturnType<typeof setTimeout>; terminal: boolean; sdkInvoked: boolean;
  credentialReturned: boolean; done?: Promise<void>;
};
class AuthFailure extends Error {
  readonly code: AuthCode;
  constructor(code: AuthCode) { super(code); this.code = code; }
}
const abort = (reason: 'cancelled' | 'timeout' | 'interrupted') => new DOMException(reason, 'AbortError');
const isHelper = () => process.env.KOTA_LOCAL_AUTH_HELPER === '1';
function ownedDir(): string {
  const dir = process.env.PI_CODING_AGENT_DIR;
  if (!dir || !isAbsolute(dir) || getAgentDir() !== dir) throw new AuthFailure('source-policy');
  return dir;
}
function capabilities(provider: Provider): ProviderCapabilities {
  const key = provider.auth.apiKey;
  const oauth = provider.auth.oauth;
  return { providerId: provider.id, providerLabel: provider.name,
    canApiKey: typeof key?.login === 'function' && typeof key.resolve === 'function',
    canOAuth: typeof oauth?.login === 'function' && typeof oauth.refresh === 'function' && typeof oauth.toAuth === 'function' };
}
async function freshStoredTypes(): Promise<Map<string, 'api_key' | 'oauth'>> {
  let content: string;
  try { content = await readFile(join(ownedDir(), 'auth.json'), 'utf8'); }
  catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return new Map();
    throw new AuthFailure('storage-failed');
  }
  if (Buffer.byteLength(content, 'utf8') > 2_097_152) throw new AuthFailure('storage-failed');
  let parsed: unknown;
  try { parsed = JSON.parse(content.replace(/^\uFEFF/u, '')); } catch { throw new AuthFailure('storage-failed'); }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new AuthFailure('storage-failed');
  const result = new Map<string, 'api_key' | 'oauth'>();
  for (const [id, credential] of Object.entries(parsed)) {
    if (!credential || typeof credential !== 'object' || Array.isArray(credential) ||
        (credential.type !== 'api_key' && credential.type !== 'oauth')) throw new AuthFailure('storage-failed');
    result.set(id, credential.type);
  }
  return result;
}
function checkedRefresh(runtime: ModelRuntime, result: { aborted: boolean; errors: ReadonlyMap<string, Error> }): void {
  if (result.aborted || result.errors.size || runtime.getError()) throw new AuthFailure('credential-sync-failed');
}
function emit(flow: Flow, fields: EventFields): void {
  if (flow.terminal) return;
  // The emitter alone constructs the private envelope; no exception/credential serialization.
  const event = { schema: 1, channel: 'kota.auth', operationId: flow.request.operationId, ...fields } as AuthEvent;
  try { flow.ui.notify(encodeAuthEvent(event), 'info'); }
  catch { /* Dead owner/transport is fenced and classified by Rust; never serialize its error. */ }
}
function setDeadline(flow: Flow, deadline: number): void {
  flow.deadline = Math.min(flow.deadline, deadline);
  if (flow.timer) clearTimeout(flow.timer);
  flow.timer = setTimeout(() => flow.controller.abort(abort('timeout')), Math.max(0, flow.deadline - performance.now()));
}

/** Parse the hint, but replace only its raw value span: URL reserialization changes other query bytes. */
function chatGptAuthorizationUrl(url: string): string {
  try {
    if (new URL(url).searchParams.get('agent_name_hint') !== 'Pi') return url;
    const start = url.indexOf('?') + 1;
    const fragment = url.indexOf('#', start);
    const end = fragment < 0 ? url.length : fragment;
    let offset = start;
    for (const part of url.slice(start, end).split('&')) {
      if (new URLSearchParams(part).get('agent_name_hint') === 'Pi') {
        return url.slice(0, offset + part.indexOf('=') + 1) + 'Kota' + url.slice(offset + part.length);
      }
      offset += part.length + 1;
    }
  } catch { /* An invalid native URL stays unchanged; do not interrupt its auth flow. */ }
  return url;
}

/** Fixed Pi flow semantics, not titles or a blanket "secret means key" inference. */
function keyRoles(providerId: string) {
  let index = 0;
  let method: string | undefined;
  return {
    field(prompt: NativePrompt): 'api_key.key' | undefined {
      let expected: string;
      let key = false;
      if (providerId === 'amazon-bedrock' || providerId === 'google-vertex') {
        if (index === 0) expected = 'select';
        else if (providerId === 'amazon-bedrock') {
          expected = method === 'bearer-token' ? 'secret' : 'text';
          key = method === 'bearer-token';
          if (index !== 1) throw new AuthFailure('unsupported-auth');
        } else {
          expected = method === 'api-key' ? 'secret' : 'text';
          key = method === 'api-key';
          const steps = method === 'api-key' ? 2 : method === 'service-account' ? 4 : 3;
          if (index >= steps) throw new AuthFailure('unsupported-auth');
        }
      } else if (providerId === 'cloudflare-workers-ai' || providerId === 'cloudflare-ai-gateway') {
        const steps = providerId === 'cloudflare-workers-ai' ? 2 : 3;
        if (index >= steps) throw new AuthFailure('unsupported-auth');
        expected = index === 0 ? 'secret' : 'text'; key = index === 0;
      } else {
        // All other pinned builtin key methods and the native compatibility fallback are
        // the audited single-secret, verbatim Credential.key flow. Upgrade tests pin this.
        if (index !== 0) throw new AuthFailure('unsupported-auth');
        expected = 'secret'; key = true;
      }
      if (prompt.type !== expected) throw new AuthFailure('unsupported-auth');
      return key ? 'api_key.key' : undefined;
    },
    answered(prompt: NativePrompt, value: string) {
      if (index === 0 && prompt.type === 'select') {
        const allowed = providerId === 'amazon-bedrock'
          ? ['bearer-token', 'aws-profile', 'credential-chain'] : ['api-key', 'adc', 'service-account'];
        if (!allowed.includes(value)) throw new AuthFailure('unsupported-auth');
        method = value;
      }
      index++;
    },
  };
}

/** Helper-local evidence at the public native-return boundary; never inspect/store/hash its value. */
function helperLoginRuntime(runtime: ModelRuntime, flow: Flow): void {
  if (!isHelper()) throw new AuthFailure('source-policy');
  const provider = runtime.getProvider(flow.request.providerId!);
  if (!provider) throw new AuthFailure('unsupported-provider');
  if (flow.request.authType === 'api_key') {
    const method = provider.auth.apiKey;
    if (!method?.login) throw new AuthFailure('unsupported-auth');
    const nativeLogin = method.login;
    runtime.registerNativeProvider({ ...provider, auth: { ...provider.auth, apiKey: { ...method,
      async login(interaction) {
        const credential = await nativeLogin.call(method, interaction);
        flow.credentialReturned = true;
        return provider.id === 'azure'
          ? { ...credential, env: { ...credential.env, AZURE_OPENAI_BASE_URL: flow.request.resourceUrl! } }
          : credential;
      },
    } } });
  } else {
    const method = provider.auth.oauth;
    if (!method?.login) throw new AuthFailure('unsupported-auth');
    const nativeLogin = method.login;
    runtime.registerNativeProvider({ ...provider, auth: { ...provider.auth, oauth: { ...method,
      async login(interaction, options) {
        const credential = await nativeLogin.call(method, interaction, options);
        flow.credentialReturned = true;
        return credential;
      },
    } } });
  }
}

export function registerAuth(pi: ExtensionAPI, dependencies: AuthDependencies = {}) {
  const createRuntime = dependencies.createRuntime ?? ModelRuntime.create;
  let runtimePromise: Promise<ModelRuntime> | undefined;
  const flows = new Map<string, Flow>();
  async function runtime(signal: AbortSignal): Promise<ModelRuntime> {
    if (!runtimePromise) {
      runtimePromise = createRuntime({ authPath: join(ownedDir(), 'auth.json'),
        modelsPath: join(ownedDir(), 'models.json'), allowModelNetwork: false, signal });
      void runtimePromise.catch(() => { runtimePromise = undefined; });
    }
    const result = await runtimePromise;
    if (result.getError()) throw new AuthFailure('storage-failed');
    return result;
  }
  async function statuses(models: ModelRuntime, providerId?: string): Promise<AuthStatus[]> {
    const stored = await freshStoredTypes();
    const providers = providerId ? [models.getProvider(providerId)] : models.getProviders();
    if (providers.some(provider => !provider)) throw new AuthFailure('unsupported-provider');
    return providers.map(provider => ({ ...capabilities(provider!), storedType: stored.get(provider!.id) ?? null,
      configured: models.hasConfiguredAuth(provider!.id), policy: 'unknown', validity: 'not-checked' }));
  }
  async function execute(flow: Flow, ctx: ExtensionCommandContext) {
    const request = flow.request;
    const signal = flow.controller.signal;
    emit(flow, { event: 'started', operation: request.operation, providerId: request.providerId });
    let outcome: AuthOutcome = 'succeeded';
    let commit: AuthCommit = 'not-written';
    let code: AuthCode | undefined;
    let status: AuthStatus | undefined;
    let allStatuses: AuthStatus[] | undefined;
    let catalog: Extract<AuthEvent, { event: 'result' }>['catalog'];
    let resolved: Extract<AuthEvent, { event: 'result' }>['resolved'];
    try {
      signal.throwIfAborted();
      if (request.operation === 'refresh') {
        // This is a command handler's asynchronous path, never an injection flush hook.
        if (!ctx.isIdle()) throw new AuthFailure('invalid-request');
        const result = await ctx.modelRegistry.refresh({ allowNetwork: false, signal });
        if (result.aborted || result.errors.size || ctx.modelRegistry.getError()) throw new AuthFailure('credential-sync-failed');
        // Pi refreshes the catalog, not the already-selected model object. Keep
        // Magpie's same ID on the saved gateway after a Settings change; never
        // pick a replacement if the directory removed that model.
        if (ctx.model?.provider === 'magpie') {
          const current = ctx.modelRegistry.find(ctx.model.provider, ctx.model.id);
          if (current && JSON.stringify(current) !== JSON.stringify(ctx.model)) {
            signal.throwIfAborted();
            if (!ctx.isIdle()) throw new AuthFailure('invalid-request');
            const thinking = pi.getThinkingLevel();
            if (!await pi.setModel(current)) throw new AuthFailure('credential-sync-failed');
            pi.setThinkingLevel(thinking); // Keep the user's level; Pi clamps only if the new metadata requires it.
          }
        }
      } else {
        await freshStoredTypes(); // Explicit fresh I/O failure, not Pi's cached fallback.
        const models = request.operation === 'login'
          ? await createRuntime({ authPath: join(ownedDir(), 'auth.json'),
            modelsPath: request.providerId === 'magpie' ? join(ownedDir(), 'models.json') : null, allowModelNetwork: false, signal })
          : await runtime(signal);
        signal.throwIfAborted();
        if (request.operation === 'status') {
          const list = await statuses(models, request.providerId);
          if (request.providerId) status = list[0]; else allStatuses = list;
        } else if (request.operation === 'catalog') {
          const providers = request.providerId ? [models.getProvider(request.providerId)] : models.getProviders();
          if (providers.some(provider => !provider)) throw new AuthFailure('unsupported-provider');
          catalog = { providers: providers.map(provider => ({ ...capabilities(provider!),
            models: models.getModels(provider!.id).map(model => ({ modelId: model.id, name: model.name,
              thinkingLevels: getSupportedThinkingLevels(model) })) })) };
        } else if (request.operation === 'resolve') {
          const model = models.getModel(request.providerId!, request.modelId!);
          if (!model) throw new AuthFailure('model-not-found');
          resolved = { providerId: model.provider, modelId: model.id, requestedThinkingLevel: request.thinkingLevel!,
            thinkingLevel: clampThinkingLevel(model, request.thinkingLevel!) };
        } else if (request.operation === 'logout') {
          if (!models.getProvider(request.providerId!)) throw new AuthFailure('unsupported-provider');
          flow.sdkInvoked = true;
          await models.logout(request.providerId!, { signal });
          commit = 'confirmed'; status = (await statuses(models, request.providerId))[0];
        } else if (request.operation === 'login') {
          if (request.providerId === 'llama.cpp') throw new AuthFailure('unsupported-provider');
          // Only Magpie uses Pi's models.json-composed API-key handler; all other
          // logins stay native-only. No custom provider factory or network catalog.
          helperLoginRuntime(models, flow);
          checkedRefresh(models, await models.refresh({ allowNetwork: false, signal }));
          const provider = models.getProvider(request.providerId!);
          if (!provider) throw new AuthFailure('unsupported-provider');
          const caps = capabilities(provider);
          if (request.authType === 'api_key' ? !caps.canApiKey : !caps.canOAuth) throw new AuthFailure('unsupported-auth');
          const uuid = process.env.KOTA_LOCAL_INSTALLATION_UUID;
          if (request.providerId === 'openai' && request.authType === 'oauth' &&
              (!uuid || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/iu.test(uuid))) throw new AuthFailure('source-policy');
          const roles = keyRoles(provider.id);
          flow.sdkInvoked = true;
          await models.login(provider.id, request.authType!, {
            signal,
            notify(notification) {
              if (signal.aborted || flow.terminal) return;
              if (notification.type === 'device_code' && notification.expiresInSeconds !== undefined &&
                  Number.isFinite(notification.expiresInSeconds) && notification.expiresInSeconds >= 0) {
                setDeadline(flow, performance.now() + notification.expiresInSeconds * 1000);
              }
              const visibleNotification = provider.id === 'openai' && notification.type === 'auth_url'
                ? { ...notification, url: chatGptAuthorizationUrl(notification.url) } : notification;
              emit(flow, { event: 'auth_event', providerId: provider.id, notification: visibleNotification });
            },
            async prompt(prompt) {
              signal.throwIfAborted();
              const promptId = randomUUID();
              const combined = prompt.signal ? AbortSignal.any([signal, prompt.signal]) : signal;
              const credentialField = request.authType === 'api_key' ? roles.field(prompt) : undefined;
              const title = encodeAuthEvent({ schema: 1, channel: 'kota.auth', operationId: request.operationId,
                event: 'prompt', providerId: provider.id, authType: request.authType!, promptId,
                credentialField, prompt });
              let reason: PromptCloseReason = 'answered';
              try {
                const value = prompt.type === 'select'
                  ? await flow.ui.select(title, prompt.options.map(option => option.id), { signal: combined })
                  : await flow.ui.input(title, prompt.placeholder, { signal: combined });
                combined.throwIfAborted();
                if (value === undefined) {
                  flow.controller.abort(abort('cancelled'));
                  throw signal.reason;
                }
                if (prompt.type === 'select' && !prompt.options.some(option => option.id === value)) throw new AuthFailure('invalid-request');
                // Rust checks key syntax before forwarding any reply. No second JS lexer.
                if (credentialField && value.length === 0) throw new AuthFailure('invalid-request');
                if (request.authType === 'api_key') roles.answered(prompt, value);
                return value;
              } catch (error) {
                if (signal.aborted) {
                  reason = signal.reason?.message === 'timeout' ? 'timeout' : signal.reason?.message === 'interrupted' ? 'interrupted' : 'cancelled';
                } else if (prompt.signal?.aborted) reason = 'step-aborted';
                else reason = 'interrupted';
                throw error;
              } finally {
                emit(flow, { event: 'prompt_closed', providerId: provider.id, promptId, reason });
              }
            },
          }, { getDeviceId: () => uuid! });
          commit = 'confirmed'; status = (await statuses(models, request.providerId))[0];
        }
      }
    } catch (error) {
      if (error instanceof CredentialSynchronizationError) {
        outcome = 'committed-unsynchronized'; commit = 'confirmed'; code = 'credential-sync-failed';
      } else {
        code = error instanceof AuthFailure ? error.code : 'auth-failed';
        if (flow.sdkInvoked && commit !== 'confirmed') {
          // No native credential => Models.login could not enter modify. After return,
          // an abort/storage error is not commit evidence. Logout has no such boundary.
          commit = request.operation === 'login' && !flow.credentialReturned ? 'not-written' : 'unknown';
        }
        outcome = commit === 'confirmed' ? 'committed-unsynchronized' : commit === 'unknown' ? 'indeterminate' : signal.aborted
          ? signal.reason?.message === 'timeout' ? 'timeout' : signal.reason?.message === 'interrupted' ? 'interrupted' : 'cancelled'
          : 'failed';
      }
    } finally {
      if (flow.timer) clearTimeout(flow.timer);
      emit(flow, { event: 'result', operation: request.operation, providerId: request.providerId,
        outcome, commit, code, status, statuses: allStatuses, catalog, resolved });
      flow.terminal = true;
      flows.delete(request.operationId);
    }
  }
  pi.registerCommand('kota-auth', {
    handler: async (args, ctx) => {
      const request = parseAuthCommand(args);
      if (!request || ctx.mode !== 'rpc') throw new Error('kota-auth-invalid-request');
      ownedDir();
      if (!isHelper() && request.operation !== 'refresh') throw new Error('kota-auth-helper-required');
      if (request.operation === 'cancel') {
        flows.get(request.targetOperationId!)?.controller.abort(abort('cancelled'));
        ctx.ui.notify(encodeAuthEvent({ schema: 1, channel: 'kota.auth', operationId: request.operationId,
          event: 'result', operation: 'cancel', outcome: 'succeeded', commit: 'not-written' }), 'info');
        return;
      }
      if (flows.size) {
        ctx.ui.notify(encodeAuthEvent({ schema: 1, channel: 'kota.auth', operationId: request.operationId,
          event: 'result', operation: request.operation, providerId: request.providerId,
          outcome: 'failed', commit: 'not-written', code: 'busy-auth' }), 'info');
        return;
      }
      const timeout = request.operation === 'login'
        ? dependencies.loginTimeoutMs ?? AUTH_LOGIN_TIMEOUT_MS
        : dependencies.controlTimeoutMs ?? AUTH_CONTROL_TIMEOUT_MS;
      const flow: Flow = { request, controller: new AbortController(), ui: ctx.ui,
        deadline: performance.now() + timeout, terminal: false, sdkInvoked: false,
        credentialReturned: false };
      flows.set(request.operationId, flow);
      setDeadline(flow, flow.deadline);
      flow.done = execute(flow, ctx);
      await flow.done;
    },
  });
  pi.on('session_shutdown', () => {
    for (const flow of flows.values()) flow.controller.abort(abort('interrupted'));
  });
}
