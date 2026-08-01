import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import type { AgentCli } from '../types/agent-pty';
import type { MagiProvider } from '../types/smart-terminal';
import type { WorkingHero } from '../types/agentbar';
import type { AgentId } from '../types/scene';
import {
  MAGI_PROMPT_TEMPLATE,
  MAGI_PROMPT_PATH,
  SYSTEM_STORAGE_KEY,
  loadMagiPromptTemplate,
  magiHandoffCommand,
  magiProviderOrder,
  magiTranslateCommand,
  normalizeMagiProvider,
} from '../magi-config';
import {
  BBS_POST_PROMPT_TEMPLATE,
  BBS_POST_PROMPT_PATH,
  BBS_REPLY_PROMPT_TEMPLATE,
  BBS_REPLY_PROMPT_PATH,
  loadBbsPostPromptTemplate,
  loadBbsReplyPromptTemplate,
} from '../bbs-config';
import {
  BARTENDER_PULL_CONFLICT_PROMPT_PATH,
  BARTENDER_PULL_CONFLICT_PROMPT,
  BARTENDER_SYNC_CONFLICT_PROMPT_PATH,
  BARTENDER_SYNC_CONFLICT_PROMPT,
  loadBartenderFactoryConflictPrompts,
} from '../bartender-config';
import {
  DEFAULT_VIOLET_SUMMARY_CONFIG,
  VIOLET_SUMMARY_LOG_PATH,
  VIOLET_SUMMARY_PROMPT_PATH,
  VIOLET_SUMMARY_PROMPT_TEMPLATE,
  loadVioletSummaryPromptTemplate,
  normalizeVioletSummaryProvider,
  violetSummaryCommand,
  type VioletSummaryProvider,
} from '../violet-summary-config';
import {
  EMBER_DREAM_AGENT_PROMPT_PATH,
  EMBER_DREAM_AGENT_PROMPT_TEMPLATE,
  EMBER_DREAM_CONSOLIDATE_PROMPT_PATH,
  EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE,
  loadEmberDreamAgentPromptTemplate,
  loadEmberDreamConsolidatePromptTemplate,
} from '../ember-config';
import {
  GITHUB_CLI_INSTALL_URL,
  GITHUB_CLI_LOGIN_COMMAND,
  authConfigStatus,
  deleteAccountSkill,
  deleteTavernHero,
  deleteAccountRule,
  accountDreamsOpen,
  accountDreamsStatus,
  ghAuthStatus,
  hasTauriRuntime,
  importAccountSkillFromPicker,
  inspectWorkspaceProject,
  listAccountRules,
  listAccountSkills,
  loadAccountUserIdentity,
  listArchivedWorkspaceProjects,
  loadTavernHeroProfiles,
  openAccountSkillFolder,
  openAccountSkillsFolder,
  revealTavernHeroFile,
  refreshProviderModelOptions,
  resetDefaultAccountRules,
  removeWorkspaceProject,
  resumeWorkspaceProject,
  saveAccountRule,
  saveAccountUserIdentity,
  saveTavernHeroProfiles,
  resetSystemPrompt,
  saveTerminalEnhancement,
  storageMeasureStart,
  storageMeasureStatus,
  supportedShellsStatus,
  terminalEnhancementStatus,
  type AccountSkillDraft,
  type AccountUserIdentity,
  type AccountRuleDraft,
  type AccountRuleSaveRequest,
  type GhAuthInfo,
  type OAuthConfigStatus,
  type ProjectAgentRecord,
  type StorageMeasurementStatus,
  type SupportedShellStatus,
  type TavernHeroProfileDraft,
  type WorkspaceProject,
} from '../pty-client';
import {
  composeProjectAgentName,
  ProjectAgentName,
  projectAgentNameFields,
  type ProjectAgentNameFields,
} from './ProjectAgentName';
import { ProjectAgentTitlePicker } from './ProjectAgentTitlePicker';
import { ShellComboBox, uniqueShellComboOptions } from './ShellComboBox';
import { HeroAvatarArt, HeroAvatarPicker } from './HeroAvatarPicker';
import { SkillActivationList } from './SkillActivationList';
import { SkillDescription } from './SkillDescription';
import {
  avatarClassForId,
  normalizeHeroAvatarId,
  type HeroAvatarId,
} from '../lib/hero-avatars';
import { skillLoomEntries } from '../lib/account-skills';
import { normalizedRuleTrigger } from '../lib/rule-trigger';
import avatarBartender from '../assets/tavern/optimized/avatars/bartender.webp';
import avatarClaude from '../assets/tavern/optimized/avatars/claude.webp';
import avatarClaudeLantern from '../assets/tavern/optimized/avatars/claude-lantern.webp';
import avatarClaudeQuill from '../assets/tavern/optimized/avatars/claude-quill.webp';
import avatarCodex from '../assets/tavern/optimized/avatars/codex.webp';
import avatarCodexPrism from '../assets/tavern/optimized/avatars/codex-prism.webp';
import avatarCodexSlate from '../assets/tavern/optimized/avatars/codex-slate.webp';
import avatarEmber from '../assets/tavern/optimized/avatars/ember.webp';
import avatarAntigravity from '../assets/tavern/optimized/avatars/antigravity.webp';
import avatarLaughingMan from '../assets/tavern/optimized/avatars/laughing-man.webp';
import avatarMagi from '../assets/tavern/optimized/avatars/magi.webp';
import avatarKimi from '../assets/tavern/optimized/avatars/kimi.webp';
import avatarOpencode from '../assets/tavern/optimized/avatars/opencode.webp';
import avatarPi from '../assets/tavern/optimized/avatars/pi.webp';
import avatarPuppeteer from '../assets/tavern/optimized/avatars/puppeteer.webp';
import avatarUserDefault from '../assets/tavern/optimized/avatars/user-default.webp';
import avatarViolet from '../assets/tavern/optimized/avatars/violet.webp';
import profileStage from '../assets/tavern/optimized/backgrounds/profile-stage.webp';
import tavernRoom from '../assets/tavern/optimized/backgrounds/tavern-room.webp';
import roomClassic from '../assets/tavern/optimized/rooms/classic.webp';
import roomObservatory from '../assets/tavern/optimized/rooms/observatory.webp';
import roomStudy from '../assets/tavern/optimized/rooms/study.webp';
import roomWorkshop from '../assets/tavern/optimized/rooms/workshop.webp';
import tableEmber from '../assets/tavern/optimized/tables/ember.webp';
import tableParchment from '../assets/tavern/optimized/tables/parchment.webp';
import tableStar from '../assets/tavern/optimized/tables/star.webp';
import tableTerminal from '../assets/tavern/optimized/tables/terminal.webp';
import tableWalnut from '../assets/tavern/optimized/tables/walnut.webp';
import tableWarm from '../assets/tavern/optimized/tables/warm.webp';
import iconCommends from '../assets/tavern/icons/commends.svg';
import iconGhost from '../assets/tavern/icons/ghost.svg';
import iconShell from '../assets/tavern/icons/shell.svg';
import iconSkills from '../assets/tavern/icons/skills.svg';
import iconTurns from '../assets/tavern/icons/turns.svg';
import providerIconAntigravity from '../assets/tavern/icons/providers/googlegemini.svg';
import providerIconClaude from '../assets/tavern/icons/providers/claude.svg';
import providerIconCodex from '../assets/tavern/icons/providers/openai.svg';
import providerIconKimi from '../assets/tavern/icons/providers/kimi.svg';
import providerIconOpencode from '../assets/tavern/icons/providers/opencode.svg';
import providerIconPi from '../assets/tavern/icons/providers/pi.svg';
import { LaughingManSettings } from './LaughingManSettings';

interface TavernModalProps {
  open: boolean;
  onClose: () => void;
  initialGhAuth?: GhAuthInfo | null;
  ghosttyTerminalEnhancement?: boolean;
  onGhosttyTerminalEnhancementChange?: (enabled: boolean) => void;
  initialTab?: TavernTab;
  onWorkspaceResumed?: (workspace: WorkspaceProject) => void;
}

export type TavernTab = 'heroes' | 'rules' | 'skills' | 'link' | 'archived';
const SHOW_GOOGLE_DRIVE_CARD = false;
const TAVERN_TABS: TavernTab[] = ['heroes', 'rules', 'skills', 'link', 'archived'];
type ProviderId = 'claude' | 'codex' | 'antigravity' | 'opencode' | 'pi' | 'kimi';
export type AgentCardKind = 'invited' | 'custom';
type SystemHeroId = 'magi' | 'violet' | 'ember' | 'bbs' | 'laughing-man' | 'puppeteer' | 'bartender';
type ProfileTarget =
  | { type: 'agent'; id: string }
  | { type: 'system'; id: SystemHeroId }
  | { type: 'user' };
type TavernLoadTask = 'heroes' | 'account' | 'archived' | 'shells' | 'skills' | 'rules' | 'prompts';
type TavernPrepareTask = 'heroes' | 'accountUser' | 'images';
export interface TavernLoadingLogItem {
  id: string;
  label: string;
  startedAt: number;
}
type RawHeroFile = 'GHOST.md' | 'SHELL.yaml';
interface ProviderSpec {
  id: ProviderId;
  name: string;
  cli: string;
  icon: string;
  installUrl: string;
  defaultModel: string;
  defaultEffort?: string;
  defaultAvatarId: HeroAvatarId;
  beta?: boolean;
}

interface AgentCardSpec {
  id: string;
  kind: AgentCardKind;
  provider: ProviderId;
  name: string;
}

interface HeroDraft {
  name: string;
  nameFields?: ProjectAgentNameFields | null;
  provider: ProviderId;
  model: string;
  effort?: string;
  avatarId: HeroAvatarId;
  skills: string[];
  ghost: string;
  shell: string;
  record?: ProjectAgentRecord | null;
  archived?: boolean;
  dismissed?: boolean;
}

interface SystemHeroSpec {
  id: SystemHeroId;
  name: string;
  role: string;
  avatarClass: string;
  description: string;
  configKind: 'magi' | 'violet' | 'ember' | 'bbs' | 'laughing-man' | 'placeholder' | 'bartender';
}

interface SystemHeroDraft {
  provider?: MagiProvider;
  startupArgs?: string;
  prompt?: string;
  intervalMinutes?: number;
  launchCommand?: string;
  summaryTriggerMessages?: number;
  summaryTriggerHours?: number;
  summaryTriggerMinOutstanding?: number;
  conflictPrompt?: string;
  pullConflictPrompt?: string;
  bbsPostPrompt?: string;
  bbsReplyPrompt?: string;
}

interface SystemPromptTemplates {
  magiPrompt: string;
  violetSummaryPrompt: string;
  emberDreamAgentPrompt: string;
  emberDreamConsolidatePrompt: string;
  bbsPostPrompt: string;
  bbsReplyPrompt: string;
  bartenderSyncConflictPrompt: string;
  bartenderPullConflictPrompt: string;
}

type SystemPromptTemplateKey = keyof SystemPromptTemplates;

interface SystemPromptResetTarget {
  key: SystemPromptTemplateKey;
  path: string;
  fallback: string;
}

const PROFILE_STORAGE_KEY = 'kota-v2.tavern.hero-profiles';
const CUSTOM_HERO_STORAGE_KEY = 'kota-v2.tavern.custom-heroes';
const TAVERN_PREPARE_LABELS: Record<TavernPrepareTask, string> = {
  heroes: 'Heroes',
  accountUser: 'Account user',
  images: 'Images',
};
const TAVERN_PREPARE_TIMEOUT_MS = 5000;
const OPENCODE_LEGACY_KIMI_MODEL = 'kimi-k2.6';
const OPENCODE_KIMI_MODEL = 'kimi-for-coding/k2p6';
const PI_DEFAULT_MODEL = 'zai/glm-5.2';
export const TAVERN_PROFILE_CHANGED_EVENT = 'kota-v2:tavern-profile-changed';
export const TAVERN_HERO_CREDIT_CHANGED_EVENT = 'kota-v2:tavern-hero-credit-changed';
const DEFAULT_ACCOUNT_USER_IDENTITY: AccountUserIdentity = { name: 'User', avatarId: 'user-default' };

export interface TavernHeroIncarnationProfile {
  agentId: AgentId;
  kind: AgentCardKind;
  cli: AgentCli;
  name: string;
  provider: ProviderId;
  model: string;
  effort?: string;
  avatarId: HeroAvatarId;
  skills: string[];
  ghost: string;
  shell: string;
  args: string[];
}

const TAVERN_PRELOAD_ASSETS = [
  tavernRoom,
  profileStage,
  avatarClaude,
  avatarClaudeLantern,
  avatarClaudeQuill,
  avatarCodex,
  avatarCodexPrism,
  avatarCodexSlate,
  avatarAntigravity,
  avatarOpencode,
  avatarPi,
  avatarKimi,
  avatarMagi,
  avatarViolet,
  avatarEmber,
  avatarLaughingMan,
  avatarPuppeteer,
  avatarBartender,
  avatarUserDefault,
  iconCommends,
  iconTurns,
  iconSkills,
  iconShell,
  iconGhost,
  roomClassic,
  roomStudy,
  roomWorkshop,
  roomObservatory,
  tableWarm,
  tableWalnut,
  tableEmber,
  tableTerminal,
  tableStar,
  tableParchment,
];

const TAVERN_CRITICAL_PRELOAD_ASSETS = [
  tavernRoom,
  profileStage,
  avatarClaude,
  avatarCodex,
  avatarAntigravity,
  avatarOpencode,
  avatarPi,
  avatarKimi,
  avatarUserDefault,
  roomClassic,
];

interface PreparedTavernOpenState {
  profiles?: TavernHeroProfileDraft[];
  accountUser?: AccountUserIdentity;
}

interface PreparedTavernAccountStatus {
  config: OAuthConfigStatus;
  storageMeasurement: StorageMeasurementStatus;
}

let preparedTavernOpenState: PreparedTavernOpenState | null = null;

export function preloadTavernAssets(delayMs = 1200): () => void {
  if (typeof window === 'undefined') return () => {};
  let cancelled = false;
  const timer = window.setTimeout(() => {
    window.requestAnimationFrame(() => {
      if (cancelled) return;
      TAVERN_PRELOAD_ASSETS.forEach((src) => {
        const img = new Image();
        img.decoding = 'async';
        img.src = src;
      });
    });
  }, delayMs);
  return () => {
    cancelled = true;
    window.clearTimeout(timer);
  };
}

export async function prepareTavernForOpen(onLoadingChange?: (items: TavernLoadingLogItem[]) => void): Promise<void> {
  if (typeof window === 'undefined') return;
  const activeTasks = new Map<TavernPrepareTask, TavernLoadingLogItem>();
  const publishLoading = () => onLoadingChange?.(Array.from(activeTasks.values()));
  const trackTask = <T,>(task: TavernPrepareTask, promise: Promise<T>): Promise<T> => {
    activeTasks.set(task, {
      id: task,
      label: TAVERN_PREPARE_LABELS[task],
      startedAt: Date.now(),
    });
    publishLoading();
    return withTavernPrepareTimeout(promise).finally(() => {
      activeTasks.delete(task);
      publishLoading();
    });
  };

  const [
    profilesResult,
    accountUserResult,
  ] = await Promise.allSettled([
    trackTask('heroes', loadTavernHeroProfiles()),
    trackTask('accountUser', loadAccountUserIdentity()),
    trackTask('images', decodeTavernImages(TAVERN_CRITICAL_PRELOAD_ASSETS)),
  ]).finally(() => {
    activeTasks.clear();
    publishLoading();
  });
  const prepared: PreparedTavernOpenState = {};
  if (profilesResult.status === 'fulfilled') {
    syncTavernHeroStorageFromProfiles(profilesResult.value);
    prepared.profiles = profilesResult.value;
  }
  if (accountUserResult.status === 'fulfilled') {
    prepared.accountUser = accountUserResult.value;
  }
  if (Object.keys(prepared).length > 0) {
    preparedTavernOpenState = prepared;
  }
  await nextTavernPaint();
}

async function loadTavernAccountStatus(): Promise<PreparedTavernAccountStatus> {
  const [config, storageMeasurement] = await Promise.all([
    authConfigStatus(),
    storageMeasureStatus(),
  ]);
  return { config, storageMeasurement };
}

function withTavernPrepareTimeout<T>(task: Promise<T>): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = window.setTimeout(() => reject(new Error('Tavern preparation timed out')), TAVERN_PREPARE_TIMEOUT_MS);
    task.then(
      (value) => {
        window.clearTimeout(timer);
        resolve(value);
      },
      (err) => {
        window.clearTimeout(timer);
        reject(err);
      },
    );
  });
}

function consumePreparedTavernOpenState(): PreparedTavernOpenState | null {
  const prepared = preparedTavernOpenState;
  preparedTavernOpenState = null;
  return prepared;
}

async function decodeTavernImages(sources: readonly string[]): Promise<void> {
  await Promise.all(sources.map((src) => decodeTavernImage(src)));
}

async function decodeTavernImage(src: string): Promise<void> {
  const img = new Image();
  img.decoding = 'async';
  img.src = src;
  try {
    if (typeof img.decode === 'function') {
      await img.decode();
      return;
    }
    await new Promise<void>((resolve) => {
      img.onload = () => resolve();
      img.onerror = () => resolve();
    });
  } catch {
    // A missing decorative image must not trap the user in the opening gate.
  }
}

function nextTavernPaint(): Promise<void> {
  return new Promise((resolve) => {
    window.requestAnimationFrame(() => {
      window.requestAnimationFrame(() => resolve());
    });
  });
}

