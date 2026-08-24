import dreamAgentPromptTemplate from '../src-tauri/prompts/ember-dream-agent.md?raw';
import dreamConsolidatePromptTemplate from '../src-tauri/prompts/ember-dream-consolidate.md?raw';
import { readSystemPrompt, type AgentBusTerminalTiming } from './pty-client';
import type { AgentId } from './types/scene';

export type EmberScheduleMode = 'idle' | 'delay' | 'at' | 'daily' | 'interval';
export type EmberDelayUnit = 'minutes' | 'hours' | 'days';
export type EmberRepeatUnit = 'minutes' | 'hours' | 'days';
export type EmberEndMode = 'never' | 'after' | 'at';
export type EmberRepeatKind = 'fixed' | 'weekly' | 'monthly';
export type EmberScheduleStatus = 'scheduled' | 'paused' | 'sent' | 'failed';
export type EmberHistoryStatus = 'delivered' | 'failed';
export type EmberHistoryTrigger = 'schedule' | 'manual';
export type EmberActorKind = 'human' | 'agent';

export interface EmberActorRef {
  kind: EmberActorKind;
  label: string;
}

export interface EmberDraft {
  id: string;
  text: string;
  createdAt: string;
  updatedAt: string;
}

export interface EmberSchedule {
  id: string;
  text: string;
  targetAgentId: AgentId;
  targetAgentName: string;
  targetAgentIds?: AgentId[];
  targetAgentNames?: string[];
  mode: EmberScheduleMode;
  delayAmount?: number;
  delayUnit?: EmberDelayUnit;
  atDateTime?: string;
  timeOfDay?: string;
  intervalHours?: number;
  waitForIdle?: boolean;
  repeatEnabled?: boolean;
  repeatAmount?: number;        // legacy single-value interval (back-compat)
  repeatUnit?: EmberRepeatUnit; // legacy
  repeatKind?: EmberRepeatKind;       // 'fixed' (default) | 'weekly' | 'monthly'
  repeatEveryMinutes?: number;        // fixed: total minutes (days*1440 + hrs*60 + min)
  repeatWeekDays?: number[];          // weekly: 0=Sun .. 6=Sat
  repeatEveryWeeks?: number;          // weekly: 1-50
  repeatMonthDays?: string[];         // monthly: '1'..'31' | 'last'
  repeatEveryMonths?: number;         // monthly: 1-12
  endMode?: EmberEndMode;
  endAfterCount?: number;
  endAt?: string;
  createdAt: string;
  updatedAt: string;
  nextRunAt: string;
  lastRunAt?: string | null;
  runCount: number;
  status: EmberScheduleStatus;
  error?: string | null;
  createdBy?: EmberActorRef | null;
  updatedBy?: EmberActorRef | null;
}

export interface EmberHistoryRecord {
  id: string;
  scheduleId: string;
  prompt: string;
  targetAgentIds: AgentId[];
  targetAgentNames: string[];
  sentAt: string;
  status: EmberHistoryStatus;
  triggeredBy?: EmberHistoryTrigger | null;
  error?: string | null;
  scheduledFor?: string | null;
  startedAt?: string | null;
  finishedAt?: string | null;
  reason?: string | null;
  missedRuns?: number | null;
}

export interface EmberState {
  schema?: string;
  drafts: EmberDraft[];
  schedules: EmberSchedule[];
  history: EmberHistoryRecord[];
  appLastSeenAt?: string | null;
}

export interface EmberScheduleInput {
  text: string;
  targetAgentId: AgentId;
  targetAgentName: string;
  targetAgentIds?: AgentId[];
  targetAgentNames?: string[];
  mode: EmberScheduleMode;
  delayAmount?: number;
  delayUnit?: EmberDelayUnit;
  atDateTime?: string;
  timeOfDay?: string;
  intervalHours?: number;
  waitForIdle?: boolean;
  repeatEnabled?: boolean;
  repeatAmount?: number;
  repeatUnit?: EmberRepeatUnit;
  repeatKind?: EmberRepeatKind;
  repeatEveryMinutes?: number;
  repeatWeekDays?: number[];
  repeatEveryWeeks?: number;
  repeatMonthDays?: string[];
  repeatEveryMonths?: number;
  endMode?: EmberEndMode;
  endAfterCount?: number;
  endAt?: string;
}

