import { invoke } from '@tauri-apps/api/core';
import type { TemporalContextPreparedPrompt, VioletChatMessage } from '../pty-client';
import { hasLeadingTemporalGap, prepareComposerDeliveryDedupeText, stripLeadingTemporalGap } from './violet-message-dedupe';

export interface DanceSettings {
  schema: 1; enabled: boolean; sentinel: string | null; strategy: 'context' | 'rule';
  threshold: number; conditions: Record<string, string>;
}
export interface DanceTimedText { at: string; text: string }
export interface DanceRosterItem {
  id: string; name: string; lastReply: DanceTimedText | null; recentUserMessages: DanceTimedText[];
  /** Private same-second tie-break, excluded from the Jev request body. */
  lastReplyOrder?: number;
}
export interface DanceRouteRequest {
  messageId: string; timestamp: string;
  /** The unchanged ordinary delivery payload, including attachment references. */
  payload: string;
  /** Clean draft for classification. Omission uses attachment-insensitive payload text. */
  text?: string;
  attachments?: Array<{ name: string; path?: string | null; mimeType?: string | null; size?: number | null }>;
  links?: string[]; pathsMentioned?: string[]; roster: DanceRosterItem[];
}
export interface DanceRouted {
  messageId: string; targetAgentId: string; fallback: boolean; recorded: true;
  /** Reuse verbatim for Retry; never run route or ordinary temporal preparation again. */
  prepared?: TemporalContextPreparedPrompt;
}
export interface DanceSettingsSnapshot { settings: DanceSettings; sourceRevision: string; currentSentinel: string | null }
export const readDance = (projectRoot: string) => invoke<DanceSettingsSnapshot>('puppeteer_dance_read', { projectRoot });
export const saveDance = (projectRoot: string, settings: DanceSettings, sourceRevision: string) => invoke<DanceSettingsSnapshot>('puppeteer_dance_save', { projectRoot, settings, sourceRevision });
export const routeDance = (projectRoot: string, request: DanceRouteRequest) => invoke<DanceRouted>('puppeteer_dance_route', { projectRoot, request });

const clip = (text: string, n: number) => Array.from(text).slice(0, n).join('');
type ContextMessage = VioletChatMessage & { private?: boolean };
type Recent = { message: ContextMessage; time: number; index: number };
const timed = ({ message }: Recent): DanceTimedText => ({ at: message.timestamp, text: clip(stripLeadingTemporalGap(message.text), 200) });

/** Bounded context from already-loaded/merged room rows; no history, listener or polling.
 * Handles Puppeteer and ordinary temporal-gap echoes here, not in the caller.
 * Echo pairs must be in the room's chronological order (as for withoutDanceNativeEchoes).
 * Call only when Dance is enabled; seated agents with no recent messages remain in the result.
 */
export function buildDanceRoster(seats: readonly { id: string; name: string }[], messages: readonly ContextMessage[]): DanceRosterItem[] {
  const ids = new Set(seats.slice(0, 8).map(s => s.id));
  // The room orders the whole second by sequence when any timestamp is coarse.
  // Preserve that causal order, rather than treating a second-only reply as earlier.
  const coarseSeconds = new Set(messages.filter(message => /T\d{2}:\d{2}:\d{2}(?:\.\d{1,2})?(?:Z|[+-]\d{2}:\d{2})$/i.test(message.timestamp)).map(message => Math.floor(Date.parse(message.timestamp) / 1000)));
  const later = (a: Recent, b: Recent) => Math.floor(a.time / 1000) === Math.floor(b.time / 1000) && coarseSeconds.has(Math.floor(a.time / 1000))
    ? a.index > b.index : a.time > b.time || (a.time === b.time && a.index > b.index);
  const replies = new Map<string, Recent>();
  const inputs = new Map<string, Recent[]>();
  withoutDanceNativeEchoes(messages).forEach((message, index) => {
    if (message.role === 'user' && message.agentId !== 'user' && hasLeadingTemporalGap(message.text)) return;
    if (message.private || message.kind !== 'message' || ['shell_handoff', 'subagent', 'shadow_clone', 'private'].includes(message.messageOrigin ?? '')) return;
    const time = Date.parse(message.timestamp);
    if (!Number.isFinite(time)) return;
    const recent = { message, time, index };
    if (message.role === 'assistant' && ids.has(message.agentId) && message.shell !== 'system' && !message.actorIntent) {
      const before = replies.get(message.agentId);
      if (!before || later(recent, before)) replies.set(message.agentId, recent);
      return;
    }
    const lm = message.agentId === 'laughing-man' && message.actorIntent === 'telegram';
    if (!lm && (message.role !== 'user' || message.actorIntent)) return;
    const targets = message.targetAgentIds?.length ? message.targetAgentIds : lm ? [] : [message.agentId];
    for (const target of targets) {
      if (!ids.has(target)) continue;
      const kept = inputs.get(target) ?? [];
      kept.push(recent); kept.sort((a,b) => later(a,b) ? -1 : later(b,a) ? 1 : 0); kept.length = Math.min(kept.length, 2);
      inputs.set(target, kept);
    }
  });
  return seats.slice(0, 8).map(seat => ({ id: seat.id, name: clip(seat.name, 64),
    lastReply: replies.has(seat.id) ? timed(replies.get(seat.id)!) : null,
    ...(replies.has(seat.id) ? { lastReplyOrder: replies.get(seat.id)!.index } : {}),
    recentUserMessages: (inputs.get(seat.id) ?? []).slice().reverse().map(timed),
  }));
}

