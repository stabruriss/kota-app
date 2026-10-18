export const CUSTOM_TYPES = [
  'kota.user', 'kota.lm', 'kota.bus', 'kota.bbs', 'kota.ember', 'kota.dream', 'kota.bartender', 'kota.shadow_clone',
] as const;
export const MAX_INJECTION_BYTES = 16 * 1024 * 1024;

export type InjectionContent = string | (
  | { type: 'text'; text: string }
  | { type: 'image'; data: string; mimeType: string }
)[];
export interface InjectionDetails {
  schema: 1;
  messageId: string;
  targetAgentId: string;
  createdAt: string;
  private: boolean;
  sourceActorId?: string;
  sourceIntent?: string;
}
export interface InjectionEnvelope {
  kotaId: string;
  customType: typeof CUSTOM_TYPES[number];
  content: InjectionContent;
  details: InjectionDetails;
}

type ErrorCode = 'invalid-json' | 'invalid-envelope' | 'invalid-id' | 'invalid-details' | 'invalid-content' | 'too-large';
const isObject = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
const component = (value: unknown, maximum: number): value is string => typeof value === 'string' && value.length <= maximum && /^[A-Za-z0-9._:-]+$/.test(value);
const completeId = (value: unknown): value is string => {
  if (typeof value !== 'string') return false;
  const parts = value.split('#');
  return parts.length === 2 && component(parts[0], 256) && component(parts[1], 128);
};
const exactKeys = (value: Record<string, unknown>, required: string[], optional: string[] = []) =>
  required.every(key => Object.hasOwn(value, key)) && Object.keys(value).every(key => required.includes(key) || optional.includes(key));

function failed(code: ErrorCode, kotaId?: unknown): never {
  throw new Error(`kota-inject-failed kotaId=${completeId(kotaId) ? encodeURIComponent(kotaId) : 'unknown'} code=${code}`);
}

function validTimestamp(value: unknown): boolean {
  if (typeof value !== 'string') return false;
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d+)?(?:Z|([+-])(\d{2}):(\d{2}))$/i.exec(value);
  if (!match) return false;
  const [, year, month, day, hour, minute, second, , offsetHour = '0', offsetMinute = '0'] = match;
  const leapYear = Number(year) % 4 === 0 && (Number(year) % 100 !== 0 || Number(year) % 400 === 0);
  const days = [31, leapYear ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  return Number(month) >= 1 && Number(month) <= 12 && Number(day) >= 1 && Number(day) <= days[Number(month) - 1]
    && Number(hour) <= 23 && Number(minute) <= 59 && Number(second) <= 60
    && Number(offsetHour) <= 23 && Number(offsetMinute) <= 59;
}

function validContent(value: unknown): boolean {
  if (typeof value === 'string') return value.length > 0;
  if (!Array.isArray(value) || value.length === 0) return false;
  return value.every(block => {
    if (!isObject(block)) return false;
    if (block.type === 'text') return exactKeys(block, ['type', 'text']) && typeof block.text === 'string';
    return block.type === 'image' && exactKeys(block, ['type', 'data', 'mimeType'])
      && typeof block.data === 'string' && block.data.length > 0
      // Round-trip validation is linear and avoids a regex stack overflow on multi-MiB images.
      && Buffer.from(block.data, 'base64').toString('base64') === block.data
      && typeof block.mimeType === 'string' && ['image/png', 'image/jpeg', 'image/webp', 'image/gif'].includes(block.mimeType);
  });
}

/** Synchronous validation only. Error payloads never contain input text or arbitrary exceptions. */
export function parseInjection(raw: string): InjectionEnvelope {
  if (Buffer.byteLength(raw, 'utf8') > MAX_INJECTION_BYTES) failed('too-large');
  let input: unknown;
  try { input = JSON.parse(raw); }
  catch { failed('invalid-json'); }
  if (!isObject(input)) failed('invalid-envelope');
  const id = input.kotaId;
  if (!exactKeys(input, ['kotaId', 'customType', 'content', 'details'])) failed('invalid-envelope', id);
  if (!completeId(id)) failed('invalid-id');
  if (typeof input.customType !== 'string' || !(CUSTOM_TYPES as readonly string[]).includes(input.customType)) failed('invalid-envelope', id);
  const details = input.details;
  if (!isObject(details) || !exactKeys(details, ['schema', 'messageId', 'targetAgentId', 'createdAt', 'private'], ['sourceActorId', 'sourceIntent'])
    || details.schema !== 1 || !component(details.messageId, 256) || !component(details.targetAgentId, 128)
    || !validTimestamp(details.createdAt) || typeof details.private !== 'boolean'
    || (Object.hasOwn(details, 'sourceActorId') && typeof details.sourceActorId !== 'string')
    || (Object.hasOwn(details, 'sourceIntent') && typeof details.sourceIntent !== 'string')) failed('invalid-details', id);
  if (id !== `${details.messageId}#${details.targetAgentId}`) failed('invalid-id', id);
  if (!validContent(input.content)) failed('invalid-content', id);
  return input as unknown as InjectionEnvelope;
}

/** Internal queue command; no body or unknown ID is inferred. */
export function parseCancelId(raw: string): string {
  const id = raw.trim();
  if (!completeId(id)) throw new Error('kota-cancel-failed kotaId=unknown code=invalid-id');
  return id;
}

/** Private Stop handshake, never a model message. */
export const QUEUE_CONTROL_CODEC = 'kota.queue.v1:';
export function parseQueueControl(raw: string): string {
  let value: unknown;
  try { value = JSON.parse(raw); } catch { throw new Error('Invalid Kota queue control.'); }
  if (!isObject(value) || !exactKeys(value, ['requestId']) || !component(value.requestId, 128)) {
    throw new Error('Invalid Kota queue control.');
  }
  return value.requestId;
}

export function parseQueueReorder(raw: string): { requestId: string; ids: string[] } {
  const value: unknown = JSON.parse(raw);
  if (!isObject(value) || !exactKeys(value, ['requestId', 'ids']) || !component(value.requestId, 128)
    || !Array.isArray(value.ids) || value.ids.length < 2 || value.ids.length > 64
    || !value.ids.every(completeId) || new Set(value.ids).size !== value.ids.length) {
    throw new Error('Invalid Kota queue reorder.');
  }
  return { requestId: value.requestId, ids: value.ids };
}
