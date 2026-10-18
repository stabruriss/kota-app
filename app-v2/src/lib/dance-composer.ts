import type { ComposerTemporalGap, TemporalContextPreparedPrompt, VioletChatMessage } from '../pty-client';
import { buildDanceRoster, routeDance } from './puppeteer-dance';
import type { DanceAgent } from './dance-settings';

export interface DanceComposerDraft {
  text: string;
  attachments: Array<{ name: string; path?: string | null }>;
}
export interface DanceDelivery { messageId: string; timestamp: string; prepared: TemporalContextPreparedPrompt }
export async function prepareDanceDelivery(projectRoot: string, payload: string, draft: DanceComposerDraft,
  seats: readonly DanceAgent[], messages: readonly VioletChatMessage[]): Promise<DanceDelivery> {
  const messageId = crypto.randomUUID(); const timestamp = new Date().toISOString();
  const result = await routeDance(projectRoot, { messageId, timestamp, payload, text: draft.text,
    attachments: draft.attachments.slice(0, 8), links: [...new Set(draft.text.match(/https?:\/\/[^\s<>]+/g) ?? [])].slice(0, 8),
    pathsMentioned: [...new Set(draft.text.match(/(?:^|\s)(\/[^\s<>]+)/g)?.map(path => path.trim()) ?? [])].slice(0, 8),
    roster: buildDanceRoster(seats, messages),
  });
  if (!result.recorded || result.messageId !== messageId || !result.prepared || result.prepared.targetAgentId !== result.targetAgentId) throw { code: 'dance_record_failed' };
  return { messageId, timestamp, prepared: result.prepared };
}
/** Durable record contains the unchanged payload and the exact first gap prefix.
 * Rebuild after a room/App remount without classifying or preparing again. */
export function restoreDanceDelivery(messageId: string, timestamp: string, targetAgentId: string, payload: string,
  gap?: ComposerTemporalGap | null): DanceDelivery {
  return { messageId, timestamp, prepared: { targetAgentId, payload: `${gap?.prompt ?? ''}${payload}`, temporalGap: gap } };
}
