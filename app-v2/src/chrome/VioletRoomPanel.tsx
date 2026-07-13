import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent, MouseEvent, ReactNode, UIEvent } from 'react';
import { createPortal } from 'react-dom';
import {
  agentBusRetryDelivery,
  fileImageDataUrl,
  readVioletRoomCache,
  onVioletRoomSynced,
  violetOpenFileRef,
  violetRevealFileRef,
  violetResolveFileRef,
  type AgentBusReceipt,
  type VioletChatMessage,
  type VioletFileRefResolveResult,
  type VioletRoomState,
  type ProjectAgentCommendSource,
  type ProjectAgentRecord,
} from '../pty-client';
import type { Agent, AgentId } from '../types/scene';
import { splitProjectAgentName } from './ProjectAgentName';
import {
  VIOLET_COMPOSER_DELIVERY_EVENT,
  VIOLET_COMPOSER_SENT_EVENT,
  violetComposerSentHistory,
  type VioletComposerDeliveryDetail,
  type VioletComposerSentDetail,
} from './violet-room-events';
import { ProjectAgentName } from './ProjectAgentName';
import { avatarClassForId, avatarImageStyleForId } from '../lib/hero-avatars';
import { AgentCommendButton } from './AgentCommendButton';

interface VioletRoomPanelProps {
  projectRoot?: string | null;
  agentIds?: readonly AgentId[];
  chatFilterActive?: boolean;
  chatFilterAgentIds?: readonly AgentId[];
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  agentRecords?: Readonly<Record<AgentId, ProjectAgentRecord>>;
  onAgentContextMenu?: (
    id: AgentId,
    point: { x: number; y: number },
    source?: ProjectAgentCommendSource,
  ) => void;
  onCommendAgent?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  onOpenAgentTerminal?: (id: AgentId) => void;
  onRetryComposerMessage?: (request: VioletComposerRetryRequest) => boolean | void | Promise<boolean | void>;
  onClose?: () => void;
}

export interface VioletComposerRetryRequest {
  text: string;
  targetAgentIds: AgentId[];
  privacy: boolean;
  mentions?: { agentId: AgentId; aka: string }[];
}

type VioletDeliveryStatus = 'failed' | 'unconfirmed' | 'retrying';

type VioletRoomMessage = VioletChatMessage & {
  local?: boolean;
  projectRoot?: string | null;
  targetAgentIds?: readonly string[];
  privacy?: boolean;
  composerMentions?: readonly { agentId: string; aka: string }[];
  deliveryStatus?: VioletDeliveryStatus;
  deliveryReason?: string;
  deliveryRetryTargetAgentIds?: readonly string[];
  progressItems?: readonly string[];
  progressEntries?: readonly VioletProgressEntry[];
  ghostSasayaki?: boolean;
  ghostSasayakiSortTime?: number;
};

type AgentBusDeliveryState = {
  status: VioletDeliveryStatus;
  reason?: string;
  retryTargetAgentIds: readonly string[];
  attemptEventIds: readonly string[];
  lastAttemptAt: number;
};

type VioletProgressEntry = {
  agentId: string;
  shell: string;
  timestamp: string;
  text: string;
  agentDisplayName?: string | null;
  agentAvatarId?: string | null;
  agentProvider?: string | null;
  agentStatus?: string | null;
};

type ScrollAnchor = {
  id: string;
  top: number;
};

type PrependScrollSnapshot = {
  scrollTop: number;
  scrollHeight: number;
};

const VIOLET_ROOM_STATE_CACHE = new Map<string, VioletRoomState>();
const VIOLET_ROOM_HISTORY_EXPANDED_CACHE_KEYS = new Set<string>();
const BROADCAST_USER_GROUP_WINDOW_MS = 5000;
const GHOST_SASAYAKI_WINDOW_MS = 8000;
const DELIVERY_CONFIRM_TIMEOUT_MS = 90000;
const DELIVERY_CONFIRM_OBSERVE_GRACE_MS = 15000;
// Agent Bus prompts can remain queued behind an active turn beyond the receipt timeout.
// Keep delivery tracking and retry wiring intact while hiding the misleading UI affordance.
const AGENT_BUS_RETRY_UI_ENABLED: boolean = false;
const VIOLET_ROOM_PAGE_SIZE = 30;
const VIOLET_ROOM_LIVE_LIMIT = 200;
const FILTER_AUTOFILL_PAGE_LIMIT = 4;
const VIOLET_TIME_FORMATTER = new Intl.DateTimeFormat(undefined, {
  hour: '2-digit',
  minute: '2-digit',
});

function useStableRoomMessages<T extends VioletChatMessage>(
  next: readonly T[],
  resetKey: string,
): T[] {
  const previousRef = useRef<readonly T[]>([]);
  const resetKeyRef = useRef(resetKey);
  const stable = useMemo(() => (
    reuseStableRoomMessages(resetKeyRef.current === resetKey ? previousRef.current : [], next)
  ), [next, resetKey]);

  useLayoutEffect(() => {
    resetKeyRef.current = resetKey;
    previousRef.current = stable;
  }, [resetKey, stable]);

  return stable;
}