const EMBER_STORAGE_PREFIX = 'kota-v2.ember.';
export const EMBER_DREAM_AGENT_PROMPT_PATH = '$KOTA_HOME/heroes/system-ember/ember-dream-agent.md';
export const EMBER_DREAM_CONSOLIDATE_PROMPT_PATH = '$KOTA_HOME/heroes/system-ember/ember-dream-consolidate.md';
export const EMBER_DREAM_AGENT_PROMPT_TEMPLATE = dreamAgentPromptTemplate.trimEnd();
export const EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE = dreamConsolidatePromptTemplate.trimEnd();
// Must match HUMAN_TELEGRAM_TARGET_ID in src-tauri/src/ember.rs.
export const HUMAN_TELEGRAM_TARGET_ID = '__kota_human_telegram__' as AgentId;
export const EMBER_NOT_DELIVERED = 'Not Delivered';

export function isHumanTelegramTarget(value: string | null | undefined): boolean {
  return typeof value === 'string' && value.trim() === HUMAN_TELEGRAM_TARGET_ID;
}

export function emberStorageKey(projectKey: string | null | undefined): string | null {
  const key = projectKey?.trim();
  if (!key) return null;
  return `${EMBER_STORAGE_PREFIX}${encodeURIComponent(key)}`;
}

export function loadEmberState(projectKey: string | null | undefined): EmberState {
  const key = emberStorageKey(projectKey);
  if (!key || typeof window === 'undefined') return emptyEmberState();
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return emptyEmberState();
    return normalizeEmberState(JSON.parse(raw));
  } catch {
    return emptyEmberState();
  }
}

export function saveEmberState(projectKey: string | null | undefined, state: EmberState): void {
  const key = emberStorageKey(projectKey);
  if (!key || typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(key, JSON.stringify(normalizeEmberState(state)));
  } catch {
    // Storage is best-effort; keep the in-memory state usable.
  }
}

export function createEmberDraft(text: string): EmberDraft {
  const now = new Date().toISOString();
  return {
    id: mintEmberId('draft'),
    text: text.trim(),
    createdAt: now,
    updatedAt: now,
  };
}

export function createEmberSchedule(input: EmberScheduleInput, nowMs = Date.now()): EmberSchedule {
  const now = new Date(nowMs).toISOString();
  const targetAgentIds = normalizeTargetAgentIds(input.targetAgentIds, input.targetAgentId);
  const targetAgentNames = normalizeTargetAgentNames(input.targetAgentNames, targetAgentIds, input.targetAgentName);
  return {
    id: mintEmberId('schedule'),
    text: input.text.trim(),
    targetAgentId: targetAgentIds[0] ?? input.targetAgentId,
    targetAgentName: targetAgentNames[0] ?? input.targetAgentName,
    targetAgentIds,
    targetAgentNames,
    mode: input.mode,
    delayAmount: input.mode === 'delay' ? normalizePositiveInt(input.delayAmount, 10) : undefined,
    delayUnit: input.mode === 'delay' ? normalizeDelayUnit(input.delayUnit) : undefined,
    atDateTime: input.mode === 'at' ? normalizeDateTimeInput(input.atDateTime, nowMs) : undefined,
    timeOfDay: input.mode === 'daily' ? normalizeTimeOfDay(input.timeOfDay) : undefined,
    intervalHours: input.mode === 'interval' ? normalizePositiveInt(input.intervalHours, 4) : undefined,
    waitForIdle: input.mode === 'idle' || input.waitForIdle === true,
    repeatEnabled: input.repeatEnabled === true,
    repeatAmount: input.repeatEnabled === true ? normalizePositiveInt(input.repeatAmount, 1) : undefined,
    repeatUnit: input.repeatEnabled === true ? normalizeRepeatUnit(input.repeatUnit) : undefined,
    repeatKind: input.repeatEnabled === true ? normalizeRepeatKind(input.repeatKind) : undefined,
    repeatEveryMinutes: input.repeatEnabled === true && normalizeRepeatKind(input.repeatKind) === 'fixed'
      ? normalizeEveryMinutes(input.repeatEveryMinutes)
      : undefined,
    repeatWeekDays: input.repeatEnabled === true && normalizeRepeatKind(input.repeatKind) === 'weekly'
      ? normalizeWeekDays(input.repeatWeekDays)
      : undefined,
    repeatEveryWeeks: input.repeatEnabled === true && normalizeRepeatKind(input.repeatKind) === 'weekly'
      ? Math.min(50, Math.max(1, normalizePositiveInt(input.repeatEveryWeeks, 1)))
      : undefined,
    repeatMonthDays: input.repeatEnabled === true && normalizeRepeatKind(input.repeatKind) === 'monthly'
      ? normalizeMonthDays(input.repeatMonthDays)
      : undefined,
    repeatEveryMonths: input.repeatEnabled === true && normalizeRepeatKind(input.repeatKind) === 'monthly'
      ? Math.min(12, Math.max(1, normalizePositiveInt(input.repeatEveryMonths, 1)))
      : undefined,
    endMode: input.repeatEnabled === true ? normalizeEndMode(input.endMode) : undefined,
    endAfterCount: input.repeatEnabled === true && input.endMode === 'after'
      ? normalizePositiveInt(input.endAfterCount, 1)
      : undefined,
    endAt: input.repeatEnabled === true && input.endMode === 'at'
      ? normalizeDateTimeInput(input.endAt, nowMs)
      : undefined,
    createdAt: now,
    updatedAt: now,
    nextRunAt: computeNextRunAt(input, nowMs),
    lastRunAt: null,
    runCount: 0,
    status: 'scheduled',
    error: null,
    createdBy: emberActorHuman(),
    updatedBy: emberActorHuman(),
  };
}

