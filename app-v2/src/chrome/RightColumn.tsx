/** RightColumn — system agents + memory summary.
 *  Ported from `.context/attachments/cd-handoff-3/…/rightcol.jsx`. */

import { AnimatePresence, motion } from 'framer-motion';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CSSProperties, PointerEvent as ReactPointerEvent, ReactNode } from 'react';
import {
  renderBbsReplyPromptFromFile,
  type BbsPromptProject,
} from '../bbs-config';
import {
  TAVERN_SYSTEM_CONFIG_CHANGED_EVENT,
  loadBartenderConflictPrompts,
  loadBartenderConflictPromptsFromFiles,
  type BartenderConflictPrompts,
} from '../bartender-config';
import {
  VIOLET_SUMMARY_CLI_TIMEOUT_SECS,
  loadVioletSummaryConfig,
  type VioletSummaryConfig,
} from '../violet-summary-config';
import dreamVideoC01 from '../assets/ember-dream/videos/C01-final-loop-4s-medium.m4v?url';
import dreamVideoC03 from '../assets/ember-dream/videos/C03-final-loop-4s-medium.m4v?url';
import dreamVideoC05 from '../assets/ember-dream/videos/C05-final-loop-4s-medium.m4v?url';
import dreamVideoC07 from '../assets/ember-dream/videos/C07-final-loop-4s-medium.m4v?url';
import dreamVideoC08 from '../assets/ember-dream/videos/C08-final-loop-4s-medium.m4v?url';
import dreamVideoC09 from '../assets/ember-dream/videos/C09-final-loop-4s-medium.m4v?url';
import dreamVideoC10 from '../assets/ember-dream/videos/C10-final-loop-4s-medium.m4v?url';
import dreamVideoC11 from '../assets/ember-dream/videos/C11-final-loop-4s-medium.m4v?url';
import dreamVideoD00 from '../assets/ember-dream/videos/D00-final-loop-4s-medium.m4v?url';
import dreamVideoF00 from '../assets/ember-dream/videos/F00-final-loop-4s-medium.m4v?url';
import dreamVideoT00 from '../assets/ember-dream/videos/T00-final-loop-4s-medium.m4v?url';
import dreamVideoW00 from '../assets/ember-dream/videos/W00-final-loop-4s-medium.m4v?url';
import violetAvatarUrl from '../assets/tavern/optimized/avatars/violet.webp';
import { SCENES, type SceneKey } from '../mock/fixtures';
import {
  agentBusSend,
  bbsDelete,
  bbsHumanPost,
  bbsHumanReply,
  fileImageDataUrl,
  lmMessageLog,
  lmSetMuted,
  lmStart,
  lmStatus,
  lmStandbyDeleteQueued,
  lmStandbyQueue,
  lmStandbySendQueued,
  type LmLogEntry,
  type LmStandbyQueueItem,
  type LmStatus,
  bbsMarkProcessed,
  bbsSnapshot,
  loadAccountUserIdentity,
  bartenderFetch,
  bartenderPullFromGithub,
  bartenderPushToGithub,
  bartenderRoutePullConflict,
  bartenderStatus,
  bartenderSyncLocal,
  bartenderSyncReceipt,
  emberDeliverHumanReminder,
  emberConsolidateDreams,
  emberPrepareDreams,
  emberScheduleSave,
  emberScheduleState,
  hasTauriRuntime,
  onBartenderSyncEvent,
  onBartenderSyncProgressEvent,
  onEmberSchedulesChanged,
  readVioletSummary,
  summarizeVioletAuto,
  summarizeVioletNow,
  type AgentBusSendResult,
  type BbsPost,
  type BbsSnapshot,
  type BbsThread,
  type BartenderConflict,
  type BartenderPullConflict,
  type BartenderSyncEvent,
  type BartenderSyncProgressEvent,
  type BartenderStatus,
  type AccountUserIdentity,
  type EmberScheduleState,
  type VioletSummaryEntry,
  type VioletSummaryState,
  type WorkspaceProject,
} from '../pty-client';
import { VIOLET_COMPOSER_SENT_EVENT, lastVioletComposerSentAt } from './violet-room-events';
import { InputBar, type ComposerAttachment, type InputBarHandle } from './InputBar';
import { LaughingManSettings } from './LaughingManSettings';
import { MarkdownText } from './VioletRoomPanel';
import { splitProjectAgentName } from './ProjectAgentName';
import { EmberScheduleInstrument } from './EmberScheduleInstrument';
import type { Agent, AgentId } from '../types/scene';
import type { LogRow as LogRowType } from '../types/scene';
import { avatarClassForAgentFallback, avatarClassForId, avatarImageStyleForId } from '../lib/hero-avatars';
import {
  createEmberDraft,
  createEmberHistoryRecord,
  createEmberSchedule,
  emberActorHuman,
  EMBER_NOT_DELIVERED,
  HUMAN_TELEGRAM_TARGET_ID,
  emberScheduleSummary,
  emberScheduleTargetIds,
  emberScheduleTargetLabel,
  emberScheduleTargetNames,
  emberTimeLabel,
  failedEmberSchedule,
  isHumanTelegramTarget,
  loadEmberState,
  renderDreamPromptFromFile,
  renderEmberReminderPrompt,
  rescheduleEmberSchedule,
  resumeEmberSchedule,
  saveEmberState,
  isRepeatingEmberSchedule,
  type EmberDelayUnit,
  type EmberDraft,
  type EmberEndMode,
  type EmberHistoryRecord,
  type EmberRepeatKind,
  type EmberSchedule,
  type EmberState,
} from '../ember-config';

const HOT_MEMORY_TEASER_ENABLED = false;
const BARTENDER_COLD_MS = 3 * 60 * 1000;
const BARTENDER_SYNC_RECEIPT_TIMEOUT_MS = 10 * 60 * 1000;
const BARTENDER_AUTO_SYNC_STORAGE_PREFIX = 'kota-v2.bartender.auto-sync.';
const EMPTY_AGENT_IDS: readonly AgentId[] = [];
const EMBER_ACTOR_ID = 'ember';
const EMBER_ACTOR_NAME = 'Ember';
const BBS_ACTOR_ID = 'bbs';
const BBS_ACTOR_NAME = 'BBS';
const EMBER_DREAM_TITLE = "It's time to dream.";
const ROCKER_COVER_CLOSE_MS = 1700;
const DREAM_AGENT_OVERLAY_TTL_MS = 30 * 60 * 1000;
const DREAM_MIN_CONSOLIDATE_DELAY_MS = 120_000;
const DREAM_DIGEST_WAIT_MESSAGE = 'Last dreams is still being digested, waiting...';
let emberWorkSessionStartedAt = Date.now();
type DreamOverlayState = 'off' | 'wrapping' | 'countdown' | 'dreaming' | 'finished';
type DreamVideoPhase = Exclude<DreamOverlayState, 'off'>;
type DreamVideoSet = {
  id: string;
  clips: Record<DreamVideoPhase, string>;
};
type DreamAgentOverlay = {
  runId: string;
  agentId: AgentId;
  phase: 'pending' | 'dreaming' | 'completed';
  assignedAt: number;
  startedAt?: number;
  completedAt?: number;
  blockUntilIdle?: boolean;
};
export type DreamProjectTarget = {
  projectId: string;
  projectRoot: string;
  projectName: string;
  agents: readonly {
    id: AgentId;
    name: string;
  }[];
};
export type ResolveDreamProjectTargets = () => Promise<readonly DreamProjectTarget[]> | readonly DreamProjectTarget[];
type EmberModalTab = 'scheduled' | 'drafts' | 'history';
type EmberSendMode = 'idle' | 'delay' | 'at';
type EmberEditorTarget = { kind: 'new' | 'schedule' | 'draft'; id?: string };
type EmberTargetOption = { id: AgentId; name: string; kind: 'agent' | 'human' };
type BbsFilter = 'tagged' | 'all';
type BartenderConflictBlocker = {
  agentId: AgentId;
  agentName: string;
  count: number;
  observedWorking: boolean;
};
type BartenderExternalSyncActivity = {
  projectRoot: string;
  requestId: string;
  startedAt: number;
  progress: BartenderSyncProgressEvent;
};

const DREAM_VIDEO_SETS: readonly DreamVideoSet[] = [
  {
    id: 'set-01',
    clips: {
      wrapping: dreamVideoW00,
      countdown: dreamVideoT00,
      dreaming: dreamVideoC05,
      finished: dreamVideoF00,
    },
  },
  {
    id: 'set-02',
    clips: {
      wrapping: dreamVideoC01,
      countdown: dreamVideoD00,
      dreaming: dreamVideoC07,
      finished: dreamVideoC09,
    },
  },
  {
    id: 'set-03',
    clips: {
      wrapping: dreamVideoC03,
      countdown: dreamVideoC11,
      dreaming: dreamVideoC08,
      finished: dreamVideoC10,
    },
  },
] as const;

function randomDreamVideoSetIndex(): number {
  return Math.floor(Math.random() * DREAM_VIDEO_SETS.length);
}

function LaughingManMuteIcon({ muted }: { muted: boolean }) {
  return (
    <svg className="lm-mute-icon" viewBox="0 0 1024 1024" aria-hidden>
      {muted ? (
        <path
          fill="currentColor"
          d="M571.32 704a76.36 76.36 0 0 1-144.19 0zm-258.84-51.2L623.3 342c-8.46-25.2-31.52-39-73.21-44.49.06-1 .31-2 .31-3.06a51.2 51.2 0 1 0-102.4 0c0 1.05.25 2 .31 3.06-54.12 7.12-77.11 28.06-77.11 70.08v29.21c0 121.6-66.51 175.51-102.4 204.8.4.4 0 51.2 0 51.2zm315-248.63l119-119-16.6-16.6-461.32 461.31 16.58 16.6 93.68-93.68H729.6s-.4-50.8 0-51.2c-35.14-28.68-99.47-81.13-102.15-197.43z"
        />
      ) : (
        <path
          fill="currentColor"
          d="M439.9 716.8h144.2C573.5 746.6 545.4 768 512 768s-61.5-21.4-72.1-51.2zm302.5-102.4C706.5 585.1 640 531.2 640 409.6v-29.3c0-42-23-63-77.1-70.1.1-1 .3-2 .3-3.1 0-28.3-22.9-51.2-51.2-51.2s-51.2 22.9-51.2 51.2c0 1 .2 2 .3 3.1-54.1 7.1-77.1 28.1-77.1 70.1v29.3c0 121.6-66.5 175.5-102.4 204.8.4.4 0 51.2 0 51.2h460.8s-.4-50.8 0-51.2z"
        />
      )}
    </svg>
  );
}

function conflictBlockerFromConflicts(
  conflicts: readonly BartenderConflict[] | undefined,
  agentMeta: Readonly<Record<AgentId, Agent>> | undefined,
  workingAgents: ReadonlySet<AgentId> | undefined,
): BartenderConflictBlocker | null {
  const first = conflicts?.[0];
  if (!first?.agentId) return null;
  const agentId = first.agentId as AgentId;
  return {
    agentId,
    agentName: agentMeta?.[agentId]?.name ?? first.agentId,
    count: conflicts?.length ?? 1,
    observedWorking: workingAgents?.has(agentId) ?? false,
  };
}

function bartenderAutoSyncStorageKey(projectId: string | null | undefined): string | null {
  return projectId ? `${BARTENDER_AUTO_SYNC_STORAGE_PREFIX}${projectId}` : null;
}

function loadBartenderAutoSync(projectId: string | null | undefined): boolean {
  const key = bartenderAutoSyncStorageKey(projectId);
  if (!key || typeof window === 'undefined') return false;
  try {
    return window.localStorage.getItem(key) === 'true';
  } catch {
    return false;
  }
}

function saveBartenderAutoSync(projectId: string | null | undefined, enabled: boolean): void {
  const key = bartenderAutoSyncStorageKey(projectId);
  if (!key || typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(key, String(enabled));
  } catch {
    // Ignore storage failures; the toggle still works for the current render.
  }
}

function bartenderSyncButtonLabel(progress: BartenderSyncProgressEvent | null): string {
  if (!progress) return 'Syncing local';
  const step = ({
    starting: 'Starting',
    waiting_status: 'Waiting status',
    preparing: 'Preparing',
    checking_source: 'Checking source',
    snapshot_source: 'Snapshotting source',
    checking_agents: 'Checking agents',
    snapshot_agent: 'Snapshotting agents',
    publishing_agents: 'Publishing agents',
    refreshing_agents: 'Refreshing agents',
    refreshing_status: 'Refreshing status',
    blocked: 'Blocked',
    failed: 'Failed',
    finished: 'Done',
  } as Record<string, string>)[progress.phase] ?? progress.message;
  return `Syncing · ${step}`;
}

function emberStateSignature(state: EmberState): string {
  return JSON.stringify(state);
}

function emberStateHasContent(state: EmberState): boolean {
  return state.drafts.length > 0 || state.schedules.length > 0 || state.history.length > 0;
}

function samePathString(left: string | null | undefined, right: string | null | undefined): boolean {
  return bartenderPathKey(left) === bartenderPathKey(right);
}

function bartenderPathKey(value: string | null | undefined): string {
  return (value ?? '').replace(/\/+$/, '');
}

function emberScheduleAttribution(schedule: EmberSchedule): string {
  const updated = schedule.updatedBy?.label || schedule.createdBy?.label;
  if (updated) return `Updated by ${updated}`;
  return 'Updated by Human';
}

function formatBbsTime(value: string): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}

