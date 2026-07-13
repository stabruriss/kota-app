export const VIOLET_COMPOSER_SENT_EVENT = 'kota:violet-composer-sent';
export const VIOLET_COMPOSER_DELIVERY_EVENT = 'kota:violet-composer-delivery';
const VIOLET_COMPOSER_SENT_HISTORY_LIMIT = 160;
const VIOLET_COMPOSER_SENT_HISTORY: VioletComposerSentDetail[] = [];

export interface VioletComposerSentDetail {
  id: string;
  projectRoot?: string | null;
  text: string;
  targetAgentIds: string[];
  mentions?: { agentId: string; aka: string }[];
  timestamp: string;
  privacy: boolean;
}

export interface VioletComposerDeliveryDetail {
  id: string;
  status: 'failed' | 'unconfirmed' | 'retrying' | 'clear';
  reason?: string;
  retryTargetAgentIds?: string[];
}

export function violetComposerSentHistory(projectRoot?: string | null): VioletComposerSentDetail[] {
  if (projectRoot === undefined) return [...VIOLET_COMPOSER_SENT_HISTORY];
  const requestedRoot = normalizeProjectRoot(projectRoot);
  return VIOLET_COMPOSER_SENT_HISTORY.filter((item) => (
    normalizeProjectRoot(item.projectRoot) === requestedRoot
  ));
}

export function lastVioletComposerSentAt(projectRoot?: string | null): string | null {
  return violetComposerSentHistory(projectRoot).at(-1)?.timestamp ?? null;
}

export function __clearVioletComposerSentHistoryForTests() {
  VIOLET_COMPOSER_SENT_HISTORY.splice(0);
}

export function emitVioletComposerSent(
  detail: Omit<VioletComposerSentDetail, 'id' | 'timestamp'>,
): VioletComposerSentDetail | null {
  if (typeof window === 'undefined') return null;
  const id = typeof crypto !== 'undefined' && 'randomUUID' in crypto
    ? crypto.randomUUID()
    : `violet-local-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  const next: VioletComposerSentDetail = {
    ...detail,
    projectRoot: normalizeProjectRoot(detail.projectRoot),
    id,
    timestamp: new Date().toISOString(),
  };
  VIOLET_COMPOSER_SENT_HISTORY.push(next);
  if (VIOLET_COMPOSER_SENT_HISTORY.length > VIOLET_COMPOSER_SENT_HISTORY_LIMIT) {
    VIOLET_COMPOSER_SENT_HISTORY.splice(
      0,
      VIOLET_COMPOSER_SENT_HISTORY.length - VIOLET_COMPOSER_SENT_HISTORY_LIMIT,
    );
  }
  window.dispatchEvent(new CustomEvent<VioletComposerSentDetail>(VIOLET_COMPOSER_SENT_EVENT, {
    detail: next,
  }));
  return next;
}

export function emitVioletComposerDelivery(detail: VioletComposerDeliveryDetail) {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new CustomEvent<VioletComposerDeliveryDetail>(VIOLET_COMPOSER_DELIVERY_EVENT, {
    detail,
  }));
}

function normalizeProjectRoot(projectRoot?: string | null): string | null {
  const trimmed = projectRoot?.trim();
  return trimmed ? trimmed : null;
}