export function rescheduleEmberSchedule(schedule: EmberSchedule, nowMs = Date.now()): EmberSchedule {
  const completedRunCount = schedule.runCount + 1;
  if (!isRepeatingEmberSchedule(schedule)) {
    return {
      ...schedule,
      updatedAt: new Date(nowMs).toISOString(),
      lastRunAt: new Date(nowMs).toISOString(),
      runCount: completedRunCount,
      status: 'sent',
      error: null,
    };
  }
  if (repeatEnded(schedule, completedRunCount, nowMs)) {
    return {
      ...schedule,
      updatedAt: new Date(nowMs).toISOString(),
      lastRunAt: new Date(nowMs).toISOString(),
      runCount: completedRunCount,
      status: 'sent',
      error: null,
    };
  }
  return {
    ...schedule,
    updatedAt: new Date(nowMs).toISOString(),
    lastRunAt: new Date(nowMs).toISOString(),
    runCount: completedRunCount,
    status: 'scheduled',
    nextRunAt: computeNextRunAt(schedule, nowMs + 1000),
    error: null,
  };
}

export function failedEmberSchedule(schedule: EmberSchedule, error: string, nowMs = Date.now()): EmberSchedule {
  return {
    ...schedule,
    updatedAt: new Date(nowMs).toISOString(),
    status: 'failed',
    error,
  };
}

export function createEmberHistoryRecord(
  schedule: EmberSchedule,
  status: EmberHistoryStatus,
  sentAt: string,
  error?: string | null,
  triggeredBy: EmberHistoryTrigger = 'schedule',
): EmberHistoryRecord {
  return {
    id: mintEmberId('history'),
    scheduleId: schedule.id,
    prompt: schedule.text.trim(),
    targetAgentIds: emberScheduleTargetIds(schedule),
    targetAgentNames: emberScheduleTargetNames(schedule),
    sentAt,
    status,
    triggeredBy,
    error: error?.trim() || null,
    scheduledFor: schedule.nextRunAt,
    startedAt: status === 'delivered' ? sentAt : null,
    finishedAt: sentAt,
    reason: error?.trim() || null,
    missedRuns: null,
  };
}

export function resumeEmberSchedule(schedule: EmberSchedule, nowMs = Date.now()): EmberSchedule {
  const nextRunAt = Date.parse(schedule.nextRunAt) <= nowMs
    ? computeNextRunAt(schedule, nowMs)
    : schedule.nextRunAt;
  return {
    ...schedule,
    updatedAt: new Date(nowMs).toISOString(),
    status: 'scheduled',
    nextRunAt,
    error: null,
  };
}

export function dueEmberSchedules(schedules: readonly EmberSchedule[], nowMs = Date.now()): EmberSchedule[] {
  return schedules.filter((schedule) => (
    schedule.status === 'scheduled'
    && Number.isFinite(Date.parse(schedule.nextRunAt))
    && Date.parse(schedule.nextRunAt) <= nowMs
  ));
}

export function isRepeatingEmberSchedule(schedule: EmberSchedule): boolean {
  return schedule.repeatEnabled === true || schedule.mode === 'daily' || schedule.mode === 'interval';
}

export function emberTimeLabel(value: string | null | undefined): string {
  if (!value) return 'Not scheduled';
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  }).format(date);
}

export function emberScheduleSummary(schedule: EmberSchedule): string {
  if (schedule.mode === 'idle') return 'When agents idle';
  if (schedule.mode === 'at') return `At ${emberTimeLabel(schedule.nextRunAt)}`;
  if (schedule.mode === 'daily') return `Daily ${schedule.timeOfDay ?? '09:00'}`;
  if (schedule.mode === 'interval') return `Every ${schedule.intervalHours ?? 4}h`;
  const unit = schedule.delayUnit === 'days' ? 'd' : schedule.delayUnit === 'hours' ? 'h' : 'min';
  return `In ${schedule.delayAmount ?? 10} ${unit}`;
}

