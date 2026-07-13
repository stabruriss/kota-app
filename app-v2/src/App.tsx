import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { TopBar } from './chrome/TopBar';
import { Stage } from './chrome/Stage';
import {
  DESK_SWATCHES,
  ROOM_SWATCHES,
  type DeskTheme,
  type RoomTheme,
} from './chrome/ColorPicker';
import { RightColumn, type DreamProjectTarget } from './chrome/RightColumn';
import { InputBar, type ComposerAttachment, type InputBarHandle } from './chrome/InputBar';
import type { Centerpiece } from './chrome/Hearth';
import { HotMemoryPopup } from './popups/HotMemoryPopup';
import { RowPopup } from './popups/RowPopup';
import { FileTree } from './chrome/FileTree';
import { SmartTerminal } from './chrome/SmartTerminal';
import { AgentWindowsLayer, type AgentWindowsLayerHandle } from './chrome/AgentWindowsLayer';
import { BroadcastTargetPopover } from './chrome/BroadcastTargetPopover';
import { ShortcutRecruitModal } from './chrome/ShortcutRecruitModal';
import {
  IncarnationProgressBar,
  normalizeIncarnationProgressStep,
  type IncarnationProgressPhase,
  type IncarnationProgressStepId,
  type IncarnationProgressView,
} from './chrome/IncarnationProgressBar';
import { ProjectAgentProfileOverlay } from './chrome/ProjectAgentProfileOverlay';
import {
  TavernModal,
  TavernLoadingLog,
  TAVERN_HERO_CREDIT_CHANGED_EVENT,
  TAVERN_PROFILE_CHANGED_EVENT,
  loadTavernHeroIncarnationProfile,
  loadTavernWorkingHeroes,
  prepareTavernForOpen,
  preloadTavernAssets,
  syncTavernHeroStorageFromDisk,
  type TavernLoadingLogItem,
  type TavernTab,
  type TavernHeroIncarnationProfile,
} from './chrome/TavernModal';
import { ProjectSetupModal } from './chrome/ProjectSetupModal';
import { Hearth } from './chrome/Hearth';
import { useAgentRuntime } from './hooks/useAgentRuntime';
import type { AgentCli, AgentSpawnRequest, AgentSummary } from './types/agent-pty';
import {
  GITHUB_CLI_INSTALL_URL,
  GITHUB_CLI_LOGIN_COMMAND,
  archiveProjectAgent,
  archiveWorkspaceProject,
  callBackProjectAgent,
  clearProjectAgentSessionMetadata,
  commendProjectAgent,
  dismissProjectAgent,
  emberSchedulerTick,
  ghAuthStatus,
  hasTauriRuntime,
  incarnateTavernHero,
  isAgentSessionLeaseConflictError,
  inspectWorkspaceProject,
  kageBunshinProjectAgent,
  listWorkspaceProjects,
  listProjectAgentIdentities,
  loadProjectAgentLayoutFile,
  saveProjectAgentLayoutFile,
  listAgentPtys,
  listArchivedProjectAgents,
  loadProjectAgentDetail,
  lmStatus,
  lmUpdateWorkingAgents,
  materializeComposerAttachmentPath,
  openWorkspaceProject,
  onVioletRoomSynced,
  onIncarnationProgressEvent,
  resolveProjectAgentLaunch,
  resolveWorkspaceAgentLaunch,
  resolveDevProjectRoot,
  saveComposerClipboardImage,
  startFreshProjectAgentSession,
  loadWhiteboardCanvas,
  renameWhiteboardCanvasPage,
  saveWhiteboardCanvas,
  saveWhiteboardCanvasSnapshot,
  setVioletPrivacy,
  supportedShellsStatus,
  terminalEnhancementStatus,
  workspaceStatus,
  type GhAuthInfo,
  type LmSelected,
  type ProjectAgentCommendSource,
  type ProjectAgentDetail,
  type ProjectAgentIdentity,
  type ProjectAgentRecord,
  type SupportedShellStatus,
  type TavernHeroProfileDraft,
  type VioletChatMessage,
  type WorkspaceProject,
} from './pty-client';
import type { SceneKey } from './mock/fixtures';
import { AGENTS, DEFAULT_PROJECT_ID, SEAT_POSITIONS } from './mock/fixtures';
import type { Agent, AgentId, LogRow } from './types/scene';
import type { Project, ProjectId } from './types/project';
import type { WorkingHero } from './types/agentbar';
import {
  composeProjectAgentName,
  projectSurnameLabel,
  projectAgentNameFields,
  splitProjectAgentName,
} from './chrome/ProjectAgentName';
import { avatarClassForId, refreshUserHeroAvatars } from './lib/hero-avatars';
import { emitVioletComposerSent } from './chrome/violet-room-events';
import { formatAgentPromptInput, normalizeAgentPromptPayload } from './lib/agent-prompt';
import { mintProjectAgentId } from './lib/project-agent-ids';
import { agentSlotIndexFromKey, MAX_AGENT_SLOTS } from './lib/agent-slots';
import {
  coordinateExistingAgentLaunch,
  type ExistingAgentLaunchResult,
} from './lib/existing-agent-launch';
import {
  connectVioletProjectSyncEngine,
  requestVioletProjectAgentSync,
  requestVioletProjectPromptSync,
  type VioletProjectSyncHandle,
} from './lib/violet-sync-engine';

const PRIVATE_CHAT_UI_ENABLED = false;
const KAGE_BUNSHIN_UI_ENABLED = false;
const EMPTY_AGENT_SET: ReadonlySet<AgentId> = new Set();

const WhiteboardPanel = lazy(() =>
  import('./popups/WhiteboardPanel').then((module) => ({ default: module.WhiteboardPanel })),
);

type Popup =
  | { kind: 'hotmem' }
  | { kind: 'whiteboard' }
  | { kind: 'row'; row: LogRow }
  | null;

type AgentContextMenuState = {
  agentId: AgentId;
  x: number;
  y: number;
  source: ProjectAgentCommendSource;
} | null;

type ConfirmDialogState = {
  title: string;
  body: string;
  confirmLabel: string;
  cancelLabel: string;
  tone?: 'danger';
  plainCopy?: boolean;
  confirmOnEnter?: boolean;
  resolve: (confirmed: boolean) => void;
} | null;

const LS_CENTER = 'kota-v2.centerpiece';
const LS_ROOM   = 'kota-v2.room-color';
const LS_DESK   = 'kota-v2.desk-color';
const LS_ROOM_THEME = 'kota-v2.room-theme';
const LS_DESK_THEME = 'kota-v2.desk-theme';
const LS_PROJECT_APPEARANCE = 'kota-v2.project-appearance';
const LS_DEV_PROJECT_ROOT = 'kota-v2.dev.project-root';
const LS_VIOLET_UNREAD = 'kota-v2.violet-unread.v1';
const LS_AGENT_LAYOUT = 'kota-v2.agent-layout.v1';
const AGENT_LAYOUT_WRITE_DEBOUNCE_MS = 800;
// Legacy localStorage layouts predate the clockwise slot order (layout file
// v2); remap them so agents keep their physical seats during migration.
const LEGACY_SEAT_ORDER_TO_CLOCKWISE = [6, 0, 1, 2, 7, 5, 4, 3] as const;

function remapLegacySeatOrder(
  slots: readonly (AgentId | null)[] | null,
): readonly (AgentId | null)[] | null {
  if (!slots) return null;
  const normalized = normalizeAgentTableSlots(slots);
  return LEGACY_SEAT_ORDER_TO_CLOCKWISE.map((index) => normalized[index] ?? null);
}
const LS_WORKSPACE_TAB_ORDER = 'kota-v2.workspace-tab-order.v1';
const BROWSER_DEV_PROJECT_ROOT = '/tmp/kota-dev';

const DEFAULT_CENTER: Centerpiece = 'fire';
const DEFAULT_ROOM = '#2C2720';
const DEFAULT_DESK = '#2A2119';
const DEFAULT_ROOM_THEME: RoomTheme = 'classic';
const DEFAULT_DESK_THEME: DeskTheme = 'warm';

const DEFAULT_TABLE_SLOTS: (AgentId | null)[] = SEAT_POSITIONS.slice(0, MAX_AGENT_SLOTS).map(() => null);
const DEFAULT_OFF_TABLE_AGENTS: AgentId[] = [];

const ROMAN_SUFFIXES: Record<number, string> = {
  2: 'II',
  3: 'III',
  4: 'IV',
  5: 'V',
  6: 'VI',
  7: 'VII',
  8: 'VIII',
  9: 'IX',
  10: 'X',
};

interface IncarnationLaunchProfile {
  templateId: string;
  displayName: string;
  profile: TavernHeroProfileDraft;
}

interface RecruitProgressOptions {
  progressId?: string | null;
  suppressFailureAlert?: boolean;
  onStep?: (stepId: IncarnationProgressStepId, message: string) => void;
  onError?: (err: unknown) => void;
}

interface AgentLayoutState {
  tableSlots: (AgentId | null)[];
  offTableAgents: AgentId[];
}

interface AgentHydrationProgress {
  projectId: ProjectId;
  completed: number;
  total: number;
}

interface RoomUiSnapshot {
  minimized: AgentId[];
  terminalFocusedAgent: AgentId | null;
  composerTarget: AgentId | null;
  composerBroadcast: boolean;
  broadcastPopupOpen: boolean;
  broadcastRecipients: AgentId[];
  privateAgents: AgentId[];
  agentLayout: AgentLayoutState;
  groupChatOpen: boolean;
  chatFilterActive: boolean;
}

interface VioletUnreadProjectState {
  lastReadAt?: string | null;
  unreadIds: string[];
  unreadAgentIds: AgentId[];
}

type VioletUnreadState = Record<string, VioletUnreadProjectState>;

function loadVioletUnreadState(): VioletUnreadState {
  if (typeof window === 'undefined') return {};
  try {
    const raw = window.localStorage.getItem(LS_VIOLET_UNREAD);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, Partial<VioletUnreadProjectState>>;
    if (!parsed || typeof parsed !== 'object') return {};
    const next: VioletUnreadState = {};
    for (const [key, value] of Object.entries(parsed)) {
      if (!value || typeof value !== 'object') continue;
      next[key] = {
        lastReadAt: typeof value.lastReadAt === 'string' ? value.lastReadAt : null,
        unreadIds: Array.isArray(value.unreadIds)
          ? value.unreadIds.filter((id): id is string => typeof id === 'string')
          : [],
        unreadAgentIds: Array.isArray(value.unreadAgentIds)
          ? value.unreadAgentIds.filter((id): id is AgentId => typeof id === 'string')
          : [],
      };
    }
    return next;
  } catch {
    return {};
  }
}

function persistVioletUnreadState(state: VioletUnreadState): void {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(LS_VIOLET_UNREAD, JSON.stringify(state));
  } catch {
    // Best-effort UI state; failure should not block the app.
  }
}

function violetProjectUnreadKey(projectRoot?: string | null): string {
  return projectRoot?.trim() || '__default__';
}

function unreadTotal(state: VioletUnreadState): number {
  return Object.values(state).reduce((sum, project) => sum + project.unreadIds.length, 0);
}

function timestampMs(value?: string | null): number {
  const parsed = Date.parse(value ?? '');
  return Number.isFinite(parsed) ? parsed : 0;
}

function latestVioletMessageTimestamp(messages: readonly VioletChatMessage[]): string | null {
  let latest: string | null = null;
  let latestMs = 0;
  for (const message of messages) {
    const ms = timestampMs(message.timestamp);
    if (ms >= latestMs) {
      latest = message.timestamp;
      latestMs = ms;
    }
  }
  return latest;
}

function isUnreadVioletMessage(message: VioletChatMessage): boolean {
  return message.role === 'assistant' && message.kind === 'message' && message.agentId !== 'user';
}

function unreadAgentIdsForMessages(
  unreadIds: ReadonlySet<string>,
  messages: readonly VioletChatMessage[],
): AgentId[] {
  const agentIds = new Set<AgentId>();
  for (const message of messages) {
    if (!unreadIds.has(message.id)) continue;
    if (!isUnreadVioletMessage(message)) continue;
    agentIds.add(message.agentId as AgentId);
  }
  return Array.from(agentIds).sort();
}

function reduceVioletUnreadOnSync(
  previous: VioletUnreadState,
  projectKey: string,
  messages: readonly VioletChatMessage[],
  markRead: boolean,
): VioletUnreadState {
  const existing = previous[projectKey];
  const latestAt = latestVioletMessageTimestamp(messages) ?? new Date().toISOString();
  if (!existing || markRead) {
    const nextProject: VioletUnreadProjectState = { lastReadAt: latestAt, unreadIds: [], unreadAgentIds: [] };
    if (
      existing &&
      existing.unreadIds.length === 0 &&
      existing.unreadAgentIds.length === 0 &&
      existing.lastReadAt === nextProject.lastReadAt
    ) {
      return previous;
    }
    return { ...previous, [projectKey]: nextProject };
  }

  const lastReadMs = timestampMs(existing.lastReadAt);
  const unreadIds = new Set(existing.unreadIds);
  for (const message of messages) {
    if (!isUnreadVioletMessage(message)) continue;
    if (timestampMs(message.timestamp) <= lastReadMs) continue;
    unreadIds.add(message.id);
  }
  const nextUnreadIds = Array.from(unreadIds);
  const nextUnreadAgentIds = Array.from(new Set([
    ...existing.unreadAgentIds,
    ...unreadAgentIdsForMessages(unreadIds, messages),
  ])).sort();
  if (
    nextUnreadIds.length === existing.unreadIds.length &&
    nextUnreadIds.every((id, index) => id === existing.unreadIds[index]) &&
    nextUnreadAgentIds.length === existing.unreadAgentIds.length &&
    nextUnreadAgentIds.every((id, index) => id === existing.unreadAgentIds[index])
  ) {
    return previous;
  }
  return {
    ...previous,
    [projectKey]: { ...existing, unreadIds: nextUnreadIds, unreadAgentIds: nextUnreadAgentIds },
  };
}

function markVioletProjectRead(
  previous: VioletUnreadState,
  projectKey: string,
  readAt = new Date().toISOString(),
): VioletUnreadState {
  const existing = previous[projectKey];
  if (
    existing &&
    existing.unreadIds.length === 0 &&
    existing.unreadAgentIds.length === 0 &&
    timestampMs(existing.lastReadAt) >= timestampMs(readAt)
  ) {
    return previous;
  }
  return { ...previous, [projectKey]: { lastReadAt: readAt, unreadIds: [], unreadAgentIds: [] } };
}

function notifyRecruitFailure(agentId: AgentId, err: unknown) {
  const message = `Recruit ${agentId} failed: ${err}`;
  if (typeof window !== 'undefined' && typeof window.alert === 'function') {
    window.alert(message);
  }
}

function notifyExistingAgentLaunchFailure(agentId: AgentId, err: unknown) {
  const summary = err instanceof Error && err.message.trim()
    ? err.message
    : typeof err === 'string' && err.trim()
      ? err
      : 'Unknown launch error.';
  if (typeof window !== 'undefined' && typeof window.alert === 'function') {
    window.alert(`Open ${agentId} failed: ${summary}`);
  }
}

function recruitErrorMessage(agentId: AgentId, err: unknown): string {
  if (err instanceof Error && err.message.trim()) {
    return `Recruit ${agentId} failed: ${err.message}`;
  }
  if (typeof err === 'string' && err.trim()) {
    return `Recruit ${agentId} failed: ${err}`;
  }
  return `Recruit ${agentId} failed.`;
}

