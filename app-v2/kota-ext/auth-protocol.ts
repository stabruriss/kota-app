/** Private controller/UI wire types. Never used as model messages or tools. */
export const AUTH_CODEC_PREFIX = 'kota.auth.v1:';
export const AUTH_LOGIN_TIMEOUT_MS = 600_000;
export const AUTH_CONTROL_TIMEOUT_MS = 30_000;
export const AUTH_CANCEL_GRACE_MS = 5_000;
export const THINKING_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const;
export const AUTH_OPERATIONS = ['login', 'status', 'logout', 'cancel', 'refresh', 'catalog', 'resolve'] as const;
export type AuthOperation = typeof AUTH_OPERATIONS[number];
export type AuthType = 'api_key' | 'oauth';
export type ThinkingLevel = typeof THINKING_LEVELS[number];
export type AuthCommand = {
  schema: 1;
  operationId: string;
  operation: AuthOperation;
  providerId?: string;
  authType?: AuthType;
  uiSchema?: 1;
  targetOperationId?: string;
  resourceUrl?: string;
  modelId?: string;
  thinkingLevel?: ThinkingLevel;
};

/** One root object, not a root union. Operation-specific checks are below. */
export const AUTH_COMMAND_SCHEMA = {
  type: 'object', additionalProperties: false, required: ['schema', 'operationId', 'operation'],
  properties: {
    schema: { type: 'integer', const: 1 },
    operationId: { type: 'string', minLength: 1, maxLength: 128 },
    operation: { type: 'string', enum: AUTH_OPERATIONS },
    providerId: { type: 'string', minLength: 1, maxLength: 128 },
    authType: { type: 'string', enum: ['api_key', 'oauth'] },
    uiSchema: { type: 'integer', const: 1 },
    targetOperationId: { type: 'string', minLength: 1, maxLength: 128 },
    resourceUrl: { type: 'string', minLength: 1, maxLength: 4096 },
    modelId: { type: 'string', minLength: 1, maxLength: 1024 },
    thinkingLevel: { type: 'string', enum: THINKING_LEVELS },
  },
} as const;

const operationFields: Record<AuthOperation, readonly string[]> = {
  login: ['providerId', 'authType', 'uiSchema', 'resourceUrl'],
  status: ['providerId'], logout: ['providerId'], cancel: ['targetOperationId'],
  refresh: [], catalog: ['providerId'], resolve: ['providerId', 'modelId', 'thinkingLevel'],
};
const boundedString = (value: unknown, max: number): value is string =>
  typeof value === 'string' && value.length > 0 && value.length <= max &&
  !/[\u0000-\u001f\u007f]/u.test(value) && value.trim().length > 0;
export function validResourceUrl(value: unknown): value is string {
  if (!boundedString(value, 4096)) return false;
  try {
    const url = new URL(value);
    return (url.protocol === 'https:' || url.protocol === 'http:') &&
      url.hostname.length > 0 && !url.username && !url.password;
  } catch { return false; }
}

/** No partial IDs are recovered from malformed input. No credentials belong in this object. */
export function parseAuthCommand(text: string): AuthCommand | undefined {
  if (Buffer.byteLength(text, 'utf8') > 16_384) return undefined;
  let value: unknown;
  try { value = JSON.parse(text); } catch { return undefined; }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined;
  const object = value as Record<string, unknown>;
  if (object.schema !== 1 || !boundedString(object.operationId, 128) ||
      !AUTH_OPERATIONS.includes(object.operation as AuthOperation)) return undefined;
  const operation = object.operation as AuthOperation;
  const allowed = new Set(['schema', 'operationId', 'operation', ...operationFields[operation]]);
  if (Object.keys(object).some(key => !allowed.has(key))) return undefined;
  if ('providerId' in object && !boundedString(object.providerId, 128)) return undefined;
  if (operation === 'login') {
    if (!boundedString(object.providerId, 128) || object.uiSchema !== 1 ||
        (object.authType !== 'api_key' && object.authType !== 'oauth')) return undefined;
    const azure = object.providerId === 'azure' && object.authType === 'api_key';
    if (azure ? !validResourceUrl(object.resourceUrl) : 'resourceUrl' in object) return undefined;
  }
  if (operation === 'logout' && !boundedString(object.providerId, 128)) return undefined;
  if (operation === 'cancel' && !boundedString(object.targetOperationId, 128)) return undefined;
  if (operation === 'resolve' && (!boundedString(object.providerId, 128) ||
      !boundedString(object.modelId, 1024) || !THINKING_LEVELS.includes(object.thinkingLevel as ThinkingLevel))) return undefined;
  return object as AuthCommand;
}

export type ProviderCapabilities = {
  providerId: string; providerLabel: string; canApiKey: boolean; canOAuth: boolean;
};
export type AuthStatus = ProviderCapabilities & {
  storedType: AuthType | null; configured: boolean;
  policy: 'allowed' | 'blocked' | 'unknown';
  validity: 'not-checked' | 'invalid' | 'unknown';
};
export type AuthCatalog = {
  providers: Array<ProviderCapabilities & {
    models: Array<{ modelId: string; name: string; thinkingLevels: string[] }>;
  }>;
};
export type AuthResolved = {
  providerId: string; modelId: string; requestedThinkingLevel: string; thinkingLevel: string;
};
export type AuthPrompt =
  | { type: 'text' | 'secret' | 'manual_code'; message: string; placeholder?: string }
  | { type: 'select'; message: string; options: ReadonlyArray<{ id: string; label: string; description?: string }> };