export function emberReminderTerminalTiming(
  schedule: EmberSchedule,
  source: 'auto' | 'manual',
): AgentBusTerminalTiming {
  if (source === 'manual') return { trigger: 'manual' };
  if (schedule.mode === 'idle' || schedule.waitForIdle) return { trigger: 'idle' };
  return {
    trigger: 'scheduled',
    scheduledFor: schedule.nextRunAt,
  };
}

export function emberScheduleTargetIds(schedule: EmberSchedule): AgentId[] {
  return normalizeTargetAgentIds(schedule.targetAgentIds, schedule.targetAgentId);
}

export function emberScheduleTargetNames(schedule: EmberSchedule): string[] {
  return normalizeTargetAgentNames(schedule.targetAgentNames, emberScheduleTargetIds(schedule), schedule.targetAgentName);
}

export function emberScheduleTargetLabel(schedule: EmberSchedule): string {
  const names = emberScheduleTargetNames(schedule);
  if (names.length === 0) return schedule.targetAgentName || schedule.targetAgentId;
  if (names.length === 1) return `@${names[0]}`;
  return `@${names[0]} +${names.length - 1}`;
}

export async function loadEmberDreamAgentPromptTemplate(): Promise<string> {
  const result = await readSystemPrompt(
    { path: EMBER_DREAM_AGENT_PROMPT_PATH },
    EMBER_DREAM_AGENT_PROMPT_TEMPLATE,
  );
  return result.content;
}

export async function loadEmberDreamConsolidatePromptTemplate(): Promise<string> {
  const result = await readSystemPrompt(
    { path: EMBER_DREAM_CONSOLIDATE_PROMPT_PATH },
    EMBER_DREAM_CONSOLIDATE_PROMPT_TEMPLATE,
  );
  return result.content;
}

export function renderDreamPrompt(
  agentNames: readonly string[],
  accountDreamsPath: string | null,
  template = EMBER_DREAM_AGENT_PROMPT_TEMPLATE,
): string {
  return template
    .replaceAll('{{dreams_path}}', accountDreamsPath || '$KOTA_HOME/dreams/dreams.md')
    .replaceAll('{{dreaming_agents}}', agentNames.join(', ') || 'current active agents');
}

export async function renderDreamPromptFromFile(
  agentNames: readonly string[],
  accountDreamsPath: string | null,
): Promise<string> {
  return renderDreamPrompt(agentNames, accountDreamsPath, await loadEmberDreamAgentPromptTemplate());
}

function computeNextRunAt(input: EmberScheduleInput | EmberSchedule, nowMs: number): string {
  if (
    'repeatEnabled' in input
    && input.repeatEnabled === true
    && input.mode !== 'daily'
    && input.mode !== 'interval'
  ) {
    const kind = input.repeatKind ?? 'fixed';
    const runCount = 'runCount' in input ? input.runCount : 0;
    // weekly/monthly are calendar-based — even the first run is derived from the kind
    // (using the At-a-time clock). fixed uses atDateTime for the first run, then interval.
    if (kind === 'weekly' || kind === 'monthly' || runCount > 0) {
      return nextRepeatRunAt(input as EmberSchedule, nowMs).toISOString();
    }
  }
  if (input.mode === 'idle') {
    return new Date(nowMs).toISOString();
  }
  if (input.mode === 'at') {
    return new Date(normalizeDateTimeInput(input.atDateTime, nowMs)).toISOString();
  }
  if (input.mode === 'daily') {
    return nextDailyRunAt(normalizeTimeOfDay(input.timeOfDay), nowMs).toISOString();
  }
  if (input.mode === 'interval') {
    const hours = normalizePositiveInt(input.intervalHours, 4);
    return new Date(nowMs + hours * 60 * 60 * 1000).toISOString();
  }
  const amount = normalizePositiveInt(input.delayAmount, 10);
  const unit = normalizeDelayUnit(input.delayUnit);
  const ms = durationMs(amount, unit);
  return new Date(nowMs + ms).toISOString();
}

function nextRepeatRunAt(input: EmberSchedule, nowMs: number): Date {
  const kind = input.repeatKind ?? 'fixed';
  if (kind === 'weekly') return nextWeeklyRunAt(input, nowMs);
  if (kind === 'monthly') return nextMonthlyRunAt(input, nowMs);
  return new Date(nowMs + repeatIntervalMs(input));
}