function makeIncarnationProgressId(): string {
  const randomUUID = globalThis.crypto?.randomUUID;
  if (typeof randomUUID === 'function') return randomUUID.call(globalThis.crypto);
  return `incarnation-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

function incarnationName(baseName: string, index: number, projectName?: string | null): string {
  const fields = projectAgentNameFields(baseName);
  const suffix = index <= 1 ? '' : ` ${ROMAN_SUFFIXES[index] ?? index}`;
  const project = projectSurnameLabel(projectName);
  return composeProjectAgentName({
    ...fields,
    given: `${fields.given || baseName.trim() || 'Agent'}${suffix}`,
    surname: project ? `v. ${project}` : '',
  });
}

function profileDraftForIncarnation(
  templateId: string,
  profile: TavernHeroIncarnationProfile,
): TavernHeroProfileDraft {
  return {
    heroId: templateId,
    name: profile.name,
    provider: profile.provider,
    model: profile.model,
    effort: profile.effort ?? null,
    avatarId: profile.avatarId,
    skills: [...profile.skills],
    ghost: profile.ghost,
    shell: profile.shell,
    archived: false,
    dismissed: false,
    kind: profile.kind,
  };
}

function shortProjectAgentName(name: string): string {
  const parts = splitProjectAgentName(name);
  return parts.base || name;
}

function projectAgentDetailToWorkingHero(detail: ProjectAgentDetail): WorkingHero {
  return {
    id: detail.agentId,
    templateId: detail.sourceHeroId || detail.agentId,
    cli: detail.cli,
    name: detail.displayName,
    record: detail.effort ? `${detail.model} / ${detail.effort}` : detail.model,
    avatarId: detail.avatarId,
    avatarClass: avatarClassForId(detail.avatarId, detail.provider || detail.cli),
  };
}

function projectAgentIdentityFromDetail(
  detail: ProjectAgentDetail,
  status = detail.status,
): ProjectAgentIdentity {
  return {
    agentId: detail.agentId,
    displayName: detail.displayName,
    sourceHeroId: detail.sourceHeroId,
    status,
    provider: detail.provider || detail.cli,
    avatarId: detail.avatarId ?? null,
  };
}

function upsertProjectAgentIdentity(
  identities: readonly ProjectAgentIdentity[],
  nextIdentity: ProjectAgentIdentity,
): ProjectAgentIdentity[] {
  const next = identities.filter((identity) => identity.agentId !== nextIdentity.agentId);
  next.push(nextIdentity);
  next.sort((a, b) => a.displayName.localeCompare(b.displayName));
  return next;
}

function projectAgentLifecycleStatus(status: string | null | undefined): Agent['lifecycleStatus'] | undefined {
  const normalized = (status ?? '').trim().toLowerCase();
  if (normalized === 'archived') return 'archived';
  if (normalized === 'dismissed' || normalized === 'removed' || normalized === 'deleted' || normalized === 'left') return 'left';
  return undefined;
}

function displayableProjectAgentStatus(status: string): boolean {
  const normalized = status.trim().toLowerCase();
  return (
    normalized !== 'archived' &&
    normalized !== 'dismissed' &&
    normalized !== 'removed' &&
    normalized !== 'deleted' &&
    normalized !== 'left'
  );
}

function orderedProjectAgentIds(
  workspaceAgents: readonly AgentSpawnRequest[],
  identities: readonly ProjectAgentIdentity[],
): AgentId[] {
  const seen = new Set<string>();
  const out: AgentId[] = [];
  const push = (id: string) => {
    if (!id || seen.has(id)) return;
    seen.add(id);
    out.push(id as AgentId);
  };
  for (const identity of identities) push(identity.agentId);
  for (const agent of workspaceAgents) push(agent.agentId);
  return out;
}

function tableLayoutForProjectAgents(agentIds: readonly AgentId[]): AgentLayoutState {
  return {
    tableSlots: [
      ...agentIds.slice(0, MAX_AGENT_SLOTS),
      ...Array.from({ length: Math.max(0, MAX_AGENT_SLOTS - agentIds.length) }, () => null),
    ],
    offTableAgents: [],
  };
}

function normalizeAgentTableSlots(slots: readonly unknown[]): (AgentId | null)[] {
  const seen = new Set<string>();
  const normalized: (AgentId | null)[] = slots.slice(0, MAX_AGENT_SLOTS).map((slot) => {
    if (typeof slot !== 'string' || !slot || seen.has(slot)) return null;
    seen.add(slot);
    return slot as AgentId;
  });
  while (normalized.length < MAX_AGENT_SLOTS) normalized.push(null);
  return normalized;
}

function loadProjectAgentLayout(projectId: ProjectId | null | undefined): AgentLayoutState | null {
  if (!projectId) return null;
  try {
    const raw = localStorage.getItem(LS_AGENT_LAYOUT);
    if (!raw) return null;
    const all = JSON.parse(raw) as Record<string, { tableSlots?: unknown } | undefined>;
    const stored = all[projectId];
    if (!stored || !Array.isArray(stored.tableSlots)) return null;
    return {
      tableSlots: normalizeAgentTableSlots(stored.tableSlots),
      offTableAgents: [],
    };
  } catch {
    return null;
  }
}

function tableLayoutMergingSavedSlots(
  savedSlots: readonly (string | null)[] | null,
  agentIds: readonly AgentId[],
): AgentLayoutState {
  if (!savedSlots) return tableLayoutForProjectAgents(agentIds);
  const activeIds = new Set(agentIds);
  const tableSlots = normalizeAgentTableSlots(savedSlots).map((id) => (id && activeIds.has(id) ? id : null));
  const placed = new Set(tableSlots.filter((id): id is AgentId => !!id));
  for (const id of agentIds) {
    if (placed.has(id)) continue;
    const emptyIndex = tableSlots.findIndex((slot) => slot == null);
    if (emptyIndex < 0) break;
    tableSlots[emptyIndex] = id;
    placed.add(id);
  }
  return { tableSlots, offTableAgents: [] };
}

function tableLayoutForProjectAgentsWithSavedSlots(
  projectId: ProjectId | null | undefined,
  agentIds: readonly AgentId[],
): AgentLayoutState {
  // Legacy-store reader (provisional layout only): remap so the pre-hydrate
  // flash already shows the clockwise slot order.
  return tableLayoutMergingSavedSlots(
    remapLegacySeatOrder(loadProjectAgentLayout(projectId)?.tableSlots ?? null),
    agentIds,
  );
}

function provisionalProjectAgentIds(workspace: WorkspaceProject | null | undefined): AgentId[] {
  if (!workspace) return [];
  const seen = new Set<string>();
  const ids: AgentId[] = [];
  for (const agent of workspace.agents) {
    if (!agent.agentId || seen.has(agent.agentId)) continue;
    seen.add(agent.agentId);
    ids.push(agent.agentId as AgentId);
  }
  return ids;
}

function provisionalLayoutForWorkspace(workspace: WorkspaceProject | null | undefined): AgentLayoutState | null {
  const ids = provisionalProjectAgentIds(workspace);
  if (ids.length === 0) return null;
  return tableLayoutForProjectAgentsWithSavedSlots(workspace?.projectId, ids);
}

function workspaceProjectName(workspace: WorkspaceProject): string {
  const trimmed = workspace.repoFullName.trim();
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? (trimmed || workspace.projectId);
}

function workspaceProjectToTab(workspace: WorkspaceProject): Project {
  return {
    id: workspace.projectId,
    name: workspaceProjectName(workspace),
    path: workspace.sourceDir,
    sourcePath: workspace.sourceDir,
    accountPath: workspace.localRoot,
    githubUrl: workspace.githubHtmlUrl,
  };
}

function normalizedPathKey(path: string | null | undefined): string {
  return (path ?? '').trim().replace(/\/+$/, '');
}

function fallbackDreamProjectName(projectRoot: string, projectId: string): string {
  const parts = projectRoot.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? projectId;
}

type DreamProjectAccumulator = {
  projectId: string;
  projectRoot: string;
  projectName: string;
  agents: Map<AgentId, { id: AgentId; name: string }>;
};

function dreamProjectsFromRunningPtys(
  summaries: readonly AgentSummary[],
  agentMeta: Readonly<Record<AgentId, Agent>>,
  visibleWorkspaces: readonly WorkspaceProject[],
  currentWorkspace: WorkspaceProject | null | undefined,
  currentLiveAgents: ReadonlySet<AgentId>,
): DreamProjectTarget[] {
  const workspaceByRoot = new Map<string, WorkspaceProject>();
  visibleWorkspaces.forEach((workspace) => {
    const key = normalizedPathKey(workspace.localRoot);
    if (key && !workspaceByRoot.has(key)) workspaceByRoot.set(key, workspace);
  });

  const groups = new Map<string, DreamProjectAccumulator>();
  const ensureGroup = (projectRootInput: string, projectIdInput?: string | null): DreamProjectAccumulator | null => {
    const projectRoot = normalizedPathKey(projectRootInput);
    if (!projectRoot) return null;
    const projectId = (projectIdInput ?? '').trim() || projectRoot;
    const workspace = workspaceByRoot.get(projectRoot);
    const key = projectRoot;
    const existing = groups.get(key);
    if (existing) {
      if (existing.projectId === existing.projectRoot && projectId !== projectRoot) {
        existing.projectId = projectId;
      }
      return existing;
    }
    const group = {
      projectId,
      projectRoot,
      projectName: workspace ? workspaceProjectName(workspace) : fallbackDreamProjectName(projectRoot, projectId),
      agents: new Map<AgentId, { id: AgentId; name: string }>(),
    };
    groups.set(key, group);
    return group;
  };

  const addAgent = (group: DreamProjectAccumulator | null, agentIdInput: string | null | undefined) => {
    if (!group || !agentIdInput) return;
    const agentId = agentIdInput as AgentId;
    if (!agentId || group.agents.has(agentId)) return;
    group.agents.set(agentId, {
      id: agentId,
      name: agentMeta[agentId]?.name ?? agentId,
    });
  };

  summaries.forEach((summary) => {
    if (!summary.running) return;
    addAgent(ensureGroup(summary.projectRoot, summary.projectId), summary.agentId);
  });

  if (currentWorkspace?.localRoot) {
    const currentRoster = new Set(currentWorkspace.agents.map((agent) => agent.agentId as AgentId));
    const currentGroup = ensureGroup(currentWorkspace.localRoot, currentWorkspace.projectId);
    currentLiveAgents.forEach((agentId) => {
      if (currentRoster.has(agentId)) addAgent(currentGroup, agentId);
    });
  }

  return Array.from(groups.values()).flatMap((group) => {
    const agents = Array.from(group.agents.values())
      .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
    if (agents.length === 0) return [];
    return [{
      projectId: group.projectId,
      projectRoot: group.projectRoot,
      projectName: group.projectName,
      agents,
    }];
  }).sort((left, right) => (
    left.projectName.localeCompare(right.projectName)
    || left.projectRoot.localeCompare(right.projectRoot)
  ));
}

function upsertWorkspaceProject(
  workspaces: readonly WorkspaceProject[],
  workspace: WorkspaceProject,
): WorkspaceProject[] {
  const index = workspaces.findIndex((item) => item.projectId === workspace.projectId);
  if (index < 0) return [...workspaces, workspace];
  const next = [...workspaces];
  next[index] = workspace;
  return next;
}

function workspaceProjectIds(workspaces: readonly WorkspaceProject[]): ProjectId[] {
  return workspaces.map((workspace) => workspace.projectId);
}

function uniqueProjectIds(ids: readonly unknown[]): ProjectId[] {
  const seen = new Set<string>();
  const next: ProjectId[] = [];
  for (const id of ids) {
    if (typeof id !== 'string' || id.length === 0 || seen.has(id)) continue;
    seen.add(id);
    next.push(id);
  }
  return next;
}

function sameProjectIdOrder(left: readonly ProjectId[], right: readonly ProjectId[]): boolean {
  return left.length === right.length && left.every((id, index) => id === right[index]);
}

function loadWorkspaceTabOrder(): ProjectId[] {
  try {
    const parsed = JSON.parse(loadStorageValue(LS_WORKSPACE_TAB_ORDER) ?? '[]');
    return Array.isArray(parsed) ? uniqueProjectIds(parsed) : [];
  } catch {
    return [];
  }
}

function persistWorkspaceTabOrder(ids: readonly ProjectId[]): void {
  setStorageValue(LS_WORKSPACE_TAB_ORDER, JSON.stringify(uniqueProjectIds(ids)));
}

function orderWorkspaceProjects(workspaces: readonly WorkspaceProject[]): WorkspaceProject[] {
  const byId = new Map(workspaces.map((workspace) => [workspace.projectId, workspace]));
  const order = loadWorkspaceTabOrder();
  const ordered = order
    .map((projectId) => byId.get(projectId))
    .filter((workspace): workspace is WorkspaceProject => !!workspace);
  const orderedIds = new Set(ordered.map((workspace) => workspace.projectId));
  for (const workspace of workspaces) {
    if (!orderedIds.has(workspace.projectId)) ordered.push(workspace);
  }
  const nextOrder = workspaceProjectIds(ordered);
  if (!sameProjectIdOrder(order, nextOrder)) persistWorkspaceTabOrder(nextOrder);
  return ordered;
}

function sameLmSelectedForSync(left: LmSelected | null, right: LmSelected | null): boolean {
  if (left === right) return true;
  if (!left || !right) return false;
  return left.projectId === right.projectId &&
    left.projectRoot === right.projectRoot &&
    left.agentId === right.agentId &&
    left.muted === right.muted;
}

function findLmSelectedWorkspace(
  workspaces: readonly WorkspaceProject[],
  selected: LmSelected | null,
): WorkspaceProject | null {
  if (!selected) return null;
  return workspaces.find((workspace) => (
    workspace.localRoot === selected.projectRoot ||
    workspace.projectId === selected.projectId
  )) ?? null;
}

function lmSelectedWorkspaceAgentIds(
  workspace: WorkspaceProject | null,
  selected: LmSelected | null,
): AgentId[] {
  const ids = new Set<AgentId>();
  for (const agent of workspace?.agents ?? []) {
    if (agent.agentId) ids.add(agent.agentId as AgentId);
  }
  if (selected?.agentId) ids.add(selected.agentId as AgentId);
  return Array.from(ids).sort();
}

type AppearanceStorageKey =
  | typeof LS_CENTER
  | typeof LS_ROOM
  | typeof LS_DESK
  | typeof LS_ROOM_THEME
  | typeof LS_DESK_THEME;

function projectAppearanceStorageKey(projectId: ProjectId, key: AppearanceStorageKey): string {
  return `${LS_PROJECT_APPEARANCE}.${encodeURIComponent(projectId)}.${key}`;
}

function loadStorageValue(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function setStorageValue(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Ignore storage failures in tests/webview privacy modes.
  }
}

function loadProjectStorageValue(projectId: ProjectId, key: AppearanceStorageKey): string | null {
  return loadStorageValue(projectAppearanceStorageKey(projectId, key)) ?? loadStorageValue(key);
}

function loadCenterpiece(projectId?: ProjectId): Centerpiece {
  try {
    const raw = projectId ? loadProjectStorageValue(projectId, LS_CENTER) : loadStorageValue(LS_CENTER);
    if (raw === 'fire' || raw === 'magic' || raw === 'flora') return raw;
    return DEFAULT_CENTER;
  } catch {
    return DEFAULT_CENTER;
  }
}

function loadColor(
  key: string,
  fallback: string,
  allowed?: ReadonlyArray<{ color: string }>,
  projectId?: ProjectId,
): string {
  try {
    const raw = projectId
      ? loadProjectStorageValue(projectId, key as AppearanceStorageKey)
      : loadStorageValue(key);
    // Only accept plain hex strings — defensive against tampering.
    if (!raw || !/^#[0-9A-Fa-f]{3,8}$/.test(raw)) return fallback;
    return !allowed || allowed.some((item) => item.color.toLowerCase() === raw.toLowerCase())
      ? raw
      : fallback;
  } catch {
    return fallback;
  }
}

function loadRoomTheme(projectId?: ProjectId): RoomTheme {
  try {
    const raw = projectId
      ? loadProjectStorageValue(projectId, LS_ROOM_THEME)
      : loadStorageValue(LS_ROOM_THEME);
    return raw === 'classic' ||
      raw === 'study' ||
      raw === 'workshop' ||
      raw === 'observatory' ||
      raw === 'dark' ||
      raw === 'light'
      ? raw
      : DEFAULT_ROOM_THEME;
  } catch {
    return DEFAULT_ROOM_THEME;
  }
}

function loadDeskTheme(projectId?: ProjectId): DeskTheme {
  try {
    const raw = projectId
      ? loadProjectStorageValue(projectId, LS_DESK_THEME)
      : loadStorageValue(LS_DESK_THEME);
    return raw === 'warm' ||
      raw === 'walnut' ||
      raw === 'ember' ||
      raw === 'terminal' ||
      raw === 'star' ||
      raw === 'parchment' ||
      raw === 'dark' ||
      raw === 'light'
      ? raw
      : DEFAULT_DESK_THEME;
  } catch {
    return DEFAULT_DESK_THEME;
  }
}

/** Root — routing target + popup orchestration.
 *
 *  Per W3 (HANDOFF-W-FloatingWindows.md), the desk no longer hosts
 *  tile-mode terminals; live agent PTYs render as floating windows
 *  in <AgentWindowsLayer>. So App.tsx no longer carries layoutMode
 *  or openAgents — the source of truth for "what's on screen" is
 *  agentRuntime.liveAgents. */
export function App() {
  const sceneKey: SceneKey = 'conversation';
  const [popup, setPopup] = useState<Popup>(null);
  const closePopup = useCallback(() => setPopup(null), []);
  const toggleWhiteboard = useCallback(() => {
    setPopup((current) => current?.kind === 'whiteboard' ? null : { kind: 'whiteboard' });
  }, []);

  const [inputText, setInputText] = useState('');
  const [composerTarget, setComposerTarget] = useState<AgentId | null>(null);
  const [composerBroadcast, setComposerBroadcast] = useState(false);
  const [broadcastPopupOpen, setBroadcastPopupOpen] = useState(false);
  const [broadcastRecipients, setBroadcastRecipients] = useState<Set<AgentId>>(() => new Set());
  const [privateAgents, setPrivateAgents] = useState<Set<AgentId>>(() => new Set());
  const persistLayoutTimerRef = useRef<number | null>(null);
  const [agentLayout, setAgentLayout] = useState<AgentLayoutState>(() => ({
    tableSlots: [...DEFAULT_TABLE_SLOTS],
    offTableAgents: [...DEFAULT_OFF_TABLE_AGENTS],
  }));
  const [recruitSeatIndex, setRecruitSeatIndex] = useState<number | null>(null);
  const [shortcutRecruitSeatIndex, setShortcutRecruitSeatIndex] = useState<number | null>(null);
  const [incarnationProgress, setIncarnationProgress] = useState<IncarnationProgressView | null>(null);
  const incarnationProgressIdRef = useRef<string | null>(null);
  const incarnationRetryPayloadRef = useRef<{ hero: WorkingHero; seatIndex: number } | null>(null);
  const incarnationProgressHideTimerRef = useRef<number | null>(null);
  const [workingHeroes, setWorkingHeroes] = useState<readonly WorkingHero[]>(() =>
    loadTavernWorkingHeroes(),
  );
  const workingHeroShellsRef = useRef<readonly SupportedShellStatus[] | null>(null);
  const [agentInstances, setAgentInstances] = useState<Record<AgentId, WorkingHero>>({});
  const [projectAgentIdentities, setProjectAgentIdentities] = useState<ProjectAgentIdentity[]>([]);
  const [agentRecords, setAgentRecords] = useState<Record<AgentId, ProjectAgentRecord>>({});
  const agentRecordsRef = useRef(agentRecords);
  const [dreamingStatusAgents, setDreamingStatusAgents] = useState<ReadonlySet<AgentId>>(() => new Set());
  const [avatarLibraryVersion, setAvatarLibraryVersion] = useState(0);
  const [ghosttyTerminalEnhancement, setGhosttyTerminalEnhancement] = useState(false);
  const [terminalFocusedAgent, setTerminalFocusedAgent] = useState<AgentId | null>(null);
  const [activeProjectId, setActiveProjectId] = useState<ProjectId>(DEFAULT_PROJECT_ID);
  const [tavernOpen, setTavernOpen] = useState(false);
  const [tavernPreparing, setTavernPreparing] = useState(false);
  const [tavernPrepareItems, setTavernPrepareItems] = useState<TavernLoadingLogItem[]>([]);
  const [tavernInitialTab, setTavernInitialTab] = useState<TavernTab>('heroes');
  const [projectSetupOpen, setProjectSetupOpen] = useState(false);
  const [activeWorkspace, setActiveWorkspace] = useState<WorkspaceProject | null>(null);
  const [workspaceInitialLoadComplete, setWorkspaceInitialLoadComplete] = useState(() => !hasTauriRuntime());
  const [workspaceTabs, setWorkspaceTabs] = useState<WorkspaceProject[]>([]);
  const [fileTreeRefreshToken, setFileTreeRefreshToken] = useState(0);
  const [ghAuth, setGhAuth] = useState<GhAuthInfo | null>(null);
  const [recruitProjectRoot, setRecruitProjectRoot] = useState<string>(() => {
    try {
      return localStorage.getItem(LS_DEV_PROJECT_ROOT) ?? BROWSER_DEV_PROJECT_ROOT;
    } catch {
      return BROWSER_DEV_PROJECT_ROOT;
    }
  });
  const projectAgentRoot = activeWorkspace ? null : recruitProjectRoot;
  const violetProjectRoot = activeWorkspace?.localRoot ?? projectAgentRoot;
  const projectRulesDir = activeWorkspace?.rulesDir ?? (projectAgentRoot ? `${projectAgentRoot}/project-rules` : null);
  const [agentContextMenu, setAgentContextMenu] = useState<AgentContextMenuState>(null);
  const [projectAgentDetailId, setProjectAgentDetailId] = useState<AgentId | null>(null);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [archivedAgents, setArchivedAgents] = useState<ProjectAgentDetail[]>([]);
  const [confirmDialog, setConfirmDialog] = useState<ConfirmDialogState>(null);
  const agentPromptQueuesRef = useRef<Map<AgentId, Promise<void>>>(new Map());
  const existingAgentLaunchesRef = useRef<Map<AgentId, Promise<ExistingAgentLaunchResult>>>(new Map());
  const emberSchedulerInFlightRef = useRef(false);
  const tavernPrepareSeqRef = useRef(0);
  const archivedAgentsRequestRef = useRef(0);
  const violetSyncRef = useRef<VioletProjectSyncHandle | null>(null);
  const lmVioletSyncRef = useRef<VioletProjectSyncHandle | null>(null);
  const [lmSelectedForSync, setLmSelectedForSync] = useState<LmSelected | null>(null);
  const hydratedAgentLayoutProjectRef = useRef<ProjectId | null>(null);
  const [hydratedLayoutProjectId, setHydratedLayoutProjectId] = useState<ProjectId | null>(null);
  const [agentHydrationProgress, setAgentHydrationProgress] = useState<AgentHydrationProgress | null>(null);
  const [appearanceProjectId, setAppearanceProjectId] = useState<ProjectId>(DEFAULT_PROJECT_ID);
  const [centerpiece, setCenterpiece] = useState<Centerpiece>(() => loadCenterpiece(DEFAULT_PROJECT_ID));
  const [roomColor, setRoomColor] = useState<string>(() =>
    loadColor(LS_ROOM, DEFAULT_ROOM, ROOM_SWATCHES, DEFAULT_PROJECT_ID),
  );
  const [deskColor, setDeskColor] = useState<string>(() =>
    loadColor(LS_DESK, DEFAULT_DESK, DESK_SWATCHES, DEFAULT_PROJECT_ID),
  );
  const [roomTheme, setRoomTheme] = useState<RoomTheme>(() => loadRoomTheme(DEFAULT_PROJECT_ID));
  const [deskTheme, setDeskTheme] = useState<DeskTheme>(() => loadDeskTheme(DEFAULT_PROJECT_ID));

  const onTableAgents = useMemo(
    () => agentLayout.tableSlots.filter((id): id is AgentId => !!id),
    [agentLayout.tableSlots],
  );
  const shortcutAgentsOrdered = useMemo(
    () => agentLayout.tableSlots.slice(0, MAX_AGENT_SLOTS),
    [agentLayout.tableSlots],
  );
  const shortcutTargetAgents = useMemo(
    () => shortcutAgentsOrdered.filter((id): id is AgentId => !!id),
    [shortcutAgentsOrdered],
  );
  const firstEmptySeatIndex = useMemo(
    () => shortcutAgentsOrdered.findIndex((slot) => slot == null),
    [shortcutAgentsOrdered],
  );
  const tableFull = firstEmptySeatIndex < 0;
  const agentsHydrating = !!activeWorkspace && hydratedLayoutProjectId !== activeWorkspace.projectId;
  const activeAgentHydrationProgress = agentHydrationProgress?.projectId === activeWorkspace?.projectId
    ? agentHydrationProgress
    : null;
  const agentMeta = useMemo<Record<AgentId, Agent>>(() => {
    const next: Record<AgentId, Agent> = { ...AGENTS };
    for (const identity of projectAgentIdentities) {
      const base = AGENTS[identity.sourceHeroId] ?? AGENTS[identity.agentId] ?? {
        name: identity.displayName,
        emoji: '◇',
        role: 'Project agent',
        hue: 'var(--brass-bright)',
      };
      const lifecycleStatus = projectAgentLifecycleStatus(identity.status);
      next[identity.agentId] = {
        ...base,
        name: identity.displayName,
        captain: false,
        avatarId: identity.avatarId ?? base.avatarId,
        avatarClass: avatarClassForId(identity.avatarId ?? base.avatarId, identity.provider),
        lifecycleStatus,
      };
    }
    for (const instance of Object.values(agentInstances)) {
      const templateId = instance.templateId ?? instance.id;
      const base = AGENTS[templateId] ?? AGENTS[instance.id] ?? {
        name: instance.name,
        emoji: '◇',
        role: instance.record,
        hue: 'var(--brass-bright)',
      };
      next[instance.id] = {
        ...base,
        name: instance.name,
        captain: false,
        avatarId: instance.avatarId,
        avatarClass: instance.avatarClass ?? avatarClassForId(instance.avatarId, instance.cli),
        lifecycleStatus: undefined,
      };
    }
    return next;
  }, [agentInstances, avatarLibraryVersion, projectAgentIdentities]);
  const confirmInApp = useCallback((
    title: string,
    body: string,
    options?: {
      confirmLabel?: string;
      cancelLabel?: string;
      tone?: 'danger';
      plainCopy?: boolean;
      confirmOnEnter?: boolean;
    },
  ) => new Promise<boolean>((resolve) => {
    setConfirmDialog({
      title,
      body,
      confirmLabel: options?.confirmLabel ?? 'Confirm',
      cancelLabel: options?.cancelLabel ?? 'Cancel',
      tone: options?.tone,
      plainCopy: options?.plainCopy,
      confirmOnEnter: options?.confirmOnEnter,
      resolve,
    });
  }), []);
  const closeConfirmDialog = useCallback((confirmed: boolean) => {
    setConfirmDialog((dialog) => {
      dialog?.resolve(confirmed);
      return null;
    });
  }, []);
  const refreshFileTree = useCallback(() => {
    setFileTreeRefreshToken((token) => token + 1);
  }, []);
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    let refreshTimer: number | null = null;
    const scheduleRefresh = () => {
      if (refreshTimer !== null) window.clearTimeout(refreshTimer);
      refreshTimer = window.setTimeout(refreshFileTree, 250);
    };

    void onVioletRoomSynced(({ request }) => {
      const projectRoot = request.projectRoot?.trim();
      if (
        activeWorkspace &&
        projectRoot &&
        projectRoot !== activeWorkspace.sourceDir &&
        projectRoot !== activeWorkspace.localRoot
      ) {
        return;
      }
      scheduleRefresh();
    }).then((nextUnlisten) => {
      if (cancelled) {
        nextUnlisten();
      } else {
        unlisten = nextUnlisten;
      }
    });

    return () => {
      cancelled = true;
      if (refreshTimer !== null) window.clearTimeout(refreshTimer);
      if (unlisten) unlisten();
    };
  }, [activeWorkspace, refreshFileTree]);
  const rememberWorkspaceTab = useCallback((workspace: WorkspaceProject) => {
    setWorkspaceTabs((prev) => {
      const next = upsertWorkspaceProject(prev, workspace);
      persistWorkspaceTabOrder(workspaceProjectIds(next));
      return next;
    });
  }, []);
  const forgetWorkspaceTab = useCallback((projectId: ProjectId) => {
    setWorkspaceTabs((prev) => {
      const next = prev.filter((workspace) => workspace.projectId !== projectId);
      persistWorkspaceTabOrder(workspaceProjectIds(next));
      return next;
    });
  }, []);
  const reorderWorkspaceTabs = useCallback((projectIds: readonly ProjectId[]) => {
    setWorkspaceTabs((prev) => {
      const byId = new Map(prev.map((workspace) => [workspace.projectId, workspace]));
      const seen = new Set<ProjectId>();
      const next: WorkspaceProject[] = [];
      for (const projectId of uniqueProjectIds(projectIds)) {
        const workspace = byId.get(projectId);
        if (!workspace || seen.has(projectId)) continue;
        seen.add(projectId);
        next.push(workspace);
      }
      for (const workspace of prev) {
        if (seen.has(workspace.projectId)) continue;
        next.push(workspace);
      }
      persistWorkspaceTabOrder(workspaceProjectIds(next));
      return next;
    });
  }, []);
  const removeWorkspaceAgentSpec = useCallback((agentId: AgentId) => {
    setActiveWorkspace((prev) => {
      if (!prev || prev.agents.every((agent) => agent.agentId !== agentId)) return prev;
      return {
        ...prev,
        agents: prev.agents.filter((agent) => agent.agentId !== agentId),
      };
    });
  }, []);
  const agentName = useCallback((id: AgentId) => agentMeta[id]?.name ?? id, [agentMeta]);
  const targetAgent = composerBroadcast ? null : composerTarget;
  const effectivePrivateAgents = PRIVATE_CHAT_UI_ENABLED ? privateAgents : EMPTY_AGENT_SET;
  const currentTargetPrivate = composerTarget ? effectivePrivateAgents.has(composerTarget) : false;
  const broadcastPrivacyInfo = useMemo(() => {
    if (!composerBroadcast) return undefined;
    const privateNames: string[] = [];
    const publicNames: string[] = [];
    for (const id of broadcastRecipients) {
      const name = agentName(id);
      if (effectivePrivateAgents.has(id)) privateNames.push(name);
      else publicNames.push(name);
    }
    return { privateNames, publicNames };
  }, [agentName, broadcastRecipients, composerBroadcast, effectivePrivateAgents]);
  const visibleWorkspaceTabs = useMemo(
    () => (activeWorkspace ? upsertWorkspaceProject(workspaceTabs, activeWorkspace) : workspaceTabs),
    [activeWorkspace, workspaceTabs],
  );
  // M6.A — live agent PTYs (CC / Codex spawned via pty/agent.rs).
  // Recruit/send/dismiss happen here; AgentWindowsLayer renders the windows.
  const agentRuntime = useAgentRuntime();
  const recruitWithLeaseTakeover = useCallback(async (request: AgentSpawnRequest): Promise<boolean> => {
    try {
      await agentRuntime.recruit(request);
      return true;
    } catch (err) {
      if (!isAgentSessionLeaseConflictError(err)) throw err;
      const name = agentName(err.conflict.agentId as AgentId);
      const confirmed = await confirmInApp(
        `${name} is running in another window`,
        `Open ${name} here will end the other session.`,
        {
          cancelLabel: 'Cancel',
          confirmLabel: 'Yes, takeover here.',
          plainCopy: true,
          tone: 'danger',
        },
      );
      if (!confirmed) return false;
      await agentRuntime.recruit({ ...request, takeover: true });
      return true;
    }
  }, [agentName, agentRuntime, confirmInApp]);
  const [agentPtySummaries, setAgentPtySummaries] = useState<AgentSummary[]>([]);
  const refreshAgentPtySummaries = useCallback(async () => {
    try {
      setAgentPtySummaries(await listAgentPtys());
    } catch (err) {
      console.warn('[kota-agent] could not refresh agent PTYs', err);
    }
  }, []);
  useEffect(() => {
    if (!hasTauriRuntime()) return;
    let cancelled = false;
    const refresh = async () => {
      try {
        const summaries = await listAgentPtys();
        if (!cancelled) setAgentPtySummaries(summaries);
      } catch (err) {
        if (!cancelled) console.warn('[kota-agent] could not refresh agent PTYs', err);
      }
    };
    void refresh();
    const timer = window.setInterval(() => {
      void refresh();
    }, 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);
  const buildDreamProjects = useCallback((summaries: readonly AgentSummary[]) => (
    dreamProjectsFromRunningPtys(
      summaries,
      agentMeta,
      visibleWorkspaceTabs,
      activeWorkspace,
      agentRuntime.liveAgents,
    )
  ), [
    activeWorkspace,
    agentMeta,
    agentRuntime.liveAgents,
    visibleWorkspaceTabs,
  ]);
  const dreamProjects = useMemo(
    () => buildDreamProjects(agentPtySummaries),
    [agentPtySummaries, buildDreamProjects],
  );
  const resolveDreamProjects = useCallback(async () => {
    if (!hasTauriRuntime()) return buildDreamProjects(agentPtySummaries);
    const summaries = await listAgentPtys();
    setAgentPtySummaries(summaries);
    const targets = buildDreamProjects(summaries);
    return targets;
  }, [agentPtySummaries, buildDreamProjects]);
  const projectTabs = useMemo(
    () => visibleWorkspaceTabs.map(workspaceProjectToTab),
    [visibleWorkspaceTabs],
  );
  const activeProjectName = projectTabs.find((p) => p.id === activeProjectId)?.name ?? activeProjectId;
  const showProjectLoadingState = hasTauriRuntime() && !workspaceInitialLoadComplete;
  const showProjectEmptyState = hasTauriRuntime() && workspaceInitialLoadComplete && !activeWorkspace;

  const clearIncarnationProgressHideTimer = useCallback(() => {
    if (incarnationProgressHideTimerRef.current !== null) {
      window.clearTimeout(incarnationProgressHideTimerRef.current);
      incarnationProgressHideTimerRef.current = null;
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onIncarnationProgressEvent((payload) => {
      if (payload.progressId !== incarnationProgressIdRef.current) return;
      const stepId = normalizeIncarnationProgressStep(payload.step);
      const nextPhase: IncarnationProgressPhase = payload.status === 'error' ? 'error' : 'running';
      setIncarnationProgress((prev) => {
        if (!prev || prev.id !== payload.progressId || prev.phase === 'error') return prev;
        return {
          ...prev,
          stepId,
          phase: nextPhase,
          message: payload.message || prev.message,
          copied: false,
        };
      });
    }).then((off) => {
      if (cancelled) off();
      else unlisten = off;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  useEffect(() => () => clearIncarnationProgressHideTimer(), [clearIncarnationProgressHideTimer]);

  const markIncarnationProgressStep = useCallback((stepId: IncarnationProgressStepId, message: string) => {
    clearIncarnationProgressHideTimer();
    setIncarnationProgress((prev) => (
      prev
        ? {
            ...prev,
            stepId,
            message,
            phase: 'running',
            errorMessage: undefined,
            copied: false,
          }
        : prev
    ));
  }, [clearIncarnationProgressHideTimer]);

  const failIncarnationProgress = useCallback((agentId: AgentId, err: unknown) => {
    clearIncarnationProgressHideTimer();
    const message = recruitErrorMessage(agentId, err);
    setIncarnationProgress((prev) => (
      prev
        ? {
            ...prev,
            phase: 'error',
            message,
            errorMessage: message,
            copied: false,
          }
        : prev
    ));
  }, [clearIncarnationProgressHideTimer]);

  const finishIncarnationProgress = useCallback(() => {
    clearIncarnationProgressHideTimer();
    const progressId = incarnationProgressIdRef.current;
    setIncarnationProgress((prev) => (
      prev
        ? {
            ...prev,
            stepId: 'launch',
            phase: 'success',
            message: `${prev.heroName} is ready.`,
            errorMessage: undefined,
            copied: false,
          }
        : prev
    ));
    incarnationProgressHideTimerRef.current = window.setTimeout(() => {
      if (incarnationProgressIdRef.current !== progressId) return;
      incarnationProgressIdRef.current = null;
      incarnationRetryPayloadRef.current = null;
      setIncarnationProgress(null);
    }, 1200);
  }, [clearIncarnationProgressHideTimer]);

  const dismissIncarnationProgress = useCallback(() => {
    clearIncarnationProgressHideTimer();
    incarnationProgressIdRef.current = null;
    incarnationRetryPayloadRef.current = null;
    setIncarnationProgress(null);
  }, [clearIncarnationProgressHideTimer]);

  const copyIncarnationProgressError = useCallback(() => {
    const error = incarnationProgress?.errorMessage ?? incarnationProgress?.message;
    if (!error) return;
    void navigator.clipboard?.writeText(error);
    setIncarnationProgress((prev) => (prev ? { ...prev, copied: true } : prev));
  }, [incarnationProgress?.errorMessage, incarnationProgress?.message]);

  // Paint state is project-scoped. Legacy global keys remain as a migration
  // fallback, but writes go to the active project's namespace only.
  useEffect(() => {
    setAppearanceProjectId(activeProjectId);
    setCenterpiece(loadCenterpiece(activeProjectId));
    setRoomColor(loadColor(LS_ROOM, DEFAULT_ROOM, ROOM_SWATCHES, activeProjectId));
    setDeskColor(loadColor(LS_DESK, DEFAULT_DESK, DESK_SWATCHES, activeProjectId));
    setRoomTheme(loadRoomTheme(activeProjectId));
    setDeskTheme(loadDeskTheme(activeProjectId));
  }, [activeProjectId]);
  useEffect(() => {
    if (appearanceProjectId !== activeProjectId) return;
    setStorageValue(projectAppearanceStorageKey(activeProjectId, LS_CENTER), centerpiece);
  }, [activeProjectId, appearanceProjectId, centerpiece]);
  useEffect(() => {
    if (appearanceProjectId !== activeProjectId) return;
    setStorageValue(projectAppearanceStorageKey(activeProjectId, LS_ROOM), roomColor);
  }, [activeProjectId, appearanceProjectId, roomColor]);
  useEffect(() => {
    if (appearanceProjectId !== activeProjectId) return;
    setStorageValue(projectAppearanceStorageKey(activeProjectId, LS_DESK), deskColor);
  }, [activeProjectId, appearanceProjectId, deskColor]);
  useEffect(() => {
    if (appearanceProjectId !== activeProjectId) return;
    setStorageValue(projectAppearanceStorageKey(activeProjectId, LS_ROOM_THEME), roomTheme);
  }, [activeProjectId, appearanceProjectId, roomTheme]);
  useEffect(() => {
    if (appearanceProjectId !== activeProjectId) return;
    setStorageValue(projectAppearanceStorageKey(activeProjectId, LS_DESK_THEME), deskTheme);
  }, [activeProjectId, appearanceProjectId, deskTheme]);
  useEffect(() => {
    agentRecordsRef.current = agentRecords;
  }, [agentRecords]);

  useEffect(() => {
    if (!hasTauriRuntime()) {
      setWorkspaceInitialLoadComplete(true);
      return;
    }
    let cancelled = false;
    void (async () => {
      const [statusResult, projectsResult] = await Promise.allSettled([
        workspaceStatus(),
        listWorkspaceProjects(),
      ]);
      if (cancelled) return;
      const availableProjects = projectsResult.status === 'fulfilled'
        ? orderWorkspaceProjects(projectsResult.value.filter((workspace) => !workspace.archived))
        : [];
      if (statusResult.status === 'fulfilled' && statusResult.value.active) {
        const workspace = statusResult.value.active;
        const nextTabs = upsertWorkspaceProject(availableProjects, workspace);
        persistWorkspaceTabOrder(workspaceProjectIds(nextTabs));
        setActiveWorkspace(workspace);
        setActiveProjectId(workspace.projectId);
        setWorkspaceTabs(nextTabs);
        setWorkspaceInitialLoadComplete(true);
        return;
      }
      const firstProject = availableProjects[0];
      if (firstProject) {
        try {
          const workspace = await openWorkspaceProject(firstProject.projectId);
          if (cancelled) return;
          const nextTabs = upsertWorkspaceProject(availableProjects, workspace);
          persistWorkspaceTabOrder(workspaceProjectIds(nextTabs));
          setActiveWorkspace(workspace);
          setActiveProjectId(workspace.projectId);
          setWorkspaceTabs(nextTabs);
          refreshFileTree();
          setWorkspaceInitialLoadComplete(true);
          return;
        } catch (err) {
          console.warn('[workspace] could not restore first project tab', err);
        }
      }
      setWorkspaceTabs(availableProjects);
      setWorkspaceInitialLoadComplete(true);
    })().catch((err) => {
      if (!cancelled) {
        console.warn('[workspace] initial load failed', err);
        setWorkspaceInitialLoadComplete(true);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [refreshFileTree]);

  useEffect(() => {
    if (!activeWorkspace) return;
    setWorkspaceTabs((prev) => {
      const existing = prev.find((workspace) => workspace.projectId === activeWorkspace.projectId);
      if (existing === activeWorkspace) return prev;
      const next = upsertWorkspaceProject(prev, activeWorkspace);
      persistWorkspaceTabOrder(workspaceProjectIds(next));
      return next;
    });
  }, [activeWorkspace]);

  useEffect(() => {
    if (!activeWorkspace && workspaceTabs.length > 0 && hasTauriRuntime()) {
      void openWorkspaceProject(workspaceTabs[0]!.projectId)
        .then((workspace) => {
          setActiveWorkspace(workspace);
          setActiveProjectId(workspace.projectId);
          refreshFileTree();
        })
        .catch((err) => {
          console.warn('[workspace] could not restore first project tab', err);
        });
    }
  }, [activeWorkspace, refreshFileTree, workspaceTabs]);

  useEffect(() => {
    void terminalEnhancementStatus()
      .then((status) => {
        setGhosttyTerminalEnhancement(status.ghosttyTerminalEnhancementEnabled);
      })
      .catch(() => {});
  }, []);

  const refreshGhAuth = useCallback(async () => {
    try {
      setGhAuth(await ghAuthStatus());
    } catch (err) {
      setGhAuth({
        authenticated: false,
        username: null,
        scopes: [],
        error: String(err),
        cliMissing: false,
      });
    }
  }, []);

  useEffect(() => {
    void refreshGhAuth();
  }, [refreshGhAuth]);

  useEffect(() => {
    if (ghAuth?.authenticated) return;
    const interval = window.setInterval(() => {
      void refreshGhAuth();
    }, 3000);
    return () => window.clearInterval(interval);
  }, [ghAuth?.authenticated, refreshGhAuth]);

  useEffect(() => preloadTavernAssets(), []);

  useEffect(() => {
    const refresh = () => setAvatarLibraryVersion((version) => version + 1);
    window.addEventListener('kota-v2:user-hero-avatars-changed', refresh);
    void refreshUserHeroAvatars().catch(() => {});
    return () => window.removeEventListener('kota-v2:user-hero-avatars-changed', refresh);
  }, []);

  useEffect(() => {
    const refreshWorkingHeroes = () => {
      setWorkingHeroes(loadTavernWorkingHeroes(workingHeroShellsRef.current ?? undefined));
      void supportedShellsStatus()
        .then((shells) => {
          workingHeroShellsRef.current = shells;
          setWorkingHeroes(loadTavernWorkingHeroes(shells));
        })
        .catch(() => {});
    };
    window.addEventListener(TAVERN_PROFILE_CHANGED_EVENT, refreshWorkingHeroes);
    window.addEventListener('kota-v2:user-hero-avatars-changed', refreshWorkingHeroes);
    void syncTavernHeroStorageFromDisk()
      .then((heroes) => {
        setWorkingHeroes(heroes);
        return supportedShellsStatus();
      })
      .then((shells) => {
        workingHeroShellsRef.current = shells;
        setWorkingHeroes(loadTavernWorkingHeroes(shells));
      })
      .catch(() => refreshWorkingHeroes());
    return () => {
      window.removeEventListener(TAVERN_PROFILE_CHANGED_EVENT, refreshWorkingHeroes);
      window.removeEventListener('kota-v2:user-hero-avatars-changed', refreshWorkingHeroes);
    };
  }, []);

  const handleInputChange = useCallback((next: string) => {
    setInputText(next);
  }, []);

  const unavailableHeroIds = useMemo(() => new Set<AgentId>(), []);
  const roomAgentIds = useMemo(() => {
    const ids = new Set<AgentId>();
    for (const id of agentLayout.tableSlots) {
      if (id) ids.add(id);
    }
    return ids;
  }, [agentLayout.tableSlots]);

  // Keep PTYs globally alive, but render/route only the current project's
  // agents in this room. Switching project tabs should hide old sessions,
  // not close them.
  const activeLiveAgents = useMemo(() => {
    const next = new Set<AgentId>();
    for (const id of agentRuntime.liveAgents) {
      if (roomAgentIds.has(id)) next.add(id);
    }
    return next;
  }, [agentRuntime.liveAgents, roomAgentIds]);
  const activeLiveAgentsOrdered = useMemo(
    () => Array.from(activeLiveAgents),
    [activeLiveAgents],
  );
  // Indicator liveness for ribbon pills + round-table seats: an agent counts as
  // "live" if THIS frontend recruited it (activeLiveAgents) OR the backend
  // reports a running PTY for it (pty_agent_list → agentPtySummaries). The
  // recruit-only set still drives window rendering (which needs a live grid
  // subscription); this augmented set only lights the live/grey dot, so a PTY
  // that survived a webview reload / session takeover isn't shown as offline.
  // The signature string keeps the Set reference stable across the 2s PTY poll
  // when membership is unchanged, so Stage doesn't re-render every tick.
  const runningPtyAgentSig = useMemo(() => {
    const ids: AgentId[] = [];
    for (const summary of agentPtySummaries) {
      if (summary.running && roomAgentIds.has(summary.agentId as AgentId)) {
        ids.push(summary.agentId as AgentId);
      }
    }
    return ids.sort().join(',');
  }, [agentPtySummaries, roomAgentIds]);
  const activeRunningAgents = useMemo(() => {
    const next = new Set<AgentId>(activeLiveAgents);
    if (runningPtyAgentSig) {
      for (const id of runningPtyAgentSig.split(',')) next.add(id as AgentId);
    }
    return next;
  }, [activeLiveAgents, runningPtyAgentSig]);
  const activeWorkingAgents = useMemo(() => {
    const next = new Set<AgentId>();
    for (const [id, work] of agentRuntime.workState) {
      if (
        roomAgentIds.has(id) &&
        (work.state === 'working' || work.state === 'maybeIdle')
      ) {
        next.add(id);
      }
    }
    return next;
  }, [agentRuntime.workState, roomAgentIds]);
  const activeWorkingStartedAt = useMemo(() => {
    const next = new Map<AgentId, string>();
    for (const [id, work] of agentRuntime.workState) {
      if (!activeWorkingAgents.has(id)) continue;
      next.set(id, work.startedAt ?? work.timestamp);
    }
    return next;
  }, [activeWorkingAgents, agentRuntime.workState]);
  const activeWorkingAgentIds = useMemo(
    () => Array.from(activeWorkingAgents).sort(),
    [activeWorkingAgents],
  );
  const activeWorkingAgentKey = activeWorkingAgentIds.join('|');
  const globalWorkingAgentIds = useMemo(() => {
    const ids: AgentId[] = [];
    for (const [id, work] of agentRuntime.workState) {
      if (work.state === 'working' || work.state === 'maybeIdle') ids.push(id);
    }
    ids.sort();
    return ids;
  }, [agentRuntime.workState]);
  const globalWorkingAgentKey = globalWorkingAgentIds.join('|');
  useEffect(() => {
    if (!hasTauriRuntime()) return;
    void lmUpdateWorkingAgents(globalWorkingAgentIds).catch((err) => {
      console.warn('[laughing-man] working status sync failed', err);
    });
  }, [globalWorkingAgentKey]);

  const emberSchedulerProjectRoots = useMemo(() => (
    visibleWorkspaceTabs
      .filter((workspace) => !workspace.archived && workspace.localRoot)
      .map((workspace) => workspace.localRoot)
      .sort()
  ), [visibleWorkspaceTabs]);
  const emberSchedulerProjectRootKey = emberSchedulerProjectRoots.join('|');
  const handleDreamingStatusAgentsChange = useCallback((agentIds: readonly AgentId[]) => {
    setDreamingStatusAgents((current) => {
      const nextIds = Array.from(new Set(agentIds.filter((agentId) => roomAgentIds.has(agentId)))).sort();
      const currentIds = Array.from(current).sort();
      if (
        currentIds.length === nextIds.length &&
        currentIds.every((agentId, index) => agentId === nextIds[index])
      ) {
        return current;
      }
      return new Set(nextIds);
    });
  }, [roomAgentIds]);
  useEffect(() => {
    setDreamingStatusAgents((current) => {
      const nextIds = Array.from(current).filter((agentId) => roomAgentIds.has(agentId)).sort();
      if (nextIds.length === current.size) return current;
      return new Set(nextIds);
    });
  }, [roomAgentIds]);
  const roomAgentIdsOrdered = useMemo(
    () => Array.from(roomAgentIds).sort(),
    [roomAgentIds],
  );
  const chatFilterTargetAgents = useMemo(() => {
    if (composerBroadcast) {
      return Array.from(broadcastRecipients).filter((id) => roomAgentIds.has(id));
    }
    return targetAgent && roomAgentIds.has(targetAgent) ? [targetAgent] : [];
  }, [broadcastRecipients, composerBroadcast, roomAgentIds, targetAgent]);
  const roomAgentIdsKey = roomAgentIdsOrdered.join('|');
  const [groupChatOpen, setGroupChatOpen] = useState(false);
  const [chatFilterActive, setChatFilterActive] = useState(false);
  const [chatFilterOpenRequest, setChatFilterOpenRequest] = useState<{
    agentId: AgentId;
    nonce: number;
  } | null>(null);
  const [appForeground, setAppForeground] = useState(() => (
    typeof document === 'undefined' ||
    (document.visibilityState === 'visible' && document.hasFocus())
  ));
  const [violetUnreadState, setVioletUnreadState] = useState<VioletUnreadState>(() =>
    loadVioletUnreadState(),
  );
  const violetUnreadCount = useMemo(
    () => unreadTotal(violetUnreadState),
    [violetUnreadState],
  );
  const violetUnreadByProjectId = useMemo(() => {
    const next: Record<ProjectId, number> = {};
    for (const workspace of visibleWorkspaceTabs) {
      const projectUnread = violetUnreadState[violetProjectUnreadKey(workspace.localRoot)]?.unreadIds.length ?? 0;
      if (projectUnread > 0) next[workspace.projectId] = projectUnread;
    }
    return next;
  }, [violetUnreadState, visibleWorkspaceTabs]);
  const activeVioletUnreadCount = violetUnreadState[violetProjectUnreadKey(violetProjectRoot)]?.unreadIds.length ?? 0;
  const activeVioletUnreadAgentIds = useMemo(
    () => new Set(violetUnreadState[violetProjectUnreadKey(violetProjectRoot)]?.unreadAgentIds ?? []),
    [violetUnreadState, violetProjectRoot],
  );
  const violetRoomVisible = groupChatOpen && appForeground;
  const toggleGroupChat = useCallback(() => {
    setGroupChatOpen((v) => !v);
  }, []);
  const openAgentFilteredRoom = useCallback((id: AgentId) => {
    setComposerTarget(id);
    setComposerBroadcast(false);
    setBroadcastPopupOpen(false);
    setBroadcastRecipients(new Set());
    setTerminalFocusedAgent(null);
    setGroupChatOpen(true);
    setChatFilterActive(true);
    setChatFilterOpenRequest((prev) => ({
      agentId: id,
      nonce: (prev?.nonce ?? 0) + 1,
    }));
  }, []);

  useEffect(() => {
    const computeForeground = () => {
      setAppForeground(document.visibilityState === 'visible' && document.hasFocus());
    };
    let cancelled = false;
    let unlistenFocus: (() => void) | null = null;
    window.addEventListener('focus', computeForeground);
    window.addEventListener('blur', computeForeground);
    document.addEventListener('visibilitychange', computeForeground);
    computeForeground();
    if (hasTauriRuntime()) {
      void import('@tauri-apps/api/window')
        .then(({ getCurrentWindow }) => getCurrentWindow().onFocusChanged(({ payload }) => {
          if (!cancelled) setAppForeground(document.visibilityState === 'visible' && payload);
        }))
        .then((unlisten) => {
          if (cancelled) {
            unlisten();
            return;
          }
          unlistenFocus = unlisten;
        })
        .catch(() => {});
    }
    return () => {
      cancelled = true;
      window.removeEventListener('focus', computeForeground);
      window.removeEventListener('blur', computeForeground);
      document.removeEventListener('visibilitychange', computeForeground);
      if (unlistenFocus) unlistenFocus();
    };
  }, []);

  useEffect(() => {
    persistVioletUnreadState(violetUnreadState);
  }, [violetUnreadState]);

  useEffect(() => {
    const projectKey = violetProjectUnreadKey(violetProjectRoot);
    setVioletUnreadState((prev) => {
      if (prev[projectKey]) return prev;
      return {
        ...prev,
        [projectKey]: { lastReadAt: new Date().toISOString(), unreadIds: [], unreadAgentIds: [] },
      };
    });
  }, [violetProjectRoot]);

  useEffect(() => {
    if (!hasTauriRuntime()) return;
    let cancelled = false;
    void import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) => {
        if (cancelled) return;
        return getCurrentWindow().setBadgeCount(
          violetUnreadCount > 0 ? violetUnreadCount : undefined,
        );
      })
      .catch((err) => {
        console.warn('Failed to update Violet unread Dock badge', err);
      });
    return () => {
      cancelled = true;
    };
  }, [violetUnreadCount]);

  useEffect(() => {
    if (!violetRoomVisible) return;
    const projectKey = violetProjectUnreadKey(violetProjectRoot);
    setVioletUnreadState((prev) => markVioletProjectRead(prev, projectKey));
  }, [violetRoomVisible, violetProjectRoot]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void onVioletRoomSynced(({ request, state }) => {
      if (cancelled) return;
      const projectKey = violetProjectUnreadKey(request.projectRoot ?? violetProjectRoot);
      const currentProjectKey = violetProjectUnreadKey(violetProjectRoot);
      setVioletUnreadState((prev) => reduceVioletUnreadOnSync(
        prev,
        projectKey,
        state.messages,
        violetRoomVisible && projectKey === currentProjectKey,
      ));
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
  }, [violetRoomVisible, violetProjectRoot]);

  useEffect(() => {
    if (!violetProjectRoot) return;
    const handle = connectVioletProjectSyncEngine({
      projectRoot: violetProjectRoot,
      roomAgentIds: roomAgentIdsOrdered,
      workingAgentIds: activeWorkingAgentIds,
      foreground: violetRoomVisible,
    });
    violetSyncRef.current = handle;
    return () => {
      handle.dispose();
      if (violetSyncRef.current === handle) violetSyncRef.current = null;
    };
  }, [violetProjectRoot]);

  useEffect(() => {
    violetSyncRef.current?.update({
      projectRoot: violetProjectRoot,
      roomAgentIds: roomAgentIdsOrdered,
      workingAgentIds: activeWorkingAgentIds,
      foreground: violetRoomVisible,
    });
  }, [
    activeWorkingAgentKey,
    activeWorkingAgentIds,
    violetRoomVisible,
    roomAgentIdsKey,
    roomAgentIdsOrdered,
    violetProjectRoot,
  ]);

  useEffect(() => {
    if (!hasTauriRuntime()) {
      setLmSelectedForSync(null);
      return undefined;
    }
    let cancelled = false;
    const refreshLmSelected = async () => {
      try {
        const status = await lmStatus();
        if (cancelled) return;
        const next = status?.enabled &&
          status.running &&
          status.ownerUserId &&
          status.selected &&
          !status.selected.muted
          ? status.selected
          : null;
        setLmSelectedForSync((current) => (
          sameLmSelectedForSync(current, next) ? current : next
        ));
      } catch (err) {
        console.warn('[laughing-man] selected project sync status failed', err);
      }
    };
    void refreshLmSelected();
    const timer = window.setInterval(() => {
      void refreshLmSelected();
    }, 5000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, []);

  const lmSelectedWorkspace = useMemo(
    () => findLmSelectedWorkspace(visibleWorkspaceTabs, lmSelectedForSync),
    [lmSelectedForSync, visibleWorkspaceTabs],
  );
  const lmSelectedProjectAgentIds = useMemo(
    () => lmSelectedWorkspaceAgentIds(lmSelectedWorkspace, lmSelectedForSync),
    [lmSelectedForSync, lmSelectedWorkspace],
  );
  const lmSelectedProjectAgentKey = lmSelectedProjectAgentIds.join('|');
  const lmSelectedWorkingAgentIds = useMemo(() => (
    lmSelectedProjectAgentIds.filter((agentId) => {
      const work = agentRuntime.workState.get(agentId);
      return work?.state === 'working' || work?.state === 'maybeIdle';
    })
  ), [agentRuntime.workState, lmSelectedProjectAgentIds]);
  const lmSelectedWorkingAgentKey = lmSelectedWorkingAgentIds.join('|');

  // LM background sync is split deliberately: projectRoot owns the
  // subscription lifetime, while agent/working changes update that same
  // subscription without tearing down the shared project watcher.
  useEffect(() => {
    const projectRoot = lmSelectedForSync?.projectRoot ?? null;
    if (!projectRoot) return undefined;
    const handle = connectVioletProjectSyncEngine({
      projectRoot,
      roomAgentIds: lmSelectedProjectAgentIds,
      workingAgentIds: lmSelectedWorkingAgentIds,
      foreground: false,
    });
    lmVioletSyncRef.current = handle;
    return () => {
      handle.dispose();
      if (lmVioletSyncRef.current === handle) lmVioletSyncRef.current = null;
    };
  }, [lmSelectedForSync?.projectRoot]);

  useEffect(() => {
    const projectRoot = lmSelectedForSync?.projectRoot ?? null;
    if (!projectRoot) return;
    lmVioletSyncRef.current?.update({
      projectRoot,
      roomAgentIds: lmSelectedProjectAgentIds,
      workingAgentIds: lmSelectedWorkingAgentIds,
      foreground: false,
    });
  }, [
    lmSelectedForSync?.projectRoot,
    lmSelectedProjectAgentKey,
    lmSelectedProjectAgentIds,
    lmSelectedWorkingAgentKey,
    lmSelectedWorkingAgentIds,
  ]);

  useEffect(() => {
    if (!hasTauriRuntime() || emberSchedulerProjectRoots.length === 0) return undefined;
    let cancelled = false;
    const tick = async () => {
      if (cancelled || emberSchedulerInFlightRef.current) return;
      emberSchedulerInFlightRef.current = true;
      try {
        await emberSchedulerTick({
          projectRoots: emberSchedulerProjectRoots,
          workingAgentIds: globalWorkingAgentIds,
        });
      } catch (err) {
        console.warn('[ember] scheduler tick failed', err);
      } finally {
        emberSchedulerInFlightRef.current = false;
      }
    };
    void tick();
    const timer = window.setInterval(() => {
      void tick();
    }, 10_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [emberSchedulerProjectRootKey, emberSchedulerProjectRoots, globalWorkingAgentIds, globalWorkingAgentKey]);

  // W4 — taskbar state. App owns these so AgentRibbon and
  // AgentWindowsLayer can both reflect them, and Ctrl/⌘1-9 can dispatch
  // through both at once.
  const [minimized, setMinimized] = useState<Set<AgentId>>(() => new Set());
  const roomUiSnapshotsRef = useRef<Record<ProjectId, RoomUiSnapshot>>({});
  const windowsRef = useRef<AgentWindowsLayerHandle | null>(null);
  const inputBarRef = useRef<InputBarHandle | null>(null);
  const pendingComposerFocusRef = useRef<number | null>(null);

  // Composer focus + explicit routing controls. Textarea content never
  // routes via @mentions; target state is owned by App chrome.
  const clearPendingComposerFocus = useCallback(() => {
    if (pendingComposerFocusRef.current == null) return;
    window.clearTimeout(pendingComposerFocusRef.current);
    pendingComposerFocusRef.current = null;
  }, []);
  const scheduleComposerFocus = useCallback((focus: () => void) => {
    clearPendingComposerFocus();
    pendingComposerFocusRef.current = window.setTimeout(() => {
      pendingComposerFocusRef.current = null;
      focus();
    }, 0);
  }, [clearPendingComposerFocus]);
  const focusComposerSoon = useCallback(() => {
    scheduleComposerFocus(() => inputBarRef.current?.focus());
  }, [scheduleComposerFocus]);
  const focusComposerEndSoon = useCallback(() => {
    scheduleComposerFocus(() => inputBarRef.current?.focusEnd());
  }, [scheduleComposerFocus]);
  useEffect(() => clearPendingComposerFocus, [clearPendingComposerFocus]);
  const insertComposerAttachment = useCallback((attachment: ComposerAttachment) => {
    inputBarRef.current?.insertAttachment(attachment);
    focusComposerEndSoon();
  }, [focusComposerEndSoon]);

  // Restoring a terminal only affects terminal visibility. Composer routing is
  // owned by selectComposerTarget so target switching never pops a TUI.
  const restoreAgent = useCallback((id: AgentId) => {
    setMinimized((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
  }, []);

  const minimizeAgentFromWindow = useCallback((id: AgentId) => {
    setMinimized((prev) => {
      if (prev.has(id)) return prev;
      const next = new Set(prev);
      next.add(id);
      return next;
    });
    setTerminalFocusedAgent((prev) => (prev === id ? null : prev));
    focusComposerEndSoon();
  }, [focusComposerEndSoon]);

  const persistAgentPrivacy = useCallback((id: AgentId, isPrivate: boolean) => {
    void setVioletPrivacy({
      projectRoot: projectAgentRoot,
      agentId: id,
      private: isPrivate,
    }).catch((err) => {
      console.warn('Failed to update Violet privacy span', err);
    });
  }, [projectAgentRoot]);
  const togglePrivacyMode = useCallback(() => {
    if (!PRIVATE_CHAT_UI_ENABLED) return;
    if (!composerTarget) return;
    setPrivateAgents((prev) => {
      const next = new Set(prev);
      const isPrivate = !next.has(composerTarget);
      if (isPrivate) next.add(composerTarget);
      else next.delete(composerTarget);
      persistAgentPrivacy(composerTarget, isPrivate);
      return next;
    });
    focusComposerSoon();
  }, [composerTarget, focusComposerSoon, persistAgentPrivacy]);
  const togglePrivacyAgent = useCallback((id: AgentId) => {
    if (!PRIVATE_CHAT_UI_ENABLED) return;
    setPrivateAgents((prev) => {
      const next = new Set(prev);
      const isPrivate = !next.has(id);
      if (isPrivate) next.add(id);
      else next.delete(id);
      persistAgentPrivacy(id, isPrivate);
      return next;
    });
  }, [persistAgentPrivacy]);
  const toggleAllPrivacy = useCallback(() => {
    if (!PRIVATE_CHAT_UI_ENABLED) return;
    setPrivateAgents((prev) => {
      const next = new Set(prev);
      const allPrivate = onTableAgents.length > 0 && onTableAgents.every((id) => next.has(id));
      for (const id of onTableAgents) {
        const isPrivate = !allPrivate;
        if (isPrivate) next.add(id);
        else next.delete(id);
        persistAgentPrivacy(id, isPrivate);
      }
      return next;
    });
  }, [onTableAgents, persistAgentPrivacy]);
  const minimizeAllAgents = useCallback(() => {
    setMinimized(new Set(activeLiveAgents));
    setTerminalFocusedAgent(null);
    focusComposerEndSoon();
  }, [activeLiveAgents, focusComposerEndSoon]);

  const resetRoomUiState = useCallback((provisionalLayout?: AgentLayoutState | null) => {
    hydratedAgentLayoutProjectRef.current = null;
    setHydratedLayoutProjectId(null);
    setMinimized(new Set());
    setTerminalFocusedAgent(null);
    setComposerTarget(null);
    setComposerBroadcast(false);
    setBroadcastPopupOpen(false);
    setBroadcastRecipients(new Set());
    setPrivateAgents(new Set());
    setAgentLayout(provisionalLayout ?? {
      tableSlots: [...DEFAULT_TABLE_SLOTS],
      offTableAgents: [...DEFAULT_OFF_TABLE_AGENTS],
    });
    setAgentInstances({});
    setProjectAgentIdentities([]);
    setAgentRecords({});
    setProjectAgentDetailId(null);
    setArchiveOpen(false);
    setGroupChatOpen(false);
    setChatFilterActive(false);
  }, []);

  const currentRoomUiSnapshot = useCallback((): RoomUiSnapshot => ({
    minimized: [...minimized],
    terminalFocusedAgent,
    composerTarget,
    composerBroadcast,
    broadcastPopupOpen,
    broadcastRecipients: [...broadcastRecipients],
    privateAgents: [...privateAgents],
    agentLayout: {
      tableSlots: normalizeAgentTableSlots(agentLayout.tableSlots),
      offTableAgents: [],
    },
    groupChatOpen,
    chatFilterActive,
  }), [
    agentLayout,
    broadcastPopupOpen,
    broadcastRecipients,
    chatFilterActive,
    composerBroadcast,
    composerTarget,
    groupChatOpen,
    minimized,
    privateAgents,
    terminalFocusedAgent,
  ]);

  const saveActiveRoomUiSnapshot = useCallback(() => {
    const projectId = activeWorkspace?.projectId;
    if (!projectId) return;
    roomUiSnapshotsRef.current[projectId] = currentRoomUiSnapshot();
  }, [activeWorkspace?.projectId, currentRoomUiSnapshot]);

  const restoreRoomUiSnapshot = useCallback((snapshot: RoomUiSnapshot | undefined, workspace?: WorkspaceProject | null) => {
    hydratedAgentLayoutProjectRef.current = null;
    setHydratedLayoutProjectId(null);
    if (!snapshot) {
      resetRoomUiState(provisionalLayoutForWorkspace(workspace));
      return;
    }
    setMinimized(new Set(snapshot.minimized));
    setTerminalFocusedAgent(snapshot.terminalFocusedAgent);
    setComposerTarget(snapshot.composerTarget);
    setComposerBroadcast(snapshot.composerBroadcast);
    setBroadcastPopupOpen(snapshot.broadcastPopupOpen);
    setBroadcastRecipients(new Set(snapshot.broadcastRecipients));
    setPrivateAgents(new Set(snapshot.privateAgents));
    setAgentLayout({
      tableSlots: normalizeAgentTableSlots(snapshot.agentLayout.tableSlots),
      offTableAgents: [],
    });
    setAgentInstances({});
    setProjectAgentIdentities([]);
    setAgentRecords({});
    setProjectAgentDetailId(null);
    setArchiveOpen(false);
    setGroupChatOpen(snapshot.groupChatOpen);
    setChatFilterActive(snapshot.chatFilterActive ?? true);
  }, [resetRoomUiState]);

  const dismissAgentSessions = useCallback(async (agentIds: readonly AgentId[]) => {
    const ids = [...new Set(agentIds)];
    if (ids.length === 0) return;
    const idSet = new Set(ids);
    await Promise.all(ids.map((id) => agentRuntime.dismiss(id).catch(() => {})));
    await refreshAgentPtySummaries();
    setMinimized((prev) => {
      if (![...prev].some((id) => idSet.has(id))) return prev;
      return new Set([...prev].filter((id) => !idSet.has(id)));
    });
    setTerminalFocusedAgent((prev) => (prev && idSet.has(prev) ? null : prev));
  }, [agentRuntime, refreshAgentPtySummaries]);

  const adoptWorkspace = useCallback((workspace: WorkspaceProject) => {
    rememberWorkspaceTab(workspace);
    setActiveWorkspace(workspace);
    setActiveProjectId(workspace.projectId);
    setProjectSetupOpen(false);
    refreshFileTree();
  }, [refreshFileTree, rememberWorkspaceTab]);

  const switchToWorkspace = useCallback((workspace: WorkspaceProject) => {
    const switchingProjects = activeWorkspace?.projectId !== workspace.projectId;
    if (switchingProjects) {
      saveActiveRoomUiSnapshot();
      restoreRoomUiSnapshot(roomUiSnapshotsRef.current[workspace.projectId], workspace);
    }
    adoptWorkspace(workspace);
  }, [
    activeWorkspace?.projectId,
    adoptWorkspace,
    restoreRoomUiSnapshot,
    saveActiveRoomUiSnapshot,
  ]);

  const handleWorkspacePrepared = useCallback((workspace: WorkspaceProject) => {
    void (async () => {
      switchToWorkspace(workspace);
      focusComposerEndSoon();
    })().catch((err) => {
      window.alert(`Open project failed: ${String(err)}`);
    });
  }, [focusComposerEndSoon, switchToWorkspace]);

  const handleSelectProject = useCallback((projectId: ProjectId) => {
    if (activeWorkspace?.projectId === projectId) {
      setActiveProjectId(projectId);
      refreshFileTree();
      return;
    }
    const cachedWorkspace = visibleWorkspaceTabs.find((workspace) => workspace.projectId === projectId);
    if (cachedWorkspace) {
      switchToWorkspace(cachedWorkspace);
      focusComposerEndSoon();
      void openWorkspaceProject(projectId).catch((err) => {
        window.alert(`Open project failed: ${String(err)}`);
      });
      return;
    }
    void (async () => {
      const workspace = await openWorkspaceProject(projectId);
      switchToWorkspace(workspace);
      focusComposerEndSoon();
    })().catch((err) => {
      window.alert(`Open project failed: ${String(err)}`);
    });
  }, [
    activeWorkspace?.projectId,
    focusComposerEndSoon,
    refreshFileTree,
    switchToWorkspace,
    visibleWorkspaceTabs,
  ]);

  const handleArchiveProjectTab = useCallback(async (projectId: ProjectId) => {
    const workspace = visibleWorkspaceTabs.find((item) => item.projectId === projectId);
    if (!workspace) return;
    try {
      let status = { dirty: false, dirtySummary: '' };
      try {
        status = await inspectWorkspaceProject(projectId);
      } catch (err) {
        status = {
          dirty: true,
          dirtySummary: `Could not verify Git sync status: ${String(err)}`,
        };
      }
      const body = status.dirty
        ? `Archive ${workspace.repoFullName}? It will disappear from the project tabs and all running agents for this project will stop.\n\nUnsynced or dirty Git state was detected:\n\n${status.dirtySummary}`
        : `Archive ${workspace.repoFullName}? It will disappear from the project tabs and all running agents for this project will stop. Files stay on disk.`;
      const confirmed = await confirmInApp(
        'Archive project?',
        body,
        {
          confirmLabel: 'Archive project',
          tone: status.dirty ? 'danger' : undefined,
        },
      );
      if (!confirmed) return;
      const result = await archiveWorkspaceProject({
        projectId,
        forceDirty: status.dirty,
      });
      if (!result.ok) {
        window.alert(result.dirtySummary || 'Project archive did not complete.');
        return;
      }
      const remainingTabs = visibleWorkspaceTabs.filter((item) => item.projectId !== projectId);
      forgetWorkspaceTab(projectId);
      const closingActiveProject = activeWorkspace?.projectId === projectId;
      await dismissAgentSessions(workspace.agents.map((agent) => agent.agentId as AgentId));
      if (closingActiveProject) {
        resetRoomUiState();
        const nextWorkspace = remainingTabs[0] ?? null;
        if (nextWorkspace) {
          try {
            adoptWorkspace(await openWorkspaceProject(nextWorkspace.projectId));
          } catch (err) {
            console.warn('[workspace] could not activate next project after archive', err);
            setActiveWorkspace(null);
            setActiveProjectId(DEFAULT_PROJECT_ID);
          }
        } else {
          setActiveWorkspace(null);
          setActiveProjectId(DEFAULT_PROJECT_ID);
        }
      }
      setProjectSetupOpen(false);
      refreshFileTree();
      focusComposerEndSoon();
    } catch (err) {
      window.alert(`Archive project failed: ${String(err)}`);
    }
  }, [
    activeWorkspace?.projectId,
    adoptWorkspace,
    confirmInApp,
    dismissAgentSessions,
    focusComposerEndSoon,
    forgetWorkspaceTab,
    refreshFileTree,
    resetRoomUiState,
    visibleWorkspaceTabs,
  ]);

  const selectComposerTarget = useCallback((id: AgentId) => {
    setComposerTarget(id);
    setComposerBroadcast(false);
    setBroadcastPopupOpen(false);
    setTerminalFocusedAgent(null);
    focusComposerSoon();
  }, [focusComposerSoon]);
  const openTargetPicker = useCallback(() => {
    setBroadcastRecipients(() => {
      if (composerBroadcast) return new Set(broadcastRecipients);
      return composerTarget ? new Set([composerTarget]) : new Set();
    });
    setBroadcastPopupOpen(true);
    setTerminalFocusedAgent(null);
    focusComposerSoon();
  }, [broadcastRecipients, composerBroadcast, composerTarget, focusComposerSoon]);
  const toggleBroadcastRecipient = useCallback((id: AgentId) => {
    setBroadcastRecipients((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const confirmBroadcastRecipients = useCallback(() => {
    setBroadcastPopupOpen(false);
    if (broadcastRecipients.size > 1) {
      setComposerBroadcast(true);
    } else if (broadcastRecipients.size === 1) {
      const only = [...broadcastRecipients][0]!;
      setComposerTarget(only);
      setComposerBroadcast(false);
      setBroadcastRecipients(new Set());
    } else {
      setComposerTarget(null);
      setComposerBroadcast(false);
    }
    setTerminalFocusedAgent(null);
    focusComposerSoon();
  }, [broadcastRecipients, focusComposerSoon]);
  const cancelBroadcastMode = useCallback(() => {
    setBroadcastPopupOpen(false);
    setComposerBroadcast(false);
    setBroadcastRecipients(new Set());
    focusComposerSoon();
  }, [focusComposerSoon]);
  const clearBroadcastRecipients = useCallback(() => {
    setBroadcastRecipients(new Set());
  }, []);

  const syncComposerTargetForTerminalFocus = useCallback((id: AgentId) => {
    if (groupChatOpen || composerBroadcast || broadcastPopupOpen) return;
    setComposerTarget(id);
  }, [broadcastPopupOpen, composerBroadcast, groupChatOpen]);

  const focusAgent = useCallback((id: AgentId) => {
    clearPendingComposerFocus();
    restoreAgent(id);
    setTerminalFocusedAgent(id);
    syncComposerTargetForTerminalFocus(id);
    windowsRef.current?.bringToFront(id);
  }, [clearPendingComposerFocus, restoreAgent, syncComposerTargetForTerminalFocus]);

  const swapAgentSeats = useCallback((fromIndex: number, toIndex: number) => {
    if (fromIndex === toIndex) return;
    setAgentLayout((prev) => {
      const slots = [...prev.tableSlots];
      if (
        fromIndex < 0 || toIndex < 0 ||
        fromIndex >= slots.length || toIndex >= slots.length ||
        !slots[fromIndex]
      ) return prev;
      const displaced = slots[toIndex];
      slots[toIndex] = slots[fromIndex];
      slots[fromIndex] = displaced;
      return { ...prev, tableSlots: slots };
    });
  }, []);

  const removeAgentFromRoom = useCallback((id: AgentId) => {
    setAgentLayout((prev) => ({
      tableSlots: prev.tableSlots.map((slot) => (slot === id ? null : slot)),
      offTableAgents: prev.offTableAgents.filter((agentId) => agentId !== id),
    }));
    setAgentInstances((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setAgentRecords((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
    setPrivateAgents((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
    setComposerTarget((prev) => (prev === id ? null : prev));
    setBroadcastRecipients((prev) => {
      if (!prev.has(id)) return prev;
      const next = new Set(prev);
      next.delete(id);
      return next;
    });
  }, []);

  const openAgentContextMenu = useCallback((
    id: AgentId,
    point: { x: number; y: number },
    source: ProjectAgentCommendSource = 'agent-bar',
  ) => {
    if (agentsHydrating) return;
    const x = Math.max(8, Math.min(point.x, window.innerWidth - 220));
    const y = Math.max(8, Math.min(point.y, window.innerHeight - 190));
    setAgentContextMenu({ agentId: id, x, y, source });
  }, [agentsHydrating]);

  const toggleAgentPrivateFromMenu = useCallback((id: AgentId) => {
    if (!PRIVATE_CHAT_UI_ENABLED) return;
    setComposerTarget(id);
    setComposerBroadcast(false);
    setBroadcastPopupOpen(false);
    setPrivateAgents((prev) => {
      const next = new Set(prev);
      const isPrivate = !next.has(id);
      if (isPrivate) next.add(id);
      else next.delete(id);
      persistAgentPrivacy(id, isPrivate);
      return next;
    });
    focusComposerSoon();
  }, [focusComposerSoon, persistAgentPrivacy]);

  const handleCommendAgent = useCallback((id: AgentId, source: ProjectAgentCommendSource) => {
    if (agentsHydrating) return;
    const previous = agentRecordsRef.current[id];
    const baseline = previous ?? {
      turns: 0,
      incarnations: 1,
      estimatedTokens: 0,
      commends: 0,
      lastActiveAt: null,
    };
    setAgentRecords((prev) => ({
      ...prev,
      [id]: {
        ...baseline,
        commends: (baseline.commends ?? 0) + 1,
        lastActiveAt: new Date().toISOString(),
      },
    }));
    void commendProjectAgent({
      agentId: id,
      projectRoot: projectAgentRoot,
      source,
    })
      .then((record) => {
        setAgentRecords((prev) => ({ ...prev, [id]: record }));
        window.dispatchEvent(new Event(TAVERN_HERO_CREDIT_CHANGED_EVENT));
      })
      .catch((err) => {
        console.warn('[kota-credit] commend failed', err);
        setAgentRecords((prev) => {
          if (previous) return { ...prev, [id]: previous };
          const next = { ...prev };
          delete next[id];
          return next;
        });
      });
  }, [agentsHydrating, projectAgentRoot]);

  // Seat / ribbon: single-click = switch target; double-click opens the terminal.
  const handleSeatClick = useCallback((id: AgentId) => {
    if (agentsHydrating) return;
    if (composerBroadcast) return;
    selectComposerTarget(id);
  }, [agentsHydrating, composerBroadcast, selectComposerTarget]);

  const handleRibbonAgentClick = useCallback(async (id: AgentId) => {
    if (agentsHydrating) return;
    if (!composerBroadcast) {
      selectComposerTarget(id);
      return;
    }
    const confirmed = await confirmInApp(
      'Exit Broadcast?',
      `Switch to ${agentName(id)} as the only target?`,
      { confirmLabel: 'Yes', cancelLabel: 'No' },
    );
    if (!confirmed) {
      focusComposerSoon();
      return;
    }
    setBroadcastRecipients(new Set());
    selectComposerTarget(id);
  }, [agentName, agentsHydrating, composerBroadcast, confirmInApp, focusComposerSoon, selectComposerTarget]);

  const closeTavern = useCallback(() => {
    tavernPrepareSeqRef.current += 1;
    setTavernPreparing(false);
    setTavernPrepareItems([]);
    setTavernOpen(false);
  }, []);

  const openTavern = useCallback(() => {
    if (tavernOpen) {
      closeTavern();
      return;
    }
    if (tavernPreparing) return;
    const seq = tavernPrepareSeqRef.current + 1;
    tavernPrepareSeqRef.current = seq;
    setTavernInitialTab('heroes');
    setTavernPrepareItems([{ id: 'opening', label: 'Opening Tavern', startedAt: Date.now() }]);
    setTavernPreparing(true);
    const minimumGate = new Promise<void>((resolve) => {
      window.setTimeout(resolve, 160);
    });
    const preparation = waitForTavernOpeningGatePaint()
      .then(() => {
        if (tavernPrepareSeqRef.current !== seq) return undefined;
        return prepareTavernForOpen((items) => {
          if (tavernPrepareSeqRef.current !== seq) return;
          setTavernPrepareItems(items);
        });
      })
      .catch(() => undefined);
    void Promise.all([preparation, minimumGate]).finally(() => {
      window.requestAnimationFrame(() => {
        if (tavernPrepareSeqRef.current !== seq) return;
        setTavernOpen(true);
        setTavernPreparing(false);
        setTavernPrepareItems([]);
      });
    });
  }, [closeTavern, tavernOpen, tavernPreparing]);

  const placeRecruitedAgent = useCallback((id: AgentId, seatIndex?: number | null) => {
    setAgentLayout((prev) => {
      if (prev.tableSlots.includes(id) || prev.offTableAgents.includes(id)) return prev;
      const nextSlots = prev.tableSlots.slice(0, MAX_AGENT_SLOTS);
      const requestedIdx =
        typeof seatIndex === 'number' &&
        seatIndex >= 0 &&
        seatIndex < MAX_AGENT_SLOTS &&
        nextSlots[seatIndex] == null
          ? seatIndex
          : -1;
      const emptyIdx = requestedIdx >= 0 ? requestedIdx : nextSlots.findIndex((slot) => slot == null);
      if (emptyIdx >= 0) {
        nextSlots[emptyIdx] = id;
        return { tableSlots: nextSlots, offTableAgents: [] };
      }
      return { ...prev, tableSlots: nextSlots, offTableAgents: [] };
    });
  }, []);

  const openNextEmptySeatRecruitModal = useCallback(() => {
    if (agentsHydrating) return;
    if (tableFull || firstEmptySeatIndex < 0) return;
    setBroadcastPopupOpen(false);
    setComposerBroadcast(false);
    setRecruitSeatIndex(null);
    setShortcutRecruitSeatIndex(firstEmptySeatIndex);
    setTerminalFocusedAgent(null);
  }, [agentsHydrating, firstEmptySeatIndex, tableFull]);

  const openSpecificSeatRecruitModal = useCallback((seatIndex: number) => {
    if (agentsHydrating) return;
    if (seatIndex < 0 || seatIndex >= MAX_AGENT_SLOTS) return;
    if (agentLayout.tableSlots[seatIndex]) return;
    setBroadcastPopupOpen(false);
    setComposerBroadcast(false);
    setRecruitSeatIndex(null);
    setShortcutRecruitSeatIndex(seatIndex);
    setTerminalFocusedAgent(null);
  }, [agentLayout.tableSlots, agentsHydrating]);

  const applyProjectAgentDetail = useCallback((detail: ProjectAgentDetail) => {
    setProjectAgentIdentities((prev) => (
      upsertProjectAgentIdentity(prev, projectAgentIdentityFromDetail(detail))
    ));
    setAgentRecords((prev) => ({ ...prev, [detail.agentId]: detail.record }));
    setAgentInstances((prev) => {
      return {
        ...prev,
        [detail.agentId]: projectAgentDetailToWorkingHero(detail),
      };
    });
  }, []);

  useEffect(() => {
    if (!activeWorkspace) {
      hydratedAgentLayoutProjectRef.current = null;
      setHydratedLayoutProjectId(null);
      setAgentHydrationProgress(null);
      setProjectAgentIdentities([]);
      return;
    }
    let cancelled = false;
    const hydrationProjectId = activeWorkspace.projectId;
    setAgentHydrationProgress({ projectId: hydrationProjectId, completed: 0, total: 0 });
    setAgentLayout((prev) => {
      if (hydratedAgentLayoutProjectRef.current === activeWorkspace.projectId) return prev;
      if (prev.tableSlots.some(Boolean)) return prev;
      return provisionalLayoutForWorkspace(activeWorkspace) ?? prev;
    });

    const hydrateRoot = activeWorkspace.localRoot;
    void (async () => {
      let identities: ProjectAgentIdentity[] = [];
      let identityListLoaded = true;
      try {
        identities = await listProjectAgentIdentities(hydrateRoot);
      } catch (err) {
        identityListLoaded = false;
        console.warn('[kota-agent] could not hydrate project agents', err);
      }
      if (!cancelled) {
        setProjectAgentIdentities(identityListLoaded ? identities : []);
      }

      const displayableIdentities = identities.filter((identity) => (
        displayableProjectAgentStatus(identity.status)
      ));
      const displayableIdentityIds = new Set(displayableIdentities.map((identity) => identity.agentId));
      const workspaceAgents = identityListLoaded
        ? activeWorkspace.agents.filter((agent) => displayableIdentityIds.has(agent.agentId))
        : activeWorkspace.agents;
      const orderedIds = orderedProjectAgentIds(workspaceAgents, displayableIdentities);
      if (!cancelled) {
        setAgentHydrationProgress({
          projectId: hydrationProjectId,
          completed: 0,
          total: orderedIds.length,
        });
      }
      const detailResults = await Promise.allSettled(
        orderedIds.map(async (agentId) => {
          try {
            return await loadProjectAgentDetail({ agentId, projectRoot: hydrateRoot });
          } finally {
            if (!cancelled) {
              setAgentHydrationProgress((current) => {
                if (current?.projectId !== hydrationProjectId) return current;
                return {
                  ...current,
                  completed: Math.min(current.completed + 1, current.total),
                };
              });
            }
          }
        }),
      );
      if (cancelled) return;

      const details = detailResults
        .filter((result): result is PromiseFulfilledResult<ProjectAgentDetail> => result.status === 'fulfilled')
        .map((result) => result.value)
        .filter((detail) => displayableProjectAgentStatus(detail.status));
      const activeIds = details.map((detail) => detail.agentId as AgentId);
      const nextInstances = Object.fromEntries(
        details.map((detail) => [detail.agentId, projectAgentDetailToWorkingHero(detail)]),
      ) as Record<AgentId, WorkingHero>;
      const nextRecords = Object.fromEntries(
        details.map((detail) => [detail.agentId, detail.record]),
      ) as Record<AgentId, ProjectAgentRecord>;

      const savedLayoutFile = await loadProjectAgentLayoutFile(hydrateRoot).catch(() => null);
      if (cancelled) return;
      let savedSlots: readonly (string | null)[] | null = savedLayoutFile?.tableSlots ?? null;
      let migrateLegacySlots = false;
      if (!savedSlots) {
        // One-time migration from the legacy browser-local layout store.
        savedSlots = remapLegacySeatOrder(loadProjectAgentLayout(activeWorkspace.projectId)?.tableSlots ?? null);
        migrateLegacySlots = savedSlots != null;
      }
      const nextLayout = tableLayoutMergingSavedSlots(savedSlots, activeIds);
      if (migrateLegacySlots) {
        // Persist only the roster-filtered slots: the legacy store may carry
        // cross-project poisoning, which must not reach the new file store.
        void saveProjectAgentLayoutFile(hydrateRoot, nextLayout.tableSlots).catch(() => {});
      }
      hydratedAgentLayoutProjectRef.current = activeWorkspace.projectId;
      setHydratedLayoutProjectId(activeWorkspace.projectId);
      setAgentInstances(nextInstances);
      setAgentRecords(nextRecords);
      setAgentLayout(nextLayout);
      setComposerTarget((prev) => (prev && activeIds.includes(prev) ? prev : null));
      setBroadcastRecipients((prev) => new Set([...prev].filter((id) => activeIds.includes(id))));
      setPrivateAgents((prev) => new Set([...prev].filter((id) => activeIds.includes(id))));
    })();

    return () => {
      cancelled = true;
    };
  }, [activeWorkspace]);

  useEffect(() => {
    const projectId = activeWorkspace?.projectId ?? null;
    const localRoot = activeWorkspace?.localRoot ?? null;
    if (!projectId || !localRoot || hydratedAgentLayoutProjectRef.current !== projectId) return undefined;
    // Defense in depth: never persist slots naming agents outside the hydrated
    // roster (e.g. a stale cross-project hydrate) — see seat-order findings.
    const roster = new Set(Object.keys(agentInstances));
    if (agentLayout.tableSlots.some((id) => id && !roster.has(id))) return undefined;
    if (persistLayoutTimerRef.current != null) window.clearTimeout(persistLayoutTimerRef.current);
    persistLayoutTimerRef.current = window.setTimeout(() => {
      persistLayoutTimerRef.current = null;
      void saveProjectAgentLayoutFile(localRoot, agentLayout.tableSlots).catch(() => {});
    }, AGENT_LAYOUT_WRITE_DEBOUNCE_MS);
    return () => {
      if (persistLayoutTimerRef.current != null) {
        window.clearTimeout(persistLayoutTimerRef.current);
        persistLayoutTimerRef.current = null;
      }
    };
  }, [activeWorkspace?.projectId, activeWorkspace?.localRoot, agentInstances, agentLayout]);

  useEffect(() => {
    const ids = Object.keys(agentInstances) as AgentId[];
    if (ids.length === 0) return;
    let cancelled = false;
    void Promise.allSettled(
      ids.map((agentId) => loadProjectAgentDetail({ agentId, projectRoot: projectAgentRoot })),
    ).then((results) => {
      if (cancelled) return;
      setAgentRecords((prev) => {
        const next = { ...prev };
        results.forEach((result) => {
          if (result.status === 'fulfilled') next[result.value.agentId] = result.value.record;
        });
        return next;
      });
    });
    return () => {
      cancelled = true;
    };
  }, [agentInstances, projectAgentRoot]);
  const handleRecruitTest = useCallback(
    async (
      agentId: AgentId,
      cli: AgentCli,
      seatIndex?: number | null,
      incarnation?: IncarnationLaunchProfile,
      progress?: RecruitProgressOptions,
    ) => {
      let request: AgentSpawnRequest;
      if (incarnation) {
        try {
          const result = await incarnateTavernHero({
            agentId,
            templateId: incarnation.templateId,
            displayName: incarnation.displayName,
            projectRoot: activeWorkspace ? null : recruitProjectRoot,
            progressId: progress?.progressId,
            profile: incarnation.profile,
          });
          request = result.request;
          window.dispatchEvent(new Event(TAVERN_HERO_CREDIT_CHANGED_EVENT));
          if (!activeWorkspace && result.projectRoot && result.projectRoot !== recruitProjectRoot) {
            setRecruitProjectRoot(result.projectRoot);
            try {
              localStorage.setItem(LS_DEV_PROJECT_ROOT, result.projectRoot);
            } catch {
              // localStorage can be unavailable in test/webview edge cases.
            }
            console.info(`[kota-recruit] using project root ${result.projectRoot}`);
          }
          if (result.missingSkills.length > 0) {
            console.warn(
              `[kota-recruit] ${agentId} missing skills: ${result.missingSkills.join(', ')}`,
            );
          }
        } catch (err) {
          console.warn('[kota-recruit] incarnation materialization failed', err);
          progress?.onError?.(err);
          if (!progress?.suppressFailureAlert) notifyRecruitFailure(agentId, err);
          return false;
        }
      } else if (activeWorkspace) {
        try {
          request = await resolveWorkspaceAgentLaunch(agentId, cli);
        } catch (err) {
          console.warn('[kota-recruit] active workspace launch spec failed', err);
          progress?.onError?.(err);
          if (!progress?.suppressFailureAlert) notifyRecruitFailure(agentId, err);
          return false;
        }
      } else {
        let projectRoot: string;
        try {
          projectRoot = await resolveDevProjectRoot(recruitProjectRoot, agentId, cli);
          if (projectRoot !== recruitProjectRoot) {
            setRecruitProjectRoot(projectRoot);
            try {
              localStorage.setItem(LS_DEV_PROJECT_ROOT, projectRoot);
            } catch {
              // localStorage can be unavailable in test/webview edge cases.
            }
            console.info(`[kota-recruit] using project root ${projectRoot}`);
          }
        } catch (err) {
          console.warn('[kota-recruit] could not resolve a valid project root', err);
          progress?.onError?.(err);
          if (!progress?.suppressFailureAlert) notifyRecruitFailure(agentId, err);
          return false;
        }
        request = {
          agentId,
          cli,
          cwd: `${projectRoot}/.agent-workspaces/${agentId}`,
          projectRoot,
        };
      }

      console.log(`[kota-recruit] spawning ${cli} for ${agentId} in ${request.cwd}`);
      progress?.onStep?.('launch', 'Launching terminal.');
      try {
        const recruited = await recruitWithLeaseTakeover(request);
        if (!recruited) {
          progress?.onError?.(new Error('Agent launch was cancelled.'));
          return false;
        }
        await refreshAgentPtySummaries();
        console.log(`[kota-recruit] ✓ ${agentId} (${cli}) spawned`);
        // Make this agent the composer target. The terminal window still
        // owns direct keystroke input when the user clicks inside it.
        placeRecruitedAgent(agentId, seatIndex);
        setComposerTarget(agentId);
        setComposerBroadcast(false);
        setBroadcastPopupOpen(false);
        refreshFileTree();
        return true;
      } catch (err) {
        console.error(`[kota-recruit] ✗ ${agentId} (${cli}) failed:`, err);
        progress?.onError?.(err);
        if (!progress?.suppressFailureAlert) notifyRecruitFailure(agentId, err);
        return false;
      }
    },
    [activeWorkspace, placeRecruitedAgent, recruitProjectRoot, recruitWithLeaseTakeover, refreshAgentPtySummaries, refreshFileTree],
  );

  const handleIncarnateHero = useCallback(async (hero: WorkingHero, seatIndex?: number) => {
    const targetSeatIndex =
      typeof seatIndex === 'number' ? seatIndex : agentLayout.tableSlots.findIndex((slot) => slot == null);
    if (
      targetSeatIndex < 0 ||
      targetSeatIndex >= MAX_AGENT_SLOTS ||
      agentLayout.tableSlots[targetSeatIndex] != null
    ) {
      return;
    }
    const progressId = makeIncarnationProgressId();
    incarnationProgressIdRef.current = progressId;
    incarnationRetryPayloadRef.current = { hero, seatIndex: targetSeatIndex };
    clearIncarnationProgressHideTimer();
    setRecruitSeatIndex(null);
    setShortcutRecruitSeatIndex(null);
    setIncarnationProgress({
      id: progressId,
      heroName: hero.name,
      projectName: activeProjectName,
      stepId: 'profile',
      message: 'Preparing incarnation profile.',
      phase: 'running',
    });
    const templateId = hero.templateId ?? hero.id;
    let existingProjectAgents: ProjectAgentIdentity[] = [];
    try {
      existingProjectAgents = await listProjectAgentIdentities(projectAgentRoot);
    } catch (err) {
      console.warn('[kota-recruit] could not list existing project agents for naming', err);
    }
    const occupiedIds = new Set<AgentId>([
      ...existingProjectAgents.map((agent) => agent.agentId),
      ...Object.keys(agentInstances),
      ...agentLayout.tableSlots.filter((id): id is AgentId => !!id),
      ...agentLayout.offTableAgents,
      ...agentRuntime.liveAgents,
    ]);
    const reservedProjectAgentNames = existingProjectAgents
      .filter((agent) => {
        const status = agent.status.trim().toLowerCase();
        return status === 'active' || status === 'archived';
      })
      .map((agent) => agent.displayName.trim().toLowerCase());
    const occupiedNames = new Set<string>([
      ...reservedProjectAgentNames,
      ...Object.values(agentInstances).map((agent) => agent.name.trim().toLowerCase()),
    ].filter(Boolean));
    let nameIndex = 1;
    let guard = 0;
    while (occupiedNames.has(incarnationName(hero.name, nameIndex, activeProjectName).trim().toLowerCase())) {
      nameIndex += 1;
      guard += 1;
      if (guard > 1000) {
        setIncarnationProgress((prev) => (
          prev?.id === progressId
            ? {
                ...prev,
                phase: 'error',
                message: 'Could not find an available agent name.',
                errorMessage: 'Could not find an available agent name.',
                copied: false,
              }
            : prev
        ));
        return;
      }
    }
    const displayName = incarnationName(hero.name, nameIndex, activeProjectName);
    const agentId = mintProjectAgentId(occupiedIds) as AgentId;
    setIncarnationProgress((prev) => (
      prev?.id === progressId
        ? {
            ...prev,
            heroName: displayName,
          }
        : prev
    ));
    const instance: WorkingHero = {
      ...hero,
      id: agentId,
      templateId,
      name: displayName,
    };
    const profile = loadTavernHeroIncarnationProfile(templateId);
    const ok = await handleRecruitTest(
      agentId,
      profile?.cli ?? hero.cli,
      targetSeatIndex,
      profile
        ? {
            templateId,
            displayName: instance.name,
            profile: profileDraftForIncarnation(templateId, profile),
          }
        : undefined,
      {
        progressId,
        suppressFailureAlert: true,
        onStep: markIncarnationProgressStep,
        onError: (err) => failIncarnationProgress(agentId, err),
      },
    );
    if (ok) {
      setAgentInstances((prev) => ({ ...prev, [agentId]: instance }));
      setProjectAgentIdentities((prev) => (
        upsertProjectAgentIdentity(prev, {
          agentId,
          displayName: instance.name,
          sourceHeroId: templateId,
          status: 'active',
          provider: instance.cli,
          avatarId: instance.avatarId ?? null,
        })
      ));
      setRecruitSeatIndex(null);
      setShortcutRecruitSeatIndex(null);
      focusAgent(agentId);
      finishIncarnationProgress();
    }
  }, [
    agentInstances,
    agentLayout.offTableAgents,
    agentLayout.tableSlots,
    agentRuntime.liveAgents,
    activeProjectName,
    clearIncarnationProgressHideTimer,
    failIncarnationProgress,
    finishIncarnationProgress,
    focusAgent,
    handleRecruitTest,
    markIncarnationProgressStep,
    projectAgentRoot,
  ]);

  const retryIncarnationProgress = useCallback(() => {
    const payload = incarnationRetryPayloadRef.current;
    if (!payload) return;
    void handleIncarnateHero(payload.hero, payload.seatIndex);
  }, [handleIncarnateHero]);

  const refreshArchivedAgents = useCallback(async () => {
    const requestId = archivedAgentsRequestRef.current + 1;
    archivedAgentsRequestRef.current = requestId;
    try {
      const next = await listArchivedProjectAgents(projectAgentRoot);
      if (archivedAgentsRequestRef.current === requestId) {
        setArchivedAgents(next);
      }
    } catch (err) {
      if (archivedAgentsRequestRef.current === requestId) {
        setArchivedAgents([]);
        console.warn('[kota-agent-archive] list failed', err);
      }
    }
  }, [projectAgentRoot]);

  const launchExistingProjectAgent = useCallback(async (
    id: AgentId,
    seatIndex?: number | null,
    options: { selectComposer?: boolean } = {},
  ) => {
    const result = await coordinateExistingAgentLaunch(
      existingAgentLaunchesRef.current,
      id,
      async () => {
        const request = await resolveProjectAgentLaunch({
          agentId: id,
          projectRoot: projectAgentRoot,
        });
        requestVioletProjectAgentSync(violetProjectRoot, [id], roomAgentIdsOrdered);
        const recruited = await recruitWithLeaseTakeover(request);
        if (!recruited) return false;
        await refreshAgentPtySummaries();
        refreshFileTree();
        return true;
      },
      (err) => {
        console.warn('[kota-agent] existing project-agent launch failed', err);
        notifyExistingAgentLaunchFailure(id, err);
      },
    );
    if (result.status === 'launched') {
      placeRecruitedAgent(id, seatIndex);
      if (options.selectComposer ?? true) {
        setComposerTarget(id);
        setComposerBroadcast(false);
        setBroadcastPopupOpen(false);
      }
    }
    return result;
  }, [
    placeRecruitedAgent,
    projectAgentRoot,
    recruitWithLeaseTakeover,
    refreshAgentPtySummaries,
    refreshFileTree,
    roomAgentIdsOrdered,
    violetProjectRoot,
  ]);

  const spawnExistingProjectAgent = useCallback(
    (id: AgentId, seatIndex?: number | null) => launchExistingProjectAgent(id, seatIndex, {
      selectComposer: true,
    }),
    [launchExistingProjectAgent],
  );

  const wakeAgentForComposerSend = useCallback(
    (id: AgentId) => launchExistingProjectAgent(id, agentLayout.tableSlots.indexOf(id), {
      selectComposer: false,
    }),
    [agentLayout.tableSlots, launchExistingProjectAgent],
  );

  const openAgentTerminalFromMenu = useCallback((id: AgentId) => {
    if (agentsHydrating) return;
    if (agentRuntime.liveAgents.has(id)) {
      focusAgent(id);
      return;
    }
    const seatIndex = agentLayout.tableSlots.indexOf(id);
    void spawnExistingProjectAgent(id, seatIndex).then((result) => {
      if (result.status === 'launched') focusAgent(id);
    });
  }, [
    agentLayout.tableSlots,
    agentRuntime.liveAgents,
    agentsHydrating,
    focusAgent,
    spawnExistingProjectAgent,
  ]);

  const handleStartFreshSession = useCallback(async (id: AgentId) => {
    if (agentsHydrating) return;
    const name = agentName(id);
    const confirmed = await confirmInApp(
      `Start fresh session for ${name}?`,
      'This will end the current terminal session and open a new provider session. Workspace files, adapter, skills, and project memory stay unchanged.',
      {
        cancelLabel: 'Cancel',
        confirmLabel: 'Start Fresh Session',
        plainCopy: true,
        tone: 'danger',
      },
    );
    if (!confirmed) return;
    try {
      const result = await startFreshProjectAgentSession({
        agentId: id,
        projectRoot: projectAgentRoot,
      });
      const recruited = await recruitWithLeaseTakeover(result.request);
      if (!recruited) return;
      const detail = await clearProjectAgentSessionMetadata({
        agentId: id,
        projectRoot: projectAgentRoot,
      });
      applyProjectAgentDetail(detail);
      requestVioletProjectAgentSync(violetProjectRoot, [id], roomAgentIdsOrdered);
      await refreshAgentPtySummaries();
      placeRecruitedAgent(id, agentLayout.tableSlots.indexOf(id));
      refreshFileTree();
      focusAgent(id);
    } catch (err) {
      window.alert(`Start fresh session failed: ${String(err)}`);
    }
  }, [
    agentLayout.tableSlots,
    agentName,
    agentsHydrating,
    applyProjectAgentDetail,
    clearProjectAgentSessionMetadata,
    confirmInApp,
    focusAgent,
    placeRecruitedAgent,
    projectAgentRoot,
    recruitWithLeaseTakeover,
    refreshAgentPtySummaries,
    refreshFileTree,
    roomAgentIdsOrdered,
    violetProjectRoot,
  ]);

  const handleSeatDblClick = useCallback((id: AgentId) => {
    if (agentsHydrating) return;
    openAgentTerminalFromMenu(id);
  }, [agentsHydrating, openAgentTerminalFromMenu]);

  const handleProjectAgentSaved = useCallback((detail: ProjectAgentDetail) => {
    applyProjectAgentDetail(detail);
  }, [applyProjectAgentDetail]);

  const handleRemoveProjectAgent = useCallback(async (detail: ProjectAgentDetail) => {
    try {
      const removeConfirmed = await confirmInApp(
        'Remove agent from project?',
        `${detail.displayName} will leave the table and agent bar. Its workspace will stay in Archive so you can call it back later.`,
        { confirmLabel: 'Remove from project' },
      );
      if (!removeConfirmed) return;
      let result = await archiveProjectAgent({
        agentId: detail.agentId,
        projectRoot: projectAgentRoot,
        forceDirty: false,
      });
      if (!result.ok && result.dirty) {
        const confirmed = await confirmInApp(
          'Unsaved work in workspace',
          `${detail.displayName} has uncommitted project file changes:\n\n${result.dirtySummary}\n\nRemove this agent from the project anyway?`,
          { confirmLabel: 'Remove anyway', tone: 'danger' },
        );
        if (!confirmed) return;
        result = await archiveProjectAgent({
          agentId: detail.agentId,
          projectRoot: projectAgentRoot,
          forceDirty: true,
        });
      }
      if (!result.ok) {
        window.alert('Archive did not complete.');
        return;
      }
      const archivedDetail = result.detail;
      if (archivedDetail) {
        setProjectAgentIdentities((prev) => (
          upsertProjectAgentIdentity(prev, projectAgentIdentityFromDetail(archivedDetail, 'archived'))
        ));
      }
      removeAgentFromRoom(detail.agentId);
      removeWorkspaceAgentSpec(detail.agentId);
      setProjectAgentDetailId(null);
      void agentRuntime.dismiss(detail.agentId)
        .then(refreshAgentPtySummaries)
        .catch(() => {});
      await refreshArchivedAgents();
      refreshFileTree();
      focusComposerEndSoon();
    } catch (err) {
      window.alert(`Archive failed: ${String(err)}`);
    }
  }, [
    agentRuntime,
    confirmInApp,
    focusComposerEndSoon,
    projectAgentRoot,
    refreshArchivedAgents,
    refreshAgentPtySummaries,
    refreshFileTree,
    removeAgentFromRoom,
    removeWorkspaceAgentSpec,
  ]);

  const handleCallBackArchivedAgent = useCallback(async (detail: ProjectAgentDetail) => {
    if (firstEmptySeatIndex < 0) {
      window.alert('Table is full.');
      return;
    }
    try {
      const restored = await callBackProjectAgent({
        agentId: detail.agentId,
        projectRoot: projectAgentRoot,
      });
      applyProjectAgentDetail(restored);
      placeRecruitedAgent(restored.agentId, firstEmptySeatIndex);
      setComposerTarget(restored.agentId);
      setComposerBroadcast(false);
      setBroadcastPopupOpen(false);
      await refreshArchivedAgents();
      refreshFileTree();
      focusComposerEndSoon();
    } catch (err) {
      window.alert(`Call back failed: ${String(err)}`);
    }
  }, [
    applyProjectAgentDetail,
    firstEmptySeatIndex,
    focusComposerEndSoon,
    placeRecruitedAgent,
    projectAgentRoot,
    refreshArchivedAgents,
    refreshFileTree,
  ]);

  const handleDismissArchivedAgent = useCallback(async (detail: ProjectAgentDetail) => {
    try {
      const dismissConfirmed = await confirmInApp(
        'Delete archived agent?',
        `${detail.displayName}'s workspace will be permanently deleted. This cannot be undone.`,
        { confirmLabel: 'Delete', tone: 'danger' },
      );
      if (!dismissConfirmed) return;
      let result = await dismissProjectAgent({
        agentId: detail.agentId,
        projectRoot: projectAgentRoot,
        forceDirty: false,
      });
      if (!result.ok && result.dirty) {
        const confirmed = await confirmInApp(
          'Unsaved work in workspace',
          `${detail.displayName} has uncommitted project file changes:\n\n${result.dirtySummary}\n\nDelete this workspace anyway?`,
          { confirmLabel: 'Delete anyway', tone: 'danger' },
        );
        if (!confirmed) return;
        result = await dismissProjectAgent({
          agentId: detail.agentId,
          projectRoot: projectAgentRoot,
          forceDirty: true,
        });
      }
      if (!result.ok) {
        window.alert('Dismiss did not complete.');
        return;
      }
      setProjectAgentIdentities((prev) => (
        upsertProjectAgentIdentity(prev, projectAgentIdentityFromDetail(detail, 'left'))
      ));
      removeAgentFromRoom(detail.agentId);
      removeWorkspaceAgentSpec(detail.agentId);
      await refreshArchivedAgents();
      refreshFileTree();
    } catch (err) {
      window.alert(`Dismiss failed: ${String(err)}`);
    }
  }, [
    confirmInApp,
    projectAgentRoot,
    refreshArchivedAgents,
    refreshFileTree,
    removeAgentFromRoom,
    removeWorkspaceAgentSpec,
  ]);

  const handleKageBunshin = useCallback(async (id: AgentId) => {
    if (firstEmptySeatIndex < 0) {
      window.alert('Table is full.');
      return;
    }
    try {
      const result = await kageBunshinProjectAgent({
        agentId: id,
        projectRoot: projectAgentRoot,
      });
      applyProjectAgentDetail(result.detail);
      window.dispatchEvent(new Event(TAVERN_HERO_CREDIT_CHANGED_EVENT));
      const recruited = await recruitWithLeaseTakeover(result.request);
      if (!recruited) return;
      await refreshAgentPtySummaries();
      placeRecruitedAgent(result.detail.agentId, firstEmptySeatIndex);
      setComposerTarget(result.detail.agentId);
      setComposerBroadcast(false);
      setBroadcastPopupOpen(false);
      refreshFileTree();
      focusAgent(result.detail.agentId);
    } catch (err) {
      window.alert(`Kage Bunshin failed: ${String(err)}`);
    }
  }, [
    applyProjectAgentDetail,
    firstEmptySeatIndex,
    focusAgent,
    placeRecruitedAgent,
    projectAgentRoot,
    recruitWithLeaseTakeover,
    refreshAgentPtySummaries,
    refreshFileTree,
  ]);

  useEffect(() => {
    void refreshArchivedAgents();
  }, [refreshArchivedAgents]);

  useEffect(() => {
    if (archiveOpen) void refreshArchivedAgents();
  }, [archiveOpen, refreshArchivedAgents]);

  useEffect(() => {
    if (!agentContextMenu) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setAgentContextMenu(null);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [agentContextMenu]);

  // Ctrl/⌘N over the table slots means terminal visibility only:
  // focus/open the seat's terminal, or minimize it if already focused.
  // Empty seats open a shortcut-scoped add-agent dialog above terminals.
  useEffect(() => {
    const isPlainKey = (e: KeyboardEvent) =>
      !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey;
    const isComposerFocused = () => {
      const active = document.activeElement;
      return active instanceof HTMLElement && !!active.closest('[data-testid="input-field"]');
    };
    const cycleComposerTarget = (direction: 1 | -1) => {
      if (composerBroadcast || shortcutTargetAgents.length === 0) return;
      const currentIndex = composerTarget ? shortcutTargetAgents.indexOf(composerTarget) : -1;
      const fallbackIndex = direction === 1 ? 0 : shortcutTargetAgents.length - 1;
      const nextIndex = currentIndex < 0
        ? fallbackIndex
        : (currentIndex + direction + shortcutTargetAgents.length) % shortcutTargetAgents.length;
      const nextTarget = shortcutTargetAgents[nextIndex];
      if (nextTarget) selectComposerTarget(nextTarget);
    };
    const openShortcutRecruitModal = (seatIndex: number) => {
      if (seatIndex < 0 || seatIndex >= MAX_AGENT_SLOTS) return;
      if (agentLayout.tableSlots[seatIndex]) return;
      setBroadcastPopupOpen(false);
      setComposerBroadcast(false);
      setRecruitSeatIndex(null);
      setShortcutRecruitSeatIndex(seatIndex);
      setTerminalFocusedAgent(null);
    };
    const activateShortcutSeat = (seatIndex: number) => {
      const id = shortcutAgentsOrdered[seatIndex] ?? null;
      if (!id) {
        openShortcutRecruitModal(seatIndex);
        return;
      }
      if (agentRuntime.liveAgents.has(id)) {
        const alreadyFocused = terminalFocusedAgent === id && !minimized.has(id);
        if (alreadyFocused) minimizeAgentFromWindow(id);
        else focusAgent(id);
        return;
      }
      const seatIndexForAgent = agentLayout.tableSlots.indexOf(id);
      void spawnExistingProjectAgent(id, seatIndexForAgent).then((result) => {
        if (result.status === 'launched') focusAgent(id);
      });
    };

    const onKeyDown = (e: KeyboardEvent) => {
      const plain = isPlainKey(e);

      if (shortcutRecruitSeatIndex !== null && plain && e.key === 'Escape') {
        e.preventDefault();
        setShortcutRecruitSeatIndex(null);
        return;
      }

      if (recruitSeatIndex !== null && plain && e.key === 'Escape') {
        e.preventDefault();
        setRecruitSeatIndex(null);
        return;
      }

      if (broadcastPopupOpen && plain) {
        const seatIndex = agentSlotIndexFromKey(e.key);
        if (seatIndex !== null) {
          const id = shortcutAgentsOrdered[seatIndex];
          if (id) {
            e.preventDefault();
            toggleBroadcastRecipient(id);
          }
          return;
        }
        if (e.key === 'Enter') {
          e.preventDefault();
          confirmBroadcastRecipients();
          return;
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          cancelBroadcastMode();
          return;
        }
      }

      if (!broadcastPopupOpen && plain && isComposerFocused()) {
        if (e.key === 'PageDown') {
          e.preventDefault();
          cycleComposerTarget(1);
          return;
        }
        if (e.key === 'PageUp') {
          e.preventDefault();
          cycleComposerTarget(-1);
          return;
        }
      }

      const meta = e.metaKey || e.ctrlKey;
      if (!meta || e.shiftKey || e.altKey) return;
      if (e.repeat) return;

      if (e.key === '0') {
        e.preventDefault();
        minimizeAllAgents();
        return;
      }
      if (e.key === '9') {
        e.preventDefault();
        toggleGroupChat();
        return;
      }
      const seatIndex = agentSlotIndexFromKey(e.key);
      if (seatIndex !== null) {
        e.preventDefault();
        activateShortcutSeat(seatIndex);
      }
    };

    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [
    agentLayout.tableSlots,
    agentRuntime.liveAgents,
    broadcastPopupOpen,
    cancelBroadcastMode,
    composerBroadcast,
    composerTarget,
    confirmBroadcastRecipients,
    focusAgent,
    minimizeAgentFromWindow,
    minimized,
    minimizeAllAgents,
    recruitSeatIndex,
    selectComposerTarget,
    shortcutAgentsOrdered,
    shortcutTargetAgents,
    shortcutRecruitSeatIndex,
    spawnExistingProjectAgent,
    terminalFocusedAgent,
    toggleBroadcastRecipient,
    toggleGroupChat,
  ]);

  const submitPromptToAgent = useCallback((agentId: AgentId, rawPayload: string) => {
    const input = formatAgentPromptInput(rawPayload);
    if (!input) return Promise.resolve();
    const previous = agentPromptQueuesRef.current.get(agentId) ?? Promise.resolve();
    const next = previous
      .catch(() => undefined)
      .then(async () => {
        await agentRuntime.submitPrompt(agentId, input);
      });
    agentPromptQueuesRef.current.set(agentId, next);
    void next.finally(() => {
      if (agentPromptQueuesRef.current.get(agentId) === next) {
        agentPromptQueuesRef.current.delete(agentId);
      }
    });
    return next;
  }, [agentRuntime]);

  const handlePasteComposerImage = useCallback(
    (file: File) => saveComposerClipboardImage(file, { projectRoot: projectAgentRoot }),
    [projectAgentRoot],
  );

  const handleMaterializeComposerAttachments = useCallback(
    async (attachments: readonly ComposerAttachment[]) => Promise.all(
      attachments.map(async (attachment) => {
        try {
          if (attachment.kind === 'drawing') return attachment;
          const path = await materializeComposerAttachmentPath(attachment.path, {
            projectRoot: projectAgentRoot,
          });
          return { ...attachment, path };
        } catch {
          return attachment;
        }
      }),
    ),
    [projectAgentRoot],
  );

  const handleSend = useCallback(
    async (
      target: AgentId | null,
      payload: string,
      options?: {
        broadcast: boolean;
        privacy: boolean;
        mentions?: { agentId: AgentId; aka: string }[];
        recipientIds?: AgentId[];
      },
    ) => {
      const requestedRecipients = options?.recipientIds?.length
        ? options.recipientIds
        : options?.broadcast
        ? Array.from(broadcastRecipients).filter((id) => roomAgentIds.has(id))
        : target && roomAgentIds.has(target)
          ? [target]
          : [];
      const recipients = Array.from(new Set(requestedRecipients)).filter((id) => roomAgentIds.has(id));
      const normalizedPayload = normalizeAgentPromptPayload(payload);
      if (!normalizedPayload.trim()) return false;
      if (recipients.length === 0) return false;
      const liveBeforeWake = new Set(agentRuntime.liveAgents);
      const sleepingRecipients = recipients.filter((id) => !liveBeforeWake.has(id));
      if (sleepingRecipients.length > 0) {
        const names = sleepingRecipients.map((id) => shortProjectAgentName(agentMeta[id]?.name ?? id));
        const label = names.length === 1 ? names[0] : names.join(', ');
        const confirmed = await confirmInApp(
          'Wake sleeping agent?',
          names.length === 1
            ? `${label} is sleeping, wake up now?`
            : `${label} are sleeping, wake up now?`,
          {
            confirmLabel: 'Wake up',
            cancelLabel: 'Cancel',
            confirmOnEnter: true,
          },
        );
        if (!confirmed) return false;
        const wakeResults = await Promise.all(
          sleepingRecipients.map(async (id) => ({
            id,
            result: await wakeAgentForComposerSend(id),
          })),
        );
        const wokenRecipients = wakeResults.flatMap(({ id, result }) => (
          result.status === 'launched' ? [id] : []
        ));
        const failedWakeRecipients = wakeResults.flatMap(({ id, result }) => (
          result.status === 'failed' ? [id] : []
        ));
        if (wokenRecipients.length > 0) {
          const wokenSet = new Set(wokenRecipients);
          setMinimized((prev) => {
            const next = new Set(prev);
            for (const id of wokenSet) next.delete(id);
            return next;
          });
          const first = wokenRecipients[0]!;
          setTerminalFocusedAgent(first);
          window.setTimeout(() => windowsRef.current?.bringToFront(first), 150);
        }
        if (failedWakeRecipients.length > 0) {
          console.warn('[composer] agent wake failed', failedWakeRecipients);
        }
        return false;
      }
      emitVioletComposerSent({
        projectRoot: violetProjectRoot,
        text: normalizedPayload,
        targetAgentIds: recipients,
        privacy: PRIVATE_CHAT_UI_ENABLED && !!options?.privacy,
        mentions: options?.mentions,
      });
      const results = await Promise.allSettled(
        recipients.map(async (id) => {
          await submitPromptToAgent(id, payload);
          return id;
        }),
      );
      const deliveredRecipients = results.flatMap((result) => (
        result.status === 'fulfilled' ? [result.value] : []
      ));
      const failedRecipients = results.flatMap((result, index) => (
        result.status === 'rejected' ? [recipients[index]!] : []
      ));
      if (failedRecipients.length > 0) {
        console.warn('[composer] prompt delivery failed', failedRecipients);
      }
      if (deliveredRecipients.length === 0) {
        return true;
      }
      requestVioletProjectPromptSync(
        violetProjectRoot,
        deliveredRecipients,
        roomAgentIdsOrdered,
      );
      return true;
    },
    [
      agentRuntime.liveAgents,
      agentRuntime.status,
      agentMeta,
      broadcastRecipients,
      confirmInApp,
      roomAgentIds,
      roomAgentIdsOrdered,
      submitPromptToAgent,
      violetProjectRoot,
      wakeAgentForComposerSend,
    ],
  );

  const handleRetryComposerMessage = useCallback(
    async (request: {
      text: string;
      targetAgentIds: AgentId[];
      privacy: boolean;
      mentions?: { agentId: AgentId; aka: string }[];
    }) => handleSend(null, request.text, {
      broadcast: false,
      privacy: request.privacy,
      mentions: request.mentions,
      recipientIds: request.targetAgentIds,
    }),
    [handleSend],
  );

  return (
    <>
      <TopBar
        projects={projectTabs}
        activeProjectId={activeProjectId}
        projectUnreadCounts={violetUnreadByProjectId}
        onSelectProject={handleSelectProject}
        onReorderProjects={reorderWorkspaceTabs}
        onNewProject={() => setProjectSetupOpen(true)}
        onCloseProject={(projectId) => {
          void handleArchiveProjectTab(projectId);
        }}
        onOpenTavern={openTavern}
        tavernOpen={tavernOpen}
        tavernPreparing={tavernPreparing}
        hideProjectTabs={tavernOpen || tavernPreparing}
        ghAuth={ghAuth}
        onGhAuthClick={() => {
          if (ghAuth?.authenticated) return;
          if (ghAuth?.cliMissing) {
            window.open(GITHUB_CLI_INSTALL_URL, '_blank', 'noopener,noreferrer');
            return;
          }
          window.dispatchEvent(new CustomEvent('kota:smart-run', {
            detail: { command: GITHUB_CLI_LOGIN_COMMAND },
          }));
        }}
      />
      <>
        <>
          {!showProjectEmptyState && !showProjectLoadingState && (
            <ProjectSetupModal
              open={projectSetupOpen}
              mode="openProject"
              onClose={() => setProjectSetupOpen(false)}
              onWorkspacePrepared={handleWorkspacePrepared}
            />
          )}
          <div className="workspace">
            <div className="workspace-main-shell">
              {showProjectLoadingState ? (
                <div className="workspace-state-panel">
                  <ProjectLoadingState centerpiece={centerpiece} />
                </div>
              ) : showProjectEmptyState ? (
                <div className="workspace-state-panel">
                  {projectSetupOpen ? (
                    <ProjectSetupModal
                      open={projectSetupOpen}
                      mode="firstProject"
                      embedded
                      onClose={() => setProjectSetupOpen(false)}
                      onWorkspacePrepared={handleWorkspacePrepared}
                    />
                  ) : (
                    <EmptyProjectState
                      onCreateProject={() => setProjectSetupOpen(true)}
                      centerpiece={centerpiece}
                    />
                  )}
                </div>
              ) : (
                <>
                  <FileTree
                    projectId={activeWorkspace?.projectId ?? activeProjectId}
                    repoName={
                      projectTabs.find((p) => p.id === activeProjectId)?.name ?? 'project'
                    }
                    sourceDir={activeWorkspace?.sourceDir ?? recruitProjectRoot}
                    workspaceDir={activeWorkspace?.localRoot ?? `${recruitProjectRoot}/Kota/Workspaces/${activeProjectId}`}
                    refreshToken={fileTreeRefreshToken}
                  />
                  <Stage
                    sceneKey={sceneKey}
                    liveAgents={activeRunningAgents}
                    workingAgents={activeWorkingAgents}
                    workingStartedAt={activeWorkingStartedAt}
                    dreamingStatusAgents={dreamingStatusAgents}
                    minimizedAgents={minimized}
                    shortcutAgentsOrdered={shortcutAgentsOrdered}
                    agentsHydrating={agentsHydrating}
                    agentHydrationProgress={activeAgentHydrationProgress}
                    tableSlots={agentLayout.tableSlots}
                    offTableAgents={[]}
                    agentMeta={agentMeta}
                    projectName={activeProjectName}
                    targetAgent={targetAgent}
                    chatFilterTargetAgents={chatFilterTargetAgents}
                    chatFilterActive={chatFilterActive}
                    onChatFilterActiveChange={setChatFilterActive}
                    chatFilterOpenRequest={chatFilterOpenRequest}
                    privateAgents={effectivePrivateAgents}
                    privacyControlsEnabled={PRIVATE_CHAT_UI_ENABLED}
                    onOpenAgent={handleSeatClick}
                    onOpenRibbonAgent={handleRibbonAgentClick}
                    onDblClickAgent={handleSeatDblClick}
                    onOpenAgentTerminal={openAgentTerminalFromMenu}
                    onRetryComposerMessage={handleRetryComposerMessage}
                    onTogglePrivacyAgent={PRIVATE_CHAT_UI_ENABLED ? togglePrivacyAgent : undefined}
                    onToggleAllPrivacy={PRIVATE_CHAT_UI_ENABLED ? toggleAllPrivacy : undefined}
                    onAgentContextMenu={openAgentContextMenu}
                    onCommendAgent={handleCommendAgent}
                    centerpiece={centerpiece}
                    roomColor={roomColor}
                    deskColor={deskColor}
                    roomTheme={roomTheme}
                    deskTheme={deskTheme}
                    onChangeCenter={setCenterpiece}
                    onChangeRoom={setRoomColor}
                    onChangeDesk={setDeskColor}
                    onChangeRoomTheme={setRoomTheme}
                    onChangeDeskTheme={setDeskTheme}
                    workingHeroes={workingHeroes}
                    agentRecords={agentRecords}
                    onIncarnateHero={handleIncarnateHero}
                    onOpenAgentAdd={openNextEmptySeatRecruitModal}
                    onOpenAgentSlotAdd={openSpecificSeatRecruitModal}
                    onSwapSeats={swapAgentSeats}
                    unavailableHeroIds={unavailableHeroIds}
                    recruitSeatIndex={recruitSeatIndex}
                    onRecruitSeatIndexChange={setRecruitSeatIndex}
                    groupChatOpen={groupChatOpen}
                    groupChatUnreadCount={violetRoomVisible ? 0 : activeVioletUnreadCount}
                    unreadAgentIds={activeVioletUnreadAgentIds}
                    onToggleGroupChat={toggleGroupChat}
                    projectRoot={violetProjectRoot}
                    projectRulesDir={projectRulesDir}
                    broadcastPopover={
                      broadcastPopupOpen ? (
                        <BroadcastTargetPopover
                          onTableAgents={shortcutTargetAgents}
                          offTableAgents={[]}
                          agentMeta={agentMeta}
                          selectedAgents={broadcastRecipients}
                          liveAgents={activeRunningAgents}
                          onToggleAgent={toggleBroadcastRecipient}
                          onConfirm={confirmBroadcastRecipients}
                          onCancel={cancelBroadcastMode}
                          onClear={clearBroadcastRecipients}
                        />
                      ) : null
                    }
                    composer={
                      <InputBar
                        ref={inputBarRef}
                        value={inputText}
                        onChange={handleInputChange}
                        targetAgent={targetAgent}
                        agentMeta={agentMeta}
                        mentionAgentIds={roomAgentIdsOrdered}
                        broadcastMode={composerBroadcast}
                        broadcastRecipientCount={broadcastRecipients.size}
                        broadcastPrivacyInfo={PRIVATE_CHAT_UI_ENABLED ? broadcastPrivacyInfo : undefined}
                        privacyMode={currentTargetPrivate}
                        privacyControlsEnabled={PRIVATE_CHAT_UI_ENABLED}
                        onBroadcastToggle={openTargetPicker}
                        onPrivacyToggle={PRIVATE_CHAT_UI_ENABLED ? togglePrivacyMode : undefined}
                        onSend={handleSend}
                        onPasteImage={handlePasteComposerImage}
                        onMaterializeAttachments={handleMaterializeComposerAttachments}
                        onWhiteboardOpen={toggleWhiteboard}
                        onFocus={() => setTerminalFocusedAgent(null)}
                      />
                    }
                  />
                </>
              )}
            </div>
            <RightColumn
              sceneKey={sceneKey}
              onOpenHotMem={() => setPopup({ kind: 'hotmem' })}
              onOpenRow={(row) => setPopup({ kind: 'row', row })}
              workspace={activeWorkspace}
              workspaceProjects={visibleWorkspaceTabs}
              workingAgents={activeWorkingAgents}
              workingStartedAt={activeWorkingStartedAt}
              onDreamingStatusAgentsChange={handleDreamingStatusAgentsChange}
              roomAgents={shortcutTargetAgents}
              dreamProjects={dreamProjects}
              resolveDreamProjects={resolveDreamProjects}
              agentMeta={agentMeta}
              projectRoot={projectAgentRoot}
              onBartenderSynced={refreshFileTree}
              onOpenAgentFilteredChat={openAgentFilteredRoom}
              onInsertComposerAttachment={insertComposerAttachment}
              onPasteImage={handlePasteComposerImage}
              onMaterializeAttachments={handleMaterializeComposerAttachments}
              footerSlot={(
                <button
                  type="button"
                  className={`archive-launcher ${archiveOpen ? 'open' : ''}`}
                  aria-label="Archived agents"
                  title="Archived agents"
                  onClick={() => setArchiveOpen((open) => !open)}
                >
                  <svg className="archive-launcher-icon" viewBox="0 0 400 400" fill="none" aria-hidden>
                    <path d="M208.966 110.117C254.405 154.438 251.905 240.684 230.919 288.101C201.051 355.593 209.978 250.359 184.602 277.117C177.704 284.391 181.81 317.719 156.516 320.269C138.085 322.126 154.096 266.606 141.635 277.117C127.283 289.224 121.293 331.099 103.61 320.269C96.98 288.749 95.6539 205.826 103.61 164.619" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                    <path d="M334.001 205.901C300.792 236.16 270.173 269.031 239.891 302.247C231.256 311.719 217.546 319.675 207.086 324.435" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                    <path d="M240.001 310.082C261.705 300.029 259.1 324.324 252.155 325.999C233.056 330.607 236.719 314.627 238.123 312.595" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                    <path d="M318.739 227.487C333.322 228.815 333.932 239.822 328.721 245.562C322.396 252.532 303.881 248.558 312.768 230.427" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                    <path d="M67 154.417C74.2958 151.934 94.2222 144.012 102.587 140.456M102.587 140.456C132.714 127.648 168.181 109.352 197.507 96.1835C186.549 83.4764 160.167 66.0223 132.221 75.9106C98.42 87.8702 101.97 124.204 102.587 140.456Z" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                    <path d="M212.18 168.071C211.362 164.294 211.429 160.869 210.359 156.308" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                    <path d="M189.816 170.532C190.383 167.512 189.063 162.444 188.656 159.594" stroke="currentColor" strokeOpacity="0.9" strokeWidth="16" strokeLinecap="round" strokeLinejoin="round" />
                  </svg>
                  <span className="archive-launcher-copy">Archive</span>
                  {archivedAgents.length > 0 && <span className="archive-launcher-count">{archivedAgents.length}</span>}
                </button>
              )}
            />
            {/* W3 — floating agent windows. Mounted as workspace overlay
                so it covers Stage but not TopBar / Smart Terminal. W4 lifts
                minimized state up so AgentRibbon and AgentWindowsLayer
                share a single source of truth. */}
            {!showProjectLoadingState && !showProjectEmptyState && (
            <AgentWindowsLayer
              ref={windowsRef}
              liveAgents={activeLiveAgentsOrdered}
              grids={agentRuntime.grids}
              status={agentRuntime.status}
              agentMeta={agentMeta}
              focusedAgent={
                terminalFocusedAgent && activeLiveAgents.has(terminalFocusedAgent)
                  ? terminalFocusedAgent
                  : null
              }
              minimized={minimized}
              onFocusAgent={(id) => {
                clearPendingComposerFocus();
                restoreAgent(id);
                setTerminalFocusedAgent(id);
                syncComposerTargetForTerminalFocus(id);
              }}
              onMinimizeAgent={minimizeAgentFromWindow}
              onAgentKey={(id, bytes) => { void agentRuntime.send(id, bytes); }}
              projectId={activeProjectId}
              projectName={activeProjectName}
              ghosttyTerminalEnhancement={ghosttyTerminalEnhancement}
              onOpenAgentDetail={setProjectAgentDetailId}
              onAgentContextMenu={(id, point) => openAgentContextMenu(id, point, 'terminal-header')}
              onCommendAgent={handleCommendAgent}
              agentRecords={agentRecords}
            />
            )}
            {archiveOpen && (
              <ArchivePopover
                agents={archivedAgents}
                onCallBack={(detail) => void handleCallBackArchivedAgent(detail)}
                onDismiss={(detail) => void handleDismissArchivedAgent(detail)}
                onClose={() => setArchiveOpen(false)}
              />
            )}
            {popup?.kind === 'whiteboard' && (
              <Suspense fallback={<div className="wb-overlay"><div className="wb-window"><div className="wb-loading">Loading canvas...</div></div></div>}>
                <WhiteboardPanel
                  projectRoot={violetProjectRoot}
                  onClose={closePopup}
                  onInsertDrawing={(attachment) => {
                    inputBarRef.current?.insertAttachment(attachment);
                  }}
                  onLoadCanvas={loadWhiteboardCanvas}
                  onSaveCanvas={saveWhiteboardCanvas}
                  onRenamePage={renameWhiteboardCanvasPage}
                  onSaveSnapshot={saveWhiteboardCanvasSnapshot}
                />
              </Suspense>
            )}
            {agentContextMenu && (
              <AgentContextMenu
                state={agentContextMenu}
                agentName={agentName(agentContextMenu.agentId)}
                isPrivate={effectivePrivateAgents.has(agentContextMenu.agentId)}
                cmdSlot={shortcutAgentsOrdered.indexOf(agentContextMenu.agentId) + 1 || null}
                showTerminal={agentContextMenu.source !== 'terminal-header'}
                showPrivateChat={PRIVATE_CHAT_UI_ENABLED}
                showKageBunshin={KAGE_BUNSHIN_UI_ENABLED}
                onClose={() => setAgentContextMenu(null)}
                onDetail={() => {
                  setProjectAgentDetailId(agentContextMenu.agentId);
                  setAgentContextMenu(null);
                }}
                onTerminal={() => {
                  openAgentTerminalFromMenu(agentContextMenu.agentId);
                  setAgentContextMenu(null);
                }}
                onStartFreshSession={() => {
                  void handleStartFreshSession(agentContextMenu.agentId);
                  setAgentContextMenu(null);
                }}
                onPrivate={() => {
                  toggleAgentPrivateFromMenu(agentContextMenu.agentId);
                  setAgentContextMenu(null);
                }}
                onKageBunshin={() => {
                  void handleKageBunshin(agentContextMenu.agentId);
                  setAgentContextMenu(null);
                }}
              />
            )}
            {incarnationProgress && (
              <IncarnationProgressBar
                progress={incarnationProgress}
                onRetry={retryIncarnationProgress}
                onDismiss={dismissIncarnationProgress}
                onCopyError={copyIncarnationProgressError}
              />
            )}
            {shortcutRecruitSeatIndex !== null && (
              <ShortcutRecruitModal
                seatNumber={shortcutRecruitSeatIndex + 1}
                heroes={workingHeroes}
                unavailableHeroIds={unavailableHeroIds}
                onSelect={(hero) => {
                  if (shortcutRecruitSeatIndex !== null) {
                    return handleIncarnateHero(hero, shortcutRecruitSeatIndex);
                  }
                  return undefined;
                }}
                onDismiss={() => setShortcutRecruitSeatIndex(null)}
              />
            )}
            {confirmDialog && (
              <ConfirmDialog dialog={confirmDialog} onClose={closeConfirmDialog} />
            )}
            {projectAgentDetailId && (
              <ProjectAgentProfileOverlay
                agentId={projectAgentDetailId}
                projectRoot={projectAgentRoot}
                existingNames={Object.values(agentInstances).map((hero) => ({ id: hero.id, name: hero.name }))}
                onClose={() => setProjectAgentDetailId(null)}
                onSaved={handleProjectAgentSaved}
                onRemoveFromProject={handleRemoveProjectAgent}
              />
            )}
          </div>
          <SmartTerminal />
          {popup?.kind === 'hotmem' && <HotMemoryPopup onClose={closePopup} />}
          {popup?.kind === 'row' && <RowPopup row={popup.row} onClose={closePopup} />}
        </>
        <TavernOpeningGate open={tavernPreparing} items={tavernPrepareItems} />
        <TavernModal
          open={tavernOpen}
          onClose={closeTavern}
          initialTab={tavernInitialTab}
          initialGhAuth={ghAuth}
          ghosttyTerminalEnhancement={ghosttyTerminalEnhancement}
          onGhosttyTerminalEnhancementChange={setGhosttyTerminalEnhancement}
          onWorkspaceResumed={handleWorkspacePrepared}
        />
      </>
    </>
  );
}

function TavernOpeningGate({ open, items }: { open: boolean; items: readonly TavernLoadingLogItem[] }) {
  if (!open) return null;
  return (
    <section className="tavern-opening-gate" aria-live="polite" aria-label="Opening Tavern">
      <div className="tavern-opening-card" role="status">
        <div className="tavern-opening-art" aria-hidden="true">
          <svg
            className="tavern-opening-scene"
            viewBox="-13 252 506 130"
            xmlns="http://www.w3.org/2000/svg"
            focusable="false"
          >
            <defs>
              <linearGradient id="tavernLoaderBeer" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor="var(--tavern-loader-amber-hi)" />
                <stop offset="0.5" stopColor="var(--tavern-loader-amber)" />
                <stop offset="1" stopColor="var(--tavern-loader-terra)" />
              </linearGradient>
              <linearGradient id="tavernLoaderBeerBar" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor="var(--tavern-loader-amber-hi)" />
                <stop offset="1" stopColor="var(--tavern-loader-terra)" />
              </linearGradient>
              <linearGradient id="tavernLoaderFoam" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stopColor="var(--tavern-loader-brass-hi)" />
                <stop offset="1" stopColor="var(--text-2)" />
              </linearGradient>
              <linearGradient id="tavernLoaderShine" x1="0" y1="0" x2="1" y2="0">
                <stop offset="0" stopColor="#fff" stopOpacity="0" />
                <stop offset="0.5" stopColor="var(--tavern-loader-brass-hi)" stopOpacity="0.72" />
                <stop offset="1" stopColor="#fff" stopOpacity="0" />
              </linearGradient>
              <clipPath id="tavernLoaderBarInner">
                <rect x="49" y="313" width="382" height="17" rx="8" />
              </clipPath>
              <clipPath id="tavernLoaderFillReveal">
                <rect className="tavern-loader-fill-clip" x="49" y="313" width="382" height="17" />
              </clipPath>
              <clipPath id="tavernLoaderMugInner">
                <path d="M48,34 L116,34 Q122,34 122,42 C129,75 128,125 110,158 Q108,166 100,166 L64,166 Q56,166 54,158 C36,125 35,75 42,42 Q42,34 48,34 Z" />
              </clipPath>
            </defs>

            <rect x="40" y="307.5" width="400" height="28" rx="14" fill="#14100d" />
            <rect
              x="43"
              y="310.5"
              width="394"
              height="22"
              rx="11"
              fill="none"
              stroke="#090a0b"
              strokeWidth="2"
              opacity="0.8"
            />
            <g clipPath="url(#tavernLoaderBarInner)">
              <g clipPath="url(#tavernLoaderFillReveal)">
                <rect
                  className="tavern-loader-bar-fill"
                  x="49"
                  y="313"
                  width="382"
                  height="17"
                  fill="url(#tavernLoaderBeerBar)"
                />
                <g className="tavern-loader-bar-fill">
                  <circle
                    className="tavern-loader-bubble"
                    cx="90"
                    cy="325"
                    r="2.4"
                    fill="var(--tavern-loader-brass-hi)"
                    style={{ animationDelay: '0.2s' }}
                  />
                  <circle
                    className="tavern-loader-bubble"
                    cx="150"
                    cy="326"
                    r="2"
                    fill="var(--tavern-loader-brass-hi)"
                    style={{ animationDelay: '1.1s' }}
                  />
                  <circle
                    className="tavern-loader-bubble"
                    cx="120"
                    cy="324"
                    r="1.8"
                    fill="var(--tavern-loader-brass-hi)"
                    style={{ animationDelay: '1.7s' }}
                  />
                  <circle
                    className="tavern-loader-bubble"
                    cx="210"
                    cy="326"
                    r="2.2"
                    fill="var(--tavern-loader-brass-hi)"
                    style={{ animationDelay: '0.6s' }}
                  />
                  <circle
                    className="tavern-loader-bubble"
                    cx="265"
                    cy="324"
                    r="2"
                    fill="var(--tavern-loader-brass-hi)"
                    style={{ animationDelay: '1.4s' }}
                  />
                </g>
                <rect
                  className="tavern-loader-shimmer"
                  x="49"
                  y="313"
                  width="96"
                  height="17"
                  fill="url(#tavernLoaderShine)"
                  opacity="0"
                  transform="skewX(-16)"
                />
              </g>
            </g>
            <rect
              x="40"
              y="307.5"
              width="400"
              height="28"
              rx="14"
              fill="none"
              stroke="var(--text-2)"
              strokeWidth="6"
            />
            <rect
              x="40"
              y="307.5"
              width="400"
              height="28"
              rx="14"
              fill="none"
              stroke="var(--tavern-loader-brass-hi)"
              strokeWidth="2.5"
            />
            <rect
              x="44.5"
              y="312"
              width="391"
              height="19"
              rx="9.5"
              fill="none"
              stroke="rgba(122,110,96,0.78)"
              strokeWidth="1"
              opacity="0.7"
            />

            <g className="tavern-loader-mug-ride">
              <g className="tavern-loader-mug-bob">
                <g transform="translate(8,286) rotate(16 80 150) scale(0.5)">
                  <g clipPath="url(#tavernLoaderMugInner)">
                    <rect x="34" y="38" width="100" height="134" fill="url(#tavernLoaderBeer)" />
                    <circle
                      className="tavern-loader-bubble"
                      cx="66"
                      cy="150"
                      r="4"
                      fill="var(--tavern-loader-brass-hi)"
                    />
                    <circle
                      className="tavern-loader-bubble"
                      cx="94"
                      cy="158"
                      r="3.4"
                      fill="var(--tavern-loader-brass-hi)"
                      style={{ animationDelay: '0.8s' }}
                    />
                    <circle
                      className="tavern-loader-bubble"
                      cx="80"
                      cy="146"
                      r="3"
                      fill="var(--tavern-loader-brass-hi)"
                      style={{ animationDelay: '1.5s' }}
                    />
                    <path
                      d="M58,56 C50,90 50,120 53,146"
                      fill="none"
                      stroke="#fff"
                      strokeWidth="6"
                      strokeLinecap="round"
                      opacity="0.2"
                    />
                  </g>
                  <path
                    d="M42,40 C 38,22 58,18 68,26 C 74,12 98,12 106,26 C 118,16 134,24 126,40 C 136,42 138,54 126,56 L 46,56 C 34,54 34,42 42,40 Z"
                    fill="url(#tavernLoaderFoam)"
                    stroke="var(--tavern-loader-brass-hi)"
                    strokeWidth="6"
                    strokeLinejoin="round"
                  />
                  <path
                    d="M48,34 L116,34 Q122,34 122,42 C129,75 128,125 110,158 Q108,166 100,166 L64,166 Q56,166 54,158 C36,125 35,75 42,42 Q42,34 48,34 Z"
                    fill="none"
                    stroke="var(--tavern-loader-brass-hi)"
                    strokeWidth="11"
                    strokeLinejoin="round"
                  />
                  <g transform="translate(112,158)">
                    <circle className="tavern-loader-drop one" r="5" fill="var(--tavern-loader-amber)" />
                    <circle className="tavern-loader-drop two" r="4" fill="var(--tavern-loader-amber-hi)" />
                    <circle className="tavern-loader-drop three" r="3.4" fill="var(--tavern-loader-amber)" />
                  </g>
                </g>
              </g>
            </g>
          </svg>
          <div className="tavern-opening-title">LOADING</div>
        </div>
        <TavernLoadingLog items={items} lead="Preparing Tavern" className="tavern-opening-loading-log" />
      </div>
    </section>
  );
}

function waitForTavernOpeningGatePaint(): Promise<void> {
  return new Promise((resolve) => {
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => resolve());
    });
  });
}