export type AuthNotification =
  | { type: 'info'; message: string; links?: ReadonlyArray<{ url: string; label?: string }> }
  | { type: 'auth_url'; url: string; instructions?: string }
  | { type: 'device_code'; userCode: string; verificationUri: string; intervalSeconds?: number; expiresInSeconds?: number }
  | { type: 'progress'; message: string };
export type AuthOutcome = 'succeeded' | 'failed' | 'cancelled' | 'timeout' | 'interrupted' |
  'committed-unsynchronized' | 'indeterminate';
export type AuthCommit = 'confirmed' | 'not-written' | 'unknown';
export type AuthCode = 'invalid-request' | 'unsupported-provider' | 'unsupported-auth' | 'busy-auth' |
  'auth-failed' | 'storage-failed' | 'credential-sync-failed' | 'runtime-exited' | 'source-policy' |
  'model-not-found' | 'thinking-invalid';
export type PromptCloseReason = 'answered' | 'cancelled' | 'timeout' | 'step-aborted' | 'interrupted';
type AuthBase = { schema: 1; channel: 'kota.auth'; operationId: string };
export type AuthEvent = AuthBase & (
  | { event: 'started'; operation: AuthOperation; providerId?: string }
  | { event: 'prompt'; providerId: string; authType: AuthType; promptId: string;
      credentialField?: 'api_key.key'; prompt: AuthPrompt }
  | { event: 'auth_event'; providerId: string; notification: AuthNotification }
  | { event: 'prompt_closed'; providerId: string; promptId: string; reason: PromptCloseReason }
  | { event: 'result'; operation: AuthOperation; providerId?: string; outcome: AuthOutcome; commit: AuthCommit;
      code?: AuthCode; status?: AuthStatus; statuses?: AuthStatus[]; catalog?: AuthCatalog; resolved?: AuthResolved }
);

function capabilities(value: ProviderCapabilities): ProviderCapabilities {
  return { providerId: value.providerId, providerLabel: value.providerLabel,
    canApiKey: value.canApiKey, canOAuth: value.canOAuth };
}
function status(value: AuthStatus): AuthStatus {
  return { ...capabilities(value), storedType: value.storedType, configured: value.configured,
    policy: value.policy, validity: value.validity };
}
function prompt(value: AuthPrompt): AuthPrompt {
  if (value.type === 'select') return { type: value.type, message: value.message,
    options: value.options.map(option => ({ id: option.id, label: option.label, description: option.description })) };
  return { type: value.type, message: value.message, placeholder: value.placeholder };
}
function notification(value: AuthNotification): AuthNotification {
  switch (value.type) {
    case 'info': return { type: value.type, message: value.message,
      links: value.links?.map(link => ({ url: link.url, label: link.label })) };
    case 'progress': return { type: value.type, message: value.message };
    case 'auth_url': return { type: value.type, url: value.url, instructions: value.instructions };
    case 'device_code': return { type: value.type, userCode: value.userCode,
      verificationUri: value.verificationUri, intervalSeconds: value.intervalSeconds, expiresInSeconds: value.expiresInSeconds };
  }
}

/** Construct a whitelist even when callers accidentally pass additional properties. */
export function encodeAuthEvent(value: AuthEvent): string {
  if (value.event === 'prompt' && value.credentialField !== undefined &&
      (value.credentialField !== 'api_key.key' || value.authType !== 'api_key' || value.prompt.type !== 'secret')) {
    throw new Error('invalid-auth-metadata');
  }
  const base: AuthBase = { schema: 1, channel: 'kota.auth', operationId: value.operationId };
  let event: AuthEvent;
  switch (value.event) {
    case 'started': event = { ...base, event: value.event, operation: value.operation, providerId: value.providerId }; break;
    case 'prompt': event = { ...base, event: value.event, providerId: value.providerId, authType: value.authType,
      promptId: value.promptId, credentialField: value.credentialField, prompt: prompt(value.prompt) }; break;
    case 'auth_event': event = { ...base, event: value.event, providerId: value.providerId,
      notification: notification(value.notification) }; break;
    case 'prompt_closed': event = { ...base, event: value.event, providerId: value.providerId,
      promptId: value.promptId, reason: value.reason }; break;
    case 'result': event = { ...base, event: value.event, operation: value.operation, providerId: value.providerId,
      outcome: value.outcome, commit: value.commit, code: value.code,
      status: value.status && status(value.status), statuses: value.statuses?.map(status),
      catalog: value.catalog && { providers: value.catalog.providers.map(provider => ({ ...capabilities(provider),
        models: provider.models.map(model => ({ modelId: model.modelId, name: model.name,
          thinkingLevels: [...model.thinkingLevels] })) })) },
      resolved: value.resolved && { providerId: value.resolved.providerId, modelId: value.resolved.modelId,
        requestedThinkingLevel: value.resolved.requestedThinkingLevel, thinkingLevel: value.resolved.thinkingLevel } }; break;
  }
  return AUTH_CODEC_PREFIX + JSON.stringify(event);
}