// Fixed interval: prefer the new total-minutes field; fall back to legacy amount+unit.
function repeatIntervalMs(input: EmberSchedule): number {
  if (typeof input.repeatEveryMinutes === 'number' && input.repeatEveryMinutes > 0) {
    return Math.max(60_000, Math.floor(input.repeatEveryMinutes) * 60_000);
  }
  const amount = normalizePositiveInt(input.repeatAmount, 1);
  const unit = normalizeRepeatUnit(input.repeatUnit);
  return durationMs(amount, unit);
}

// weekly/monthly fire at the "At a time" clock (taken from atDateTime).
function scheduleClock(input: EmberSchedule): { h: number; m: number } {
  const base = input.atDateTime ? new Date(input.atDateTime) : null;
  if (base && Number.isFinite(base.getTime())) return { h: base.getHours(), m: base.getMinutes() };
  return { h: 9, m: 0 };
}

function startOfMondayWeek(d: Date): Date {
  const x = new Date(d);
  x.setHours(0, 0, 0, 0);
  x.setDate(x.getDate() - ((x.getDay() + 6) % 7)); // back to Monday
  return x;
}

function normalizeWeekDays(value: unknown): number[] {
  const out = Array.isArray(value)
    ? Array.from(new Set(value.map((n) => Math.floor(Number(n))).filter((n) => n >= 0 && n <= 6)))
    : [];
  return out.length > 0 ? out.sort((a, b) => a - b) : [1]; // default Monday
}

function nextWeeklyRunAt(input: EmberSchedule, nowMs: number): Date {
  const days = normalizeWeekDays(input.repeatWeekDays);
  const everyN = Math.min(50, Math.max(1, normalizePositiveInt(input.repeatEveryWeeks, 1)));
  const { h, m } = scheduleClock(input);
  const anchorWeek = startOfMondayWeek(input.atDateTime ? new Date(input.atDateTime) : new Date(nowMs));
  for (let i = 0; i <= 7 * everyN + 7; i++) {
    const cand = new Date(nowMs);
    cand.setDate(cand.getDate() + i);
    cand.setHours(h, m, 0, 0);
    if (cand.getTime() <= nowMs) continue;
    if (!days.includes(cand.getDay())) continue;
    if (everyN > 1) {
      const weeks = Math.round((startOfMondayWeek(cand).getTime() - anchorWeek.getTime()) / (7 * 86_400_000));
      if ((((weeks % everyN) + everyN) % everyN) !== 0) continue;
    }
    return cand;
  }
  return new Date(nowMs + 7 * 86_400_000);
}

function normalizeMonthDays(value: unknown): string[] {
  const out = Array.isArray(value)
    ? value.map((v) => String(v)).filter((v) => v === 'last' || (/^\d{1,2}$/.test(v) && +v >= 1 && +v <= 31))
    : [];
  return out.length > 0 ? Array.from(new Set(out)) : ['1'];
}

function nextMonthlyRunAt(input: EmberSchedule, nowMs: number): Date {
  const tokens = normalizeMonthDays(input.repeatMonthDays);
  const everyN = Math.min(12, Math.max(1, normalizePositiveInt(input.repeatEveryMonths, 1)));
  const { h, m } = scheduleClock(input);
  const now = new Date(nowMs);
  const anchorBase = input.atDateTime ? new Date(input.atDateTime) : now;
  const anchorMonth = anchorBase.getFullYear() * 12 + anchorBase.getMonth();
  for (let step = 0; step <= 12 * everyN + 12; step++) {
    const moIndex = now.getMonth() + step;
    const y = now.getFullYear();
    const monthAbs = y * 12 + moIndex;
    if (everyN > 1 && ((((monthAbs - anchorMonth) % everyN) + everyN) % everyN) !== 0) continue;
    const lastDay = new Date(y, moIndex + 1, 0).getDate();
    // Resolve tokens → concrete days. Days a month lacks (e.g. 31 in Feb) clamp to the last
    // day, and the Set dedupes so multiple missing days fall back to the last day only ONCE.
    const dayNums = Array.from(new Set(
      tokens.map((t) => (t === 'last' ? lastDay : Math.min(parseInt(t, 10), lastDay))),
    )).sort((a, b) => a - b);
    for (const day of dayNums) {
      const cand = new Date(y, moIndex, day, h, m, 0, 0);
      if (cand.getTime() > nowMs) return cand;
    }
  }
  return new Date(nowMs + 30 * 86_400_000);
}