function EmptyProjectState({
  onCreateProject,
  centerpiece,
}: {
  onCreateProject: () => void;
  centerpiece: Centerpiece;
}) {
  return (
    <section className="empty-project-state" data-testid="empty-project-state" aria-label="No project open">
      <div className="empty-project-hearth">
        <Hearth centerpiece={centerpiece} />
      </div>
      <div className="empty-project-actions">
        <button type="button" onClick={onCreateProject}>
          Create your first Kota project
        </button>
      </div>
    </section>
  );
}

function ProjectLoadingState({ centerpiece }: { centerpiece: Centerpiece }) {
  return (
    <section className="empty-project-state loading" data-testid="project-loading-state" aria-label="Loading project">
      <div className="empty-project-hearth">
        <Hearth centerpiece={centerpiece} />
      </div>
      <div className="empty-project-loading-copy">Loading project...</div>
    </section>
  );
}

function ConfirmDialog({
  dialog,
  onClose,
}: {
  dialog: NonNullable<ConfirmDialogState>;
  onClose: (confirmed: boolean) => void;
}) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose(false);
        return;
      }
      if (event.key === 'Enter' && dialog.confirmOnEnter) {
        event.preventDefault();
        onClose(true);
      }
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [dialog.confirmOnEnter, onClose]);

  return (
    <div className="kota-confirm-layer" role="presentation" onMouseDown={() => onClose(false)}>
      <section
        className={[
          'kota-confirm-card',
          dialog.tone === 'danger' ? 'danger' : '',
          dialog.plainCopy ? 'plain-copy' : '',
        ].filter(Boolean).join(' ')}
        role="dialog"
        aria-modal="true"
        aria-label={dialog.title}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <h2>{dialog.title}</h2>
        <pre>{dialog.body}</pre>
        <div className="kota-confirm-actions">
          <button type="button" onClick={() => onClose(false)}>
            {dialog.cancelLabel}
          </button>
          <button type="button" className="confirm" onClick={() => onClose(true)}>
            {dialog.confirmLabel}
          </button>
        </div>
      </section>
    </div>
  );
}

