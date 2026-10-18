import { CUSTOM_TYPES } from './inject.ts';
import { randomUUID } from 'node:crypto';
import { closeSync, fsyncSync, lstatSync, openSync, readFileSync, renameSync, rmSync, writeSync } from 'node:fs';
import { dirname, join } from 'node:path';
import prompts from './puppeteer-prompts.json' with { type: 'json' };
import { BUNDLED_PROMPTS, type PuppeteerPrompts } from './puppeteer-prompts-core.ts';

/** String: one virtual model, the Local agent's own 2–6-model group, and no global presets. */
export const AUTO_ROUTE_PROVIDER = 'kota';
export const AUTO_ROUTE_MODEL = 'auto';
export const AUTO_ROUTE_FLAG = 'kota-auto-route';
export const AUTO_ROUTE_COMMAND = 'kota-auto-route';
/** Private workbench facts, never a model message or room history. */
export const ROUTE_CODEC = 'kota.route.v1:';
export const JEV_PROVIDERS = ['openrouter', 'vercel-ai-gateway', 'opencode', 'typesafe'] as const;
export type JevProvider = typeof JEV_PROVIDERS[number];
export const JEV_MODELS: Record<JevProvider, string> = {
  openrouter: '~typesafe/jev-latest', 'vercel-ai-gateway': 'typesafe-ai/jev', opencode: 'jev-1.13', typesafe: 'jev-latest',
};
export const JEV_TIMEOUT_MS = 3_000;
export const DEFAULT_THRESHOLD = 0.5;
export const PREVIOUS_REPLY_CHARS = 1_500;
export const NEW_MESSAGE_CHARS = 4_000;
export type RouteErrorCode = 'jev_auth' | 'jev_timeout' | 'jev_unreachable' | 'route_empty' | 'route_exhausted';
/** Tested against Pi's retry classifier. Error replies are excluded from model input. */
export const ROUTE_ERROR_TEXT = {
  jev_auth: 'Jev authentication failed.',
  jev_timeout: 'Jev did not respond within 3 seconds.',
  jev_unreachable: 'Could not reach Jev.',
  route_empty: 'No models are configured for this Auto String.',
  route_exhausted: 'The selected model failed for this turn.',
} as const;