function repeatEnded(schedule: EmberSchedule, completedRunCount: number, nowMs: number): boolean {
  if (schedule.endMode === 'after') {
    return completedRunCount >= normalizePositiveInt(schedule.endAfterCount, 1);
  }
  if (schedule.endMode === 'at' && schedule.endAt) {
    const endAt = Date.parse(schedule.endAt);
    return Number.isFinite(endAt) && nowMs >= endAt;
  }
  return false;
}

function nextDailyRunAt(timeOfDay: string, nowMs: number): Date {
  const now = new Date(nowMs);
  const [hourRaw, minuteRaw] = timeOfDay.split(':');
  const hour = Number(hourRaw);
  const minute = Number(minuteRaw);
  const next = new Date(now);
  next.setHours(
    Number.isFinite(hour) ? hour : 9,
    Number.isFinite(minute) ? minute : 0,
    0,
    0,
  );
  if (next.getTime() <= nowMs) next.setDate(next.getDate() + 1);
  return next;
}

function normalizeEmberState(value: unknown): EmberState {
  const candidate = value as Partial<EmberState> | null;
  if (!candidate || typeof candidate !== 'object') return emptyEmberState();
  return {
    schema: typeof candidate.schema === 'string' ? candidate.schema : undefined,
    drafts: Array.isArray(candidate.drafts)
      ? candidate.drafts.flatMap(normalizeDraft).slice(0, 20)
      : [],
    schedules: Array.isArray(candidate.schedules)
      ? candidate.schedules.flatMap(normalizeSchedule).slice(0, 40)
      : [],
    history: Array.isArray(candidate.history)
      ? candidate.history.flatMap(normalizeHistoryRecord).slice(0, 80)
      : [],
    appLastSeenAt: typeof candidate.appLastSeenAt === 'string' ? candidate.appLastSeenAt : null,
  };
}

function normalizeDraft(value: unknown): EmberDraft[] {
  const draft = value as Partial<EmberDraft> | null;
  const text = typeof draft?.text === 'string' ? draft.text.trim() : '';
  if (!draft || !text) return [];
  const now = new Date().toISOString();
  return [{
    id: typeof draft.id === 'string' && draft.id ? draft.id : mintEmberId('draft'),
    text,
    createdAt: typeof draft.createdAt === 'string' ? draft.createdAt : now,
    updatedAt: typeof draft.updatedAt === 'string' ? draft.updatedAt : now,
  }];
}

function normalizeHistoryRecord(value: unknown): EmberHistoryRecord[] {
  const record = value as Partial<EmberHistoryRecord> | null;
  const prompt = typeof record?.prompt === 'string' ? record.prompt.trim() : '';
  const targetAgentIds = normalizeTargetAgentIds(record?.targetAgentIds, undefined);
  if (!record || !prompt || targetAgentIds.length === 0) return [];
  const now = new Date().toISOString();
  const targetAgentNames = normalizeTargetAgentNames(record.targetAgentNames, targetAgentIds, undefined);
  return [{
    id: typeof record.id === 'string' && record.id ? record.id : mintEmberId('history'),
    scheduleId: typeof record.scheduleId === 'string' && record.scheduleId ? record.scheduleId : '',
    prompt,
    targetAgentIds,
    targetAgentNames,
    sentAt: typeof record.sentAt === 'string' ? record.sentAt : now,
    status: record.status === 'failed' ? 'failed' : 'delivered',
    triggeredBy: record.triggeredBy === 'manual' ? 'manual' : 'schedule',
    error: normalizeEmberDeliveryError(record.error),
    scheduledFor: typeof record.scheduledFor === 'string' ? record.scheduledFor : null,
    startedAt: typeof record.startedAt === 'string' ? record.startedAt : null,
    finishedAt: typeof record.finishedAt === 'string' ? record.finishedAt : null,
    reason: normalizeEmberDeliveryError(record.reason),
    missedRuns: normalizeNullablePositiveInt(record.missedRuns),
  }];
}