const PROVIDERS: Record<ProviderId, ProviderSpec> = {
  claude: {
    id: 'claude',
    name: 'Claude Code',
    cli: 'claude',
    icon: providerIconClaude,
    installUrl: 'https://docs.anthropic.com/en/docs/claude-code/setup',
    defaultModel: 'default',
    defaultEffort: 'max',
    defaultAvatarId: 'claude',
  },
  codex: {
    id: 'codex',
    name: 'Codex',
    cli: 'codex',
    icon: providerIconCodex,
    installUrl: 'https://github.com/openai/codex',
    defaultModel: 'default',
    defaultEffort: 'xhigh',
    defaultAvatarId: 'codex',
  },
  antigravity: {
    id: 'antigravity',
    name: 'Antigravity CLI',
    cli: 'agy',
    icon: providerIconAntigravity,
    installUrl: 'https://www.antigravity.google/docs/cli/cli-getting-started',
    defaultModel: 'default',
    defaultAvatarId: 'antigravity',
    beta: true,
  },
  opencode: {
    id: 'opencode',
    name: 'OpenCode',
    cli: 'opencode',
    icon: providerIconOpencode,
    installUrl: 'https://opencode.ai/docs',
    defaultModel: 'opencode/deepseek-v4-flash-free',
    defaultAvatarId: 'opencode',
    beta: true,
  },
  pi: {
    id: 'pi',
    name: 'Pi',
    cli: 'pi',
    icon: providerIconPi,
    installUrl: 'https://pi.dev',
    defaultModel: PI_DEFAULT_MODEL,
    defaultEffort: 'xhigh',
    defaultAvatarId: 'pi',
    beta: true,
  },
  kimi: {
    id: 'kimi',
    name: 'Kimi Code',
    cli: 'kimi',
    icon: providerIconKimi,
    installUrl: 'https://code.kimi.com/',
    defaultModel: 'default',
    defaultAvatarId: 'kimi',
    beta: true,
  },
};

const PROVIDER_IDS = Object.keys(PROVIDERS) as ProviderId[];

const HERO_TEMPLATES: AgentCardSpec[] = [
  { id: 'hero-cc', kind: 'custom', provider: 'claude', name: 'CC' },
  { id: 'hero-dex', kind: 'custom', provider: 'codex', name: 'Dex' },
  { id: 'hero-gem', kind: 'custom', provider: 'antigravity', name: 'Gem' },
  { id: 'hero-op', kind: 'custom', provider: 'opencode', name: 'Op' },
  { id: 'hero-pi', kind: 'custom', provider: 'pi', name: 'Pi' },
  { id: 'hero-kimi', kind: 'custom', provider: 'kimi', name: 'Kimi' },
];
const DEFAULT_HERO_TEMPLATE_IDS = new Set(HERO_TEMPLATES.map((hero) => hero.id));
const FACTORY_DEFAULT_MODEL_MIGRATIONS: Record<string, { provider: ProviderId; model: string }> = {
  'hero-cc': { provider: 'claude', model: 'claude-opus-4-8[1m]' },
  'hero-dex': { provider: 'codex', model: 'gpt-5.5' },
};
const LEGACY_FACTORY_TEMPLATE_IDS = new Set(['alice', 'bob', 'charlie', 'david', 'claude', 'codex']);

const TAVERN_ROMAN_SUFFIXES: Record<number, string> = {
  2: 'II',
  3: 'III',
  4: 'IV',
  5: 'V',
  6: 'VI',
  7: 'VII',
  8: 'VIII',
  9: 'IX',
};

const SYSTEM_HEROES: SystemHeroSpec[] = [
  {
    id: 'magi',
    name: 'Magi',
    role: 'Smart Shell',
    avatarClass: 'system-magi',
    description: 'The command-line spellwork behind Smart Shell and the # ask handoff.',
    configKind: 'magi',
  },
  {
    id: 'violet',
    name: 'Violet',
    role: 'Notes',
    avatarClass: 'system-violet',
    description: 'The automatic note doll that reads native logs and keeps the room memory tidy.',
    configKind: 'violet',
  },
  {
    id: 'ember',
    name: 'Ember',
    role: 'Reminder',
    avatarClass: 'system-ember',
    description: 'A small reminder service for later prompts, nudges, and lightweight automation.',
    configKind: 'ember',
  },
  {
    id: 'bbs',
    name: 'BBS',
    role: 'Bulletin Board',
    avatarClass: 'system-bbs',
    description: 'Cross-project Bulletin Board handoff entrypoints.',
    configKind: 'bbs',
  },
  {
    id: 'laughing-man',
    name: 'Laughing Man',
    role: 'Telegram',
    avatarClass: 'system-laughing-man',
    description: 'Telegram bridge into Kota rooms. Configure here or from the room\'s right-column card; history lives on the card.',
    configKind: 'laughing-man',
  },
  {
    id: 'puppeteer',
    name: 'Puppeteer',
    role: 'Routing',
    avatarClass: 'system-puppeteer',
    description: 'A future topic-based routing mechanism. Placeholder only for now.',
    configKind: 'placeholder',
  },
  {
    id: 'bartender',
    name: 'Bartender',
    role: 'Sync',
    avatarClass: 'system-bartender',
    description: 'The steward watching worktrees, publish gates, and conflict handoff prompts.',
    configKind: 'bartender',
  },
];

const DEFAULT_SKILLS = ['frontend-design'];
export const FACTORY_HERO_GHOST = [
  'Prefer concrete file references and clear handoff notes.',
  'Preserve unknown changes from the user or other agents.',
  "Do not revert another agent's work unless explicitly requested.",
  'Concise, precise, and occasionally offer some emotional value.',
].join('\n');

function defaultSystemPromptTemplates(): SystemPromptTemplates {
  return {
    magiPrompt: MAGI_PROMPT_TEMPLATE,
    violetSummaryPrompt: VIOLET_SUMMARY_PROMPT_TEMPLATE,
    emberDreamAgentPrompt: EMBER_DREAM_AGENT_PROMPT_TEMPLATE,
    emberDreamConsolidatePrompt: EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE,
    bbsPostPrompt: BBS_POST_PROMPT_TEMPLATE,
    bbsReplyPrompt: BBS_REPLY_PROMPT_TEMPLATE,
    bartenderSyncConflictPrompt: BARTENDER_SYNC_CONFLICT_PROMPT,
    bartenderPullConflictPrompt: BARTENDER_PULL_CONFLICT_PROMPT,
  };
}

const SYSTEM_PROMPT_RESET_TARGETS: Partial<Record<SystemHeroId, SystemPromptResetTarget[]>> = {
  magi: [
    { key: 'magiPrompt', path: MAGI_PROMPT_PATH, fallback: MAGI_PROMPT_TEMPLATE },
  ],
  violet: [
    { key: 'violetSummaryPrompt', path: VIOLET_SUMMARY_PROMPT_PATH, fallback: VIOLET_SUMMARY_PROMPT_TEMPLATE },
  ],
  ember: [
    { key: 'emberDreamAgentPrompt', path: EMBER_DREAM_AGENT_PROMPT_PATH, fallback: EMBER_DREAM_AGENT_PROMPT_TEMPLATE },
    { key: 'emberDreamConsolidatePrompt', path: EMBER_DREAM_CONSOLIDATE_PROMPT_PATH, fallback: EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE },
  ],
  bbs: [
    { key: 'bbsPostPrompt', path: BBS_POST_PROMPT_PATH, fallback: BBS_POST_PROMPT_TEMPLATE },
    { key: 'bbsReplyPrompt', path: BBS_REPLY_PROMPT_PATH, fallback: BBS_REPLY_PROMPT_TEMPLATE },
  ],
  bartender: [
    { key: 'bartenderSyncConflictPrompt', path: BARTENDER_SYNC_CONFLICT_PROMPT_PATH, fallback: BARTENDER_SYNC_CONFLICT_PROMPT },
    { key: 'bartenderPullConflictPrompt', path: BARTENDER_PULL_CONFLICT_PROMPT_PATH, fallback: BARTENDER_PULL_CONFLICT_PROMPT },
  ],
};

function systemPromptResetTargets(heroId: SystemHeroId): SystemPromptResetTarget[] {
  return SYSTEM_PROMPT_RESET_TARGETS[heroId] ?? [];
}

async function loadSystemPromptTemplates(): Promise<SystemPromptTemplates> {
  const [
    magiPrompt,
    violetSummaryPrompt,
    emberDreamAgentPrompt,
    emberDreamConsolidatePrompt,
    bbsPostPrompt,
    bbsReplyPrompt,
    bartenderPrompts,
  ] = await Promise.all([
    loadMagiPromptTemplate(),
    loadVioletSummaryPromptTemplate(),
    loadEmberDreamAgentPromptTemplate(),
    loadEmberDreamConsolidatePromptTemplate(),
    loadBbsPostPromptTemplate(),
    loadBbsReplyPromptTemplate(),
    loadBartenderFactoryConflictPrompts(),
  ]);
  return {
    magiPrompt,
    violetSummaryPrompt,
    emberDreamAgentPrompt,
    emberDreamConsolidatePrompt,
    bbsPostPrompt,
    bbsReplyPrompt,
    bartenderSyncConflictPrompt: bartenderPrompts.conflictPrompt,
    bartenderPullConflictPrompt: bartenderPrompts.pullConflictPrompt,
  };
}

function sameRuleDraft(a: AccountRuleDraft, b: AccountRuleDraft): boolean {
  return (
    a.title.trim() === b.title.trim() &&
    a.loadPolicy === b.loadPolicy &&
    normalizedRuleTrigger(a) === normalizedRuleTrigger(b) &&
    a.body.trim() === b.body.trim()
  );
}

function ruleSaveRequest(rule: AccountRuleDraft): AccountRuleSaveRequest {
  return {
    fileName: rule.fileName || null,
    title: rule.title,
    loadPolicy: rule.loadPolicy,
    taskTrigger: normalizedRuleTrigger(rule),
    body: rule.body,
  };
}

const LEGACY_TEMPLATE_NAMES: Record<ProviderId, readonly string[]> = {
  claude: ['CC', 'Alice', 'Claude', 'Claude Code', 'Amber Clerk'],
  codex: ['Dex', 'Bob', 'Codex', 'Glass Scribe'],
  antigravity: ['Agy', 'Gem', 'David', 'Gemini', 'Gemini CLI', 'Antigravity', 'Antigravity CLI', 'Twin Star'],
  opencode: ['Op', 'Charlie', 'OpenCode', 'OpenCode CLI', 'Open Lantern'],
  pi: ['Pi', 'Pi CLI', 'Pi Agent'],
  kimi: ['Kimi', 'Kimi Code', 'Kimi Code CLI'],
};
const STALE_FAKE_CUSTOM_HERO_NAMES = new Set([
  'Amber Clerk',
  'Glass Scribe',
  'Twin Star',
  'Open Lantern',
  'Claude',
  'Claude Code',
  'Codex',
  'Gemini',
  'Gemini CLI',
  'Antigravity',
  'Antigravity CLI',
  'OpenCode',
  'OpenCode CLI',
  'Pi',
  'Pi CLI',
  'Pi Agent',
]);

function skillImportErrorText(err: unknown): string {
  const message = err instanceof Error ? err.message : String(err);
  return message.replace(/^Error:\s*/, '') || 'Could not import skill.';
}