/** JEV-PROMPTS.md, 老无's final instructions, copied verbatim. */
export const JEV_INSTRUCTIONS = {
  first: prompts.stringFirst.questions.model.instructions,
  later: prompts.stringLater.questions.model.instructions,
};
export const DEFAULT_INTENTS = prompts.defaultIntents;
const COMPACTION_SUMMARY_PREFIX = 'The conversation history before this point was compacted into the following summary:\n\n<summary>\n';
const BRANCH_SUMMARY_PREFIX = 'The following is a summary of a branch that this conversation came back from:\n\n<summary>\n';
const MAX_FILE_BYTES = 1024 * 1024;
export const THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const;
export interface RouteModel { id: string; providerId: string; modelId: string; thinkingLevel: string; intent: string }
export interface Group { models: RouteModel[]; defaultModel: string; threshold: number }
export interface RouteState {
  v: 2 | 3;
  switchId: string | null;
  selectedModel: string;
  routeModel: string;
  fallback: boolean;
  snapshot: Group;
  /** Stable delivery identities; a requeued Kota input keeps its original kotaId. */
  inputIds?: string[];
  thinkingLevel?: string;
}
export type Message = { role: string; content?: unknown; stopReason?: string };
export type Physical = { provider: string; id: string; contextWindow?: number };
export class RouteError extends Error {
  readonly code: RouteErrorCode;
  constructor(code: RouteErrorCode, text: string) { super(text); this.code = code; }
}
export const isObject = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
export const chars = (text: string) => Array.from(text);
/** Text only; attachments and tool data never reach String's classifier. */
export function messageText(message: Message): string {
  if (typeof message.content === 'string') return message.content;
  if (!Array.isArray(message.content)) return '';
  return message.content.flatMap(block => isObject(block) && block.type === 'text' && typeof block.text === 'string' ? [block.text] : []).join('\n');
}
const isSummary = (message: Message) => {
  const text = messageText(message);
  return text.startsWith(COMPACTION_SUMMARY_PREFIX) || text.startsWith(BRANCH_SUMMARY_PREFIX);
};
/** Kota custom messages arrive as continuations. Only fresh user-role input starts a new turn. */
export function newMessageText(messages: readonly Message[]): string | undefined {
  const last = messages.findLastIndex(message => message.role === 'assistant');
  const fresh = messages.slice(last + 1).filter(message => message.role === 'user' && !isSummary(message));
  if (!fresh.length) return undefined;
  return chars(fresh.map(messageText).join('\n\n')).slice(0, NEW_MESSAGE_CHARS).join('');
}
export function previousReply(messages: readonly Message[]): string | undefined {
  for (let index = messages.findLastIndex(message => message.role === 'assistant'); index >= 0; index--) {
    const message = messages[index];
    if (message.role !== 'assistant' || (message.stopReason !== 'stop' && message.stopReason !== 'length')) continue;
    const text = messageText(message);
    if (text.trim()) return chars(text).slice(-PREVIOUS_REPLY_CHARS).join('');
  }
  return undefined;
}
/** Only used if routing fails before Pi can persist the returned virtual-model state. */
export const FAILED_SELECTION_ENTRY = 'kota.auto-route.selection';
/** Read branch-local choices, not projected summaries or text equality. */
export function routingHistory(branch: readonly Record<string, unknown>[]) {
  const change = branch.findLastIndex(entry => entry.type === 'model_change');
  const boundary = change >= 0 && branch[change].provider === AUTO_ROUTE_PROVIDER && branch[change].modelId === AUTO_ROUTE_MODEL
    ? branch[change].id as string : null;
  const choices: Array<{ index: number; state: RouteState }> = [];
  for (let index = branch.length - 1; index > change; index--) {
    const entry = branch[index];
    if (entry.type !== 'custom' || !isObject(entry.data)) continue;
    const data = entry.data;
    const state = entry.customType === FAILED_SELECTION_ENTRY ? data : entry.customType === 'pi.virtual-model-state'
      && data.provider === AUTO_ROUTE_PROVIDER && data.modelId === AUTO_ROUTE_MODEL ? data.state : undefined;
    if (!isObject(state) || ![2, 3].includes(state.v as number) || state.switchId !== boundary || !parseGroup(state.snapshot)
      || typeof state.selectedModel !== 'string' || typeof state.routeModel !== 'string') continue;
    choices.push({ index, state: state as unknown as RouteState });
  }
  const current = choices[0]?.state;
  const response = branch.findLastIndex(entry => entry.type === 'message' && isObject(entry.message) && entry.message.role === 'assistant');
  const after = Math.max(choices[0]?.index ?? -1, response);
  const inputs = branch.slice(after + 1).flatMap(entry => {
    if (entry.type === 'custom_message' && (CUSTOM_TYPES as readonly unknown[]).includes(entry.customType)
      && isObject(entry.details) && typeof entry.details.kotaId === 'string') {
      return [{ id: `kota:${entry.details.kotaId}`, text: messageText({ role: 'user', content: entry.content }) }];
    }
    if (entry.type === 'message' && isObject(entry.message) && entry.message.role === 'user' && typeof entry.id === 'string') {
      return [{ id: `pi:${entry.id}`, text: messageText(entry.message as Message) }];
    }
    return [];
  });
  const choiceFor = (id: string) => choices.find(({ state }) => state.inputIds?.includes(id))?.state;
  return { boundary, current, inputs, choiceFor };
}

/** First request omits both current_model and previous_reply, even after switching from a real model. */
export function jevContext(newMessage: string, previous: string | undefined, current: string | undefined, group: Group, prompts: PuppeteerPrompts = BUNDLED_PROMPTS) {
  const state: Record<string, unknown> = { new_message: newMessage };
  if (current) {
    state.current_model = current;
    if (previous !== undefined) state.previous_reply = previous;
  }
  state.models = group.models.map(({ id, providerId, modelId, thinkingLevel }) => ({ id, providerId, modelId, thinkingLevel }));
  const template = current ? prompts.stringLater.questions.model : prompts.stringFirst.questions.model;
  return { state, questions: { model: { type: 'choice' as const, instructions: template.instructions,
    criteria: Object.fromEntries(group.models.map(model => [model.id, template.criteria['<model_id>'].replaceAll('<model_id>', model.id).replaceAll('<intent>', model.intent)])) } } };
}
/** Probabilities only. Strict >; first defaults, later keeps the current outlet below/equal threshold. */
export function routingCandidate(answer: { probabilities: Record<string, number> }, current: string | undefined, group: Group) {
  const validCurrent = group.models.some(m => m.id === current) ? current : undefined;
  const candidates = group.models.filter(m => m.id !== validCurrent);
  return candidates.reduce<RouteModel | undefined>((best, m) => !best || answer.probabilities[m.id] > answer.probabilities[best.id] ? m : best, undefined);
}
export function decideModel(answer: { probabilities: Record<string, number> }, current: string | undefined, group: Group): string {
  const validCurrent = group.models.some(m => m.id === current) ? current : undefined;
  const highest = routingCandidate(answer, current, group);
  return highest && answer.probabilities[highest.id] > group.threshold ? highest.id : validCurrent ?? group.defaultModel;
}

