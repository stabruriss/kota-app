import { dirname, isAbsolute } from 'node:path';
import { getAgentDir, type ExtensionAPI, type ExtensionContext } from '@earendil-works/pi-coding-agent';
import { clampThinkingLevel } from '@earendil-works/pi-ai';
import { loadPuppeteerPrompts } from './puppeteer-prompts.ts';
import {
  AUTO_ROUTE_COMMAND, AUTO_ROUTE_FLAG, AUTO_ROUTE_MODEL, AUTO_ROUTE_PROVIDER, FAILED_SELECTION_ENTRY, JEV_MODELS, JEV_TIMEOUT_MS,
  ROUTE_CODEC, ROUTE_ERROR_TEXT, RouteError, chars, decideModel, isObject, jevContext, markJevRejected, NEW_MESSAGE_CHARS, routingHistory,
  parseGroup, previousReply, readJevCredential, routingCandidate,
  type JevCredential, type Message, type Group, type Physical, type RouteErrorCode, type RouteState,
} from './auto-route-core.ts';

function ownedRoot(): string | undefined {
  const dir = process.env.PI_CODING_AGENT_DIR;
  if (!dir || !isAbsolute(dir) || getAgentDir() !== dir) return undefined;
  return dirname(dir);
}
export type Classify = (model: unknown, context: ReturnType<typeof jevContext>, options: Record<string, unknown>) =>
  Promise<{ stopReason: string; answers?: Record<string, unknown> }>;
export interface AutoRouteDependencies { root?: () => string | undefined; timeoutMs?: number; fetch?: typeof fetch }

