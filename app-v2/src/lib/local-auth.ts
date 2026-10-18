import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import claudeIcon from '../assets/tavern/icons/providers/claude.svg';
import githubIcon from '../assets/tavern/icons/providers/github.svg';
import geminiIcon from '../assets/tavern/icons/providers/googlegemini.svg';
import kimiIcon from '../assets/tavern/icons/providers/kimi.svg';
import openaiIcon from '../assets/tavern/icons/providers/openai.svg';
import opencodeIcon from '../assets/tavern/icons/providers/opencode.svg';

// Kota Local sign-ins in Tavern Link (M2 D4/D5). Contracts follow M2-BACKEND-OVERVIEW v1.4 §1–§2, the
// D4 addenda and D5-IMPLEMENTATION-PROTOCOL v1.1 (auth commands and the local-auth-event projection).

export type LocalAuthStatus = 'signed_in' | 'not_signed_in' | 'invalid' | 'checking' | 'error';
export type LocalAuthType = 'api_key' | 'oauth';
export interface LocalAuthAvailability {
  schema: 1;
  providers: Array<{ providerId: string; status: LocalAuthStatus; authType?: LocalAuthType; errorCode?: string }>;
  summary: 'ready' | 'not_ready' | 'checking' | 'error';
  error?: { code: string; providerId?: string; stage?: string };
}
export interface LocalModelCatalog {
  schema: 1;
  /** Every Pi provider; `linked` is the fresh owned gate. Unlinked names stay listed for display only. */
  providers: Array<{ providerId: string; linked: boolean; models: Array<{ modelId: string; name: string; thinkingLevels: string[] }> }>;
}
export type LocalAuthPrompt =
  | { type: 'text' | 'secret' | 'manual_code'; message: string; placeholder?: string }
  | { type: 'select'; message: string; options: Array<{ id: string; label: string; description?: string }> };
export type LocalAuthNotification =
  | { type: 'info'; message: string; links?: Array<{ url: string; label?: string }> }
  | { type: 'auth_url'; url: string; instructions?: string }
  | { type: 'device_code'; userCode: string; verificationUri: string; intervalSeconds?: number; expiresInSeconds?: number }
  | { type: 'progress'; message: string };
export type LocalAuthOutcome =
  | 'succeeded' | 'failed' | 'cancelled' | 'timeout' | 'interrupted' | 'committed-unsynchronized' | 'indeterminate';
/** What Rust knows was written by the operation (D5 v1.1). */
export type LocalAuthCommit = 'confirmed' | 'not-written' | 'unknown';
export type LocalAuthEvent = { schema: 1; channel: 'kota.auth'; operationId: string } & (
  | { event: 'started'; operation: string; providerId?: string }
  | { event: 'prompt'; providerId: string; authType: LocalAuthType; promptId: string; credentialField?: 'api_key.key'; prompt: LocalAuthPrompt }
  | { event: 'auth_event'; providerId: string; notification: LocalAuthNotification }
  | { event: 'prompt_closed'; providerId: string; promptId: string; reason: 'answered' | 'cancelled' | 'timeout' | 'step-aborted' | 'interrupted' }
  | { event: 'result'; operation: string; providerId?: string; outcome: LocalAuthOutcome; commit: LocalAuthCommit; code?: string }
);
export interface LocalAuthStartRequest {
  operation: 'login' | 'logout';
  providerId: string;
  authType?: LocalAuthType;
  /** Azure API key only (required there): stored as credential.env.AZURE_OPENAI_BASE_URL by the helper. */
  resourceUrl?: string;
}
/** value and cancelled are mutually exclusive. */
export interface LocalAuthResponse { operationId: string; promptId: string; value?: string; cancelled?: true }
/** Live Local owners in this App whose running model uses the provider (other Apps are not visible). */
export interface LocalAuthInUse { agentId: string; projectRoot: string }