export function VioletRoomPanel({
  projectRoot,
  agentIds = [],
  chatFilterActive,
  chatFilterAgentIds = [],
  agentMeta,
  agentRecords,
  onAgentContextMenu,
  onCommendAgent,
  onOpenAgentTerminal,
  onRetryComposerMessage,
  onClose,
}: VioletRoomPanelProps) {
  const agentIdsKey = agentIds.join('|');
  const roomCacheKey = `${projectRoot ?? ''}::all`;
  const [state, setState] = useState<VioletRoomState | null>(() => (
    VIOLET_ROOM_STATE_CACHE.get(roomCacheKey) ?? null
  ));
  const [localMessages, setLocalMessages] = useState<VioletRoomMessage[]>([]);
  const [agentBusDeliveries, setAgentBusDeliveries] = useState<Record<string, AgentBusDeliveryState>>({});
  const [loading, setLoading] = useState(() => !VIOLET_ROOM_STATE_CACHE.has(roomCacheKey));
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [hasOlder, setHasOlder] = useState(true);
  const [showJumpToLatest, setShowJumpToLatest] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickToBottomRef = useRef(true);
  const forceScrollBottomRef = useRef(false);
  const hasLoadedLatestRef = useRef(false);
  const historyExpandedRef = useRef(VIOLET_ROOM_HISTORY_EXPANDED_CACHE_KEYS.has(roomCacheKey));
  const filterAutofillRef = useRef({ key: '', pages: 0 });
  const scrollFrameRef = useRef<number | null>(null);
  const bottomSettleFrameRef = useRef<number | null>(null);
  const bottomSettleTimeoutRef = useRef<number | null>(null);
  const pendingScrollElRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const visibleAgentIds = useMemo(() => (
    agentIdsKey ? new Set(agentIdsKey.split('|')) : null
  ), [agentIdsKey]);
  const chatFilterAgentIdsKey = chatFilterAgentIds.join('|');
  const chatFilterAgentSet = useMemo(() => (
    new Set(chatFilterAgentIds)
  ), [chatFilterAgentIdsKey]);
  const chatFilterPageAgentIds = useMemo(() => (
    chatFilterActive ? normalizeAgentIds(chatFilterAgentIds) : []
  ), [chatFilterActive, chatFilterAgentIdsKey]);

  const commitRoomState = useCallback((next: VioletRoomState) => {
    setState((prev) => {
      const merged = prev
        ? {
            ...next,
            messages: mergeSyncedNativeMessages(prev.messages, next.messages, {
              preserveLoadedHistory: historyExpandedRef.current,
            }),
            agentBusReceipts: mergeAgentBusReceipts(prev.agentBusReceipts, next.agentBusReceipts),
          }
        : next;
      if (
        prev &&
        sameRoomMessages(prev.messages, merged.messages) &&
        sameAgentBusReceipts(prev.agentBusReceipts, merged.agentBusReceipts)
      ) return prev;
      VIOLET_ROOM_STATE_CACHE.set(roomCacheKey, merged);
      return merged;
    });
  }, [roomCacheKey]);

  const commitRoomStatePreservingView = useCallback((next: VioletRoomState) => {
    const anchor = (!stickToBottomRef.current && !forceScrollBottomRef.current)
      ? captureScrollAnchor(scrollRef.current)
      : null;
    commitRoomState(next);
    restoreScrollAnchor(scrollRef.current, anchor);
  }, [commitRoomState]);

  const commitOlderRoomState = useCallback((older: VioletRoomState) => {
    setState((prev) => {
      const merged = prev
        ? {
            ...prev,
            messages: mergeOlderRoomMessages(prev.messages, older.messages),
            agentBusReceipts: mergeAgentBusReceipts(prev.agentBusReceipts, older.agentBusReceipts),
            syncedAt: older.syncedAt,
          }
        : older;
      if (
        prev &&
        sameRoomMessages(prev.messages, merged.messages) &&
        sameAgentBusReceipts(prev.agentBusReceipts, merged.agentBusReceipts)
      ) return prev;
      VIOLET_ROOM_STATE_CACHE.set(roomCacheKey, merged);
      return merged;
    });
  }, [roomCacheKey]);

  const mergeExternalRoomState = useCallback((incoming: VioletRoomState) => {
    setState((prev) => {
      const next: VioletRoomState = prev
        ? {
            ...prev,
            messages: mergeSyncedNativeMessages(prev.messages, incoming.messages, {
              preserveLoadedHistory: historyExpandedRef.current,
            }),
            agentBusReceipts: mergeAgentBusReceipts(prev.agentBusReceipts, incoming.agentBusReceipts),
            syncedAt: incoming.syncedAt,
          }
        : incoming;
      if (
        prev &&
        sameRoomMessages(prev.messages, next.messages) &&
        sameAgentBusReceipts(prev.agentBusReceipts, next.agentBusReceipts)
      ) return prev;
      VIOLET_ROOM_STATE_CACHE.set(roomCacheKey, next);
      return next;
    });
  }, [roomCacheKey]);

  const mergeExternalRoomStatePreservingView = useCallback((incoming: VioletRoomState) => {
    const anchor = (!stickToBottomRef.current && !forceScrollBottomRef.current)
      ? captureScrollAnchor(scrollRef.current)
      : null;
    mergeExternalRoomState(incoming);
    restoreScrollAnchor(scrollRef.current, anchor);
  }, [mergeExternalRoomState]);

  const readCache = useCallback(async () => {
    try {
      const filteredAgentIds = chatFilterPageAgentIds.length > 0 ? chatFilterPageAgentIds : null;
      const next = await readVioletRoomCache({
        projectRoot: projectRoot ?? null,
        limit: VIOLET_ROOM_PAGE_SIZE,
        agentIds: filteredAgentIds,
      });
      if (!hasLoadedLatestRef.current) forceScrollBottomRef.current = true;
      commitRoomStatePreservingView(next);
      hasLoadedLatestRef.current = true;
      const nextHasOlder = next.messages.length >= VIOLET_ROOM_PAGE_SIZE;
      setHasOlder((current) => (current === nextHasOlder ? current : nextHasOlder));
      setError((current) => (current === null ? current : null));
    } catch {
      // Cache reads are best-effort; background ingestion will publish fresh state.
    }
  }, [chatFilterPageAgentIds, commitRoomStatePreservingView, projectRoot]);

  useEffect(() => {
    let cancelled = false;
    const cached = VIOLET_ROOM_STATE_CACHE.get(roomCacheKey) ?? null;
    setState(cached);
    setLoading(!cached);
    setLoadingOlder(false);
    setHasOlder(true);
    setError(null);
    setAgentBusDeliveries({});
    historyExpandedRef.current = VIOLET_ROOM_HISTORY_EXPANDED_CACHE_KEYS.has(roomCacheKey);
    stickToBottomRef.current = true;
    forceScrollBottomRef.current = true;
    hasLoadedLatestRef.current = false;

    void readCache().finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [readCache, roomCacheKey, projectRoot]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onVioletRoomSynced((payload) => {
      if (cancelled) return;
      if (!violetSyncMatchesProject(projectRoot ?? null, payload.request.projectRoot ?? null)) {
        return;
      }
      const requestedIds = payload.request.agentIds?.filter(Boolean) ?? [];
      const exactAgentSet = sameAgentSet(requestedIds, agentIds);
      if (exactAgentSet || requestedIds.length === 0) {
        commitRoomStatePreservingView(payload.state);
        return;
      }
      if (visibleAgentIds && requestedIds.every((agentId) => visibleAgentIds.has(agentId))) {
        mergeExternalRoomStatePreservingView(payload.state);
        return;
      }
      // Same-project sync payloads are authoritative even when their requested
      // agent set does not line up with the current filter/table view. Dropping
      // them leaves the room stale until a later explicit cache read.
      mergeExternalRoomStatePreservingView(payload.state);
    }).then((nextUnlisten) => {
      if (cancelled) {
        void nextUnlisten();
        return;
      }
      unlisten = nextUnlisten;
    });
    return () => {
      cancelled = true;
      if (unlisten) void unlisten();
    };
  }, [
    agentIds,
    commitRoomStatePreservingView,
    mergeExternalRoomStatePreservingView,
    projectRoot,
    visibleAgentIds,
  ]);

  const buildComposerMessage = useCallback((detail: VioletComposerSentDetail): VioletRoomMessage | null => {
    if (!detail || detail.targetAgentIds.length === 0) return null;
    if (!violetProjectRootsEqual(projectRoot ?? null, detail.projectRoot ?? null)) return null;
    const targetAgentIds = visibleAgentIds
      ? detail.targetAgentIds.filter((agentId) => visibleAgentIds.has(agentId))
      : detail.targetAgentIds;
    if (targetAgentIds.length === 0) return null;
    return {
      id: detail.id,
      sessionId: 'composer',
      agentId: 'user',
      shell: 'composer',
      role: 'user',
      kind: 'message',
      timestamp: detail.timestamp,
      text: detail.text,
      sourcePath: null,
      nativeEventId: null,
      local: true,
      projectRoot: normalizeVioletProjectRoot(detail.projectRoot),
      targetAgentIds,
      privacy: detail.privacy,
      composerMentions: detail.mentions,
    };
  }, [projectRoot, visibleAgentIds]);

  useEffect(() => {
    const seedMessages = violetComposerSentHistory(projectRoot ?? null)
      .map(buildComposerMessage)
      .filter((message): message is VioletRoomMessage => !!message)
      .slice(-80);
    setLocalMessages(seedMessages);

    const applyComposerSent = (detail: VioletComposerSentDetail) => {
      const message = buildComposerMessage(detail);
      if (!message) return;
      setLocalMessages((prev) => {
        const existing = prev.find((item) => item.id === detail.id);
        if (
          existing &&
          existing.text === message.text &&
          existing.timestamp === message.timestamp &&
          existing.targetAgentIds?.join('|') === message.targetAgentIds?.join('|')
        ) {
          return prev;
        }
        return [...prev.filter((item) => item.id !== detail.id), message].slice(-80);
      });
      stickToBottomRef.current = true;
    };
    const onComposerSent = (event: Event) => {
      applyComposerSent((event as CustomEvent<VioletComposerSentDetail>).detail);
    };
    window.addEventListener(VIOLET_COMPOSER_SENT_EVENT, onComposerSent);
    return () => window.removeEventListener(VIOLET_COMPOSER_SENT_EVENT, onComposerSent);
  }, [buildComposerMessage, projectRoot]);

  const applyComposerDelivery = useCallback((detail: VioletComposerDeliveryDetail) => {
    setLocalMessages((prev) => {
      let changed = false;
      const next = prev.map((message) => {
        if (message.id !== detail.id) return message;
        const updated = detail.status === 'clear'
          ? clearMessageDelivery(message)
          : setMessageDelivery(message, {
              status: detail.status,
              reason: detail.reason,
              retryTargetAgentIds: detail.retryTargetAgentIds,
            });
        if (updated !== message) changed = true;
        return updated;
      });
      return changed ? next : prev;
    });
  }, []);

  useEffect(() => {
    const onComposerDelivery = (event: Event) => {
      applyComposerDelivery((event as CustomEvent<VioletComposerDeliveryDetail>).detail);
    };
    window.addEventListener(VIOLET_COMPOSER_DELIVERY_EVENT, onComposerDelivery);
    return () => window.removeEventListener(VIOLET_COMPOSER_DELIVERY_EVENT, onComposerDelivery);
  }, [applyComposerDelivery]);

  const refreshComposerDelivery = useCallback(() => {
    const nativeMessages = state?.messages ?? [];
    const now = Date.now();
    setLocalMessages((prev) => {
      let changed = false;
      const next = prev.map((message) => {
        if (!shouldTrackComposerDelivery(message)) return message;
        const unconfirmedAgentIds = unconfirmedTargetAgentIds(message, nativeMessages);
        const activeAgentIds = activeComposerTargetAgentIds(message, nativeMessages);
        const retryTargetAgentIds = unconfirmedAgentIds.filter((agentId) => !activeAgentIds.includes(agentId));
        if (message.deliveryStatus === 'failed') {
          const failedTargetAgentIds = message.deliveryRetryTargetAgentIds?.length
            ? uniqueAgentIds(message.deliveryRetryTargetAgentIds)
            : uniqueAgentIds(message.targetAgentIds ?? []);
          const updated = setMessageDelivery(message, {
            status: 'failed',
            reason: message.deliveryReason,
            retryTargetAgentIds: failedTargetAgentIds,
          });
          if (updated !== message) changed = true;
          return updated;
        }
        if (retryTargetAgentIds.length === 0) {
          const updated = clearMessageDelivery(message);
          if (updated !== message) changed = true;
          return updated;
        }
        if (message.deliveryStatus === 'retrying') return message;
        const sentAt = Date.parse(message.timestamp);
        if (!Number.isFinite(sentAt) || now - sentAt < DELIVERY_CONFIRM_TIMEOUT_MS) return message;
        const updated = setMessageDelivery(message, {
          status: 'unconfirmed',
          reason: 'Prompt was not confirmed in the provider log.',
          retryTargetAgentIds,
        });
        if (updated !== message) changed = true;
        return updated;
      });
      return changed ? next : prev;
    });
  }, [state?.messages]);

  useEffect(() => {
    refreshComposerDelivery();
    const hasTrackedLocalMessages = localMessages.some(shouldTrackComposerDelivery);
    if (!hasTrackedLocalMessages) return undefined;
    const timer = window.setInterval(refreshComposerDelivery, 1000);
    return () => window.clearInterval(timer);
  }, [localMessages, refreshComposerDelivery]);

  const retryComposerMessage = useCallback(async (message: VioletRoomMessage) => {
    if (!onRetryComposerMessage || !shouldTrackComposerDelivery(message)) return;
    const retryTargetAgentIds = uniqueAgentIds(
      message.deliveryRetryTargetAgentIds?.length
        ? message.deliveryRetryTargetAgentIds
        : message.targetAgentIds ?? [],
    );
    if (retryTargetAgentIds.length === 0) return;
    setLocalMessages((prev) => prev.map((item) => (
      item.id === message.id
        ? setMessageDelivery(item, {
            status: 'retrying',
            reason: item.deliveryReason,
            retryTargetAgentIds,
          })
        : item
    )));
    try {
      const result = await onRetryComposerMessage({
        text: message.text,
        targetAgentIds: retryTargetAgentIds,
        privacy: !!message.privacy,
        mentions: uniqueComposerMentions(message.composerMentions),
      });
      setLocalMessages((prev) => prev.map((item) => (
        item.id === message.id
          ? result === false
            ? setMessageDelivery(item, {
                status: 'failed',
                reason: 'Prompt was not delivered.',
                retryTargetAgentIds,
              })
            : clearMessageDelivery(item)
          : item
      )));
    } catch {
      setLocalMessages((prev) => prev.map((item) => (
        item.id === message.id
          ? setMessageDelivery(item, {
              status: 'failed',
              reason: 'Prompt was not delivered.',
              retryTargetAgentIds,
            })
          : item
      )));
    }
  }, [onRetryComposerMessage]);

  const refreshAgentBusDelivery = useCallback(() => {
    const nativeMessages = state?.messages ?? [];
    const receipts = state?.agentBusReceipts ?? [];
    const candidates = nativeMessages.filter(shouldTrackAgentBusDelivery);
    const candidateIds = new Set(candidates.map((message) => message.id));
    const now = Date.now();
    setAgentBusDeliveries((prev) => {
      let changed = false;
      const next: Record<string, AgentBusDeliveryState> = { ...prev };
      for (const messageId of Object.keys(next)) {
        if (candidateIds.has(messageId)) continue;
        delete next[messageId];
        changed = true;
      }
      for (const message of candidates) {
        const current = next[message.id];
        const attemptEventIds = uniqueStrings([
          message.nativeEventId ?? '',
          ...(current?.attemptEventIds ?? []),
        ]);
        const retryTargetAgentIds = unconfirmedAgentBusTargetAgentIds(
          message,
          nativeMessages,
          receipts,
          attemptEventIds,
        );
        if (retryTargetAgentIds.length === 0) {
          if (current) {
            delete next[message.id];
            changed = true;
          }
          continue;
        }
        const sentAt = Date.parse(message.timestamp);
        const lastAttemptAt = current?.lastAttemptAt ?? (Number.isFinite(sentAt) ? sentAt : 0);
        if (
          !current &&
          (!Number.isFinite(sentAt) || now - sentAt > DELIVERY_CONFIRM_TIMEOUT_MS + DELIVERY_CONFIRM_OBSERVE_GRACE_MS)
        ) {
          continue;
        }
        if (!current && now - lastAttemptAt < DELIVERY_CONFIRM_TIMEOUT_MS) continue;
        if (current?.status === 'retrying' && now - lastAttemptAt < DELIVERY_CONFIRM_TIMEOUT_MS) {
          const updated = {
            ...current,
            retryTargetAgentIds,
            attemptEventIds,
          };
          if (!sameAgentBusDeliveryState(current, updated)) {
            next[message.id] = updated;
            changed = true;
          }
          continue;
        }
        const updated: AgentBusDeliveryState = {
          status: current?.status === 'failed' ? 'failed' : 'unconfirmed',
          reason: current?.status === 'failed'
            ? current.reason
            : 'A2A message was not confirmed in the provider log.',
          retryTargetAgentIds,
          attemptEventIds,
          lastAttemptAt,
        };
        if (!current || !sameAgentBusDeliveryState(current, updated)) {
          next[message.id] = updated;
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, [state?.agentBusReceipts, state?.messages]);

  useEffect(() => {
    refreshAgentBusDelivery();
    const hasTrackedAgentBusMessages = state?.messages.some(shouldTrackAgentBusDelivery) ?? false;
    if (!hasTrackedAgentBusMessages) return undefined;
    const timer = window.setInterval(refreshAgentBusDelivery, 1000);
    return () => window.clearInterval(timer);
  }, [refreshAgentBusDelivery, state?.messages]);

  const retryAgentBusMessage = useCallback(async (message: VioletRoomMessage) => {
    if (!projectRoot || !shouldTrackAgentBusDelivery(message) || !message.nativeEventId) return;
    const retryTargetAgentIds = uniqueAgentIds(
      message.deliveryRetryTargetAgentIds?.length
        ? message.deliveryRetryTargetAgentIds
        : message.targetAgentIds ?? [],
    );
    if (retryTargetAgentIds.length === 0) return;
    const now = Date.now();
    const attempts = retryTargetAgentIds.map((targetAgentId, index) => ({
      targetAgentId,
      eventId: `${message.nativeEventId}:retry:${now.toString(36)}-${index}-${Math.random().toString(36).slice(2, 8)}`,
    }));
    const attemptEventIds = attempts.map((attempt) => attempt.eventId);
    setAgentBusDeliveries((prev) => {
      const current = prev[message.id];
      return {
        ...prev,
        [message.id]: {
          status: 'retrying',
          reason: current?.reason,
          retryTargetAgentIds,
          attemptEventIds: uniqueStrings([
            message.nativeEventId ?? '',
            ...(current?.attemptEventIds ?? []),
            ...attemptEventIds,
          ]),
          lastAttemptAt: now,
        },
      };
    });
    try {
      const results = await Promise.all(attempts.map((attempt) => agentBusRetryDelivery({
        projectRoot,
        senderAgentId: message.agentId,
        senderName: message.agentDisplayName ?? null,
        targetAgentId: attempt.targetAgentId,
        intent: agentBusIntentForRetry(message),
        text: message.text,
        originalEventId: message.nativeEventId!,
        attemptEventId: attempt.eventId,
      })));
      const failedTargetAgentIds = results
        .filter((result) => !result.submitted)
        .map((result) => result.targetAgentId);
      if (failedTargetAgentIds.length > 0) {
        console.warn('[violet-room] agent bus retry delivery failed', failedTargetAgentIds);
      }
    } catch (err) {
      console.warn('[violet-room] agent bus retry delivery failed', err);
    }
  }, [projectRoot]);

  const localMessagesForProject = useMemo(
    () => localMessages.filter((message) => (
      !message.local || violetProjectRootsEqual(projectRoot ?? null, message.projectRoot ?? null)
    )),
    [localMessages, projectRoot],
  );

  const mergedMessages = useMemo(
    () => mergeRoomMessages(state?.messages ?? [], localMessagesForProject),
    [localMessagesForProject, state?.messages],
  );
  const messagesWithAgentBusDelivery = useMemo(
    () => applyAgentBusDeliveryStates(mergedMessages, agentBusDeliveries),
    [agentBusDeliveries, mergedMessages],
  );
  const messages = useStableRoomMessages(messagesWithAgentBusDelivery, roomCacheKey);
  const nextScopedMessages = useMemo(
    () => chatFilterActive
      ? messages.filter((message) => violetMessageMatchesAgentFilter(message, chatFilterAgentSet))
      : messages,
    [chatFilterActive, chatFilterAgentSet, messages],
  );
  const scopedMessages = useStableRoomMessages(
    nextScopedMessages,
    `${roomCacheKey}::${chatFilterActive ? chatFilterAgentIdsKey : 'all'}`,
  );
  const nextVisibleMessages = useMemo(
    () => chatFilterActive
      ? groupConsecutiveSameAgentProgressMessages(scopedMessages)
      : groupAdjacentProgressRunMessages(scopedMessages),
    [chatFilterActive, scopedMessages],
  );
  const visibleMessages = useStableRoomMessages(
    nextVisibleMessages,
    `${roomCacheKey}::visible::${chatFilterActive ? chatFilterAgentIdsKey : 'all'}`,
  );
  const latestVisibleMessage = visibleMessages[visibleMessages.length - 1];
  const latestVisibleMessageKey = latestVisibleMessage
    ? [
        latestVisibleMessage.id,
        latestVisibleMessage.timestamp,
        latestVisibleMessage.kind,
        latestVisibleMessage.text.length,
      ].join('|')
    : '';
  const chatFilterNames = chatFilterAgentIds.map((id) => shortAgentLabel(id, agentMeta) ?? id);
  const chatFilterLabel = formatAgentList(chatFilterNames);

  const setJumpToLatestVisible = useCallback((visible: boolean) => {
    setShowJumpToLatest((prev) => (prev === visible ? prev : visible));
  }, []);

  const cancelBottomSettle = useCallback(() => {
    if (bottomSettleFrameRef.current !== null) {
      window.cancelAnimationFrame(bottomSettleFrameRef.current);
      bottomSettleFrameRef.current = null;
    }
    if (bottomSettleTimeoutRef.current !== null) {
      window.clearTimeout(bottomSettleTimeoutRef.current);
      bottomSettleTimeoutRef.current = null;
    }
  }, []);

  const restorePrependScrollPosition = useCallback((snapshot: PrependScrollSnapshot | null) => {
    if (!snapshot) return;
    let attempts = 0;
    const restore = () => {
      const el = scrollRef.current;
      if (!el) return;
      const addedHeight = Math.max(0, el.scrollHeight - snapshot.scrollHeight);
      el.scrollTop = snapshot.scrollTop + addedHeight;
      const atBottom = isNearRoomBottom(el);
      stickToBottomRef.current = atBottom;
      setJumpToLatestVisible(visibleMessages.length > 0 && !atBottom);
      attempts += 1;
      if (attempts < 3) window.requestAnimationFrame(restore);
    };
    window.requestAnimationFrame(restore);
  }, [setJumpToLatestVisible, visibleMessages.length]);

  const loadOlder = useCallback(async () => {
    if (loadingOlder || !hasOlder) return;
    const filteredAgentIds = chatFilterPageAgentIds.length > 0 ? chatFilterPageAgentIds : null;
    const before = filteredAgentIds
      ? scopedMessages[0]?.timestamp ?? state?.messages[0]?.timestamp
      : state?.messages[0]?.timestamp;
    if (!before) {
      setHasOlder(false);
      return;
    }
    cancelBottomSettle();
    const scrollEl = scrollRef.current;
    const snapshot = scrollEl
      ? {
          scrollTop: scrollEl.scrollTop,
          scrollHeight: scrollEl.scrollHeight,
        }
      : null;
    stickToBottomRef.current = false;
    forceScrollBottomRef.current = false;
    setLoadingOlder(true);
    try {
      const next = await readVioletRoomCache({
        projectRoot: projectRoot ?? null,
        limit: VIOLET_ROOM_PAGE_SIZE,
        before,
        agentIds: filteredAgentIds,
      });
      setHasOlder(next.messages.length >= VIOLET_ROOM_PAGE_SIZE);
      if (next.messages.length > 0) {
        historyExpandedRef.current = true;
        VIOLET_ROOM_HISTORY_EXPANDED_CACHE_KEYS.add(roomCacheKey);
        commitOlderRoomState(next);
        restorePrependScrollPosition(snapshot);
      }
      setError(null);
    } catch {
      setError('Could not load older Violet messages.');
    } finally {
      setLoadingOlder(false);
    }
  }, [
    cancelBottomSettle,
    chatFilterPageAgentIds,
    commitOlderRoomState,
    hasOlder,
    loadingOlder,
    projectRoot,
    restorePrependScrollPosition,
    roomCacheKey,
    scopedMessages,
    state?.messages,
  ]);

  useEffect(() => {
    stickToBottomRef.current = true;
    forceScrollBottomRef.current = true;
    setHasOlder(true);
    setJumpToLatestVisible(false);
  }, [chatFilterActive, chatFilterAgentIdsKey, setJumpToLatestVisible]);

  useEffect(() => {
    const key = chatFilterActive ? chatFilterAgentIdsKey : '';
    if (filterAutofillRef.current.key !== key) {
      filterAutofillRef.current = { key, pages: 0 };
    }
    if (
      !chatFilterActive ||
      chatFilterAgentSet.size === 0 ||
      loading ||
      loadingOlder ||
      !hasOlder ||
      visibleMessages.length > 0 ||
      !state?.messages[0]?.timestamp ||
      filterAutofillRef.current.pages >= FILTER_AUTOFILL_PAGE_LIMIT
    ) {
      return;
    }
    filterAutofillRef.current.pages += 1;
    void loadOlder();
  }, [
    chatFilterActive,
    chatFilterAgentIdsKey,
    chatFilterAgentSet.size,
    hasOlder,
    loadOlder,
    loading,
    loadingOlder,
    state?.messages,
    visibleMessages.length,
  ]);

  const settleScrollToBottom = useCallback(() => {
    cancelBottomSettle();
    let attempts = 0;
    const settle = () => {
      bottomSettleFrameRef.current = null;
      const current = scrollRef.current;
      if (!current) {
        forceScrollBottomRef.current = false;
        return;
      }
      current.scrollTop = current.scrollHeight;
      stickToBottomRef.current = true;
      setJumpToLatestVisible(false);
      attempts += 1;
      if (attempts < 10) {
        bottomSettleFrameRef.current = window.requestAnimationFrame(settle);
        return;
      }
      forceScrollBottomRef.current = false;
      setJumpToLatestVisible(visibleMessages.length > 0 && !isNearRoomBottom(current));
    };
    settle();
  }, [cancelBottomSettle, setJumpToLatestVisible, visibleMessages.length]);

  const jumpToLatest = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    cancelBottomSettle();
    stickToBottomRef.current = true;
    forceScrollBottomRef.current = true;
    setJumpToLatestVisible(false);
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    bottomSettleTimeoutRef.current = window.setTimeout(() => {
      bottomSettleTimeoutRef.current = null;
      settleScrollToBottom();
    }, 280);
  }, [cancelBottomSettle, settleScrollToBottom, setJumpToLatestVisible]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || (!stickToBottomRef.current && !forceScrollBottomRef.current)) return;
    settleScrollToBottom();
  }, [
    chatFilterActive,
    chatFilterAgentIdsKey,
    latestVisibleMessageKey,
    loading,
    settleScrollToBottom,
    visibleMessages.length,
  ]);

  useEffect(() => {
    const content = contentRef.current;
    if (!content || typeof ResizeObserver === 'undefined') return undefined;
    let frame: number | null = null;
    const observer = new ResizeObserver(() => {
      if (!stickToBottomRef.current && !forceScrollBottomRef.current) return;
      if (frame !== null) return;
      frame = window.requestAnimationFrame(() => {
        frame = null;
        settleScrollToBottom();
      });
    });
    observer.observe(content);
    return () => {
      if (frame !== null) window.cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [settleScrollToBottom]);

  const handleScroll = useCallback((event: UIEvent<HTMLDivElement>) => {
    pendingScrollElRef.current = event.currentTarget;
    if (scrollFrameRef.current !== null) return;
    scrollFrameRef.current = window.requestAnimationFrame(() => {
      scrollFrameRef.current = null;
      const el = pendingScrollElRef.current;
      if (!el) return;
      const atBottom = isNearRoomBottom(el);
      if (!atBottom) {
        forceScrollBottomRef.current = false;
        cancelBottomSettle();
      }
      stickToBottomRef.current = atBottom;
      setJumpToLatestVisible(visibleMessages.length > 0 && !atBottom);
    });
  }, [cancelBottomSettle, setJumpToLatestVisible, visibleMessages.length]);

  useEffect(() => (
    () => {
      if (scrollFrameRef.current !== null) {
        window.cancelAnimationFrame(scrollFrameRef.current);
      }
      cancelBottomSettle();
    }
  ), [cancelBottomSettle]);

  return (
    <section
      className={[
        'violet-room-panel',
        chatFilterActive ? 'chat-filter-active' : '',
      ].filter(Boolean).join(' ')}
      aria-label={chatFilterActive ? `Violet room filtered to ${chatFilterLabel || 'current target'}` : 'Violet room'}
      data-chat-filter-agents={chatFilterActive ? chatFilterAgentIds.join('|') : undefined}
    >
      {onClose && (
        <button
          type="button"
          className="violet-room-minimize"
          onClick={onClose}
          aria-label="Minimize Violet room"
          title="Minimize"
        >
          −
        </button>
      )}

      <div
        ref={scrollRef}
        className="violet-room-scroll"
        onScroll={handleScroll}
      >
        <div ref={contentRef} className="violet-room-content">
          {(loadingOlder || hasOlder) && visibleMessages.length > 0 && (
            <button
              type="button"
              className="violet-room-older"
              onClick={() => void loadOlder()}
              disabled={loadingOlder || !hasOlder}
            >
              {loadingOlder && <i aria-hidden />}
              <span>{loadingOlder ? 'Loading previous messages' : 'Load previous 30 messages'}</span>
            </button>
          )}
          {error && <div className="violet-room-state error">{error}</div>}
          {loading && messages.length === 0 && (
            <div className="violet-room-loading" aria-live="polite">
              <i aria-hidden />
              <span>Loading Violet room</span>
            </div>
          )}
          {!error && !loading && visibleMessages.length === 0 && (
            chatFilterActive ? (
              <div className="violet-room-state chat-filter">
                <b>{chatFilterLabel ? `No messages for ${chatFilterLabel}.` : 'No target selected.'}</b>
                <span>This filter follows the current composer target and includes direct prompts, matching broadcasts, and selected agent replies.</span>
              </div>
            ) : (
              <div className="violet-room-state">
                <b>No room messages yet.</b>
                <span>Violet did not find provider native logs for this project, or they have not produced a turn.</span>
              </div>
            )
          )}
          {visibleMessages.map((message) => (
            <VioletMessageBubble
              key={message.id}
              message={message}
              projectRoot={projectRoot ?? null}
              agent={agentMeta?.[message.agentId]}
              agentMeta={agentMeta}
              record={agentRecords?.[message.agentId]}
              onAgentContextMenu={onAgentContextMenu}
              onCommendAgent={onCommendAgent}
              onOpenAgentTerminal={onOpenAgentTerminal}
              onRetryComposerMessage={retryComposerMessage}
              onRetryAgentBusMessage={retryAgentBusMessage}
            />
          ))}
        </div>
      </div>

      <button
        type="button"
        className={`violet-room-jump-latest ${showJumpToLatest ? 'visible' : ''}`}
        onClick={jumpToLatest}
        disabled={!showJumpToLatest}
        aria-hidden={!showJumpToLatest}
        aria-label="Jump to latest Violet message"
        title="Latest message"
        tabIndex={showJumpToLatest ? 0 : -1}
      >
        <span aria-hidden="true">↓</span>
        <span>Latest</span>
      </button>
    </section>
  );
}

const VioletMessageBubble = memo(function VioletMessageBubble({
  message,
  projectRoot,
  agent,
  agentMeta,
  record,
  onAgentContextMenu,
  onCommendAgent,
  onOpenAgentTerminal,
  onRetryComposerMessage,
  onRetryAgentBusMessage,
}: {
  message: VioletRoomMessage;
  projectRoot?: string | null;
  agent?: Agent;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  record?: ProjectAgentRecord;
  onAgentContextMenu?: (
    id: AgentId,
    point: { x: number; y: number },
    source?: ProjectAgentCommendSource,
  ) => void;
  onCommendAgent?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  onOpenAgentTerminal?: (id: AgentId) => void;
  onRetryComposerMessage?: (message: VioletRoomMessage) => void;
  onRetryAgentBusMessage?: (message: VioletRoomMessage) => void;
}) {
  if (message.kind === 'compaction') {
    return (
      <article className="violet-msg compaction" data-violet-message-id={message.id}>
        <span className="violet-compaction-chip">{message.text}</span>
      </article>
    );
  }
  if (isTurnInterruptMessage(message)) {
    return (
      <article className="violet-msg system-line interrupt" data-violet-message-id={message.id}>
        <span className="violet-system-line-text">
          <strong>{turnInterruptActorLabel(message, agentMeta)}</strong>
          {' '}
          interrupted the previous turn per human request
        </span>
      </article>
    );
  }
  if (message.ghostSasayaki) {
    return <GhostSasayakiBubble message={message} projectRoot={projectRoot} />;
  }
  if (isBbsThreadPromptMessage(message)) {
    return <BbsThreadPromptBubble message={message} projectRoot={projectRoot} />;
  }
  const isCommentary = message.kind === 'commentary';
  const isProcess = message.kind === 'tool' || message.kind === 'thinking' || isCommentary;
  const isUser = message.role === 'user' && !isProcess;
  const isBartenderConflict = isBartenderConflictMessage(message);
  const isEmberDream = isEmberDreamMessage(message);
  const showAvatar = !isUser && (!isProcess || isCommentary);
  const progressEntries = isCommentary ? progressEntriesForMessage(message) : [];
  const progressItems = progressEntries.map((entry) => entry.text);
  const latestProgressText = progressItems[progressItems.length - 1] ?? message.text;
  const progressAgentIds = isCommentary ? distinctProgressAgentIds(progressEntries) : [];
  const isMultiAgentProgress = progressAgentIds.length > 1;
  const canOpenTerminal = (
    message.kind === 'control' && !isUser && !isProcess && !!onOpenAgentTerminal
  );
  const messageAgent = isMultiAgentProgress ? undefined : (agent ?? agentSnapshotFromMessage(message));
  const actorName = systemActorName(message.agentId);
  const actorDescription = systemActorDescription(message.agentId);
  const label = isUser
    ? 'You'
    : isMultiAgentProgress
      ? `${progressAgentIds.length} agents`
      : messageAgent?.name ?? actorName ?? message.agentId;
  const lifecycle = isMultiAgentProgress
    ? null
    : projectAgentLifecycleStatus(
      agent ? agent.lifecycleStatus : (messageAgent?.lifecycleStatus ?? message.agentStatus),
    );
  const lifecycleLabel = lifecycle === 'archived' ? 'Archived' : lifecycle === 'left' ? 'Left' : null;
  const targetBadges = message.targetAgentIds?.length
    ? message.targetAgentIds
    : isUser
      ? [message.agentId].filter((id) => id !== 'user')
      : [];
  const avatarTitle = isMultiAgentProgress
    ? progressAgentIds
      .map((agentId) => progressAgentLabel(
        agentId,
        agentMeta,
        progressEntries.find((entry) => entry.agentId === agentId),
      ))
      .join(', ')
    : messageAgent?.name ?? actorDescription ?? actorName ?? message.agentId;
  const showComposerRetry = (
    isUser &&
    !!onRetryComposerMessage &&
    (message.deliveryStatus === 'failed' || message.deliveryStatus === 'unconfirmed')
  );
  const showAgentBusRetry = (
    !isUser &&
    shouldTrackAgentBusDelivery(message) &&
    !!onRetryAgentBusMessage &&
    (message.deliveryStatus === 'failed' || message.deliveryStatus === 'unconfirmed')
  );
  const showRetry = showComposerRetry || (AGENT_BUS_RETRY_UI_ENABLED && showAgentBusRetry);
  return (
    <article
      className={[
        'violet-msg',
        isUser ? 'user' : isProcess ? 'process' : 'agent',
        message.kind,
        showRetry ? 'delivery-issue' : '',
        isEmberDream ? 'ember-dream' : '',
        isBartenderConflict ? 'bartender-conflict' : '',
        lifecycle ? 'inactive-agent' : '',
        lifecycle ? `inactive-${lifecycle}` : '',
      ].filter(Boolean).join(' ')}
      data-violet-message-id={message.id}
    >
      {showAvatar && (
        <span className={[
          'violet-msg-avatar-host',
          isCommentary ? '' : 'agent-commend-host',
        ].filter(Boolean).join(' ')}
        >
          {isMultiAgentProgress ? (
            <ProgressAvatarStack
              agentIds={progressAgentIds}
              agentMeta={agentMeta}
              entries={progressEntries}
              title={avatarTitle}
              onAgentContextMenu={onAgentContextMenu}
            />
          ) : (
            <span
              className={`violet-msg-avatar tavern-avatar-art ${messageAgent?.avatarClass ?? providerAvatarClass(message.agentId)}`}
              style={avatarImageStyleForId(messageAgent?.avatarId)}
              title={avatarTitle}
              aria-hidden
              onContextMenu={(event) => {
                if (!onAgentContextMenu) return;
                event.preventDefault();
                event.stopPropagation();
                onAgentContextMenu(message.agentId, { x: event.clientX, y: event.clientY }, 'violet-room');
              }}
            >
              <span />
              <i />
              <b />
            </span>
          )}
          {!isCommentary && (
            <AgentCommendButton
              agentId={message.agentId}
              agentName={messageAgent?.name ?? message.agentId}
              source="violet-room"
              count={record?.commends}
              onCommend={onCommendAgent}
            />
          )}
        </span>
      )}
      {showRetry && (
        <button
          type="button"
          className="violet-msg-retry-send"
          onClick={() => {
            if (showComposerRetry) {
              onRetryComposerMessage?.(message);
            } else {
              onRetryAgentBusMessage?.(message);
            }
          }}
          aria-label="Retry sending this message"
          title="Retry"
        >
          <RetrySendIcon />
        </button>
      )}
      <div className="violet-msg-content">
        {!isUser && (
          <div className="violet-msg-meta">
            <span className="violet-agent-name-group">
              <ProjectAgentName name={label} compact className={lifecycle ? 'inactive' : ''} />
              {lifecycleLabel && <em className="violet-agent-status-label">{lifecycleLabel}</em>}
            </span>
            {targetBadges.map((agentId) => (
              <em key={agentId} className="violet-target-badge">
                @{shortAgentLabel(agentId, agentMeta)}
              </em>
            ))}
            {message.kind !== 'message' && !isCommentary && <b>{message.kind}</b>}
            <time>{formatTime(message.timestamp)}</time>
          </div>
        )}
        {isUser && targetBadges.length > 0 && (
          <div className="violet-msg-targets">
            {targetBadges.map((agentId) => (
              <em key={agentId} className="violet-target-badge">
                @{shortAgentLabel(agentId, agentMeta)}
              </em>
            ))}
            {message.privacy && <em className="violet-target-badge private">private</em>}
            <time>{formatTime(message.timestamp)}</time>
          </div>
        )}
        <div className="violet-msg-body">
          {isCommentary ? (
            <details className="violet-commentary-details">
              <summary>
                <span>Progress</span>
                <em>{progressSummary(progressItems.length, progressAgentIds.length, latestProgressText)}</em>
              </summary>
              {progressEntries.length > 1 ? (
                <div className="violet-commentary-feed">
                  {progressEntries.map((entry, index) => (
                    <div key={`${message.id}-progress-${index}`} className="violet-commentary-entry">
                      {isMultiAgentProgress && (
                        <ProgressEntrySpeaker
                          entry={entry}
                          agentMeta={agentMeta}
                        />
                      )}
                      <MarkdownText text={entry.text} projectRoot={projectRoot} enableLocalFileRefs />
                    </div>
                  ))}
                </div>
              ) : (
                <MarkdownText text={message.text} projectRoot={projectRoot} enableLocalFileRefs />
              )}
            </details>
          ) : isEmberDream ? (
            <details className="violet-ember-dream-details">
              <summary>
                <span>{EMBER_DREAM_MESSAGE_TITLE}</span>
              </summary>
              <MarkdownText text={emberDreamMessageBody(message.text)} projectRoot={projectRoot} enableLocalFileRefs />
            </details>
          ) : isBartenderConflict ? (
            <details className="violet-bartender-conflict-details">
              <summary>
                <span>{bartenderConflictSummary(message, agentMeta)}</span>
              </summary>
              <MarkdownText text={message.text} projectRoot={projectRoot} enableLocalFileRefs />
            </details>
          ) : (
            <MarkdownText text={message.text} projectRoot={projectRoot} enableLocalFileRefs />
          )}
          {canOpenTerminal && (
            <button
              type="button"
              className="violet-msg-action"
              onClick={() => onOpenAgentTerminal?.(message.agentId)}
            >
              Open terminal
            </button>
          )}
        </div>
      </div>
    </article>
  );
});

function RetrySendIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
      <path d="M12 2C6.48 2 2 6.48 2 12C2 17.52 6.48 22 12 22C17.52 22 22 17.52 22 12C22 6.48 17.52 2 12 2ZM17.19 15.94C17.15 16.03 17.1 16.11 17.03 16.18L15.34 17.87C15.19 18.02 15 18.09 14.81 18.09C14.62 18.09 14.43 18.02 14.28 17.87C13.99 17.58 13.99 17.1 14.28 16.81L14.69 16.4H9.1C7.8 16.4 6.75 15.34 6.75 14.05V12.28C6.75 11.87 7.09 11.53 7.5 11.53C7.91 11.53 8.25 11.87 8.25 12.28V14.05C8.25 14.52 8.63 14.9 9.1 14.9H14.69L14.28 14.49C13.99 14.2 13.99 13.72 14.28 13.43C14.57 13.14 15.05 13.14 15.34 13.43L17.03 15.12C17.1 15.19 17.15 15.27 17.19 15.36C17.27 15.55 17.27 15.76 17.19 15.94ZM17.25 11.72C17.25 12.13 16.91 12.47 16.5 12.47C16.09 12.47 15.75 12.13 15.75 11.72V9.95C15.75 9.48 15.37 9.1 14.9 9.1H9.31L9.72 9.5C10.01 9.79 10.01 10.27 9.72 10.56C9.57 10.71 9.38 10.78 9.19 10.78C9 10.78 8.81 10.71 8.66 10.56L6.97 8.87C6.9 8.8 6.85 8.72 6.81 8.63C6.73 8.45 6.73 8.24 6.81 8.06C6.85 7.97 6.9 7.88 6.97 7.81L8.66 6.12C8.95 5.83 9.43 5.83 9.72 6.12C10.01 6.41 10.01 6.89 9.72 7.18L9.31 7.59H14.9C16.2 7.59 17.25 8.65 17.25 9.94V11.72Z" />
    </svg>
  );
}

const GhostSasayakiBubble = memo(function GhostSasayakiBubble({
  message,
  projectRoot,
}: {
  message: VioletRoomMessage;
  projectRoot?: string | null;
}) {
  return (
    <article
      className="violet-msg user ghost-sasayaki"
      data-violet-message-id={message.id}
    >
      <details className="violet-ghost-sasayaki-details">
        <summary>
          <span>Ghost Sasayaki</span>
        </summary>
        <div className="violet-ghost-sasayaki-body">
          <MarkdownText text={message.text} projectRoot={projectRoot} enableLocalFileRefs />
        </div>
      </details>
    </article>
  );
});

/* BBS prompts (reply/post wrappers sent from the BBS panel or composer)
   read as internal plumbing in the room — collapse them behind a pill. */
const BBS_PROMPT_PREFIXES = [
  'You are replying to a Kota BBS thread',
  'You are creating a Kota BBS thread',
] as const;

function isBbsThreadPromptMessage(message: VioletRoomMessage): boolean {
  if (message.kind !== 'message') return false;
  const text = trimTerminalEnvelopePadding(stripLeadingProviderAttachmentMarkers(message.text));
  const inner = text.startsWith('<KOTA_MESSAGE')
    ? text.replace(/^<KOTA_MESSAGE[^>]*>\s*/, '')
    : text;
  return BBS_PROMPT_PREFIXES.some((prefix) => inner.startsWith(prefix));
}

const BbsThreadPromptBubble = memo(function BbsThreadPromptBubble({
  message,
  projectRoot,
}: {
  message: VioletRoomMessage;
  projectRoot?: string | null;
}) {
  return (
    <article
      className="violet-msg user ghost-sasayaki bbs-thread-prompt"
      data-violet-message-id={message.id}
    >
      <details className="violet-ghost-sasayaki-details">
        <summary>
          <span>Check this BBS Thread</span>
        </summary>
        <div className="violet-ghost-sasayaki-body">
          <MarkdownText text={message.text} projectRoot={projectRoot} enableLocalFileRefs />
        </div>
      </details>
    </article>
  );
});

const EMBER_DREAM_MESSAGE_TITLE = "It's time to dream.";

const ProgressAvatarStack = memo(function ProgressAvatarStack({
  agentIds,
  agentMeta,
  entries,
  title,
  onAgentContextMenu,
}: {
  agentIds: readonly string[];
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  entries: readonly VioletProgressEntry[];
  title: string;
  onAgentContextMenu?: (
    id: AgentId,
    point: { x: number; y: number },
    source?: ProjectAgentCommendSource,
  ) => void;
}) {
  const visibleAgentIds = agentIds.slice(0, 3);
  const overflow = Math.max(0, agentIds.length - visibleAgentIds.length);
  return (
    <span className="violet-progress-avatar-stack" title={title} aria-hidden>
      {visibleAgentIds.map((agentId) => {
        const agent = progressEntryAgent(agentId, entries, agentMeta);
        return (
          <span
            key={agentId}
            className={`violet-progress-avatar-mini tavern-avatar-art ${agent?.avatarClass ?? providerAvatarClass(agentId)}`}
            style={avatarImageStyleForId(agent?.avatarId)}
            onContextMenu={(event) => {
              if (!onAgentContextMenu) return;
              event.preventDefault();
              event.stopPropagation();
              onAgentContextMenu(agentId, { x: event.clientX, y: event.clientY }, 'violet-room');
            }}
          >
            <span />
            <i />
            <b />
          </span>
        );
      })}
      {overflow > 0 && <span className="violet-progress-avatar-more">+{overflow}</span>}
    </span>
  );
});

const ProgressEntrySpeaker = memo(function ProgressEntrySpeaker({
  entry,
  agentMeta,
}: {
  entry: VioletProgressEntry;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
}) {
  const agent = progressEntryAgent(entry.agentId, [entry], agentMeta);
  const label = progressAgentLabel(entry.agentId, agentMeta, entry);
  return (
    <div className="violet-commentary-entry-speaker">
      <span
        className={`violet-commentary-entry-avatar tavern-avatar-art ${agent?.avatarClass ?? providerAvatarClass(entry.agentId)}`}
        style={avatarImageStyleForId(agent?.avatarId)}
        aria-hidden
      >
        <span />
        <i />
        <b />
      </span>
      <ProjectAgentName name={label} compact />
      <time>{formatTime(entry.timestamp)}</time>
    </div>
  );
});

function compactPreview(text: string): string {
  const value = text.replace(/\s+/g, ' ').trim();
  if (value.length <= 96) return value;
  return `${value.slice(0, 95)}...`;
}

function progressSummary(updateCount: number, agentCount: number, latestText: string): string {
  const preview = compactPreview(latestText);
  const parts = [];
  if (updateCount > 1) parts.push(`${updateCount} updates`);
  if (agentCount > 1) parts.push(`${agentCount} agents`);
  parts.push(preview);
  return parts.join(' · ');
}

function progressEntriesForMessage(message: VioletRoomMessage): readonly VioletProgressEntry[] {
  const structured = message.progressEntries?.filter((entry) => entry.text.trim().length > 0) ?? [];
  if (structured.length > 0) return structured;
  const textEntries = message.progressItems?.filter((item) => item.trim().length > 0) ?? [];
  const texts = textEntries.length > 0 ? textEntries : [message.text];
  return texts
    .filter((text) => text.trim().length > 0)
    .map((text) => progressEntryFromMessage(message, text));
}

function progressEntryFromMessage(message: VioletRoomMessage, text: string): VioletProgressEntry {
  return {
    agentId: message.agentId,
    shell: message.shell,
    timestamp: message.timestamp,
    text,
    agentDisplayName: message.agentDisplayName,
    agentAvatarId: message.agentAvatarId,
    agentProvider: message.agentProvider,
    agentStatus: message.agentStatus,
  };
}

function distinctProgressAgentIds(entries: readonly VioletProgressEntry[]): string[] {
  const out: string[] = [];
  for (const entry of entries) {
    if (!out.includes(entry.agentId)) out.push(entry.agentId);
  }
  return out;
}

function progressEntryAgent(
  agentId: string,
  entries: readonly VioletProgressEntry[],
  agentMeta?: Readonly<Record<AgentId, Agent>>,
): Agent | undefined {
  const liveAgent = agentMeta?.[agentId];
  if (liveAgent) return liveAgent;
  const entry = entries.find((item) => item.agentId === agentId);
  return entry ? agentSnapshotFromProgressEntry(entry) : undefined;
}

function progressAgentLabel(
  agentId: string,
  agentMeta?: Readonly<Record<AgentId, Agent>>,
  entry?: VioletProgressEntry,
): string {
  return agentMeta?.[agentId]?.name
    ?? entry?.agentDisplayName
    ?? systemActorName(agentId)
    ?? agentId;
}

function isBartenderConflictMessage(message: VioletRoomMessage): boolean {
  return (
    message.agentId === 'bartender' &&
    message.kind === 'message' &&
    !!message.nativeEventId?.startsWith('bartender-conflict:') &&
    message.text.includes('Git said:')
  );
}

function bartenderConflictSummary(
  message: VioletRoomMessage,
  agentMeta?: Readonly<Record<AgentId, Agent>>,
): string {
  const target = message.targetAgentIds?.[0];
  return target
    ? `@${shortAgentLabel(target, agentMeta)} resolving worktree conflict`
    : 'Resolve worktree conflict';
}

function isEmberDreamMessage(message: VioletRoomMessage): boolean {
  return (
    message.agentId === 'ember' &&
    message.kind === 'message' &&
    message.text.trimStart().startsWith(EMBER_DREAM_MESSAGE_TITLE)
  );
}

function isTurnInterruptMessage(message: VioletRoomMessage): boolean {
  if (message.kind === 'interrupt') return true;
  const text = message.text.trim().toLowerCase();
  return (
    text.startsWith('<turn_aborted>') &&
    text.endsWith('</turn_aborted>') &&
    text.includes('interrupted the previous turn')
  );
}

function turnInterruptActorLabel(
  message: VioletRoomMessage,
  agentMeta?: Readonly<Record<AgentId, Agent>>,
): string {
  return agentMeta?.[message.agentId]?.name
    ?? message.agentDisplayName
    ?? systemActorName(message.agentId)
    ?? message.agentId;
}

function emberDreamMessageBody(text: string): string {
  const trimmed = text.trimStart();
  if (!trimmed.startsWith(EMBER_DREAM_MESSAGE_TITLE)) return text;
  return trimmed.slice(EMBER_DREAM_MESSAGE_TITLE.length).replace(/^\s+/, '');
}

type MarkdownRenderOptions = {
  projectRoot?: string | null;
  previewImageRefs: ReadonlySet<string>;
  enableLocalFileRefs: boolean;
};

const VIOLET_INLINE_IMAGE_PREVIEW_LIMIT = 4;
const VIOLET_INLINE_IMAGE_CACHE_LIMIT = 50;
const violetInlineImageDataUrlCache = new Map<string, string>();
const EMPTY_IMAGE_REF_SET = new Set<string>();
const DEFAULT_MARKDOWN_RENDER_OPTIONS: MarkdownRenderOptions = {
  projectRoot: null,
  previewImageRefs: EMPTY_IMAGE_REF_SET,
  enableLocalFileRefs: false,
};

export const MarkdownText = memo(function MarkdownText({
  text,
  projectRoot,
  enableLocalFileRefs = false,
}: {
  text: string;
  projectRoot?: string | null;
  enableLocalFileRefs?: boolean;
}) {
  const blocks = useMemo(() => parseMarkdownBlocks(text), [text]);
  const previewImageRefs = useMemo(() => collectPreviewImageRefs(text), [text]);
  const renderOptions = useMemo<MarkdownRenderOptions>(() => ({
    projectRoot: projectRoot ?? null,
    previewImageRefs,
    enableLocalFileRefs,
  }), [enableLocalFileRefs, previewImageRefs, projectRoot]);
  return (
    <div className="violet-msg-text">
      {blocks.map((block, index) => {
        if (block.kind === 'code') {
          if (block.lang === 'html') {
            return <HtmlDrawingBlock key={index} html={block.text} />;
          }
          return (
            <pre key={index} className="violet-md-code">
              <code>{block.text}</code>
            </pre>
          );
        }
        if (block.kind === 'heading') {
          const Heading = `h${Math.min(block.level, 4)}` as keyof JSX.IntrinsicElements;
          return <Heading key={index} className="violet-md-heading">{renderInlineMarkdown(block.text, renderOptions)}</Heading>;
        }
        if (block.kind === 'quote') {
          return <blockquote key={index}>{renderInlineMarkdown(block.text, renderOptions)}</blockquote>;
        }
        if (block.kind === 'ul') {
          return <ul key={index}>{block.items.map((item, itemIndex) => <li key={itemIndex}>{renderInlineMarkdown(item, renderOptions)}</li>)}</ul>;
        }
        if (block.kind === 'ol') {
          return <ol key={index} start={block.start}>{block.items.map((item, itemIndex) => <li key={itemIndex}>{renderInlineMarkdown(item, renderOptions)}</li>)}</ol>;
        }
        if (block.kind === 'table') {
          return (
            <div key={index} className="violet-md-table-wrap">
              <table className="violet-md-table">
                <thead>
                  <tr>
                    {block.headers.map((header, cellIndex) => (
                      <th key={cellIndex}>{renderInlineMarkdown(header, renderOptions)}</th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {block.rows.map((row, rowIndex) => (
                    <tr key={rowIndex}>
                      {block.headers.map((_, cellIndex) => (
                        <td key={cellIndex}>{renderInlineMarkdown(row[cellIndex] ?? '', renderOptions)}</td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          );
        }
        if (block.kind === 'canvas') {
          return (
            <CanvasReferenceBlock
              key={index}
              snapshot={block.snapshot}
              editable={block.editable}
              note={block.note}
            />
          );
        }
        if (block.kind === 'paragraph') {
          return <p key={index}>{renderInlineMarkdown(block.text, renderOptions)}</p>;
        }
        return null;
      })}
    </div>
  );
});

const HTML_DRAWING_MIN_HEIGHT = 160;
const HTML_DRAWING_MAX_HEIGHT = 420;
const HTML_DRAWING_MAX_VIEWPORT_RATIO = 0.58;
const HTML_DRAWING_PREVIEW_STYLE_ID = 'kota-html-preview-spacing';
const HTML_DRAWING_PREVIEW_STYLE = `
:where(body.kota-html-preview-spacing) {
  overflow-wrap: anywhere;
}
:where(body.kota-html-preview-spacing > * + *) {
  margin-block-start: 12px;
}
:where(body.kota-html-preview-spacing :is(table, pre, figure, blockquote, ul, ol) + *) {
  margin-block-start: 14px;
}
`;

function clampHtmlDrawingHeight(height: number): number {
  const viewportMax = typeof window === 'undefined'
    ? HTML_DRAWING_MAX_HEIGHT
    : Math.floor(window.innerHeight * HTML_DRAWING_MAX_VIEWPORT_RATIO);
  const maxHeight = Math.max(
    HTML_DRAWING_MIN_HEIGHT,
    Math.min(HTML_DRAWING_MAX_HEIGHT, viewportMax),
  );
  return Math.min(maxHeight, Math.max(HTML_DRAWING_MIN_HEIGHT, Math.ceil(height)));
}

function readHtmlDrawingHeight(frame: HTMLIFrameElement | null): number | null {
  try {
    const document = frame?.contentDocument;
    if (!document) return null;
    const body = document.body;
    const root = document.documentElement;
    const height = Math.max(
      body?.scrollHeight ?? 0,
      body?.offsetHeight ?? 0,
      root?.scrollHeight ?? 0,
      root?.offsetHeight ?? 0,
    );
    return clampHtmlDrawingHeight(height);
  } catch {
    return null;
  }
}

function applyHtmlDrawingPreviewSpacing(document: Document | null) {
  if (!document?.documentElement) return;
  let style = document.getElementById(HTML_DRAWING_PREVIEW_STYLE_ID) as HTMLStyleElement | null;
  if (!style) {
    style = document.createElement('style');
    style.id = HTML_DRAWING_PREVIEW_STYLE_ID;
    style.textContent = HTML_DRAWING_PREVIEW_STYLE;
    (document.head ?? document.documentElement).appendChild(style);
  }
  document.body?.classList.add('kota-html-preview-spacing');
}

const HtmlDrawingBlock = memo(function HtmlDrawingBlock({ html }: { html: string }) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);
  const [frameHeight, setFrameHeight] = useState(HTML_DRAWING_MIN_HEIGHT);

  const cleanupObservers = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;
  }, []);

  const measureHeight = useCallback(() => {
    const nextHeight = readHtmlDrawingHeight(iframeRef.current);
    if (nextHeight === null) return;
    setFrameHeight((currentHeight) => (
      currentHeight === nextHeight ? currentHeight : nextHeight
    ));
  }, []);

  const handleLoad = useCallback(() => {
    cleanupObservers();

    let document: Document | null = null;
    let frameWindow: Window | null = null;
    try {
      document = iframeRef.current?.contentDocument ?? null;
      frameWindow = iframeRef.current?.contentWindow ?? null;
    } catch {
      return;
    }
    applyHtmlDrawingPreviewSpacing(document);
    measureHeight();

    const cleanups: Array<() => void> = [];
    if (document && typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver(measureHeight);
      observer.observe(document.documentElement);
      if (document.body) observer.observe(document.body);
      cleanups.push(() => observer.disconnect());
    }

    const settleTimer = window.setTimeout(measureHeight, 80);
    cleanups.push(() => window.clearTimeout(settleTimer));

    if (frameWindow) {
      frameWindow.addEventListener('resize', measureHeight);
      cleanups.push(() => frameWindow.removeEventListener('resize', measureHeight));
    }

    cleanupRef.current = () => {
      cleanups.forEach((cleanup) => cleanup());
    };
  }, [cleanupObservers, measureHeight]);

  useEffect(() => {
    setFrameHeight(HTML_DRAWING_MIN_HEIGHT);
    cleanupObservers();
  }, [cleanupObservers, html]);

  useEffect(() => {
    window.addEventListener('resize', measureHeight);
    return () => window.removeEventListener('resize', measureHeight);
  }, [measureHeight]);

  useEffect(() => cleanupObservers, [cleanupObservers]);

  return (
    <div className="violet-html-drawing">
      <iframe
        ref={iframeRef}
        title="Agent HTML drawing"
        // Same-origin lets the parent measure srcDoc height; do not add allow-scripts.
        sandbox="allow-same-origin"
        srcDoc={html}
        style={{ height: frameHeight }}
        onLoad={handleLoad}
      />
      <details>
        <summary>HTML source</summary>
        <pre className="violet-md-code">
          <code>{html}</code>
        </pre>
      </details>
    </div>
  );
});

const RichInline = memo(function RichInline({
  text,
  options,
}: {
  text: string;
  options: MarkdownRenderOptions;
}) {
  const parts = useMemo(() => splitRichText(text), [text]);
  return (
    <>
      {parts.map((part, index) => {
        if (part.kind === 'text') return <span key={index}>{part.value}</span>;
        return (
          <LocalFileRef
            key={index}
            value={part.value}
            kind={part.kind}
            projectRoot={options.projectRoot}
            enableLocalFileRefs={options.enableLocalFileRefs}
            previewImage={part.kind === 'image' && options.previewImageRefs.has(part.value)}
          />
        );
      })}
    </>
  );
});

type MarkdownBlock =
  | { kind: 'paragraph'; text: string }
  | { kind: 'heading'; level: number; text: string }
  | { kind: 'quote'; text: string }
  | { kind: 'ul'; items: string[] }
  | { kind: 'ol'; items: string[]; start?: number }
  | { kind: 'table'; headers: string[]; rows: string[][] }
  | { kind: 'code'; text: string; lang?: string }
  | { kind: 'canvas'; snapshot?: string; editable?: string; note?: string };

function parseMarkdownBlocks(text: string): MarkdownBlock[] {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  const blocks: MarkdownBlock[] = [];
  let paragraph: string[] = [];
  const flushParagraph = () => {
    if (paragraph.length > 0) {
      blocks.push({ kind: 'paragraph', text: paragraph.join('\n') });
      paragraph = [];
    }
  };
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? '';
    const trimmed = line.trim();
    if (trimmed === '[canvas]') {
      flushParagraph();
      const canvasLines: string[] = [];
      let trailing = '';
      index += 1;
      while (index < lines.length) {
        const current = lines[index] ?? '';
        const closeIndex = current.indexOf('[/canvas]');
        if (closeIndex >= 0) {
          const beforeClose = current.slice(0, closeIndex);
          if (beforeClose.trim()) canvasLines.push(beforeClose);
          trailing = current.slice(closeIndex + '[/canvas]'.length).trim();
          break;
        }
        canvasLines.push(current);
        index += 1;
      }
      blocks.push(parseCanvasBlock(canvasLines));
      if (trailing) paragraph.push(trailing);
      continue;
    }
    if (trimmed.startsWith('```')) {
      flushParagraph();
      const lang = trimmed.slice(3).trim().split(/\s+/)[0]?.toLowerCase() || undefined;
      const codeLines: string[] = [];
      index += 1;
      while (index < lines.length && !(lines[index] ?? '').trim().startsWith('```')) {
        codeLines.push(lines[index] ?? '');
        index += 1;
      }
      blocks.push({ kind: 'code', text: codeLines.join('\n'), lang });
      continue;
    }
    if (!trimmed) {
      flushParagraph();
      continue;
    }
    if (isMarkdownTableStart(lines, index)) {
      flushParagraph();
      const headers = parseMarkdownTableRow(lines[index] ?? '');
      index += 2;
      const rows: string[][] = [];
      while (index < lines.length && isMarkdownTableRow(lines[index] ?? '')) {
        rows.push(parseMarkdownTableRow(lines[index] ?? ''));
        index += 1;
      }
      index -= 1;
      blocks.push({ kind: 'table', headers, rows });
      continue;
    }
    const heading = trimmed.match(/^(#{1,4})\s+(.+)$/);
    if (heading) {
      flushParagraph();
      blocks.push({ kind: 'heading', level: heading[1]!.length, text: heading[2]! });
      continue;
    }
    if (/^>\s?/.test(trimmed)) {
      flushParagraph();
      const quotes = [trimmed.replace(/^>\s?/, '')];
      while (index + 1 < lines.length && /^>\s?/.test((lines[index + 1] ?? '').trim())) {
        index += 1;
        quotes.push((lines[index] ?? '').trim().replace(/^>\s?/, ''));
      }
      blocks.push({ kind: 'quote', text: quotes.join('\n') });
      continue;
    }
    if (/^[-*+]\s+/.test(trimmed)) {
      flushParagraph();
      const items = [trimmed.replace(/^[-*+]\s+/, '')];
      while (index + 1 < lines.length && /^[-*+]\s+/.test((lines[index + 1] ?? '').trim())) {
        index += 1;
        items.push((lines[index] ?? '').trim().replace(/^[-*+]\s+/, ''));
      }
      blocks.push({ kind: 'ul', items });
      continue;
    }
    const orderedListMatch = trimmed.match(/^(\d+)[.)]\s+(.+)$/);
    if (orderedListMatch) {
      flushParagraph();
      const start = Number.parseInt(orderedListMatch[1] ?? '1', 10);
      const items = [orderedListMatch[2] ?? ''];
      while (index + 1 < lines.length && /^\d+[.)]\s+/.test((lines[index + 1] ?? '').trim())) {
        index += 1;
        items.push((lines[index] ?? '').trim().replace(/^\d+[.)]\s+/, ''));
      }
      blocks.push({ kind: 'ol', items, start: Number.isFinite(start) ? start : undefined });
      continue;
    }
    paragraph.push(line);
  }
  flushParagraph();
  return blocks.length > 0 ? blocks : [{ kind: 'paragraph', text }];
}

function parseCanvasBlock(lines: readonly string[]): MarkdownBlock {
  let snapshot: string | undefined;
  let editable: string | undefined;
  const note: string[] = [];
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const snapshotMatch = trimmed.match(/^snapshot:\s*(.+)$/i);
    if (snapshotMatch) {
      snapshot = snapshotMatch[1]!.trim();
      continue;
    }
    const editableMatch = trimmed.match(/^editable:\s*(.+)$/i);
    if (editableMatch) {
      editable = editableMatch[1]!.trim();
      continue;
    }
    note.push(line);
  }
  return {
    kind: 'canvas',
    snapshot,
    editable,
    note: note.join('\n').trim() || undefined,
  };
}

function isMarkdownTableStart(lines: readonly string[], index: number): boolean {
  const header = lines[index] ?? '';
  const divider = lines[index + 1] ?? '';
  return isMarkdownTableRow(header) && isMarkdownTableDivider(divider);
}

function isMarkdownTableRow(line: string): boolean {
  return line.includes('|') && parseMarkdownTableRow(line).length >= 2;
}

function isMarkdownTableDivider(line: string): boolean {
  const cells = parseMarkdownTableRow(line);
  return cells.length >= 2 && cells.every((cell) => /^:?-{3,}:?$/.test(cell.trim()));
}

function parseMarkdownTableRow(line: string): string[] {
  let value = line.trim();
  if (value.startsWith('|')) value = value.slice(1);
  if (value.endsWith('|')) value = value.slice(0, -1);
  return value.split('|').map((cell) => cell.trim());
}

function renderInlineMarkdown(
  text: string,
  options: MarkdownRenderOptions = DEFAULT_MARKDOWN_RENDER_OPTIONS,
): ReactNode {
  const nodes: ReactNode[] = [];
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\(([^)]+)\))/g;
  let lastIndex = 0;
  for (const match of text.matchAll(pattern)) {
    const value = match[0];
    const index = match.index ?? 0;
    if (index > lastIndex) nodes.push(<RichInline key={`t-${index}`} text={text.slice(lastIndex, index)} options={options} />);
    if (value.startsWith('`')) {
      const codeText = value.slice(1, -1);
      const codeFileRef = localFileRefFromInlineCode(codeText);
      nodes.push(codeFileRef
        ? (
          <LocalFileRef
            key={`c-${index}`}
            value={codeFileRef.value}
            label={codeText}
            kind={codeFileRef.kind}
            projectRoot={options.projectRoot}
            enableLocalFileRefs={options.enableLocalFileRefs}
            previewImage={options.previewImageRefs.has(codeFileRef.value)}
            inlineCode
          />
        )
        : <code key={`c-${index}`} className="violet-md-inline-code">{codeText}</code>);
    } else if (value.startsWith('**')) {
      nodes.push(<strong key={`b-${index}`}>{renderInlineMarkdown(value.slice(2, -2), options)}</strong>);
    } else {
      const link = value.match(/^\[([^\]]+)\]\(([^)]+)\)$/);
      if (link && /^https?:\/\//i.test(link[2])) {
        nodes.push(<a key={`a-${index}`} href={link[2]} target="_blank" rel="noreferrer">{link[1]}</a>);
      } else if (link) {
        nodes.push(
          <LocalFileRef
            key={`a-${index}`}
            value={link[2]}
            label={link[1]}
            rawText={value}
            kind={isImageFileRef(link[2]) ? 'image' : 'file'}
            projectRoot={options.projectRoot}
            enableLocalFileRefs={options.enableLocalFileRefs}
            previewImage={options.previewImageRefs.has(link[2])}
          />,
        );
      } else {
        nodes.push(<RichInline key={`x-${index}`} text={value} options={options} />);
      }
    }
    lastIndex = index + value.length;
  }
  if (lastIndex < text.length) nodes.push(<RichInline key="tail" text={text.slice(lastIndex)} options={options} />);
  return nodes;
}