export function TavernModal({
  open,
  onClose,
  initialGhAuth = null,
  ghosttyTerminalEnhancement = false,
  onGhosttyTerminalEnhancementChange,
  initialTab = 'heroes',
  onWorkspaceResumed,
}: TavernModalProps) {
  const [tab, setTab] = useState<TavernTab>('heroes');
  const [config, setConfig] = useState<OAuthConfigStatus | null>(null);
  const [storageMeasurement, setStorageMeasurement] = useState<StorageMeasurementStatus | null>(null);
  const [ghAuth, setGhAuth] = useState<GhAuthInfo | null>(initialGhAuth);
  const [archivedWorkspaces, setArchivedWorkspaces] = useState<WorkspaceProject[]>([]);
  const [shells, setShells] = useState<SupportedShellStatus[]>([]);
  const [modelRefreshBusy, setModelRefreshBusy] = useState<ProviderId | null>(null);
  const [customHeroes, setCustomHeroes] = useState<AgentCardSpec[]>(() => loadCustomHeroes());
  const [selectedHeroId, setSelectedHeroId] = useState(HERO_TEMPLATES[0].id);
  const [profileTarget, setProfileTarget] = useState<ProfileTarget | null>(null);
  const [profiles, setProfiles] = useState<Record<string, Partial<HeroDraft>>>(() =>
    loadProfileDrafts(),
  );
  const [systemDrafts, setSystemDrafts] = useState<Record<SystemHeroId, SystemHeroDraft>>(() =>
    loadSystemDrafts(),
  );
  const [accountUser, setAccountUser] = useState<AccountUserIdentity>(DEFAULT_ACCOUNT_USER_IDENTITY);
  const [accountUserSaving, setAccountUserSaving] = useState(false);
  const [systemPromptTemplates, setSystemPromptTemplates] = useState<SystemPromptTemplates>(() =>
    defaultSystemPromptTemplates(),
  );
  const [systemPromptsLoaded, setSystemPromptsLoaded] = useState(() => !hasTauriRuntime());
  const [loadingTasks, setLoadingTasks] = useState<Partial<Record<TavernLoadTask, number>>>({});
  const [accountRules, setAccountRules] = useState<AccountRuleDraft[]>([]);
  const [selectedRuleFile, setSelectedRuleFile] = useState<string | null>(null);
  const [ruleDraft, setRuleDraft] = useState<AccountRuleDraft | null>(null);
  const [rulesBusy, setRulesBusy] = useState(false);
  const [rulesLoading, setRulesLoading] = useState(false);
  const [rulesLoaded, setRulesLoaded] = useState(false);
  const [ruleAutoStatus, setRuleAutoStatus] = useState<'idle' | 'editing' | 'saving' | 'saved'>('idle');
  const [deleteRuleTarget, setDeleteRuleTarget] = useState<AccountRuleDraft | null>(null);
  const [accountSkills, setAccountSkills] = useState<AccountSkillDraft[]>([]);
  const [skillsBusy, setSkillsBusy] = useState(false);
  const [skillsLoading, setSkillsLoading] = useState(false);
  const [skillsLoaded, setSkillsLoaded] = useState(false);
  const [archivedLoading, setArchivedLoading] = useState(false);
  const [archivedLoaded, setArchivedLoaded] = useState(false);
  const [deleteSkillTarget, setDeleteSkillTarget] = useState<AccountSkillDraft | null>(null);
  const [promptResetTarget, setPromptResetTarget] = useState<SystemHeroSpec | null>(null);
  const [promptResetBusy, setPromptResetBusy] = useState(false);
  const [systemOpen, setSystemOpen] = useState(false);
  const [ghostExpanded, setGhostExpanded] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [skillImportDialog, setSkillImportDialog] = useState<{
    tone: 'success' | 'error';
    title: string;
    body: string;
  } | null>(null);
  const [heroFilesReady, setHeroFilesReady] = useState(false);
  const [storageAgeNow, setStorageAgeNow] = useState(() => Date.now());
  const saveProfilesTimerRef = useRef<number | null>(null);
  const lastSavedProfilesPayloadRef = useRef<string | null>(null);
  const profilePersistenceHeroesRef = useRef<AgentCardSpec[]>([...HERO_TEMPLATES, ...customHeroes]);
  const saveRuleTimerRef = useRef<number | null>(null);
  const saveRuleSeqRef = useRef(0);
  const systemPromptLoadRef = useRef<Promise<void> | null>(null);
  const mountedRef = useRef(false);
  const backButtonRef = useRef<HTMLButtonElement | null>(null);

  const setTaskLoading = useCallback((task: TavernLoadTask, loading: boolean) => {
    setLoadingTasks((prev) => {
      if (!!prev[task] === loading) return prev;
      const next = { ...prev };
      if (loading) next[task] = Date.now();
      else delete next[task];
      return next;
    });
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  useEffect(() => {
    setGhAuth(initialGhAuth);
  }, [initialGhAuth]);

  useEffect(() => {
    profilePersistenceHeroesRef.current = [...HERO_TEMPLATES, ...customHeroes];
  }, [customHeroes]);

  const refreshAccount = useCallback(async () => {
    const status = await loadTavernAccountStatus();
    setConfig(status.config);
    setStorageMeasurement(status.storageMeasurement);
    setStorageAgeNow(Date.now());
  }, []);

  const beginStorageMeasurement = useCallback(async () => {
    setStorageMeasurement((current) => ({
      updating: true,
      onDiskBytes: current?.onDiskBytes ?? null,
      availableBytes: current?.availableBytes ?? null,
      measuredAt: current?.measuredAt ?? null,
      error: null,
    }));
    try {
      const status = await storageMeasureStart();
      if (!mountedRef.current) return;
      setStorageMeasurement(status);
      setStorageAgeNow(Date.now());
    } catch (err) {
      if (!mountedRef.current) return;
      setStorageMeasurement((current) => ({
        updating: false,
        onDiskBytes: current?.onDiskBytes ?? null,
        availableBytes: current?.availableBytes ?? null,
        measuredAt: current?.measuredAt ?? null,
        error: String(err).replace(/^Error:\s*/, '') || 'Refresh failed',
      }));
    }
  }, []);

  const refreshArchivedWorkspaces = useCallback(async () => {
    setArchivedWorkspaces(await listArchivedWorkspaceProjects());
    setArchivedLoaded(true);
  }, []);

  const refreshShells = useCallback(async () => {
    setShells(await supportedShellsStatus());
  }, []);

  const refreshAccountUser = useCallback(async () => {
    setAccountUser(await loadAccountUserIdentity());
  }, []);

  const refreshTerminalEnhancement = useCallback(async () => {
    const status = await terminalEnhancementStatus();
    onGhosttyTerminalEnhancementChange?.(status.ghosttyTerminalEnhancementEnabled);
  }, [onGhosttyTerminalEnhancementChange]);

  const loadSystemPrompts = useCallback(() => {
    if (systemPromptsLoaded) return Promise.resolve();
    if (systemPromptLoadRef.current) return systemPromptLoadRef.current;
    setTaskLoading('prompts', true);
    const task = loadSystemPromptTemplates()
      .then((templates) => {
        if (!mountedRef.current) return;
        setSystemPromptTemplates(templates);
        setSystemPromptsLoaded(true);
      })
      .catch((err) => {
        if (!mountedRef.current) return;
        setError(String(err));
        systemPromptLoadRef.current = null;
      })
      .finally(() => {
        if (mountedRef.current) setTaskLoading('prompts', false);
      });
    systemPromptLoadRef.current = task;
    return task;
  }, [setTaskLoading, systemPromptsLoaded]);

  useEffect(() => {
    if (!open) return;
    setTab(initialTab);
    setError(null);
    setSkillImportDialog(null);
    setLoadingTasks({});
    setHeroFilesReady(false);
    lastSavedProfilesPayloadRef.current = null;
    let cancelled = false;
    const applyProfiles = (savedProfiles: TavernHeroProfileDraft[]) => {
      const synced = tavernStateFromProfiles(savedProfiles);
      setCustomHeroes(synced.customHeroes);
      setProfiles(synced.profiles);
      lastSavedProfilesPayloadRef.current = synced.migratedFactoryDefaults
        ? null
        : serializeTavernProfilesForPersistence(
          [...HERO_TEMPLATES, ...synced.customHeroes],
          synced.profiles,
        );
      setHeroFilesReady(true);
    };
    const prepared = consumePreparedTavernOpenState();
    if (prepared?.profiles) {
      applyProfiles(prepared.profiles);
      setTaskLoading('heroes', false);
    } else {
      setTaskLoading('heroes', true);
      void loadTavernHeroProfiles()
        .then((savedProfiles) => {
          if (cancelled) return;
          syncTavernHeroStorageFromProfiles(savedProfiles);
          applyProfiles(savedProfiles);
        })
        .catch((err) => {
          if (!cancelled) setError(String(err));
        })
        .finally(() => {
          if (!cancelled) setTaskLoading('heroes', false);
        });
    }
    if (prepared?.accountUser) {
      setAccountUser(prepared.accountUser);
    } else {
      void refreshAccountUser().catch((err) => {
        if (!cancelled) setError(String(err));
      });
    }
    setTaskLoading('account', true);
    void refreshAccount()
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setTaskLoading('account', false);
      });
    setTaskLoading('shells', true);
    void refreshShells()
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (!cancelled) setTaskLoading('shells', false);
      });
    const cancelPreload = preloadTavernAssets(900);
    return () => {
      cancelPreload();
      cancelled = true;
    };
  }, [
    initialTab,
    open,
    refreshAccount,
    refreshAccountUser,
    refreshShells,
    setTaskLoading,
  ]);

  useEffect(() => {
    if (!open || tab !== 'link' || !storageMeasurement?.updating) return;
    let cancelled = false;
    let inFlight = false;
    const poll = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const status = await storageMeasureStatus();
        if (!cancelled) {
          setStorageMeasurement(status);
          setStorageAgeNow(Date.now());
        }
      } catch {
        // Keep the optimistic Updating state. The next cheap status poll may recover.
      } finally {
        inFlight = false;
      }
    };
    const timer = window.setInterval(() => void poll(), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [open, storageMeasurement?.updating, tab]);

  useEffect(() => {
    if (!open || tab !== 'link' || storageMeasurement?.measuredAt == null) return;
    setStorageAgeNow(Date.now());
    const timer = window.setInterval(() => setStorageAgeNow(Date.now()), 60_000);
    return () => window.clearInterval(timer);
  }, [open, storageMeasurement?.measuredAt, tab]);

  useEffect(() => {
    if (!open) return;
    const frame = window.requestAnimationFrame(() => {
      backButtonRef.current?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      event.stopPropagation();
      if (skillImportDialog) {
        setSkillImportDialog(null);
      } else if (promptResetTarget) {
        setPromptResetTarget(null);
      } else if (deleteRuleTarget) {
        setDeleteRuleTarget(null);
      } else if (deleteSkillTarget) {
        setDeleteSkillTarget(null);
      } else if (profileTarget?.type === 'agent') {
        return;
      } else if (profileTarget) {
        setProfileTarget(null);
      } else {
        onClose();
      }
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [deleteRuleTarget, deleteSkillTarget, onClose, open, profileTarget, promptResetTarget, skillImportDialog]);

  useEffect(() => {
    if (!open) return;
    const refreshHeroRecords = () => {
      void loadTavernHeroProfiles()
        .then((savedProfiles) => {
          setProfiles((prev) => {
            const next = { ...prev };
            for (const profile of savedProfiles) {
              next[profile.heroId] = {
                ...next[profile.heroId],
                record: profile.record ?? next[profile.heroId]?.record ?? null,
              };
            }
            lastSavedProfilesPayloadRef.current = serializeTavernProfilesForPersistence(
              profilePersistenceHeroesRef.current,
              next,
            );
            return next;
          });
        })
        .catch((err) => setError(String(err)));
    };
    window.addEventListener(TAVERN_HERO_CREDIT_CHANGED_EVENT, refreshHeroRecords);
    return () => window.removeEventListener(TAVERN_HERO_CREDIT_CHANGED_EVENT, refreshHeroRecords);
  }, [open]);

  useEffect(() => {
    try {
      window.localStorage.setItem(PROFILE_STORAGE_KEY, JSON.stringify(profiles));
      window.dispatchEvent(new Event(TAVERN_PROFILE_CHANGED_EVENT));
    } catch {
      // Draft persistence is best-effort.
    }
  }, [profiles]);

  useEffect(() => {
    if (!heroFilesReady) return;
    const nextProfiles = tavernProfilesForPersistence([...HERO_TEMPLATES, ...customHeroes], profiles);
    const nextPayload = JSON.stringify(nextProfiles);
    if (nextPayload === lastSavedProfilesPayloadRef.current) return;
    if (saveProfilesTimerRef.current != null) {
      window.clearTimeout(saveProfilesTimerRef.current);
    }
    saveProfilesTimerRef.current = window.setTimeout(() => {
      saveProfilesTimerRef.current = null;
      void saveTavernHeroProfiles(nextProfiles)
        .then(() => {
          lastSavedProfilesPayloadRef.current = nextPayload;
        })
        .catch((err) => {
          setError(String(err));
        });
    }, 250);
    return () => {
      if (saveProfilesTimerRef.current != null) {
        window.clearTimeout(saveProfilesTimerRef.current);
        saveProfilesTimerRef.current = null;
      }
    };
  }, [customHeroes, heroFilesReady, profiles]);

  useEffect(() => {
    try {
      window.localStorage.setItem(CUSTOM_HERO_STORAGE_KEY, JSON.stringify(customHeroes));
      window.dispatchEvent(new Event(TAVERN_PROFILE_CHANGED_EVENT));
    } catch {
      // Draft persistence is best-effort.
    }
  }, [customHeroes]);

  useEffect(() => {
    try {
      window.localStorage.setItem(SYSTEM_STORAGE_KEY, JSON.stringify(systemDrafts));
      window.dispatchEvent(new Event(TAVERN_PROFILE_CHANGED_EVENT));
    } catch {
      // Draft persistence is best-effort.
    }
  }, [systemDrafts]);

  useEffect(() => {
    const selected = accountRules.find((rule) => rule.fileName === selectedRuleFile) ?? accountRules[0] ?? null;
    setRuleDraft((prev) => {
      if (!selected) return null;
      if (prev?.fileName === selected.fileName && !sameRuleDraft(prev, selected)) return prev;
      return { ...selected };
    });
  }, [accountRules, selectedRuleFile]);

  useEffect(() => () => {
    if (saveRuleTimerRef.current != null) {
      window.clearTimeout(saveRuleTimerRef.current);
      saveRuleTimerRef.current = null;
    }
  }, []);

  useEffect(() => {
    if (!open || tab !== 'rules' || !ruleDraft?.fileName) return;
    const saved = accountRules.find((rule) => rule.fileName === ruleDraft.fileName);
    if (!saved) return;
    if (sameRuleDraft(ruleDraft, saved)) {
      setRuleAutoStatus((prev) => (prev === 'editing' || prev === 'saving' ? 'saved' : prev));
      return;
    }
    if (saveRuleTimerRef.current != null) {
      window.clearTimeout(saveRuleTimerRef.current);
    }
    setRuleAutoStatus('editing');
    const snapshot = ruleSaveRequest(ruleDraft);
    saveRuleTimerRef.current = window.setTimeout(() => {
      saveRuleTimerRef.current = null;
      const seq = saveRuleSeqRef.current + 1;
      saveRuleSeqRef.current = seq;
      setRuleAutoStatus('saving');
      void saveAccountRule(snapshot)
        .then((rules) => {
          setAccountRules(rules);
          if (selectedRuleFile !== snapshot.fileName) return;
          if (saveRuleSeqRef.current === seq) setRuleAutoStatus('saved');
        })
        .catch((err) => {
          setRuleAutoStatus('idle');
          setError(String(err));
        });
    }, 500);
    return () => {
      if (saveRuleTimerRef.current != null) {
        window.clearTimeout(saveRuleTimerRef.current);
        saveRuleTimerRef.current = null;
      }
    };
  }, [accountRules, open, ruleDraft, selectedRuleFile, tab]);

  useLayoutEffect(() => {
    if (!open || tab !== 'archived' || archivedLoaded) return;
    let cancelled = false;
    setArchivedLoading(true);
    setTaskLoading('archived', true);
    void refreshArchivedWorkspaces()
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (mountedRef.current) {
          setArchivedLoading(false);
          setTaskLoading('archived', false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [archivedLoaded, open, refreshArchivedWorkspaces, setTaskLoading, tab]);

  useLayoutEffect(() => {
    if (!open || tab !== 'rules' || rulesLoaded) return;
    let cancelled = false;
    setRulesLoading(true);
    setTaskLoading('rules', true);
    void listAccountRules()
      .then((rules) => {
        if (cancelled) return;
        setAccountRules(rules);
        setSelectedRuleFile((prev) => prev ?? rules[0]?.fileName ?? null);
        setRulesLoaded(true);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (mountedRef.current) {
          setRulesLoading(false);
          setTaskLoading('rules', false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, rulesLoaded, setTaskLoading, tab]);

  useLayoutEffect(() => {
    if (!open || skillsLoaded) return;
    const shouldLoadSkills = tab === 'skills' || profileTarget?.type === 'agent';
    if (!shouldLoadSkills) return;
    let cancelled = false;
    setSkillsBusy(true);
    setSkillsLoading(true);
    setTaskLoading('skills', true);
    void listAccountSkills()
      .then((skills) => {
        if (!cancelled) setAccountSkills(skills);
        if (!cancelled) setSkillsLoaded(true);
      })
      .catch((err) => {
        if (!cancelled) setError(String(err));
      })
      .finally(() => {
        if (mountedRef.current) {
          setSkillsBusy(false);
          setSkillsLoading(false);
          setTaskLoading('skills', false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [open, profileTarget?.type, setTaskLoading, skillsLoaded, tab]);

  useEffect(() => {
    if (!open || tab !== 'link') return;
    let cancelled = false;
    void refreshTerminalEnhancement()
      .catch((err) => {
        if (!cancelled) setError(String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [open, refreshTerminalEnhancement, tab]);

  const beginGithubCliLogin = useCallback(async () => {
    const status = await ghAuthStatus();
    setGhAuth(status);
    if (status.cliMissing) {
      window.open(GITHUB_CLI_INSTALL_URL, '_blank', 'noopener,noreferrer');
      return;
    }
    window.dispatchEvent(new CustomEvent('kota:smart-run', {
      detail: { command: GITHUB_CLI_LOGIN_COMMAND },
    }));
  }, []);

  const heroes = useMemo(() => [...HERO_TEMPLATES, ...customHeroes], [customHeroes]);
  const selectedHero = heroes.find((hero) => hero.id === selectedHeroId) ?? heroes[0];
  const shellById = useMemo(() => new Map(shells.map((shell) => [shell.id, shell])), [shells]);
  const activeHeroes = heroes.filter((hero) => !normalizeDraft(hero, profiles[hero.id]).dismissed);
  const visibleHeroes = activeHeroes.filter((hero) => !normalizeDraft(hero, profiles[hero.id]).archived);
  const archivedHeroes = activeHeroes.filter((hero) => normalizeDraft(hero, profiles[hero.id]).archived);
  const duplicateTavernHeroName = useCallback((name: string, excludeHeroId?: string): string | null => {
    const key = tavernHeroNameKey(name);
    if (!key) return null;
    for (const hero of heroes) {
      if (hero.id === excludeHeroId) continue;
      const draft = normalizeDraft(hero, profiles[hero.id]);
      if (draft.dismissed) continue;
      if (tavernHeroNameKey(draft.name) === key) return draft.name;
    }
    return null;
  }, [heroes, profiles]);
  const uniqueTavernHeroName = useCallback((base: string, excludeHeroId?: string): string => {
    const trimmed = base.trim() || 'New Hero';
    if (!duplicateTavernHeroName(trimmed, excludeHeroId)) return trimmed;
    for (let index = 2; index < 100; index += 1) {
      const suffix = TAVERN_ROMAN_SUFFIXES[index] ?? String(index);
      const candidate = `${trimmed} ${suffix}`;
      if (!duplicateTavernHeroName(candidate, excludeHeroId)) return candidate;
    }
    return `${trimmed} ${Date.now()}`;
  }, [duplicateTavernHeroName]);
  const updateHeroName = useCallback((
    hero: AgentCardSpec,
    name: string,
    nameFields: ProjectAgentNameFields,
  ): boolean => {
    const next = name.trim();
    const duplicate = duplicateTavernHeroName(next, hero.id);
    if (duplicate) {
      setError(`Hero name already exists in Tavern: ${duplicate}`);
      return false;
    }
    setError(null);
    updateHero(hero, { name: next, nameFields });
    return true;
  }, [duplicateTavernHeroName]);
  const refreshModelsForProvider = useCallback(async (providerId: ProviderId) => {
    setModelRefreshBusy(providerId);
    setError(null);
    try {
      const modelOptions = await refreshProviderModelOptions(providerId);
      setShells((prev) => prev.map((shell) => (
        shell.id === providerId ? { ...shell, modelOptions } : shell
      )));
    } catch (err) {
      setError(String(err));
    } finally {
      setModelRefreshBusy((current) => (current === providerId ? null : current));
    }
  }, []);

  const saveAccountUser = useCallback(async (identity: AccountUserIdentity) => {
    setAccountUserSaving(true);
    setError(null);
    try {
      const saved = await saveAccountUserIdentity({
        name: identity.name.trim() || DEFAULT_ACCOUNT_USER_IDENTITY.name,
        avatarId: identity.avatarId || DEFAULT_ACCOUNT_USER_IDENTITY.avatarId,
      });
      setAccountUser(saved);
      setProfileTarget(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setAccountUserSaving(false);
    }
  }, []);

  if (!open) return null;

  const run = async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  };

  const changeTerminalEnhancement = async (enabled: boolean) => {
    const status = await saveTerminalEnhancement(enabled);
    onGhosttyTerminalEnhancementChange?.(status.ghosttyTerminalEnhancementEnabled);
  };

  const updateHero = (hero: AgentCardSpec, patch: Partial<HeroDraft>) => {
    setProfiles((prev) => {
      const current = normalizeDraft(hero, prev[hero.id]);
      const next = { ...current, ...patch };
      next.model = normalizeModelIdForProvider(next.provider, next.model);
      if (
        patch.provider != null ||
        patch.model != null ||
        patch.effort != null ||
        patch.skills != null
      ) {
        next.shell = buildShellYaml(next);
      }
      return { ...prev, [hero.id]: next };
    });
  };

  const selectHero = (id: string) => {
    setSelectedHeroId(id);
    setGhostExpanded(false);
    setProfileTarget({ type: 'agent', id });
  };

  const selectSystemHero = (id: SystemHeroId) => {
    setGhostExpanded(false);
    setProfileTarget({ type: 'system', id });
    void loadSystemPrompts();
  };

  const selectProvider = (hero: AgentCardSpec, nextProviderId: ProviderId) => {
    const nextProvider = PROVIDERS[nextProviderId];
    updateHero(hero, {
      provider: nextProviderId,
      model: nextProvider.defaultModel,
      effort: nextProvider.defaultEffort,
      avatarId: nextProvider.defaultAvatarId,
    });
  };

  const addHero = () => {
    const id = `hero-${Date.now()}`;
    const provider = PROVIDER_IDS.find((providerId) => shellById.get(providerId)?.installed) ?? 'codex';
    const providerSpec = PROVIDERS[provider];
    const name = uniqueTavernHeroName('New Hero');
    const hero: AgentCardSpec = {
      id,
      kind: 'custom',
      provider,
      name,
    };
    setCustomHeroes((prev) => [...prev, hero]);
    setProfiles((prev) => ({
      ...prev,
      [id]: normalizeDraft(hero, {
        provider,
        model: providerSpec.defaultModel,
        effort: providerSpec.defaultEffort,
        avatarId: providerSpec.defaultAvatarId,
      }),
    }));
    setSelectedHeroId(id);
    setGhostExpanded(false);
    setProfileTarget({ type: 'agent', id });
  };

  const removeHero = (hero: AgentCardSpec) => {
    updateHero(hero, { archived: true });
    setProfileTarget(null);
  };

  const callBackHero = (hero: AgentCardSpec) => {
    setSelectedHeroId(hero.id);
    setProfiles((prev) => ({
      ...prev,
      [hero.id]: { ...normalizeDraft(hero, prev[hero.id]), archived: false, dismissed: false },
    }));
    setGhostExpanded(false);
    setProfileTarget({ type: 'agent', id: hero.id });
  };

  const dismissHero = async (hero: AgentCardSpec) => {
    setBusy('delete hero');
    setError(null);
    try {
      await deleteTavernHero({ heroId: hero.id });
      setCustomHeroes((prev) => prev.filter((item) => item.id !== hero.id));
      setProfiles((prev) => {
        const next = { ...prev };
        delete next[hero.id];
        return next;
      });
      if (selectedHeroId === hero.id) setSelectedHeroId(HERO_TEMPLATES[0].id);
      if (profileTarget?.type === 'agent' && profileTarget.id === hero.id) setProfileTarget(null);
      window.dispatchEvent(new Event(TAVERN_PROFILE_CHANGED_EVENT));
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(null);
    }
  };

  const resumeArchivedWorkspace = async (workspace: WorkspaceProject) => {
    await run('resume project', async () => {
      const next = await resumeWorkspaceProject(workspace.projectId);
      await refreshArchivedWorkspaces();
      onWorkspaceResumed?.(next);
      onClose();
    });
  };

  const removeArchivedWorkspace = async (workspace: WorkspaceProject) => {
    await run('remove project', async () => {
      let status = { dirty: false, dirtySummary: '' };
      try {
        status = await inspectWorkspaceProject(workspace.projectId);
      } catch (err) {
        status = {
          dirty: true,
          dirtySummary: `Could not verify Git sync status: ${String(err)}`,
        };
      }
      const confirmed = window.confirm(
        status.dirty
          ? `Remove ${workspace.repoFullName} from Kota?\n\nThis deletes Kota account state for the project but leaves local project files on disk.\n\nUnsynced or dirty Git state was detected:\n\n${status.dirtySummary}`
          : `Remove ${workspace.repoFullName} from Kota?\n\nThis deletes Kota account state for the project but leaves local project files on disk.`,
      );
      if (!confirmed) return;
      const result = await removeWorkspaceProject({
        projectId: workspace.projectId,
        forceDirty: status.dirty,
      });
      if (!result.ok) {
        throw new Error(result.dirtySummary || 'Project remove did not complete.');
      }
      await refreshArchivedWorkspaces();
    });
  };

  const commitHeroSkills = (hero: AgentCardSpec, skills: string[]) => {
    const draft = normalizeDraft(hero, profiles[hero.id]);
    if (draft.skills.join('\n') === skills.join('\n')) return;
    updateHero(hero, { skills });
  };

  const revealRawFile = async (hero: AgentCardSpec, fileName: RawHeroFile) => {
    const draft = normalizeDraft(hero, profiles[hero.id]);
    await run(`open ${fileName}`, async () => {
      await revealTavernHeroFile({
        heroId: hero.id,
        fileName,
        content: fileName === 'GHOST.md' ? draft.ghost : buildShellYaml(draft),
      });
    });
  };

  const updateSystemHero = (id: SystemHeroId, patch: SystemHeroDraft) => {
    setSystemDrafts((prev) => ({
      ...prev,
      [id]: { ...defaultSystemDraft(id, systemPromptTemplates), ...prev[id], ...patch },
    }));
  };

  const requestResetSystemPromptFiles = (hero: SystemHeroSpec) => {
    if (systemPromptResetTargets(hero.id).length === 0) return;
    setPromptResetTarget(hero);
  };

  const confirmResetSystemPromptFiles = async () => {
    const hero = promptResetTarget;
    if (!hero) return;
    const targets = systemPromptResetTargets(hero.id);
    if (targets.length === 0) {
      setPromptResetTarget(null);
      return;
    }
    setPromptResetBusy(true);
    setError(null);
    try {
      const results = await Promise.all(targets.map(async (target) => {
        const result = await resetSystemPrompt({ path: target.path }, target.fallback);
        return { key: target.key, content: result.content };
      }));
      setSystemPromptTemplates((prev) => {
        const next = { ...prev };
        for (const result of results) {
          next[result.key] = result.content;
        }
        return next;
      });
      if (hero.id === 'bbs') {
        setSystemDrafts((prev) => {
          const current = prev.bbs;
          if (!current) return prev;
          const { bbsPostPrompt: _bbsPostPrompt, bbsReplyPrompt: _bbsReplyPrompt, ...rest } = current;
          return { ...prev, bbs: rest };
        });
      }
      setPromptResetTarget(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setPromptResetBusy(false);
    }
  };

  const updateRuleDraft = (patch: Partial<AccountRuleDraft>) => {
    setRuleDraft((prev) => {
      if (!prev) return prev;
      const next = { ...prev, ...patch };
      if (patch.loadPolicy === 'always') next.taskTrigger = '';
      return next;
    });
  };

  const createRuleDraft = async () => {
    const before = new Set(accountRules.map((rule) => rule.fileName));
    setRulesBusy(true);
    setError(null);
    try {
      const rules = await saveAccountRule({
        fileName: null,
        title: 'New Account Rule',
        loadPolicy: 'on-demand',
        taskTrigger: '',
        body: '- Describe the rule here.',
      });
      setAccountRules(rules);
      const selected = rules.find((rule) => !before.has(rule.fileName)) ?? rules.at(-1) ?? rules[0] ?? null;
      setSelectedRuleFile(selected?.fileName ?? null);
      if (selected) setRuleDraft({ ...selected });
      setRuleAutoStatus('saved');
    } catch (err) {
      setError(String(err));
    } finally {
      setRulesBusy(false);
    }
  };

  const requestDeleteSelectedRule = () => {
    if (!ruleDraft?.fileName) return;
    if (ruleDraft.bundledDefault) return;
    setDeleteRuleTarget({ ...ruleDraft });
  };

  const confirmDeleteRule = async () => {
    const target = deleteRuleTarget;
    if (!target?.fileName || target.bundledDefault) return;
    if (saveRuleTimerRef.current != null) {
      window.clearTimeout(saveRuleTimerRef.current);
      saveRuleTimerRef.current = null;
    }
    saveRuleSeqRef.current += 1;
    setRulesBusy(true);
    setRuleAutoStatus('idle');
    setError(null);
    try {
      const rules = await deleteAccountRule(target.fileName);
      setAccountRules(rules);
      const nextSelected = rules[0]?.fileName ?? null;
      setSelectedRuleFile((prev) => (prev === target.fileName ? nextSelected : prev ?? nextSelected));
      const nextDraft = rules.find((rule) => rule.fileName === nextSelected) ?? null;
      setRuleDraft(nextDraft ? { ...nextDraft } : null);
      setDeleteRuleTarget(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setRulesBusy(false);
    }
  };

  const resetRulesToFactory = async () => {
    if (saveRuleTimerRef.current != null) {
      window.clearTimeout(saveRuleTimerRef.current);
      saveRuleTimerRef.current = null;
    }
    saveRuleSeqRef.current += 1;
    setRulesBusy(true);
    setRuleAutoStatus('idle');
    setError(null);
    try {
      const rules = await resetDefaultAccountRules();
      setAccountRules(rules);
      setSelectedRuleFile((prev) => prev ?? rules[0]?.fileName ?? null);
    } catch (err) {
      setError(String(err));
    } finally {
      setRulesBusy(false);
    }
  };

  const requestDeleteSkill = (skill: AccountSkillDraft) => {
    if (skill.bundledDefault) return;
    setDeleteSkillTarget(skill);
  };

  const openSkillPool = async () => {
    await run('open skill pool', async () => {
      await openAccountSkillsFolder();
    });
  };

  const openSkillFolder = async (skill: AccountSkillDraft) => {
    await run(`open ${skill.id}`, async () => {
      await openAccountSkillFolder(skill.id);
    });
  };

  const importSkillFromPicker = async () => {
    setSkillsBusy(true);
    setError(null);
    setSkillImportDialog(null);
    try {
      const result = await importAccountSkillFromPicker();
      if (!result) return;
      setAccountSkills(result.skills);
      setSkillImportDialog({
        tone: 'success',
        title: 'Skill Imported',
        body: result.message,
      });
    } catch (err) {
      setSkillImportDialog({
        tone: 'error',
        title: 'Skill Import Failed',
        body: skillImportErrorText(err),
      });
    } finally {
      setSkillsBusy(false);
    }
  };

  const confirmDeleteSkill = async () => {
    const target = deleteSkillTarget;
    if (!target || target.bundledDefault) return;
    setSkillsBusy(true);
    setError(null);
    try {
      setAccountSkills(await deleteAccountSkill(target.id));
      setDeleteSkillTarget(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setSkillsBusy(false);
    }
  };

  const githubCliState = ghAuth == null ? 'checking' : ghAuth.authenticated ? 'ok' : ghAuth.cliMissing ? 'missing' : 'setup';
  const githubCliPrimary = ghAuth?.authenticated
    ? (
      <span className="github-cli-account">
        <span aria-hidden>{githubInitial(ghAuth.username)}</span>
        <b>{ghAuth.username ?? 'GitHub CLI ready'}</b>
      </span>
    )
    : ghAuth == null
      ? 'Checking GitHub CLI'
      : ghAuth.cliMissing
      ? 'GitHub CLI missing'
      : 'GitHub CLI not logged in';
  const githubCliAction = ghAuth?.cliMissing ? 'Install GitHub CLI' : 'Login GitHub';
  const accountUserName = accountUser.name.trim() || DEFAULT_ACCOUNT_USER_IDENTITY.name;

  return (
    <main className="tavern-page" data-testid="tavern-page" aria-label="Tavern">
      <header className="tavern-page-head">
        <button ref={backButtonRef} type="button" className="tavern-back" onClick={onClose} aria-label="Back to room">
          Back
        </button>
        <div className="tavern-heading">
          <div className="tavern-title">Tavern</div>
          <div className="tavern-subtitle">Heroes, shared pools, and account links</div>
        </div>
      </header>

      <div className="tavern-tabs" role="tablist">
        {TAVERN_TABS.map((nextTab) => (
          <button
            key={nextTab}
            type="button"
            className={tab === nextTab ? 'active' : ''}
            onClick={() => {
              setTab(nextTab);
              setProfileTarget(null);
            }}
          >
            {nextTab === 'heroes' ? 'Hero' : titleCase(nextTab)}
          </button>
        ))}
      </div>

      {tab === 'heroes' && (
        <div className="tavern-pane heroes">
          <section className="tavern-hero-stage" aria-label="Hero roster">
            <div className="tavern-hero-header">
              <button
                type="button"
                className="tavern-user-corner user-entry"
                onClick={() => setProfileTarget({ type: 'user' })}
                aria-label={`Edit human identity: ${accountUserName}`}
                title={`${accountUserName} · Human`}
              >
                <span className="ue-avatar">
                  <HeroAvatarArt avatarId={accountUser.avatarId} provider="codex" />
                </span>
                <span className="ue-text">
                  <span className="ue-label">Human</span>
                  <span className="ue-name">{accountUserName}</span>
                </span>
              </button>
              <div className="tavern-hero-title">Meet your Heros</div>
              <span className="tavern-user-corner-spacer" aria-hidden="true" />
            </div>

            <section className={`tavern-system-heroes ${systemOpen ? 'open' : ''}`}>
              <button
                type="button"
                className="tavern-system-toggle"
                onClick={() => {
                  setSystemOpen((value) => !value);
                  void loadSystemPrompts();
                }}
                aria-expanded={systemOpen}
              >
                <span>System Heros</span>
                <small>{SYSTEM_HEROES.length} resident spirits</small>
              </button>
              {systemOpen && (
                <div className="tavern-system-grid">
                  {SYSTEM_HEROES.map((hero) => (
                    <SystemHeroCard key={hero.id} hero={hero} onSelect={() => selectSystemHero(hero.id)} />
                  ))}
                </div>
              )}
            </section>

            <div className="tavern-hero-gathering">
              {visibleHeroes.map((hero) => {
                const draft = normalizeDraft(hero, profiles[hero.id]);
                const heroProvider = PROVIDERS[draft.provider];
                return (
                  <button
                    key={hero.id}
                    type="button"
                    className={`tavern-hero-card ${selectedHero.id === hero.id ? 'active' : ''}`}
                    onClick={() => selectHero(hero.id)}
                    title={draft.name}
                    data-full-name={draft.name}
                    data-testid={`tavern-hero-${hero.id}`}
                  >
                    <HeroAvatarArt avatarId={draft.avatarId} provider={draft.provider} />
                    <span className="tavern-hero-card-copy">
                      <b>{draft.name}</b>
                      <span>{heroProvider.name}</span>
                    </span>
                    <HeroCardMerits record={draft.record} />
                  </button>
                );
              })}
              <button type="button" className="tavern-hero-card add" onClick={addHero}>
                <span className="tavern-avatar-add" aria-hidden />
                <span className="tavern-hero-card-copy">
                  <b>New Hero</b>
                  <span>create template</span>
                </span>
              </button>
            </div>

            <SupportedProviders
              shells={shells}
              shellsLoading={!!loadingTasks.shells}
            />
          </section>

          {archivedHeroes.length > 0 && (
            <section className="tavern-drifters">
              <div className="tavern-section-title">Drifters</div>
              <div className="tavern-drifter-list">
                {archivedHeroes.map((hero) => {
                  const draft = normalizeDraft(hero, profiles[hero.id]);
                  return (
                    <div key={hero.id} className="tavern-drifter">
                      <HeroAvatarArt avatarId={draft.avatarId} provider={draft.provider} />
                      <span>{draft.name}</span>
                      <span className="tavern-drifter-actions">
                        <button type="button" onClick={() => callBackHero(hero)}>
                          Call Back
                        </button>
                        <button type="button" className="danger" onClick={() => { void dismissHero(hero); }}>
                          Dismiss
                        </button>
                      </span>
                    </div>
                  );
                })}
              </div>
            </section>
          )}
        </div>
      )}

      {tab === 'rules' && (
        <div className="tavern-pane rules">
          <section className="tavern-rule-list" aria-label="Account rules">
            <div className="tavern-rule-list-head">
              <div>
                <div className="tavern-section-title">Account Rules</div>
                <div className="tavern-identity">~/Kota/rules</div>
              </div>
              <div className="tavern-rule-list-actions">
                <button type="button" className="tavern-raw-button" disabled={rulesBusy} onClick={() => void createRuleDraft()}>
                  New
                </button>
                <button type="button" className="tavern-raw-button" disabled={rulesBusy} onClick={() => void resetRulesToFactory()}>
                  Reset to Factory
                </button>
              </div>
            </div>
            {rulesLoading && accountRules.length === 0 ? (
              <TavernLoadingBlock label="Rules" />
            ) : accountRules.map((rule) => (
              <button
                key={rule.fileName}
                type="button"
                className={[
                  'tavern-rule-row',
                  selectedRuleFile === rule.fileName ? 'active' : '',
                ].filter(Boolean).join(' ')}
                onClick={() => setSelectedRuleFile(rule.fileName)}
              >
                <span>
                  <b>{rule.title}</b>
                  <small>{rule.fileName}</small>
                </span>
                <span className="tavern-rule-badges">
                  {rule.bundledDefault && <i>Kota default</i>}
                  <i>{rule.loadPolicy}</i>
                </span>
              </button>
            ))}
          </section>

          <section className="tavern-rule-editor">
            {ruleDraft ? (
              <>
                <div className="tavern-rule-editor-scroll" data-testid="account-rule-editor-scroll">
                  <label className="tavern-profile-field">
                    Title
                    <input
                      value={ruleDraft.title}
                      onChange={(e) => updateRuleDraft({ title: e.currentTarget.value })}
                    />
                  </label>
                  <label className="tavern-profile-field">
                    Load policy
                    <select
                      value={ruleDraft.loadPolicy}
                      onChange={(e) => updateRuleDraft({ loadPolicy: e.currentTarget.value })}
                    >
                      <option value="always">always</option>
                      <option value="on-demand">on-demand</option>
                    </select>
                  </label>
                  {ruleDraft.loadPolicy === 'on-demand' && (
                    <label className="tavern-profile-field">
                      On-demand trigger
                      <textarea
                        className="tavern-rule-trigger"
                        rows={2}
                        wrap="soft"
                        value={ruleDraft.taskTrigger}
                        onChange={(e) => updateRuleDraft({ taskTrigger: e.currentTarget.value })}
                        placeholder="coding, debugging, refactoring..."
                      />
                    </label>
                  )}
                  <label className="tavern-profile-field tavern-rule-body">
                    Body
                    <textarea
                      value={ruleDraft.body}
                      spellCheck={false}
                      onChange={(e) => updateRuleDraft({ body: e.currentTarget.value })}
                    />
                  </label>
                </div>
                <div className="tavern-rule-editor-footer" data-testid="account-rule-editor-footer">
                  <div className="tavern-rule-meta">
                    <span title={ruleDraft.path}>{ruleDraft.path || 'New rule file'}</span>
                    {ruleDraft.bundledDefault && <b>{ruleDraft.modified ? 'Modified Kota default' : 'Kota default'}</b>}
                    <b>{ruleAutoStatus === 'saving' ? 'Saving' : ruleAutoStatus === 'editing' ? 'Unsaved changes' : ruleAutoStatus === 'saved' ? 'Saved' : 'Auto-save'}</b>
                  </div>
                  {!ruleDraft.bundledDefault && (
                    <div className="tavern-rule-actions">
                      <button type="button" className="tavern-raw-button danger" disabled={rulesBusy || !ruleDraft.fileName} onClick={requestDeleteSelectedRule}>
                        Delete
                      </button>
                    </div>
                  )}
                </div>
              </>
            ) : (
              <div className="tavern-rule-empty">No account rules found.</div>
            )}
          </section>
        </div>
      )}

      {tab === 'skills' && (
        <div className="tavern-pane simple">
          <section className="tavern-section full tavern-skill-pool-header">
            <div className="tavern-card-head">
              <div>
                <div className="tavern-section-title">Skill Pool</div>
                <div className="tavern-identity">$KOTA_HOME/skills</div>
              </div>
              <div className="tavern-rule-list-actions">
                <button
                  type="button"
                  className="tavern-raw-button"
                  disabled={skillsBusy}
                  onClick={() => void importSkillFromPicker()}
                >
                  Upload Skill
                </button>
                <button type="button" className="tavern-raw-button" disabled={skillsBusy} onClick={() => void openSkillPool()}>
                  Open
                </button>
              </div>
            </div>
          </section>
          {skillsLoading && accountSkills.length === 0 ? (
            <TavernLoadingBlock label="Skills" />
          ) : accountSkills.map((skill) => (
            <section key={skill.id} className="tavern-list-card tavern-skill-pool-card">
              <div className="tavern-skill-pool-copy">
                <div className="tavern-skill-pool-title">{skill.name}</div>
                <SkillDescription
                  className="tavern-skill-pool-description"
                  text={skill.description || skill.error || ''}
                  fallback={skill.path}
                />
                <div className="tavern-skill-path-row">
                  <code title={skill.path}>{skill.path}</code>
                  <button
                    type="button"
                    className="tavern-raw-button"
                    disabled={skillsBusy || !skill.path}
                    onClick={() => void openSkillFolder(skill)}
                  >
                    Open
                  </button>
                </div>
              </div>
              <span
                className={`tavern-skill-kind-label ${
                  skill.bundledDefault ? 'default' : skill.valid ? 'manual' : 'invalid'
                }`}
              >
                {skill.bundledDefault ? 'default' : skill.valid ? skill.kind : 'invalid'}
              </span>
              {!skill.bundledDefault && (
                <button
                  type="button"
                  className="tavern-raw-button danger"
                  disabled={skillsBusy}
                  onClick={() => requestDeleteSkill(skill)}
                >
                  Delete
                </button>
              )}
            </section>
          ))}
          {!skillsLoading && accountSkills.length === 0 && (
            <div className="tavern-rule-empty">No valid skills found in $KOTA_HOME/skills.</div>
          )}
        </div>
      )}

      {tab === 'link' && (
        <div className="tavern-pane connections">
          {SHOW_GOOGLE_DRIVE_CARD && (
            <ConnectionCard
              icon="google-drive"
              title="Google Drive"
              connected={false}
              showStatusDot={false}
              primary="Coming soon"
              description="Sync and continue anywhere"
              details={[]}
              disabled={busy != null}
            />
          )}

          <ConnectionCard
            icon="github"
            title="GitHub CLI"
            connected={!!ghAuth?.authenticated}
            fullWidth={!SHOW_GOOGLE_DRIVE_CARD}
            status={githubCliState}
            primary={githubCliPrimary}
            description={
              ghAuth?.authenticated
                ? undefined
                : 'Connect your projects'
            }
            details={[]}
            actionLabel={ghAuth?.authenticated || ghAuth == null ? undefined : githubCliAction}
            actionTone={ghAuth?.cliMissing ? 'danger' : undefined}
            disabled={busy != null}
            onAction={ghAuth?.authenticated || ghAuth == null ? undefined : () => run('github cli', beginGithubCliLogin)}
          />

          <section className="tavern-section full tavern-terminal-enhancement">
            <div className="tavern-card-head">
              <div>
                <div className="tavern-section-title">Terminal Rendering</div>
              </div>
              <span className={`tavern-dot ${ghosttyTerminalEnhancement ? 'ok' : ''}`} />
            </div>
            <div
              className="terminal-enhancement-toggle"
              role="group"
              aria-label="Ghostty terminal enhancement"
            >
              <button
                type="button"
                className={!ghosttyTerminalEnhancement ? 'active' : ''}
                disabled={busy != null}
                onClick={() => run('terminal rendering', () => changeTerminalEnhancement(false))}
              >
                Native
              </button>
              <button
                type="button"
                className={ghosttyTerminalEnhancement ? 'active' : ''}
                disabled={busy != null}
                onClick={() => run('terminal rendering', () => changeTerminalEnhancement(true))}
              >
                Ghostty Enhanced
              </button>
            </div>
          </section>

          <section
            className="tavern-section full tavern-local-storage"
            aria-label="Local Storage"
            data-testid="tavern-local-storage"
          >
            <div className="tavern-card-head">
              <div>
                <div className="tavern-section-title">Local Storage</div>
              </div>
            </div>
            <div className="tavern-readouts tavern-local-readouts">
              {config?.appPath && <Readout label="Running app" value={config.appPath} />}
              {config?.localAccountFolder && (
                <Readout
                  label="Account folder"
                  value={config.localAccountFolder}
                  detail={(
                    <StorageMeasurementDetail
                      status={storageMeasurement}
                      now={storageAgeNow}
                      onRefresh={() => void beginStorageMeasurement()}
                    />
                  )}
                />
              )}
            </div>
          </section>
        </div>
      )}

      {tab === 'archived' && (
        <div className="tavern-pane simple archived-projects">
          {archivedLoading && archivedWorkspaces.length === 0 ? (
            <TavernLoadingBlock label="Archives" />
          ) : archivedWorkspaces.length === 0 ? (
            <section className="tavern-list-card">
              <div>
                <div className="tavern-section-title">Archived Projects</div>
                <div className="tavern-identity">No archived projects.</div>
              </div>
            </section>
          ) : archivedWorkspaces.map((workspace) => (
            <section key={workspace.projectId} className="tavern-list-card archived-project-card">
              <div>
                <div className="tavern-section-title">{workspace.repoFullName}</div>
                <div className="tavern-identity">{workspace.sourceDir}</div>
              </div>
              <span className="archived-project-actions">
                <button
                  type="button"
                  disabled={busy != null}
                  onClick={() => { void resumeArchivedWorkspace(workspace); }}
                >
                  Resume
                </button>
                <button
                  type="button"
                  className="danger"
                  disabled={busy != null}
                  onClick={() => { void removeArchivedWorkspace(workspace); }}
                >
                  Remove
                </button>
              </span>
            </section>
          ))}
        </div>
      )}

      {profileTarget?.type === 'agent' && (
        <AgentProfileOverlay
          hero={heroes.find((hero) => hero.id === profileTarget.id) ?? selectedHero}
          profiles={profiles}
          shellById={shellById}
          accountSkills={accountSkills}
          skillsLoading={skillsLoading}
          modelRefreshBusy={modelRefreshBusy}
          ghostExpanded={ghostExpanded}
          onGhostExpanded={setGhostExpanded}
          onBack={() => setProfileTarget(null)}
          onUpdate={updateHero}
          onUpdateName={updateHeroName}
          duplicateNameFor={(name, heroId) => duplicateTavernHeroName(name, heroId)}
          onSelectProvider={selectProvider}
          onRefreshModels={refreshModelsForProvider}
          onCommitSkills={commitHeroSkills}
          onRevealRawFile={revealRawFile}
          onOpenSkillFolder={openSkillFolder}
          onRemove={removeHero}
        />
      )}

      {profileTarget?.type === 'system' && (
        <SystemProfileOverlay
          hero={SYSTEM_HEROES.find((hero) => hero.id === profileTarget.id) ?? SYSTEM_HEROES[0]}
          draft={{
            ...defaultSystemDraft(profileTarget.id, systemPromptTemplates),
            ...systemDrafts[profileTarget.id],
          }}
          templates={systemPromptTemplates}
          promptsReady={systemPromptsLoaded}
          promptsLoading={!!loadingTasks.prompts}
          onBack={() => setProfileTarget(null)}
          onUpdate={(patch) => updateSystemHero(profileTarget.id, patch)}
          promptResetBusy={promptResetBusy && promptResetTarget?.id === profileTarget.id}
          onResetPromptFiles={() => requestResetSystemPromptFiles(
            SYSTEM_HEROES.find((hero) => hero.id === profileTarget.id) ?? SYSTEM_HEROES[0],
          )}
        />
      )}

      {profileTarget?.type === 'user' && (
        <AccountUserProfileOverlay
          identity={accountUser}
          saving={accountUserSaving}
          onBack={() => setProfileTarget(null)}
          onSave={saveAccountUser}
        />
      )}

      {skillImportDialog && (
        <div className="kota-confirm-layer" role="dialog" aria-modal="true" aria-label={skillImportDialog.title}>
          <div className={`kota-confirm-card ${skillImportDialog.tone === 'error' ? 'danger' : 'success'}`}>
            <h2>{skillImportDialog.title}</h2>
            <pre>{skillImportDialog.body}</pre>
            <div className="kota-confirm-actions">
              <button type="button" className="confirm" onClick={() => setSkillImportDialog(null)}>
                OK
              </button>
            </div>
          </div>
        </div>
      )}

      {promptResetTarget && (
        <div className="kota-confirm-layer" role="dialog" aria-modal="true" aria-label="Reset prompt files">
          <div className="kota-confirm-card danger">
            <h2>Reset Prompt Files</h2>
            <pre>{`Reset ${promptResetTarget.name} prompt files to factory defaults?\n\n${systemPromptResetTargets(promptResetTarget.id).map((target) => target.path).join('\n')}\n\nThis overwrites local edits to these prompt files only.`}</pre>
            <div className="kota-confirm-actions">
              <button type="button" onClick={() => setPromptResetTarget(null)}>
                Cancel
              </button>
              <button type="button" className="confirm" disabled={promptResetBusy} onClick={() => void confirmResetSystemPromptFiles()}>
                Reset prompt files
              </button>
            </div>
          </div>
        </div>
      )}

      {deleteRuleTarget && (
        <div className="kota-confirm-layer" role="dialog" aria-modal="true" aria-label="Delete account rule">
          <div className="kota-confirm-card danger">
            <h2>Delete Rule</h2>
            <pre>{`Delete "${deleteRuleTarget.title}"?\n\n${deleteRuleTarget.fileName}\n\nThis removes the account rule file from ~/Kota/rules.`}</pre>
            <div className="kota-confirm-actions">
              <button type="button" onClick={() => setDeleteRuleTarget(null)}>
                Cancel
              </button>
              <button type="button" className="confirm" disabled={rulesBusy} onClick={() => void confirmDeleteRule()}>
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

      {deleteSkillTarget && (
        <div className="kota-confirm-layer" role="dialog" aria-modal="true" aria-label="Delete account skill">
          <div className="kota-confirm-card danger">
            <h2>Delete Skill</h2>
            <pre>{`Delete "${deleteSkillTarget.name}"?\n\n${deleteSkillTarget.path}\n\nThis removes only the Kota Skill Pool copy under $KOTA_HOME/skills.`}</pre>
            <div className="kota-confirm-actions">
              <button type="button" onClick={() => setDeleteSkillTarget(null)}>
                Cancel
              </button>
              <button type="button" className="confirm" disabled={skillsBusy} onClick={() => void confirmDeleteSkill()}>
                Delete
              </button>
            </div>
          </div>
        </div>
      )}

      {(busy || error) && (
        <div className={`tavern-status ${error ? 'error' : ''}`}>
          {error ?? `Working: ${busy}`}
        </div>
      )}
    </main>
  );
}

function AccountUserProfileOverlay({
  identity,
  saving,
  onBack,
  onSave,
}: {
  identity: AccountUserIdentity;
  saving: boolean;
  onBack: () => void;
  onSave: (identity: AccountUserIdentity) => Promise<void>;
}) {
  const [draft, setDraft] = useState<AccountUserIdentity>(() => normalizeAccountUserIdentity(identity));
  const [dreamsAvailable, setDreamsAvailable] = useState(false);
  useEffect(() => {
    setDraft(normalizeAccountUserIdentity(identity));
  }, [identity.avatarId, identity.name]);
  useEffect(() => {
    let cancelled = false;
    void accountDreamsStatus()
      .then((status) => {
        if (!cancelled) setDreamsAvailable(status.exists);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, []);
  const name = draft.name;
  const trimmedName = name.trim();
  const canSave = trimmedName.length > 0 && !saving;

  return (
    <section className="tavern-profile-overlay" role="dialog" aria-label="Human">
      <div className="tavern-profile-card user">
        <button type="button" className="tavern-profile-back" onClick={onBack}>
          Back
        </button>
        <div className="tavern-user-profile">
          <HeroAvatarPicker
            provider="codex"
            value={draft.avatarId}
            disabled={saving}
            className="profile"
            onChange={(avatarId) => setDraft((prev) => ({ ...prev, avatarId }))}
          />
          <div className="tavern-user-profile-copy">
            <h2>Human</h2>
            <label className="tavern-profile-field">
              Name
              <input
                value={name}
                disabled={saving}
                onChange={(event) => {
                  // currentTarget is nulled once dispatch ends, but updater
                  // fns run later at render time — read the value now or the
                  // first keystroke crashes the whole tree (black screen).
                  const value = event.currentTarget.value;
                  setDraft((prev) => ({ ...prev, name: value }));
                }}
                onKeyDown={(event) => {
                  if (event.key === 'Enter' && canSave) {
                    event.preventDefault();
                    void onSave({ ...draft, name: trimmedName });
                  }
                }}
                autoFocus
              />
            </label>
            {dreamsAvailable && (
              <button
                type="button"
                className="tavern-user-dreams"
                onClick={() => void accountDreamsOpen().catch(() => setDreamsAvailable(false))}
              >
                See agent dreams about you
              </button>
            )}
            <div className="tavern-user-profile-actions">
              <button type="button" onClick={onBack} disabled={saving}>
                Cancel
              </button>
              <button
                type="button"
                className="primary"
                disabled={!canSave}
                onClick={() => void onSave({ ...draft, name: trimmedName })}
              >
                {saving ? 'Saving' : 'Save'}
              </button>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

function AgentProfileOverlay({
  hero,
  profiles,
  shellById,
  accountSkills,
  skillsLoading,
  modelRefreshBusy,
  ghostExpanded,
  onGhostExpanded,
  onBack,
  onUpdate,
  onUpdateName,
  duplicateNameFor,
  onSelectProvider,
  onRefreshModels,
  onCommitSkills,
  onRevealRawFile,
  onOpenSkillFolder,
  onRemove,
}: {
  hero: AgentCardSpec;
  profiles: Record<string, Partial<HeroDraft>>;
  shellById: Map<string, SupportedShellStatus>;
  accountSkills: AccountSkillDraft[];
  skillsLoading: boolean;
  modelRefreshBusy: ProviderId | null;
  ghostExpanded: boolean;
  onGhostExpanded: (expanded: boolean) => void;
  onBack: () => void;
  onUpdate: (hero: AgentCardSpec, patch: Partial<HeroDraft>) => void;
  onUpdateName: (hero: AgentCardSpec, name: string, nameFields: ProjectAgentNameFields) => boolean;
  duplicateNameFor: (name: string, heroId: string) => string | null;
  onSelectProvider: (hero: AgentCardSpec, provider: ProviderId) => void;
  onRefreshModels: (provider: ProviderId) => Promise<void>;
  onCommitSkills: (hero: AgentCardSpec, skills: string[]) => void;
  onRevealRawFile: (hero: AgentCardSpec, fileName: RawHeroFile) => Promise<void>;
  onOpenSkillFolder: (skill: AccountSkillDraft) => Promise<void>;
  onRemove: (hero: AgentCardSpec) => void;
}) {
  const draft = normalizeDraft(hero, profiles[hero.id]);
  const provider = PROVIDERS[draft.provider];
  const shellStatus = shellById.get(provider.id);
  const effortOptions = shellStatus?.effortOptions ?? [];
  const selectedEffort = draft.effort ?? provider.defaultEffort ?? effortOptions[0]?.value ?? '';
  // SHELL edits are staged locally and only committed on Save Shell, so the
  // 250ms profile autosave never persists a half-typed model id.
  const [shellEditing, setShellEditing] = useState(false);
  const [providerMenuOpen, setProviderMenuOpen] = useState(false);
  const [editProvider, setEditProvider] = useState<ProviderId>(draft.provider);
  const [editModel, setEditModel] = useState(draft.model);
  const [editEffort, setEditEffort] = useState(selectedEffort);
  const providerControlRef = useRef<HTMLDivElement | null>(null);
  const shellProviderId = shellEditing ? editProvider : draft.provider;
  const editProviderSpec = PROVIDERS[editProvider];
  const editShellStatus = shellById.get(editProvider);
  const editModelSeed = editShellStatus?.modelOptions?.length
    ? editShellStatus.modelOptions
    : [{ id: editProviderSpec.defaultModel, label: editProviderSpec.defaultModel, source: 'default' }];
  const editEffortSeed = editShellStatus?.effortOptions ?? [];
  const modelComboOptions = uniqueShellComboOptions([
    ...editModelSeed.map((option) => ({
      id: option.id,
      label: option.label || option.id,
      source: option.source || 'seed',
    })),
    editModel.trim() ? { id: editModel.trim(), label: editModel.trim(), source: 'current' } : null,
  ]);
  const effortComboOptions = uniqueShellComboOptions([
    ...editEffortSeed.map((option) => ({
      id: option.value,
      label: option.label || option.value,
      source: 'provider',
    })),
    editEffort.trim() ? { id: editEffort.trim(), label: editEffort.trim(), source: 'current' } : null,
  ]);
  const editModelRefreshing = modelRefreshBusy === editProvider;
  const [skillDraftIds, setSkillDraftIds] = useState<string[]>(() => draft.skills);
  const skillDraftRef = useRef(skillDraftIds);
  const skillEntries = skillLoomEntries(accountSkills, skillDraftIds);
  useEffect(() => {
    setShellEditing(false);
    setProviderMenuOpen(false);
    setSkillDraftIds(draft.skills);
    skillDraftRef.current = draft.skills;
  }, [hero.id, provider.id]);
  useEffect(() => {
    if (!providerMenuOpen) return undefined;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Node && providerControlRef.current?.contains(target)) return;
      setProviderMenuOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    return () => document.removeEventListener('pointerdown', onPointerDown, true);
  }, [providerMenuOpen]);
  useEffect(() => {
    skillDraftRef.current = skillDraftIds;
  }, [skillDraftIds]);
  const commitSkillDraft = () => {
    const nextSkills = skillDraftRef.current;
    if (draft.skills.join('\n') !== nextSkills.join('\n')) {
      onCommitSkills(hero, nextSkills);
    }
  };
  const closeProfile = () => {
    commitSkillDraft();
    onBack();
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (event.target instanceof Element && event.target.closest('.project-agent-combo[data-open="true"]')) return;
      if (providerMenuOpen) {
        event.preventDefault();
        event.stopPropagation();
        setProviderMenuOpen(false);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      closeProfile();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  });
  const setSkillActive = (skillId: string, active: boolean) => {
    setSkillDraftIds((current) => {
      const selected = current.includes(skillId);
      if (selected === active) return current;
      return active ? [...current, skillId] : current.filter((id) => id !== skillId);
    });
  };
  const startShellEdit = () => {
    setEditProvider(draft.provider);
    setEditModel(draft.model);
    setEditEffort(selectedEffort);
    setProviderMenuOpen(false);
    setShellEditing(true);
  };
  const chooseEditProvider = (providerId: ProviderId) => {
    if (providerId !== editProvider) {
      const spec = PROVIDERS[providerId];
      setEditProvider(providerId);
      setEditModel(spec.defaultModel);
      setEditEffort(spec.defaultEffort ?? '');
    }
    setProviderMenuOpen(false);
  };
  const saveShell = () => {
    const spec = PROVIDERS[editProvider];
    const nextModel = editModel.trim() || spec.defaultModel;
    const nextEffort = editEffort.trim();
    if (editProvider !== draft.provider) {
      onSelectProvider(hero, editProvider);
    }
    const patch: Partial<HeroDraft> = { model: nextModel };
    if (editEffortSeed.length > 0 || nextEffort) {
      patch.effort = nextEffort || undefined;
    }
    onUpdate(hero, patch);
    setProviderMenuOpen(false);
    setShellEditing(false);
  };

  return (
    <section className="tavern-profile-overlay" role="dialog" aria-label={`${draft.name} profile`}>
      <div className="tavern-profile-card">
        <button type="button" className="tavern-profile-back" onClick={closeProfile}>
          Back
        </button>
        <HeroMeritBar record={draft.record} />

        <div className="tavern-profile-layout">
          <section className="tavern-profile-panel shell-panel" aria-label="SHELL">
            <PanelTitle icon={iconShell}>SHELL</PanelTitle>
            <div className="project-agent-shell-core">
              <div className="tavern-shell-provider-control" ref={providerControlRef}>
                <button
                  type="button"
                  className={`project-agent-shell-provider tavern-shell-provider-button ${tavernShellProviderClass(shellProviderId)}`}
                  aria-haspopup="listbox"
                  aria-expanded={providerMenuOpen}
                  aria-label={`Provider ${PROVIDERS[shellProviderId].name}`}
                  disabled={!shellEditing}
                  onClick={() => setProviderMenuOpen((open) => !open)}
                >
                  <span className="project-agent-shell-provider-icon" aria-hidden="true" />
                  <span className="project-agent-shell-provider-copy">
                    <span className="project-agent-shell-provider-label">Provider</span>
                    <span className="project-agent-shell-provider-name">{PROVIDERS[shellProviderId].name}</span>
                  </span>
                </button>
                {providerMenuOpen && shellEditing && (
                  <div className="tavern-shell-provider-menu" role="listbox" aria-label="Provider">
                    {PROVIDER_IDS.map((providerId) => {
                      const spec = PROVIDERS[providerId];
                      const status = shellById.get(providerId);
                      const shellStatusLoaded = shellById.size > 0;
                      const checking = !shellStatusLoaded;
                      const installed = !!status?.installed;
                      const unavailable = shellStatusLoaded && !installed;
                      return (
                        <button
                          key={providerId}
                          type="button"
                          role="option"
                          aria-selected={providerId === editProvider}
                          className={`tavern-shell-provider-option ${providerId === editProvider ? 'active' : ''}`}
                          disabled={unavailable}
                          title={checking ? `Checking ${spec.name} CLI` : unavailable ? `${spec.name} CLI is not available` : spec.name}
                          onClick={() => chooseEditProvider(providerId)}
                        >
                          <span
                            className={`project-agent-shell-provider-icon tavern-shell-provider-option-icon ${tavernShellProviderClass(providerId)}`}
                            aria-hidden="true"
                          />
                          <span className={`tavern-provider-dot ${checking ? 'checking' : installed ? 'ok' : ''}`} />
                          <strong>{spec.name}</strong>
                          {spec.beta && <span className="tavern-beta-badge" aria-label="Beta provider">BETA</span>}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
              {shellEditing ? (
                <div className="project-agent-shell-edit-fields">
                  <div className="tavern-profile-field">
                    <span>Model</span>
                    <ShellComboBox
                      value={editModel}
                      options={modelComboOptions}
                      placeholder="Exact model ID"
                      status={editModelRefreshing
                        ? 'Fetching model IDs...'
                        : `${editModelSeed.length} model IDs in catalog`}
                      refreshing={editModelRefreshing}
                      onChange={setEditModel}
                      onRefresh={() => void onRefreshModels(editProvider)}
                    />
                  </div>
                  {effortComboOptions.length > 0 && (
                    <div className="tavern-profile-field">
                      <span>Effort</span>
                      <ShellComboBox
                        value={editEffort}
                        options={effortComboOptions}
                        placeholder="default"
                        onChange={setEditEffort}
                      />
                    </div>
                  )}
                </div>
              ) : (
                <div className="project-agent-shell-summary" aria-label="Current SHELL settings">
                  <code title={draft.model}>{draft.model}</code>
                  <code title={selectedEffort || 'default'}>{selectedEffort || 'default'}</code>
                </div>
              )}
            </div>
            {shellEditing && (
              <div className="project-agent-shell-apply-note">Apply on Next Incarnation</div>
            )}
            <div className="tavern-profile-actions">
              <button type="button" onClick={shellEditing ? saveShell : startShellEdit}>
                {shellEditing ? 'Save Shell' : 'Edit Shell'}
              </button>
            </div>
          </section>

          <section className="tavern-profile-center" aria-label="Hero identity">
            <TavernHeroNameEditor
              name={draft.name}
              nameFields={draft.nameFields}
              duplicateNameFor={(name) => duplicateNameFor(name, hero.id)}
              renderAvatar={() => (
                <HeroAvatarPicker
                  provider={draft.provider}
                  value={draft.avatarId}
                  className="profile"
                  onChange={(avatarId) => onUpdate(hero, { avatarId })}
                />
              )}
              onChange={(name, nameFields) => onUpdateName(hero, name, nameFields)}
            />
            <section className={`tavern-ghost-scroll ${ghostExpanded ? 'expanded' : ''}`}>
              <PanelTitle icon={iconGhost}>GHOST</PanelTitle>
              {ghostExpanded ? (
                <textarea
                  value={draft.ghost}
                  onChange={(e) => onUpdate(hero, { ghost: e.currentTarget.value })}
                  spellCheck={false}
                  autoFocus
                />
              ) : (
                <button type="button" className="tavern-ghost-preview" onClick={() => onGhostExpanded(true)}>
                  {ghostPreview(draft.ghost)}
                </button>
              )}
              <div className="tavern-profile-actions">
                <button type="button" onClick={() => onGhostExpanded(!ghostExpanded)}>
                  {ghostExpanded ? 'Save GHOST' : 'Edit GHOST'}
                </button>
                <button type="button" onClick={() => void onRevealRawFile(hero, 'GHOST.md')}>
                  Open raw file
                </button>
              </div>
            </section>
            <button type="button" className="tavern-remove-quiet" onClick={() => onRemove(hero)}>
              Remove Hero
            </button>
          </section>

          <section className="tavern-profile-panel skills-panel" aria-label="SKILLS">
            <PanelTitle icon={iconSkills}>SKILLS</PanelTitle>
            {skillsLoading && accountSkills.length === 0 ? (
              <TavernLoadingBlock label="Skills" />
            ) : skillEntries.length === 0 ? (
              <div className="tavern-rule-empty">No skills in $KOTA_HOME/skills.</div>
            ) : (
              <SkillActivationList
                entries={skillEntries}
                onChange={setSkillActive}
                onOpenSkillFolder={(skill) => void onOpenSkillFolder(skill)}
              />
            )}
          </section>
        </div>
      </div>
    </section>
  );
}

function SystemProfileOverlay({
  hero,
  draft,
  templates,
  promptsReady,
  promptsLoading,
  onBack,
  onUpdate,
  promptResetBusy,
  onResetPromptFiles,
}: {
  hero: SystemHeroSpec;
  draft: SystemHeroDraft;
  templates: SystemPromptTemplates;
  promptsReady: boolean;
  promptsLoading: boolean;
  onBack: () => void;
  onUpdate: (patch: SystemHeroDraft) => void;
  promptResetBusy: boolean;
  onResetPromptFiles: () => void;
}) {
  const needsPromptTemplates = systemHeroNeedsPromptTemplates(hero);
  return (
    <section className="tavern-profile-overlay" role="dialog" aria-label={`${hero.name} profile`}>
      <div className="tavern-profile-card system">
        <button type="button" className="tavern-profile-back" onClick={onBack}>
          Back
        </button>
        <div className="tavern-system-profile">
          <Avatar className={`${hero.avatarClass} profile`} />
          <div className="tavern-system-profile-title">
            <span>{hero.role}</span>
            <h2>{hero.name}</h2>
            <p>{hero.description}</p>
          </div>
          {needsPromptTemplates && !promptsReady ? (
            <TavernLoadingBlock label={promptsLoading ? 'Prompts' : 'Prompt files'} />
          ) : (
            <SystemHeroConfig
              hero={hero}
              draft={draft}
              templates={templates}
              onUpdate={onUpdate}
              promptResetBusy={promptResetBusy}
              onResetPromptFiles={onResetPromptFiles}
            />
          )}
        </div>
      </div>
    </section>
  );
}

function systemHeroNeedsPromptTemplates(hero: SystemHeroSpec): boolean {
  return hero.configKind === 'magi'
    || hero.configKind === 'violet'
    || hero.configKind === 'ember'
    || hero.configKind === 'bbs'
    || hero.configKind === 'bartender';
}

function SystemHeroConfig({
  hero,
  draft,
  templates,
  onUpdate,
  promptResetBusy,
  onResetPromptFiles,
}: {
  hero: SystemHeroSpec;
  draft: SystemHeroDraft;
  templates: SystemPromptTemplates;
  onUpdate: (patch: SystemHeroDraft) => void;
  promptResetBusy: boolean;
  onResetPromptFiles: () => void;
}) {
  if (hero.configKind === 'magi') {
    const provider = normalizeMagiProvider(draft.provider);
    const [primaryProvider, backupProvider] = magiProviderOrder(provider);
    return (
      <div className="tavern-system-config">
        <section className="tavern-profile-field magi-provider-field">
          <span>NLP provider</span>
          <div className="tavern-provider-row" role="radiogroup" aria-label="Magi NLP provider">
            {(['claude', 'codex'] as MagiProvider[]).map((providerId) => (
              <button
                key={providerId}
                type="button"
                className={provider === providerId ? 'active' : ''}
                aria-checked={provider === providerId}
                role="radio"
                onClick={() => onUpdate({ provider: providerId })}
              >
                {providerId === 'claude' ? 'Claude Code' : 'Codex'}
              </button>
            ))}
          </div>
          <small className="tavern-profile-hint">
            Primary {primaryProvider}; fallback {backupProvider}.
          </small>
        </section>
        <label className="tavern-profile-field">
          Translation command
          <textarea
            value={[
              `1. ${magiTranslateCommand(primaryProvider)}`,
              `2. ${magiTranslateCommand(backupProvider)}`,
            ].join('\n')}
            readOnly
            spellCheck={false}
          />
        </label>
        <label className="tavern-profile-field">
          Handoff command
          <input value={magiHandoffCommand(primaryProvider)} readOnly />
        </label>
        <label className="tavern-profile-field">
          Prompt
          <textarea value={templates.magiPrompt} readOnly spellCheck={false} />
          <small className="tavern-profile-hint">Prompt file: {MAGI_PROMPT_PATH}</small>
        </label>
        <button type="button" className="tavern-raw-button system-reset" disabled={promptResetBusy} onClick={onResetPromptFiles}>
          Reset prompt files
        </button>
      </div>
    );
  }

  if (hero.configKind === 'violet') {
    const provider = normalizeVioletSummaryProvider(draft.provider);
    const triggerMessages = draft.summaryTriggerMessages ?? DEFAULT_VIOLET_SUMMARY_CONFIG.triggerAMessages;
    const triggerHours = draft.summaryTriggerHours ?? DEFAULT_VIOLET_SUMMARY_CONFIG.triggerBHours;
    const triggerMinOutstanding = draft.summaryTriggerMinOutstanding
      ?? DEFAULT_VIOLET_SUMMARY_CONFIG.triggerBMinOutstanding;
    return (
      <div className="tavern-system-config">
        <section className="tavern-profile-field violet-provider-field">
          <span>Summary provider</span>
          <div className="tavern-provider-row" role="radiogroup" aria-label="Violet summary provider">
            {(['codex', 'claude'] as VioletSummaryProvider[]).map((providerId) => (
              <button
                key={providerId}
                type="button"
                className={provider === providerId ? 'active' : ''}
                aria-checked={provider === providerId}
                role="radio"
                onClick={() => onUpdate({ provider: providerId })}
              >
                {providerId === 'claude' ? 'Claude Code' : 'Codex'}
              </button>
            ))}
          </div>
          <small className="tavern-profile-hint">
            Counts only project-local end-turn user/assistant messages.
          </small>
        </section>
        <label className="tavern-profile-field">
          Trigger A · outstanding messages
          <input
            type="number"
            min={1}
            value={triggerMessages}
            onChange={(e) => onUpdate({ summaryTriggerMessages: Number(e.currentTarget.value) })}
          />
          <small className="tavern-profile-hint">
            Auto-summary runs when unsummarized messages reach this number.
          </small>
        </label>
        <label className="tavern-profile-field">
          Trigger B · hours since last summary
          <input
            type="number"
            min={1}
            value={triggerHours}
            onChange={(e) => onUpdate({ summaryTriggerHours: Number(e.currentTarget.value) })}
          />
          <small className="tavern-profile-hint">
            Default is 2 hours after the last auto/manual summary.
          </small>
        </label>
        <label className="tavern-profile-field">
          Trigger B · outstanding messages
          <input
            type="number"
            min={1}
            value={triggerMinOutstanding}
            onChange={(e) => onUpdate({ summaryTriggerMinOutstanding: Number(e.currentTarget.value) })}
          />
          <small className="tavern-profile-hint">
            Trigger B also requires outstanding messages greater than this number.
          </small>
        </label>
        <label className="tavern-profile-field">
          Launch command
          <input value={violetSummaryCommand(provider)} readOnly />
        </label>
        <label className="tavern-profile-field">
          Prompt
          <textarea
            value={templates.violetSummaryPrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {VIOLET_SUMMARY_PROMPT_PATH}</small>
        </label>
        <label className="tavern-profile-field">
          Summary log
          <input value={VIOLET_SUMMARY_LOG_PATH} readOnly />
        </label>
        <button type="button" className="tavern-raw-button system-reset" disabled={promptResetBusy} onClick={onResetPromptFiles}>
          Reset prompt files
        </button>
      </div>
    );
  }

  if (hero.configKind === 'bartender') {
    return (
      <div className="tavern-system-config">
        <label className="tavern-profile-field">
          Local sync conflict handoff prompt
          <textarea
            value={templates.bartenderSyncConflictPrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {BARTENDER_SYNC_CONFLICT_PROMPT_PATH}</small>
        </label>
        <label className="tavern-profile-field">
          Pull conflict handoff prompt
          <textarea
            value={templates.bartenderPullConflictPrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {BARTENDER_PULL_CONFLICT_PROMPT_PATH}</small>
        </label>
        <button type="button" className="tavern-raw-button system-reset" disabled={promptResetBusy} onClick={onResetPromptFiles}>
          Reset prompt files
        </button>
      </div>
    );
  }

  if (hero.configKind === 'ember') {
    return (
      <div className="tavern-system-config">
        <label className="tavern-profile-field">
          Role
          <textarea
            value="Project timed prompts, side notes, and the account-level Dream routine."
            readOnly
            spellCheck={false}
          />
        </label>
        <label className="tavern-profile-field">
          Dream Agent Prompt
          <textarea
            value={templates.emberDreamAgentPrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {EMBER_DREAM_AGENT_PROMPT_PATH}</small>
        </label>
        <label className="tavern-profile-field">
          Dream Consolidation Prompt
          <textarea
            value={templates.emberDreamConsolidatePrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {EMBER_DREAM_CONSOLIDATE_PROMPT_PATH}</small>
        </label>
        <button type="button" className="tavern-raw-button system-reset" disabled={promptResetBusy} onClick={onResetPromptFiles}>
          Reset prompt files
        </button>
      </div>
    );
  }

  if (hero.configKind === 'laughing-man') {
    return (
      <div className="tavern-system-config">
        <LaughingManSettings />
      </div>
    );
  }

  if (hero.configKind === 'bbs') {
    return (
      <div className="tavern-system-config">
        <label className="tavern-profile-field">
          BBS Post wrapper
          <textarea
            value={templates.bbsPostPrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {BBS_POST_PROMPT_PATH}</small>
        </label>
        <label className="tavern-profile-field">
          BBS Reply wrapper
          <textarea
            value={templates.bbsReplyPrompt}
            readOnly
            spellCheck={false}
          />
          <small className="tavern-profile-hint">Prompt file: {BBS_REPLY_PROMPT_PATH}</small>
        </label>
        <button type="button" className="tavern-raw-button system-reset" disabled={promptResetBusy} onClick={onResetPromptFiles}>
          Reset prompt files
        </button>
      </div>
    );
  }

  return (
    <div className="tavern-system-config">
      <div className="tavern-placeholder-note">This system Hero is a placeholder until its runtime is implemented.</div>
    </div>
  );
}

function HeroMeritBar({ record }: { record?: ProjectAgentRecord | null }) {
  const merits = heroMerits(record);
  return (
    <div className="tavern-merit-bar" aria-label="Hero merits">
      {merits.map((merit) => (
        <MeritBadge key={merit.label} image={merit.icon} label={merit.label} value={merit.value} />
      ))}
    </div>
  );
}

function MeritBadge({ image, label, value }: { image: string; label: string; value: string }) {
  return (
    <div className="tavern-merit-badge">
      <img src={image} alt="" aria-hidden />
      <span>
        <b>{value}</b>
        <small>{label}</small>
      </span>
    </div>
  );
}

function TavernHeroNameEditor({
  name,
  nameFields,
  duplicateNameFor,
  renderAvatar,
  onChange,
}: {
  name: string;
  nameFields?: ProjectAgentNameFields | null;
  duplicateNameFor?: (name: string) => string | null;
  renderAvatar: () => ReactNode;
  onChange: (name: string, nameFields: ProjectAgentNameFields) => boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<ProjectAgentNameFields>(() => nameFields ?? projectAgentNameFields(name));

  useEffect(() => {
    if (!editing) setDraft(nameFields ?? projectAgentNameFields(name));
  }, [editing, name, nameFields]);

  const editedName = editing ? composeProjectAgentName(draft).trim() : name.trim();
  const duplicateName = editing && editedName ? duplicateNameFor?.(editedName) ?? null : null;
  const canSave = !!draft.given.trim() && !duplicateName;

  const confirm = () => {
    const next = editedName;
    if (!next) return;
    if (!canSave) return;
    if (!onChange(next, draft)) return;
    setEditing(false);
  };
  const cancel = () => {
    setDraft(nameFields ?? projectAgentNameFields(name));
    setEditing(false);
  };

  if (!editing) {
    return (
      <>
        {renderAvatar()}
        <button
          type="button"
          className="profile-name-button"
          title={name}
          data-full-name={name}
          onClick={() => setEditing(true)}
        >
          <ProjectAgentName name={name} titleLine className="profile-name-display" />
        </button>
      </>
    );
  }

  return (
    <>
      <ProjectAgentTitlePicker
        titleId={draft.titleId}
        onChange={(titleId) => setDraft((prev) => ({ ...prev, titleId }))}
      />
      {renderAvatar()}
      <div className="project-agent-name-editor-wrap">
        <div className="project-agent-name-editor tavern-hero-name-editor" aria-label="Edit hero name">
          <label>
            <span>Given name</span>
            <input
              aria-label="Given name"
              value={draft.given}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setDraft((prev) => ({ ...prev, given: value }));
              }}
              autoFocus
            />
          </label>
          <label>
            <span>Middle name</span>
            <input
              aria-label="Middle name"
              value={draft.middle}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setDraft((prev) => ({ ...prev, middle: value }));
              }}
            />
          </label>
          <label className="surname-field">
            <span>Surname</span>
            <input
              aria-label="Surname"
              value={draft.surname}
              onChange={(event) => {
                const value = event.currentTarget.value;
                setDraft((prev) => ({ ...prev, surname: value }));
              }}
            />
          </label>
          <div className="project-agent-name-editor-actions">
            <button type="button" className="accept" disabled={!canSave} onClick={confirm} aria-label="Save hero name">
              ✓
            </button>
            <button type="button" onClick={cancel} aria-label="Cancel hero name edit">
              x
            </button>
          </div>
        </div>
        {duplicateName && <div className="project-agent-error">Name already exists in Tavern.</div>}
      </div>
    </>
  );
}

function heroMerits(record?: ProjectAgentRecord | null) {
  return [
    { icon: iconTurns, label: 'Turns', value: formatHeroRecordValue(record?.turns ?? 0) },
    { icon: iconCommends, label: 'Commends', value: formatHeroRecordValue(record?.commends ?? 0) },
  ];
}

function HeroCardMerits({ record }: { record?: ProjectAgentRecord | null }) {
  const merits = heroMerits(record);
  return (
    <span className="tavern-card-merits" aria-label="Hero merits">
      {merits.map((merit) => (
        <span key={merit.label} className="tavern-card-merit" title={`${merit.label}: ${merit.value}`}>
          <img src={merit.icon} alt="" aria-hidden />
          <b>{merit.value}</b>
        </span>
      ))}
    </span>
  );
}

function formatHeroRecordValue(value: number): string {
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(value >= 10_000_000 ? 0 : 1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}K`;
  return String(value);
}

function PanelTitle({ icon, children }: { icon: string; children: ReactNode }) {
  return (
    <div className="tavern-profile-panel-title">
      <img src={icon} alt="" aria-hidden />
      <span>{children}</span>
    </div>
  );
}

function SystemHeroCard({ hero, onSelect }: { hero: SystemHeroSpec; onSelect: () => void }) {
  return (
    <button type="button" className="tavern-system-card" onClick={onSelect}>
      <Avatar className={hero.avatarClass} />
      <span>
        <b>{hero.name}</b>
        <small>{hero.role}</small>
      </span>
    </button>
  );
}

function Avatar({ className }: { className: string }) {
  return (
    <span className={`tavern-avatar-art ${className}`} aria-hidden>
      <span />
      <i />
      <b />
    </span>
  );
}

export function TavernLoadingLog({
  items,
  lead = 'Loading',
  className = '',
}: {
  items: readonly TavernLoadingLogItem[];
  lead?: string;
  className?: string;
}) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (items.length === 0) return undefined;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(timer);
  }, [items.length]);
  if (items.length === 0) return null;
  const visibleItems = items.slice(0, 4);
  const extraCount = items.length - visibleItems.length;
  return (
    <div
      className={['tavern-loading-log', className].filter(Boolean).join(' ')}
      role="status"
      aria-live="polite"
      aria-label={`${lead}: ${items.map((item) => item.label).join(', ')}`}
    >
      <span className="tavern-loading-log-dot" aria-hidden />
      <span className="tavern-loading-log-copy">
        <b>{lead}</b>
        {visibleItems.map((item) => (
          <span key={item.id} className="tavern-loading-log-item">
            <span>{item.label}</span>
            <span className="tavern-loading-elapsed" aria-hidden="true">
              {formatTavernLoadingElapsed(now, item.startedAt)}
            </span>
          </span>
        ))}
        {extraCount > 0 && <span className="tavern-loading-log-extra">+{extraCount}</span>}
      </span>
    </div>
  );
}

function formatTavernLoadingElapsed(now: number, startedAt: number): string {
  const elapsedSeconds = Math.max(0, now - startedAt) / 1000;
  return `${elapsedSeconds.toFixed(1)}s`;
}

function TavernLoadingBlock({ label }: { label: string }) {
  return (
    <div className="tavern-loading-block" role="status" aria-live="polite">
      <span className="tavern-loading-spinner" aria-hidden />
      <span>Loading {label}</span>
    </div>
  );
}

function SupportedProviders({
  shells,
  shellsLoading,
}: {
  shells: SupportedShellStatus[];
  shellsLoading: boolean;
}) {
  const shellById = new Map(shells.map((shell) => [shell.id, shell]));
  return (
    <section className="tavern-provider-status" aria-label="Providers' Status">
      <div className="tavern-provider-status-label">Providers' Status</div>
      <div className="tavern-provider-lights">
        {PROVIDER_IDS.map((providerId) => {
          const provider = PROVIDERS[providerId];
          const shell = shellById.get(providerId);
          const checking = shellsLoading && !shell;
          const installed = !!shell?.installed;
          const state = checking ? 'checking' : installed ? 'ready' : 'missing';
          return (
            <a
              key={providerId}
              href={shell?.installUrl ?? provider.installUrl}
              target="_blank"
              rel="noreferrer"
              className={state}
              title={checking ? `Checking ${provider.name}` : installed ? `${provider.name} installed at ${shell?.resolvedBin}` : shell?.summary ?? provider.cli}
            >
              <span className="tavern-provider-icon" aria-hidden="true">
                <img src={provider.icon} alt="" />
                <span className={`tavern-provider-dot ${checking ? 'checking' : installed ? 'ok' : 'missing'}`} />
              </span>
              <span className="tavern-provider-name">{provider.name}</span>
              {provider.beta && <span className="tavern-beta-badge" aria-label="Beta provider">BETA</span>}
            </a>
          );
        })}
      </div>
    </section>
  );
}

function StorageMeasurementDetail({
  status,
  now,
  onRefresh,
}: {
  status: StorageMeasurementStatus | null;
  now: number;
  onRefresh: () => void;
}) {
  const hasMeasurement = status?.onDiskBytes != null;
  const updating = status?.updating ?? false;
  const size = hasMeasurement
    ? `≈ ${formatBytes(status.onDiskBytes ?? 0)}`
    : updating
      ? 'Updating…'
      : status
        ? 'not measured'
        : '…';
  return (
    <span className="tavern-storage-detail" role="status" aria-live="polite">
      <span>
        Size <span className="tavern-storage-value">{size}</span>
        {!hasMeasurement && updating && (
          <span className="tavern-storage-update-hint">(will take 2–5 min)</span>
        )}
      </span>
      {hasMeasurement && updating && (
        <>
          <StorageDetailSeparator />
          <span className="tavern-storage-updating">
            Updating… <span className="tavern-storage-update-hint">(will take 2–5 min)</span>
          </span>
        </>
      )}
      {hasMeasurement && status?.availableBytes != null && (
        <>
          <StorageDetailSeparator />
          <span><span className="tavern-storage-value">{formatBytes(status.availableBytes)}</span> available</span>
        </>
      )}
      {hasMeasurement && status?.measuredAt != null && (
        <>
          <StorageDetailSeparator />
          <span>Updated {formatStorageMeasurementAge(status.measuredAt, now)}</span>
        </>
      )}
      {status?.error && (
        <>
          <StorageDetailSeparator />
          <span className="tavern-storage-error">Refresh failed</span>
        </>
      )}
      {status && (
        <button
          type="button"
          className={`tavern-storage-refresh${updating ? ' updating' : ''}`}
          aria-label="Refresh storage usage"
          title="Refresh storage usage"
          disabled={updating}
          onClick={onRefresh}
        >
          <svg viewBox="0 0 16 16" aria-hidden>
            <path d="M13.2 5.25A5.7 5.7 0 1 0 13 11" />
            <path d="M13.2 2.6v2.9h-2.9" />
          </svg>
        </button>
      )}
    </span>
  );
}

function StorageDetailSeparator() {
  return <span className="tavern-storage-separator" aria-hidden>·</span>;
}

export function formatStorageMeasurementAge(measuredAt: number, now = Date.now()): string {
  const elapsedSeconds = Math.max(0, Math.floor(now / 1000) - measuredAt);
  if (elapsedSeconds < 60) return 'just now';
  const elapsedMinutes = Math.floor(elapsedSeconds / 60);
  if (elapsedMinutes < 60) return `${elapsedMinutes}m ago`;
  const elapsedHours = Math.floor(elapsedMinutes / 60);
  if (elapsedHours < 24) return `${elapsedHours}h ago`;
  const elapsedDays = Math.floor(elapsedHours / 24);
  return `${elapsedDays} ${elapsedDays === 1 ? 'day' : 'days'} ago`;
}

function githubInitial(username: string | null | undefined): string {
  const trimmed = username?.trim();
  return trimmed ? trimmed.slice(0, 1).toUpperCase() : 'G';
}

function ConnectionCard({
  icon,
  title,
  connected,
  fullWidth = false,
  showStatusDot = true,
  status,
  primary,
  description,
  details,
  actionLabel,
  actionTone,
  disabled,
  onAction,
  children,
}: {
  icon?: 'google-drive' | 'github';
  title: string;
  connected: boolean;
  fullWidth?: boolean;
  showStatusDot?: boolean;
  status?: 'missing' | 'setup' | 'ok' | 'checking';
  primary: ReactNode;
  description?: string;
  details: Array<[string, string]>;
  actionLabel?: string;
  actionTone?: 'danger';
  disabled?: boolean;
  onAction?: () => void;
  children?: ReactNode;
}) {
  return (
    <section className={`tavern-connection-card${fullWidth ? ' full' : ''}`}>
      <div className="tavern-card-head">
        <div className="tavern-connection-title">
          {icon && <ConnectionLogo kind={icon} />}
          <div>
            <div className="tavern-section-title">{title}</div>
            <div className="tavern-identity">{primary}</div>
          </div>
        </div>
        {showStatusDot && <span className={`tavern-dot ${status ?? (connected ? 'ok' : '')}`} />}
      </div>
      {description && <div className="tavern-connection-copy">{description}</div>}
      {children}
      {details.length > 0 && (
        <div className="tavern-readouts">
          {details.map(([label, value]) => (
            <Readout key={label} label={label} value={value || 'unset'} />
          ))}
        </div>
      )}
      {actionLabel && onAction && (
        <button
          type="button"
          className={actionTone === 'danger' ? 'danger' : undefined}
          disabled={disabled}
          onClick={onAction}
        >
          {actionLabel}
        </button>
      )}
    </section>
  );
}

function ConnectionLogo({ kind }: { kind: 'google-drive' | 'github' }) {
  if (kind === 'google-drive') {
    return (
      <span className="tavern-connection-logo google-drive" aria-hidden>
        <svg viewBox="0 0 48 42" role="img">
          <path fill="#1fa463" d="M17.2 1h13.6l16.3 28.2H33.4L17.2 1Z" />
          <path fill="#fbbc04" d="M17.2 1 .9 29.2l6.8 11.8L24 12.8 17.2 1Z" />
          <path fill="#4285f4" d="M7.7 41h32.6l6.8-11.8H14.5L7.7 41Z" />
          <path fill="#188038" d="M24 12.8 14.5 29.2h18.9L24 12.8Z" opacity=".9" />
        </svg>
      </span>
    );
  }
  return (
    <span className="tavern-connection-logo github" aria-hidden>
      <svg viewBox="0 0 24 24" role="img">
        <path
          fill="currentColor"
          d="M12 .8a11.2 11.2 0 0 0-3.54 21.82c.56.1.76-.24.76-.54v-2.06c-3.12.68-3.78-1.32-3.78-1.32-.5-1.28-1.24-1.62-1.24-1.62-1.02-.7.08-.68.08-.68 1.12.08 1.72 1.16 1.72 1.16 1 .1.52 2.08 3.18 1.5.1-.72.4-1.2.72-1.48-2.5-.28-5.12-1.24-5.12-5.56 0-1.24.44-2.24 1.16-3.02-.12-.28-.5-1.44.1-2.98 0 0 .94-.3 3.08 1.16A10.66 10.66 0 0 1 12 6.78c.96 0 1.92.14 2.82.4 2.14-1.46 3.08-1.16 3.08-1.16.6 1.54.22 2.7.1 2.98.72.78 1.16 1.78 1.16 3.02 0 4.34-2.62 5.28-5.12 5.56.4.34.76 1.04.76 2.08v2.42c0 .3.2.64.78.54A11.2 11.2 0 0 0 12 .8Z"
        />
      </svg>
    </span>
  );
}

function Readout({ label, value, detail }: { label: string; value: string; detail?: ReactNode }) {
  return (
    <div className="tavern-readout">
      <span>{label}</span>
      <b>
        {value}
        {detail != null && <small>{detail}</small>}
      </b>
    </div>
  );
}

function normalizeDraft(hero: AgentCardSpec, draft?: Partial<HeroDraft>): HeroDraft {
  const provider = providerIdFromStored(draft?.provider, hero.provider) ?? hero.provider;
  const providerSpec = PROVIDERS[provider];
  const draftName = draft?.name;
  const storedModel = draft?.model ?? providerSpec.defaultModel;
  const migratingFactoryDefault = isFactoryDefaultModelMigration(hero.id, provider, storedModel);
  const model = normalizeDefaultModel(hero, provider, storedModel);
  const avatarId = normalizeHeroAvatarId(draft?.avatarId, provider);
  const merged: HeroDraft = {
    name: isLegacyTemplateName(hero, draftName) ? hero.name : draftName ?? hero.name,
    nameFields: draft?.nameFields ?? null,
    provider,
    model,
    effort: draft?.effort ?? providerSpec.defaultEffort,
    avatarId,
    skills: draft?.skills ?? DEFAULT_SKILLS,
    ghost: normalizeFactoryGhost(hero, providerSpec, draft?.ghost),
    shell: migratingFactoryDefault ? '' : draft?.shell ?? '',
    record: draft?.record ?? null,
    archived: draft?.archived ?? false,
    dismissed: draft?.dismissed ?? false,
  };
  if (!merged.shell) merged.shell = buildShellYaml(merged);
  return merged;
}

function heroNameFields(draft: Pick<HeroDraft, 'name' | 'nameFields'>): ProjectAgentNameFields {
  const parsed = projectAgentNameFields(draft.name);
  return {
    titleId: draft.nameFields?.titleId ?? parsed.titleId,
    given: draft.nameFields?.given?.trim() || parsed.given,
    middle: draft.nameFields?.middle ?? parsed.middle,
    surname: draft.nameFields?.surname ?? parsed.surname,
  };
}

function isLegacyTemplateName(hero: AgentCardSpec, name?: string): boolean {
  if (!DEFAULT_HERO_TEMPLATE_IDS.has(hero.id) || !name) return false;
  return LEGACY_TEMPLATE_NAMES[hero.provider].includes(name);
}

function normalizeModelIdForProvider(provider: ProviderId, model: string): string {
  if (provider === 'opencode' && model === OPENCODE_LEGACY_KIMI_MODEL) {
    return OPENCODE_KIMI_MODEL;
  }
  if (provider === 'pi') {
    return normalizePiModelId(model);
  }
  return model;
}

function normalizePiModelId(model: string): string {
  const trimmed = model.trim();
  if (trimmed.includes('/')) return trimmed;
  if (trimmed.startsWith('glm-')) return `zai/${trimmed}`;
  if (trimmed.startsWith('kimi-') || trimmed.startsWith('k2p')) return `kimi-coding/${trimmed}`;
  return trimmed;
}

function normalizeDefaultModel(hero: AgentCardSpec, provider: ProviderId, model: string): string {
  const normalized = normalizeModelIdForProvider(provider, model);
  if (isFactoryDefaultModelMigration(hero.id, provider, normalized)) {
    return 'default';
  }
  if (DEFAULT_HERO_TEMPLATE_IDS.has(hero.id) && provider === 'opencode' && normalized === 'openai/gpt-5.5') {
    return PROVIDERS.opencode.defaultModel;
  }
  return normalized;
}

function isFactoryDefaultModelMigration(heroId: string, provider: ProviderId, model: string): boolean {
  const migration = FACTORY_DEFAULT_MODEL_MIGRATIONS[heroId];
  return !!migration
    && migration.provider === provider
    && migration.model === normalizeModelIdForProvider(provider, model);
}

function defaultGhost(hero: AgentCardSpec, provider: ProviderSpec): string {
  void hero;
  void provider;
  return FACTORY_HERO_GHOST;
}

function normalizeFactoryGhost(
  hero: AgentCardSpec,
  provider: ProviderSpec,
  ghost?: string,
): string {
  if (!ghost || isLegacyFactoryGhost(hero, ghost)) return defaultGhost(hero, provider);
  return ghost;
}

function isLegacyFactoryGhost(hero: AgentCardSpec, ghost: string): boolean {
  const trimmed = ghost.trim();
  if (trimmed === FACTORY_HERO_GHOST.trim()) return true;
  return (
    trimmed.startsWith(`# ${hero.name}`) &&
    trimmed.includes(`You are ${hero.name}, a Kota hero`) &&
    trimmed.includes('Keep work scoped to the current project')
  );
}

function buildShellYaml(draft: HeroDraft): string {
  const provider = PROVIDERS[draft.provider];
  const model = normalizeModelIdForProvider(draft.provider, draft.model);
  const skills = draft.skills.length
    ? draft.skills.map((skill) => `  - ${skill}`).join('\n')
    : '  []';
  const args = launchArgs(draft).map((arg) => `  - ${JSON.stringify(arg)}`).join('\n');
  const comments = provider.id === 'antigravity'
    ? '\n# Antigravity model and thinking level are configured in Antigravity CLI settings, not emitted as startup flags.'
    : '';
  return [
    '# SHELL.yaml',
    `provider: ${provider.id}`,
    `command: ${provider.cli}`,
    'cwd: "$KOTA_WORKTREE_ROOT"',
    `model: ${model}`,
    draft.effort ? `effort: ${draft.effort}` : undefined,
    'skills:',
    skills,
    'args:',
    args || '  []',
    comments,
  ].filter((line) => line != null).join('\n');
}

function providerCli(provider: ProviderId): AgentCli {
  switch (provider) {
    case 'claude':
      return 'claude';
    case 'codex':
      return 'codex';
    case 'antigravity':
      return 'antigravity';
    case 'opencode':
      return 'opencode';
    case 'pi':
      return 'pi';
    case 'kimi':
      return 'kimi';
  }
}

function isProviderId(value: string): value is ProviderId {
  return Object.prototype.hasOwnProperty.call(PROVIDERS, value);
}

function providerIdFromStored(value: unknown, fallback?: ProviderId): ProviderId | undefined {
  if (typeof value !== 'string') return fallback;
  if (isProviderId(value)) return value;
  if (value === 'gemini' || value === 'gemini-cli') return 'antigravity';
  if (value === 'kimi-code') return 'kimi';
  return fallback;
}

function normalizeAccountUserIdentity(identity: AccountUserIdentity): AccountUserIdentity {
  return {
    name: identity.name?.trim() || DEFAULT_ACCOUNT_USER_IDENTITY.name,
    avatarId: identity.avatarId || DEFAULT_ACCOUNT_USER_IDENTITY.avatarId,
  };
}

function tavernProfileToDraft(profile: TavernHeroProfileDraft): Partial<HeroDraft> {
  const provider = providerIdFromStored(profile.provider);
  return {
    name: profile.name,
    nameFields: profile.nameFields ? {
      titleId: profile.nameFields.titleId ?? null,
      given: profile.nameFields.given,
      middle: profile.nameFields.middle ?? '',
      surname: profile.nameFields.surname ?? '',
    } : null,
    provider,
    model: provider ? normalizeModelIdForProvider(provider, profile.model) : profile.model,
    effort: profile.effort ?? undefined,
    avatarId: normalizeHeroAvatarId(profile.avatarId, profile.provider),
    skills: [...profile.skills],
    ghost: profile.ghost,
    shell: profile.shell,
    record: profile.record ?? null,
    archived: profile.archived,
    dismissed: profile.dismissed,
  };
}

function tavernProfilesForPersistence(
  heroes: AgentCardSpec[],
  profiles: Record<string, Partial<HeroDraft>>,
): TavernHeroProfileDraft[] {
  return heroes.flatMap((hero) => {
    const draft = normalizeDraft(hero, profiles[hero.id]);
    return [{
      heroId: hero.id,
      name: draft.name,
      nameFields: heroNameFields(draft),
      provider: draft.provider,
      model: draft.model,
      effort: draft.effort ?? null,
      avatarId: draft.avatarId,
      skills: [...draft.skills],
      ghost: draft.ghost,
      shell: draft.shell || buildShellYaml(draft),
      archived: !!draft.archived,
      dismissed: !!draft.dismissed,
      kind: hero.kind,
      record: draft.record ?? null,
    }];
  });
}

function serializeTavernProfilesForPersistence(
  heroes: AgentCardSpec[],
  profiles: Record<string, Partial<HeroDraft>>,
): string {
  return JSON.stringify(tavernProfilesForPersistence(heroes, profiles));
}

function launchArgs(draft: HeroDraft): string[] {
  const model = normalizeModelIdForProvider(draft.provider, draft.model);
  const modelArgs = model === 'default' ? [] : ['--model', model];
  switch (draft.provider) {
    case 'claude':
      return [...modelArgs, '--effort', draft.effort ?? 'max', '--dangerously-skip-permissions'];
    case 'codex':
      return [
        ...modelArgs,
        '--config',
        `model_reasoning_effort="${draft.effort ?? 'xhigh'}"`,
        '--dangerously-bypass-approvals-and-sandbox',
      ];
    case 'antigravity':
      return ['--dangerously-skip-permissions'];
    case 'opencode':
      return [...modelArgs, '--pure', '--dangerously-skip-permissions'];
    case 'pi':
      return [...modelArgs, '--thinking', draft.effort ?? 'xhigh', '--approve'];
    case 'kimi':
      return [...modelArgs, '--yolo'];
  }
}

function defaultSystemDraft(
  id: SystemHeroId,
  templates: SystemPromptTemplates = defaultSystemPromptTemplates(),
): SystemHeroDraft {
  switch (id) {
    case 'magi':
      return {
        provider: 'claude',
      };
    case 'violet':
      return {
        provider: DEFAULT_VIOLET_SUMMARY_CONFIG.provider,
        summaryTriggerMessages: DEFAULT_VIOLET_SUMMARY_CONFIG.triggerAMessages,
        summaryTriggerHours: DEFAULT_VIOLET_SUMMARY_CONFIG.triggerBHours,
        summaryTriggerMinOutstanding: DEFAULT_VIOLET_SUMMARY_CONFIG.triggerBMinOutstanding,
        launchCommand: violetSummaryCommand(DEFAULT_VIOLET_SUMMARY_CONFIG.provider),
        prompt: templates.violetSummaryPrompt,
      };
    case 'ember':
      return {};
    case 'bbs':
      return {};
    case 'bartender':
      return {};
    case 'laughing-man':
    case 'puppeteer':
      return {};
  }
}

function tavernHeroNameKey(name: string): string {
  return name.trim().replace(/\s+/g, ' ').toLowerCase();
}

function tavernShellProviderClass(providerId: ProviderId): string {
  switch (providerId) {
    case 'claude': return 'provider-claude';
    case 'codex': return 'provider-codex';
    case 'antigravity': return 'provider-gemini';
    case 'opencode': return 'provider-opencode';
    case 'pi': return 'provider-pi';
    case 'kimi': return 'provider-kimi';
  }
}

function ghostPreview(value: string): string {
  return value
    .split('\n')
    .filter((line) => line.trim().length > 0)
    .slice(0, 5)
    .join('\n');
}

function loadProfileDrafts(): Record<string, Partial<HeroDraft>> {
  if (typeof window === 'undefined') return {};
  try {
    const raw = window.localStorage.getItem(PROFILE_STORAGE_KEY);
    if (!raw) return {};
    return JSON.parse(raw) as Record<string, Partial<HeroDraft>>;
  } catch {
    return {};
  }
}

function tavernStateFromProfiles(savedProfiles: TavernHeroProfileDraft[]): {
  customHeroes: AgentCardSpec[];
  profiles: Record<string, Partial<HeroDraft>>;
  migratedFactoryDefaults: boolean;
} {
  const builtInIds = new Set(HERO_TEMPLATES.map((hero) => hero.id));
  const profiles: Record<string, Partial<HeroDraft>> = {};
  const customHeroes: AgentCardSpec[] = [];
  let migratedFactoryDefaults = false;
  for (const profile of savedProfiles) {
    const provider = providerIdFromStored(profile.provider);
    if (provider && isFactoryDefaultModelMigration(profile.heroId, provider, profile.model)) {
      migratedFactoryDefaults = true;
    }
    profiles[profile.heroId] = tavernProfileToDraft(profile);
    if (builtInIds.has(profile.heroId)) continue;
    if (LEGACY_FACTORY_TEMPLATE_IDS.has(profile.heroId)) continue;
    customHeroes.push({
      id: profile.heroId,
      kind: profile.kind === 'invited' ? 'invited' : 'custom',
      provider: providerIdFromStored(profile.provider, 'antigravity') ?? 'antigravity',
      name: profile.name || 'New Hero',
    });
  }
  return { customHeroes, profiles, migratedFactoryDefaults };
}

export function syncTavernHeroStorageFromProfiles(savedProfiles: TavernHeroProfileDraft[]): void {
  if (typeof window === 'undefined') return;
  const { customHeroes, profiles } = tavernStateFromProfiles(savedProfiles);
  try {
    window.localStorage.setItem(PROFILE_STORAGE_KEY, JSON.stringify(profiles));
    window.localStorage.setItem(CUSTOM_HERO_STORAGE_KEY, JSON.stringify(customHeroes));
    window.dispatchEvent(new Event(TAVERN_PROFILE_CHANGED_EVENT));
  } catch {
    // localStorage sync is best-effort; disk remains the source of truth.
  }
}

export async function syncTavernHeroStorageFromDisk(): Promise<readonly WorkingHero[]> {
  const shells = await supportedShellsStatus();
  if (!hasTauriRuntime()) return loadTavernWorkingHeroes(shells);
  const savedProfiles = await loadTavernHeroProfiles();
  syncTavernHeroStorageFromProfiles(savedProfiles);
  return loadTavernWorkingHeroes(shells);
}

function loadSystemDrafts(): Record<SystemHeroId, SystemHeroDraft> {
  if (typeof window === 'undefined') return {} as Record<SystemHeroId, SystemHeroDraft>;
  try {
    const raw = window.localStorage.getItem(SYSTEM_STORAGE_KEY);
    if (!raw) return {} as Record<SystemHeroId, SystemHeroDraft>;
    return JSON.parse(raw) as Record<SystemHeroId, SystemHeroDraft>;
  } catch {
    return {} as Record<SystemHeroId, SystemHeroDraft>;
  }
}

function loadCustomHeroes(): AgentCardSpec[] {
  if (typeof window === 'undefined') return [];
  try {
    const raw = window.localStorage.getItem(CUSTOM_HERO_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as AgentCardSpec[];
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((hero) => {
      if (STALE_FAKE_CUSTOM_HERO_NAMES.has(hero.name)) return [];
      return [{
        ...hero,
        kind: hero.kind === 'invited' ? 'invited' : 'custom',
        provider: providerIdFromStored(hero.provider, 'antigravity') ?? 'antigravity',
      }];
    });
  } catch {
    return [];
  }
}

export function loadTavernWorkingHeroes(shells?: readonly SupportedShellStatus[]): readonly WorkingHero[] {
  const profiles = loadProfileDrafts();
  const heroes = [...HERO_TEMPLATES, ...loadCustomHeroes()];
  const shellByProvider = shells ? new Map(shells.map((shell) => [shell.id, shell])) : null;
  return heroes
    .filter((hero) => isWorkingHeroAvailable(hero, profiles[hero.id]))
    .map((hero) => {
      const draft = normalizeDraft(hero, profiles[hero.id]);
      const shell = shellByProvider?.get(draft.provider);
      const available = shell ? shell.installed : undefined;
      return {
        id: hero.id,
        templateId: hero.id,
        cli: providerCli(draft.provider),
        name: draft.name,
        record: draft.effort ? `${draft.model} / ${draft.effort}` : draft.model,
        avatarId: draft.avatarId,
        avatarClass: avatarClassForId(draft.avatarId, draft.provider),
        available,
        unavailableReason: available === false ? `${PROVIDERS[draft.provider].name} CLI unavailable` : undefined,
      };
    });
}

export function loadTavernHeroIncarnationProfile(
  agentId: AgentId,
): TavernHeroIncarnationProfile | null {
  const profiles = loadProfileDrafts();
  const hero = [...HERO_TEMPLATES, ...loadCustomHeroes()].find((candidate) => candidate.id === agentId);
  if (!hero) return null;
  const draft = normalizeDraft(hero, profiles[hero.id]);
  if (!isWorkingHeroDraftAvailable(draft)) return null;
  return {
    agentId: hero.id,
    kind: hero.kind,
    cli: providerCli(draft.provider),
    name: draft.name,
    provider: draft.provider,
    model: draft.model,
    effort: draft.effort,
    avatarId: draft.avatarId,
    skills: [...draft.skills],
    ghost: draft.ghost,
    shell: buildShellYaml(draft),
    args: launchArgs(draft),
  };
}

function isWorkingHeroAvailable(hero: AgentCardSpec, draft?: Partial<HeroDraft>): boolean {
  return isWorkingHeroDraftAvailable(normalizeDraft(hero, draft));
}

function isWorkingHeroDraftAvailable(draft: HeroDraft): boolean {
  return !draft.archived && !draft.dismissed;
}

function titleCase(value: string): string {
  return value.slice(0, 1).toUpperCase() + value.slice(1);
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  let value = bytes;
  let idx = 0;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx++;
  }
  return `${value.toFixed(idx === 0 ? 0 : 1)} ${units[idx]}`;
}
