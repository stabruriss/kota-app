import type { VioletChatMessage } from '../pty-client';
import { syncVioletProjectAgentsNow } from '../lib/violet-sync-engine';
import {
  prepareDedupeText,
  preparedDedupeTextsMatch,
  timestampsWithinComposerConfirmationWindow,
} from '../lib/violet-message-dedupe';
import {
  emitVioletComposerDelivery,
  VIOLET_COMPOSER_AGENT_EXIT_REASON,
  violetComposerSentHistory,
  type VioletComposerSentDetail,
} from './violet-room-events';

const EXIT_RECONCILES = new Map<string, Promise<void>>();

export interface VioletComposerAgentExit {
  projectRoot: string | null;
  agentId: string;
}

export function reconcileVioletComposerAfterAgentExit(
  exit: VioletComposerAgentExit,
): Promise<void> {
  const key = `${normalizeProjectRoot(exit.projectRoot) ?? ''}\u001f${exit.agentId}`;
  const existing = EXIT_RECONCILES.get(key);
  if (existing) return existing;
  const pending = reconcileAfterAgentExit(exit).finally(() => {
    if (EXIT_RECONCILES.get(key) === pending) EXIT_RECONCILES.delete(key);
  });
  EXIT_RECONCILES.set(key, pending);
  return pending;
}

async function reconcileAfterAgentExit(exit: VioletComposerAgentExit): Promise<void> {
  const candidates = unresolvedExitCandidates(exit);
  if (candidates.length === 0) return;

  const targetAgentIds = uniqueIds(candidates.flatMap((message) => message.targetAgentIds));
  const state = await syncVioletProjectAgentsNow(exit.projectRoot, targetAgentIds);
  const nativeUserMessages = state.messages.filter(isNativeUserPrompt);

  for (const candidate of candidates) {
    // Delivery may have settled while the final native-log sync was in flight.
    const current = violetComposerSentHistory(exit.projectRoot)
      .find((message) => message.id === candidate.id);
    if (!current || current.delivery) continue;

    const currentTargetAgentIds = uniqueIds(current.targetAgentIds);
    const confirmedTargetAgentIds = new Set(currentTargetAgentIds.filter((agentId) => (
      hasNativeUserEvidence(current, agentId, nativeUserMessages)
    )));
    if (confirmedTargetAgentIds.size === currentTargetAgentIds.length) {
      emitVioletComposerDelivery({ id: current.id, status: 'clear' });
      continue;
    }
    if (confirmedTargetAgentIds.has(exit.agentId)) continue;

    // Composer delivery is currently message-level. Only accelerate a group
    // message when every peer is already strongly confirmed; otherwise keep
    // the original 180-second reconciliation for the whole pending group.
    const allPeersConfirmed = currentTargetAgentIds.every((agentId) => (
      agentId === exit.agentId || confirmedTargetAgentIds.has(agentId)
    ));
    if (!allPeersConfirmed) continue;

    emitVioletComposerDelivery({
      id: current.id,
      status: 'unconfirmed',
      reason: VIOLET_COMPOSER_AGENT_EXIT_REASON,
      retryTargetAgentIds: [exit.agentId],
    });
  }
}

function unresolvedExitCandidates(exit: VioletComposerAgentExit): VioletComposerSentDetail[] {
  return violetComposerSentHistory(exit.projectRoot).filter((message) => (
    !message.delivery &&
    message.targetAgentIds.includes(exit.agentId)
  ));
}

function hasNativeUserEvidence(
  message: VioletComposerSentDetail,
  agentId: string,
  nativeMessages: readonly VioletChatMessage[],
): boolean {
  const localText = prepareDedupeText(message.text);
  const localTime = Date.parse(message.timestamp);
  return nativeMessages.some((nativeMessage) => (
    nativeMessage.agentId === agentId &&
    preparedDedupeTextsMatch(prepareDedupeText(nativeMessage.text), localText) &&
    timestampsWithinComposerConfirmationWindow(Date.parse(nativeMessage.timestamp), localTime)
  ));
}

function isNativeUserPrompt(message: VioletChatMessage): boolean {
  return message.role === 'user' && message.kind === 'message' && message.agentId !== 'user';
}

function uniqueIds(values: readonly string[]): string[] {
  return Array.from(new Set(values.filter(Boolean))).sort();
}

function normalizeProjectRoot(projectRoot?: string | null): string | null {
  const trimmed = projectRoot?.trim();
  return trimmed ? trimmed : null;
}