function splitRichText(text: string): Array<{ kind: 'text' | 'file' | 'image'; value: string }> {
  const pattern = /((?:~\/|\/|\.{1,2}\/|[\w@.-]+\/)[^\s"'`<>()]+?\.(?:tsx?|jsx?|rs|md|markdown|json|ya?ml|toml|css|scss|html?|png|jpe?g|webp|gif|svg|pdf|txt|log|csv|tsv|zip|dmg|app|command|sh|scpt|pkg|terminal|workflow)(?::\d+)?)/gi;
  const out: Array<{ kind: 'text' | 'file' | 'image'; value: string }> = [];
  let lastIndex = 0;
  for (const match of text.matchAll(pattern)) {
    const value = match[0];
    const index = match.index ?? 0;
    if (index > lastIndex) out.push({ kind: 'text', value: text.slice(lastIndex, index) });
    out.push({
      kind: isImageFileRef(value) ? 'image' : 'file',
      value,
    });
    lastIndex = index + value.length;
  }
  if (lastIndex < text.length) out.push({ kind: 'text', value: text.slice(lastIndex) });
  return out;
}

function localFileRefFromInlineCode(value: string): { kind: 'file' | 'image'; value: string } | null {
  const parts = splitRichText(value);
  if (parts.length !== 1) return null;
  const [part] = parts;
  if (!part || part.kind === 'text' || part.value !== value) return null;
  return { kind: part.kind, value: part.value };
}

function collectPreviewImageRefs(text: string): ReadonlySet<string> {
  const refs = new Set<string>();
  for (const match of text.matchAll(/\[[^\]]+\]\(([^)]+)\)/g)) {
    const ref = match[1] ?? '';
    if (!isPreviewableImageRef(ref)) continue;
    refs.add(ref);
    if (refs.size >= VIOLET_INLINE_IMAGE_PREVIEW_LIMIT) return refs;
  }
  for (const part of splitRichText(text)) {
    if (part.kind !== 'image' || !isPreviewableImageRef(part.value)) continue;
    refs.add(part.value);
    if (refs.size >= VIOLET_INLINE_IMAGE_PREVIEW_LIMIT) break;
  }
  return refs;
}

