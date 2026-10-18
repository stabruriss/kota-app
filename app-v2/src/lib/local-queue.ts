import { readVioletRoomCache } from '../pty-client';
import { violetComposerSentHistory } from '../chrome/violet-room-events';
import type { LocalQueueItem, LocalScope } from './local-rpc';

export const EMPTY_LOCAL_QUEUE_GUARDS: ReadonlySet<string> = new Set();
export const localQueueGuardKey = (projectRoot: string, messageId: string, target: string) =>
  JSON.stringify([projectRoot, messageId, target]);
export const localQueueMessageId = (kotaId: string, target: string) =>
  kotaId.endsWith(`#${target}`) ? kotaId.slice(0, -target.length - 1) : null;
export const localQueueCanEdit = (item: LocalQueueItem) => item.customType === 'kota.user' || item.customType === 'kota.lm';
export interface LocalQueueOriginal { text: string; privacy: boolean }

/** Read the human's complete original, never the clipped queue projection. */
export async function readLocalQueueOriginal(scope: LocalScope, item: LocalQueueItem): Promise<LocalQueueOriginal | null> {
  const id = localQueueMessageId(item.kotaId, scope.agentId);
  if (!id || !localQueueCanEdit(item)) return null;
  if (item.customType === 'kota.user') {
    const original = violetComposerSentHistory(scope.projectRoot).find(m => m.id === id && m.targetAgentIds.includes(scope.agentId));
    // Room caches do not retain Composer privacy; never infer it from the text.
    return original ? { text: original.text, privacy: original.privacy } : null;
  }
  const room = await readVioletRoomCache({ projectRoot: scope.projectRoot, limit: 200 });
  const original = room.messages.find(m => (m.id === id || m.nativeEventId === id || m.nativeEventId === item.kotaId)
    && m.agentId === 'laughing-man' && m.actorIntent === 'telegram'
    && (m.agentId === scope.agentId || m.targetAgentIds?.includes(scope.agentId)));
  return original ? { text: original.text, privacy: false } : null;
}