function normalizeSchedule(value: unknown): EmberSchedule[] {
  const schedule = value as Partial<EmberSchedule> | null;
  const text = typeof schedule?.text === 'string' ? schedule.text.trim() : '';
  const targetAgentIds = normalizeTargetAgentIds(schedule?.targetAgentIds, schedule?.targetAgentId);
  const targetAgentId = targetAgentIds[0] ?? '';
  if (!schedule || !text || !targetAgentId) return [];
  const targetAgentNames = normalizeTargetAgentNames(schedule.targetAgentNames, targetAgentIds, schedule.targetAgentName);
  const now = new Date().toISOString();
  const mode: EmberScheduleMode = schedule.mode === 'daily' || schedule.mode === 'interval'
    || schedule.mode === 'idle' || schedule.mode === 'at'
    ? schedule.mode
    : 'delay';
  const status: EmberScheduleStatus = (
    schedule.status === 'paused'
    || schedule.status === 'sent'
    || schedule.status === 'failed'
  ) ? schedule.status : 'scheduled';
  return [{
    id: typeof schedule.id === 'string' && schedule.id ? schedule.id : mintEmberId('schedule'),
    text,
    targetAgentId,
    targetAgentName: targetAgentNames[0] ?? targetAgentId,
    targetAgentIds,
    targetAgentNames,
    mode,
    delayAmount: mode === 'delay' ? normalizePositiveInt(schedule.delayAmount, 10) : undefined,
    delayUnit: mode === 'delay' ? normalizeDelayUnit(schedule.delayUnit) : undefined,
    atDateTime: mode === 'at' ? normalizeDateTimeInput(schedule.atDateTime, Date.now()) : undefined,
    timeOfDay: mode === 'daily' ? normalizeTimeOfDay(schedule.timeOfDay) : undefined,
    intervalHours: mode === 'interval' ? normalizePositiveInt(schedule.intervalHours, 4) : undefined,
    waitForIdle: schedule.waitForIdle === true || mode === 'idle',
    repeatEnabled: schedule.repeatEnabled === true,
    repeatAmount: schedule.repeatEnabled === true ? normalizePositiveInt(schedule.repeatAmount, 1) : undefined,
    repeatUnit: schedule.repeatEnabled === true ? normalizeRepeatUnit(schedule.repeatUnit) : undefined,
    repeatKind: schedule.repeatEnabled === true ? normalizeRepeatKind(schedule.repeatKind) : undefined,
    repeatEveryMinutes: schedule.repeatEnabled === true && normalizeRepeatKind(schedule.repeatKind) === 'fixed'
      ? normalizeEveryMinutes(
          // back-compat: legacy schedules only had repeatAmount + repeatUnit
          schedule.repeatEveryMinutes
            ?? (durationMs(normalizePositiveInt(schedule.repeatAmount, 1), normalizeRepeatUnit(schedule.repeatUnit)) / 60000),
        )
      : undefined,
    repeatWeekDays: schedule.repeatEnabled === true && normalizeRepeatKind(schedule.repeatKind) === 'weekly'
      ? normalizeWeekDays(schedule.repeatWeekDays)
      : undefined,
    repeatEveryWeeks: schedule.repeatEnabled === true && normalizeRepeatKind(schedule.repeatKind) === 'weekly'
      ? Math.min(50, Math.max(1, normalizePositiveInt(schedule.repeatEveryWeeks, 1)))
      : undefined,
    repeatMonthDays: schedule.repeatEnabled === true && normalizeRepeatKind(schedule.repeatKind) === 'monthly'
      ? normalizeMonthDays(schedule.repeatMonthDays)
      : undefined,
    repeatEveryMonths: schedule.repeatEnabled === true && normalizeRepeatKind(schedule.repeatKind) === 'monthly'
      ? Math.min(12, Math.max(1, normalizePositiveInt(schedule.repeatEveryMonths, 1)))
      : undefined,
    endMode: schedule.repeatEnabled === true ? normalizeEndMode(schedule.endMode) : undefined,
    endAfterCount: schedule.repeatEnabled === true && schedule.endMode === 'after'
      ? normalizePositiveInt(schedule.endAfterCount, 1)
      : undefined,
    endAt: schedule.repeatEnabled === true && schedule.endMode === 'at'
      ? normalizeDateTimeInput(schedule.endAt, Date.now())
      : undefined,
    createdAt: typeof schedule.createdAt === 'string' ? schedule.createdAt : now,
    updatedAt: typeof schedule.updatedAt === 'string' ? schedule.updatedAt : now,
    nextRunAt: typeof schedule.nextRunAt === 'string' ? schedule.nextRunAt : now,
    lastRunAt: typeof schedule.lastRunAt === 'string' ? schedule.lastRunAt : null,
    runCount: normalizeNonNegativeInt(schedule.runCount, 0),
    status,
    error: normalizeEmberDeliveryError(schedule.error),
    createdBy: normalizeActorRef(schedule.createdBy) ?? emberActorHuman(),
    updatedBy: normalizeActorRef(schedule.updatedBy) ?? normalizeActorRef(schedule.createdBy) ?? emberActorHuman(),
  }];
}