function isImageFileRef(value: string): boolean {
  return /\.(png|jpe?g|webp|gif|svg)(?::\d+)?$/i.test(stripLocalLineSuffix(value));
}

function isPreviewableImageRef(value: string): boolean {
  return /\.(png|jpe?g|webp|gif)$/i.test(stripLocalLineSuffix(value));
}

function stripLocalLineSuffix(value: string): string {
  return value.replace(/:\d+$/, '');
}

function rememberInlineImageDataUrl(path: string, dataUrl: string): void {
  if (violetInlineImageDataUrlCache.has(path)) {
    violetInlineImageDataUrlCache.delete(path);
  }
  violetInlineImageDataUrlCache.set(path, dataUrl);
  while (violetInlineImageDataUrlCache.size > VIOLET_INLINE_IMAGE_CACHE_LIMIT) {
    const oldest = violetInlineImageDataUrlCache.keys().next().value;
    if (!oldest) break;
    violetInlineImageDataUrlCache.delete(oldest);
  }
}

const LocalFileRef = memo(function LocalFileRef({
  value,
  label,
  rawText,
  kind,
  projectRoot,
  enableLocalFileRefs,
  previewImage,
  inlineCode = false,
}: {
  value: string;
  label?: string;
  rawText?: string;
  kind: 'file' | 'image';
  projectRoot?: string | null;
  enableLocalFileRefs?: boolean;
  previewImage?: boolean;
  inlineCode?: boolean;
}) {
  const [resolved, setResolved] = useState<VioletFileRefResolveResult | null>(null);
  const [imageContextMenu, setImageContextMenu] = useState<{ x: number; y: number; path: string } | null>(null);
  const pointerDownRef = useRef<{ x: number; y: number } | null>(null);
  const displayText = label ?? value;

  useEffect(() => {
    let cancelled = false;
    setResolved(null);
    if (!enableLocalFileRefs) return undefined;
    void violetResolveFileRef({ projectRoot: projectRoot ?? null, path: value })
      .then((result) => {
        if (cancelled) return;
        setResolved(result);
      })
      .catch(() => {
        if (cancelled) return;
        setResolved(null);
      });
    return () => {
      cancelled = true;
    };
  }, [enableLocalFileRefs, projectRoot, value]);

  const openRef = useCallback(() => {
    if (!enableLocalFileRefs || !resolved) return;
    void violetOpenFileRef({ projectRoot: projectRoot ?? null, path: value }).catch((err) => {
      console.warn('[violet-room] open file ref failed', err);
    });
  }, [enableLocalFileRefs, projectRoot, resolved, value]);

  const handleMouseDown = useCallback((event: MouseEvent<HTMLElement>) => {
    pointerDownRef.current = { x: event.clientX, y: event.clientY };
  }, []);

  const shouldSkipClickOpen = useCallback((event: MouseEvent<HTMLElement>) => {
    const start = pointerDownRef.current;
    pointerDownRef.current = null;
    if (start) {
      const dx = Math.abs(event.clientX - start.x);
      const dy = Math.abs(event.clientY - start.y);
      if (dx > 3 || dy > 3) return true;
    }
    const selection = window.getSelection();
    return !!selection && (!selection.isCollapsed || selection.toString().length > 0);
  }, []);

  const handleClick = useCallback((event: MouseEvent<HTMLElement>) => {
    if (!resolved || shouldSkipClickOpen(event)) return;
    openRef();
  }, [openRef, resolved, shouldSkipClickOpen]);

  const handleKeyDown = useCallback((event: KeyboardEvent<HTMLElement>) => {
    if (!resolved) return;
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    openRef();
  }, [openRef, resolved]);

  useEffect(() => {
    if (!imageContextMenu) return undefined;
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      setImageContextMenu(null);
    };
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Element && target.closest('[data-violet-image-context-menu="true"]')) return;
      setImageContextMenu(null);
    };
    document.addEventListener('keydown', onKeyDown, true);
    document.addEventListener('pointerdown', onPointerDown, true);
    return () => {
      document.removeEventListener('keydown', onKeyDown, true);
      document.removeEventListener('pointerdown', onPointerDown, true);
    };
  }, [imageContextMenu]);

  const handleImageContextMenu = useCallback((event: MouseEvent<HTMLImageElement>) => {
    if (!resolved) return;
    event.preventDefault();
    event.stopPropagation();
    setImageContextMenu({ x: event.clientX, y: event.clientY, path: resolved.path });
  }, [resolved]);

  const runImageContextAction = useCallback(async (action: 'open' | 'reveal' | 'copy') => {
    const path = imageContextMenu?.path;
    if (!path) return;
    try {
      if (action === 'open') {
        openRef();
      } else if (action === 'reveal') {
        await violetRevealFileRef({ projectRoot: null, path });
      } else if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(path);
      }
    } catch (err) {
      console.warn(`[violet-room] image ${action} failed`, err);
      if (typeof window !== 'undefined' && typeof window.alert === 'function') {
        window.alert(`${action} failed: ${err}`);
      }
    } finally {
      setImageContextMenu(null);
    }
  }, [imageContextMenu?.path, openRef]);

  const imageContextMenuPortal = imageContextMenu && typeof document !== 'undefined'
    ? createPortal(
      <div
        data-violet-image-context-menu="true"
        className="tree-context-menu"
        style={contextMenuPosition(imageContextMenu.x, imageContextMenu.y)}
        role="menu"
        onPointerDown={(event) => event.stopPropagation()}
        onMouseDown={(event) => event.stopPropagation()}
        onClick={(event) => event.stopPropagation()}
      >
        <button type="button" role="menuitem" onClick={() => void runImageContextAction('open')}>
          Open
        </button>
        <button type="button" role="menuitem" onClick={() => void runImageContextAction('reveal')}>
          Reveal in Finder
        </button>
        <button type="button" role="menuitem" onClick={() => void runImageContextAction('copy')}>
          Copy full path
        </button>
      </div>,
      document.body,
    )
    : null;

  if (!resolved && rawText) return <span>{rawText}</span>;

  const className = [
    inlineCode ? 'violet-md-inline-code' : 'violet-file-ref',
    kind === 'image' ? 'image' : '',
    resolved ? 'clickable' : '',
  ].filter(Boolean).join(' ');

  const chip = kind === 'image' ? (
    <span
      className={className}
      role={resolved ? 'link' : undefined}
      tabIndex={resolved ? 0 : undefined}
      title={resolved ? `Open ${resolved.path}` : undefined}
      onMouseDown={handleMouseDown}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
    >
      {displayText}
    </span>
  ) : (
    <code
      className={className}
      role={resolved ? 'link' : undefined}
      tabIndex={resolved ? 0 : undefined}
      title={resolved ? `Open ${resolved.path}` : undefined}
      onMouseDown={handleMouseDown}
      onClick={handleClick}
      onKeyDown={handleKeyDown}
    >
      {displayText}
    </code>
  );

  if (kind !== 'image' || !previewImage || !resolved || resolved.isDir || !isPreviewableImageRef(value)) {
    return chip;
  }

  return (
    <span className="violet-image-ref">
      <InlineLocalImagePreview
        path={resolved.path}
        alt={basename(value)}
        onOpen={openRef}
        onContextMenu={handleImageContextMenu}
      />
      {imageContextMenuPortal}
    </span>
  );
});