function readJson(path: string): unknown {
  let stat;
  try { stat = lstatSync(path); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined; throw error; }
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > MAX_FILE_BYTES) throw new Error('invalid owned file');
  return JSON.parse(readFileSync(path, 'utf8'));
}
function writeJsonAtomic(path: string, value: unknown) {
  const temporary = join(dirname(path), `.kota-${randomUUID()}.tmp`);
  const fd = openSync(temporary, 'wx', 0o600);
  try { writeSync(fd, `${JSON.stringify(value)}\n`); fsyncSync(fd); }
  finally { closeSync(fd); }
  try { renameSync(temporary, path); }
  catch (error) { rmSync(temporary, { force: true }); throw error; }
}
const bounded = (value: unknown, maximum: number): value is string =>
  typeof value === 'string' && value.length > 0 && value.length <= maximum && !/[\u0000-\u001f\u007f]/u.test(value);
/** Malformed groups are rejected whole; never fall back to partially parsed cards. */
export function parseGroup(raw: unknown): Group | undefined {
  let value: unknown = raw;
  if (typeof raw === 'string') { try { value = JSON.parse(raw); } catch { return undefined; } }
  if (!isObject(value) || Object.keys(value).length !== 3 || !Array.isArray(value.models) || value.models.length < 2 || value.models.length > 6
    || typeof value.threshold !== 'number' || !Number.isFinite(value.threshold) || value.threshold < 0.05 || value.threshold > 0.95
    || Math.abs(value.threshold * 20 - Math.round(value.threshold * 20)) > 1e-9) return undefined;
  const models: RouteModel[] = [];
  for (const [index, model] of value.models.entries()) {
    if (!isObject(model) || Object.keys(model).length !== 5 || model.id !== `model-${index + 1}`
      || !bounded(model.providerId, 128) || model.providerId.includes('/') || model.providerId === AUTO_ROUTE_PROVIDER
      || !bounded(model.modelId, 1024) || !(THINKING_LEVELS as readonly unknown[]).includes(model.thinkingLevel)
      || typeof model.intent !== 'string' || chars(model.intent).length > 200 || model.intent.includes('\0')) return undefined;
    models.push(model as unknown as RouteModel);
  }
  if (!models.some(m => m.id === value.defaultModel)) return undefined;
  return { models, defaultModel: value.defaultModel as string, threshold: value.threshold };
}
export interface JevCredential { provider: JevProvider; key: string; keyId: string }
export function readJevCredential(root: string): JevCredential | undefined {
  const value = readJson(join(root, 'jev', 'keys.json'));
  if (!isObject(value) || value.schema !== 1 || !(JEV_PROVIDERS as readonly unknown[]).includes(value.provider)) return undefined;
  const provider = value.provider as JevProvider;
  const entry = isObject(value.keys) ? value.keys[provider] : undefined;
  if (!isObject(entry) || typeof entry.key !== 'string' || !entry.key || typeof entry.keyId !== 'string') return undefined;
  const rejected = readJson(join(root, 'jev', `rejected-${provider}.json`));
  if (isObject(rejected) && rejected.keyId === entry.keyId) return undefined;
  return { provider, key: entry.key, keyId: entry.keyId };
}
export function markJevRejected(root: string, credential: JevCredential) {
  writeJsonAtomic(join(root, 'jev', `rejected-${credential.provider}.json`), { schema: 1, keyId: credential.keyId });
}