function emptyEmberState(): EmberState {
  return { drafts: [], schedules: [], history: [], appLastSeenAt: null };
}

function normalizeEmberDeliveryError(value: unknown): string | null {
  if (typeof value !== 'string' || !value.trim()) return null;
  const error = value.trim();
  return error === 'app not running' ? EMBER_NOT_DELIVERED : error;
}

export function emberActorHuman(): EmberActorRef {
  return { kind: 'human', label: 'Human' };
}

export function emberActorAgent(): EmberActorRef {
  return { kind: 'agent', label: 'Agent' };
}

function normalizeActorRef(value: unknown): EmberActorRef | null {
  const candidate = value as Partial<EmberActorRef> | null;
  if (!candidate || typeof candidate !== 'object') return null;
  const kind = candidate.kind === 'agent' ? 'agent' : candidate.kind === 'human' ? 'human' : null;
  if (!kind) return null;
  return {
    kind,
    label: typeof candidate.label === 'string' && candidate.label.trim()
      ? candidate.label.trim()
      : kind === 'agent' ? 'Agent' : 'Human',
  };
}

function normalizeTargetAgentIds(value: unknown, fallback: unknown): AgentId[] {
  const out: AgentId[] = [];
  const add = (candidate: unknown) => {
    if (typeof candidate !== 'string') return;
    const id = candidate.trim() as AgentId;
    if (!id || out.includes(id)) return;
    out.push(id);
  };
  if (Array.isArray(value)) value.forEach(add);
  add(fallback);
  return out.slice(0, 12);
}

function normalizeTargetAgentNames(value: unknown, targetAgentIds: readonly AgentId[], fallback: unknown): string[] {
  const raw = Array.isArray(value)
    ? value
    : (typeof fallback === 'string' ? [fallback] : []);
  return targetAgentIds.map((agentId, index) => {
    const name = raw[index];
    return typeof name === 'string' && name.trim() ? name.trim() : agentId;
  });
}

function normalizePositiveInt(value: unknown, fallback: number): number {
  const parsed = Math.floor(Number(value));
  return Number.isFinite(parsed) && parsed > 0 ? Math.min(parsed, 999) : fallback;
}

function normalizeRepeatKind(value: unknown): EmberRepeatKind {
  if (value === 'weekly') return 'weekly';
  if (value === 'monthly') return 'monthly';
  return 'fixed';
}

// Fixed-interval total minutes — own bound (<= 1 year), independent of the global 999 cap.
function normalizeEveryMinutes(value: unknown): number {
  const n = Math.floor(Number(value));
  return Number.isFinite(n) && n > 0 ? Math.min(n, 525_600) : 1440; // default 1 day
}

function normalizeDelayUnit(value: unknown): EmberDelayUnit {
  if (value === 'days') return 'days';
  if (value === 'hours') return 'hours';
  return 'minutes';
}

function normalizeRepeatUnit(value: unknown): EmberRepeatUnit {
  if (value === 'days') return 'days';
  if (value === 'minutes') return 'minutes';
  return 'hours';
}

function normalizeEndMode(value: unknown): EmberEndMode {
  if (value === 'after') return 'after';
  if (value === 'at') return 'at';
  return 'never';
}

function durationMs(amount: number, unit: EmberDelayUnit | EmberRepeatUnit): number {
  if (unit === 'days') return amount * 24 * 60 * 60 * 1000;
  if (unit === 'hours') return amount * 60 * 60 * 1000;
  return amount * 60 * 1000;
}

function normalizeDateTimeInput(value: unknown, nowMs: number): string {
  const raw = typeof value === 'string' ? value.trim() : '';
  const parsed = raw ? Date.parse(raw) : NaN;
  if (Number.isFinite(parsed)) return new Date(parsed).toISOString();
  return new Date(nowMs).toISOString();
}

function normalizeNonNegativeInt(value: unknown, fallback: number): number {
  const parsed = Math.floor(Number(value));
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : fallback;
}

function normalizeNullablePositiveInt(value: unknown): number | null {
  const parsed = Math.floor(Number(value));
  return Number.isFinite(parsed) && parsed > 0 ? parsed : null;
}

function normalizeTimeOfDay(value: unknown): string {
  const raw = typeof value === 'string' ? value : '';
  const match = /^(\d{1,2}):(\d{2})$/.exec(raw);
  if (!match) return '09:00';
  const hour = Math.min(23, Math.max(0, Number(match[1])));
  const minute = Math.min(59, Math.max(0, Number(match[2])));
  return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;
}

function mintEmberId(prefix: string): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return `${prefix}-${crypto.randomUUID()}`;
  }
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}