const InlineLocalImagePreview = memo(function InlineLocalImagePreview({
  path,
  alt,
  onOpen,
  onContextMenu,
}: {
  path: string;
  alt: string;
  onOpen: () => void;
  onContextMenu?: (event: MouseEvent<HTMLImageElement>) => void;
}) {
  const [src, setSrc] = useState<string | null>(() => violetInlineImageDataUrlCache.get(path) ?? null);
  const handleKeyDown = useCallback((event: KeyboardEvent<HTMLImageElement>) => {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault();
    onOpen();
  }, [onOpen]);

  useEffect(() => {
    if (violetInlineImageDataUrlCache.has(path)) {
      setSrc(violetInlineImageDataUrlCache.get(path) ?? null);
      return;
    }
    let cancelled = false;
    void fileImageDataUrl(path)
      .then((dataUrl) => {
        rememberInlineImageDataUrl(path, dataUrl);
        if (!cancelled) setSrc(dataUrl);
      })
      .catch(() => {
        if (!cancelled) setSrc(null);
      });
    return () => {
      cancelled = true;
    };
  }, [path]);

  if (!src) return null;
  return (
    <img
      className="violet-inline-image-preview"
      src={src}
      alt={alt}
      draggable={false}
      role="button"
      tabIndex={0}
      title={`Open ${path}`}
      onClick={onOpen}
      onContextMenu={onContextMenu}
      onKeyDown={handleKeyDown}
    />
  );
});

