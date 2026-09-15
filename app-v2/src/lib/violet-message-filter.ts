import type { VioletChatMessage } from '../pty-client';

/** Display-only selector. Keep the original messages for delivery tracking and caches. */
export function filterAgentBusMessages<T extends VioletChatMessage>(
  messages: readonly T[],
  showAgentToAgentMessages: boolean,
): readonly T[] {
  if (showAgentToAgentMessages) return messages;
  // Actor-side Agent Bus IDs are distinct from Ember, BBS and Telegram IDs.
  // Do not infer the source from intent or user-visible message text.
  return messages.filter((message) => !(
    message.shell === 'system' && message.nativeEventId?.startsWith('agentbus-')
  ));
}