type Echo = { id: string; index: number; time: number; used: boolean };
type Queue = { items: Echo[]; head: number };
/** Removes only native echoes of durable Puppeteer composer records. Local is exact-ID;
 * other providers consume the first unmatched later native user row, one-to-one, with
 * no 15-minute cutoff. Namespaced text indexes share the same consumed record token.
 * Input is the room's chronological order. Ordinary composer dedupe is untouched.
 */
export function withoutDanceNativeEchoes<T extends VioletChatMessage>(messages: readonly T[]): T[] {
  const exact = new Map<string, Echo>();
  const texts = new Map<string, Queue>();
  const result: T[] = [];
  const key = (agent: string, kind: string, text: string) => `${agent}\0${kind}\0${text}`;
  const add = (key: string, echo: Echo) => {
    const queue = texts.get(key) ?? { items: [], head: 0 }; queue.items.push(echo); texts.set(key, queue);
  };
  messages.forEach((message, index) => {
    if (message.messageOrigin === 'puppeteer' && message.role === 'user') {
      const prepared = prepareComposerDeliveryDedupeText(message.text);
      for (const agent of message.targetAgentIds ?? []) {
        const echo = { id: message.id, index, time: Date.parse(message.timestamp), used: false };
        exact.set(`${agent}\0${message.id}#${agent}`, echo);
        add(key(agent, 'text', prepared.normalized), echo);
        add(key(agent, 'attachment', prepared.attachmentInsensitive), echo);
      }
      result.push(message); return;
    }
    if (message.role !== 'user' || message.kind !== 'message' || message.actorIntent || (message.messageOrigin && message.messageOrigin !== 'user') || message.agentId === 'user') {
      result.push(message); return;
    }
    let match: Echo | undefined;
    if (message.shell === 'kota-local') {
      match = message.nativeEventId ? exact.get(`${message.agentId}\0${message.nativeEventId}`) : undefined;
    } else {
      const prepared = prepareComposerDeliveryDedupeText(message.text);
      const time = Date.parse(message.timestamp);
      if (prepared.normalized) {
        for (const candidate of [key(message.agentId, 'text', prepared.normalized), key(message.agentId, 'attachment', prepared.attachmentInsensitive)]) {
          const queue = texts.get(candidate);
          if (!queue) continue;
          while (queue.head < queue.items.length && queue.items[queue.head]!.used) queue.head++;
          const first = queue.items[queue.head];
          // Terminal logs may truncate milliseconds. Input order remains causal;
          // allow that same second only, not an earlier second or a fuzzy time window.
          const timeEnd = /T\d{2}:\d{2}:\d{2}(?:Z|[+-]\d{2}:\d{2})$/i.test(message.timestamp) ? time + 999 : time;
          if (first && first.index < index && first.time <= timeEnd && (!match || first.index < match.index)) match = first;
        }
      }
    }
    if (match && !match.used && match.index < index) { match.used = true; return; }
    result.push(message);
  });
  return result;
}