function contextMenuPosition(x: number, y: number): { left: number; top: number } {
  if (typeof window === 'undefined') return { left: x, top: y };
  const margin = 10;
  const menuWidth = 190;
  const menuHeight = 102;
  return {
    left: Math.min(Math.max(margin, x), Math.max(margin, window.innerWidth - menuWidth - margin)),
    top: Math.min(Math.max(margin, y), Math.max(margin, window.innerHeight - menuHeight - margin)),
  };
}

const CanvasReferenceBlock = memo(function CanvasReferenceBlock({
  snapshot,
  editable,
  note,
}: {
  snapshot?: string;
  editable?: string;
  note?: string;
}) {
  return (
    <div className="violet-canvas-ref">
      <div className="violet-canvas-ref-head">
        <span className="violet-canvas-ref-icon">DRAW</span>
        <strong>Drawing</strong>
        {snapshot && <span className="violet-canvas-ref-file">{basename(snapshot)}</span>}
      </div>
      {editable && (
        <div className="violet-canvas-ref-path">
          <span>editable</span>
          <code>{editable}</code>
        </div>
      )}
      {note && <p>{renderInlineMarkdown(note)}</p>}
    </div>
  );
});


export function mergeRoomMessages(
  nativeMessages: readonly VioletChatMessage[],
  localMessages: readonly VioletRoomMessage[],
): VioletRoomMessage[] {
  const filteredNative = nativeMessages.filter((message) => !isDuplicateNativeUserMessage(message, localMessages));
  const markedNative = markGhostSasayakiMessages(
    collapseNativeBroadcastUserMessages(filteredNative),
    localMessages,
  );
  return dedupeRoomMessages([...markedNative, ...localMessages])
    .sort(compareRoomMessagesForDisplay);
}

function violetMessageMatchesAgentFilter(message: VioletRoomMessage, agentIds: ReadonlySet<AgentId>): boolean {
  if (agentIds.size === 0) return false;
  if (message.role === 'user') {
    if (message.targetAgentIds) return message.targetAgentIds.some((agentId) => agentIds.has(agentId));
    return agentIds.has(message.agentId);
  }
  if (agentIds.has(message.agentId)) return true;
  return message.targetAgentIds?.some((agentId) => agentIds.has(agentId)) ?? false;
}

function normalizeAgentIds(ids: readonly AgentId[]): AgentId[] {
  return Array.from(new Set(ids.filter((id): id is AgentId => Boolean(id)))).sort();
}

function collapseNativeBroadcastUserMessages(messages: readonly VioletChatMessage[]): VioletRoomMessage[] {
  const sorted = [...messages].sort((a, b) => a.timestamp.localeCompare(b.timestamp) || a.id.localeCompare(b.id));
  const groups: Array<{
    textKey: string;
    time: number;
    message: VioletRoomMessage;
  }> = [];
  const out: VioletRoomMessage[] = [];

  for (const message of sorted) {
    if (!isNativeUserPrompt(message)) {
      out.push(message);
      continue;
    }
    const textKey = normalizeForDedupe(message.text).toLowerCase();
    const time = Date.parse(message.timestamp);
    if (!textKey || !Number.isFinite(time)) {
      out.push(message);
      continue;
    }
    const existing = groups.find((group) => (
      group.textKey === textKey &&
      Math.abs(group.time - time) <= BROADCAST_USER_GROUP_WINDOW_MS
    ));
    if (existing) {
      const targets = existing.message.targetAgentIds ?? [];
      if (!targets.includes(message.agentId)) {
        existing.message = {
          ...existing.message,
          targetAgentIds: [...targets, message.agentId],
        };
        const index = out.findIndex((item) => item.id === existing.message.id);
        if (index >= 0) out[index] = existing.message;
      }
      continue;
    }
    const next: VioletRoomMessage = {
      ...message,
      targetAgentIds: [message.agentId],
    };
    groups.push({ textKey, time, message: next });
    out.push(next);
  }

  return out;
}

function markGhostSasayakiMessages(
  nativeMessages: readonly VioletRoomMessage[],
  localMessages: readonly VioletRoomMessage[],
): VioletRoomMessage[] {
  const claimedLocalTargets = new Set<string>();
  const localCandidates = localMessages
    .map((message) => ({ message, time: Date.parse(message.timestamp) }))
    .filter((item) => (
      item.message.role === 'user' &&
      item.message.targetAgentIds &&
      item.message.targetAgentIds.length > 0 &&
      Number.isFinite(item.time)
    ))
    .sort((a, b) => a.time - b.time || a.message.id.localeCompare(b.message.id));

  const markedInternalEchoes = nativeMessages.map((message) => (
    isNativeInternalAgentBusEnvelopeEcho(message)
      ? { ...message, ghostSasayaki: true }
      : message
  ));

  if (localCandidates.length === 0) return markedInternalEchoes;

  return markedInternalEchoes.map((message) => {
    if (message.ghostSasayaki) return message;
    if (!isNativeUserPrompt(message)) return message;
    const nativeTime = Date.parse(message.timestamp);
    if (!Number.isFinite(nativeTime)) return message;
    const claims = ghostSasayakiClaimsForMessage(message, nativeTime, localCandidates, claimedLocalTargets);
    if (claims.length === 0) return message;
    for (const claim of claims) claimedLocalTargets.add(claim.claimKey);
    const latestLocalTime = Math.max(...claims.map((claim) => claim.localTime));
    return {
      ...message,
      ghostSasayaki: true,
      ghostSasayakiSortTime: latestLocalTime + 1,
    };
  });
}

type GhostSasayakiClaim = {
  claimKey: string;
  localTime: number;
};

function ghostSasayakiClaimsForMessage(
  message: VioletRoomMessage,
  nativeTime: number,
  localCandidates: readonly { message: VioletRoomMessage; time: number }[],
  claimedLocalTargets: ReadonlySet<string>,
): GhostSasayakiClaim[] {
  const claims: GhostSasayakiClaim[] = [];
  for (const agentId of ghostSasayakiTargetAgentIds(message)) {
    const candidates = localCandidates
      .map((item) => {
        if (!item.message.targetAgentIds?.includes(agentId)) return null;
        const claimKey = ghostSasayakiClaimKey(item.message, agentId);
        if (
          claimedLocalTargets.has(claimKey) ||
          claims.some((claim) => claim.claimKey === claimKey)
        ) {
          return null;
        }
        const delta = Math.abs(nativeTime - item.time);
        if (delta > GHOST_SASAYAKI_WINDOW_MS) return null;
        return { item, claimKey, delta };
      })
      .filter((item): item is {
        item: { message: VioletRoomMessage; time: number };
        claimKey: string;
        delta: number;
      } => !!item)
      .sort((a, b) => (
        a.delta - b.delta ||
        a.item.time - b.item.time ||
        a.item.message.id.localeCompare(b.item.message.id)
      ));
    const candidate = candidates[0];
    if (candidate) {
      claims.push({ claimKey: candidate.claimKey, localTime: candidate.item.time });
    }
  }
  return claims;
}