export function registerAutoRoute(pi: ExtensionAPI, dependencies: AutoRouteDependencies = {}) {
  const root = dependencies.root ?? ownedRoot;
  const timeoutMs = dependencies.timeoutMs ?? JEV_TIMEOUT_MS;
  let commandGroup: Group | undefined;
  let turn: RouteState | undefined;
  let physicalFailure = false;
  const currentGroup = () => commandGroup ?? parseGroup(pi.getFlag(AUTO_ROUTE_FLAG));
  const notify = (ctx: ExtensionContext, body: Record<string, unknown>) => {
    try { ctx.ui.notify(ROUTE_CODEC + JSON.stringify({ schema: 1, ...body }), 'info'); }
    catch { /* Routing never depends on workbench projection. */ }
  };
  const fail = (ctx: ExtensionContext, code: RouteErrorCode, text: string, physical?: { provider: string; model: string }): never => {
    notify(ctx, { event: 'error', code, kind: code.startsWith('jev_') ? 'jev_error' : 'string_error', message: text, ...physical });
    throw new RouteError(code, text);
  };
  const history = (ctx: ExtensionContext) => routingHistory(ctx.sessionManager.getBranch() as unknown as Record<string, unknown>[]);
  const dispatch = (ctx: ExtensionContext, state: RouteState, observe = true) => {
    const config = state.snapshot.models.find(m => m.id === state.routeModel);
    if (!config) return fail(ctx, 'route_empty', ROUTE_ERROR_TEXT.route_empty);
    const model = ctx.modelRegistry.find(config.providerId, config.modelId);
    const issue = !model ? `Model ${config.providerId}/${config.modelId} is unavailable.`
      : !ctx.modelRegistry.hasConfiguredAuth(model) ? `No credentials for ${config.providerId}/${config.modelId}.` : null;
    // Pi clamps once on selection. Retry must retain the actual level reported by the failed response.
    const thinkingLevel = state.thinkingLevel ?? (model ? clampThinkingLevel(model, config.thinkingLevel as never) : config.thinkingLevel);
    if (model && state.thinkingLevel !== thinkingLevel) state = { ...state, thinkingLevel };
    turn = state;
    if (observe) {
      physicalFailure = false;
      notify(ctx, { event: 'decision', selectedModel: state.selectedModel, routeModel: state.routeModel, fallback: state.fallback,
        provider: config.providerId, model: config.modelId, thinkingLevel });
    }
    if (issue) {
      // A thrown route never reaches Pi's state persistence. Keep this choice privately so the
      // same delivery retry (including after restart) cannot ask Jev for a different model.
      pi.appendEntry(FAILED_SELECTION_ENTRY, state);
      return fail(ctx, 'route_exhausted', `${ROUTE_ERROR_TEXT.route_exhausted} ${issue}`, { provider: config.providerId, model: config.modelId });
    }
    return { model: model!, thinkingLevel: thinkingLevel as never, state };
  };

  async function askJev(ctx: ExtensionContext, signal: AbortSignal | undefined, context: ReturnType<typeof jevContext>,
    credential: JevCredential, location: string) {
    const registry = ctx.modelRegistry as unknown as {
      findOfType(type: 'classifier', provider: string, id: string): unknown; classify: Classify;
    };
    const model = registry.findOfType('classifier', credential.provider, JEV_MODELS[credential.provider]);
    if (!model) fail(ctx, 'jev_unreachable', ROUTE_ERROR_TEXT.jev_unreachable);
    const timer = new AbortController();
    let timedOut = false;
    const clock = setTimeout(() => { timedOut = true; timer.abort(); }, timeoutMs);
    const combined = signal ? AbortSignal.any([signal, timer.signal]) : timer.signal;
    let status: number | undefined;
    const baseFetch = dependencies.fetch ?? globalThis.fetch;
    const recordingFetch: typeof fetch = async (input, init) => {
      const response = await baseFetch(input, init); status = response.status; return response;
    };
    let result;
    try {
      result = await registry.classify(model, context, { apiKey: credential.key, signal: combined, maxRetries: 0, fetch: recordingFetch });
    } catch {
      if (signal?.aborted) throw signal.reason ?? new Error('Request aborted.');
      fail(ctx, timedOut ? 'jev_timeout' : 'jev_unreachable', timedOut ? ROUTE_ERROR_TEXT.jev_timeout : ROUTE_ERROR_TEXT.jev_unreachable);
    } finally { clearTimeout(clock); }
    if (signal?.aborted) throw signal.reason ?? new Error('Request aborted.');
    if (timedOut) fail(ctx, 'jev_timeout', ROUTE_ERROR_TEXT.jev_timeout);
    if (result.stopReason !== 'stop') {
      if (status === 401 || status === 403) {
        try { markJevRejected(location, credential); } catch { /* The turn still fails closed. */ }
        fail(ctx, 'jev_auth', ROUTE_ERROR_TEXT.jev_auth);
      }
      fail(ctx, 'jev_unreachable', ROUTE_ERROR_TEXT.jev_unreachable);
    }
    const answer = result.answers?.model;
    if (!isObject(answer) || answer.type !== 'choice' || typeof answer.choice !== 'string' || !isObject(answer.probabilities)
      || !Object.hasOwn(context.questions.model.criteria, answer.choice)
      || Object.keys(context.questions.model.criteria).some(id => typeof (answer.probabilities as Record<string, unknown>)[id] !== 'number'
        || !Number.isFinite((answer.probabilities as Record<string, unknown>)[id]) || ((answer.probabilities as Record<string, unknown>)[id] as number) < 0 || ((answer.probabilities as Record<string, unknown>)[id] as number) > 1)) {
      fail(ctx, 'jev_unreachable', ROUTE_ERROR_TEXT.jev_unreachable);
    }
    return answer as unknown as { probabilities: Record<string, number> };
  }

  async function decide(request: { messages: readonly Message[]; signal?: AbortSignal }, ctx: ExtensionContext,
    context: ReturnType<typeof routingHistory>, inputs: Array<{ id: string; text: string }>) {
    const snapshot = currentGroup();
    if (!snapshot) return fail(ctx, 'route_empty', ROUTE_ERROR_TEXT.route_empty);
    const location = root();
    let credential: JevCredential | undefined;
    try { credential = location ? readJevCredential(location) : undefined; } catch { credential = undefined; }
    if (!credential || !location) return fail(ctx, 'jev_auth', ROUTE_ERROR_TEXT.jev_auth);
    const current = context.current && snapshot.models.some(m => m.id === context.current!.selectedModel)
      ? context.current.selectedModel : undefined;
    const loaded = loadPuppeteerPrompts(location);
    notify(ctx, { event: 'prompt_status', path: loaded.path, warning: loaded.warning ?? null });
    const text = chars(inputs.map(input => input.text).join('\n\n')).slice(0, NEW_MESSAGE_CHARS).join('');
    const answer = await askJev(ctx, request.signal,
      jevContext(text, previousReply(request.messages), current, snapshot, loaded.prompts), credential, location);
    const selectedModel = decideModel(answer, current, snapshot);
    const selected = snapshot.models.find(model => model.id === selectedModel)!;
    const candidate = routingCandidate(answer, current, snapshot)!;
    const physical = ctx.modelRegistry.find(selected.providerId, selected.modelId);
    const thinkingLevel = physical ? clampThinkingLevel(physical, selected.thinkingLevel as never) : selected.thinkingLevel;
    // Only a real classifier result creates a log row. decision remains a repeatable projection.
    notify(ctx, { event: 'selection', outcome: answer.probabilities[candidate.id] > snapshot.threshold ? 'selected' : current ? 'retained' : 'default',
      selected: { provider: selected.providerId, model: selected.modelId, thinkingLevel },
      candidate: { routeModel: candidate.id, provider: candidate.providerId, model: candidate.modelId, probability: answer.probabilities[candidate.id] },
      threshold: snapshot.threshold });
    return dispatch(ctx, { v: 3, switchId: context.boundary, selectedModel, routeModel: selectedModel, fallback: false, snapshot,
      thinkingLevel, inputIds: inputs.map(input => input.id) });
  }
  function retry(request: { failed?: { model: Physical; thinkingLevel?: string } }, ctx: ExtensionContext) {
    // Missing failed means routing itself failed. Never reinterpret it as a fresh input.
    if (!request.failed) return fail(ctx, 'route_exhausted', ROUTE_ERROR_TEXT.route_exhausted);
    const state = history(ctx).current;
    const thinkingLevel = request.failed.thinkingLevel ?? 'off';
    turn = state ? state.thinkingLevel === thinkingLevel ? state : { ...state, thinkingLevel } : undefined;
    return { model: request.failed.model as never, thinkingLevel: thinkingLevel as never, state: turn };
  }
  function direct(ctx: ExtensionContext) {
    // Threshold compaction happens after a new choice is persisted but before its first reply.
    // request.previous can still be the preceding input's model. The branch choice wins.
    const context = history(ctx);
    if (context.current) return dispatch(ctx, context.current, false);
    const snapshot = currentGroup();
    if (!snapshot) return fail(ctx, 'route_empty', ROUTE_ERROR_TEXT.route_empty);
    return dispatch(ctx, { v: 3, switchId: context.boundary, selectedModel: snapshot.defaultModel, routeModel: snapshot.defaultModel,
      fallback: false, snapshot, inputIds: [] }, false);
  }
  pi.registerFlag(AUTO_ROUTE_FLAG, { description: 'Kota Auto String model group (JSON).', type: 'string' });
  pi.registerCommand(AUTO_ROUTE_COMMAND, {
    description: 'Update the Kota Auto String model group.',
    handler: async raw => {
      const group = parseGroup(raw);
      if (!group) throw new Error('kota-auto-route-failed code=invalid');
      commandGroup = group;
    },
  });
  pi.registerVirtualModel<RouteState>({
    provider: AUTO_ROUTE_PROVIDER, id: AUTO_ROUTE_MODEL, name: 'Auto String', thinkingLevels: ['off'],
    async route(request, ctx) {
      if (request.reason === 'direct') return direct(ctx);
      if (request.reason === 'retry') return retry(request as never, ctx);
      const context = history(ctx);
      const fresh = context.inputs.filter(input => !context.choiceFor(input.id));
      if (fresh.length) return decide(request as never, ctx, context, fresh);
      // A repeated delivery retains its original selection, even if another input or group edit
      // came between. Text is never used as identity; batched new inputs share one choice.
      const replay = context.inputs.length ? context.choiceFor(context.inputs.at(-1)!.id) : undefined;
      if (replay ?? context.current) return dispatch(ctx, (replay ?? context.current)!);
      return direct(ctx);
    },
  });
  pi.on('message_end', (event, ctx) => {
    const message = event.message as unknown as { role: string; stopReason?: string; errorMessage?: string; provider?: string; model?: string };
    if (message.role !== 'assistant' || ctx.model?.provider !== AUTO_ROUTE_PROVIDER || ctx.model?.id !== AUTO_ROUTE_MODEL) return;
    physicalFailure = false;
    if (message.stopReason !== 'error' || message.provider === AUTO_ROUTE_PROVIDER || !turn || !message.provider || !message.model) return;
    physicalFailure = true;
    notify(ctx, { event: 'model_error', provider: message.provider, model: message.model,
      message: chars(message.errorMessage ?? '').slice(0, 2_000).join('') });
    // Observe only: Pi owns classification, retry budget/backoff and context-overflow recovery.
    // In particular neither retryable nor terminal errors are rewritten.
  });
  pi.on('agent_settled', (event, ctx) => {
    if (!event.aborted && physicalFailure && ctx.model?.provider === AUTO_ROUTE_PROVIDER && ctx.model?.id === AUTO_ROUTE_MODEL) {
      notify(ctx, { event: 'error', code: 'route_exhausted' });
    }
    physicalFailure = false;
  });
}