function AgentContextMenu({
  state,
  agentName,
  isPrivate,
  cmdSlot,
  showTerminal,
  showPrivateChat,
  showKageBunshin,
  onClose,
  onDetail,
  onTerminal,
  onStartFreshSession,
  onPrivate,
  onKageBunshin,
}: {
  state: NonNullable<AgentContextMenuState>;
  agentName: string;
  isPrivate: boolean;
  cmdSlot: number | null;
  showTerminal: boolean;
  showPrivateChat: boolean;
  showKageBunshin: boolean;
  onClose: () => void;
  onDetail: () => void;
  onTerminal: () => void;
  onStartFreshSession: () => void;
  onPrivate: () => void;
  onKageBunshin: () => void;
}) {
  return (
    <div className="agent-context-menu-scrim" onPointerDown={onClose}>
      <div
        className="agent-context-menu"
        style={{ left: state.x, top: state.y }}
        onPointerDown={(event) => event.stopPropagation()}
        role="menu"
        aria-label={`${agentName} menu`}
      >
        <div className="agent-context-title">{agentName}</div>
        <button type="button" role="menuitem" onClick={onDetail}>Detail</button>
        {showTerminal && (
          <button type="button" role="menuitem" onClick={onTerminal}>
            <span>Terminal</span>
            {cmdSlot && <kbd>⌘{cmdSlot}</kbd>}
          </button>
        )}
        <button type="button" role="menuitem" onClick={onStartFreshSession}>Start Fresh Session</button>
        {showPrivateChat && (
          <button type="button" role="menuitem" onClick={onPrivate}>
            {isPrivate ? 'End Private Chat' : 'Private Chat'}
          </button>
        )}
        {showKageBunshin && (
          <button type="button" role="menuitem" onClick={onKageBunshin}>Kage Bunshin</button>
        )}
      </div>
    </div>
  );
}

function ArchivePopover({
  agents,
  onCallBack,
  onDismiss,
  onClose,
}: {
  agents: readonly ProjectAgentDetail[];
  onCallBack: (detail: ProjectAgentDetail) => void;
  onDismiss: (detail: ProjectAgentDetail) => void;
  onClose: () => void;
}) {
  return (
    <div className="archive-popover" role="dialog" aria-label="Archived agents">
      <div className="archive-popover-head">
        <span>Archive</span>
        <button type="button" onClick={onClose} aria-label="Close archive">x</button>
      </div>
      {agents.length === 0 ? (
        <div className="archive-empty">No archived agents.</div>
      ) : (
        <div className="archive-list">
          {agents.map((agent) => (
            <div key={agent.agentId} className="archive-item">
              <div>
                <strong>{agent.displayName}</strong>
                <small>{agent.projectName}</small>
              </div>
              <div className="archive-actions">
                <button type="button" onClick={() => onCallBack(agent)}>Call back</button>
                <button type="button" onClick={() => onDismiss(agent)}>Dismiss</button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