function ghostSasayakiTargetAgentIds(message: VioletRoomMessage): string[] {
  const ids = message.targetAgentIds && message.targetAgentIds.length > 0
    ? message.targetAgentIds
    : [message.agentId];
  return Array.from(new Set(ids.filter((id) => id && id !== 'user')));
}

function ghostSasayakiClaimKey(message: VioletRoomMessage, agentId: string): string {
  return `${message.id}\u001f${agentId}`;
}

function isNativeInternalAgentBusEnvelopeEcho(message: VioletRoomMessage): boolean {
  return isNativeUserPrompt(message) && isInternalAgentBusEnvelopeText(message.text);
}

function isInternalAgentBusEnvelopeText(text: string): boolean {
  const trimmed = trimTerminalEnvelopePadding(stripLeadingProviderAttachmentMarkers(text));
  return (
    (trimmed.startsWith('<KOTA_MESSAGE ') || trimmed.startsWith('<KOTA_MESSAGE>')) &&
    trimmed.endsWith('</KOTA_MESSAGE>')
  );
}

function stripLeadingProviderAttachmentMarkers(text: string): string {
  let rest = trimTerminalEnvelopePadding(text);
  for (let index = 0; index < 8; index += 1) {
    const match = rest.match(/^\[([^\]\n]{1,40})\]\s*/);
    if (!match || !isProviderAttachmentMarker(match[1] ?? '')) break;
    rest = trimTerminalEnvelopePadding(rest.slice(match[0].length));
  }
  return rest;
}

function trimTerminalEnvelopePadding(text: string): string {
  return text.replace(/^[\s\u0000-\u001f\u007f-\u009f]+|[\s\u0000-\u001f\u007f-\u009f]+$/gu, '');
}

function trimTerminalControlPadding(text: string): string {
  return text.replace(
    /^[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]+|[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f]+$/gu,
    '',
  );
}

function isProviderAttachmentMarker(marker: string): boolean {
  const hashIndex = marker.lastIndexOf('#');
  if (hashIndex <= 0) return false;
  const label = marker.slice(0, hashIndex).trim();
  const ordinal = marker.slice(hashIndex + 1).trim();
  return label.length > 0 && /^[0-9]+$/.test(ordinal);
}

function compareRoomMessagesForDisplay(left: VioletRoomMessage, right: VioletRoomMessage): number {
  const leftTime = roomMessageDisplaySortTime(left);
  const rightTime = roomMessageDisplaySortTime(right);
  if (leftTime !== rightTime) return leftTime - rightTime;
  return left.timestamp.localeCompare(right.timestamp) || left.id.localeCompare(right.id);
}

function roomMessageDisplaySortTime(message: VioletRoomMessage): number {
  if (Number.isFinite(message.ghostSasayakiSortTime)) return message.ghostSasayakiSortTime!;
  const parsed = Date.parse(message.timestamp);
  return Number.isFinite(parsed) ? parsed : 0;
}

function groupConsecutiveSameAgentProgressMessages(messages: readonly VioletRoomMessage[]): VioletRoomMessage[] {
  const out: VioletRoomMessage[] = [];
  for (const message of messages) {
    const previous = out[out.length - 1];
    if (
      previous &&
      isProgressMessage(previous) &&
      isProgressMessage(message) &&
      previous.agentId === message.agentId
    ) {
      out[out.length - 1] = appendProgressMessage(previous, message);
      continue;
    }
    out.push(message);
  }
  return out;
}

function groupAdjacentProgressRunMessages(messages: readonly VioletRoomMessage[]): VioletRoomMessage[] {
  const out: VioletRoomMessage[] = [];
  for (const message of messages) {
    const previous = out[out.length - 1];
    if (previous && isProgressMessage(previous) && isProgressMessage(message)) {
      out[out.length - 1] = appendProgressMessage(previous, message);
      continue;
    }
    out.push(message);
  }
  return out;
}

// `commentary` must never contain a user-visible final/reply. All progress folding
// depends on final assistant messages staying `kind: "message"` so run boundaries hold.
function isProgressMessage(message: VioletRoomMessage): boolean {
  return message.role === 'assistant' && message.kind === 'commentary';
}

function appendProgressMessage(previous: VioletRoomMessage, next: VioletRoomMessage): VioletRoomMessage {
  const progressEntries = [
    ...progressEntriesForMessage(previous),
    ...progressEntriesForMessage(next),
  ];
  const progressItems = progressEntries.map((entry) => entry.text);
  return {
    ...previous,
    timestamp: next.timestamp,
    text: progressItems.join('\n\n'),
    progressItems,
    progressEntries,
    sourcePath: next.sourcePath ?? previous.sourcePath,
    nativeEventId: next.nativeEventId ?? previous.nativeEventId,
    agentDisplayName: next.agentDisplayName ?? previous.agentDisplayName,
    agentAvatarId: next.agentAvatarId ?? previous.agentAvatarId,
    agentProvider: next.agentProvider ?? previous.agentProvider,
    agentStatus: next.agentStatus ?? previous.agentStatus,
  };
}

function shouldTrackComposerDelivery(message: VioletRoomMessage): boolean {
  return (
    !!message.local &&
    message.role === 'user' &&
    message.kind === 'message' &&
    message.agentId === 'user' &&
    !!message.targetAgentIds?.length
  );
}

function shouldTrackAgentBusDelivery(message: VioletRoomMessage): boolean {
  return (
    !message.local &&
    message.role === 'assistant' &&
    message.kind === 'message' &&
    message.shell === 'system' &&
    message.agentId !== 'user' &&
    !!message.nativeEventId?.startsWith('agentbus-') &&
    !message.nativeEventId.includes(':skipped') &&
    !!message.targetAgentIds?.length &&
    message.text.trim().length > 0
  );
}

function applyAgentBusDeliveryStates(
  messages: readonly VioletRoomMessage[],
  deliveries: Readonly<Record<string, AgentBusDeliveryState>>,
): VioletRoomMessage[] {
  if (Object.keys(deliveries).length === 0) return messages as VioletRoomMessage[];
  let changed = false;
  const next = messages.map((message) => {
    const delivery = deliveries[message.id];
    if (!delivery || !shouldTrackAgentBusDelivery(message)) return message;
    const updated = setMessageDelivery(message, {
      status: delivery.status,
      reason: delivery.reason,
      retryTargetAgentIds: delivery.retryTargetAgentIds,
    });
    if (updated !== message) changed = true;
    return updated;
  });
  return changed ? next : messages as VioletRoomMessage[];
}

function unconfirmedTargetAgentIds(
  message: VioletRoomMessage,
  nativeMessages: readonly VioletChatMessage[],
): string[] {
  const targetAgentIds = uniqueAgentIds(message.targetAgentIds ?? []);
  return targetAgentIds.filter((agentId) => {
    const probe: VioletRoomMessage = { ...message, targetAgentIds: [agentId] };
    return !nativeMessages.some((nativeMessage) => isDuplicateNativeUserMessage(nativeMessage, [probe]));
  });
}

function activeComposerTargetAgentIds(
  message: VioletRoomMessage,
  nativeMessages: readonly VioletChatMessage[],
): string[] {
  const sentAt = Date.parse(message.timestamp);
  if (!Number.isFinite(sentAt)) return [];
  const targetAgentIds = uniqueAgentIds(message.targetAgentIds ?? []);
  // Busy CLIs can queue composer input before emitting a native user event.
  return targetAgentIds.filter((agentId) => nativeMessages.some((nativeMessage) => (
    nativeMessage.agentId === agentId &&
    nativeMessage.role === 'assistant' &&
    Date.parse(nativeMessage.timestamp) >= sentAt
  )));
}

function unconfirmedAgentBusTargetAgentIds(
  message: VioletRoomMessage,
  nativeMessages: readonly VioletChatMessage[],
  receipts: readonly AgentBusReceipt[],
  attemptEventIds: readonly string[],
): string[] {
  const targetAgentIds = uniqueAgentIds(message.targetAgentIds ?? []);
  if (targetAgentIds.length === 0) return [];
  const confirmed = confirmedAgentBusTargetAgentIds(message, nativeMessages, receipts, attemptEventIds);
  return targetAgentIds.filter((agentId) => !confirmed.has(agentId));
}

function confirmedAgentBusTargetAgentIds(
  message: VioletRoomMessage,
  nativeMessages: readonly VioletChatMessage[],
  receipts: readonly AgentBusReceipt[],
  attemptEventIds: readonly string[],
): ReadonlySet<string> {
  const eventIds = new Set(uniqueStrings([message.nativeEventId ?? '', ...attemptEventIds]));
  const targetAgentIds = new Set(uniqueAgentIds(message.targetAgentIds ?? []));
  const confirmed = new Set<string>();
  if (eventIds.size === 0 || targetAgentIds.size === 0) return confirmed;
  for (const receipt of receipts) {
    if (!targetAgentIds.has(receipt.agentId)) continue;
    if (!eventIds.has(receipt.eventId)) continue;
    confirmed.add(receipt.agentId);
  }
  for (const nativeMessage of nativeMessages) {
    if (!isNativeUserPrompt(nativeMessage)) continue;
    if (!targetAgentIds.has(nativeMessage.agentId)) continue;
    const eventId = internalAgentBusEnvelopeEventId(nativeMessage.text);
    if (!eventId || !eventIds.has(eventId)) continue;
    confirmed.add(nativeMessage.agentId);
  }
  return confirmed;
}