export const readLocalAuthAvailability = () => invoke<LocalAuthAvailability>('local_rpc_auth_availability');
export const readLocalModelCatalog = () => invoke<LocalModelCatalog>('local_rpc_model_catalog');
export const startLocalAuth = (request: LocalAuthStartRequest) =>
  invoke<{ operationId: string }>('local_rpc_auth_start', { request });
export const respondLocalAuth = (request: LocalAuthResponse) => invoke<void>('local_rpc_auth_respond', { request });
export const cancelLocalAuth = (operationId: string) => invoke<void>('local_rpc_auth_cancel', { operationId });
export const readLocalAuthInUse = (providerId: string) =>
  invoke<LocalAuthInUse[]>('local_rpc_auth_in_use', { providerId });
export const onLocalAuthEvent = (callback: (event: LocalAuthEvent) => void) =>
  listen<LocalAuthEvent>('local-auth-event', (event) => callback(event.payload));

/** Providers whose sign-in passes the fresh owned gate: the only ones offered for selection (Link, D7). */
export const linkedCatalogProviders = (catalog: LocalModelCatalog | null) =>
  (catalog?.providers ?? []).filter((provider) => provider.linked);
/** A model from the full catalog, linked or not, so a saved model keeps its name and levels after sign-out. */
export const catalogModel = (catalog: LocalModelCatalog | null, providerId: string, modelId: string) =>
  catalog?.providers.find((provider) => provider.providerId === providerId)?.models.find((model) => model.modelId === modelId);

/**
 * Listens before starting, so events that beat the start response are not lost: they are held
 * (bounded) until the operationId is known, then only the current operation's events pass through.
 * A handled command is not success; the `result` event decides.
 */
export async function openLocalAuthSession(
  request: LocalAuthStartRequest,
  onEvent: (event: LocalAuthEvent) => void,
): Promise<{ operationId: string; close: () => void }> {
  const early: LocalAuthEvent[] = [];
  let current: string | null = null;
  const unlisten = await onLocalAuthEvent((event) => {
    if (current === null) { if (early.length < 64) early.push(event); return; }
    if (event.operationId === current) onEvent(event);
  });
  try {
    const { operationId } = await startLocalAuth(request);
    current = operationId;
    for (const event of early.splice(0)) if (event.operationId === operationId) onEvent(event);
    return { operationId, close: unlisten };
  } catch (error) {
    unlisten();
    throw error;
  }
}

/** Readable text for an IPC rejection (a controlled code string, an Error, or a plain object). */
export function localAuthErrorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  try { return JSON.stringify(error); } catch { return String(error); }
}

/** The controlled code of an IPC rejection: Rust's LocalError is {code, providerId?}. */
export function localAuthErrorCode(error: unknown): string | undefined {
  if (error && typeof error === 'object' && typeof (error as { code?: unknown }).code === 'string') return (error as { code: string }).code;
  return typeof error === 'string' ? error : undefined;
}

/** The helper's structural check for Azure's Resource URL: absolute http(s), a host, no user info. */
export function isLocalResourceUrl(value: string): boolean {
  try {
    const url = new URL(value.trim());
    return (url.protocol === 'https:' || url.protocol === 'http:') && !!url.hostname && !url.username && !url.password;
  } catch {
    return false;
  }
}

/** Sign in opens Tavern Link; Jev setup opens the Puppeteer card. */
export type KotaLocalLinkTarget = 'kota-local' | 'kota-local-jev' | 'puppeteer-jev';
export const KOTA_LOCAL_LINK_EVENT = 'kota:tavern-open-link';
export function requestKotaLocalLink(target: KotaLocalLinkTarget = 'kota-local') {
  window.dispatchEvent(new CustomEvent<{ target: KotaLocalLinkTarget }>(KOTA_LOCAL_LINK_EVENT, { detail: { target } }));
}

/** Pi 1.3.0 ModelThinkingLevel order (pi-ai models.js EXTENDED_THINKING_LEVELS). */
export const LOCAL_THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const;