function formatLmQueueTime(value: string | null | undefined): string {
  if (!value) return '';
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  const now = new Date();
  const sameDay = date.getFullYear() === now.getFullYear()
    && date.getMonth() === now.getMonth()
    && date.getDate() === now.getDate();
  return new Intl.DateTimeFormat(undefined, sameDay ? {
    hour: 'numeric',
    minute: '2-digit',
  } : {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}

function formatSummaryTime(value: string | null | undefined, emptyLabel = 'Never'): string {
  if (!value) return emptyLabel;
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}

function formatSummaryClock(value: string | null | undefined): string {
  if (!value) return '';
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}

function sameSummaryDate(left: string | null | undefined, right: string | null | undefined): boolean {
  if (!left || !right) return false;
  const leftDate = new Date(left);
  const rightDate = new Date(right);
  if (!Number.isFinite(leftDate.getTime()) || !Number.isFinite(rightDate.getTime())) return false;
  return (
    leftDate.getFullYear() === rightDate.getFullYear() &&
    leftDate.getMonth() === rightDate.getMonth() &&
    leftDate.getDate() === rightDate.getDate()
  );
}

function summaryRangeLabel(
  start: string | null | undefined,
  end: string | null | undefined,
  count: number,
): string | null {
  if (!start && !end) return null;
  const startLabel = formatSummaryTime(start, 'Beginning');
  const endLabel = sameSummaryDate(start, end) ? formatSummaryClock(end) : formatSummaryTime(end, 'Now');
  return `${startLabel} - ${endLabel} · ${summaryMessageLabel(count)} summarized`;
}

function summaryMessageLabel(count: number): string {
  return `${count} ${count === 1 ? 'turn' : 'turns'}`;
}

function formatSummaryCountdown(ms: number): string {
  const totalSeconds = Math.max(0, Math.ceil(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function summaryAutoCountdownDueAt(
  latest: VioletSummaryEntry | null,
  outstanding: number,
  triggerMessages: number,
  triggerHours: number,
  triggerMinOutstanding: number,
): number | null {
  if (outstanding <= 0) return null;
  if (outstanding >= triggerMessages) return null;
  if (outstanding <= triggerMinOutstanding || !latest) return null;
  const lastSummary = new Date(latest.updatedAt).getTime();
  if (!Number.isFinite(lastSummary)) return null;
  return lastSummary + triggerHours * 60 * 60 * 1000;
}

function summaryAutoCountdownLabel(
  latest: VioletSummaryEntry | null,
  outstanding: number,
  triggerMessages: number,
  triggerHours: number,
  triggerMinOutstanding: number,
  nowMs: number,
): string | null {
  if (outstanding <= 0) return null;
  if (outstanding >= triggerMessages) return 'Auto summary due';
  const dueAt = summaryAutoCountdownDueAt(
    latest,
    outstanding,
    triggerMessages,
    triggerHours,
    triggerMinOutstanding,
  );
  if (!dueAt) return null;
  const remaining = dueAt - nowMs;
  if (remaining <= 0) return 'Auto summary due';
  return `Auto summary in ${formatSummaryCountdown(remaining)}`;
}

function normalizeProjectRoot(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;
  return trimmed.replace(/\/+$/, '');
}

function sameProjectRoot(a: string | null | undefined, b: string | null | undefined): boolean {
  const left = normalizeProjectRoot(a);
  const right = normalizeProjectRoot(b);
  return !!left && !!right && left === right;
}

function violetSummaryEntryView(entry: VioletSummaryEntry | null | undefined) {
  if (!entry) return null;
  return {
    id: entry.id,
    updatedAt: entry.updatedAt,
    trigger: entry.trigger,
    provider: entry.provider,
    summaryStartTs: entry.summaryStartTs,
    summaryEndTs: entry.summaryEndTs,
    messageCount: entry.messageCount,
    completed: entry.completed,
    lastEventId: entry.lastEventId,
    cliError: entry.cliError ?? null,
  };
}

function violetSummaryViewKey(state: VioletSummaryState | null): string {
  if (!state) return 'null';
  return JSON.stringify({
    latest: violetSummaryEntryView(state.latest),
    history: state.history.map(violetSummaryEntryView),
    outstanding: state.outstanding,
    logPath: state.logPath,
    promptPath: state.promptPath,
    error: state.error ?? null,
  });
}

function violetSummaryAutoAttemptKey(
  projectRoot: string | null | undefined,
  state: VioletSummaryState | null,
  config: VioletSummaryConfig,
): string | null {
  const root = normalizeProjectRoot(projectRoot);
  const outstanding = state?.outstanding;
  if (!root || !outstanding || outstanding.messageCount <= 0) return null;
  return JSON.stringify({
    root,
    sinceTs: outstanding.sinceTs,
    messageCount: outstanding.messageCount,
    provider: config.provider,
    triggerMessages: config.triggerAMessages,
    triggerHours: config.triggerBHours,
    triggerMinOutstanding: config.triggerBMinOutstanding,
  });
}

function violetSummaryIsAutoDue(
  state: VioletSummaryState | null,
  config: VioletSummaryConfig,
  nowMs: number,
): boolean {
  const outstanding = state?.outstanding.messageCount ?? 0;
  if (outstanding <= 0) return false;
  if (outstanding >= config.triggerAMessages) return true;
  const dueAt = summaryAutoCountdownDueAt(
    state?.latest ?? null,
    outstanding,
    config.triggerAMessages,
    config.triggerBHours,
    config.triggerBMinOutstanding,
  );
  return dueAt !== null && nowMs >= dueAt;
}



function removeBbsPostFromSnapshot(snapshot: BbsSnapshot | null, post: BbsPost): BbsSnapshot | null {
  if (!snapshot) return snapshot;
  const threads = snapshot.threads.flatMap((thread) => {
    if (thread.threadId !== post.threadId) return [thread];
    if (post.kind === 'topic' || thread.posts.length <= 1) return [];
    const posts = thread.posts.filter((candidate) => candidate.postId !== post.postId);
    if (posts.length === 0) return [];
    const latest = posts.reduce((current, candidate) => (
      candidate.createdAt > current.createdAt ? candidate : current
    ), posts[0]!);
    return [{
      ...thread,
      posts,
      latestPostId: latest.postId,
      updatedAt: latest.createdAt,
      isNew: posts.some((candidate) => candidate.state === 'new'),
    }];
  });
  return {
    ...snapshot,
    threads,
    newCount: threads.filter((thread) => thread.isNew).length,
  };
}

function workspaceProjectDisplayName(workspace: WorkspaceProject): string {
  const parts = workspace.repoFullName.trim().split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? workspace.projectId;
}

function workspacePromptProject(workspace: WorkspaceProject): BbsPromptProject {
  return {
    projectId: workspace.projectId,
    displayName: workspaceProjectDisplayName(workspace),
  };
}

const bbsImageDataUrlCache = new Map<string, string>();
const BBS_IMAGE_PATH_RE = /(?:^|[\s("'\`])((?:~\/|\/)?[^\s)"'\`]*(?:attachments|images?|screenshots?)[^\s)"'\`]*\.(?:png|jpe?g|gif|webp)|(?:~\/|\/)[^\s)"'\`]+\.(?:png|jpe?g|gif|webp))/gi;

function bbsImagePathsFromBody(body: string): string[] {
  const out: string[] = [];
  for (const match of body.matchAll(BBS_IMAGE_PATH_RE)) {
    const raw = (match[1] ?? '').trim();
    if (raw && !out.includes(raw)) out.push(raw);
    if (out.length >= 6) break;
  }
  return out;
}

function resolveBbsImagePath(path: string, baseRoot: string | null): string | null {
  if (path.startsWith('/')) return path;
  if (path.startsWith('~/')) return null;
  if (!baseRoot) return null;
  return `${baseRoot.replace(/\/$/, '')}/${path}`;
}

function BbsInlineImage({ path }: { path: string }) {
  const [src, setSrc] = useState<string | null>(() => bbsImageDataUrlCache.get(path) ?? null);
  useEffect(() => {
    if (bbsImageDataUrlCache.has(path)) {
      setSrc(bbsImageDataUrlCache.get(path) ?? null);
      return;
    }
    let cancelled = false;
    void fileImageDataUrl(path)
      .then((dataUrl) => {
        bbsImageDataUrlCache.set(path, dataUrl);
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
  return <img className="bbs-post-image" src={src} alt={path.split('/').pop() ?? 'attachment'} />;
}

function BbsPostImages({ body, baseRoot }: { body: string; baseRoot: string | null }) {
  const paths = useMemo(() => (
    bbsImagePathsFromBody(body)
      .map((path) => resolveBbsImagePath(path, baseRoot))
      .filter((path): path is string => !!path)
  ), [baseRoot, body]);
  if (paths.length === 0) return null;
  return (
    <div className="bbs-post-images">
      {paths.map((path) => <BbsInlineImage key={path} path={path} />)}
    </div>
  );
}

function emberProjectKey(workspace: WorkspaceProject | null | undefined, projectRoot: string | null | undefined): string | null {
  return workspace?.projectId ?? workspace?.localRoot ?? projectRoot ?? null;
}

function sortedEmberSchedules(schedules: readonly EmberSchedule[]): EmberSchedule[] {
  return [...schedules].sort((a, b) => {
    const aStatus = emberStatusRank(a.status);
    const bStatus = emberStatusRank(b.status);
    if (aStatus !== bStatus) return aStatus - bStatus;
    return Date.parse(a.nextRunAt) - Date.parse(b.nextRunAt);
  });
}

function emberStatusRank(status: EmberSchedule['status']): number {
  if (status === 'scheduled') return 0;
  if (status === 'paused') return 1;
  if (status === 'failed') return 2;
  return 3;
}

function compactEmberText(text: string): string {
  const value = text.replace(/\s+/g, ' ').trim();
  if (value.length <= 84) return value;
  return `${value.slice(0, 83)}...`;
}

function emberAgentName(agentId: AgentId, agentMeta: Readonly<Record<AgentId, Agent>> | undefined): string {
  return agentMeta?.[agentId]?.name ?? agentId;
}

function emberEventId(kind: string, id: string, targetAgentId: AgentId): string {
  return `ember-${kind}-${id}-${targetAgentId}-${Date.now()}`.replace(/[^a-zA-Z0-9._:-]+/g, '-');
}

function dreamAgentList(names: readonly string[]): string {
  if (names.length === 0) return 'Ember';
  if (names.length === 1) return names[0]!;
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.slice(0, -1).join(', ')}, and ${names[names.length - 1]}`;
}

function dreamScopeLabel(projects: readonly DreamProjectTarget[]): string | null {
  if (projects.length <= 1) return null;
  const agentCount = projects.reduce((sum, project) => sum + project.agents.length, 0);
  const projectWord = projects.length === 1 ? 'project' : 'projects';
  const agentWord = agentCount === 1 ? 'agent' : 'agents';
  return `${agentCount} ${agentWord} across ${projects.length} ${projectWord}`;
}

function normalizeDreamProjectTargets(
  projects: readonly DreamProjectTarget[],
  agentMeta: Readonly<Record<AgentId, Agent>> | undefined,
): DreamProjectTarget[] {
  const seenProjects = new Set<string>();
  return projects.flatMap((project) => {
    const root = project.projectRoot.trim();
    const projectKey = `${project.projectId}:${root}`;
    if (!root || seenProjects.has(projectKey)) return [];
    const seenAgents = new Set<AgentId>();
    const agents = project.agents.flatMap((agent) => {
      if (!agent.id || seenAgents.has(agent.id)) return [];
      seenAgents.add(agent.id);
      return [{
        id: agent.id,
        name: agent.name.trim() || emberAgentName(agent.id, agentMeta),
      }];
    });
    if (agents.length === 0) return [];
    seenProjects.add(projectKey);
    return [{ ...project, projectRoot: root, agents }];
  });
}

function workingStartedAtMapKey(startedAt: ReadonlyMap<AgentId, string> | undefined): string {
  return Array.from(startedAt ?? [])
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([agentId, value]) => `${agentId}:${value}`)
    .join('|');
}

function dreamOverlayMapKey(map: ReadonlyMap<AgentId, DreamAgentOverlay>): string {
  return Array.from(map.values())
    .sort((left, right) => left.agentId.localeCompare(right.agentId))
    .map((entry) => [
      entry.runId,
      entry.agentId,
      entry.phase,
      entry.assignedAt,
      entry.startedAt ?? '',
      entry.completedAt ?? '',
      entry.blockUntilIdle ? 'blocked' : '',
    ].join(':'))
    .join('|');
}

const WORKED_DURATION_WRAP_SECONDS = 100 * 60 * 60;
const DAYBAR_SOURCE_WIDTH = 1200;
const DAYBAR_SOURCE_HEIGHT = 160;
const DAYBAR_WINDOW_WIDTH = 70;
const DAYBAR_WINDOW_HEIGHT = 30;
const DAYBAR_RENDERED_WIDTH = DAYBAR_SOURCE_WIDTH * (DAYBAR_WINDOW_HEIGHT / DAYBAR_SOURCE_HEIGHT);

function formatWorkedDurationParts(ms: number): { hours: string; minutes: string; seconds: string; label: string } {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000)) % WORKED_DURATION_WRAP_SECONDS;
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  const hourLabel = String(hours).padStart(2, '0');
  const minuteLabel = String(minutes).padStart(2, '0');
  const secondLabel = String(seconds).padStart(2, '0');
  return {
    hours: hourLabel,
    minutes: minuteLabel,
    seconds: secondLabel,
    label: `Worked for ${hourLabel} Hrs ${minuteLabel} Min ${secondLabel} Sec`,
  };
}

function daybarPositionX(nowMs: number): number {
  const now = new Date(nowMs);
  const secondsSinceMidnight = now.getHours() * 3600 + now.getMinutes() * 60 + now.getSeconds();
  const noonToNoonProgress = ((secondsSinceMidnight - 12 * 3600 + 24 * 3600) % (24 * 3600)) / (24 * 3600);
  return DAYBAR_WINDOW_WIDTH / 2 - noonToNoonProgress * DAYBAR_RENDERED_WIDTH;
}

function formatDreamElapsed(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function localDateInput(ms: number): string {
  const date = new Date(ms);
  return [
    date.getFullYear(),
    String(date.getMonth() + 1).padStart(2, '0'),
    String(date.getDate()).padStart(2, '0'),
  ].join('-');
}

function localTimeInput(ms: number): string {
  const date = new Date(ms);
  return [
    String(date.getHours()).padStart(2, '0'),
    String(date.getMinutes()).padStart(2, '0'),
  ].join(':');
}

function localDateTimeIso(dateValue: string, timeValue: string): string {
  const parsed = new Date(`${dateValue || localDateInput(Date.now())}T${timeValue || '09:00'}:00`);
  return Number.isFinite(parsed.getTime()) ? parsed.toISOString() : new Date().toISOString();
}

function resetEmberWorkSessionStartedAt(): number {
  emberWorkSessionStartedAt = Date.now();
  return emberWorkSessionStartedAt;
}

function scheduleDateFromIso(value: string | null | undefined): string {
  const parsed = value ? Date.parse(value) : NaN;
  return localDateInput(Number.isFinite(parsed) ? parsed : Date.now());
}

function scheduleTimeFromIso(value: string | null | undefined): string {
  const parsed = value ? Date.parse(value) : NaN;
  return localTimeInput(Number.isFinite(parsed) ? parsed : Date.now());
}

const REPEAT_WEEKDAY_SHORT = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'] as const;

function formatRepeatMinutes(totalMinutes: number): string {
  const total = Math.max(1, Math.round(totalMinutes));
  const days = Math.floor(total / 1440);
  const hours = Math.floor((total % 1440) / 60);
  const minutes = total % 60;
  const parts: string[] = [];
  if (days > 0) parts.push(`${days}d`);
  if (hours > 0) parts.push(`${hours}h`);
  if (minutes > 0 || parts.length === 0) parts.push(`${minutes}min`);
  return parts.join(' ');
}

function emberScheduleRepeatLabel(schedule: EmberSchedule): string | null {
  if (schedule.repeatEnabled) {
    const end = schedule.endMode === 'after'
      ? ` · ends after ${schedule.endAfterCount ?? 1}`
      : schedule.endMode === 'at' && schedule.endAt
        ? ` · ends ${emberTimeLabel(schedule.endAt)}`
        : '';
    const kind = schedule.repeatKind ?? 'fixed';
    if (kind === 'weekly') {
      const days = (schedule.repeatWeekDays ?? [])
        .filter((day) => day >= 0 && day <= 6)
        .map((day) => REPEAT_WEEKDAY_SHORT[day])
        .join('/') || 'Mon';
      const weeks = schedule.repeatEveryWeeks ?? 1;
      const cadence = weeks > 1 ? `every ${weeks} wks` : 'weekly';
      return `Repeats ${cadence} on ${days}${end}`;
    }
    if (kind === 'monthly') {
      const days = (schedule.repeatMonthDays ?? []).join(', ') || '1';
      const months = schedule.repeatEveryMonths ?? 1;
      const cadence = months > 1 ? `every ${months} months` : 'monthly';
      return `Repeats ${cadence} on day ${days}${end}`;
    }
    // fixed: prefer the instrument's total-minutes field; fall back to the
    // legacy amount/unit pair with the same arithmetic the editor hydration
    // uses, so pre-instrument schedules keep reading correctly.
    const totalMinutes = schedule.repeatEveryMinutes
      ?? ((schedule.repeatAmount ?? 1) * (schedule.repeatUnit === 'days' ? 1440 : schedule.repeatUnit === 'hours' ? 60 : 1));
    return `Repeats every ${formatRepeatMinutes(totalMinutes)}${end}`;
  }
  if (schedule.mode === 'daily' || schedule.mode === 'interval') return emberScheduleSummary(schedule);
  return null;
}

function delayPartsFromSchedule(schedule: EmberSchedule): { hours: number; minutes: number } {
  const amount = Math.max(1, Math.floor(Number(schedule.delayAmount ?? 10)));
  const unit = schedule.delayUnit ?? 'minutes';
  const totalMinutes = unit === 'days'
    ? amount * 24 * 60
    : unit === 'hours'
      ? amount * 60
      : amount;
  return {
    hours: Math.floor(totalMinutes / 60),
    minutes: totalMinutes % 60,
  };
}

function delayTotalMinutes(hours: number, minutes: number): number {
  const h = Math.max(0, Math.floor(Number(hours) || 0));
  const m = Math.max(0, Math.floor(Number(minutes) || 0));
  return Math.max(1, Math.min(999, h * 60 + m));
}

function emberCountdownLabel(schedule: EmberSchedule, nowMs: number): string {
  if (schedule.status === 'paused') return 'Paused';
  if (schedule.status === 'failed') return EMBER_NOT_DELIVERED;
  if (schedule.status === 'sent') return 'Sent';
  const target = Date.parse(schedule.nextRunAt);
  if (!Number.isFinite(target)) return emberTimeLabel(schedule.nextRunAt);
  const totalSeconds = Math.max(0, Math.ceil((target - nowMs) / 1000));
  if (totalSeconds === 0) return 'Due now';
  const days = Math.floor(totalSeconds / 86400);
  const hours = Math.floor((totalSeconds % 86400) / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function emberHistoryStatusLabel(record: EmberHistoryRecord): string {
  return record.status === 'failed' ? EMBER_NOT_DELIVERED : 'Delivered';
}

function emberHistoryKindLabel(record: EmberHistoryRecord): string {
  if (record.status === 'failed') return EMBER_NOT_DELIVERED;
  return record.triggeredBy === 'manual' ? 'Manually triggered' : 'Delivered';
}

function emberHistoryTargetLabel(record: EmberHistoryRecord): string {
  const names = record.targetAgentNames.length > 0 ? record.targetAgentNames : record.targetAgentIds;
  if (names.length === 0) return 'No target';
  if (names.length === 1) return `@${names[0]}`;
  return `@${names[0]} +${names.length - 1}`;
}

function RockerSwitch({
  checked,
  label,
  disabled = false,
  className = '',
  onClick,
}: {
  checked: boolean;
  label: string;
  disabled?: boolean;
  className?: string;
  onClick: () => void;
}) {
  const [coverOpen, setCoverOpen] = useState(false);
  const closeTimerRef = useRef<number | null>(null);

  const clearCloseTimer = useCallback(() => {
    if (closeTimerRef.current === null) return;
    window.clearTimeout(closeTimerRef.current);
    closeTimerRef.current = null;
  }, []);

  const scheduleCoverClose = useCallback(() => {
    clearCloseTimer();
    closeTimerRef.current = window.setTimeout(() => {
      closeTimerRef.current = null;
      setCoverOpen(false);
    }, ROCKER_COVER_CLOSE_MS);
  }, [clearCloseTimer]);

  useEffect(() => () => clearCloseTimer(), [clearCloseTimer]);

  useEffect(() => {
    if (!disabled) return;
    clearCloseTimer();
    setCoverOpen(false);
  }, [clearCloseTimer, disabled]);

  const handlePress = useCallback(() => {
    if (disabled) return;
    if (!coverOpen) {
      setCoverOpen(true);
      scheduleCoverClose();
      return;
    }
    clearCloseTimer();
    onClick();
    scheduleCoverClose();
  }, [clearCloseTimer, coverOpen, disabled, onClick, scheduleCoverClose]);

  return (
    <button
      type="button"
      className={[
        'kota-rocker-switch',
        checked ? 'on' : 'off',
        coverOpen ? 'cover-open' : '',
        className,
      ].filter(Boolean).join(' ')}
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={handlePress}
    >
      <span className="krs-guard-locator" aria-hidden />
      <span className="krs-switch-shell" aria-hidden>
        <span className="krs-guard-well">
          <span className="krs-guard-rocker">
            <span className="krs-guard-glow" />
            <span className="krs-guard-gloss" />
            <span className="krs-guard-bevel" />
            <span className="krs-guard-label">PWR<br />UP</span>
          </span>
          <span className="krs-cover">
            <span className="krs-cover-glass" />
            <span className="krs-cover-grip" />
          </span>
        </span>
      </span>
    </button>
  );
}

export function RightColumn({
  sceneKey,
  onOpenHotMem,
  workspace,
  workspaceProjects,
  workingAgents,
  roomAgents = EMPTY_AGENT_IDS,
  dreamProjects,
  resolveDreamProjects,
  agentMeta,
  projectRoot,
  onBartenderSynced,
  onOpenAgentFilteredChat,
  // onInsertComposerAttachment stays in props for App compatibility; the BBS
  // flows now use the embedded composer instead of the room composer.
  onPasteImage,
  onMaterializeAttachments,
  footerSlot,
  workingStartedAt,
  onDreamingStatusAgentsChange,
}: {
  sceneKey: SceneKey;
  onOpenHotMem: () => void;
  onOpenRow?: (row: LogRowType) => void;
  workspace?: WorkspaceProject | null;
  workspaceProjects?: readonly WorkspaceProject[];
  workingAgents?: ReadonlySet<AgentId>;
  roomAgents?: readonly AgentId[];
  dreamProjects?: readonly DreamProjectTarget[];
  resolveDreamProjects?: ResolveDreamProjectTargets;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  projectRoot?: string | null;
  onBartenderSynced?: () => void;
  onOpenAgentFilteredChat?: (agentId: AgentId) => void;
  onInsertComposerAttachment?: (attachment: ComposerAttachment) => void;
  onPasteImage?: (file: File) => Promise<string | null>;
  onMaterializeAttachments?: (attachments: readonly ComposerAttachment[]) => Promise<ComposerAttachment[]>;
  footerSlot?: ReactNode;
  workingStartedAt?: ReadonlyMap<AgentId, string>;
  onDreamingStatusAgentsChange?: (agentIds: readonly AgentId[]) => void;
}) {
  const scene = SCENES[sceneKey]!;
  const { hotMem } = scene;
  const [status, setStatus] = useState<BartenderStatus | null>(null);
  const [busyAction, setBusyAction] = useState<'sync' | 'pull' | 'push' | 'routePull' | null>(null);
  const busyActionRef = useRef<'sync' | 'pull' | 'push' | 'routePull' | null>(null);
  const busyActionProjectRootRef = useRef<string | null>(null);
  const [externalSyncActivities, setExternalSyncActivities] = useState<ReadonlyMap<string, BartenderExternalSyncActivity>>(
    () => new Map(),
  );
  const externalSyncActivitiesRef = useRef<ReadonlyMap<string, BartenderExternalSyncActivity>>(new Map());
  const externalSyncEventHandlerRef = useRef<(payload: BartenderSyncEvent) => void>(() => {});
  const syncProgressEventHandlerRef = useRef<(payload: BartenderSyncProgressEvent) => void>(() => {});
  const [syncProgress, setSyncProgress] = useState<BartenderSyncProgressEvent | null>(null);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [pullConflict, setPullConflict] = useState<BartenderPullConflict | null>(null);
  const [conflictBlocker, setConflictBlocker] = useState<BartenderConflictBlocker | null>(null);
  const [lastPromptAt, setLastPromptAt] = useState<string | null>(() => (
    lastVioletComposerSentAt(projectRoot ?? null)
  ));
  const lastSyncAtRef = useRef(0);
  const silentSyncRunningRef = useRef(false);
  const workingCount = workingAgents?.size ?? 0;
  const workingAgentsKey = useMemo(() => (
    Array.from(workingAgents ?? []).sort().join('|')
  ), [workingAgents]);
  const workingStartedAtKey = useMemo(() => (
    workingStartedAtMapKey(workingStartedAt)
  ), [workingStartedAt]);
  const bartenderProjectRoot = workspace?.localRoot ?? projectRoot ?? null;
  const bartenderProjectId = workspace?.projectId ?? null;
  const violetSummaryProjectRoot = workspace?.localRoot ?? projectRoot ?? null;
  const emberProjectRoot = workspace?.localRoot ?? projectRoot ?? null;
  const emberKey = emberProjectKey(workspace, projectRoot);
  const latestBartenderProjectRootRef = useRef<string | null>(bartenderProjectRoot);
  latestBartenderProjectRootRef.current = bartenderProjectRoot;
  const latestVioletSummaryProjectRootRef = useRef<string | null>(violetSummaryProjectRoot);
  const bartenderRefreshInFlightRef = useRef<string | null>(null);
  const violetSummaryRefreshInFlightRef = useRef<string | null>(null);
  const violetSummaryAutoRunInFlightRef = useRef<string | null>(null);
  const violetSummaryAutoAttemptKeyRef = useRef<string | null>(null);
  const violetSummaryRefreshSeqRef = useRef(0);
  const violetSummaryManualRunningRef = useRef(false);
  const emberInputRef = useRef<InputBarHandle | null>(null);
  const emberRunningRef = useRef<Set<string>>(new Set());
  const dreamRunningRef = useRef(false);
  const dreamConsolidatingRef = useRef(false);
  const dreamStartedAtRef = useRef(0);
  const [bartenderPromptVersion, setBartenderPromptVersion] = useState(0);
  const [bartenderPrompts, setBartenderPrompts] = useState<BartenderConflictPrompts>(() => (
    loadBartenderConflictPrompts()
  ));
  const violetSummaryConfig = useMemo(
    () => loadVioletSummaryConfig(),
    [bartenderPromptVersion],
  );
  const bartenderRequest = useMemo(
    () => ({ projectRoot: bartenderProjectRoot, ...bartenderPrompts }),
    [bartenderProjectRoot, bartenderPrompts],
  );
  const activeExternalSync = useMemo(() => (
    Array.from(externalSyncActivities.values())
      .find((activity) => samePathString(activity.projectRoot, bartenderProjectRoot)) ?? null
  ), [bartenderProjectRoot, externalSyncActivities]);
  const localBusyAction = busyAction === 'sync'
    ? (samePathString(busyActionProjectRootRef.current, bartenderProjectRoot) ? busyAction : null)
    : busyAction;
  const activeBusyAction = activeExternalSync ? 'sync' : localBusyAction;
  const activeSyncProgress = activeExternalSync?.progress
    ?? (localBusyAction === 'sync' ? syncProgress : null);
  const canUseBartender = !!workspace;
  const [autoSyncEnabled, setAutoSyncEnabled] = useState(() => (
    loadBartenderAutoSync(bartenderProjectId)
  ));
  const [bbs, setBbs] = useState<BbsSnapshot | null>(null);
  const [bbsOpen, setBbsOpen] = useState(false);
  const [bbsFilter, setBbsFilter] = useState<BbsFilter>('all');
  const [bbsBusy, setBbsBusy] = useState<string | null>(null);
  const [bbsError, setBbsError] = useState<string | null>(null);
  // Forum-style BBS: list ⇄ thread detail ⇄ compose views. Replies are
  // written by the human user (account identity) straight to BBS storage;
  // optionally selected agents get a bus delivery on top.
  const [bbsView, setBbsView] = useState<'list' | 'detail' | 'compose'>('list');
  const [bbsDetailThreadId, setBbsDetailThreadId] = useState<string | null>(null);
  const [bbsReplyText, setBbsReplyText] = useState('');
  const [bbsReplyAgentBarOpen, setBbsReplyAgentBarOpen] = useState(false);
  const [bbsReplyAgents, setBbsReplyAgents] = useState<AgentId[]>([]);
  const [bbsReplyBusy, setBbsReplyBusy] = useState(false);
  const [bbsComposeText, setBbsComposeText] = useState('');
  const [bbsComposeProjects, setBbsComposeProjects] = useState<Set<string>>(() => new Set());
  const [bbsUserIdentity, setBbsUserIdentity] = useState<AccountUserIdentity | null>(null);
  // Laughing Man (Telegram bridge)
  const [lm, setLm] = useState<LmStatus | null>(null);
  const [lmHistoryOpen, setLmHistoryOpen] = useState(false);
  const [lmSetupOpen, setLmSetupOpen] = useState(false);
  const [lmLog, setLmLog] = useState<LmLogEntry[]>([]);
  const [lmQueue, setLmQueue] = useState<LmStandbyQueueItem[]>([]);
  const [lmLogFilter, setLmLogFilter] = useState<'all' | 'project'>('all');
  const [lmQueueOnly, setLmQueueOnly] = useState(false);

  useEffect(() => {
    busyActionRef.current = busyAction;
  }, [busyAction]);
  const [lmQueueBusy, setLmQueueBusy] = useState<string | null>(null);
  const [lmQueueError, setLmQueueError] = useState<string | null>(null);
  const [lmRetryBusy, setLmRetryBusy] = useState(false);
  const [lmMuteBusy, setLmMuteBusy] = useState(false);

  const [bbsDeleteTarget, setBbsDeleteTarget] = useState<BbsPost | null>(null);
  const bbsDetailScrollRef = useRef<HTMLDivElement | null>(null);
  const pendingBbsScrollPostIdRef = useRef<string | null>(null);
  const pendingBbsScrollThreadIdRef = useRef<string | null>(null);
  const bbsReplyInputRef = useRef<InputBarHandle | null>(null);
  const bbsComposeInputRef = useRef<InputBarHandle | null>(null);
  const [emberState, setEmberState] = useState<EmberState>(() => loadEmberState(emberKey));
  const [emberModalOpen, setEmberModalOpen] = useState(false);
  const [emberTab, setEmberTab] = useState<EmberModalTab>('scheduled');
  const [emberEditorTarget, setEmberEditorTarget] = useState<EmberEditorTarget | null>(null);
  const [emberHistoryDetailId, setEmberHistoryDetailId] = useState<string | null>(null);
  const [emberText, setEmberText] = useState('');
  const [emberTargets, setEmberTargets] = useState<AgentId[]>([]);
  const [emberStep, setEmberStep] = useState<1 | 2>(1);
  const [emberSendMode, setEmberSendMode] = useState<EmberSendMode>('delay');
  const [emberDelayHours, setEmberDelayHours] = useState(0);
  const [emberDelayMinutes, setEmberDelayMinutes] = useState(10);
  const [emberAtDate, setEmberAtDate] = useState(() => localDateInput(Date.now()));
  const [emberAtTime, setEmberAtTime] = useState(() => localTimeInput(Date.now() + 60 * 60 * 1000));
  const [emberRepeatEnabled, setEmberRepeatEnabled] = useState(false);
  const [emberRepeatKind, setEmberRepeatKind] = useState<EmberRepeatKind>('fixed');
  const [emberRepDays, setEmberRepDays] = useState(1);
  const [emberRepHrs, setEmberRepHrs] = useState(0);
  const [emberRepMin, setEmberRepMin] = useState(0);
  const [emberWeekDays, setEmberWeekDays] = useState<number[]>([1]);
  const [emberEveryNWeeks, setEmberEveryNWeeks] = useState(1);
  const [emberMonthDays, setEmberMonthDays] = useState<string[]>(['1']);
  const [emberEveryNMonths, setEmberEveryNMonths] = useState(1);
  const [emberEndMode, setEmberEndMode] = useState<EmberEndMode>('never');
  const [emberEndAfterCount, setEmberEndAfterCount] = useState(3);
  const [emberEndDate, setEmberEndDate] = useState(() => localDateInput(Date.now() + 24 * 60 * 60 * 1000));
  const [emberEndTime, setEmberEndTime] = useState(() => localTimeInput(Date.now() + 24 * 60 * 60 * 1000));
  const [emberBusy, setEmberBusy] = useState<string | null>(null);
  const [emberMessage, setEmberMessage] = useState<string | null>(null);
  const [emberNow, setEmberNow] = useState(Date.now());
  const emberStateLoadedRef = useRef(false);
  const emberPersistedSignatureRef = useRef<string | null>(null);
  const emberSaveTimerRef = useRef<number | null>(null);
  const [workStartedAt, setWorkStartedAt] = useState(() => emberWorkSessionStartedAt);
  const [dreamOverlay, setDreamOverlay] = useState<DreamOverlayState>('off');
  const [dreamDueAt, setDreamDueAt] = useState<number | null>(null);
  const [dreamCountdownPaused, setDreamCountdownPaused] = useState(false);
  const [dreamPausedRemainingSeconds, setDreamPausedRemainingSeconds] = useState<number | null>(null);
  const [dreamVideoSetIndex, setDreamVideoSetIndex] = useState<number | null>(null);
  const [dreamingAgents, setDreamingAgents] = useState<string[]>([]);
  const [dreamScope, setDreamScope] = useState<string | null>(null);
  const [dreamConsolidationPending, setDreamConsolidationPending] = useState(false);
  const [dreamConsolidationProjectRoots, setDreamConsolidationProjectRoots] = useState<string[]>([]);
  const [dreamVigilReady, setDreamVigilReady] = useState(false);
  const [dreamAgentOverlay, setDreamAgentOverlay] = useState<ReadonlyMap<AgentId, DreamAgentOverlay>>(() => new Map());
  const [dreamDigestWaitActive, setDreamDigestWaitActive] = useState(false);
  const previousWorkingAgentsRef = useRef<ReadonlySet<AgentId>>(new Set());
  const clearDreamState = useCallback((options?: { resetWorkSession?: boolean }) => {
    setDreamOverlay('off');
    setDreamDueAt(null);
    setDreamCountdownPaused(false);
    setDreamPausedRemainingSeconds(null);
    setDreamVideoSetIndex(null);
    setDreamingAgents([]);
    setDreamScope(null);
    setDreamConsolidationPending(false);
    setDreamConsolidationProjectRoots([]);
    setDreamVigilReady(false);
    setDreamAgentOverlay(new Map());
    setDreamDigestWaitActive(false);
    dreamRunningRef.current = false;
    dreamConsolidatingRef.current = false;
    dreamStartedAtRef.current = 0;
    if (options?.resetWorkSession) setWorkStartedAt(resetEmberWorkSessionStartedAt());
  }, []);
  const [violetSummary, setVioletSummary] = useState<VioletSummaryState | null>(null);
  const [violetSummaryBusy, setVioletSummaryBusy] = useState(false);
  const [violetSummaryAutoBusy, setVioletSummaryAutoBusy] = useState(false);
  const [violetSummaryManualDeadlineAt, setVioletSummaryManualDeadlineAt] = useState<number | null>(null);
  const [violetSummaryManualNow, setVioletSummaryManualNow] = useState(Date.now());
  const [violetAutoCountdownNow, setVioletAutoCountdownNow] = useState(Date.now());
  const [violetSummaryError, setVioletSummaryError] = useState<string | null>(null);
  const [violetHistoryOpen, setVioletHistoryOpen] = useState(false);
  const setVioletSummaryIfChanged = useCallback((next: VioletSummaryState) => {
    setVioletSummary((current) => (
      violetSummaryViewKey(current) === violetSummaryViewKey(next) ? current : next
    ));
  }, []);
  const bbsProjectId = workspace?.projectId ?? null;
  const bbsRefreshInFlightRef = useRef<string | null>(null);
  const emberAgentOptions = useMemo(() => {
    const seen = new Set<AgentId>();
    const out: { id: AgentId; name: string }[] = [];
    for (const agentId of roomAgents) {
      if (!agentId || seen.has(agentId)) continue;
      seen.add(agentId);
      const agent = agentMeta?.[agentId];
      if (agent?.lifecycleStatus === 'archived' || agent?.lifecycleStatus === 'left') continue;
      out.push({ id: agentId, name: emberAgentName(agentId, agentMeta) });
    }
    return out;
  }, [agentMeta, roomAgents]);
  const emberTargetOptions = useMemo<EmberTargetOption[]>(() => {
    const out: EmberTargetOption[] = emberAgentOptions.map((agent) => ({
      ...agent,
      kind: 'agent',
    }));
    const humanName = bbsUserIdentity?.name.trim() || 'User';
    out.push({
      id: HUMAN_TELEGRAM_TARGET_ID,
      name: humanName,
      kind: 'human',
    });
    return out;
  }, [bbsUserIdentity, emberAgentOptions]);
  const emberTargetNameById = useMemo(() => (
    new Map(emberTargetOptions.map((target) => [target.id, target.name] as const))
  ), [emberTargetOptions]);
  const emberTargetNameFor = useCallback((targetId: AgentId) => (
    emberTargetNameById.get(targetId)
      ?? (isHumanTelegramTarget(targetId) ? (bbsUserIdentity?.name.trim() || 'User') : emberAgentName(targetId, agentMeta))
  ), [agentMeta, bbsUserIdentity, emberTargetNameById]);
  const currentDreamProject = useMemo<DreamProjectTarget | null>(() => {
    if (!emberProjectRoot || emberAgentOptions.length === 0) return null;
    return {
      projectId: workspace?.projectId ?? 'current',
      projectRoot: emberProjectRoot,
      projectName: workspace ? workspaceProjectDisplayName(workspace) : 'Current project',
      agents: emberAgentOptions,
    };
  }, [emberAgentOptions, emberProjectRoot, workspace]);
  const dreamTargetProjects = useMemo(() => {
    const source = dreamProjects != null
      ? dreamProjects
      : currentDreamProject
        ? [currentDreamProject]
        : [];
    return normalizeDreamProjectTargets(source, agentMeta);
  }, [agentMeta, currentDreamProject, dreamProjects]);
  const emberSchedules = useMemo(() => (
    sortedEmberSchedules(emberState.schedules)
  ), [emberState.schedules]);
  const emberDrafts = emberState.drafts;
  const emberHistory = useMemo(() => (
    [...emberState.history].sort((a, b) => b.sentAt.localeCompare(a.sentAt) || b.id.localeCompare(a.id))
  ), [emberState.history]);
  const dreamTargetNames = useMemo(() => (
    dreamTargetProjects.flatMap((project) => project.agents.map((agent) => agent.name))
  ), [dreamTargetProjects]);
  const pullConflictAgents = useMemo(() => {
    const seen = new Set<AgentId>();
    const ids: AgentId[] = [];
    const add = (id: string | null | undefined) => {
      if (!id || seen.has(id as AgentId)) return;
      seen.add(id as AgentId);
      ids.push(id as AgentId);
    };
    roomAgents.forEach(add);
    workspace?.agents.forEach((agent) => add(agent.agentId));
    return ids;
  }, [roomAgents, workspace]);

  useEffect(() => {
    latestBartenderProjectRootRef.current = bartenderProjectRoot;
    setStatus(null);
    setActionMessage(null);
    setPullConflict(null);
  }, [bartenderProjectRoot]);

  useEffect(() => {
    latestVioletSummaryProjectRootRef.current = violetSummaryProjectRoot;
    violetSummaryRefreshSeqRef.current += 1;
    violetSummaryRefreshInFlightRef.current = null;
    violetSummaryAutoRunInFlightRef.current = null;
    violetSummaryAutoAttemptKeyRef.current = null;
    violetSummaryManualRunningRef.current = false;
    setVioletSummary(null);
    setVioletSummaryBusy(false);
    setVioletSummaryAutoBusy(false);
    setVioletSummaryManualDeadlineAt(null);
    setVioletSummaryError(null);
    setVioletHistoryOpen(false);
  }, [violetSummaryProjectRoot]);

  useEffect(() => {
    emberStateLoadedRef.current = false;
    emberPersistedSignatureRef.current = null;
    const localState = loadEmberState(emberKey);
    setEmberState(localState);
    setEmberModalOpen(false);
    setEmberTab('scheduled');
    setEmberEditorTarget(null);
    setEmberHistoryDetailId(null);
    setEmberText('');
    setEmberMessage(null);
    setEmberBusy(null);
    clearDreamState();
    previousWorkingAgentsRef.current = new Set();
    emberRunningRef.current.clear();
  }, [clearDreamState, emberKey, emberProjectRoot]);

  useEffect(() => {
    if (!emberProjectRoot || !hasTauriRuntime()) {
      emberStateLoadedRef.current = true;
      emberPersistedSignatureRef.current = emberStateSignature(emberState);
      return;
    }
    let cancelled = false;
    const localState = loadEmberState(emberKey);
    void (async () => {
      const remote = await emberScheduleState({ projectRoot: emberProjectRoot });
      const shouldMigrate = !emberStateHasContent(remote) && emberStateHasContent(localState);
      const next = shouldMigrate
        ? await emberScheduleSave({ projectRoot: emberProjectRoot, state: localState as EmberScheduleState })
        : remote;
      if (cancelled) return;
      emberStateLoadedRef.current = true;
      emberPersistedSignatureRef.current = emberStateSignature(next);
      setEmberState(next);
    })().catch((err) => {
      if (cancelled) return;
      console.warn('[ember] load schedule state failed', err);
      emberStateLoadedRef.current = true;
      emberPersistedSignatureRef.current = emberStateSignature(localState);
      setEmberState(localState);
    });
    return () => {
      cancelled = true;
    };
  }, [emberKey, emberProjectRoot]);

  useEffect(() => {
    if (!emberProjectRoot || !hasTauriRuntime()) return undefined;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onEmberSchedulesChanged((payload) => {
      if (cancelled || !samePathString(payload.projectRoot, emberProjectRoot)) return;
      void emberScheduleState({ projectRoot: emberProjectRoot })
        .then((next) => {
          if (cancelled) return;
          const signature = emberStateSignature(next);
          emberPersistedSignatureRef.current = signature;
          setEmberState((current) => (
            emberStateSignature(current) === signature ? current : next
          ));
        })
        .catch((err) => console.warn('[ember] refresh schedule state failed', err));
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
  }, [emberProjectRoot]);

  useEffect(() => {
    if (!emberKey) return undefined;
    const signature = emberStateSignature(emberState);
    if (emberPersistedSignatureRef.current === signature) return undefined;
    if (!emberProjectRoot || !hasTauriRuntime()) {
      saveEmberState(emberKey, emberState);
      emberPersistedSignatureRef.current = signature;
      return undefined;
    }
    if (!emberStateLoadedRef.current) return undefined;
    if (emberSaveTimerRef.current != null) window.clearTimeout(emberSaveTimerRef.current);
    emberSaveTimerRef.current = window.setTimeout(() => {
      emberSaveTimerRef.current = null;
      void emberScheduleSave({ projectRoot: emberProjectRoot, state: emberState as EmberScheduleState })
        .then((saved) => {
          emberPersistedSignatureRef.current = emberStateSignature(saved);
        })
        .catch((err) => console.warn('[ember] save schedule state failed', err));
    }, 250);
    return () => {
      if (emberSaveTimerRef.current != null) {
        window.clearTimeout(emberSaveTimerRef.current);
        emberSaveTimerRef.current = null;
      }
    };
  }, [emberKey, emberProjectRoot, emberState]);

  useEffect(() => {
    if (emberTargetOptions.length === 0) {
      setEmberTargets([]);
      return;
    }
    setEmberTargets((current) => {
      // No default preselect: like BBS, the user picks targets explicitly.
      const valid = current.filter((agentId) => emberTargetOptions.some((target) => target.id === agentId));
      return valid.length === current.length ? current : valid;
    });
  }, [emberTargetOptions]);

  useEffect(() => {
    const tick = () => setEmberNow(Date.now());
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, []);

  const sendEmberBusMessage = useCallback(async (
    targetProjectRoot: string | null | undefined,
    targetAgentId: AgentId,
    text: string,
    intent: string,
    eventId: string,
  ): Promise<AgentBusSendResult> => {
    if (!targetProjectRoot) {
      throw new Error('Open a Kota project before scheduling Ember prompts.');
    }
    const result = await agentBusSend({
      projectRoot: targetProjectRoot,
      senderAgentId: EMBER_ACTOR_ID,
      senderName: EMBER_ACTOR_NAME,
      target: targetAgentId,
      intent,
      text,
      eventId,
      dedupeKey: eventId,
    });
    if (!result.submitted && !result.duplicate) {
      throw new Error(result.skippedReason || `Could not reach ${targetAgentId}`);
    }
    return result;
  }, []);

  const sendEmberHumanReminder = useCallback(async (schedule: EmberSchedule) => {
    const result = await emberDeliverHumanReminder({
      projectRoot: emberProjectRoot,
      eventId: emberEventId('reminder', schedule.id, HUMAN_TELEGRAM_TARGET_ID),
      text: schedule.text,
    });
    if (!result.delivered) {
      throw new Error(result.warnings.join('; ') || 'Could not deliver the reminder to the human target.');
    }
    return result;
  }, [emberProjectRoot]);

  const runEmberSchedule = useCallback(async (schedule: EmberSchedule, source: 'auto' | 'manual') => {
    if (emberRunningRef.current.has(schedule.id)) return;
    emberRunningRef.current.add(schedule.id);
    setEmberBusy(schedule.id);
    setEmberMessage(source === 'manual' ? 'Sending scheduled prompt.' : null);
    try {
      const targetAgentIds = emberScheduleTargetIds(schedule);
      const targetNames = emberScheduleTargetNames(schedule);
      const results = await Promise.allSettled(targetAgentIds.map(async (agentId, index) => {
        let warnings: string[] = [];
        if (isHumanTelegramTarget(agentId)) {
          const result = await sendEmberHumanReminder(schedule);
          warnings = result.warnings;
        } else {
          await sendEmberBusMessage(
            emberProjectRoot,
            agentId,
            renderEmberReminderPrompt(schedule),
            'reminder',
            emberEventId('reminder', schedule.id, agentId),
          );
        }
        return {
          id: agentId,
          name: targetNames[index] ?? emberTargetNameFor(agentId),
          warnings,
        };
      }));
      const delivered = results.flatMap((result) => (
        result.status === 'fulfilled' ? [result.value] : []
      ));
      const failures = results.flatMap((result, index) => {
        if (result.status === 'rejected') {
          return [`${targetNames[index] ?? (targetAgentIds[index] ? emberTargetNameFor(targetAgentIds[index]!) : 'target')}: ${String(result.reason)}`];
        }
        return result.value.warnings.map((warning) => `${result.value.name}: ${warning}`);
      });
      if (delivered.length === 0) {
        throw new Error(failures.join('; ') || 'Ember schedule could not reach any target.');
      }
      const partialError = failures.length > 0 ? `Some targets failed: ${failures.join('; ')}` : null;
      const now = Date.now();
      const sentAt = new Date(now).toISOString();
      setEmberState((current) => ({
        ...current,
        history: [
          createEmberHistoryRecord(
            schedule,
            'delivered',
            sentAt,
            partialError,
            source === 'manual' ? 'manual' : 'schedule',
          ),
          ...current.history,
        ].slice(0, 80),
        schedules: current.schedules.flatMap((candidate) => {
          if (candidate.id !== schedule.id) return [candidate];
          if (source === 'manual') return [];
          const next = rescheduleEmberSchedule(candidate, now);
          return next.status === 'sent' ? [] : [next];
        }),
      }));
      setEmberMessage(`Sent to ${delivered.map((result) => result.name).join(', ')}.${partialError ? ` ${partialError}` : ''}`);
    } catch (err) {
      console.warn('[ember] schedule delivery failed', err);
      const message = EMBER_NOT_DELIVERED;
      const sentAt = new Date().toISOString();
      setEmberState((current) => ({
        ...current,
        history: [
          createEmberHistoryRecord(
            schedule,
            'failed',
            sentAt,
            message,
            source === 'manual' ? 'manual' : 'schedule',
          ),
          ...current.history,
        ].slice(0, 80),
        schedules: current.schedules.map((candidate) => (
          candidate.id === schedule.id ? failedEmberSchedule(candidate, message) : candidate
        )),
      }));
      setEmberMessage(message);
    } finally {
      emberRunningRef.current.delete(schedule.id);
      setEmberBusy((current) => (current === schedule.id ? null : current));
    }
  }, [emberProjectRoot, emberTargetNameFor, sendEmberBusMessage, sendEmberHumanReminder]);

  const resetEmberEditor = useCallback(() => {
    const now = Date.now();
    setEmberStep(1);
    setEmberEditorTarget(null);
    setEmberHistoryDetailId(null);
    setEmberText('');
    setEmberSendMode('delay');
    setEmberTargets([]);
    setEmberDelayHours(0);
    setEmberDelayMinutes(10);
    setEmberAtDate(localDateInput(now));
    setEmberAtTime(localTimeInput(now + 60 * 60 * 1000));
    setEmberRepeatEnabled(false);
    setEmberRepeatKind('fixed');
    setEmberRepDays(1);
    setEmberRepHrs(0);
    setEmberRepMin(0);
    setEmberWeekDays([1]);
    setEmberEveryNWeeks(1);
    setEmberMonthDays(['1']);
    setEmberEveryNMonths(1);
    setEmberEndMode('never');
    setEmberEndAfterCount(3);
    setEmberEndDate(localDateInput(now + 24 * 60 * 60 * 1000));
    setEmberEndTime(localTimeInput(now + 24 * 60 * 60 * 1000));
  }, []);

  const openNewEmberEditor = useCallback((tab: EmberModalTab = emberTab) => {
    resetEmberEditor();
    setEmberTab(tab);
    setEmberEditorTarget({ kind: 'new' });
    setEmberHistoryDetailId(null);
    setEmberModalOpen(true);
    setEmberMessage(null);
  }, [emberTab, resetEmberEditor]);

  const populateEmberScheduleEditor = useCallback((schedule: EmberSchedule) => {
    setEmberStep(1);
    setEmberHistoryDetailId(null);
    setEmberText(schedule.text);
    setEmberTargets(emberScheduleTargetIds(schedule));
    setEmberSendMode(schedule.mode === 'idle' ? 'idle' : schedule.mode === 'at' ? 'at' : 'delay');
    const delay = delayPartsFromSchedule(schedule);
    setEmberDelayHours(delay.hours);
    setEmberDelayMinutes(delay.minutes);
    setEmberAtDate(scheduleDateFromIso(schedule.mode === 'at' ? schedule.atDateTime ?? schedule.nextRunAt : schedule.nextRunAt));
    setEmberAtTime(scheduleTimeFromIso(schedule.mode === 'at' ? schedule.atDateTime ?? schedule.nextRunAt : schedule.nextRunAt));
    setEmberRepeatEnabled(isRepeatingEmberSchedule(schedule));
    if (schedule.repeatEnabled) {
      setEmberRepeatKind(schedule.repeatKind ?? 'fixed');
      const totalMin = schedule.repeatEveryMinutes
        ?? ((schedule.repeatAmount ?? 1) * (schedule.repeatUnit === 'days' ? 1440 : schedule.repeatUnit === 'hours' ? 60 : 1));
      setEmberRepDays(Math.floor(totalMin / 1440));
      setEmberRepHrs(Math.floor((totalMin % 1440) / 60));
      setEmberRepMin(totalMin % 60);
      setEmberWeekDays(schedule.repeatWeekDays && schedule.repeatWeekDays.length > 0 ? schedule.repeatWeekDays : [1]);
      setEmberEveryNWeeks(schedule.repeatEveryWeeks ?? 1);
      setEmberMonthDays(schedule.repeatMonthDays && schedule.repeatMonthDays.length > 0 ? schedule.repeatMonthDays : ['1']);
      setEmberEveryNMonths(schedule.repeatEveryMonths ?? 1);
      setEmberEndMode(schedule.endMode ?? 'never');
      setEmberEndAfterCount(schedule.endAfterCount ?? 3);
      setEmberEndDate(scheduleDateFromIso(schedule.endAt));
      setEmberEndTime(scheduleTimeFromIso(schedule.endAt));
    } else {
      setEmberRepeatKind('fixed');
      setEmberRepDays(1);
      setEmberRepHrs(0);
      setEmberRepMin(0);
      setEmberWeekDays([1]);
      setEmberEveryNWeeks(1);
      setEmberMonthDays(['1']);
      setEmberEveryNMonths(1);
      setEmberEndMode('never');
    }
    setEmberEditorTarget({ kind: 'schedule', id: schedule.id });
    setEmberTab('scheduled');
    setEmberModalOpen(true);
    setEmberMessage(null);
    setEmberHistoryDetailId(null);
  }, []);

  const populateEmberDraftEditor = useCallback((draft: EmberDraft) => {
    resetEmberEditor();
    setEmberText(draft.text);
    setEmberEditorTarget({ kind: 'draft', id: draft.id });
    setEmberTab('drafts');
    setEmberModalOpen(true);
    setEmberMessage(null);
    setEmberHistoryDetailId(null);
  }, [resetEmberEditor]);

  const emberComposerPayload = useCallback(() => (
    (emberInputRef.current?.serialize().payload ?? emberText).trim()
  ), [emberText]);

  const emberScheduleInput = useCallback((text: string) => {
    const targetAgentIds = emberTargets;
    const targetAgentNames = targetAgentIds.map((agentId) => emberTargetNameFor(agentId));
    return {
      text,
      targetAgentId: targetAgentIds[0] ?? '',
      targetAgentName: targetAgentNames[0] ?? '',
      targetAgentIds,
      targetAgentNames,
      mode: emberSendMode,
      delayAmount: delayTotalMinutes(emberDelayHours, emberDelayMinutes),
      delayUnit: 'minutes' as EmberDelayUnit,
      atDateTime: localDateTimeIso(emberAtDate, emberAtTime),
      waitForIdle: emberSendMode === 'idle',
      repeatEnabled: emberRepeatEnabled,
      repeatKind: emberRepeatKind,
      repeatEveryMinutes: emberRepDays * 1440 + emberRepHrs * 60 + emberRepMin,
      repeatWeekDays: emberWeekDays,
      repeatEveryWeeks: emberEveryNWeeks,
      repeatMonthDays: emberMonthDays,
      repeatEveryMonths: emberEveryNMonths,
      endMode: emberEndMode,
      endAfterCount: emberEndAfterCount,
      endAt: emberEndMode === 'at' ? localDateTimeIso(emberEndDate, emberEndTime) : undefined,
    };
  }, [
    emberTargetNameFor,
    emberAtDate,
    emberAtTime,
    emberDelayHours,
    emberDelayMinutes,
    emberEndAfterCount,
    emberEndDate,
    emberEndMode,
    emberEndTime,
    emberRepeatEnabled,
    emberRepeatKind,
    emberRepDays,
    emberRepHrs,
    emberRepMin,
    emberWeekDays,
    emberEveryNWeeks,
    emberMonthDays,
    emberEveryNMonths,
    emberSendMode,
    emberTargets,
  ]);

  const saveEmberDraft = useCallback(() => {
    const text = emberComposerPayload();
    if (!text) return;
    const draft = createEmberDraft(text);
    setEmberState((current) => ({
      ...current,
      drafts: emberEditorTarget?.kind === 'draft' && emberEditorTarget.id
        ? current.drafts.map((candidate) => (
          candidate.id === emberEditorTarget.id
            ? { ...candidate, text, updatedAt: new Date().toISOString() }
            : candidate
        ))
        : [draft, ...current.drafts].slice(0, 20),
      schedules: emberEditorTarget?.kind === 'schedule' && emberEditorTarget.id
        ? current.schedules.filter((candidate) => candidate.id !== emberEditorTarget.id)
        : current.schedules,
    }));
    resetEmberEditor();
    setEmberTab('drafts');
    setEmberMessage('Saved draft.');
  }, [emberComposerPayload, emberEditorTarget, resetEmberEditor]);

  const createScheduleFromComposer = useCallback(() => {
    const text = emberComposerPayload();
    if (!text || emberTargets.length === 0) return;
    const schedule = createEmberSchedule(emberScheduleInput(text));
    setEmberState((current) => ({
      ...current,
      schedules: emberEditorTarget?.kind === 'schedule' && emberEditorTarget.id
        ? current.schedules.map((candidate) => (
          candidate.id === emberEditorTarget.id
            ? {
              ...schedule,
              id: candidate.id,
              createdAt: candidate.createdAt,
              createdBy: candidate.createdBy ?? schedule.createdBy,
              updatedBy: emberActorHuman(),
              runCount: candidate.runCount,
              lastRunAt: candidate.lastRunAt,
              status: candidate.status === 'paused' ? 'paused' : 'scheduled',
            }
            : candidate
        ))
        : [schedule, ...current.schedules],
      drafts: emberEditorTarget?.kind === 'draft' && emberEditorTarget.id
        ? current.drafts.filter((candidate) => candidate.id !== emberEditorTarget.id)
        : current.drafts,
    }));
    resetEmberEditor();
    setEmberTab('scheduled');
    setEmberMessage(`Scheduled for ${emberTimeLabel(schedule.nextRunAt)}.`);
  }, [
    emberComposerPayload,
    emberEditorTarget,
    emberScheduleInput,
    emberTargets,
    resetEmberEditor,
  ]);

  const deleteEmberDraft = useCallback((draft: EmberDraft) => {
    setEmberState((current) => ({
      ...current,
      drafts: current.drafts.filter((candidate) => candidate.id !== draft.id),
    }));
    if (emberEditorTarget?.kind === 'draft' && emberEditorTarget.id === draft.id) resetEmberEditor();
    setEmberTab('drafts');
    setEmberMessage('Deleted draft.');
  }, [emberEditorTarget, resetEmberEditor]);

  const deleteEmberSchedule = useCallback((schedule: EmberSchedule) => {
    setEmberState((current) => ({
      ...current,
      schedules: current.schedules.filter((candidate) => candidate.id !== schedule.id),
    }));
    if (emberEditorTarget?.kind === 'schedule' && emberEditorTarget.id === schedule.id) resetEmberEditor();
    setEmberTab('scheduled');
    setEmberMessage('Deleted scheduled prompt.');
  }, [emberEditorTarget, resetEmberEditor]);

  const deleteEmberHistoryRecord = useCallback((record: EmberHistoryRecord) => {
    setEmberState((current) => ({
      ...current,
      history: current.history.filter((candidate) => candidate.id !== record.id),
    }));
    if (emberHistoryDetailId === record.id) setEmberHistoryDetailId(null);
    setEmberTab('history');
    setEmberMessage('Deleted history record.');
  }, [emberHistoryDetailId]);

  const toggleEmberSchedule = useCallback((schedule: EmberSchedule) => {
    setEmberState((current) => ({
      ...current,
      schedules: current.schedules.map((candidate) => {
        if (candidate.id !== schedule.id) return candidate;
        if (candidate.status === 'paused') {
          return resumeEmberSchedule(candidate);
        }
        if (candidate.status === 'sent' && candidate.mode === 'delay') return candidate;
        return { ...candidate, status: 'paused', updatedAt: new Date().toISOString() };
      }),
    }));
  }, []);

  const runDreamNow = useCallback(async (options?: { skipCountdown?: boolean }) => {
    if (dreamRunningRef.current || dreamConsolidationPending || dreamConsolidatingRef.current) {
      setDreamOverlay('countdown');
      if (options?.skipCountdown || !dreamDueAt) {
        setDreamDueAt(Date.now());
        setDreamCountdownPaused(false);
        setDreamPausedRemainingSeconds(null);
      }
      setDreamDigestWaitActive(true);
      return;
    }
    dreamRunningRef.current = true;
    try {
      const resolvedProjects = resolveDreamProjects
        ? await resolveDreamProjects()
        : dreamTargetProjects;
      const projects = normalizeDreamProjectTargets(resolvedProjects, agentMeta);
      if (projects.length === 0) {
        setEmberMessage('No active agents to dream.');
        clearDreamState();
        return;
      }
      const dreamRunId = String(dreamDueAt ?? Date.now());
      setDreamingAgents([]);
      setDreamScope(null);
      setDreamDueAt(null);
      setDreamCountdownPaused(false);
      setDreamPausedRemainingSeconds(null);
      setDreamOverlay('dreaming');
      setDreamVigilReady(false);
      setDreamConsolidationProjectRoots([]);
      setDreamDigestWaitActive(false);
      dreamStartedAtRef.current = Date.now();

      const prepared = await Promise.allSettled(projects.map(async (project) => {
        const dreams = await emberPrepareDreams({ projectRoot: project.projectRoot });
        const projectAgentNames = project.agents.map((agent) => agent.name);
        const body = [
          EMBER_DREAM_TITLE,
          '',
          await renderDreamPromptFromFile(projectAgentNames, dreams.accountDreamsPath),
        ].join('\n');
        return { project, body };
      }));

      const sendTasks: Promise<{
        project: DreamProjectTarget;
        agent: DreamProjectTarget['agents'][number];
      }>[] = [];
      let failedCount = 0;
      for (const [index, result] of prepared.entries()) {
        if (result.status === 'rejected') {
          failedCount += projects[index]?.agents.length ?? 1;
          continue;
        }
        const { project, body } = result.value;
        for (const agent of project.agents) {
          sendTasks.push(sendEmberBusMessage(
            project.projectRoot,
            agent.id,
            body,
            'dream',
            emberEventId('dream', `${dreamRunId}-${project.projectId}`, agent.id),
          ).then(() => ({ project, agent })));
        }
      }

      const sendResults = await Promise.allSettled(sendTasks);
      const deliveredRoots = new Set<string>();
      const deliveredProjects = new Map<string, DreamProjectTarget>();
      let deliveredCount = 0;
      for (const result of sendResults) {
        if (result.status === 'fulfilled') {
          const { project, agent } = result.value;
          const root = project.projectRoot;
          const key = `${project.projectId}:${root}`;
          deliveredRoots.add(root);
          deliveredCount += 1;
          const existing = deliveredProjects.get(key);
          if (existing) {
            deliveredProjects.set(key, {
              ...existing,
              agents: [...existing.agents, agent],
            });
          } else {
            deliveredProjects.set(key, {
              ...project,
              agents: [agent],
            });
          }
        } else {
          failedCount += 1;
        }
      }

      if (deliveredCount === 0) {
        throw new Error(failedCount > 0 ? 'Dream prompt could not reach any agent.' : 'No active agents to dream.');
      }

      const deliveredProjectList = Array.from(deliveredProjects.values());
      const deliveredNames = deliveredProjectList.flatMap((project) => project.agents.map((agent) => agent.name));
      setDreamingAgents(deliveredNames);
      setDreamScope(dreamScopeLabel(deliveredProjectList));
      const assignedAt = Date.now();
      const currentProject = emberProjectRoot
        ? deliveredProjectList.find((project) => samePathString(project.projectRoot, emberProjectRoot)) ?? null
        : null;
      setDreamAgentOverlay(new Map((currentProject?.agents ?? []).map((agent) => [agent.id, {
        runId: dreamRunId,
        agentId: agent.id,
        phase: 'pending',
        assignedAt,
        blockUntilIdle: false,
      }])));
      const roots = Array.from(deliveredRoots);
      setDreamConsolidationProjectRoots(roots);
      setDreamConsolidationPending(true);
      const sentLabel = roots.length > 1
        ? `${deliveredCount} agents across ${roots.length} projects`
        : deliveredCount === 1
          ? (deliveredNames[0] ?? '1 agent')
          : `${deliveredCount} agents`;
      setEmberMessage(`Dream prompt sent to ${sentLabel}. Ember will consolidate shortly.${failedCount > 0 ? ` ${failedCount} failed.` : ''}`);
    } catch (err) {
      setEmberMessage(String(err));
      clearDreamState();
    }
  }, [agentMeta, clearDreamState, dreamConsolidationPending, dreamDueAt, dreamTargetProjects, emberProjectRoot, resolveDreamProjects, sendEmberBusMessage]);

  const finishDreamConsolidation = useCallback(async () => {
    if (dreamConsolidatingRef.current || !dreamConsolidationPending) return;
    const roots = dreamConsolidationProjectRoots.length > 0
      ? dreamConsolidationProjectRoots
      : emberProjectRoot
        ? [emberProjectRoot]
        : [];
    if (roots.length === 0) return;
    dreamConsolidatingRef.current = true;
    setEmberMessage(roots.length > 1
      ? `Ember is consolidating dreams across ${roots.length} projects.`
      : 'Ember is consolidating dreams.');
    try {
      const state = await emberConsolidateDreams({
        projectRoots: roots,
        provider: violetSummaryConfig.provider,
      });
      const message = state.error
        ? 'Dream consolidation finished with 1 error.'
        : state.processedEntryCount > 0
          ? `Dreams consolidated: ${state.activeEntryCount} active, ${state.archivedEntryCount} archived.`
          : 'No new dream entries found.';
      setEmberMessage(message);
      setDreamVigilReady(true);
    } catch (err) {
      setEmberMessage(`Dream consolidation failed: ${String(err)}`);
      setDreamVigilReady(true);
    } finally {
      setDreamConsolidationPending(false);
      setDreamConsolidationProjectRoots([]);
      dreamRunningRef.current = false;
      dreamConsolidatingRef.current = false;
      const now = Date.now();
      setDreamAgentOverlay((current) => {
        if (current.size === 0) return current;
        const next = new Map(current);
        let changed = false;
        for (const [agentId, entry] of current) {
          const stalePending = entry.phase === 'pending' && now - entry.assignedAt > DREAM_AGENT_OVERLAY_TTL_MS;
          if (entry.phase === 'completed' || stalePending) {
            next.delete(agentId);
            changed = true;
          }
        }
        return changed ? next : current;
      });
    }
  }, [dreamConsolidationPending, dreamConsolidationProjectRoots, emberProjectRoot, violetSummaryConfig.provider]);

  useEffect(() => {
    if (!dreamConsolidationPending) return;
    const elapsed = Date.now() - dreamStartedAtRef.current;
    if (elapsed < DREAM_MIN_CONSOLIDATE_DELAY_MS) return;
    void finishDreamConsolidation();
  }, [
    dreamConsolidationPending,
    emberNow,
    finishDreamConsolidation,
  ]);

  useEffect(() => {
    if (!dreamDigestWaitActive || dreamOverlay !== 'countdown') return;
    if (dreamRunningRef.current || dreamConsolidationPending || dreamConsolidatingRef.current) return;
    setDreamDigestWaitActive(false);
  }, [dreamConsolidationPending, dreamDigestWaitActive, dreamOverlay]);

  useEffect(() => {
    const previousWorkingAgents = previousWorkingAgentsRef.current;
    const currentWorkingAgents = workingAgents ?? new Set<AgentId>();
    const now = Date.now();
    setDreamAgentOverlay((current) => {
      if (current.size === 0) return current;
      const next = new Map(current);
      let changed = false;
      for (const [agentId, entry] of current) {
        const isWorking = currentWorkingAgents.has(agentId);
        const wasWorking = previousWorkingAgents.has(agentId);
        const startedAtMs = Date.parse(workingStartedAt?.get(agentId) ?? '');
        const startedAfterAssignment = Number.isFinite(startedAtMs)
          && startedAtMs >= entry.assignedAt - 1000;

        if (entry.phase === 'pending') {
          if (entry.blockUntilIdle && !isWorking) {
            next.set(agentId, { ...entry, blockUntilIdle: false });
            changed = true;
            continue;
          }
          if (!entry.blockUntilIdle && isWorking && (!wasWorking || startedAfterAssignment)) {
            next.set(agentId, {
              ...entry,
              phase: 'dreaming',
              startedAt: Number.isFinite(startedAtMs) ? startedAtMs : now,
            });
            changed = true;
            continue;
          }
          if (now - entry.assignedAt > DREAM_AGENT_OVERLAY_TTL_MS) {
            next.delete(agentId);
            changed = true;
          }
          continue;
        }

        const startedAnotherTurn = entry.phase === 'dreaming'
          && Number.isFinite(startedAtMs)
          && entry.startedAt != null
          && startedAtMs > entry.startedAt + 1000;
        if (entry.phase === 'dreaming' && (!isWorking || startedAnotherTurn)) {
          next.set(agentId, {
            ...entry,
            phase: 'completed',
            completedAt: now,
          });
          changed = true;
        }
      }
      return changed ? next : current;
    });
    previousWorkingAgentsRef.current = new Set(currentWorkingAgents);
  }, [emberNow, workingAgents, workingAgentsKey, workingStartedAt, workingStartedAtKey]);

  const dreamAgentOverlayKey = useMemo(() => (
    dreamOverlayMapKey(dreamAgentOverlay)
  ), [dreamAgentOverlay]);
  const dreamingStatusAgentIds = useMemo(() => (
    Array.from(dreamAgentOverlay.values())
      .filter((entry) => entry.phase === 'dreaming' && (workingAgents?.has(entry.agentId) ?? false))
      .map((entry) => entry.agentId)
      .sort()
  ), [dreamAgentOverlay, dreamAgentOverlayKey, workingAgents, workingAgentsKey]);
  const dreamingStatusAgentIdsKey = dreamingStatusAgentIds.join('|');

  useEffect(() => {
    onDreamingStatusAgentsChange?.(dreamingStatusAgentIds);
  }, [dreamingStatusAgentIds, dreamingStatusAgentIdsKey, onDreamingStatusAgentsChange]);

  useEffect(() => () => {
    onDreamingStatusAgentsChange?.([]);
  }, [onDreamingStatusAgentsChange]);

  useEffect(() => {
    if (dreamOverlay !== 'countdown' || !dreamDueAt) return;
    if (dreamCountdownPaused) return;
    if (emberNow < dreamDueAt) return;
    if (dreamRunningRef.current || dreamConsolidationPending || dreamConsolidatingRef.current) {
      if (!dreamDigestWaitActive) setDreamDigestWaitActive(true);
      return;
    }
    void runDreamNow();
  }, [dreamConsolidationPending, dreamCountdownPaused, dreamDigestWaitActive, dreamDueAt, dreamOverlay, emberNow, runDreamNow]);

  // While agents are still working we hold on the "wrapping up" scene; once the room
  // goes idle, begin the normal 10-minute countdown automatically.
  useEffect(() => {
    if (dreamOverlay !== 'wrapping' || workingCount > 0) return;
    setDreamDueAt(Date.now() + 10 * 60 * 1000);
    setDreamCountdownPaused(false);
    setDreamPausedRemainingSeconds(null);
    setDreamOverlay('countdown');
  }, [dreamOverlay, workingCount]);

  const startDreamCountdown = useCallback(() => {
    if (dreamOverlay !== 'off') return;
    setDreamVideoSetIndex(randomDreamVideoSetIndex());
    setDreamingAgents(dreamTargetNames);
    setDreamScope(dreamScopeLabel(dreamTargetProjects));
    setDreamVigilReady(false);
    setDreamDigestWaitActive(false);
    setEmberMessage(null);
    setDreamCountdownPaused(false);
    setDreamPausedRemainingSeconds(null);
    // Entering night mode while agents are working shows the working-desk scene first.
    if (workingCount > 0) {
      setDreamDueAt(null);
      setDreamOverlay('wrapping');
      return;
    }
    setDreamDueAt(Date.now() + 10 * 60 * 1000);
    setDreamOverlay('countdown');
  }, [dreamOverlay, dreamTargetNames, dreamTargetProjects, workingCount]);

  const pauseDreamCountdown = useCallback(() => {
    if (dreamOverlay !== 'countdown' || dreamCountdownPaused || !dreamDueAt) return;
    setDreamPausedRemainingSeconds(Math.max(0, Math.ceil((dreamDueAt - Date.now()) / 1000)));
    setDreamDueAt(null);
    setDreamCountdownPaused(true);
  }, [dreamCountdownPaused, dreamDueAt, dreamOverlay]);

  const resumeDreamCountdown = useCallback(() => {
    if (dreamOverlay !== 'countdown' || !dreamCountdownPaused) return;
    const remainingSeconds = Math.max(0, dreamPausedRemainingSeconds ?? 0);
    setDreamDueAt(Date.now() + remainingSeconds * 1000);
    setDreamPausedRemainingSeconds(null);
    setDreamCountdownPaused(false);
  }, [dreamCountdownPaused, dreamOverlay, dreamPausedRemainingSeconds]);

  const toggleDreamCountdownPaused = useCallback(() => {
    if (dreamCountdownPaused) {
      resumeDreamCountdown();
      return;
    }
    pauseDreamCountdown();
  }, [dreamCountdownPaused, pauseDreamCountdown, resumeDreamCountdown]);

  const cancelDreamCountdown = useCallback(() => {
    const promptAlreadySent = dreamOverlay === 'dreaming'
      || dreamOverlay === 'finished'
      || dreamConsolidationPending;
    if (promptAlreadySent) {
      setDreamOverlay('off');
      setDreamDueAt(null);
      setDreamCountdownPaused(false);
      setDreamPausedRemainingSeconds(null);
      setDreamVideoSetIndex(null);
      setDreamingAgents([]);
      setDreamScope(null);
      setDreamVigilReady(false);
      setDreamAgentOverlay(new Map());
      setDreamDigestWaitActive(false);
      setWorkStartedAt(resetEmberWorkSessionStartedAt());
      return;
    }
    clearDreamState({ resetWorkSession: true });
  }, [clearDreamState, dreamConsolidationPending, dreamOverlay]);

  useEffect(() => {
    const refresh = () => setBartenderPromptVersion((version) => version + 1);
    window.addEventListener(TAVERN_SYSTEM_CONFIG_CHANGED_EVENT, refresh);
    window.addEventListener('storage', refresh);
    return () => {
      window.removeEventListener(TAVERN_SYSTEM_CONFIG_CHANGED_EVENT, refresh);
      window.removeEventListener('storage', refresh);
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    void loadBartenderConflictPromptsFromFiles()
      .then((prompts) => {
        if (!cancelled) setBartenderPrompts(prompts);
      })
      .catch(() => {
        if (!cancelled) setBartenderPrompts(loadBartenderConflictPrompts());
      });
    return () => {
      cancelled = true;
    };
  }, [bartenderPromptVersion]);

  useEffect(() => {
    setAutoSyncEnabled(loadBartenderAutoSync(bartenderProjectId));
  }, [bartenderProjectId]);

  const refreshVioletSummary = useCallback(async () => {
    if (!violetSummaryProjectRoot) {
      setVioletSummary(null);
      setVioletSummaryError(null);
      return;
    }
    if (violetSummaryManualRunningRef.current || violetSummaryAutoRunInFlightRef.current) return;
    const requestProjectRoot = violetSummaryProjectRoot;
    const requestKey = `${requestProjectRoot}:read`;
    if (violetSummaryRefreshInFlightRef.current) return;
    const requestSeq = ++violetSummaryRefreshSeqRef.current;
    violetSummaryRefreshInFlightRef.current = requestKey;
    try {
      const next = await readVioletSummary({
        projectRoot: requestProjectRoot,
        config: violetSummaryConfig,
      });
      if (requestSeq !== violetSummaryRefreshSeqRef.current) return;
      if (!sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) return;
      setVioletSummaryIfChanged(next);
      setVioletSummaryError(next.error ?? null);
    } catch (err) {
      if (requestSeq !== violetSummaryRefreshSeqRef.current) return;
      if (!sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) return;
      setVioletSummaryError(String(err));
    } finally {
      if (
        requestSeq === violetSummaryRefreshSeqRef.current
        && violetSummaryRefreshInFlightRef.current === requestKey
      ) {
        violetSummaryRefreshInFlightRef.current = null;
      }
    }
  }, [setVioletSummaryIfChanged, violetSummaryConfig, violetSummaryProjectRoot]);

  const runVioletSummaryAuto = useCallback(async () => {
    if (!violetSummaryProjectRoot || violetSummaryManualRunningRef.current) return;
    const requestProjectRoot = violetSummaryProjectRoot;
    if (violetSummaryAutoRunInFlightRef.current) return;
    violetSummaryAutoRunInFlightRef.current = requestProjectRoot;
    setVioletSummaryAutoBusy(true);
    try {
      const next = await summarizeVioletAuto({
        projectRoot: requestProjectRoot,
        config: violetSummaryConfig,
      });
      if (!sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) return;
      setVioletSummaryIfChanged(next);
      setVioletSummaryError(next.error ?? null);
    } catch (err) {
      if (!sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) return;
      setVioletSummaryError(String(err));
    } finally {
      if (sameProjectRoot(violetSummaryAutoRunInFlightRef.current, requestProjectRoot)) {
        violetSummaryAutoRunInFlightRef.current = null;
      }
      if (sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) {
        setVioletSummaryAutoBusy(false);
      }
    }
  }, [setVioletSummaryIfChanged, violetSummaryConfig, violetSummaryProjectRoot]);

  useEffect(() => {
    void refreshVioletSummary();
    if (!violetSummaryProjectRoot) return;
    const timer = window.setInterval(() => {
      void refreshVioletSummary();
    }, 60000);
    return () => window.clearInterval(timer);
  }, [refreshVioletSummary, violetSummaryProjectRoot]);

  useEffect(() => {
    if (!violetSummaryProjectRoot || !violetSummary) return;
    if (!violetSummaryIsAutoDue(violetSummary, violetSummaryConfig, violetAutoCountdownNow)) return;
    const attemptKey = violetSummaryAutoAttemptKey(
      violetSummaryProjectRoot,
      violetSummary,
      violetSummaryConfig,
    );
    if (!attemptKey || violetSummaryAutoAttemptKeyRef.current === attemptKey) return;
    violetSummaryAutoAttemptKeyRef.current = attemptKey;
    void runVioletSummaryAuto();
  }, [
    runVioletSummaryAuto,
    violetAutoCountdownNow,
    violetSummary,
    violetSummaryConfig,
    violetSummaryProjectRoot,
  ]);

  const runVioletSummaryNow = useCallback(async () => {
    if (!violetSummaryProjectRoot || violetSummaryBusy || violetSummaryAutoBusy) return;
    const requestProjectRoot = violetSummaryProjectRoot;
    const requestSeq = ++violetSummaryRefreshSeqRef.current;
    violetSummaryManualRunningRef.current = true;
    const deadlineAt = Date.now() + VIOLET_SUMMARY_CLI_TIMEOUT_SECS * 1000;
    setVioletSummaryBusy(true);
    setVioletSummaryManualNow(Date.now());
    setVioletSummaryManualDeadlineAt(deadlineAt);
    setVioletSummaryError(null);
    try {
      const next = await summarizeVioletNow({
        projectRoot: requestProjectRoot,
        config: violetSummaryConfig,
        autoRun: false,
      });
      if (requestSeq !== violetSummaryRefreshSeqRef.current) return;
      if (!sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) return;
      setVioletSummaryIfChanged(next);
      setVioletSummaryError(next.error ?? null);
    } catch (err) {
      if (requestSeq !== violetSummaryRefreshSeqRef.current) return;
      if (!sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) return;
      setVioletSummaryError(String(err));
    } finally {
      violetSummaryManualRunningRef.current = false;
      if (sameProjectRoot(latestVioletSummaryProjectRootRef.current, requestProjectRoot)) {
        setVioletSummaryBusy(false);
        setVioletSummaryManualDeadlineAt(null);
      }
    }
  }, [setVioletSummaryIfChanged, violetSummaryAutoBusy, violetSummaryBusy, violetSummaryConfig, violetSummaryProjectRoot]);

  useEffect(() => {
    if (!violetSummaryBusy || !violetSummaryManualDeadlineAt) return undefined;
    const tick = () => setVioletSummaryManualNow(Date.now());
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [violetSummaryBusy, violetSummaryManualDeadlineAt]);

  const refreshBbs = useCallback(async (force = false) => {
    if (!bbsProjectId) {
      setBbs(null);
      setBbsError(null);
      return;
    }
    if (bbsRefreshInFlightRef.current && !force) return;
    const requestKey = `${bbsProjectId}:${Date.now()}:${Math.random()}`;
    bbsRefreshInFlightRef.current = requestKey;
    try {
      const next = await bbsSnapshot({
        projectId: bbsProjectId,
        projectDisplayName: workspace ? workspaceProjectDisplayName(workspace) : null,
      });
      if (bbsRefreshInFlightRef.current !== requestKey) return;
      setBbs(next);
      setBbsError(null);
    } catch (err) {
      if (bbsRefreshInFlightRef.current !== requestKey) return;
      setBbsError(String(err));
    } finally {
      if (bbsRefreshInFlightRef.current === requestKey) {
        bbsRefreshInFlightRef.current = null;
      }
    }
  }, [bbsProjectId, workspace]);

  useEffect(() => {
    setBbs(null);
    setBbsError(null);
    setBbsFilter('all');
    setBbsView('list');
    setBbsDetailThreadId(null);
    setBbsReplyText('');
    setBbsReplyAgentBarOpen(false);
    setBbsReplyAgents([]);
    setBbsReplyBusy(false);
    setBbsComposeText('');
    setBbsComposeProjects(new Set());
    setBbsDeleteTarget(null);
    void refreshBbs();
    if (!bbsProjectId) return;
    const timer = window.setInterval(() => {
      void refreshBbs();
    }, 12000);
    return () => window.clearInterval(timer);
  }, [bbsProjectId, refreshBbs]);

  const toggleAutoSync = useCallback(() => {
    if (!bartenderProjectId) return;
    setAutoSyncEnabled((current) => {
      const next = !current;
      saveBartenderAutoSync(bartenderProjectId, next);
      return next;
    });
  }, [bartenderProjectId]);

  const updateExternalSyncActivity = useCallback((
    projectRoot: string,
    update: (current: BartenderExternalSyncActivity | null) => BartenderExternalSyncActivity | null,
  ) => {
    const key = bartenderPathKey(projectRoot);
    if (!key) return;
    const currentActivities = externalSyncActivitiesRef.current;
    const current = currentActivities.get(key) ?? null;
    const nextActivity = update(current);
    if (nextActivity === current) return;
    const nextActivities = new Map(currentActivities);
    if (nextActivity) nextActivities.set(key, nextActivity);
    else nextActivities.delete(key);
    externalSyncActivitiesRef.current = nextActivities;
    setExternalSyncActivities(nextActivities);
  }, []);

  const startLocalSync = useCallback((projectRoot: string | null) => {
    busyActionProjectRootRef.current = projectRoot;
    busyActionRef.current = 'sync';
    setBusyAction('sync');
  }, []);

  const finishLocalSync = useCallback(() => {
    busyActionProjectRootRef.current = null;
    busyActionRef.current = null;
    setBusyAction(null);
  }, []);

  const refreshBartender = useCallback(async () => {
    if (!canUseBartender) {
      setStatus(null);
      return;
    }
    const requestProjectRoot = bartenderProjectRoot;
    const requestKey = requestProjectRoot ?? '__active__';
    if (bartenderRefreshInFlightRef.current === requestKey) return;
    bartenderRefreshInFlightRef.current = requestKey;
    try {
      const next = await bartenderStatus(bartenderRequest);
      if (latestBartenderProjectRootRef.current !== requestProjectRoot) return;
      setStatus(next);
      setActionMessage((prev) => {
        if (!prev) return prev;
        if (next.state === 'idle') return null;
        if (prev.startsWith('Could not refresh') && next.roomChangeCount === 0) return null;
        return prev;
      });
    } catch (err) {
      if (latestBartenderProjectRootRef.current !== requestProjectRoot) return;
      setActionMessage(String(err));
    } finally {
      if (bartenderRefreshInFlightRef.current === requestKey) {
        bartenderRefreshInFlightRef.current = null;
      }
    }
  }, [bartenderProjectRoot, bartenderRequest, canUseBartender]);

  const applyExternalBartenderSyncEvent = useCallback((payload: BartenderSyncEvent) => {
    if (payload.phase === 'started') {
      updateExternalSyncActivity(payload.projectRoot, (current) => {
        if (current?.requestId === payload.requestId) return current;
        return {
          projectRoot: payload.projectRoot,
          requestId: payload.requestId,
          startedAt: Date.now(),
          progress: {
            projectRoot: payload.projectRoot,
            phase: 'starting',
            message: 'Starting local sync',
            elapsedMs: 0,
          },
        };
      });
      if (samePathString(payload.projectRoot, latestBartenderProjectRootRef.current)) {
        setActionMessage(null);
      }
      return;
    }

    const activity = externalSyncActivitiesRef.current.get(bartenderPathKey(payload.projectRoot));
    if (activity?.requestId !== payload.requestId) return;
    updateExternalSyncActivity(payload.projectRoot, () => null);

    if (!samePathString(payload.projectRoot, latestBartenderProjectRootRef.current)) return;
    lastSyncAtRef.current = Date.now();
    if (payload.phase === 'finished' && payload.result) {
      setStatus(payload.result.status);
      setActionMessage(payload.result.message);
      const blocker = conflictBlockerFromConflicts(payload.result.conflicts, agentMeta, workingAgents);
      if (blocker) {
        setConflictBlocker(blocker);
        setPullConflict(null);
      } else if (payload.result.ok) {
        setConflictBlocker(null);
        setPullConflict(null);
      }
      onBartenderSynced?.();
    } else if (payload.phase === 'failed') {
      setActionMessage(payload.error || 'Bartender sync failed.');
      void refreshBartender();
    }
  }, [agentMeta, onBartenderSynced, refreshBartender, updateExternalSyncActivity, workingAgents]);
  externalSyncEventHandlerRef.current = applyExternalBartenderSyncEvent;

  const applyBartenderSyncProgress = useCallback((payload: BartenderSyncProgressEvent) => {
    const externalActivity = externalSyncActivitiesRef.current.get(bartenderPathKey(payload.projectRoot));
    if (externalActivity) {
      updateExternalSyncActivity(payload.projectRoot, (current) => (
        current ? { ...current, progress: payload } : current
      ));
      return;
    }
    if (
      busyActionRef.current === 'sync'
      && samePathString(payload.projectRoot, busyActionProjectRootRef.current)
    ) {
      setSyncProgress(payload);
    }
  }, [updateExternalSyncActivity]);
  syncProgressEventHandlerRef.current = applyBartenderSyncProgress;

  useEffect(() => {
    void refreshBartender();
    if (!canUseBartender) return;
    const timer = window.setInterval(() => {
      void refreshBartender();
    }, 12000);
    return () => window.clearInterval(timer);
  }, [canUseBartender, refreshBartender]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onBartenderSyncEvent((payload) => {
      if (cancelled) return;
      externalSyncEventHandlerRef.current(payload);
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
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onBartenderSyncProgressEvent((payload) => {
      if (cancelled) return;
      syncProgressEventHandlerRef.current(payload);
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
  }, []);

  const externalSyncReceiptKey = useMemo(() => (
    Array.from(externalSyncActivities.values())
      .map((activity) => `${bartenderPathKey(activity.projectRoot)}\u0000${activity.requestId}`)
      .sort()
      .join('\u0001')
  ), [externalSyncActivities]);

  useEffect(() => {
    if (!externalSyncReceiptKey) return undefined;
    let cancelled = false;
    let running = false;
    const reconcile = async () => {
      if (running) return;
      running = true;
      try {
        const activities = Array.from(externalSyncActivitiesRef.current.values());
        for (const activity of activities) {
          if (Date.now() - activity.startedAt >= BARTENDER_SYNC_RECEIPT_TIMEOUT_MS) {
            externalSyncEventHandlerRef.current({
              projectRoot: activity.projectRoot,
              requestId: activity.requestId,
              phase: 'failed',
              error: 'Bartender sync status is unknown. Stopped waiting.',
            });
            continue;
          }
          try {
            const receipt = await bartenderSyncReceipt({
              projectRoot: activity.projectRoot,
              requestId: activity.requestId,
            });
            if (cancelled) return;
            if (receipt.phase === 'finished' && receipt.result) {
              externalSyncEventHandlerRef.current({
                projectRoot: receipt.projectRoot,
                requestId: receipt.requestId,
                phase: 'finished',
                result: receipt.result,
              });
            } else if (receipt.phase === 'failed') {
              externalSyncEventHandlerRef.current({
                projectRoot: receipt.projectRoot,
                requestId: receipt.requestId,
                phase: 'failed',
                error: receipt.error,
              });
            }
          } catch {
            // The lifecycle event remains the fast path; retry durable receipts quietly.
          }
        }
      } finally {
        running = false;
      }
    };
    void reconcile();
    const timer = window.setInterval(() => void reconcile(), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [externalSyncReceiptKey]);

  useEffect(() => {
    if (!canUseBartender) return;
    const timer = window.setInterval(() => {
      if (activeBusyAction !== null) return;
      void bartenderFetch(bartenderRequest)
        .then((result) => {
          if (latestBartenderProjectRootRef.current !== bartenderProjectRoot) return;
          setStatus(result.status);
        })
        .catch(() => {
          // Fetch is only for upstream freshness; explicit actions surface errors.
        });
    }, 60000);
    return () => window.clearInterval(timer);
  }, [activeBusyAction, bartenderProjectRoot, bartenderRequest, canUseBartender]);

  useEffect(() => {
    const update = () => setLastPromptAt(lastVioletComposerSentAt(projectRoot ?? null));
    update();
    window.addEventListener(VIOLET_COMPOSER_SENT_EVENT, update);
    return () => window.removeEventListener(VIOLET_COMPOSER_SENT_EVENT, update);
  }, [projectRoot]);

  useEffect(() => {
    setConflictBlocker((current) => {
      if (!current || !workingAgents) return current;
      const isWorking = workingAgents.has(current.agentId);
      if (isWorking && !current.observedWorking) {
        return { ...current, observedWorking: true };
      }
      if (!isWorking && current.observedWorking) {
        return null;
      }
      return current;
    });
  }, [workingAgents, workingAgentsKey]);

  const runSync = useCallback(async (mode: 'manual' | 'silent') => {
    if (
      !canUseBartender
      || busyAction
      || externalSyncActivitiesRef.current.size > 0
      || silentSyncRunningRef.current
      || conflictBlocker
    ) return;
    const requestProjectRoot = bartenderProjectRoot;
    if (mode === 'silent') silentSyncRunningRef.current = true;
    setSyncProgress({
      projectRoot: bartenderProjectRoot ?? '',
      phase: 'starting',
      message: 'Starting local sync',
      elapsedMs: 0,
    });
    startLocalSync(requestProjectRoot);
    setActionMessage(mode === 'silent' ? 'Pouring a quiet sync.' : null);
    try {
      const result = await bartenderSyncLocal(bartenderRequest);
      lastSyncAtRef.current = Date.now();
      if (samePathString(requestProjectRoot, latestBartenderProjectRootRef.current)) {
        setStatus(result.status);
        setActionMessage(result.message);
        const blocker = conflictBlockerFromConflicts(result.conflicts, agentMeta, workingAgents);
        if (blocker) {
          setConflictBlocker(blocker);
          setPullConflict(null);
        } else if (result.ok) {
          setConflictBlocker(null);
          setPullConflict(null);
        }
        onBartenderSynced?.();
      }
    } catch (err) {
      lastSyncAtRef.current = Date.now();
      if (samePathString(requestProjectRoot, latestBartenderProjectRootRef.current)) {
        setActionMessage(String(err));
      }
    } finally {
      if (mode === 'silent') silentSyncRunningRef.current = false;
      setSyncProgress(null);
      finishLocalSync();
    }
  }, [agentMeta, bartenderProjectRoot, bartenderRequest, busyAction, canUseBartender, conflictBlocker, finishLocalSync, onBartenderSynced, startLocalSync, workingAgents]);

  const runPull = useCallback(async () => {
    if (!canUseBartender || busyAction || conflictBlocker) return;
    setBusyAction('pull');
    setActionMessage(null);
    try {
      const result = await bartenderPullFromGithub(bartenderRequest);
      setStatus(result.status);
      setActionMessage(result.message);
      setPullConflict(result.needsHumanPick ? (result.conflict ?? null) : null);
      if (result.ok) onBartenderSynced?.();
    } catch (err) {
      setActionMessage(String(err));
    } finally {
      setBusyAction(null);
    }
  }, [bartenderRequest, busyAction, canUseBartender, conflictBlocker, onBartenderSynced]);

  const runPush = useCallback(async () => {
    if (!canUseBartender || busyAction || conflictBlocker) return;
    setBusyAction('push');
    setActionMessage(null);
    try {
      const result = await bartenderPushToGithub(bartenderRequest);
      setStatus(result.status);
      setActionMessage(result.message);
    } catch (err) {
      setActionMessage(String(err));
    } finally {
      setBusyAction(null);
    }
  }, [bartenderRequest, busyAction, canUseBartender, conflictBlocker]);

  const routePullConflict = useCallback(async (agentId: AgentId) => {
    if (!canUseBartender || busyAction || !pullConflict) return;
    setBusyAction('routePull');
    try {
      const result = await bartenderRoutePullConflict({
        projectRoot: bartenderProjectRoot,
        agentId,
        pullConflictPrompt: bartenderPrompts.pullConflictPrompt,
      });
      setStatus(result.status);
      setActionMessage(result.message);
      if (result.ok) setPullConflict(null);
    } catch (err) {
      setActionMessage(String(err));
    } finally {
      setBusyAction(null);
    }
  }, [
    bartenderProjectRoot,
    bartenderPrompts.pullConflictPrompt,
    busyAction,
    canUseBartender,
    pullConflict,
  ]);

  const openConflictBlocker = useCallback(() => {
    if (!conflictBlocker) return;
    onOpenAgentFilteredChat?.(conflictBlocker.agentId);
  }, [conflictBlocker, onOpenAgentFilteredChat]);

  const unlockConflictBlocker = useCallback(() => {
    setConflictBlocker(null);
  }, []);

  // Latest values the silent-sync tick reads, kept in a ref so the 15s interval
  // below never has to rebuild. (status is refreshed every 12s; when it was an
  // effect dep, the 15s timer got cleared+recreated before it could ever fire —
  // that was the auto-sync-never-runs bug.) Updating this ref does NOT rebuild
  // the timer; only autoSyncEnabled/canUseBartender do.
  const autoSyncStateRef = useRef({ status, busyAction: activeBusyAction, workingCount, conflictBlocker, lastPromptAt, runSync });
  useEffect(() => {
    autoSyncStateRef.current = {
      status,
      busyAction: activeBusyAction,
      workingCount,
      conflictBlocker,
      lastPromptAt,
      runSync,
    };
  }, [activeBusyAction, status, workingCount, conflictBlocker, lastPromptAt, runSync]);

  useEffect(() => {
    if (!autoSyncEnabled || !canUseBartender) return;
    const timer = window.setInterval(() => {
      const s = autoSyncStateRef.current;
      // dynamic guards live inside the tick now (read from the ref), so changing
      // status/workingCount/etc no longer resets the interval.
      if (!s.status || s.status.roomChangeCount <= 0 || s.conflictBlocker) return;
      if (silentSyncRunningRef.current || s.busyAction || s.workingCount > 0) return;
      const now = Date.now();
      const promptTime = s.lastPromptAt ? Date.parse(s.lastPromptAt) : 0;
      const promptCold = !Number.isFinite(promptTime) || promptTime <= 0 || now - promptTime >= BARTENDER_COLD_MS;
      const syncCold = lastSyncAtRef.current <= 0 || now - lastSyncAtRef.current >= BARTENDER_COLD_MS;
      if (!promptCold || !syncCold) return;
      void s.runSync('silent');
    }, 15000);
    return () => window.clearInterval(timer);
  }, [autoSyncEnabled, canUseBartender]);

  const bartenderBubble = actionMessage
    ?? status?.message
    ?? (workspace ? 'Checking the room.' : 'Open a GitHub room to sync.');
  const roomChangeCount = status?.roomChangeCount ?? 0;
  const githubChangeCount = status?.githubChangeCount ?? 0;
  const githubBehindCount = status?.githubBehindCount ?? 0;
  const githubNeedsInitialPush = status?.githubNeedsInitialPush ?? false;
  const roomChangeLabel = roomChangeCount === 1 ? '1 changed file' : `${roomChangeCount} changed files`;
  const syncButtonLabel = activeBusyAction === 'sync'
    ? bartenderSyncButtonLabel(activeSyncProgress)
    : `Sync ${roomChangeLabel} in room`;
  const githubBehindLabel = githubBehindCount === 1 ? '1 change' : `${githubBehindCount} changes`;
  const hasUpstreamBehind = githubBehindCount > 0;
  const conflictBlockerText = conflictBlocker
    ? `${conflictBlocker.agentName} resolving`
    : '';
  const conflictBlockerTitle = conflictBlocker
    ? `${conflictBlocker.agentName} resolving${conflictBlocker.count > 1 ? `; ${conflictBlocker.count - 1} more conflict${conflictBlocker.count === 2 ? '' : 's'} pending` : ''}`
    : '';
  const bbsThreads = bbs?.threads ?? [];
  const bbsNewCount = bbs?.newCount ?? 0;
  const bbsKnownProjects = useMemo(() => {
    const seen = new Set<string>();
    const projects: BbsPromptProject[] = [];
    const source = workspaceProjects?.length ? workspaceProjects : workspace ? [workspace] : [];
    const add = (project: BbsPromptProject) => {
      if (!project.projectId || seen.has(project.projectId)) return;
      seen.add(project.projectId);
      projects.push(project);
    };
    source.forEach((project) => add(workspacePromptProject(project)));
    if (workspace) add(workspacePromptProject(workspace));
    return projects;
  }, [workspace, workspaceProjects]);
  const bbsCurrentProject = useMemo<BbsPromptProject | null>(() => {
    if (!workspace) return null;
    const fallback = workspacePromptProject(workspace);
    return {
      projectId: workspace.projectId,
      displayName: bbs?.projectDisplayName ?? fallback.displayName,
    };
  }, [bbs?.projectDisplayName, workspace]);
  const bbsTargetProjects = useMemo(
    () => bbsKnownProjects.filter((project) => project.projectId !== bbsProjectId),
    [bbsKnownProjects, bbsProjectId],
  );
  const visibleBbsThreads = useMemo(() => (
    bbsThreads.filter((thread) => {
      // "Tagged" means relevant-to-me (backend rule: broadcast, tagged to me,
      // created by me, or I posted in it) — not just literally tagged. The
      // stricter projectTags check hid threads this project created, which
      // made replies to them unreachable in this filter.
      if (bbsFilter === 'tagged' && !thread.relevant) return false;
      return true;
    })
  ), [bbsFilter, bbsThreads]);
  const latestVioletSummary = violetSummary?.latest ?? null;
  const violetOutstanding = violetSummary?.outstanding.messageCount ?? 0;
  const violetSummaryRunning = violetSummaryBusy || violetSummaryAutoBusy;
  const violetSummaryStart = latestVioletSummary?.summaryStartTs ?? null;
  const violetSummaryEnd = latestVioletSummary?.summaryEndTs ?? null;
  const violetSummaryMessages = latestVioletSummary?.messageCount ?? 0;
  const violetSummaryManualTimeoutSeconds = violetSummaryManualDeadlineAt
    ? Math.max(0, Math.ceil((violetSummaryManualDeadlineAt - violetSummaryManualNow) / 1000))
    : VIOLET_SUMMARY_CLI_TIMEOUT_SECS;
  const violetAutoCountdownDueAt = summaryAutoCountdownDueAt(
    latestVioletSummary,
    violetOutstanding,
    violetSummaryConfig.triggerAMessages,
    violetSummaryConfig.triggerBHours,
    violetSummaryConfig.triggerBMinOutstanding,
  );
  const violetAutoCountdown = summaryAutoCountdownLabel(
    latestVioletSummary,
    violetOutstanding,
    violetSummaryConfig.triggerAMessages,
    violetSummaryConfig.triggerBHours,
    violetSummaryConfig.triggerBMinOutstanding,
    violetAutoCountdownNow,
  );
  useEffect(() => {
    if (!violetAutoCountdownDueAt) return undefined;
    const tick = () => setVioletAutoCountdownNow(Date.now());
    tick();
    if (Date.now() >= violetAutoCountdownDueAt) return undefined;
    const timer = window.setInterval(() => {
      const next = Date.now();
      setVioletAutoCountdownNow(next);
      if (next >= violetAutoCountdownDueAt) window.clearInterval(timer);
    }, 1000);
    return () => window.clearInterval(timer);
  }, [violetAutoCountdownDueAt]);
  const violetSummaryRange = latestVioletSummary
    ? summaryRangeLabel(violetSummaryStart, violetSummaryEnd, violetSummaryMessages)
    : null;
  const violetOutstandingLabel = violetOutstanding > 0
    ? `${summaryMessageLabel(violetOutstanding)} outstanding${
      violetSummaryRunning
        ? ' · summary running'
        : violetAutoCountdown
          ? ` · ${violetAutoCountdown === 'Auto summary due' ? 'summary due' : `summary running in ${violetAutoCountdown}`}`
          : ''
    }`
    : null;
  const violetSummaryBullets = latestVioletSummary?.completed.length
    ? latestVioletSummary.completed.slice(0, 5)
    : ['Your auto summary will be here'];
  const canScheduleEmber = !!emberText.trim() && emberTargets.length > 0 && !!emberProjectRoot
    && !(emberRepeatEnabled && emberRepeatKind === 'monthly' && emberEveryNMonths >= 100);
  const activeEmberSchedules = emberSchedules.filter((schedule) => (
    schedule.status !== 'sent' && schedule.id !== emberBusy
  ));
  const hasEmberDeliveryFailure = emberSchedules.some((schedule) => (
    !!schedule.error?.trim()
  ));
  const emberRunBusy = emberBusy ? emberSchedules.find((schedule) => schedule.id === emberBusy) ?? null : null;
  const activeEmberCount = activeEmberSchedules.length;
  const emberDraftCount = emberDrafts.length;
  const emberHistoryCount = emberHistory.length;
  const emberHistoryDetail = emberHistoryDetailId
    ? emberHistory.find((record) => record.id === emberHistoryDetailId) ?? null
    : null;
  const emberEditing = emberEditorTarget !== null;
  const workedDurationParts = formatWorkedDurationParts(emberNow - workStartedAt);
  // Daybar scrub: drag the day window horizontally to nudge the background.
  // The real time-driven position (--ember-daybar-x) stays put; only the scrub
  // offset (--daybar-drag) changes, and it springs back to 0 on release.
  const daybarWinRef = useRef<HTMLSpanElement | null>(null);
  const daybarOffsetRef = useRef(0);
  const daybarVelRef = useRef(0);
  const daybarDragRef = useRef<{ startX: number; startOffset: number } | null>(null);
  const daybarRafRef = useRef<number | null>(null);

  const stepDaybar = useCallback(() => {
    if (!daybarDragRef.current) {
      daybarVelRef.current += -daybarOffsetRef.current * 0.16;
      daybarVelRef.current *= 0.74;
      daybarOffsetRef.current += daybarVelRef.current;
      if (Math.abs(daybarOffsetRef.current) < 0.3 && Math.abs(daybarVelRef.current) < 0.3) {
        daybarOffsetRef.current = 0;
        daybarVelRef.current = 0;
        daybarWinRef.current?.style.setProperty('--daybar-drag', '0px');
        daybarRafRef.current = null;
        return;
      }
    }
    daybarWinRef.current?.style.setProperty('--daybar-drag', `${daybarOffsetRef.current.toFixed(2)}px`);
    daybarRafRef.current = requestAnimationFrame(stepDaybar);
  }, []);

  const onDaybarPointerDown = useCallback((event: ReactPointerEvent<HTMLSpanElement>) => {
    daybarDragRef.current = { startX: event.clientX, startOffset: daybarOffsetRef.current };
    daybarVelRef.current = 0;
    event.currentTarget.setPointerCapture?.(event.pointerId);
    if (daybarRafRef.current == null) daybarRafRef.current = requestAnimationFrame(stepDaybar);
    event.preventDefault();
  }, [stepDaybar]);

  const onDaybarPointerMove = useCallback((event: ReactPointerEvent<HTMLSpanElement>) => {
    const drag = daybarDragRef.current;
    if (!drag) return;
    daybarOffsetRef.current = drag.startOffset + (event.clientX - drag.startX);
  }, []);

  const endDaybarDrag = useCallback(() => {
    if (!daybarDragRef.current) return;
    daybarDragRef.current = null;
    if (daybarRafRef.current == null) daybarRafRef.current = requestAnimationFrame(stepDaybar);
  }, [stepDaybar]);

  useEffect(() => () => {
    if (daybarRafRef.current != null) cancelAnimationFrame(daybarRafRef.current);
  }, []);

  // Reset key (right of SECONDS): zero the worked session, then keep counting.
  // Reuses the same reset Good Night uses, so launch auto-start is unaffected.
  const resetWorkedRef = useRef<HTMLButtonElement | null>(null);
  const onResetWorked = useCallback(() => {
    setWorkStartedAt(resetEmberWorkSessionStartedAt());
    const el = resetWorkedRef.current;
    if (el) { el.classList.remove('spring'); void el.offsetWidth; el.classList.add('spring'); }
  }, []);

  const workedDaybarStyle = {
    '--ember-daybar-x': `${daybarPositionX(emberNow)}px`,
  } as CSSProperties;
  const dreamRemainingSeconds = dreamOverlay === 'countdown'
    ? dreamCountdownPaused
      ? Math.max(0, dreamPausedRemainingSeconds ?? 0)
      : dreamDueAt
        ? Math.max(0, Math.ceil((dreamDueAt - emberNow) / 1000))
        : 0
    : 0;
  const dreamCountdownLabel = `${Math.floor(dreamRemainingSeconds / 60)}:${String(dreamRemainingSeconds % 60).padStart(2, '0')}`;
  const dreamCountdownInfoLabel = dreamDigestWaitActive ? DREAM_DIGEST_WAIT_MESSAGE : null;
  const dreamStatusLabel = dreamScope
    ? `${dreamScope} dreaming`
    : dreamingAgents.length > 0
      ? `${dreamAgentList(dreamingAgents)} ${dreamingAgents.length === 1 ? 'is' : 'are'} dreaming`
      : 'Ember is dreaming';
  const dreamElapsedLabel = dreamOverlay === 'dreaming' && dreamStartedAtRef.current > 0
    ? formatDreamElapsed(emberNow - dreamStartedAtRef.current)
    : '0s';
  const dreamFinishedLabel = dreamScope
    ? `${dreamScope} had dreams`
    : dreamingAgents.length > 0
      ? `${dreamAgentList(dreamingAgents)} had dreams`
      : 'Ember had dreams';
  const dreamWaitingNames = Array.from(workingAgents ?? []).map((agentId) => emberAgentName(agentId, agentMeta));
  const dreamWaitingLabel = dreamWaitingNames.length > 0
    ? `Waiting for ${dreamAgentList(dreamWaitingNames)} to finish current work before Good Night.`
    : 'Good Night will start when the room is ready.';
  const dreamVideoSet = DREAM_VIDEO_SETS[dreamVideoSetIndex ?? 0] ?? DREAM_VIDEO_SETS[0];
  const dreamVideoSrc = dreamOverlay !== 'off'
    ? dreamVideoSet.clips[dreamOverlay as DreamVideoPhase]
    : null;
  const renderVioletHistoryEntry = (entry: VioletSummaryEntry) => (
    <article key={entry.id} className="violet-summary-history-entry">
      <div className="violet-summary-kv">
        <span>Last update</span>
        <b>{formatSummaryTime(entry.updatedAt)}</b>
      </div>
      <div className="violet-summary-window">
        Since {formatSummaryTime(entry.summaryStartTs, 'Beginning')} · {summaryMessageLabel(entry.messageCount)}
      </div>
      <ul className="violet-summary-bullets">
        {entry.completed.slice(0, 5).map((item, index) => (
          <li key={`${entry.id}-${index}`}>{item}</li>
        ))}
      </ul>
    </article>
  );

  // Load the account user identity (颦儿's account-user.json) once for
  // Human-authored posts: display name + avatar.
  useEffect(() => {
    let cancelled = false;
    void loadAccountUserIdentity()
      .then((identity) => {
        if (!cancelled) setBbsUserIdentity(identity);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);

  const refreshLm = useCallback(async () => {
    try {
      setLm(await lmStatus());
    } catch (err) {
      console.warn('[laughing-man] status failed', err);
    }
  }, []);

  const retryLmStart = useCallback(async () => {
    if (lmRetryBusy) return;
    setLmRetryBusy(true);
    try {
      await lmStart();
    } catch (err) {
      console.warn('[laughing-man] retry start failed', err);
    } finally {
      await refreshLm();
      setLmRetryBusy(false);
    }
  }, [lmRetryBusy, refreshLm]);

  const setLmMuted = useCallback(async (muted: boolean) => {
    if (lmMuteBusy) return;
    setLmMuteBusy(true);
    try {
      const next = await lmSetMuted(muted);
      if (next) setLm(next);
    } catch (err) {
      console.warn('[laughing-man] set mute failed', err);
      await refreshLm();
    } finally {
      setLmMuteBusy(false);
    }
  }, [lmMuteBusy, refreshLm]);

  const refreshLmQueue = useCallback(async () => {
    try {
      setLmQueue(await lmStandbyQueue(200));
    } catch (err) {
      console.warn('[laughing-man] standby queue failed', err);
      setLmQueue([]);
    }
  }, []);

  useEffect(() => {
    void refreshLm();
    const timer = window.setInterval(() => {
      void refreshLm();
    }, 12000);
    return () => window.clearInterval(timer);
  }, [refreshLm]);

  useEffect(() => {
    if (!lmHistoryOpen) return;
    void lmMessageLog(200).then(setLmLog).catch(() => setLmLog([]));
    void refreshLmQueue();
  }, [lmHistoryOpen, lm?.latest?.id, lm?.standby?.queueCount, refreshLmQueue]);

  const lmVisibleLog = useMemo(() => (
    lmLogFilter === 'project' && bbsProjectId
      ? lmLog.filter((entry) => entry.projectId === bbsProjectId)
      : lmLog
  ), [bbsProjectId, lmLog, lmLogFilter]);

  const lmVisibleQueue = useMemo(() => (
    lmLogFilter === 'project' && bbsProjectId
      ? lmQueue.filter((entry) => entry.projectId === bbsProjectId)
      : lmQueue
  ), [bbsProjectId, lmLogFilter, lmQueue]);

  const sendLmQueued = useCallback(async (id: string) => {
    setLmQueueBusy(`send:${id}`);
    setLmQueueError(null);
    try {
      await lmStandbySendQueued(id);
      await Promise.all([
        refreshLm(),
        refreshLmQueue(),
        lmMessageLog(200).then(setLmLog),
      ]);
    } catch (err) {
      setLmQueueError(String(err));
      await refreshLmQueue();
    } finally {
      setLmQueueBusy(null);
    }
  }, [refreshLm, refreshLmQueue]);

  const deleteLmQueued = useCallback(async (id: string) => {
    setLmQueueBusy(`delete:${id}`);
    setLmQueueError(null);
    try {
      await lmStandbyDeleteQueued(id);
      await Promise.all([
        refreshLm(),
        refreshLmQueue(),
      ]);
    } catch (err) {
      setLmQueueError(String(err));
    } finally {
      setLmQueueBusy(null);
    }
  }, [refreshLm, refreshLmQueue]);

  const bbsDetailThread = useMemo(() => (
    bbsDetailThreadId
      ? bbsThreads.find((thread) => thread.threadId === bbsDetailThreadId) ?? null
      : null
  ), [bbsDetailThreadId, bbsThreads]);

  useEffect(() => {
    if (bbsView !== 'detail' || !bbsDetailThread || !bbsDetailScrollRef.current) return undefined;
    const pendingPostId = pendingBbsScrollPostIdRef.current;
    const pendingThreadId = pendingBbsScrollThreadIdRef.current;
    if (!pendingPostId && pendingThreadId !== bbsDetailThread.threadId) return undefined;
    if (pendingPostId && !bbsDetailThread.posts.some((post) => post.postId === pendingPostId)) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const scrollRoot = bbsDetailScrollRef.current;
      if (!scrollRoot) return;
      const target = pendingPostId
        ? Array.from(scrollRoot.querySelectorAll<HTMLElement>('[data-bbs-post-id]'))
          .find((node) => node.dataset.bbsPostId === pendingPostId)
        : scrollRoot.querySelector<HTMLElement>('[data-bbs-post-id]');
      if (!target) return;
      target.scrollIntoView({ block: pendingPostId ? 'center' : 'start', behavior: 'smooth' });
      pendingBbsScrollPostIdRef.current = null;
      pendingBbsScrollThreadIdRef.current = null;
    });
    return () => window.cancelAnimationFrame(frame);
  }, [bbsDetailThread, bbsView]);

  // Opening a thread marks its unread posts as seen — that is what drives
  // the new-badge now that Insert/Ignore buttons are gone.
  const openBbsDetail = useCallback((thread: BbsThread) => {
    setBbsDetailThreadId(thread.threadId);
    setBbsView('detail');
    setBbsReplyText('');
    setBbsReplyAgents([]);
    setBbsReplyAgentBarOpen(false);
    if (!bbsProjectId) return;
    const unseen = thread.posts.filter((post) => post.state === 'new');
    if (unseen.length === 0) return;
    void Promise.all(unseen.map((post) => (
      bbsMarkProcessed({ projectId: bbsProjectId, postId: post.postId })
    )))
      .then(() => refreshBbs(true))
      .catch((err) => console.warn('[bbs] mark seen failed', err));
  }, [bbsProjectId, refreshBbs]);

  const closeBbsDetail = useCallback(() => {
    setBbsView('list');
    setBbsDetailThreadId(null);
  }, []);

  const openBbsCompose = useCallback(() => {
    setBbsComposeText('');
    setBbsComposeProjects(new Set(
      bbsTargetProjects.length === 1 ? [bbsTargetProjects[0]!.projectId] : [],
    ));
    setBbsReplyAgents([]);
    setBbsReplyAgentBarOpen(false);
    setBbsView('compose');
  }, [bbsTargetProjects]);

  // AKA (short name), matching the main room agent bar format.
  const bbsAgentNameFor = useCallback((agentId: AgentId) => {
    const full = agentMeta?.[agentId]?.name
      ?? emberAgentOptions.find((agent) => agent.id === agentId)?.name
      ?? agentId;
    return splitProjectAgentName(full).base || full;
  }, [agentMeta, emberAgentOptions]);

  // Notify the selected agents over the bus with the reply wrapper, so the
  // room shows the collapsed "Check this BBS Thread" bubble and the agents
  // get actionable context plus the human's message.
  const notifyBbsAgents = useCallback(async (
    threadId: string,
    humanBody: string,
    agents: readonly AgentId[],
  ) => {
    if (!emberProjectRoot || !bbsCurrentProject || agents.length === 0) return;
    const wrapper = await renderBbsReplyPromptFromFile({
      currentProject: bbsCurrentProject,
      threadId,
      sourceProject: bbsCurrentProject,
      latestAuthor: bbsUserIdentity?.name?.trim() || 'Human',
    });
    const text = `${wrapper}\n${humanBody}`;
    await Promise.all(agents.map((agentId) => agentBusSend({
      projectRoot: emberProjectRoot,
      senderAgentId: BBS_ACTOR_ID,
      senderName: BBS_ACTOR_NAME,
      target: agentId,
      intent: 'bbs-thread',
      text,
      eventId: emberEventId('bbs', threadId, agentId),
      dedupeKey: null,
    })));
  }, [bbsCurrentProject, bbsUserIdentity, emberProjectRoot]);

  const sendBbsReply = useCallback(async () => {
    if (!bbsProjectId || !bbsDetailThread || bbsReplyBusy) return;
    const text = (bbsReplyInputRef.current?.serialize().payload ?? bbsReplyText).trim();
    if (!text) return;
    const mentions = bbsReplyAgents.map((agentId) => `@${bbsAgentNameFor(agentId)}`).join(' ');
    const body = mentions ? `${mentions}\n${text}` : text;
    setBbsReplyBusy(true);
    setBbsError(null);
    try {
      const postId = await bbsHumanReply({
        projectId: bbsProjectId,
        projectDisplayName: bbsCurrentProject?.displayName ?? null,
        threadId: bbsDetailThread.threadId,
        body,
      });
      pendingBbsScrollPostIdRef.current = postId;
      pendingBbsScrollThreadIdRef.current = bbsDetailThread.threadId;
      await notifyBbsAgents(bbsDetailThread.threadId, body, bbsReplyAgents);
      setBbsReplyText('');
      bbsReplyInputRef.current?.clear();
      setBbsReplyAgents([]);
      await refreshBbs(true);
    } catch (err) {
      setBbsError(String(err));
    } finally {
      setBbsReplyBusy(false);
    }
  }, [
    bbsAgentNameFor,
    bbsCurrentProject?.displayName,
    bbsDetailThread,
    bbsProjectId,
    bbsReplyAgents,
    bbsReplyBusy,
    bbsReplyText,
    notifyBbsAgents,
    refreshBbs,
  ]);

  const submitBbsPost = useCallback(async () => {
    if (!bbsProjectId || bbsReplyBusy) return;
    const text = (bbsComposeInputRef.current?.serialize().payload ?? bbsComposeText).trim();
    const targets = Array.from(bbsComposeProjects);
    if (!text) return;
    const mentions = bbsReplyAgents.map((agentId) => `@${bbsAgentNameFor(agentId)}`).join(' ');
    const body = mentions ? `${mentions}\n${text}` : text;
    setBbsReplyBusy(true);
    setBbsError(null);
    try {
      const threadId = await bbsHumanPost({
        projectId: bbsProjectId,
        projectDisplayName: bbsCurrentProject?.displayName ?? null,
        projectTags: targets,
        body,
      });
      pendingBbsScrollPostIdRef.current = null;
      pendingBbsScrollThreadIdRef.current = threadId;
      await notifyBbsAgents(threadId, body, bbsReplyAgents);
      setBbsComposeText('');
      bbsComposeInputRef.current?.clear();
      setBbsReplyAgents([]);
      setBbsDetailThreadId(threadId);
      setBbsView('detail');
      await refreshBbs(true);
    } catch (err) {
      setBbsError(String(err));
    } finally {
      setBbsReplyBusy(false);
    }
  }, [
    bbsAgentNameFor,
    bbsComposeProjects,
    bbsComposeText,
    bbsCurrentProject?.displayName,
    bbsProjectId,
    bbsReplyAgents,
    bbsReplyBusy,
    notifyBbsAgents,
    refreshBbs,
  ]);

  // window.confirm is a silent no-op inside the Tauri webview (this is why
  // the old delete button "did nothing") — confirmation runs through the
  // in-app card driven by bbsDeleteTarget instead.
  const deleteBbsPost = useCallback(async (post: BbsPost) => {
    const deleteThread = post.kind === 'topic';
    setBbsBusy(post.postId);
    try {
      await bbsDelete({ threadId: post.threadId, postId: post.postId });
      setBbs((prev) => removeBbsPostFromSnapshot(prev, post));
      if (deleteThread) closeBbsDetail();
      await refreshBbs(true);
    } catch (err) {
      setBbsError(String(err));
    } finally {
      setBbsBusy(null);
    }
  }, [closeBbsDetail, refreshBbs]);

  // Slack-flat forum message: avatar | author + time + #floor | body.
  // Human-authored posts (agent_id 'human') get the serif italic name.
  const renderBbsFloor = (post: BbsPost, floor: number, isTopic: boolean, thread: BbsThread) => {
    const isHuman = post.agentId === 'human';
    const meta = agentMeta?.[post.agentId as AgentId];
    const avatarId = post.agentAvatar ?? meta?.avatarId ?? (isHuman ? 'user-default' : null);
    const avatarClass = avatarId
      ? avatarClassForId(avatarId, null)
      : meta?.avatarClass ?? avatarClassForAgentFallback(null, post.agentId);
    const avatarStyle = avatarImageStyleForId(avatarId);
    return (
      <div
        key={post.postId}
        className={`bbs-msg ${isHuman ? 'human' : ''}`}
        data-bbs-post-id={post.postId}
      >
        <span
          className={`bbs-msg-avatar tavern-avatar-art ${avatarClass}`}
          style={avatarStyle}
          aria-hidden
        >
          <span />
          <i />
          <b />
        </span>
        <div className="bbs-msg-main">
          <div className="bbs-msg-head">
            <span className={`bbs-msg-author ${isHuman ? 'human' : ''}`}>{post.agentDisplayName}</span>
            {isTopic && <span className="bbs-op-badge">OP</span>}
            <span className="bbs-time">{formatBbsTime(post.createdAt)}</span>
            <span className="bbs-floor-no">#{floor}</span>
          </div>
          {isTopic && (
            <div className="bbs-tags">
              <span className="bbs-tag-label">From</span>
              <span className="bbs-tag">{post.projectDisplayName}</span>
              {thread.visibility === 'broadcast' ? (
                <span className="bbs-tag broadcast">Broadcast</span>
              ) : (
                <>
                  <span className="bbs-tag-label">To</span>
                  {thread.projectTagLabels.map((tag) => (
                    <span key={`${post.postId}-${tag}`} className="bbs-tag">{tag}</span>
                  ))}
                </>
              )}
            </div>
          )}
          <div className="bbs-msg-body">
            <MarkdownText text={post.body} />
            <BbsPostImages body={post.body} baseRoot={emberProjectRoot} />
          </div>
        </div>
        <div className="bbs-msg-side">
          <button
            type="button"
            className="bbs-floor-delete"
            disabled={bbsBusy === post.postId}
            onClick={() => setBbsDeleteTarget(post)}
          >
            Delete
          </button>
        </div>
      </div>
    );
  };

  // Shared composer footer (reply + compose reuse the same shell).
  const renderBbsAgentBar = () => (
    <div className={`bbs-agent-bar ${bbsReplyAgentBarOpen ? 'open' : ''}`}>
      <span className="bbs-agent-bar-hint">
        Selected agents get a bus delivery after the post lands; none selected = plain post.
      </span>
      {emberAgentOptions.length === 0 ? (
        <span className="ember-target-empty">No active agent</span>
      ) : emberAgentOptions.map((agent) => {
        const selected = bbsReplyAgents.includes(agent.id);
        const meta = agentMeta?.[agent.id];
        const chipClass = meta?.avatarClass ?? avatarClassForAgentFallback(null, agent.id);
        const chipStyle = avatarImageStyleForId(meta?.avatarId);
        return (
          <button
            key={agent.id}
            type="button"
            className={`bbs-agent-chip ${selected ? 'sel' : ''}`}
            aria-pressed={selected}
            onClick={() => {
              setBbsReplyAgents((current) => (
                current.includes(agent.id)
                  ? current.filter((candidate) => candidate !== agent.id)
                  : [...current, agent.id]
              ));
            }}
          >
            <span className={`bbs-msg-avatar sm tavern-avatar-art ${chipClass}`} style={chipStyle} aria-hidden>
              <span />
              <i />
              <b />
            </span>
            <span className="bbs-agent-chip-name">{bbsAgentNameFor(agent.id)}</span>
          </button>
        );
      })}
    </div>
  );

  return (
    <>
      <aside className={`sidebar-right ${dreamOverlay !== 'off' ? 'night-active' : ''}`}>
      <div className="sr-head ember-room-head">
        <div className="ember-worked-meter" aria-label={workedDurationParts.label}>
          <span
            className="ember-worked-window"
            aria-hidden="true"
            ref={daybarWinRef}
            style={workedDaybarStyle}
            onPointerDown={onDaybarPointerDown}
            onPointerMove={onDaybarPointerMove}
            onPointerUp={endDaybarDrag}
            onPointerCancel={endDaybarDrag}
            onLostPointerCapture={endDaybarDrag}
          >
            <span className="ember-worked-window-track" />
            <span className="ember-worked-window-glass" />
          </span>
          <span className="ember-worked-flip" aria-hidden="true">
            <span className="ember-worked-flip-group">
              <span className="ember-worked-flip-cards">
                <span className="ember-worked-flip-card"><span>{workedDurationParts.hours[0]}</span></span>
                <span className="ember-worked-flip-card"><span>{workedDurationParts.hours[1]}</span></span>
              </span>
              <span className="ember-worked-flip-label">HRS</span>
            </span>
            <span className="ember-worked-flip-group">
              <span className="ember-worked-flip-cards">
                <span className="ember-worked-flip-card"><span>{workedDurationParts.minutes[0]}</span></span>
                <span className="ember-worked-flip-card"><span>{workedDurationParts.minutes[1]}</span></span>
              </span>
              <span className="ember-worked-flip-label">MIN</span>
            </span>
            <span className="ember-worked-flip-group">
              <span className="ember-worked-flip-cards">
                <span className="ember-worked-flip-card"><span>{workedDurationParts.seconds[0]}</span></span>
                <span className="ember-worked-flip-card"><span>{workedDurationParts.seconds[1]}</span></span>
              </span>
              <span className="ember-worked-flip-label">SEC</span>
            </span>
            <button
              type="button"
              className="ember-worked-reset"
              ref={resetWorkedRef}
              tabIndex={-1}
              aria-label="Reset worked timer"
              title="Reset worked timer"
              onClick={onResetWorked}
              onAnimationEnd={() => resetWorkedRef.current?.classList.remove('spring')}
            >
              <span className="ember-worked-reset-stud">
                <svg viewBox="0 0 24 24" fill="none" strokeLinecap="round" strokeLinejoin="round" strokeWidth="1.9">
                  <path d="M18.6 7.6 A7.2 7.2 0 1 0 19.3 12.4" />
                  <path d="M18.7 3.4 L18.7 7.9 L14.2 7.9" />
                </svg>
              </span>
            </button>
          </span>
        </div>
        <RockerSwitch
          checked={dreamOverlay === 'off'}
          label={dreamOverlay === 'off' ? 'Good Night' : 'Back to Kota'}
          disabled={dreamOverlay === 'off' && (!emberProjectRoot || dreamTargetProjects.length === 0)}
          className="room-night-switch"
          onClick={() => {
            if (dreamOverlay === 'off') startDreamCountdown();
            else cancelDreamCountdown();
          }}
        />
      </div>
      <div className="sr-body">
        <div className="system-agent-stack">
          <section
            className="violet-card"
            role="button"
            tabIndex={0}
            aria-label="Open Violet summary history"
            onClick={() => setVioletHistoryOpen(true)}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' && event.key !== ' ') return;
              event.preventDefault();
              setVioletHistoryOpen(true);
            }}
          >
            <div className="violet-card-head">
              <span className="system-agent-avatar tavern-avatar-art system-violet" aria-hidden>
                <img src={violetAvatarUrl} alt="" />
              </span>
              <span className="system-agent-copy violet-summary-title">
                <b>Violet</b>
                <small className="violet-summary-meta-line">
                  Last update <strong>{formatSummaryTime(latestVioletSummary?.updatedAt)}</strong>
                </small>
                {violetSummaryRange && (
                  <small className="violet-summary-meta-line violet-summary-range" title={violetSummaryRange}>
                    {violetSummaryRange}
                  </small>
                )}
              </span>
            </div>
            <ul className="violet-summary-bullets">
              {violetSummaryBullets.map((item, index) => (
                <li key={`${latestVioletSummary?.id ?? 'empty'}-${index}`}>{item}</li>
              ))}
            </ul>
            {violetOutstandingLabel && (
              <div className="violet-summary-outstanding" title={violetOutstandingLabel}>
                {violetOutstandingLabel}
              </div>
            )}
            {violetOutstanding > 0 && (
              <button
                type="button"
                className="violet-summary-now"
                disabled={violetSummaryRunning}
                onClick={(event) => {
                  event.stopPropagation();
                  void runVioletSummaryNow();
                }}
              >
                {violetSummaryBusy
                  ? `Timeout in ${violetSummaryManualTimeoutSeconds} s`
                  : violetSummaryAutoBusy
                    ? 'Summary Running'
                    : 'Summary Now'}
              </button>
            )}
            {violetSummaryError && <small className="violet-summary-error">{violetSummaryError}</small>}
          </section>
          <div className={`bartender-card ${status?.state ?? 'idle'} ${activeBusyAction ? 'busy' : ''}`}>
            <div className="bartender-row">
              <div className="bartender-avatar tavern-avatar-art system-bartender" aria-hidden>
                <span />
                <i />
                <b />
              </div>
              <div className="bartender-copy">
                <div className="bartender-title-line">
                  <b>Bartender</b>
                </div>
                <div className="bartender-bubble" aria-live="polite">
                  {bartenderBubble}
                </div>
              </div>
              <button
                type="button"
                className={`bartender-toggle ${autoSyncEnabled ? 'enabled' : ''}`}
                role="switch"
                aria-checked={autoSyncEnabled}
                aria-label="Auto sync"
                disabled={!canUseBartender}
                title={autoSyncEnabled ? 'turn off auto sync' : 'turn on auto sync'}
                onClick={toggleAutoSync}
              >
                AUTO
              </button>
            </div>
            <div className="bartender-actions">
              <AnimatePresence initial={false}>
                {conflictBlocker && (
                  <motion.div
                    key="conflict-blocker"
                    className="bartender-conflict-blocker"
                    initial={{ opacity: 0, y: -4, height: 0 }}
                    animate={{ opacity: 1, y: 0, height: 32 }}
                    exit={{ opacity: 0, y: -4, height: 0 }}
                    transition={{ duration: 0.18, ease: 'easeOut' }}
                  >
                    <button
                      type="button"
                      className="bartender-action conflict-open"
                      disabled={activeBusyAction !== null}
                      onClick={openConflictBlocker}
                      title={conflictBlockerTitle}
                      aria-label={conflictBlockerTitle}
                    >
                      <span className="bartender-conflict-open-label">{conflictBlockerText}</span>
                    </button>
                    <button
                      type="button"
                      className="bartender-action conflict-unlock"
                      onClick={unlockConflictBlocker}
                    >
                      Unlock
                    </button>
                  </motion.div>
                )}
                {!conflictBlocker && roomChangeCount > 0 && (
                  <motion.div
                    key="sync"
                    className="bartender-action-wrap"
                    initial={{ opacity: 0, y: -4, height: 0 }}
                    animate={{ opacity: 1, y: 0, height: 32 }}
                    exit={{ opacity: 0, y: -4, height: 0 }}
                    transition={{ duration: 0.18, ease: 'easeOut' }}
                  >
                    <button
                      type="button"
                      className={`bartender-action sync ${activeBusyAction === 'sync' ? 'running' : ''}`}
                      disabled={activeBusyAction !== null}
                      title={activeBusyAction === 'sync' && activeSyncProgress ? activeSyncProgress.message : undefined}
                      onClick={() => void runSync('manual')}
                    >
                      {activeBusyAction === 'sync' && <span className="bartender-action-spinner" aria-hidden />}
                      {syncButtonLabel}
                    </button>
                    {hasUpstreamBehind && (
                      <span
                        className="bartender-upstream-dot"
                        title={`GitHub has ${githubBehindLabel} — sync local first`}
                        aria-label={`GitHub has ${githubBehindLabel}`}
                      />
                    )}
                  </motion.div>
                )}
                {!conflictBlocker && roomChangeCount === 0 && githubBehindCount > 0 && (
                  <motion.button
                    key="pull"
                    type="button"
                    className={`bartender-action pull ${activeBusyAction === 'pull' ? 'running' : ''}`}
                    initial={{ opacity: 0, y: -4, height: 0 }}
                    animate={{ opacity: 1, y: 0, height: 32 }}
                    exit={{ opacity: 0, y: -4, height: 0 }}
                    transition={{ duration: 0.18, ease: 'easeOut' }}
                    disabled={activeBusyAction !== null}
                    onClick={() => void runPull()}
                  >
                    {activeBusyAction === 'pull' && <span className="bartender-action-spinner" aria-hidden />}
                    {activeBusyAction === 'pull' ? 'Pulling GitHub' : `Pull ${githubBehindLabel} from GitHub`}
                  </motion.button>
                )}
                {!conflictBlocker && (githubChangeCount > 0 || githubNeedsInitialPush) && (
                  <motion.button
                    key="push"
                    type="button"
                    className={`bartender-action push ${activeBusyAction === 'push' ? 'running' : ''}`}
                    initial={{ opacity: 0, y: -4, height: 0 }}
                    animate={{ opacity: 1, y: 0, height: 32 }}
                    exit={{ opacity: 0, y: -4, height: 0 }}
                    transition={{ duration: 0.18, ease: 'easeOut' }}
                    disabled={activeBusyAction !== null || hasUpstreamBehind}
                    title={hasUpstreamBehind ? 'Pull from GitHub first' : undefined}
                    onClick={() => void runPush()}
                  >
                    {activeBusyAction === 'push' && <span className="bartender-action-spinner" aria-hidden />}
                    {activeBusyAction === 'push'
                      ? 'Pushing GitHub'
                      : githubNeedsInitialPush
                        ? 'Initial push to GitHub'
                        : `Push ${githubChangeCount} changes to GitHub`}
                  </motion.button>
                )}
              </AnimatePresence>
              {pullConflict && (
                <div className="bartender-pull-conflict-picker">
                  <div className="bartender-pull-conflict-copy">
                    <b>Pick an agent</b>
                    <span>GitHub pull conflicted with the local version.</span>
                  </div>
                  <div className="bartender-agent-cards">
                    {pullConflictAgents.length > 0 ? pullConflictAgents.map((agentId) => {
                      const agent = agentMeta?.[agentId];
                      const workspaceAgent = workspace?.agents.find((candidate) => candidate.agentId === agentId);
                      const name = agent?.name ?? agentId;
                      const avatarClass = agent?.avatarClass ?? avatarClassForAgentFallback(workspaceAgent?.cli, agentId);
                      const avatarStyle = avatarImageStyleForId(agent?.avatarId);
                      return (
                        <button
                          key={agentId}
                          type="button"
                          className={`bartender-agent-card ${activeBusyAction === 'routePull' ? 'running' : ''}`}
                          disabled={activeBusyAction !== null}
                          onClick={() => void routePullConflict(agentId)}
                        >
                          <span
                            className={`bartender-agent-avatar tavern-avatar-art ${avatarClass}`}
                            style={avatarStyle}
                            aria-hidden
                          >
                            <span />
                            <i />
                            <b />
                          </span>
                          <b>{name}</b>
                        </button>
                      );
                    }) : (
                      <span className="bartender-agent-empty">
                        No active agents in this room.
                      </span>
                    )}
                  </div>
                </div>
              )}
            </div>
          </div>
          {HOT_MEMORY_TEASER_ENABLED && (
            <button className="hot-mem-teaser" onClick={onOpenHotMem}>
              <span className="hm-fire">🔥</span>
              <span className="hm-label">Hot Memory</span>
              <span className="hm-sep">·</span>
              <span className="hm-count">{hotMem.totalRecords} records</span>
              <span className="hm-sep">·</span>
              <span className="hm-top">
                <code>{hotMem.topFile}</code> most used
              </span>
              <span className="hm-chev">›</span>
            </button>
          )}
          <section
            className={`ember-card ${dreamOverlay !== 'off' ? 'dream-active' : ''}`}
            role="button"
            tabIndex={0}
            aria-label={hasEmberDeliveryFailure
              ? 'Open Ember schedules and drafts, not delivered'
              : 'Open Ember schedules and drafts'}
            onClick={() => setEmberModalOpen(true)}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' && event.key !== ' ') return;
              event.preventDefault();
              setEmberModalOpen(true);
            }}
          >
            <div className="ember-card-head">
              <span className="ember-card-avatar" aria-hidden>
                <span className="system-agent-avatar tavern-avatar-art system-ember">
                  <span />
                  <i />
                  <b />
                </span>
                {hasEmberDeliveryFailure && <span className="ember-delivery-alert" />}
              </span>
              <span className="system-agent-copy">
                <b>Ember</b>
                <small>Timed prompts</small>
              </span>
              <span className="ember-card-counts" aria-label="Ember counts">
                {activeEmberCount > 0 && <span><b>{activeEmberCount}</b> Active</span>}
                {emberDraftCount > 0 && <span><b>{emberDraftCount}</b> Drafts</span>}
              </span>
            </div>
            {emberMessage && <div className="ember-status">{emberMessage}</div>}
          </section>
          <div
            className={`lm-card ${lm?.lastError ? 'error' : ''} ${lm?.selected?.muted ? 'muted' : ''}`}
            role="button"
            tabIndex={0}
            aria-label="Laughing Man Telegram bridge"
            onClick={() => {
              if (!lm?.configured) setLmSetupOpen(true);
              else setLmHistoryOpen(true);
            }}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' && event.key !== ' ') return;
              event.preventDefault();
              if (!lm?.configured) setLmSetupOpen(true);
              else setLmHistoryOpen(true);
            }}
          >
            <div className="lm-card-head">
              <span className="lm-avatar-wrap" aria-hidden>
                <span className="lm-avatar tavern-avatar-art system-laughing-man">
                  <span />
                  <i />
                  <b />
                </span>
                {lm?.botUsername && (
                  <span
                    className={`lm-dot ${
                      lm.lastError ? 'err' : !lm.ownerUserId ? 'awaiting' : lm.running ? 'on' : ''
                    }`}
                  />
                )}
              </span>
              <span className="lm-copy">
                <b>Laughing Man</b>
                <small>
                  {lm?.botUsername
                    ? `${lm.botUsername} · ${
                      lm.lastError
                        ? 'error'
                        : !lm.ownerUserId
                          ? 'awaiting owner'
                          : lm.selected?.muted
                            ? 'muted'
                            : lm.running
                            ? 'connected'
                            : 'starting…'
                    }`
                    : 'Telegram bridge'}
                </small>
              </span>
              {(lm?.ownerUserId && lm.selected && !lm.lastError) || lm?.botUsername ? (
                <span className="lm-actions">
                  {lm?.ownerUserId && lm.selected && !lm.lastError && (
                    <button
                      type="button"
                      className={`lm-mute-btn ${lm.selected.muted ? 'is-muted' : ''}`}
                      disabled={lmMuteBusy}
                      aria-label={lm.selected.muted ? 'Resume Telegram messaging' : 'Pause Telegram messaging'}
                      title={lm.selected.muted ? 'Resume Telegram messaging' : 'Pause Telegram messaging'}
                      onClick={(event) => {
                        event.stopPropagation();
                        void setLmMuted(!lm.selected?.muted);
                      }}
                    >
                      <LaughingManMuteIcon muted={lm.selected.muted} />
                    </button>
                  )}
                  {lm?.botUsername && (
                    <button
                      type="button"
                      className="lm-gear"
                      aria-label="Laughing Man settings"
                      onClick={(event) => {
                        event.stopPropagation();
                        setLmSetupOpen(true);
                      }}
                    >
                      ⚙
                    </button>
                  )}
                </span>
              ) : null}
            </div>
            {!lm?.botUsername ? (
              <button
                type="button"
                className="lm-setup-btn"
                onClick={(event) => {
                  event.stopPropagation();
                  setLmSetupOpen(true);
                }}
              >
                Set up Telegram Bot
              </button>
            ) : !lm.ownerUserId ? (
              <div className="lm-latest-text muted">Finish setup: claim ownership in settings (⚙)</div>
            ) : lm.lastError ? (
              <div className="lm-err-row">
                <span>{lm.lastError}</span>
                <button
                  type="button"
                  className="lm-retry-link"
                  disabled={lmRetryBusy}
                  onClick={(event) => {
                    event.stopPropagation();
                    void retryLmStart();
                  }}
                >
                  {lmRetryBusy ? 'retrying…' : 'retry'}
                </button>
              </div>
            ) : lm.latest ? (
              <div className="lm-latest">
                <div className="lm-latest-meta">
                  <span className="dir">{lm.latest.direction === 'out' ? '←' : '→'}</span>
                  <span>{lm.latest.agentName} · {lm.latest.projectName}</span>
                  <span className="lm-latest-time">{formatBbsTime(lm.latest.ts)}</span>
                </div>
                <div className="lm-latest-text">{lm.latest.preview}</div>
              </div>
            ) : (
              <div className="lm-latest-text muted">No messages yet — talk to the bot from your phone.</div>
            )}
            {lm?.selected?.muted && !lm.lastError && (
              <div className="lm-muted-note">Telegram Messaging Paused</div>
            )}
          </div>
          <div className={`ember-bbs-card bbs-card ${bbsNewCount > 0 ? 'has-new' : ''}`}>
            <div className="ember-bbs-row">
              <div className="ember-bbs-avatar tavern-avatar-art system-bbs" aria-hidden>
                <span />
                <i />
                <b />
              </div>
              <div className="ember-bbs-copy">
                <b>BBS</b>
                <div className="ember-bbs-bubble" aria-live="polite">
                  Cross-Project Messages{workspace && bbsNewCount > 0 ? ` · ${bbsNewCount} new` : ''}
                </div>
              </div>
            </div>
            <div className="ember-bbs-actions">
              <button
                type="button"
                className="ember-bbs-action primary"
                disabled={!workspace}
                onClick={() => {
                  setBbsView('list');
                  setBbsDetailThreadId(null);
                  setBbsOpen(true);
                  void refreshBbs();
                }}
              >
                {bbsNewCount > 0 ? `Open (${bbsNewCount} new)` : 'Open'}
              </button>
              <button
                type="button"
                className="ember-bbs-action"
                disabled={!workspace}
                onClick={() => {
                  setBbsOpen(true);
                  openBbsCompose();
                }}
              >
                Post
              </button>
            </div>
          </div>
          {footerSlot && <div className="sr-footer-slot">{footerSlot}</div>}
        </div>
      </div>
    </aside>
    {emberModalOpen && (
      <div className="ember-modal-shade" role="presentation">
        <section className={`ember-modal ${emberEditing ? 'editing' : ''}`} role="dialog" aria-modal="true" aria-label="Ember scheduled prompts">
          <header className="ember-modal-head">
            <div className="ember-modal-title">
              <span className="system-agent-avatar tavern-avatar-art system-ember" aria-hidden>
                <span />
                <i />
                <b />
              </span>
              <div>
                <b>Ember</b>
                <small>{emberEditing ? 'Edit timed prompt' : 'Project timed prompts and drafts'}</small>
              </div>
            </div>
            {emberEditing ? (
              <nav className="ember-step-nav" aria-label="Editor steps">
                <button
                  type="button"
                  aria-current={emberStep === 1 ? 'step' : undefined}
                  className={`ember-step-pip${emberStep === 1 ? ' on' : ''}`}
                  onClick={() => setEmberStep(1)}
                >
                  <b>1</b>
                  <span>Prompt &amp; targets</span>
                </button>
                <span className={`ember-step-line${emberStep === 2 ? ' on' : ''}`} aria-hidden />
                <button
                  type="button"
                  aria-current={emberStep === 2 ? 'step' : undefined}
                  className={`ember-step-pip${emberStep === 2 ? ' on' : ''}`}
                  onClick={() => setEmberStep(2)}
                >
                  <b>2</b>
                  <span>Schedule</span>
                </button>
              </nav>
            ) : (
              <div className="ember-modal-tabs" role="tablist" aria-label="Ember prompt views">
                <button
                  type="button"
                  role="tab"
                  aria-selected={emberTab === 'scheduled'}
                  className={emberTab === 'scheduled' ? 'active' : ''}
                  onClick={() => {
                    setEmberTab('scheduled');
                    setEmberHistoryDetailId(null);
                  }}
                >
                  Scheduled
                  {activeEmberCount > 0 && <span>{activeEmberCount}</span>}
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={emberTab === 'drafts'}
                  className={emberTab === 'drafts' ? 'active' : ''}
                  onClick={() => {
                    setEmberTab('drafts');
                    setEmberHistoryDetailId(null);
                  }}
                >
                  Drafts
                  {emberDraftCount > 0 && <span>{emberDraftCount}</span>}
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={emberTab === 'history'}
                  className={emberTab === 'history' ? 'active' : ''}
                  onClick={() => {
                    setEmberTab('history');
                    setEmberHistoryDetailId(null);
                  }}
                >
                  History
                  {emberHistoryCount > 0 && <span>{emberHistoryCount}</span>}
                </button>
              </div>
            )}
            <div className="ember-modal-head-actions">
              {emberEditing ? (
                <button type="button" className="ember-modal-back" onClick={resetEmberEditor}>
                  Back
                </button>
              ) : (
                <>
                  <button
                    type="button"
                    className="ember-modal-new"
                    onClick={() => openNewEmberEditor(emberTab === 'history' ? 'scheduled' : emberTab)}
                  >
                    New
                  </button>
                  <button type="button" className="ember-modal-close" onClick={() => setEmberModalOpen(false)} aria-label="Close Ember">
                    <svg className="ember-modal-close-icon" viewBox="0 0 24 24" aria-hidden="true">
                      <path d="M6 6L18 18M18 6L6 18" />
                    </svg>
                  </button>
                </>
              )}
            </div>
          </header>
          {emberEditing ? (
            <div className="ember-modal-edit-view">
              <div className="ember-editor-layout" data-step={emberStep}>
                <section className="ember-editor-left" aria-label="Ember prompt editor">
                  <div className="ember-editor-header">
                    <span>{emberEditorTarget?.kind === 'schedule' ? 'Edit Schedule' : emberEditorTarget?.kind === 'draft' ? 'Edit Draft' : 'New Prompt'}</span>
                    <b>{emberTargets.length} target{emberTargets.length === 1 ? '' : 's'}</b>
                  </div>
                  <div className="ember-editor-target-row">
                    <span>Targets</span>
                    <div className="ember-target-bar" aria-label="Target agents">
                      {emberTargetOptions.length === 0 ? (
                        <span className="ember-target-empty">No active agent</span>
                      ) : emberTargetOptions.map((target) => {
                        const selected = emberTargets.includes(target.id);
                        const meta = target.kind === 'agent' ? agentMeta?.[target.id] : null;
                        const avatarId = target.kind === 'human'
                          ? bbsUserIdentity?.avatarId ?? 'user-default'
                          : meta?.avatarId;
                        const avatarClass = target.kind === 'human'
                          ? avatarClassForId(avatarId, null)
                          : meta?.avatarClass ?? avatarClassForAgentFallback(null, target.id);
                        const avatarStyle = avatarImageStyleForId(avatarId);
                        return (
                          <button
                            key={target.id}
                            type="button"
                            className={`${selected ? 'selected' : ''} ${target.kind === 'human' ? 'human' : ''}`.trim()}
                            aria-pressed={selected}
                            onClick={() => {
                              setEmberTargets((current) => {
                                if (current.includes(target.id)) {
                                  const next = current.filter((candidate) => candidate !== target.id);
                                  return next.length > 0 ? next : current;
                                }
                                return [...current, target.id];
                              });
                            }}
                          >
                            <span className={`tavern-avatar-art ${avatarClass}`} style={avatarStyle} aria-hidden>
                              <span />
                              <i />
                              <b />
                            </span>
                            <span>{target.name}</span>
                          </button>
                        );
                      })}
                    </div>
                  </div>
                  <div className="ember-editor-composer">
                    <InputBar
                      ref={emberInputRef}
                      variant="embedded"
                      value={emberText}
                      onChange={setEmberText}
                      agentMeta={agentMeta}
                      mentionAgentIds={roomAgents}
                      placeholder="Write the prompt Ember should send later..."
                      onPasteImage={onPasteImage}
                      onMaterializeAttachments={onMaterializeAttachments}
                    />
                  </div>
                </section>
                <section className="ember-editor-schedule-col" aria-label="Schedule settings">
                  <EmberScheduleInstrument
                    sendMode={emberSendMode}
                    setSendMode={setEmberSendMode}
                    delayHours={emberDelayHours}
                    setDelayHours={setEmberDelayHours}
                    delayMinutes={emberDelayMinutes}
                    setDelayMinutes={setEmberDelayMinutes}
                    atDate={emberAtDate}
                    setAtDate={setEmberAtDate}
                    atTime={emberAtTime}
                    setAtTime={setEmberAtTime}
                    repeatEnabled={emberRepeatEnabled}
                    setRepeatEnabled={setEmberRepeatEnabled}
                    repeatKind={emberRepeatKind}
                    setRepeatKind={setEmberRepeatKind}
                    repDays={emberRepDays}
                    setRepDays={setEmberRepDays}
                    repHrs={emberRepHrs}
                    setRepHrs={setEmberRepHrs}
                    repMin={emberRepMin}
                    setRepMin={setEmberRepMin}
                    weekDays={emberWeekDays}
                    setWeekDays={setEmberWeekDays}
                    everyNWeeks={emberEveryNWeeks}
                    setEveryNWeeks={setEmberEveryNWeeks}
                    monthDays={emberMonthDays}
                    setMonthDays={setEmberMonthDays}
                    everyNMonths={emberEveryNMonths}
                    setEveryNMonths={setEmberEveryNMonths}
                    endMode={emberEndMode}
                    setEndMode={setEmberEndMode}
                    endAfterCount={emberEndAfterCount}
                    setEndAfterCount={setEmberEndAfterCount}
                    endDate={emberEndDate}
                    setEndDate={setEmberEndDate}
                    endTime={emberEndTime}
                    setEndTime={setEmberEndTime}
                  />
                </section>
              </div>
              <div className="ember-editor-actions" data-step={emberStep}>
                {emberStep === 1 ? (
                  <>
                    <span className="ember-actions-grow" />
                    {emberTargets.length === 0 && (
                      <span className="ember-target-hint">Select at least 1 agent</span>
                    )}
                    <button type="button" onClick={saveEmberDraft} disabled={!emberText.trim()}>
                      Save as Draft
                    </button>
                    <button type="button" className="primary" onClick={() => setEmberStep(2)}>
                      Continue to schedule →
                    </button>
                  </>
                ) : (
                  <>
                    <button type="button" className="ember-actions-back" onClick={() => setEmberStep(1)}>
                      ‹ Back
                    </button>
                    <span className="ember-actions-grow" />
                    {!emberText.trim() && (
                      <button type="button" className="ember-actions-warning" onClick={() => setEmberStep(1)}>
                        Add a prompt
                      </button>
                    )}
                    <button type="button" onClick={saveEmberDraft} disabled={!emberText.trim()}>
                      Save as Draft
                    </button>
                    <button type="button" className="primary" onClick={createScheduleFromComposer} disabled={!canScheduleEmber}>
                      Schedule
                    </button>
                  </>
                )}
              </div>
            </div>
          ) : (
            <div className="ember-modal-list-view">
              <div className="ember-modal-list-head">
                <span>
                  {emberHistoryDetail
                    ? 'History record'
                    : emberTab === 'scheduled'
                      ? 'Scheduled prompts'
                      : emberTab === 'drafts'
                        ? 'Draft prompts'
                        : 'History'}
                </span>
                <b>
                  {emberHistoryDetail
                    ? emberHistoryStatusLabel(emberHistoryDetail)
                    : emberTab === 'scheduled'
                      ? `${activeEmberCount} active`
                      : emberTab === 'drafts'
                        ? `${emberDraftCount} draft${emberDraftCount === 1 ? '' : 's'}`
                        : `${emberHistoryCount} record${emberHistoryCount === 1 ? '' : 's'}`}
                </b>
              </div>
              <div className="ember-modal-list-panel">
                {emberHistoryDetail ? (
                  <article className={`ember-modal-card history detail ${emberHistoryDetail.status}`}>
                    <div className="ember-card-main static">
                      <span className={`ember-card-kind history ${emberHistoryDetail.status}`}>
                        {emberHistoryKindLabel(emberHistoryDetail)}
                      </span>
                      <span className={`ember-card-status ${emberHistoryDetail.status}`}>
                        {formatBbsTime(emberHistoryDetail.sentAt)}
                      </span>
                      <b>{emberHistoryTargetLabel(emberHistoryDetail)}</b>
                      <small>Sent {emberTimeLabel(emberHistoryDetail.sentAt)}</small>
                      <p>{emberHistoryDetail.prompt}</p>
                      {emberHistoryDetail.error && <em>{emberHistoryDetail.error}</em>}
                    </div>
                    <div className="ember-modal-item-actions">
                      <button type="button" onClick={() => setEmberHistoryDetailId(null)}>
                        Back
                      </button>
                      <button type="button" className="danger" onClick={() => deleteEmberHistoryRecord(emberHistoryDetail)}>
                        Delete Record
                      </button>
                    </div>
                  </article>
                ) : emberTab === 'scheduled' ? (
                  activeEmberSchedules.length === 0 ? (
                    <div className="ember-modal-empty">
                      {emberRunBusy ? 'Sending prompt...' : 'No scheduled prompts.'}
                    </div>
                  ) : activeEmberSchedules.map((schedule) => {
                    const repeating = isRepeatingEmberSchedule(schedule);
                    const repeat = emberScheduleRepeatLabel(schedule);
                    return (
                      <article key={schedule.id} className={`ember-modal-card ${schedule.status} ${repeating ? 'repeat' : 'once'}`}>
                        <button type="button" className="ember-card-main" onClick={() => populateEmberScheduleEditor(schedule)}>
                          <span className={`ember-card-kind ${repeating ? 'repeat' : 'once'}`}>
                            {repeating ? 'Repeat schedule' : 'One-time schedule'}
                          </span>
                          <span className={`ember-card-status ${schedule.status}`}>
                            {emberCountdownLabel(schedule, emberNow)}
                          </span>
                          <b>{emberScheduleSummary(schedule)} · {emberScheduleTargetLabel(schedule)}</b>
                          <small>{emberScheduleAttribution(schedule)}</small>
                          {repeat && <small>{repeat}</small>}
                          <p>{compactEmberText(schedule.text)}</p>
                          {schedule.error && <em>{schedule.error}</em>}
                        </button>
                        <div className="ember-modal-item-actions">
                          {repeating ? (
                            <>
                              <button type="button" onClick={() => populateEmberScheduleEditor(schedule)}>
                                Edit
                              </button>
                              <button type="button" className="danger" onClick={() => deleteEmberSchedule(schedule)}>
                                Delete
                              </button>
                            </>
                          ) : (
                            <>
                              <button type="button" disabled={emberBusy === schedule.id} onClick={() => void runEmberSchedule(schedule, 'manual')}>
                                {emberBusy === schedule.id ? 'Sending...' : 'Run now'}
                              </button>
                              <button type="button" onClick={() => toggleEmberSchedule(schedule)}>
                                {schedule.status === 'paused' ? 'Resume' : 'Pause'}
                              </button>
                              <button type="button" onClick={() => populateEmberScheduleEditor(schedule)}>
                                Edit
                              </button>
                              <button type="button" className="danger" onClick={() => deleteEmberSchedule(schedule)}>
                                Delete
                              </button>
                            </>
                          )}
                        </div>
                      </article>
                    );
                  })
                ) : emberTab === 'drafts' ? emberDrafts.length === 0 ? (
                  <div className="ember-modal-empty">No drafts.</div>
                ) : emberDrafts.map((draft) => (
                  <article key={draft.id} className="ember-modal-card draft">
                    <button type="button" className="ember-card-main" onClick={() => populateEmberDraftEditor(draft)}>
                      <span className="ember-card-kind draft">Draft</span>
                      <span className="ember-card-status draft">{formatBbsTime(draft.updatedAt)}</span>
                      <b>Saved draft</b>
                      <p>{compactEmberText(draft.text)}</p>
                    </button>
                    <div className="ember-modal-item-actions">
                      <button type="button" onClick={() => populateEmberDraftEditor(draft)}>
                        Edit
                      </button>
                      <button type="button" className="danger" onClick={() => deleteEmberDraft(draft)}>
                        Delete
                      </button>
                    </div>
                  </article>
                )) : emberHistory.length === 0 ? (
                  <div className="ember-modal-empty">No history records.</div>
                ) : emberHistory.map((record) => (
                  <article key={record.id} className={`ember-modal-card history ${record.status}`}>
                    <button type="button" className="ember-card-main" onClick={() => setEmberHistoryDetailId(record.id)}>
                      <span className={`ember-card-kind history ${record.status}`}>
                        {emberHistoryKindLabel(record)}
                      </span>
                      <span className={`ember-card-status ${record.status}`}>
                        {formatBbsTime(record.sentAt)}
                      </span>
                      <b>{emberHistoryTargetLabel(record)}</b>
                      <small>Sent {emberTimeLabel(record.sentAt)}</small>
                      <p>{compactEmberText(record.prompt)}</p>
                      {record.error && <em>{record.error}</em>}
                    </button>
                    <div className="ember-modal-item-actions">
                      <button type="button" onClick={() => setEmberHistoryDetailId(record.id)}>
                        View
                      </button>
                      <button type="button" className="danger" onClick={() => deleteEmberHistoryRecord(record)}>
                        Delete Record
                      </button>
                    </div>
                  </article>
                ))}
              </div>
            </div>
          )}
        </section>
      </div>
    )}
    {dreamOverlay !== 'off' && (
      <div className={`ember-dream-overlay ${dreamOverlay}`} role="presentation">
        <div className="ember-dream-drag-handle" data-tauri-drag-region aria-hidden="true">
          <div className="ember-dream-drag-groove" />
        </div>
        <div className="ember-dream-stage" aria-hidden>
          {dreamVideoSrc && (
            <video
              key={`${dreamVideoSet.id}-${dreamOverlay}`}
              className="ember-dream-video"
              src={dreamVideoSrc}
              autoPlay
              muted
              loop
              playsInline
              preload="auto"
            />
          )}
        </div>
        {dreamOverlay === 'wrapping' ? (
          <div className="ember-dream-state ember-dream-wrapping">
            <span>Good Night</span>
            <div className="ember-dreaming-label">{dreamWaitingLabel}</div>
          </div>
        ) : dreamOverlay === 'countdown' ? (
          <div className="ember-dream-state ember-dream-countdown">
            <span>Start dreaming in</span>
            <b>{dreamCountdownLabel}</b>
            {dreamCountdownInfoLabel && (
              <div className="ember-dreaming-label">{dreamCountdownInfoLabel}</div>
            )}
            <div className="ember-dream-action-row">
              <button
                type="button"
                className="ember-dream-inline-action"
                aria-pressed={dreamCountdownPaused}
                onClick={toggleDreamCountdownPaused}
              >
                {dreamCountdownPaused ? 'Resume' : 'Pause'}
              </button>
              <button type="button" className="ember-dream-inline-action" onClick={() => void runDreamNow({ skipCountdown: true })}>
                Skip
              </button>
            </div>
          </div>
        ) : dreamOverlay === 'dreaming' ? (
          <div className="ember-dream-state ember-dreaming-state">
            <span>Dreaming</span>
            <div className="ember-dreaming-label">{dreamStatusLabel}</div>
            <small>{dreamElapsedLabel}</small>
            {dreamVigilReady && (
              <div className="ember-dream-action-row">
                <button type="button" className="ember-dream-inline-action primary" onClick={() => setDreamOverlay('finished')}>
                  VIGIL
                </button>
              </div>
            )}
          </div>
        ) : (
          <div className="ember-dream-state ember-dreaming-state finished">
            <span>Had Dreams</span>
            <div className="ember-dreaming-label">{dreamFinishedLabel}</div>
          </div>
        )}
      </div>
    )}
    {bbsOpen && (
      <div className="bbs-modal-shade" role="presentation">
        <section className="bbs-modal" role="dialog" aria-modal="true" aria-label="Bulletin Board">
          <header className="bbs-modal-head">
            <div className="bbs-modal-title">
              {bbsView !== 'list' && (
                <button type="button" className="bbs-back" onClick={closeBbsDetail}>
                  ‹ Threads
                </button>
              )}
              <b>Bulletin Board</b>
              <span>
                {bbsView === 'compose'
                  ? 'New Post'
                  : bbs?.projectDisplayName ?? workspace?.repoFullName ?? 'Project'}
              </span>
            </div>
            <div className="bbs-modal-actions">
              {bbsView === 'list' && (
                <>
                  <button type="button" className="primary" onClick={openBbsCompose}>
                    + Post
                  </button>
                  <button type="button" className="ghost" onClick={() => void refreshBbs()}>
                    Refresh
                  </button>
                </>
              )}
              <button
                type="button"
                className="close"
                aria-label="Close Bulletin Board"
                onClick={() => {
                  setBbsOpen(false);
                  setBbsView('list');
                  setBbsDetailThreadId(null);
                }}
              >
                <svg className="ember-modal-close-icon" viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M6 6L18 18M18 6L6 18" />
                </svg>
              </button>
            </div>
          </header>

          {bbsView === 'list' && (
            <div className="bbs-filters">
              <div className="bbs-tabs">
                <button
                  type="button"
                  className={bbsFilter === 'all' ? 'active' : ''}
                  onClick={() => setBbsFilter('all')}
                >
                  All Threads
                </button>
                <button
                  type="button"
                  className={bbsFilter === 'tagged' ? 'active' : ''}
                  onClick={() => setBbsFilter('tagged')}
                >
                  For {bbs?.projectDisplayName ?? 'This Project'}
                </button>
              </div>
            </div>
          )}
          {bbsError && <div className="bbs-error">{bbsError}</div>}
          {bbsDeleteTarget && (
            <div className="kota-confirm-layer" role="dialog" aria-modal="true" aria-label="Delete BBS item">
              <div className="kota-confirm-card danger">
                <h2>{bbsDeleteTarget.kind === 'topic' ? 'Delete Thread' : 'Delete Reply'}</h2>
                <pre>{bbsDeleteTarget.kind === 'topic'
                  ? 'Delete this thread?\n\nAll replies under it are deleted too.'
                  : 'Delete this reply?'}</pre>
                <div className="kota-confirm-actions">
                  <button type="button" onClick={() => setBbsDeleteTarget(null)}>
                    Cancel
                  </button>
                  <button
                    type="button"
                    className="confirm"
                    disabled={bbsBusy === bbsDeleteTarget.postId}
                    onClick={() => {
                      const target = bbsDeleteTarget;
                      setBbsDeleteTarget(null);
                      if (target) void deleteBbsPost(target);
                    }}
                  >
                    Delete
                  </button>
                </div>
              </div>
            </div>
          )}

          {bbsView === 'list' && (
            <div className="bbs-thread-list">
              {visibleBbsThreads.length === 0 ? (
                <div className="bbs-empty">No BBS threads in this filter.</div>
              ) : visibleBbsThreads.map((thread) => {
                const topic = thread.posts.find((post) => post.kind === 'topic') ?? thread.posts[0];
                if (!topic) return null;
                const replyCount = thread.posts.length - 1;
                const isHuman = topic.agentId === 'human';
                const rowMeta = agentMeta?.[topic.agentId as AgentId];
                const rowAvatarId = topic.agentAvatar ?? rowMeta?.avatarId ?? (isHuman ? 'user-default' : null);
                const rowClass = rowAvatarId
                  ? avatarClassForId(rowAvatarId, null)
                  : rowMeta?.avatarClass ?? avatarClassForAgentFallback(null, topic.agentId);
                return (
                  <button
                    key={thread.threadId}
                    type="button"
                    className={`bbs-thread-row ${thread.isNew ? 'new' : ''}`}
                    onClick={() => openBbsDetail(thread)}
                  >
                    <span className={`bbs-new-dot ${thread.isNew ? '' : 'off'}`} aria-hidden />
                    <span
                      className={`bbs-msg-avatar sm tavern-avatar-art ${rowClass}`}
                      style={avatarImageStyleForId(rowAvatarId)}
                      aria-hidden
                    >
                      <span />
                      <i />
                      <b />
                    </span>
                    <span className={`bbs-msg-author ${isHuman ? 'human' : ''}`}>{topic.agentDisplayName}</span>
                    <span className="bbs-time">{formatBbsTime(topic.createdAt)}</span>
                    <span className="bbs-row-count">
                      {replyCount} {replyCount === 1 ? 'Reply' : 'Replies'}
                    </span>
                    <span className="bbs-row-preview">{topic.preview}</span>
                    {topic.body.length > 180 && <span className="bbs-row-more">See more</span>}
                  </button>
                );
              })}
            </div>
          )}

          {bbsView === 'detail' && bbsDetailThread && (() => {
            const topic = bbsDetailThread.posts.find((post) => post.kind === 'topic')
              ?? bbsDetailThread.posts[0];
            if (!topic) return null;
            const replies = bbsDetailThread.posts.filter((post) => post.postId !== topic.postId);
            return (
              <div className="bbs-detail">
                <div className="bbs-detail-scroll" ref={bbsDetailScrollRef}>
                  {renderBbsFloor(topic, 1, true, bbsDetailThread)}
                  <div className="bbs-replies-divider">
                    {replies.length} {replies.length === 1 ? 'reply' : 'replies'}
                  </div>
                  {replies.map((post, index) => renderBbsFloor(post, index + 2, false, bbsDetailThread))}
                </div>
                <div className="bbs-reply-box">
                  <div className="bbs-reply-shell">
                    <div className="bbs-reply-editor">
                      <InputBar
                        ref={bbsReplyInputRef}
                        variant="embedded"
                        value={bbsReplyText}
                        onChange={setBbsReplyText}
                        agentMeta={agentMeta}
                        mentionAgentIds={roomAgents}
                        placeholder="Reply…"
                        onPasteImage={onPasteImage}
                        onMaterializeAttachments={onMaterializeAttachments}
                      />
                    </div>
                    {renderBbsAgentBar()}
                    <div className="bbs-reply-toolbar">
                      <button
                        type="button"
                        className={`bbs-agent-bar-toggle ${bbsReplyAgentBarOpen ? 'on' : ''}`}
                        onClick={() => setBbsReplyAgentBarOpen((open) => !open)}
                      >
                        @ Agent{bbsReplyAgents.length > 0 ? ` · ${bbsReplyAgents.length}` : ''}
                      </button>
                      <button
                        type="button"
                        className="bbs-reply-send"
                        disabled={bbsReplyBusy || !bbsReplyText.trim()}
                        onClick={() => void sendBbsReply()}
                      >
                        {bbsReplyBusy ? 'Sending…' : 'Reply'}
                      </button>
                    </div>
                  </div>
                </div>
              </div>
            );
          })()}

          {bbsView === 'compose' && (
            <div className="bbs-detail">
              <div className="bbs-compose-targets">
                <span className="bbs-tag-label">To</span>
                {bbsTargetProjects.length === 0 ? (
                  <span className="bbs-target-empty">No other open projects.</span>
                ) : bbsTargetProjects.map((project) => {
                  const selected = bbsComposeProjects.has(project.projectId);
                  return (
                    <button
                      key={project.projectId}
                      type="button"
                      className={`bbs-agent-chip ${selected ? 'sel' : ''}`}
                      aria-pressed={selected}
                      onClick={() => {
                        setBbsComposeProjects((current) => {
                          const next = new Set(current);
                          if (next.has(project.projectId)) next.delete(project.projectId);
                          else next.add(project.projectId);
                          return next;
                        });
                      }}
                    >
                      <span>{project.displayName}</span>
                    </button>
                  );
                })}
              </div>
              <div className="bbs-reply-box grow">
                <div className="bbs-reply-shell grow">
                  <div className="bbs-reply-editor grow">
                    <InputBar
                      ref={bbsComposeInputRef}
                      variant="embedded"
                      value={bbsComposeText}
                      onChange={setBbsComposeText}
                      agentMeta={agentMeta}
                      mentionAgentIds={roomAgents}
                      placeholder={`Write a new post as ${bbsUserIdentity?.name?.trim() || 'Human'}…`}
                      onPasteImage={onPasteImage}
                      onMaterializeAttachments={onMaterializeAttachments}
                    />
                  </div>
                  {renderBbsAgentBar()}
                  <div className="bbs-reply-toolbar">
                    <button
                      type="button"
                      className={`bbs-agent-bar-toggle ${bbsReplyAgentBarOpen ? 'on' : ''}`}
                      onClick={() => setBbsReplyAgentBarOpen((open) => !open)}
                    >
                      @ Agent{bbsReplyAgents.length > 0 ? ` · ${bbsReplyAgents.length}` : ''}
                    </button>
                    <button
                      type="button"
                      className="bbs-reply-send"
                      disabled={bbsReplyBusy || !bbsComposeText.trim()}
                      onClick={() => void submitBbsPost()}
                    >
                      {bbsReplyBusy ? 'Posting…' : 'Post'}
                    </button>
                  </div>
                </div>
              </div>
            </div>
          )}
        </section>
      </div>
    )}
    {lmHistoryOpen && (
      <div className="violet-summary-modal-shade" role="presentation">
        <section className="violet-summary-modal lm-modal" role="dialog" aria-modal="true" aria-label="Laughing Man history">
          <header className="violet-summary-modal-head lm-modal-head">
            <b>Laughing Man · {lm?.botUsername ?? 'Telegram'}</b>
            <button type="button" onClick={() => setLmHistoryOpen(false)} aria-label="Close">
              <svg className="ember-modal-close-icon" viewBox="0 0 24 24" aria-hidden="true">
                <path d="M6 6L18 18M18 6L6 18" />
              </svg>
            </button>
          </header>
          <div className="lm-tabs">
            <button
              type="button"
              className={lmLogFilter === 'all' ? 'active' : ''}
              onClick={() => setLmLogFilter('all')}
            >
              All Messages
            </button>
            <button
              type="button"
              className={lmLogFilter === 'project' ? 'active' : ''}
              onClick={() => setLmLogFilter('project')}
            >
              For {bbs?.projectDisplayName ?? 'This Project'}
            </button>
            <button
              type="button"
              className={`lm-queue-pill ${lmQueueOnly ? 'active' : ''}`}
              disabled={!lmQueueOnly && lmVisibleQueue.length === 0}
              onClick={() => setLmQueueOnly((value) => !value)}
            >
              Queued {lmVisibleQueue.length}
            </button>
          </div>
          {lmQueueOnly ? (
            <div className="lm-msg-list">
              {lmQueueError && <div className="bbs-error">{lmQueueError}</div>}
              {lmVisibleQueue.length === 0 ? (
                <div className="bbs-empty">No queued messages in this filter.</div>
              ) : lmVisibleQueue.map((entry) => {
                const queueBusy = lmQueueBusy?.endsWith(`:${entry.id}`) ?? false;
                const sending = lmQueueBusy === `send:${entry.id}`;
                const deleting = lmQueueBusy === `delete:${entry.id}`;
                return (
                  <div key={entry.id} className="lm-msg-row lm-queue-row">
                    <span className="lm-msg-dir queue">•</span>
                    <div className="lm-msg-main">
                      <div className="lm-msg-head">
                        <span className="lm-msg-who">{entry.agentName ?? 'No agent'}</span>
                        <span className="lm-msg-proj">{entry.projectName ?? 'No project'}</span>
                        <span className="lm-msg-time">{formatLmQueueTime(entry.receivedAt)}</span>
                      </div>
                      <div className="lm-msg-preview">{entry.preview || '(attachment)'}</div>
                      <div className="lm-queue-foot">
                        <span className={entry.deliveryError ? 'lm-queue-error' : ''}>
                          {entry.deliveryError || 'Standby message'}
                        </span>
                        <div className="lm-queue-actions">
                          <button
                            type="button"
                            disabled={queueBusy}
                            onClick={() => void sendLmQueued(entry.id)}
                          >
                            {sending ? 'Sending…' : 'Send'}
                          </button>
                          <button
                            type="button"
                            className="delete"
                            disabled={queueBusy}
                            onClick={() => void deleteLmQueued(entry.id)}
                          >
                            {deleting ? 'Deleting…' : 'Delete'}
                          </button>
                        </div>
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="lm-msg-list">
              {lmVisibleLog.length === 0 ? (
                <div className="bbs-empty">No messages in this filter.</div>
              ) : lmVisibleLog.map((entry) => (
                <div key={entry.id} className="lm-msg-row">
                  <span className={`lm-msg-dir ${entry.direction}`}>{entry.direction === 'out' ? '←' : '→'}</span>
                  <div className="lm-msg-main">
                    <div className="lm-msg-head">
                      <span className="lm-msg-who">{entry.agentName}</span>
                      <span className="lm-msg-proj">{entry.projectName}</span>
                      {(entry.mediaCount ?? 0) > 0 && <span className="lm-msg-media">[图片 ×{entry.mediaCount}]</span>}
                      {entry.offlineRecordedAt && (
                        <span className="lm-msg-offline">offline message recorded at {formatLmQueueTime(entry.offlineRecordedAt)}</span>
                      )}
                      <span className="lm-msg-time">{formatBbsTime(entry.ts)}</span>
                    </div>
                    <div className="lm-msg-preview">{entry.preview}</div>
                  </div>
                </div>
              ))}
            </div>
          )}
          <div className="lm-privacy-line">⚠ Messages travel through the Telegram cloud · owner-only · read-only panel</div>
        </section>
      </div>
    )}
    {lmSetupOpen && (
      <div className="violet-summary-modal-shade" role="presentation">
        <section className="violet-summary-modal lm-modal" role="dialog" aria-modal="true" aria-label="Laughing Man settings">
          <header className="violet-summary-modal-head lm-modal-head">
            <b>Laughing Man · Telegram Setup</b>
            <button type="button" onClick={() => setLmSetupOpen(false)} aria-label="Close">
              <svg className="ember-modal-close-icon" viewBox="0 0 24 24" aria-hidden="true">
                <path d="M6 6L18 18M18 6L6 18" />
              </svg>
            </button>
          </header>
          <LaughingManSettings onChanged={refreshLm} />
        </section>
      </div>
    )}
    {violetHistoryOpen && (
      <div className="violet-summary-modal-shade" role="presentation">
        <section className="violet-summary-modal" role="dialog" aria-modal="true" aria-label="Violet summary history">
          <header className="violet-summary-modal-head">
            <b>Violet Summary History</b>
            <button type="button" onClick={() => setVioletHistoryOpen(false)}>x</button>
          </header>
          <div className="violet-summary-history-list">
            {violetSummary?.history.length
              ? violetSummary.history.map(renderVioletHistoryEntry)
              : <div className="violet-summary-empty">No summaries yet.</div>}
          </div>
        </section>
      </div>
    )}
    </>
  );
}