function internalAgentBusEnvelopeEventId(text: string): string | null {
  const trimmed = trimTerminalEnvelopePadding(stripLeadingProviderAttachmentMarkers(text));
  if (!trimmed.startsWith('<KOTA_MESSAGE') || !trimmed.endsWith('</KOTA_MESSAGE>')) return null;
  const tag = trimmed.match(/^<KOTA_MESSAGE(?:\s+[^>]*)?>/i)?.[0] ?? '';
  if (!tag) return null;
  const match = tag.match(/\bid=(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i);
  return (match?.[1] ?? match?.[2] ?? match?.[3] ?? '').trim() || null;
}

function agentBusIntentForRetry(message: VioletRoomMessage): string {
  return message.nativeEventId?.startsWith('agentbus-') ? 'handoff' : 'message';
}

function sameAgentBusDeliveryState(left: AgentBusDeliveryState, right: AgentBusDeliveryState): boolean {
  return (
    left.status === right.status &&
    left.reason === right.reason &&
    left.lastAttemptAt === right.lastAttemptAt &&
    targetAgentIdsKey(left.retryTargetAgentIds) === targetAgentIdsKey(right.retryTargetAgentIds) &&
    uniqueStrings(left.attemptEventIds).join(',') === uniqueStrings(right.attemptEventIds).join(',')
  );
}

function clearMessageDelivery(message: VioletRoomMessage): VioletRoomMessage {
  if (
    !message.deliveryStatus &&
    !message.deliveryReason &&
    !message.deliveryRetryTargetAgentIds?.length
  ) {
    return message;
  }
  const {
    deliveryStatus: _deliveryStatus,
    deliveryReason: _deliveryReason,
    deliveryRetryTargetAgentIds: _deliveryRetryTargetAgentIds,
    ...rest
  } = message;
  return rest;
}

function setMessageDelivery(
  message: VioletRoomMessage,
  delivery: {
    status: VioletDeliveryStatus;
    reason?: string;
    retryTargetAgentIds?: readonly string[];
  },
): VioletRoomMessage {
  const retryTargetAgentIds = uniqueAgentIds(delivery.retryTargetAgentIds ?? message.targetAgentIds ?? []);
  const reason = delivery.reason || undefined;
  if (
    message.deliveryStatus === delivery.status &&
    message.deliveryReason === reason &&
    targetAgentIdsKey(message.deliveryRetryTargetAgentIds) === targetAgentIdsKey(retryTargetAgentIds)
  ) {
    return message;
  }
  return {
    ...message,
    deliveryStatus: delivery.status,
    deliveryReason: reason,
    deliveryRetryTargetAgentIds: retryTargetAgentIds,
  };
}

function uniqueAgentIds(agentIds: readonly string[]): AgentId[] {
  return Array.from(new Set(agentIds.filter((id): id is AgentId => Boolean(id))));
}

function uniqueStrings(values: readonly string[]): string[] {
  return Array.from(new Set(values.map((value) => value.trim()).filter(Boolean)));
}

function uniqueComposerMentions(
  mentions?: readonly { agentId: string; aka: string }[],
): { agentId: AgentId; aka: string }[] | undefined {
  if (!mentions || mentions.length === 0) return undefined;
  const seen = new Set<string>();
  const out: { agentId: AgentId; aka: string }[] = [];
  for (const mention of mentions) {
    if (!mention.agentId || seen.has(mention.agentId)) continue;
    seen.add(mention.agentId);
    out.push({ agentId: mention.agentId, aka: mention.aka });
  }
  return out.length > 0 ? out : undefined;
}

function dedupeRoomMessages(messages: VioletRoomMessage[]): VioletRoomMessage[] {
  const sorted = [...messages].sort((a, b) => a.timestamp.localeCompare(b.timestamp) || a.id.localeCompare(b.id));
  const seen = new Set<string>();
  const out: VioletRoomMessage[] = [];
  for (const message of sorted) {
    const bucket = Math.floor(Date.parse(message.timestamp) / 120000) || 0;
    const key = [
      message.agentId,
      message.role,
      message.kind,
      targetAgentIdsKey(message.targetAgentIds),
      bucket,
      normalizeForDedupe(message.text).toLowerCase(),
    ].join('\u001f');
    if (!seen.has(key)) {
      seen.add(key);
      out.push(message);
    }
  }
  return out;
}

function captureScrollAnchor(container: HTMLDivElement | null): ScrollAnchor | null {
  if (!container) return null;
  const containerTop = container.getBoundingClientRect().top;
  const nodes = Array.from(
    container.querySelectorAll<HTMLElement>('[data-violet-message-id]'),
  );
  const anchored = nodes.find((node) => node.getBoundingClientRect().bottom > containerTop + 8)
    ?? nodes[0];
  const id = anchored?.dataset.violetMessageId;
  if (!anchored || !id) return null;
  return {
    id,
    top: anchored.getBoundingClientRect().top - containerTop,
  };
}

function restoreScrollAnchor(container: HTMLDivElement | null, anchor: ScrollAnchor | null): void {
  if (!container || !anchor) return;
  let attempts = 0;
  const restore = () => {
    const node = container.querySelector<HTMLElement>(
      `[data-violet-message-id="${cssEscape(anchor.id)}"]`,
    );
    if (!node) return;
    const containerTop = container.getBoundingClientRect().top;
    const nextTop = node.getBoundingClientRect().top - containerTop;
    container.scrollTop += nextTop - anchor.top;
    attempts += 1;
    if (attempts < 3) window.requestAnimationFrame(restore);
  };
  window.requestAnimationFrame(restore);
}

function isNearRoomBottom(container: HTMLDivElement): boolean {
  return container.scrollHeight - container.scrollTop - container.clientHeight < 48;
}

function cssEscape(value: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') {
    return CSS.escape(value);
  }
  return value.replace(/["\\]/g, '\\$&');
}

export function mergeSyncedNativeMessages(
  left: readonly VioletChatMessage[],
  right: readonly VioletChatMessage[],
  options: { preserveLoadedHistory?: boolean } = {},
): VioletChatMessage[] {
  const merged = dedupeRoomMessages([...(left as VioletRoomMessage[]), ...(right as VioletRoomMessage[])])
    .sort((a, b) => a.timestamp.localeCompare(b.timestamp) || a.id.localeCompare(b.id));
  const bounded = options.preserveLoadedHistory
    ? merged
    : merged.slice(-VIOLET_ROOM_LIVE_LIMIT);
  return reuseStableRoomMessages(left as readonly VioletRoomMessage[], bounded);
}

export function mergeOlderRoomMessages(
  left: readonly VioletChatMessage[],
  right: readonly VioletChatMessage[],
): VioletChatMessage[] {
  const merged = dedupeRoomMessages([...(left as VioletRoomMessage[]), ...(right as VioletRoomMessage[])])
    .sort((a, b) => a.timestamp.localeCompare(b.timestamp) || a.id.localeCompare(b.id));
  return reuseStableRoomMessages(left as readonly VioletRoomMessage[], merged);
}

function mergeAgentBusReceipts(
  left?: readonly AgentBusReceipt[],
  right?: readonly AgentBusReceipt[],
): AgentBusReceipt[] | undefined {
  const merged = [...(left ?? []), ...(right ?? [])];
  if (merged.length === 0) return undefined;
  const byKey = new Map<string, AgentBusReceipt>();
  for (const receipt of merged) {
    if (!receipt.eventId || !receipt.agentId) continue;
    const key = `${receipt.eventId}\u001f${receipt.agentId}`;
    const existing = byKey.get(key);
    if (!existing || receipt.timestamp < existing.timestamp) {
      byKey.set(key, receipt);
    }
  }
  const next = Array.from(byKey.values()).sort((a, b) => (
    a.timestamp.localeCompare(b.timestamp) ||
    a.agentId.localeCompare(b.agentId) ||
    a.eventId.localeCompare(b.eventId)
  ));
  return next.length > 0 ? next : undefined;
}

function sameAgentBusReceipts(
  left?: readonly AgentBusReceipt[],
  right?: readonly AgentBusReceipt[],
): boolean {
  const leftItems = left ?? [];
  const rightItems = right ?? [];
  if (leftItems.length !== rightItems.length) return false;
  for (let index = 0; index < leftItems.length; index += 1) {
    const l = leftItems[index]!;
    const r = rightItems[index]!;
    if (
      l.eventId !== r.eventId ||
      l.agentId !== r.agentId ||
      l.timestamp !== r.timestamp
    ) {
      return false;
    }
  }
  return true;
}

function sameRoomMessages(left: readonly VioletChatMessage[], right: readonly VioletChatMessage[]): boolean {
  if (left.length !== right.length) return false;
  for (let index = 0; index < left.length; index += 1) {
    if (!sameRoomMessage(left[index]!, right[index]!)) return false;
  }
  return true;
}

function reuseStableRoomMessages<T extends VioletChatMessage>(
  previous: readonly T[],
  next: readonly T[],
): T[] {
  if (next.length === 0) return previous.length === 0 ? (previous as T[]) : [];
  if (previous.length === 0) return [...next];

  const previousById = new Map<string, T>();
  for (const message of previous) {
    if (!previousById.has(message.id)) previousById.set(message.id, message);
  }

  let sameOrderAndRefs = previous.length === next.length;
  const stable = next.map((message, index) => {
    const existing = previousById.get(message.id);
    if (existing && sameRoomMessage(existing, message)) {
      if (previous[index] !== existing) sameOrderAndRefs = false;
      return existing;
    }
    sameOrderAndRefs = false;
    return message;
  });

  return sameOrderAndRefs ? (previous as T[]) : stable;
}

function sameRoomMessage(left: VioletChatMessage, right: VioletChatMessage): boolean {
  const leftRoom = left as VioletRoomMessage;
  const rightRoom = right as VioletRoomMessage;
  return (
    left.id === right.id &&
    left.sessionId === right.sessionId &&
    left.agentId === right.agentId &&
    left.shell === right.shell &&
    left.role === right.role &&
    left.kind === right.kind &&
    left.timestamp === right.timestamp &&
    left.text === right.text &&
    nullishString(left.sourcePath) === nullishString(right.sourcePath) &&
    nullishString(left.nativeEventId) === nullishString(right.nativeEventId) &&
    nullishString(left.agentDisplayName) === nullishString(right.agentDisplayName) &&
    nullishString(left.agentAvatarId) === nullishString(right.agentAvatarId) &&
    nullishString(left.agentProvider) === nullishString(right.agentProvider) &&
    nullishString(left.agentStatus) === nullishString(right.agentStatus) &&
    targetAgentIdsKey(left.targetAgentIds) === targetAgentIdsKey(right.targetAgentIds) &&
    Boolean(leftRoom.local) === Boolean(rightRoom.local) &&
    Boolean(leftRoom.privacy) === Boolean(rightRoom.privacy) &&
    composerMentionsKey(leftRoom.composerMentions) === composerMentionsKey(rightRoom.composerMentions) &&
    nullishString(leftRoom.deliveryStatus) === nullishString(rightRoom.deliveryStatus) &&
    nullishString(leftRoom.deliveryReason) === nullishString(rightRoom.deliveryReason) &&
    targetAgentIdsKey(leftRoom.deliveryRetryTargetAgentIds) === targetAgentIdsKey(rightRoom.deliveryRetryTargetAgentIds) &&
    Boolean(leftRoom.ghostSasayaki) === Boolean(rightRoom.ghostSasayaki) &&
    leftRoom.ghostSasayakiSortTime === rightRoom.ghostSasayakiSortTime &&
    normalizeVioletProjectRoot(leftRoom.projectRoot) === normalizeVioletProjectRoot(rightRoom.projectRoot) &&
    progressItemsKey(leftRoom.progressItems) === progressItemsKey(rightRoom.progressItems) &&
    progressEntriesKey(leftRoom.progressEntries) === progressEntriesKey(rightRoom.progressEntries)
  );
}

function sameAgentSet(left: readonly string[], right: readonly string[]): boolean {
  if (left.length !== right.length) return false;
  const seen = new Set(left);
  return right.every((item) => seen.has(item));
}

function targetAgentIdsKey(agentIds?: readonly string[]): string {
  return agentIds && agentIds.length > 0 ? [...agentIds].sort().join(',') : '';
}

function composerMentionsKey(mentions?: readonly { agentId: string; aka: string }[]): string {
  return mentions && mentions.length > 0
    ? mentions.map((mention) => `${mention.agentId}:${mention.aka}`).sort().join(',')
    : '';
}

function progressItemsKey(items?: readonly string[]): string {
  return items && items.length > 0 ? items.join('\u001f') : '';
}

function progressEntriesKey(entries?: readonly VioletProgressEntry[]): string {
  if (!entries || entries.length === 0) return '';
  return entries.map((entry) => [
    entry.agentId,
    entry.shell,
    entry.timestamp,
    entry.text,
    nullishString(entry.agentDisplayName),
    nullishString(entry.agentAvatarId),
    nullishString(entry.agentProvider),
    nullishString(entry.agentStatus),
  ].join('\u001e')).join('\u001f');
}

function nullishString(value: string | null | undefined): string {
  return value ?? '';
}

function violetSyncMatchesProject(panelRoot: string | null, requestRoot: string | null): boolean {
  if (!panelRoot || !requestRoot) return true;
  return panelRoot === requestRoot;
}

function violetProjectRootsEqual(left: string | null, right: string | null): boolean {
  return normalizeVioletProjectRoot(left) === normalizeVioletProjectRoot(right);
}

function normalizeVioletProjectRoot(projectRoot?: string | null): string | null {
  const trimmed = projectRoot?.trim();
  return trimmed ? trimmed : null;
}

function isNativeUserPrompt(message: VioletChatMessage): boolean {
  return message.role === 'user' && message.kind === 'message' && message.agentId !== 'user';
}

function isDuplicateNativeUserMessage(
  nativeMessage: VioletChatMessage,
  localMessages: readonly VioletRoomMessage[],
): boolean {
  if (nativeMessage.role !== 'user') return false;
  const nativeText = normalizeForDedupe(nativeMessage.text);
  if (!nativeText) return false;
  const nativeAttachmentFree = normalizeAttachmentInsensitive(nativeMessage.text);
  const nativeTime = Date.parse(nativeMessage.timestamp);
  return localMessages.some((local) => {
    if (local.role !== 'user') return false;
    if (!local.targetAgentIds?.includes(nativeMessage.agentId)) return false;
    if (normalizeForDedupe(local.text) !== nativeText) {
      // Attachment sends never match verbatim: the composer payload carries
      // the attachment *path* inline while the CLI's native log rewrites it
      // to a provider marker ("[Image #1]…"). Compare again with both kinds
      // of attachment decoration stripped. If both sides strip down to empty,
      // treat it as an attachment-only prompt and confirm by target + time.
      const localAttachmentFree = normalizeAttachmentInsensitive(local.text);
      if (localAttachmentFree !== nativeAttachmentFree) {
        return false;
      }
    }
    const localTime = Date.parse(local.timestamp);
    if (!Number.isFinite(nativeTime) || !Number.isFinite(localTime)) return true;
    return Math.abs(nativeTime - localTime) < 15 * 60 * 1000;
  });
}

/* Erase attachment decorations from both sides of the local/native compare:
   provider markers ("[Image #1]"), provider source trailers
   ("[Image: source: /path/to.png]"), and inline attachment paths the
   composer serializes for chips. */
export function normalizeAttachmentInsensitive(text: string): string {
  return normalizeForDedupe(
    text
      .replace(/\[[^\]\n#]{1,40}#\d+\]/g, ' ')
      .replace(/\[[^\]\n:]{1,20}: source: [^\]\n]+\]/g, ' ')
      .replace(/(?:~\/|\/)?\S*project-memory\/attachments\/\S+/g, ' '),
  );
}

export function normalizeForDedupe(text: string): string {
  return normalizeSharedAttachmentPathsForDedupe(trimTerminalControlPadding(text))
    .replace(/\s+/g, ' ')
    .trim();
}

function normalizeSharedAttachmentPathsForDedupe(text: string): string {
  return text.replace(
    /(^|[\s([{"'`])(?:~|\/\S+)?\/project-memory\/attachments\//g,
    '$1project-memory/attachments/',
  );
}

function shortAgentLabel(agentId: string, agentMeta?: Readonly<Record<AgentId, Agent>>): string {
  const name = agentMeta?.[agentId]?.name ?? agentId;
  const parts = splitProjectAgentName(name);
  return [
    parts.title?.abbr,
    parts.base,
    parts.bunshin,
  ].filter(Boolean).join(' ');
}

function formatAgentList(names: readonly string[]): string {
  if (names.length === 0) return '';
  if (names.length === 1) return `@${names[0]}`;
  if (names.length === 2) return `@${names[0]} and @${names[1]}`;
  return `${names.slice(0, -1).map((name) => `@${name}`).join(', ')}, and @${names[names.length - 1]}`;
}

function agentSnapshotFromMessage(message: VioletRoomMessage): Agent | undefined {
  if (!message.agentDisplayName && !message.agentAvatarId && !message.agentProvider && !message.agentStatus) {
    return undefined;
  }
  const provider = message.agentProvider ?? message.shell;
  return {
    name: message.agentDisplayName ?? message.agentId,
    emoji: '◇',
    role: 'Project agent',
    hue: 'var(--brass-bright)',
    avatarId: message.agentAvatarId ?? null,
    avatarClass: avatarClassForId(message.agentAvatarId, provider),
    lifecycleStatus: projectAgentLifecycleStatus(message.agentStatus) ?? undefined,
  };
}

function agentSnapshotFromProgressEntry(entry: VioletProgressEntry): Agent | undefined {
  if (!entry.agentDisplayName && !entry.agentAvatarId && !entry.agentProvider && !entry.agentStatus) {
    return undefined;
  }
  const provider = entry.agentProvider ?? entry.shell;
  return {
    name: entry.agentDisplayName ?? entry.agentId,
    emoji: '◇',
    role: 'Project agent',
    hue: 'var(--brass-bright)',
    avatarId: entry.agentAvatarId ?? null,
    avatarClass: avatarClassForId(entry.agentAvatarId, provider),
    lifecycleStatus: projectAgentLifecycleStatus(entry.agentStatus) ?? undefined,
  };
}

function projectAgentLifecycleStatus(status: string | null | undefined): 'archived' | 'left' | null {
  const normalized = (status ?? '').trim().toLowerCase();
  if (normalized === 'archived') return 'archived';
  if (normalized === 'dismissed' || normalized === 'removed' || normalized === 'deleted' || normalized === 'left') return 'left';
  return null;
}

function providerAvatarClass(id: string): string {
  if (id === 'bartender') return 'system-bartender';
  if (id === 'bbs') return 'system-bbs';
  if (id === 'ember') return 'system-ember';
  if (id === 'laughing-man') return 'system-laughing-man';
  if (id === 'puppeteer') return 'system-puppeteer';
  if (id === 'violet') return 'system-violet';
  const baseId = id.replace(/-bunshin.*$/i, '').replace(/-\d+$/, '');
  if (baseId === 'hero-cc') return 'provider-claude';
  if (baseId === 'hero-dex') return 'provider-codex';
  if (baseId === 'hero-gem') return 'provider-antigravity';
  if (baseId === 'hero-op') return 'provider-opencode';
  if (baseId === 'claude') return 'provider-claude';
  if (baseId === 'codex') return 'provider-codex';
  if (baseId === 'alice') return 'provider-claude';
  if (baseId === 'bob') return 'provider-codex';
  if (baseId === 'david') return 'provider-antigravity';
  if (baseId === 'charlie') return 'provider-opencode';
  return 'provider-codex';
}

function systemActorName(id: string): string | null {
  if (id === 'bartender') return 'Bartender';
  if (id === 'bbs') return 'BBS';
  if (id === 'ember') return 'Ember';
  if (id === 'laughing-man') return 'Laughing Man';
  if (id === 'puppeteer') return 'Puppeteer';
  if (id === 'violet') return 'Violet';
  return null;
}

function systemActorDescription(id: string): string | null {
  if (id === 'bartender') {
    return "I am Bartender for this project. I keep everyone's worktree synced.";
  }
  if (id === 'bbs') return 'I am BBS. I handle cross-project Bulletin Board handoff entrypoints.';
  if (id === 'ember') return 'I am Ember. I watch reminders and follow-up tasks.';
  if (id === 'laughing-man') return 'I am Laughing Man. I route room updates to external channels.';
  if (id === 'puppeteer') return 'I am Puppeteer. I coordinate scripted project actions.';
  if (id === 'violet') return 'I am Violet. I keep the room history organized.';
  return null;
}

const VIOLET_DATE_MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function formatTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  const time = VIOLET_TIME_FORMATTER.format(date);
  if (isSameLocalDay(date, new Date())) return time;
  return `${time} [${formatLocalDateLabel(date)}]`;
}

function isSameLocalDay(left: Date, right: Date): boolean {
  return left.getFullYear() === right.getFullYear()
    && left.getMonth() === right.getMonth()
    && left.getDate() === right.getDate();
}

function formatLocalDateLabel(date: Date): string {
  const month = VIOLET_DATE_MONTHS[date.getMonth()] ?? String(date.getMonth() + 1);
  return `${month}-${date.getDate()}-${date.getFullYear()}`;
}

function basename(value: string): string {
  return value.split('/').pop() || value;
}