/** Pi's clampThinkingLevel over a model's supported levels: keep, else next higher, else next lower. */
export function clampThinkingLevel(levels: readonly string[], level: string): string {
  if (levels.includes(level)) return level;
  const at = LOCAL_THINKING_LEVELS.indexOf(level as typeof LOCAL_THINKING_LEVELS[number]);
  if (at === -1) return levels[0] ?? 'off';
  for (let i = at; i < LOCAL_THINKING_LEVELS.length; i += 1) if (levels.includes(LOCAL_THINKING_LEVELS[i])) return LOCAL_THINKING_LEVELS[i];
  for (let i = at - 1; i >= 0; i -= 1) if (levels.includes(LOCAL_THINKING_LEVELS[i])) return LOCAL_THINKING_LEVELS[i];
  return levels[0] ?? 'off';
}

/** The same medium baseline as Pi's SDK, then its supported-level clamp. */
export function localThinkingForModel(levels: readonly string[], current?: string | null): string {
  return clampThinkingLevel(levels, LOCAL_THINKING_LEVELS.some(level => level === current) ? current! : 'medium');
}

export interface LocalAuthProvider {
  id: string;
  name: string;
  methods: LocalAuthType[];
  /** Pi oauth.loginLabel ?? oauth.name, and apiKey.name, for the method choice. */
  oauthLabel?: string;
  apiKeyLabel?: string;
  icon?: string;
}
const p = (id: string, name: string, methods: LocalAuthType[], labels: { oauth?: string; key?: string } = {}, icon?: string): LocalAuthProvider =>
  ({ id, name, methods, oauthLabel: labels.oauth, apiKeyLabel: labels.key, icon });

/** Main list, in 老无's order: same vendor adjacent, every provider its own row. */
export const LOCAL_AUTH_MAIN: readonly LocalAuthProvider[] = [
  p('openai', 'OpenAI', ['oauth', 'api_key'], { oauth: 'Sign in with ChatGPT', key: 'OpenAI API key' }, openaiIcon),
  p('openai-codex', 'OpenAI Codex (legacy)', ['oauth'], { oauth: 'OpenAI (ChatGPT Plus/Pro)' }, openaiIcon),
  p('anthropic', 'Anthropic', ['oauth', 'api_key'], { oauth: 'Anthropic (Claude Pro/Max)', key: 'Anthropic API key' }, claudeIcon),
  p('kimi-coding', 'Kimi For Coding', ['oauth', 'api_key'], { oauth: 'Sign in with Kimi Code', key: 'Kimi API key' }, kimiIcon),
  p('moonshotai', 'Moonshot AI', ['api_key'], { key: 'Moonshot AI API key' }),
  p('moonshotai-cn', 'Moonshot AI CN', ['api_key'], { key: 'Moonshot AI API key' }),
  p('zai', 'Z.AI', ['api_key'], { key: 'Z.AI API key' }),
  p('zai-coding-cn', 'Z.AI Coding CN', ['api_key'], { key: 'Z.AI Coding CN API key' }),
  p('minimax', 'MiniMax', ['api_key'], { key: 'MiniMax API key' }),
  p('minimax-cn', 'MiniMax CN', ['api_key'], { key: 'MiniMax CN API key' }),
];
/** More providers: OpenRouter first, then companies grouped and ordered by how well known they are. */
export const LOCAL_AUTH_MORE: readonly LocalAuthProvider[] = [
  p('openrouter', 'OpenRouter', ['oauth', 'api_key'], { oauth: 'Sign in with OpenRouter', key: 'OpenRouter API key' }),
  p('google', 'Google', ['api_key'], { key: 'Gemini API key' }, geminiIcon),
  p('google-vertex', 'Google Vertex AI', ['api_key'], { key: 'Google Cloud credentials' }),
  p('github-copilot', 'GitHub Copilot', ['oauth', 'api_key'], { oauth: 'GitHub Copilot', key: 'GitHub Copilot token' }, githubIcon),
  p('azure', 'Azure OpenAI', ['api_key'], { key: 'Azure OpenAI API key' }),
  p('amazon-bedrock', 'Amazon Bedrock', ['api_key'], { key: 'AWS credentials or bearer token' }),
  p('meta', 'Meta', ['oauth', 'api_key'], { oauth: 'Sign in with Meta', key: 'Meta Model API key' }),
  p('xai', 'xAI', ['oauth', 'api_key'], { oauth: 'Sign in with SuperGrok or X Premium', key: 'xAI API key' }),
  p('nvidia', 'NVIDIA', ['api_key'], { key: 'NVIDIA API key' }),
  p('deepseek', 'DeepSeek', ['api_key'], { key: 'DeepSeek API key' }),
  p('qwen-token-plan', 'Qwen Token Plan', ['api_key'], { key: 'Qwen Token Plan API key' }),
  p('qwen-token-plan-individual', 'Qwen Token Plan Individual', ['api_key'], { key: 'Qwen Token Plan Individual API key' }),
  p('qwen-token-plan-cn', 'Qwen Token Plan CN', ['api_key'], { key: 'Qwen Token Plan CN API key' }),
  p('mistral', 'Mistral', ['api_key'], { key: 'Mistral API key' }),
  p('xiaomi', 'Xiaomi', ['api_key'], { key: 'Xiaomi API key' }),
  p('xiaomi-token-plan-cn', 'Xiaomi Token Plan CN', ['api_key'], { key: 'Xiaomi Token Plan CN API key' }),
  p('xiaomi-token-plan-ams', 'Xiaomi Token Plan AMS', ['api_key'], { key: 'Xiaomi Token Plan AMS API key' }),
  p('xiaomi-token-plan-sgp', 'Xiaomi Token Plan SGP', ['api_key'], { key: 'Xiaomi Token Plan SGP API key' }),
  p('cloudflare-workers-ai', 'Cloudflare Workers AI', ['api_key'], { key: 'Cloudflare API key' }),
  p('cloudflare-ai-gateway', 'Cloudflare AI Gateway', ['api_key'], { key: 'Cloudflare API key' }),
  p('vercel-ai-gateway', 'Vercel AI Gateway', ['api_key'], { key: 'Vercel AI Gateway API key' }),
  p('huggingface', 'Hugging Face', ['api_key'], { key: 'Hugging Face token' }),
  p('groq', 'Groq', ['api_key'], { key: 'Groq API key' }),
  p('opencode', 'OpenCode Zen', ['api_key'], { key: 'OpenCode API key' }, opencodeIcon),
  p('opencode-go', 'OpenCode Go', ['api_key'], { key: 'OpenCode API key' }, opencodeIcon),
  p('together', 'Together', ['api_key'], { key: 'Together API key' }),
  p('fireworks', 'Fireworks', ['api_key'], { key: 'Fireworks API key' }),
  p('cerebras', 'Cerebras', ['api_key'], { key: 'Cerebras API key' }),
  p('baseten', 'Baseten', ['api_key'], { key: 'Baseten API key' }),
  p('ant-ling', 'Ant Ling', ['api_key'], { key: 'Ant Ling API key' }),
  p('typesafe', 'TypeSafe', ['api_key'], { key: 'TypeSafe API key' }),
  p('radius', 'Radius', ['oauth', 'api_key'], { oauth: 'Radius', key: 'Radius API key' }),
];
export const LOCAL_AUTH_PROVIDERS: readonly LocalAuthProvider[] = [...LOCAL_AUTH_MAIN, ...LOCAL_AUTH_MORE];
/** Jev placeholder (next package): 老无's order, switchable. */
export const LOCAL_JEV_PROVIDERS = ['openrouter', 'vercel-ai-gateway', 'opencode', 'typesafe'] as const;

export const localAuthProvider = (id: string) => LOCAL_AUTH_PROVIDERS.find((provider) => provider.id === id);

/** Two-letter tile for providers without a Kota icon asset (Microsoft never collides with Mistral here). */
export function localAuthMonogram(name: string): string {
  const words = name.match(/[A-Z]?[a-z]+|[A-Z]+(?![a-z])|\d+/g) ?? [name];
  return (words[0][0] + (words.length > 1 ? words[1][0] : words[0].slice(1, 2))).toUpperCase();
}
