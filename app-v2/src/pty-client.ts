import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type Event, type UnlistenFn } from '@tauri-apps/api/event';
import {
  INITIAL_SCROLLBACK,
  mockRunCommand,
  mockTranslateNL,
} from './mock/smart-terminal';
import type {
  FallbackCli,
  MagiProvider,
  SmartExitEvent,
  SmartOutputEvent,
  SmartPtySummary,
  SmartStatusEvent,
  SmartTuiExitEvent,
  TranslateResult,
} from './types/smart-terminal';
import {
  agentExitTopic,
  agentOutputTopic,
  agentStatusTopic,
  agentWorkTopic,
  linesToGridSnapshot,
  type AgentExitEvent,
  type AgentOutputEvent,
  type AgentRoute,
  type AgentSessionLeaseConflict,
  type AgentSpawnRequest,
  type AgentStatusEvent,
  type AgentSummary,
  type AgentWorkStateEvent,
} from './types/agent-pty';
import type {
  WorkspaceDiffChangeEntry,
  WorkspaceDiffScope,
  WorkspaceFileDiffRequest,
  WorkspaceFileDiffResult,
  WorkspaceTreeListing,
  WorkspaceTreePathRequest,
  WorkspaceTreeRootKind,
} from './types/tree';
import type { VioletSummaryConfig } from './violet-summary-config';

type SmartShellCli = 'bash' | 'zsh';
type OutputListener = (payload: SmartOutputEvent) => void;
type ExitListener = (payload: SmartExitEvent) => void;
type StatusListener = (payload: SmartStatusEvent) => void;
type TuiExitListener = (payload: SmartTuiExitEvent) => void;
type AgentWorkListener = (payload: AgentWorkStateEvent) => void;

const SMART_OUTPUT_EVENT = 'pty://smart/output';
const SMART_EXIT_EVENT = 'pty://smart/exit';
const SMART_STATUS_EVENT = 'pty://smart/status';
const SMART_TUI_EXIT_EVENT = 'pty://smart/tui-exit';
const VIOLET_ROOM_CHANGED_EVENT = 'violet://room/changed';
const VIOLET_ROOM_SYNCED_EVENT = 'violet://room/synced';
const EMBER_SCHEDULES_CHANGED_EVENT = 'ember-schedules-changed';
const LM_STANDBY_DEPLOY_EVENT = 'lm-standby-deploy';
const INCARNATION_PROGRESS_EVENT = 'kota://incarnation-progress';
const BARTENDER_SYNC_EVENT = 'bartender-sync-local';
const BARTENDER_SYNC_PROGRESS_EVENT = 'bartender-sync-progress';
const MOCK_TERMINAL_ENHANCEMENT_KEY = 'kota-v2.ghostty-terminal-enhancement';

interface SpawnSmartPtyOptions {
  cwd?: string;
  cli?: SmartShellCli;
}

interface MockPtyState {
  cwd: string;
  running: boolean;
  activeCli: FallbackCli | null;
  /** Per-pty cumulative line history. Browser-dev mock only — converted
   *  to a GridSnapshot when emitting (real PTY uses pty/ansi.rs). */
  history: import('./types/smart-terminal').ScrollbackLine[];
  /** Pending readline buffer — fakes zsh/bash line editing for the
   *  browser-dev mock. The real PTY (portable-pty + alacritty) handles
   *  this in the actual shell process; here we accumulate chars
   *  written via writeSmartPty until a CR/LF arrives, at which point
   *  the line is "run" via mockRunCommand. \x15 (Ctrl-U) and \x0b
   *  (Ctrl-K) clear the buffer, and \x7f (Backspace) pops the last char so SmartTerminal's
   *  mirror passthrough sends keystroke-by-keystroke without producing
   *  one mock prompt per character. */
  lineBuffer: string;
}

interface MockState {
  defaultPtyId: string | null;
  nextPtyId: number;
  ptys: Map<string, MockPtyState>;
  outputListeners: Set<OutputListener>;
  exitListeners: Set<ExitListener>;
  statusListeners: Set<StatusListener>;
}

const agentWorkListeners = new Map<string, Set<AgentWorkListener>>();

export interface TerminalEnhancementStatus {
  ghosttyTerminalEnhancementEnabled: boolean;
  settingsPath: string;
  engine: string;
  detail: string;
}

const mockState: MockState = {
  defaultPtyId: null,
  nextPtyId: 1,
  ptys: new Map(),
  outputListeners: new Set(),
  exitListeners: new Set(),
  statusListeners: new Set(),
};

let defaultSmartPtyId: string | null = null;

function useTauriRuntime(): boolean {
  return typeof window !== 'undefined' && isTauri();
}

export function hasTauriRuntime(): boolean {
  return useTauriRuntime();
}

function normalizeMaybePtyArgs<T>(
  ptyIdOrValue: string,
  maybeValue: T | undefined,
): { ptyId: string | null; value: T } {
  if (maybeValue === undefined) {
    return { ptyId: null, value: ptyIdOrValue as T };
  }

  return { ptyId: ptyIdOrValue, value: maybeValue };
}

function normalizeMaybePtyTuple(
  ptyIdOrCols: string | number,
  maybeCols: number | undefined,
  maybeRows: number | undefined,
): { ptyId: string | null; cols: number; rows: number } {
  if (typeof ptyIdOrCols === 'number') {
    return {
      ptyId: null,
      cols: ptyIdOrCols,
      rows: maybeCols ?? 0,
    };
  }

  return {
    ptyId: ptyIdOrCols,
    cols: maybeCols ?? 0,
    rows: maybeRows ?? 0,
  };
}

async function ensureDefaultSmartPty(): Promise<string> {
  if (defaultSmartPtyId) {
    return defaultSmartPtyId;
  }

  if (useTauriRuntime()) {
    defaultSmartPtyId = await invoke<string>('pty_smart_init');
    return defaultSmartPtyId;
  }

  defaultSmartPtyId = ensureMockDefaultPty();
  return defaultSmartPtyId;
}

async function resolveTargetPtyId(explicitPtyId: string | null): Promise<string> {
  if (explicitPtyId) {
    return explicitPtyId;
  }

  return ensureDefaultSmartPty();
}

function emitMockOutput(payload: SmartOutputEvent) {
  for (const listener of mockState.outputListeners) listener(payload);
}

/** Append `newLines` to the mock pty's history (or reset it), then emit
 *  a GridSnapshot derived from the running history. Mirrors the real
 *  pty/ansi.rs grid-emit shape so SmartTerminal sees one shape end-to-end. */
function emitMockGrid(
  ptyId: string,
  newLines: import('./types/smart-terminal').ScrollbackLine[],
  reset: boolean,
): void {
  const state = mockState.ptys.get(ptyId);
  if (!state) return;
  if (reset) state.history = [...newLines];
  else state.history = [...state.history, ...newLines];
  // Cap history so old fixtures don't accumulate forever.
  if (state.history.length > 1000) {
    state.history.splice(0, state.history.length - 1000);
  }
  emitMockOutput({ ptyId, snapshot: linesToGridSnapshot(state.history) });
}

function emitMockExit(payload: SmartExitEvent) {
  for (const listener of mockState.exitListeners) listener(payload);
}

function emitMockStatus(ptyId: string) {
  const state = mockState.ptys.get(ptyId);
  if (!state) {
    return;
  }

  const payload: SmartStatusEvent = {
    ptyId,
    running: state.running,
    cwd: state.cwd,
  };
  for (const listener of mockState.statusListeners) listener(payload);
}

function addMockListener<T>(
  set: Set<(payload: T) => void>,
  listener: (payload: T) => void,
): UnlistenFn {
  set.add(listener);
  return async () => {
    set.delete(listener);
  };
}

function addAgentWorkListener(agentId: string, listener: AgentWorkListener): UnlistenFn {
  let listeners = agentWorkListeners.get(agentId);
  if (!listeners) {
    listeners = new Set();
    agentWorkListeners.set(agentId, listeners);
  }
  listeners.add(listener);
  return async () => {
    const current = agentWorkListeners.get(agentId);
    if (!current) return;
    current.delete(listener);
    if (current.size === 0) agentWorkListeners.delete(agentId);
  };
}

function emitLocalAgentWorkState(payload: AgentWorkStateEvent): void {
  const listeners = agentWorkListeners.get(payload.agentId);
  if (!listeners) return;
  for (const listener of listeners) listener(payload);
}

function emitVioletWorkEvents(state: VioletRoomState): void {
  const seen = new Set<string>();
  const events = [...(state.workEvents ?? [])].sort((a, b) => {
    const at = Date.parse(a.timestamp);
    const bt = Date.parse(b.timestamp);
    return (Number.isFinite(at) ? at : 0) - (Number.isFinite(bt) ? bt : 0);
  });
  for (const event of events) {
    const key = [
      event.agentId,
      event.sessionId ?? '',
      event.nativeEventId ?? '',
      event.turnId ?? '',
      event.state,
      event.timestamp,
    ].join('|');
    if (seen.has(key)) continue;
    seen.add(key);
    emitLocalAgentWorkState(event);
  }
}

function emitVioletRoomSynced(request: VioletRoomRequest, state: VioletRoomState): void {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new CustomEvent<VioletRoomSyncedEvent>(VIOLET_ROOM_SYNCED_EVENT, {
    detail: { request, state },
  }));
}

function createMockPty(opts: SpawnSmartPtyOptions = {}): string {
  const ptyId = `mock-pty-${mockState.nextPtyId++}`;
  const cwd = sanitizeMockCwd(opts.cwd);
  mockState.ptys.set(ptyId, {
    cwd,
    running: true,
    activeCli: null,
    history: [],
    lineBuffer: '',
  });
  if (!mockState.defaultPtyId) {
    mockState.defaultPtyId = ptyId;
  }

  emitMockStatus(ptyId);
  emitMockGrid(ptyId, INITIAL_SCROLLBACK, /*reset=*/ true);
  return ptyId;
}

function ensureMockDefaultPty(): string {
  const existing = mockState.defaultPtyId;
  if (existing && mockState.ptys.has(existing)) {
    emitMockStatus(existing);
    return existing;
  }

  return createMockPty();
}

function sanitizeMockCwd(cwd?: string): string {
  if (!cwd || cwd.trim().length === 0) {
    return '~';
  }

  if (cwd === '/') {
    return '~';
  }

  if (cwd.startsWith('/Users/')) {
    return cwd.replace(/^\/Users\/[^/]+/, '~');
  }

  return cwd;
}

export async function spawnSmartPty(opts: SpawnSmartPtyOptions = {}): Promise<string> {
  if (useTauriRuntime()) {
    const ptyId = await invoke<string>('pty_smart_spawn', {
      cwd: opts.cwd,
      cli: opts.cli,
    });
    if (!defaultSmartPtyId) {
      defaultSmartPtyId = ptyId;
    }
    return ptyId;
  }

  const ptyId = createMockPty(opts);
  if (!defaultSmartPtyId) {
    defaultSmartPtyId = ptyId;
  }
  return ptyId;
}

export async function closeSmartPty(ptyId: string): Promise<void> {
  if (useTauriRuntime()) {
    await invoke('pty_smart_close', { ptyId });
  } else {
    const state = mockState.ptys.get(ptyId);
    if (!state) {
      return;
    }
    state.running = false;
    state.activeCli = null;
    emitMockExit({ ptyId, code: null });
    emitMockStatus(ptyId);
    mockState.ptys.delete(ptyId);
    if (mockState.defaultPtyId === ptyId) {
      mockState.defaultPtyId = null;
    }
  }

  if (defaultSmartPtyId === ptyId) {
    defaultSmartPtyId = null;
  }
}

export async function listSmartPtys(): Promise<SmartPtySummary[]> {
  if (useTauriRuntime()) {
    return invoke<SmartPtySummary[]>('pty_smart_list');
  }

  return Array.from(mockState.ptys.entries()).map(([ptyId, state]) => ({
    ptyId,
    cwd: state.cwd,
    running: state.running,
  }));
}

export async function initSmartPty(): Promise<string> {
  return ensureDefaultSmartPty();
}

export async function writeSmartPty(input: string): Promise<void>;
export async function writeSmartPty(ptyId: string, input: string): Promise<void>;
export async function writeSmartPty(
  ptyIdOrInput: string,
  maybeInput?: string,
): Promise<void> {
  const { ptyId, value: input } = normalizeMaybePtyArgs(ptyIdOrInput, maybeInput);

  if (useTauriRuntime()) {
    const resolvedPtyId = await resolveTargetPtyId(ptyId);
    await invoke('pty_smart_write', { ptyId: resolvedPtyId, input });
    return;
  }

  const resolvedPtyId = ptyId ?? ensureMockDefaultPty();
  const state = mockState.ptys.get(resolvedPtyId);
  if (!state) {
    return;
  }

  // Fake-readline buffer: SmartTerminal.tsx mirrors keystroke-by-keystroke
  // to PTY now (so zsh's readline echoes through the grid in real time).
  // The browser-dev mock has no real shell — accumulate chars here and
  // only "run" the line when CR/LF arrives. Special control bytes:
  //   \x15  Ctrl-U → clear buffer before cursor
  //   \x0b  Ctrl-K → clear buffer after cursor
  //   \x7f  Backspace → pop last char
  //   \x1b…  Arrow keys / nav escape sequences → ignored (no history)
  //   \t    Tab → ignored (no completion in mock)
  // Everything else (printable, single chars) accumulates into the buffer.
  let runs: string[] = [];
  let i = 0;
  while (i < input.length) {
    const ch = input.charCodeAt(i);
    if (ch === 0x0d || ch === 0x0a /* CR / LF */) {
      runs.push(state.lineBuffer);
      state.lineBuffer = '';
      i++;
      continue;
    }
    if (ch === 0x15 /* Ctrl-U */ || ch === 0x0b /* Ctrl-K */) {
      state.lineBuffer = '';
      i++;
      continue;
    }
    if (ch === 0x7f /* Backspace / DEL */) {
      state.lineBuffer = state.lineBuffer.slice(0, -1);
      i++;
      continue;
    }
    if (ch === 0x1b /* ESC — start of escape sequence */) {
      // Skip ESC + [ + (digits / letter terminator) for navigation keys.
      // We don't simulate history / cursor movement in the mock.
      i++;
      if (input[i] === '[') {
        i++;
        while (i < input.length && /[0-9;]/.test(input[i] ?? '')) i++;
        if (i < input.length) i++; // skip terminator
      }
      continue;
    }
    if (ch === 0x09 /* Tab */) {
      // No completion in mock — drop the tab.
      i++;
      continue;
    }
    if (ch === 0x03 /* Ctrl-C */) {
      state.lineBuffer = '';
      i++;
      continue;
    }
    state.lineBuffer += input[i] ?? '';
    i++;
  }

  // No CR seen yet — buffer-only update. Don't emit any output (real PTY
  // would echo via readline; the mock leaves the local <input> field as
  // the visible representation).
  if (runs.length === 0) {
    return;
  }

  for (const cmd of runs) {
    const trimmed = cmd.trim();

    if (trimmed === 'clear' || trimmed === 'reset') {
      emitMockGrid(resolvedPtyId, [], /*reset=*/ true);
      continue;
    }

    const cwdBefore = state.cwd;
    const nextCwd = resolveMockCwd(cwdBefore, trimmed);
    const isLaunchingCli = isFallbackCli(trimmed);
    const isLeavingCli = trimmed === 'exit' && state.activeCli !== null;

    let lines = mockRunCommand(cwdBefore, cmd);
    if (isLeavingCli) {
      lines = [
        { kind: 'prompt', text: `${cwdBefore} › ${cmd}` },
        { kind: 'dim', text: `${state.activeCli} session ended` },
      ];
    }

    if (lines.length > 0) {
      emitMockGrid(resolvedPtyId, lines, /*reset=*/ false);
    }

    if (nextCwd) {
      state.cwd = nextCwd;
      emitMockStatus(resolvedPtyId);
    }

    if (isLaunchingCli) {
      state.activeCli = trimmed as FallbackCli;
    } else if (isLeavingCli) {
      state.activeCli = null;
      emitMockStatus(resolvedPtyId);
    } else if (trimmed === 'exit') {
      state.running = false;
      state.activeCli = null;
      emitMockExit({ ptyId: resolvedPtyId, code: 0 });
      emitMockStatus(resolvedPtyId);
    }
  }
}

export async function resizeSmartPty(cols: number, rows: number): Promise<void>;
export async function resizeSmartPty(ptyId: string, cols: number, rows: number): Promise<void>;
export async function resizeSmartPty(
  ptyIdOrCols: string | number,
  maybeCols?: number,
  maybeRows?: number,
): Promise<void> {
  const { ptyId, cols, rows } = normalizeMaybePtyTuple(ptyIdOrCols, maybeCols, maybeRows);
  const resolvedPtyId = await resolveTargetPtyId(ptyId);

  if (useTauriRuntime()) {
    await invoke('pty_smart_resize', {
      ptyId: resolvedPtyId,
      cols,
      rows,
    });
  }
}

export async function scrollSmartPty(ptyId: string, lines: number): Promise<void> {
  if (useTauriRuntime()) {
    await invoke('pty_smart_scroll', { ptyId, lines });
  }
}

export async function interruptSmartPty(): Promise<void>;
export async function interruptSmartPty(ptyId: string): Promise<void>;
export async function interruptSmartPty(ptyId?: string): Promise<void> {
  const resolvedPtyId = await resolveTargetPtyId(ptyId ?? null);

  if (useTauriRuntime()) {
    await invoke('pty_smart_interrupt', { ptyId: resolvedPtyId });
    return;
  }

  const state = mockState.ptys.get(resolvedPtyId);
  if (!state) {
    return;
  }

  state.activeCli = null;
  emitMockStatus(resolvedPtyId);
}

export async function clearSmartPty(): Promise<void>;
export async function clearSmartPty(ptyId: string): Promise<void>;
export async function clearSmartPty(ptyId?: string): Promise<void> {
  const resolvedPtyId = await resolveTargetPtyId(ptyId ?? null);

  if (useTauriRuntime()) {
    await invoke('pty_smart_clear', { ptyId: resolvedPtyId });
    return;
  }

  if (!mockState.ptys.has(resolvedPtyId)) {
    return;
  }

  emitMockGrid(resolvedPtyId, [], /*reset=*/ true);
}

export async function restartSmartPty(): Promise<void>;
export async function restartSmartPty(ptyId: string): Promise<void>;
export async function restartSmartPty(ptyId?: string): Promise<void> {
  const resolvedPtyId = await resolveTargetPtyId(ptyId ?? null);

  if (useTauriRuntime()) {
    await invoke('pty_smart_restart', { ptyId: resolvedPtyId });
    return;
  }

  const state = mockState.ptys.get(resolvedPtyId);
  if (!state) {
    return;
  }

  state.cwd = '~';
  state.running = true;
  state.activeCli = null;
  emitMockGrid(resolvedPtyId, [], /*reset=*/ true);
  emitMockStatus(resolvedPtyId);
  emitMockGrid(resolvedPtyId, INITIAL_SCROLLBACK, /*reset=*/ true);
}

/** Resolve a CLI program name (e.g. "claude") to its absolute path on
 *  Kota's augmented PATH. Used by the smart-shell #ask handoff so the
 *  spawn command doesn't depend on whatever the user's interactive
 *  ~/.zshrc left in $PATH after conda / nvm / pyenv hooks ran. Falls
 *  back to the input string in non-Tauri contexts and when nothing
 *  matches on the augmented PATH (the shell will then surface its own
 *  "command not found" so the user knows). */
export async function resolveCli(name: string): Promise<string> {
  if (useTauriRuntime()) {
    return invoke<string>('pty_resolve_cli', { name });
  }
  return name;
}

export async function translateNlPrompt(
  ask: string,
  provider: MagiProvider = 'claude',
): Promise<TranslateResult> {
  if (useTauriRuntime()) {
    return invoke<TranslateResult>('pty_nl_translate', { ask, provider });
  }

  return mockTranslateNL(ask, provider);
}

export async function onSmartOutput(listener: OutputListener): Promise<UnlistenFn> {
  if (useTauriRuntime()) {
    return listen<SmartOutputEvent>(SMART_OUTPUT_EVENT, (event: Event<SmartOutputEvent>) => {
      listener(event.payload);
    });
  }

  return addMockListener(mockState.outputListeners, listener);
}

export async function onSmartExit(listener: ExitListener): Promise<UnlistenFn> {
  if (useTauriRuntime()) {
    return listen<SmartExitEvent>(SMART_EXIT_EVENT, (event: Event<SmartExitEvent>) => {
      listener(event.payload);
    });
  }

  return addMockListener(mockState.exitListeners, listener);
}

export async function onSmartStatus(listener: StatusListener): Promise<UnlistenFn> {
  if (useTauriRuntime()) {
    return listen<SmartStatusEvent>(SMART_STATUS_EVENT, (event: Event<SmartStatusEvent>) => {
      listener(event.payload);
    });
  }

  return addMockListener(mockState.statusListeners, listener);
}

/** Fires when a subprocess (claude / codex / ssh / …) yielded the
 *  controlling tty back to the shell. The browser-dev mock never fires
 *  this — agent CLIs only run under Tauri. */
export async function onSmartTuiExit(listener: TuiExitListener): Promise<UnlistenFn> {
  if (useTauriRuntime()) {
    return listen<SmartTuiExitEvent>(
      SMART_TUI_EXIT_EVENT,
      (event: Event<SmartTuiExitEvent>) => {
        listener(event.payload);
      },
    );
  }
  return async () => {};
}

function isFallbackCli(value: string): value is FallbackCli {
  return /^(claude|codex|agy|opencode|pi|kimi)$/.test(value);
}

function resolveMockCwd(cwd: string, input: string): string | null {
  const parts = input.split(/\s+/).filter(Boolean);
  if (parts[0] !== 'cd' || parts.length > 2) {
    return null;
  }

  const target = parts[1] ?? '~';
  if (target === '-') {
    return null;
  }

  if (target === '~') {
    return '~';
  }

  if (target.startsWith('/')) {
    return normalizePath(target);
  }

  if (target.startsWith('~/')) {
    return normalizePath(`/${target.slice(2)}`).replace(/^\//, '~/');
  }

  const cwdPath = cwd === '~' ? '/' : cwd.replace(/^~\//, '/');
  const next = normalizePath(`${cwdPath}/${target}`);
  return next === '/' ? '~' : next.replace(/^\//, '~/');
}

function normalizePath(path: string): string {
  const stack: string[] = [];
  for (const segment of path.split('/')) {
    if (!segment || segment === '.') continue;
    if (segment === '..') {
      stack.pop();
      continue;
    }
    stack.push(segment);
  }
  return `/${stack.join('/')}`;
}

export function __resetMockSmartPtyForTests() {
  defaultSmartPtyId = null;
  mockState.defaultPtyId = null;
  mockState.nextPtyId = 1;
  mockState.ptys.clear();
  mockState.outputListeners.clear();
  mockState.exitListeners.clear();
  mockState.statusListeners.clear();
  agentWorkListeners.clear();
  resetMockStorageMeasurement();
}

export async function terminalEnhancementStatus(): Promise<TerminalEnhancementStatus> {
  if (useTauriRuntime()) {
    return invoke<TerminalEnhancementStatus>('terminal_enhancement_status');
  }
  let enabled = false;
  try {
    enabled = window.localStorage.getItem(MOCK_TERMINAL_ENHANCEMENT_KEY) === 'true';
  } catch {
    enabled = false;
  }
  return {
    ghosttyTerminalEnhancementEnabled: enabled,
    settingsPath: 'localStorage:kota-v2.ghostty-terminal-enhancement',
    engine: 'kota-grid',
    detail: 'Browser-dev mock. PTY and terminal state remain Kota native.',
  };
}

export async function saveTerminalEnhancement(
  ghosttyTerminalEnhancementEnabled: boolean,
): Promise<TerminalEnhancementStatus> {
  if (useTauriRuntime()) {
    return invoke<TerminalEnhancementStatus>('terminal_enhancement_save', {
      request: { ghosttyTerminalEnhancementEnabled },
    });
  }
  try {
    window.localStorage.setItem(
      MOCK_TERMINAL_ENHANCEMENT_KEY,
      String(ghosttyTerminalEnhancementEnabled),
    );
  } catch {
    // Best-effort in browser-dev mode.
  }
  return terminalEnhancementStatus();
}

// ─────────────────────────────────────────── Agent PTY (M6.A) ───
//
// Per-agent CLI session (CC / Codex / Antigravity / OpenCode).
// Mirrors the Smart Terminal API but keyed by `agentId` (not pty_id) and
// uses per-agent Tauri event topics. Browser dev runtime: no-op stubs
// (agents are Tauri-only).

type AgentOutputListener = (payload: AgentOutputEvent) => void;
type AgentExitListener = (payload: AgentExitEvent) => void;
type AgentStatusListener = (payload: AgentStatusEvent) => void;
const AGENT_SESSION_LEASE_ERROR_PREFIX = 'KOTA_AGENT_SESSION_LEASE_CONFLICT:';

export class AgentSessionLeaseConflictError extends Error {
  readonly conflict: AgentSessionLeaseConflict;

  constructor(conflict: AgentSessionLeaseConflict) {
    super('Agent session is running in another window');
    this.name = 'AgentSessionLeaseConflictError';
    this.conflict = conflict;
  }
}

export function isAgentSessionLeaseConflictError(
  error: unknown,
): error is AgentSessionLeaseConflictError {
  return error instanceof AgentSessionLeaseConflictError;
}

function parseAgentSessionLeaseConflict(error: unknown): AgentSessionLeaseConflict | null {
  const message = typeof error === 'string'
    ? error
    : error instanceof Error
      ? error.message
      : String(error ?? '');
  const index = message.indexOf(AGENT_SESSION_LEASE_ERROR_PREFIX);
  if (index < 0) return null;
  const json = message.slice(index + AGENT_SESSION_LEASE_ERROR_PREFIX.length);
  try {
    const parsed = JSON.parse(json) as Partial<AgentSessionLeaseConflict>;
    if (parsed.code !== 'agent-session-lease-conflict' || typeof parsed.agentId !== 'string') {
      return null;
    }
    return {
      code: 'agent-session-lease-conflict',
      agentId: parsed.agentId,
      ownerPid: typeof parsed.ownerPid === 'number' ? parsed.ownerPid : 0,
      childPid: typeof parsed.childPid === 'number' ? parsed.childPid : null,
    };
  } catch {
    return null;
  }
}

export async function spawnAgentPty(req: AgentSpawnRequest): Promise<AgentRoute> {
  if (!useTauriRuntime()) {
    // Browser dev mode: agents not supported. Return a stub route so callers
    // can subscribe without crashing; events will never fire.
    return {
      agentId: req.agentId,
      outputEvent: agentOutputTopic(req.agentId),
      exitEvent: agentExitTopic(req.agentId),
      statusEvent: agentStatusTopic(req.agentId),
      workEvent: agentWorkTopic(req.agentId),
    };
  }

  try {
    return await invoke<AgentRoute>('pty_agent_spawn', { request: req });
  } catch (err) {
    const conflict = parseAgentSessionLeaseConflict(err);
    if (conflict) throw new AgentSessionLeaseConflictError(conflict);
    throw err;
  }
}

export async function resolveDevProjectRoot(
  candidate: string,
  agentId: string,
  cli: AgentSpawnRequest['cli'],
): Promise<string> {
  if (!useTauriRuntime()) {
    const trimmed = candidate.trim();
    if (!trimmed) {
      throw new Error("set localStorage['kota-v2.dev.project-root'] to the repo root");
    }
    return trimmed;
  }
  return invoke<string>('dev_resolve_project_root', {
    candidate: candidate || null,
    agentId,
    cli,
  });
}

export async function resolveWorkspaceAgentLaunch(
  agentId: string,
  cli: AgentSpawnRequest['cli'],
): Promise<AgentSpawnRequest> {
  if (!useTauriRuntime()) {
    throw new Error('browser-dev mode has no active Kota workspace');
  }
  return invoke<AgentSpawnRequest>('workspace_resolve_agent_launch', { agentId, cli });
}

export async function writeAgentPty(agentId: string, input: string): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('pty_agent_write', { agentId, input });
}

export async function submitAgentPromptPty(agentId: string, input: string): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('pty_agent_submit_prompt', { agentId, input });
}

export async function resizeAgentPty(
  agentId: string,
  cols: number,
  rows: number,
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('pty_agent_resize', { agentId, cols, rows });
}

export async function scrollAgentPty(agentId: string, lines: number): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('pty_agent_scroll', { agentId, lines });
}

export async function interruptAgentPty(agentId: string): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('pty_agent_interrupt', { agentId });
}

export async function closeAgentPty(agentId: string): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('pty_agent_close', { agentId });
}

export async function listAgentPtys(): Promise<AgentSummary[]> {
  if (!useTauriRuntime()) return [];
  return invoke<AgentSummary[]>('pty_agent_list');
}

export async function onAgentOutput(
  agentId: string,
  listener: AgentOutputListener,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) {
    return async () => {};
  }
  return listen<AgentOutputEvent>(
    agentOutputTopic(agentId),
    (event: Event<AgentOutputEvent>) => listener(event.payload),
  );
}

export async function onAgentExit(
  agentId: string,
  listener: AgentExitListener,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) {
    return async () => {};
  }
  return listen<AgentExitEvent>(
    agentExitTopic(agentId),
    (event: Event<AgentExitEvent>) => listener(event.payload),
  );
}

export async function onAgentStatus(
  agentId: string,
  listener: AgentStatusListener,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) {
    return async () => {};
  }
  return listen<AgentStatusEvent>(
    agentStatusTopic(agentId),
    (event: Event<AgentStatusEvent>) => listener(event.payload),
  );
}

export async function onAgentWorkState(
  agentId: string,
  listener: AgentWorkListener,
): Promise<UnlistenFn> {
  const offLocal = addAgentWorkListener(agentId, listener);
  if (!useTauriRuntime()) return offLocal;
  const offNative = await listen<AgentWorkStateEvent>(
    agentWorkTopic(agentId),
    (event: Event<AgentWorkStateEvent>) => listener(event.payload),
  );
  return async () => {
    await offLocal();
    await offNative();
  };
}

// ─────────────────────────────────────────── M6.B — gh auth ────
export interface GhAuthInfo {
  authenticated: boolean;
  username: string | null;
  scopes: string[];
  error: string | null;
  cliMissing: boolean;
}

export const GITHUB_CLI_INSTALL_URL = 'https://cli.github.com/';
export const GITHUB_CLI_LOGIN_COMMAND = 'gh auth login --hostname github.com --git-protocol https --web --scopes repo,read:user,user:email';

/** Probe `gh auth status`. Falls back to a "not authenticated" stub
 *  when running outside Tauri (vitest / vite dev). */
export async function ghAuthStatus(): Promise<GhAuthInfo> {
  if (!useTauriRuntime()) {
    return {
      authenticated: false,
      username: null,
      scopes: [],
      error: 'browser-dev mode (no gh CLI access)',
      cliMissing: false,
    };
  }
  return invoke<GhAuthInfo>('gh_auth_status');
}

// ───────────────────────────── Provider auth + project setup ─────

export interface OAuthConfigStatus {
  googleConfigured: boolean;
  githubConfigured: boolean;
  configPath: string;
  appPath: string;
  googleDrivePath: string;
  localAccountFolder: string;
  localProjectRoot: string;
}

export interface StorageMeasurementStatus {
  updating: boolean;
  onDiskBytes: number | null;
  availableBytes: number | null;
  /** Unix timestamp in seconds for the last successful measurement. */
  measuredAt: number | null;
  error: string | null;
}

export interface SupportedShellStatus {
  id: string;
  name: string;
  bin: string;
  installed: boolean;
  resolvedBin: string | null;
  installUrl: string;
  summary: string;
  modelOptions: Array<{ id: string; label: string; source: string }>;
  effortOptions: Array<{ value: string; label: string }>;
}
export type SupportedProviderModel = SupportedShellStatus['modelOptions'][number];

export interface OAuthConfigInput {
  googleClientId?: string | null;
  googleClientSecret?: string | null;
  githubClientId?: string | null;
  googleDrivePath?: string | null;
  localProjectRoot?: string | null;
}

export interface GoogleDriveStatus {
  connected: boolean;
  email: string | null;
  scopes: string[];
  folderId: string | null;
  folderName: string | null;
  folderPath: string | null;
  folderUrl: string | null;
  localAccountFolder: string;
  configMissing: boolean;
  error: string | null;
}

export interface GithubRepo {
  fullName: string;
  name: string;
  owner: string;
  private: boolean;
  defaultBranch: string;
  cloneUrl: string;
  htmlUrl: string;
}

export interface GithubCreateRepoRequest {
  name: string;
  private: boolean;
  autoInit: boolean;
}

export interface WorkspaceAgentSpec extends Omit<AgentSpawnRequest, 'cli'> {
  /** Raw persisted provider. Future Kota versions may write values this build cannot launch. */
  cli: string;
}

export interface WorkspaceProject {
  projectId: string;
  repoFullName: string;
  remoteUrl: string;
  githubHtmlUrl: string;
  defaultBranch: string;
  baseRef: string;
  /** Account project state: ~/Kota/Workspaces/{projectId}. */
  localRoot: string;
  localRootBytes: number;
  /** Real local clone of the connected GitHub project repo. */
  sourceDir: string;
  sourceDirBytes: number;
  sharedDir: string;
  rulesDir: string;
  agents: WorkspaceAgentSpec[];
  archived?: boolean;
  archivedAt?: string | null;
}

export interface WorkspaceStatus {
  active: WorkspaceProject | null;
}

export interface BartenderDirtyAgent {
  agentId: string;
  path: string;
  changeCount: number;
  pendingCommitCount: number;
}

export interface BartenderStatus {
  projectId: string;
  sourceDir: string;
  defaultBranch: string;
  roomChangeCount: number;
  sourceChangeCount: number;
  githubChangeCount: number;
  githubBehindCount: number;
  githubNeedsInitialPush: boolean;
  githubPushBranch?: string | null;
  githubInitialPushCommitCount: number;
  dirtyAgents: BartenderDirtyAgent[];
  checkedAt: string;
  state: 'idle' | 'roomDiff' | 'githubDiff' | 'githubBehind' | 'githubDiverged' | 'githubInitialPush' | string;
  message: string;
}

export interface BartenderConflict {
  agentId: string;
  commit?: string | null;
  message: string;
}

export interface BartenderPublishedAgent {
  agentId: string;
  commitCount: number;
}

export interface BartenderSyncResult {
  ok: boolean;
  message: string;
  snapshotCount: number;
  publishedCommitCount: number;
  publishedAgents: BartenderPublishedAgent[];
  conflicts: BartenderConflict[];
  status: BartenderStatus;
}

export interface BartenderSyncEvent {
  projectRoot: string;
  requestId: string;
  phase: 'started' | 'finished' | 'failed' | string;
  result?: BartenderSyncResult | null;
  error?: string | null;
}

export interface BartenderSyncReceiptRequest {
  projectRoot?: string | null;
  requestId: string;
}

export interface BartenderSyncReceipt {
  projectRoot: string;
  requestId: string;
  phase: 'pending' | 'finished' | 'failed' | string;
  result?: BartenderSyncResult | null;
  error?: string | null;
}

export interface BartenderSyncProgressEvent {
  projectRoot: string;
  phase: string;
  message: string;
  elapsedMs: number;
}

export interface BartenderPushResult {
  ok: boolean;
  message: string;
  pushedCommitCount: number;
  status: BartenderStatus;
}

export interface BartenderFetchResult {
  ok: boolean;
  message: string;
  status: BartenderStatus;
}

export interface BartenderPullConflict {
  message: string;
  sourceHead: string;
  upstream: string;
  upstreamHead: string;
  defaultBranch: string;
}

export interface BartenderPullResult {
  ok: boolean;
  message: string;
  pulledCommitCount: number;
  needsHumanPick: boolean;
  conflict?: BartenderPullConflict | null;
  status: BartenderStatus;
}

export interface BartenderRequest {
  projectRoot?: string | null;
  conflictPrompt?: string | null;
  pullConflictPrompt?: string | null;
}

export interface BartenderRoutePullConflictRequest {
  projectRoot?: string | null;
  agentId: string;
  pullConflictPrompt?: string | null;
}

export interface BartenderRoutePullConflictResult {
  ok: boolean;
  message: string;
  status: BartenderStatus;
}

export interface AgentBusSendRequest {
  projectRoot?: string | null;
  senderAgentId: string;
  senderName?: string | null;
  target: string;
  intent?: string | null;
  text: string;
  eventId?: string | null;
  dedupeKey?: string | null;
  terminalTiming?: AgentBusTerminalTiming | null;
}

export interface AgentBusTerminalTiming {
  trigger: 'scheduled' | 'idle' | 'manual';
  scheduledFor?: string | null;
}

export interface AgentBusSendResult {
  eventId: string;
  targetAgentId: string;
  submitted: boolean;
  duplicate: boolean;
  skippedReason?: string | null;
}

export interface TemporalContextPrepareRequest {
  projectRoot?: string | null;
  targetAgentIds: string[];
  payload: string;
}

export interface TemporalContextPreparedPrompt {
  targetAgentId: string;
  payload: string;
}

export interface AgentBusRetryDeliveryRequest {
  projectRoot?: string | null;
  senderAgentId: string;
  senderName?: string | null;
  targetAgentId: string;
  intent?: string | null;
  text: string;
  originalEventId: string;
  attemptEventId?: string | null;
}

export interface AgentBusRetryDeliveryResult {
  eventId: string;
  targetAgentId: string;
  submitted: boolean;
  skippedReason?: string | null;
}

export interface EmberPrepareDreamsRequest {
  projectRoot?: string | null;
}

export type EmberScheduleState = import('./ember-config').EmberState & {
  schema?: string;
  appLastSeenAt?: string | null;
};

export interface EmberScheduleStateRequest {
  projectRoot?: string | null;
}

export interface EmberScheduleSaveRequest {
  projectRoot?: string | null;
  state: EmberScheduleState;
}

export interface EmberSchedulerTickRequest {
  projectRoots: string[];
  workingAgentIds?: string[];
}

export interface EmberSchedulerTickResult {
  checkedProjects: number;
  fired: number;
  failed: number;
}

export interface EmberHumanReminderRequest {
  projectRoot?: string | null;
  eventId: string;
  text: string;
}

export interface EmberHumanReminderResult {
  eventId: string;
  delivered: boolean;
  roomStatus: 'delivered' | 'duplicate' | 'failed';
  telegramStatus: 'delivered' | 'skipped' | 'failed';
  warnings: string[];
}

export interface EmberSchedulesChangedPayload {
  projectRoot: string;
}

export interface EmberPrepareDreamsResult {
  accountDreamsPath: string;
  entriesDir: string;
  archiveDir: string;
  projectDreamsPath: string;
  projected: boolean;
}

export interface EmberDreamConsolidateRequest {
  projectRoot?: string | null;
  projectRoots?: string[];
  provider?: string | null;
}

export interface EmberDreamConsolidateState {
  accountDreamsPath: string;
  entriesDir: string;
  oldDreamsPath: string;
  promptPath: string;
  processedEntryCount: number;
  activeEntryCount: number;
  archivedEntryCount: number;
  updatedAt: string;
  error?: string | null;
}

export type BbsPostState = 'new' | 'processed' | 'ignored' | 'none' | string;

export interface BbsPost {
  postId: string;
  threadId: string;
  projectId: string;
  projectDisplayName: string;
  agentId: string;
  agentDisplayName: string;
  agentAvatar?: string | null;
  createdAt: string;
  kind: 'topic' | 'reply' | string;
  body: string;
  preview: string;
  state: BbsPostState;
  external: boolean;
}

export interface BbsThread {
  threadId: string;
  visibility: 'targeted' | 'broadcast' | string;
  projectTags: string[];
  projectTagLabels: string[];
  createdByProject: string;
  createdByProjectLabel: string;
  updatedAt: string;
  latestPostId: string;
  isNew: boolean;
  relevant: boolean;
  posts: BbsPost[];
}

export interface BbsSnapshot {
  projectId: string;
  projectDisplayName: string;
  root: string;
  newCount: number;
  threads: BbsThread[];
}

export interface BbsProjectRequest {
  projectId: string;
  projectDisplayName?: string | null;
}

export interface BbsPostStateRequest {
  projectId: string;
  postId: string;
}

export interface BbsDeleteRequest {
  threadId: string;
  postId?: string | null;
}

export interface WorkspaceProjectLifecycleRequest {
  projectId: string;
  forceDirty?: boolean;
}

export interface WorkspaceProjectLifecycleResult {
  ok: boolean;
  dirty: boolean;
  dirtySummary: string;
  project?: WorkspaceProject | null;
}

export interface WorkspaceProjectDirtyStatus {
  dirty: boolean;
  dirtySummary: string;
}

export interface VioletChatMessage {
  id: string;
  sessionId: string;
  agentId: string;
  shell: AgentSpawnRequest['cli'] | string;
  role: 'user' | 'assistant' | 'system' | string;
  kind: 'message' | 'thinking' | 'tool' | 'compaction' | string;
  timestamp: string;
  text: string;
  sourcePath?: string | null;
  nativeEventId?: string | null;
  violetSeq?: number | null;
  actorIntent?: string | null;
  messageOrigin?: string | null;
  targetAgentIds?: string[];
  agentDisplayName?: string | null;
  agentAvatarId?: string | null;
  agentProvider?: string | null;
  agentStatus?: string | null;
}

export interface VioletSourceStatus {
  agentId: string;
  shell: AgentSpawnRequest['cli'] | string;
  sessionId?: string | null;
  sourceKind: string;
  sourcePath?: string | null;
  status: 'synced' | 'missing' | 'empty' | 'error' | string;
  parsed: number;
  written: number;
  skippedPrivate: number;
  error?: string | null;
}

export interface VioletRoomState {
  messages: VioletChatMessage[];
  sources: VioletSourceStatus[];
  workEvents?: AgentWorkStateEvent[];
  agentBusReceipts?: AgentBusReceipt[];
  rawLogDir: string;
  chathistoryDir: string;
  syncedAt: string;
}

export interface AgentBusReceipt {
  eventId: string;
  agentId: string;
  timestamp: string;
}

export interface VioletRoomRequest {
  projectRoot?: string | null;
  limit?: number | null;
  before?: string | null;
  agentIds?: string[] | null;
  watchAgentIds?: string[] | null;
}

export interface VioletRoomChangedEvent {
  projectRoot: string;
  changedAt: string;
  reason: string;
  paths: string[];
}

export interface VioletRoomSyncedEvent {
  request: VioletRoomRequest;
  state: VioletRoomState;
}

export interface VioletPrivacyRequest {
  projectRoot?: string | null;
  agentId: string;
  private: boolean;
}

export interface VioletSummaryEntry {
  id: string;
  updatedAt: string;
  trigger: string;
  provider: string;
  summaryStartTs: string;
  summaryEndTs: string;
  messageCount: number;
  completed: string[];
  lastEventId: string;
  logPath: string;
  cliError?: string | null;
}

export interface VioletSummaryOutstanding {
  sinceTs?: string | null;
  messageCount: number;
}

export interface VioletSummaryState {
  latest?: VioletSummaryEntry | null;
  history: VioletSummaryEntry[];
  outstanding: VioletSummaryOutstanding;
  logPath: string;
  promptPath: string;
  updatedAt: string;
  error?: string | null;
}

export interface VioletSummaryRequest {
  projectRoot?: string | null;
  config?: VioletSummaryConfig | null;
  autoRun?: boolean | null;
}

export interface TavernHeroFileRequest {
  heroId: string;
  fileName: 'GHOST.md' | 'SHELL.yaml';
  content: string;
}

export interface TavernHeroFileResult {
  path: string;
}

export interface SystemPromptReadRequest {
  path: string;
}

export interface SystemPromptReadResult {
  path: string;
  content: string;
}

export interface TavernHeroProfileDraft {
  heroId: string;
  name: string;
  nameFields?: ProjectAgentNameFieldsPayload | null;
  provider: string;
  model: string;
  effort?: string | null;
  avatarId?: string | null;
  skills: string[];
  ghost: string;
  shell: string;
  archived?: boolean;
  dismissed?: boolean;
  kind?: string | null;
  record?: ProjectAgentRecord | null;
}

export interface AccountUserIdentity {
  name: string;
  avatarId?: string | null;
}

export interface AccountRuleDraft {
  fileName: string;
  title: string;
  loadPolicy: 'always' | 'on-demand' | string;
  taskTrigger: string;
  body: string;
  path: string;
  bundledDefault: boolean;
  modified: boolean;
}

export interface AccountRuleSaveRequest {
  fileName?: string | null;
  title: string;
  loadPolicy: 'always' | 'on-demand' | string;
  taskTrigger?: string;
  body: string;
}

export type ProjectRuleDraft = AccountRuleDraft;

export interface ProjectRulesRequest {
  projectRoot?: string | null;
  rulesDir?: string | null;
}

export type ProjectRuleSaveRequest = ProjectRulesRequest & AccountRuleSaveRequest;

export interface AccountSkillDraft {
  id: string;
  name: string;
  description: string;
  path: string;
  kind: 'builtin' | 'manual' | string;
  bundledDefault: boolean;
  valid: boolean;
  createdAt: string;
  error?: string | null;
}

export interface AccountSkillImportArchiveRequest {
  fileName: string;
  dataBase64: string;
}

export interface AccountSkillImportFolderFile {
  relativePath: string;
  dataBase64: string;
}

export interface AccountSkillImportFolderRequest {
  folderName: string;
  files: AccountSkillImportFolderFile[];
}

export interface AccountSkillImportResult {
  skills: AccountSkillDraft[];
  imported: AccountSkillDraft;
  message: string;
}

export interface AccountSkillImportPickerResult {
  result?: AccountSkillImportResult | null;
}

export interface TavernHeroDeleteRequest {
  heroId: string;
}

export interface ProjectAgentNameFieldsPayload {
  titleId?: string | null;
  given: string;
  middle?: string;
  surname?: string;
}

export interface TavernIncarnateHeroRequest {
  agentId: string;
  templateId: string;
  displayName: string;
  projectRoot?: string | null;
  progressId?: string | null;
  profile: TavernHeroProfileDraft;
}

export interface TavernIncarnateHeroResult {
  request: AgentSpawnRequest;
  adapterPath: string;
  shellPath: string;
  matchedSkills: string[];
  missingSkills: string[];
  projectRoot: string;
}

export interface IncarnationProgressEvent {
  progressId: string;
  step: string;
  status: 'running' | 'success' | 'error' | string;
  message: string;
}

export interface ProjectAgentRequest {
  agentId: string;
  projectRoot?: string | null;
}

export interface ProjectAgentRecord {
  turns: number;
  incarnations: number;
  estimatedTokens: number;
  commends?: number;
  lastActiveAt?: string | null;
}

export type ProjectAgentCommendSource = 'agent-bar' | 'table-card' | 'terminal-header' | 'violet-room';

export interface ProjectAgentCommendRequest extends ProjectAgentRequest {
  source: ProjectAgentCommendSource;
}

export interface ProjectAgentInviteEligibility {
  eligible: boolean;
  reason?: string | null;
  duplicateHeroId?: string | null;
  proposedHeroId: string;
  proposedDisplayName: string;
}

export interface ProjectAgentDetail {
  agentId: string;
  displayName: string;
  nameFields?: ProjectAgentNameFieldsPayload | null;
  sourceHeroId: string;
  sourceHeroName: string;
  projectId: string;
  projectName: string;
  cli: AgentSpawnRequest['cli'];
  provider: string;
  model: string;
  effort?: string | null;
  avatarId?: string | null;
  skills: string[];
  args: string[];
  ghost: string;
  adapterPath: string;
  shellPath: string;
  agentYamlPath: string;
  status: 'active' | 'archived' | string;
  archivedAt?: string | null;
  inviteEligibility: ProjectAgentInviteEligibility;
  record: ProjectAgentRecord;
  sessionId?: string | null;
  forkable: boolean;
  sessionSource?: string | null;
  dirty: boolean;
  dirtySummary: string;
}

export interface ProjectAgentIdentity {
  agentId: string;
  displayName: string;
  sourceHeroId: string;
  status: 'active' | 'archived' | string;
  provider?: string | null;
  avatarId?: string | null;
}

export interface ProjectAgentIdentityListing {
  identities: ProjectAgentIdentity[];
  workspaceEntryCount: number;
}

export interface ProjectAgentSaveRequest extends ProjectAgentRequest {
  displayName: string;
  nameFields?: ProjectAgentNameFieldsPayload | null;
  model: string;
  effort?: string | null;
  avatarId?: string | null;
  skills: string[];
  ghost: string;
}

export interface ProjectAgentLifecycleRequest extends ProjectAgentRequest {
  forceDirty?: boolean;
}

export interface ProjectAgentLifecycleResult {
  ok: boolean;
  dirty: boolean;
  dirtySummary: string;
  detail?: ProjectAgentDetail | null;
}

export interface ProjectAgentInviteRequest extends ProjectAgentRequest {
  displayName?: string | null;
  forceDuplicate?: boolean;
}

export interface ProjectAgentInviteResult {
  heroId: string;
  displayName: string;
  path: string;
  duplicateHeroId?: string | null;
}

export interface ProjectAgentBunshinResult {
  detail: ProjectAgentDetail;
  request: AgentSpawnRequest;
}

export interface ProjectAgentFreshSessionResult {
  detail: ProjectAgentDetail;
  request: AgentSpawnRequest;
}

export type ProjectAgentLaunchResolution =
  | { status: 'ready'; request: AgentSpawnRequest }
  | { status: 'sessionUnavailable' };

export async function authConfigStatus(): Promise<OAuthConfigStatus> {
  if (!useTauriRuntime()) {
    return {
      googleConfigured: false,
      githubConfigured: false,
      configPath: '~/Kota/oauth-config.json',
      appPath: 'browser-dev',
      googleDrivePath: 'Kota Sync',
      localAccountFolder: '~/Kota',
      localProjectRoot: '~/Kota/Projects',
    };
  }
  return invoke<OAuthConfigStatus>('auth_config_status');
}

const EMPTY_STORAGE_MEASUREMENT: StorageMeasurementStatus = {
  updating: false,
  onDiskBytes: null,
  availableBytes: null,
  measuredAt: null,
  error: null,
};

let mockStorageMeasurement: StorageMeasurementStatus = { ...EMPTY_STORAGE_MEASUREMENT };
let mockStorageMeasurementFinishesAt: number | null = null;

function resetMockStorageMeasurement() {
  mockStorageMeasurement = { ...EMPTY_STORAGE_MEASUREMENT };
  mockStorageMeasurementFinishesAt = null;
}

function mockStorageMeasurementSnapshot(): StorageMeasurementStatus {
  if (
    mockStorageMeasurement.updating
    && mockStorageMeasurementFinishesAt != null
    && Date.now() >= mockStorageMeasurementFinishesAt
  ) {
    mockStorageMeasurement = {
      updating: false,
      onDiskBytes: 46 * 1024 ** 3,
      availableBytes: 161 * 1024 ** 3,
      measuredAt: Math.floor(Date.now() / 1000),
      error: null,
    };
    mockStorageMeasurementFinishesAt = null;
  }
  return { ...mockStorageMeasurement };
}

export async function storageMeasureStatus(): Promise<StorageMeasurementStatus> {
  if (!useTauriRuntime()) return mockStorageMeasurementSnapshot();
  return invoke<StorageMeasurementStatus>('storage_measure_status');
}

export async function storageMeasureStart(): Promise<StorageMeasurementStatus> {
  if (!useTauriRuntime()) {
    mockStorageMeasurement = {
      ...mockStorageMeasurementSnapshot(),
      updating: true,
      error: null,
    };
    mockStorageMeasurementFinishesAt = Date.now() + 1200;
    return { ...mockStorageMeasurement };
  }
  return invoke<StorageMeasurementStatus>('storage_measure_start');
}

export async function supportedShellsStatus(): Promise<SupportedShellStatus[]> {
  if (!useTauriRuntime()) {
    return [
      {
        id: 'claude',
        name: 'Claude Code',
        bin: 'claude',
        installed: true,
        resolvedBin: '/opt/homebrew/bin/claude',
        installUrl: 'https://docs.anthropic.com/en/docs/claude-code/setup',
        summary: "Claude's agentic coding terminal.",
        modelOptions: [
          { id: 'default', label: 'default alias', source: 'mock' },
          { id: 'sonnet', label: 'sonnet alias', source: 'mock' },
          { id: 'opus', label: 'opus alias', source: 'mock' },
          { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6', source: 'mock' },
          { id: 'claude-opus-4-6[1m]', label: 'Claude Opus 4.6, 1M context', source: 'mock' },
          { id: 'claude-opus-4-7[1m]', label: 'Claude Opus 4.7, 1M context', source: 'mock' },
          { id: 'claude-opus-4-8[1m]', label: 'Claude Opus 4.8, 1M context', source: 'mock' },
        ],
        effortOptions: [
          { value: 'low', label: 'Low' },
          { value: 'medium', label: 'Medium' },
          { value: 'high', label: 'High' },
          { value: 'xhigh', label: 'XHigh' },
          { value: 'max', label: 'Max' },
        ],
      },
      {
        id: 'codex',
        name: 'Codex',
        bin: 'codex',
        installed: true,
        resolvedBin: '/opt/homebrew/bin/codex',
        installUrl: 'https://github.com/openai/codex',
        summary: "OpenAI's local coding agent CLI.",
        modelOptions: [
          { id: 'default', label: 'CLI default', source: 'kota seed' },
          { id: 'gpt-5.5', label: 'GPT-5.5', source: 'mock' },
          { id: 'gpt-5.4', label: 'gpt-5.4', source: 'mock' },
          { id: 'gpt-5.4-mini', label: 'GPT-5.4-Mini', source: 'mock' },
          { id: 'gpt-5.3-codex', label: 'gpt-5.3-codex', source: 'mock' },
        ],
        effortOptions: [
          { value: 'low', label: 'Low' },
          { value: 'medium', label: 'Medium' },
          { value: 'high', label: 'High' },
          { value: 'xhigh', label: 'XHigh' },
          { value: 'max', label: 'Max' },
          { value: 'ultra', label: 'Ultra' },
        ],
      },
      {
        id: 'opencode',
        name: 'OpenCode',
        bin: 'opencode',
        installed: false,
        resolvedBin: null,
        installUrl: 'https://opencode.ai/docs',
        summary: "OpenCode's terminal coding agent.",
        modelOptions: [
          { id: 'opencode/deepseek-v4-flash-free', label: 'opencode/deepseek-v4-flash-free', source: 'mock' },
          { id: 'opencode/minimax-m2.5-free', label: 'opencode/minimax-m2.5-free', source: 'mock' },
          { id: 'openai/gpt-5.4', label: 'openai/gpt-5.4', source: 'mock' },
        ],
        effortOptions: [],
      },
      {
        id: 'antigravity',
        name: 'Antigravity CLI',
        bin: 'agy',
        installed: false,
        resolvedBin: null,
        installUrl: 'https://www.antigravity.google/docs/cli/cli-getting-started',
        summary: "Google Antigravity's terminal coding agent.",
        modelOptions: [{ id: 'default', label: 'Antigravity default', source: 'mock' }],
        effortOptions: [],
      },
      {
        id: 'pi',
        name: 'Pi',
        bin: 'pi',
        installed: true,
        resolvedBin: '/opt/homebrew/bin/pi',
        installUrl: 'https://pi.dev',
        summary: "Pi's local coding agent.",
        modelOptions: [
          { id: 'google/gemini-2.5-pro', label: 'google/gemini-2.5-pro', source: 'mock' },
          { id: 'google/gemini-2.5-flash', label: 'google/gemini-2.5-flash', source: 'mock' },
          { id: 'anthropic/claude-sonnet-4-5', label: 'anthropic/claude-sonnet-4-5', source: 'mock' },
          { id: 'openai/gpt-5.5', label: 'openai/gpt-5.5', source: 'mock' },
        ],
        effortOptions: [
          { value: 'off', label: 'Off' },
          { value: 'minimal', label: 'Minimal' },
          { value: 'low', label: 'Low' },
          { value: 'medium', label: 'Medium' },
          { value: 'high', label: 'High' },
          { value: 'xhigh', label: 'XHigh' },
        ],
      },
      {
        id: 'kimi',
        name: 'Kimi Code',
        bin: 'kimi',
        installed: true,
        resolvedBin: '~/.kimi-code/bin/kimi',
        installUrl: 'https://code.kimi.com/',
        summary: "Moonshot AI's local coding agent CLI.",
        modelOptions: [{ id: 'default', label: 'Kimi CLI default', source: 'mock' }],
        effortOptions: [],
      },
    ];
  }
  return invoke<SupportedShellStatus[]>('supported_shells_status');
}

export async function refreshProviderModelOptions(provider: string): Promise<SupportedProviderModel[]> {
  if (!useTauriRuntime()) {
    const shell = (await supportedShellsStatus()).find((entry) => entry.id === provider);
    return shell?.modelOptions ?? [];
  }
  return invoke<SupportedProviderModel[]>('provider_model_options_refresh', { provider });
}

export async function saveAuthConfig(config: OAuthConfigInput): Promise<OAuthConfigStatus> {
  if (!useTauriRuntime()) return authConfigStatus();
  return invoke<OAuthConfigStatus>('auth_config_save', { config });
}

export async function googleDriveStatus(): Promise<GoogleDriveStatus> {
  if (!useTauriRuntime()) {
    return {
      connected: false,
      email: null,
      scopes: [],
      folderId: null,
      folderName: null,
      folderPath: null,
      folderUrl: null,
      localAccountFolder: '~/Kota',
      configMissing: true,
      error: 'browser-dev mode',
    };
  }
  return invoke<GoogleDriveStatus>('google_drive_status');
}

export async function connectGoogleDrive(drivePath?: string | null): Promise<GoogleDriveStatus> {
  if (!useTauriRuntime()) return googleDriveStatus();
  return invoke<GoogleDriveStatus>('google_drive_connect_and_setup', {
    drivePath: drivePath || null,
  });
}

export async function disconnectGoogleDrive(): Promise<GoogleDriveStatus> {
  if (!useTauriRuntime()) return googleDriveStatus();
  return invoke<GoogleDriveStatus>('google_drive_disconnect');
}

export async function githubListRepos(): Promise<GithubRepo[]> {
  if (!useTauriRuntime()) return [];
  return invoke<GithubRepo[]>('github_list_repos');
}

export async function githubCreateRepo(request: GithubCreateRepoRequest): Promise<GithubRepo> {
  if (!useTauriRuntime()) {
    return {
      fullName: `mock/${request.name}`,
      name: request.name,
      owner: 'mock',
      private: request.private,
      defaultBranch: 'main',
      cloneUrl: `https://github.com/mock/${request.name}.git`,
      htmlUrl: `https://github.com/mock/${request.name}`,
    };
  }
  return invoke<GithubRepo>('github_create_repo', { request });
}

export async function prepareGithubProject(repoFullName: string): Promise<WorkspaceProject> {
  if (!useTauriRuntime()) {
    throw new Error('browser-dev mode has no git workspace materializer');
  }
  return invoke<WorkspaceProject>('workspace_prepare_github_project', {
    request: { repoFullName },
  });
}

export async function workspaceStatus(): Promise<WorkspaceStatus> {
  if (!useTauriRuntime()) return { active: null };
  return invoke<WorkspaceStatus>('workspace_status');
}

function mockBartenderStatus(): BartenderStatus {
  return {
    projectId: 'browser-dev',
    sourceDir: '/tmp/kota-dev',
    defaultBranch: 'main',
    roomChangeCount: 0,
    sourceChangeCount: 0,
    githubChangeCount: 0,
    githubBehindCount: 0,
    githubNeedsInitialPush: false,
    githubPushBranch: null,
    githubInitialPushCommitCount: 0,
    dirtyAgents: [],
    checkedAt: new Date().toISOString(),
    state: 'idle',
    message: 'Room is in sync.',
  };
}

export async function bartenderStatus(request: BartenderRequest = {}): Promise<BartenderStatus> {
  if (!useTauriRuntime()) return mockBartenderStatus();
  return invoke<BartenderStatus>('bartender_status', { request });
}

export async function bartenderFetch(request: BartenderRequest = {}): Promise<BartenderFetchResult> {
  if (!useTauriRuntime()) {
    return {
      ok: true,
      message: 'Fetched GitHub.',
      status: mockBartenderStatus(),
    };
  }
  return invoke<BartenderFetchResult>('bartender_fetch', { request });
}

export async function bartenderSyncLocal(
  request: BartenderRequest = {},
): Promise<BartenderSyncResult> {
  if (!useTauriRuntime()) {
    const status = mockBartenderStatus();
    return {
      ok: true,
      message: 'Nothing to sync.',
      snapshotCount: 0,
      publishedCommitCount: 0,
      publishedAgents: [],
      conflicts: [],
      status,
    };
  }
  return invoke<BartenderSyncResult>('bartender_sync_local', { request });
}

export async function bartenderSyncReceipt(
  request: BartenderSyncReceiptRequest,
): Promise<BartenderSyncReceipt> {
  if (!useTauriRuntime()) {
    return {
      projectRoot: request.projectRoot ?? '',
      requestId: request.requestId,
      phase: 'pending',
    };
  }
  return invoke<BartenderSyncReceipt>('bartender_sync_receipt', { request });
}

export async function onBartenderSyncEvent(
  callback: (payload: BartenderSyncEvent) => void,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) return async () => {};
  return listen<BartenderSyncEvent>(BARTENDER_SYNC_EVENT, (event: Event<BartenderSyncEvent>) => {
    callback(event.payload);
  });
}

export async function onBartenderSyncProgressEvent(
  callback: (payload: BartenderSyncProgressEvent) => void,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) return async () => {};
  return listen<BartenderSyncProgressEvent>(BARTENDER_SYNC_PROGRESS_EVENT, (event: Event<BartenderSyncProgressEvent>) => {
    callback(event.payload);
  });
}

export async function bartenderPullFromGithub(
  request: BartenderRequest = {},
): Promise<BartenderPullResult> {
  if (!useTauriRuntime()) {
    const status = mockBartenderStatus();
    return {
      ok: true,
      message: 'Nothing to pull.',
      pulledCommitCount: 0,
      needsHumanPick: false,
      conflict: null,
      status,
    };
  }
  return invoke<BartenderPullResult>('bartender_pull_from_github', { request });
}

export async function bartenderPushToGithub(
  request: BartenderRequest = {},
): Promise<BartenderPushResult> {
  if (!useTauriRuntime()) {
    const status = mockBartenderStatus();
    return {
      ok: true,
      message: 'Nothing to push.',
      pushedCommitCount: 0,
      status,
    };
  }
  return invoke<BartenderPushResult>('bartender_push_to_github', { request });
}

export async function bartenderRoutePullConflict(
  request: BartenderRoutePullConflictRequest,
): Promise<BartenderRoutePullConflictResult> {
  if (!useTauriRuntime()) {
    return {
      ok: true,
      message: `Routed pull conflict task to ${request.agentId}.`,
      status: mockBartenderStatus(),
    };
  }
  return invoke<BartenderRoutePullConflictResult>('bartender_route_pull_conflict', { request });
}

export async function agentBusSend(request: AgentBusSendRequest): Promise<AgentBusSendResult> {
  if (!useTauriRuntime()) {
    return {
      eventId: request.eventId || `dev-agentbus-${Date.now()}`,
      targetAgentId: request.target,
      submitted: true,
      duplicate: false,
      skippedReason: null,
    };
  }
  return invoke<AgentBusSendResult>('agent_bus_send', { request });
}

export async function prepareComposerTemporalContext(
  request: TemporalContextPrepareRequest,
): Promise<TemporalContextPreparedPrompt[]> {
  if (!useTauriRuntime()) {
    return Array.from(new Set(request.targetAgentIds.filter(Boolean))).map((targetAgentId) => ({
      targetAgentId,
      payload: request.payload,
    }));
  }
  return invoke<TemporalContextPreparedPrompt[]>('temporal_context_prepare_composer', { request });
}

export async function agentBusRetryDelivery(
  request: AgentBusRetryDeliveryRequest,
): Promise<AgentBusRetryDeliveryResult> {
  if (!useTauriRuntime()) {
    return {
      eventId: request.attemptEventId || `${request.originalEventId}:retry:${Date.now()}`,
      targetAgentId: request.targetAgentId,
      submitted: true,
      skippedReason: null,
    };
  }
  return invoke<AgentBusRetryDeliveryResult>('agent_bus_retry_delivery', { request });
}

export async function emberScheduleState(
  request: EmberScheduleStateRequest,
): Promise<EmberScheduleState> {
  if (!useTauriRuntime()) {
    return { drafts: [], schedules: [], history: [], appLastSeenAt: null };
  }
  return invoke<EmberScheduleState>('ember_schedule_state', { request });
}

export async function emberScheduleSave(
  request: EmberScheduleSaveRequest,
): Promise<EmberScheduleState> {
  if (!useTauriRuntime()) return request.state;
  return invoke<EmberScheduleState>('ember_schedule_save', { request });
}

export async function emberSchedulerTick(
  request: EmberSchedulerTickRequest,
): Promise<EmberSchedulerTickResult> {
  if (!useTauriRuntime()) {
    return { checkedProjects: 0, fired: 0, failed: 0 };
  }
  return invoke<EmberSchedulerTickResult>('ember_scheduler_tick', { request });
}

export async function emberDeliverHumanReminder(
  request: EmberHumanReminderRequest,
): Promise<EmberHumanReminderResult> {
  if (!useTauriRuntime()) {
    return {
      eventId: request.eventId,
      delivered: true,
      roomStatus: 'delivered',
      telegramStatus: 'skipped',
      warnings: [],
    };
  }
  return invoke<EmberHumanReminderResult>('ember_deliver_human_reminder', { request });
}

export async function onEmberSchedulesChanged(
  callback: (payload: EmberSchedulesChangedPayload) => void,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) return async () => {};
  return listen<EmberSchedulesChangedPayload>(EMBER_SCHEDULES_CHANGED_EVENT, (event: Event<EmberSchedulesChangedPayload>) => {
    callback(event.payload);
  });
}

export async function emberPrepareDreams(
  request: EmberPrepareDreamsRequest,
): Promise<EmberPrepareDreamsResult> {
  if (!useTauriRuntime()) {
    const root = request.projectRoot || '/tmp/kota-dev';
    return {
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      archiveDir: '/tmp/Kota/dreams/archive',
      projectDreamsPath: `${root}/project-memory/dreams.md`,
      projected: false,
    };
  }
  return invoke<EmberPrepareDreamsResult>('ember_prepare_dreams', { request });
}

export async function emberConsolidateDreams(
  request: EmberDreamConsolidateRequest,
): Promise<EmberDreamConsolidateState> {
  if (!useTauriRuntime()) {
    const now = new Date().toISOString();
    return {
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      oldDreamsPath: '/tmp/Kota/dreams/old_dreams.md',
      promptPath: '$KOTA_HOME/heroes/system-ember/ember-dream-consolidate.md',
      processedEntryCount: 0,
      activeEntryCount: 0,
      archivedEntryCount: 0,
      updatedAt: now,
      error: null,
    };
  }
  return invoke<EmberDreamConsolidateState>('ember_consolidate_dreams', { request });
}

export async function bbsSnapshot(request: BbsProjectRequest): Promise<BbsSnapshot> {
  if (!useTauriRuntime()) {
    return {
      projectId: request.projectId,
      projectDisplayName: request.projectId,
      root: '/tmp/kota-dev/bbs',
      newCount: 0,
      threads: [],
    };
  }
  return invoke<BbsSnapshot>('bbs_snapshot', { request });
}

export async function bbsMarkProcessed(request: BbsPostStateRequest): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('bbs_mark_processed', { request });
}

export async function bbsIgnorePost(request: BbsPostStateRequest): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('bbs_ignore_post', { request });
}

export async function bbsDelete(request: BbsDeleteRequest): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('bbs_delete', { request });
}

export interface BbsHumanReplyRequest {
  projectId: string;
  projectDisplayName?: string | null;
  threadId: string;
  body: string;
}

export interface BbsHumanPostRequest {
  projectId: string;
  projectDisplayName?: string | null;
  projectTags: string[];
  body: string;
}

export async function bbsHumanReply(request: BbsHumanReplyRequest): Promise<string> {
  if (!useTauriRuntime()) throw new Error('BBS requires the Kota runtime.');
  return invoke<string>('bbs_human_reply', { request });
}

export async function bbsHumanPost(request: BbsHumanPostRequest): Promise<string> {
  if (!useTauriRuntime()) throw new Error('BBS requires the Kota runtime.');
  return invoke<string>('bbs_human_post', { request });
}

export interface AccountDreamsStatus {
  exists: boolean;
  path: string;
}

export async function accountDreamsStatus(): Promise<AccountDreamsStatus> {
  if (!useTauriRuntime()) return { exists: false, path: '' };
  return invoke<AccountDreamsStatus>('account_dreams_status');
}

export async function accountDreamsOpen(): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('account_dreams_open');
}

export interface LmSelected {
  projectId: string;
  projectRoot: string;
  projectName: string;
  agentId: string;
  agentName: string;
  muted: boolean;
}

export interface LmPendingClaim {
  userId: number;
  username?: string | null;
  firstName?: string | null;
}

export interface LmLogEntry {
  schema: string;
  id: string;
  ts: string;
  direction: 'in' | 'out' | 'system' | string;
  projectId: string;
  projectName: string;
  agentId: string;
  agentName: string;
  preview: string;
  mediaCount?: number;
  offlineRecordedAt?: string | null;
}

export interface LmStandbyStatus {
  workerUrl: string;
  live: boolean;
  lastHeartbeatAt?: string | null;
  lastSyncAt?: string | null;
  lastError?: string | null;
  relayVersion?: string | null;
  protocolVersion?: string | null;
  recommendedVersion: string;
  updateAvailable: boolean;
  queueCount: number;
}

export interface LmStandbyQueueItem {
  id: string;
  receivedAt: string;
  preview: string;
  projectId?: string | null;
  projectName?: string | null;
  agentId?: string | null;
  agentName?: string | null;
  status: 'queued' | 'sent' | 'discarded' | string;
  sentAt?: string | null;
  deliveryError?: string | null;
}

export interface LmStatus {
  configured: boolean;
  enabled: boolean;
  running: boolean;
  botUsername?: string | null;
  ownerUserId?: number | null;
  pendingClaim?: LmPendingClaim | null;
  selected?: LmSelected | null;
  lastError?: string | null;
  latest?: LmLogEntry | null;
  standby?: LmStandbyStatus | null;
}

export interface LmEmberReminderRequest {
  eventId: string;
  text: string;
  projectId?: string | null;
  projectName?: string | null;
  projectRoot?: string | null;
}

export interface LmStandbyDeployEvent {
  phase: string;
  level: 'info' | 'warn' | 'error' | string;
  line: string;
  workerUrl?: string | null;
}

export interface LmStandbyDeployResult {
  workerUrl?: string | null;
  workerDir: string;
}

export async function lmStatus(): Promise<LmStatus | null> {
  if (!useTauriRuntime()) return null;
  return invoke<LmStatus>('lm_status');
}

export async function lmSaveToken(token: string): Promise<string> {
  if (!useTauriRuntime()) throw new Error('Laughing Man requires the Kota runtime.');
  return invoke<string>('lm_save_token', { token });
}

export async function lmClaimOwner(): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('lm_claim_owner');
}

export async function lmStart(): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('lm_start');
}

export async function lmRevoke(): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('lm_revoke');
}

export async function lmSetMuted(muted: boolean): Promise<LmStatus | null> {
  if (!useTauriRuntime()) return null;
  return invoke<LmStatus>('lm_set_muted', { muted });
}

export async function lmMessageLog(limit = 100): Promise<LmLogEntry[]> {
  if (!useTauriRuntime()) return [];
  return invoke<LmLogEntry[]>('lm_message_log', { limit });
}

export async function lmUpdateWorkingAgents(workingAgentIds: readonly string[]): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('lm_update_working_agents', { request: { workingAgentIds } });
}

export async function lmStandbyDeployWorker(): Promise<LmStandbyDeployResult> {
  if (!useTauriRuntime()) throw new Error('Laughing Man requires the Kota runtime.');
  return invoke<LmStandbyDeployResult>('lm_standby_deploy_worker');
}

export async function onLmStandbyDeployEvent(
  callback: (payload: LmStandbyDeployEvent) => void,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) return async () => {};
  return listen<LmStandbyDeployEvent>(LM_STANDBY_DEPLOY_EVENT, (event: Event<LmStandbyDeployEvent>) => {
    callback(event.payload);
  });
}

export async function onIncarnationProgressEvent(
  callback: (payload: IncarnationProgressEvent) => void,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) return async () => {};
  return listen<IncarnationProgressEvent>(INCARNATION_PROGRESS_EVENT, (event: Event<IncarnationProgressEvent>) => {
    callback(event.payload);
  });
}

export async function lmStandbyConnect(workerUrl: string): Promise<LmStatus | null> {
  if (!useTauriRuntime()) return null;
  return invoke<LmStatus>('lm_standby_connect', { request: { workerUrl } });
}

export async function lmStandbyDisconnect(): Promise<LmStatus | null> {
  if (!useTauriRuntime()) return null;
  return invoke<LmStatus>('lm_standby_disconnect');
}

export async function lmStandbyQueue(limit = 100): Promise<LmStandbyQueueItem[]> {
  if (!useTauriRuntime()) return [];
  return invoke<LmStandbyQueueItem[]>('lm_standby_queue', { limit });
}

export async function lmStandbySendQueued(id: string): Promise<LmStandbyQueueItem> {
  if (!useTauriRuntime()) throw new Error('Laughing Man requires the Kota runtime.');
  return invoke<LmStandbyQueueItem>('lm_standby_send_queued', { request: { id } });
}

export async function lmStandbyDeleteQueued(id: string): Promise<LmStandbyQueueItem> {
  if (!useTauriRuntime()) throw new Error('Laughing Man requires the Kota runtime.');
  return invoke<LmStandbyQueueItem>('lm_standby_delete_queued', { request: { id } });
}

export async function lmSendEmberReminder(request: LmEmberReminderRequest): Promise<void> {
  if (!useTauriRuntime()) throw new Error('Laughing Man requires the Kota runtime.');
  await invoke('lm_send_ember_reminder', { request });
}

export async function fileImageDataUrl(path: string): Promise<string> {
  if (!useTauriRuntime()) throw new Error('Image preview requires the Kota runtime.');
  return invoke<string>('file_image_data_url', { request: { path } });
}

export interface VioletFileRefRequest {
  projectRoot?: string | null;
  path: string;
}

export interface VioletFileRefResolveResult {
  path: string;
  isDir: boolean;
}

export async function violetResolveFileRef(
  request: VioletFileRefRequest,
): Promise<VioletFileRefResolveResult | null> {
  if (!useTauriRuntime()) return null;
  return invoke<VioletFileRefResolveResult | null>('violet_resolve_file_ref', { request });
}

export async function violetOpenFileRef(request: VioletFileRefRequest): Promise<void> {
  if (!useTauriRuntime()) throw new Error('File open requires the Kota runtime.');
  await invoke('violet_open_file_ref', { request });
}

export async function violetRevealFileRef(request: VioletFileRefRequest): Promise<void> {
  if (!useTauriRuntime()) throw new Error('File reveal requires the Kota runtime.');
  await invoke('violet_reveal_file_ref', { request });
}

export async function listWorkspaceProjects(): Promise<WorkspaceProject[]> {
  if (!useTauriRuntime()) return [];
  return invoke<WorkspaceProject[]>('workspace_list_projects');
}

export async function listArchivedWorkspaceProjects(): Promise<WorkspaceProject[]> {
  if (!useTauriRuntime()) return [];
  return invoke<WorkspaceProject[]>('workspace_list_archived_projects');
}

export async function openWorkspaceProject(projectId: string): Promise<WorkspaceProject> {
  if (!useTauriRuntime()) {
    throw new Error('browser-dev mode has no workspace registry');
  }
  return invoke<WorkspaceProject>('workspace_open_project', { projectId });
}

export async function inspectWorkspaceProject(projectId: string): Promise<WorkspaceProjectDirtyStatus> {
  if (!useTauriRuntime()) return { dirty: false, dirtySummary: '' };
  return invoke<WorkspaceProjectDirtyStatus>('workspace_inspect_project', { projectId });
}

export async function archiveWorkspaceProject(
  request: WorkspaceProjectLifecycleRequest,
): Promise<WorkspaceProjectLifecycleResult> {
  if (!useTauriRuntime()) return { ok: true, dirty: false, dirtySummary: '', project: null };
  return invoke<WorkspaceProjectLifecycleResult>('workspace_archive_project', { request });
}

export async function resumeWorkspaceProject(projectId: string): Promise<WorkspaceProject> {
  if (!useTauriRuntime()) throw new Error('browser-dev mode has no workspace registry');
  return invoke<WorkspaceProject>('workspace_resume_project', { projectId });
}

export async function removeWorkspaceProject(
  request: WorkspaceProjectLifecycleRequest,
): Promise<WorkspaceProjectLifecycleResult> {
  if (!useTauriRuntime()) return { ok: true, dirty: false, dirtySummary: '', project: null };
  return invoke<WorkspaceProjectLifecycleResult>('workspace_remove_project', { request });
}

export async function workspaceListTreePath(
  request: WorkspaceTreePathRequest,
): Promise<WorkspaceTreeListing> {
  if (!useTauriRuntime()) return mockWorkspaceTreeListing(request.rootKind, request.relativePath ?? '');
  return invoke<WorkspaceTreeListing>('workspace_list_tree_path', { request });
}

export async function workspaceDiffChanges(
  request: { projectId: string; scope: WorkspaceDiffScope },
): Promise<WorkspaceDiffChangeEntry[]> {
  if (!useTauriRuntime()) return mockWorkspaceDiffChanges(request.scope);
  return invoke<WorkspaceDiffChangeEntry[]>('workspace_diff_changes', { request });
}

export async function workspaceFileDiff(
  request: WorkspaceFileDiffRequest,
): Promise<WorkspaceFileDiffResult> {
  if (!useTauriRuntime()) return mockWorkspaceFileDiff(request);
  return invoke<WorkspaceFileDiffResult>('workspace_file_diff', { request });
}

export async function workspaceRevealTreePath(
  request: WorkspaceTreePathRequest,
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('workspace_reveal_tree_path', { request });
}

export async function workspaceOpenTreePath(
  request: WorkspaceTreePathRequest,
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('workspace_open_tree_path', { request });
}

export async function openExternalUrl(url: string): Promise<void> {
  if (!useTauriRuntime()) {
    window.open(url, '_blank', 'noopener,noreferrer');
    return;
  }
  await invoke('open_external_url', { url });
}

export interface AppUpdateInfo {
  hasUpdate: boolean;
  latestVersion: string;
  homeUrl: string;
  releaseNotesUrl?: string | null;
  artifactFilename?: string | null;
}

export async function checkAppUpdate(): Promise<AppUpdateInfo> {
  if (!useTauriRuntime()) {
    return {
      hasUpdate: false,
      latestVersion: '',
      homeUrl: 'https://kota.place',
      releaseNotesUrl: null,
      artifactFilename: null,
    };
  }
  return invoke<AppUpdateInfo>('app_update_check');
}

function mockWorkspaceTreeListing(
  rootKind: WorkspaceTreeRootKind,
  relativePath: string,
): WorkspaceTreeListing {
  const rootPath = rootKind === 'projectFiles'
    ? '/tmp/kota-dev'
    : '/tmp/kota-dev/Kota/Workspaces/mock';
  const root = {
    kind: rootKind,
    label: rootKind === 'projectFiles' ? 'Project Files' : 'Project Workspace',
    absolutePath: rootPath,
    changeOverview: rootKind === 'projectFiles'
      ? { added: 2, modified: 5, deleted: 1, untracked: 3 }
      : null,
  };
  const entry = (
    name: string,
    path: string,
    kind: 'file' | 'folder' | 'symlink',
    extra: Partial<WorkspaceTreeListing['entries'][number]> = {},
  ): WorkspaceTreeListing['entries'][number] => ({
    name,
    path,
    kind,
    absolutePath: `${rootPath}/${path}`,
    isHidden: name.startsWith('.'),
    size: kind === 'folder' ? null : 1024,
    modifiedAt: '2026-05-18T12:00:00Z',
    symlinkTarget: null,
    isWorktree: false,
    worktreeSource: null,
    agentDisplayName: null,
    changeOverview: null,
    ...extra,
  });

  if (rootKind === 'projectFiles') {
    if (!relativePath) {
      return {
        root,
        entries: [
          entry('.git', '.git', 'folder'),
          entry('.github', '.github', 'folder'),
          entry('.env.local', '.env.local', 'file'),
          entry('agent-only.txt', 'agent-only.txt', 'file', {
            isGhost: true,
            fileChange: {
              status: 'added',
              addedLines: 4,
              deletedLines: 0,
              participants: [{
                actorId: 'alice',
                displayName: 'Dr. Alice Liddell',
                aka: 'Dr. Alice',
                kind: 'agent',
                provider: 'claude',
                avatarId: 'claude',
                status: 'added',
                addedLines: 4,
                deletedLines: 0,
              }],
            },
          }),
          entry('app-v2', 'app-v2', 'folder', {
            changeOverview: { added: 1, modified: 2, deleted: 0, untracked: 0 },
            treeHasChanges: true,
          }),
          entry('product-design', 'product-design', 'folder'),
        ],
      };
    }
    if (relativePath === '.git') {
      return { root, entries: [entry('HEAD', '.git/HEAD', 'file'), entry('config', '.git/config', 'file')] };
    }
    if (relativePath === '.github') {
      return { root, entries: [entry('workflows', '.github/workflows', 'folder')] };
    }
    if (relativePath === '.github/workflows') {
      return { root, entries: [entry('ci.yml', '.github/workflows/ci.yml', 'file', {
        changeOverview: { added: 0, modified: 1, deleted: 0, untracked: 0 },
      })] };
    }
    if (relativePath === 'app-v2') {
      return { root, entries: [entry('src', 'app-v2/src', 'folder'), entry('package.json', 'app-v2/package.json', 'file', {
        fileChange: {
          status: 'modified',
          addedLines: 18,
          deletedLines: 7,
          participants: [
            {
              actorId: 'human',
              displayName: 'Human',
              aka: 'Human',
              kind: 'human',
              status: 'modified',
              addedLines: 8,
              deletedLines: 2,
            },
            {
              actorId: 'alice',
              displayName: 'Dr. Alice Liddell',
              aka: 'Dr. Alice',
              kind: 'agent',
              provider: 'claude',
              avatarId: 'claude',
              status: 'modified',
              addedLines: 10,
              deletedLines: 5,
            },
          ],
        },
      })] };
    }
    if (relativePath === 'app-v2/src') {
      return { root, entries: [entry('App.tsx', 'app-v2/src/App.tsx', 'file'), entry('chrome', 'app-v2/src/chrome', 'folder')] };
    }
    if (relativePath === 'app-v2/src/chrome') {
      return { root, entries: [entry('FileTree.tsx', 'app-v2/src/chrome/FileTree.tsx', 'file')] };
    }
    return { root, entries: [] };
  }

  if (!relativePath) {
    return {
      root,
      entries: [
        entry('workspace.json', 'workspace.json', 'file'),
        entry('meta.yaml', 'meta.yaml', 'file'),
        entry('project-memory', 'project-memory', 'folder'),
        entry('project-rules', 'project-rules', 'folder'),
        entry('.agent-workspaces', '.agent-workspaces', 'folder'),
      ],
    };
  }
  if (relativePath === 'project-rules') {
    return { root, entries: [entry('coding-style.md', 'project-rules/coding-style.md', 'file')] };
  }
  if (relativePath === 'project-memory') {
    return { root, entries: [entry('raw_logs', 'project-memory/raw_logs', 'folder')] };
  }
  if (relativePath === 'project-memory/raw_logs') {
    return { root, entries: [entry('2026-05-18.md', 'project-memory/raw_logs/2026-05-18.md', 'file')] };
  }
  if (relativePath === '.agent-workspaces') {
    return {
      root,
      entries: [
        entry('alice', '.agent-workspaces/alice', 'folder', {
          agentDisplayName: 'Dr. Alice Liddell',
        }),
      ],
    };
  }
  if (relativePath === '.agent-workspaces/alice') {
    return {
      root,
      entries: [
        entry('AGENTS.md', '.agent-workspaces/alice/AGENTS.md', 'file'),
        entry('SHELL.yaml', '.agent-workspaces/alice/SHELL.yaml', 'file'),
        entry('.claude', '.agent-workspaces/alice/.claude', 'folder'),
        entry('project-memory', '.agent-workspaces/alice/project-memory', 'symlink', {
          symlinkTarget: `${rootPath}/project-memory`,
        }),
        entry('project-rules', '.agent-workspaces/alice/project-rules', 'symlink', {
          symlinkTarget: `${rootPath}/project-rules`,
        }),
        entry('project-files', '.agent-workspaces/alice/project-files', 'folder', {
          isWorktree: true,
          worktreeSource: '/tmp/kota-dev',
          changeOverview: { added: 1, modified: 3, deleted: 0, untracked: 0 },
        }),
      ],
    };
  }
  if (relativePath === '.agent-workspaces/alice/.claude') {
    return { root, entries: [entry('skills', '.agent-workspaces/alice/.claude/skills', 'folder')] };
  }
  if (relativePath === '.agent-workspaces/alice/.claude/skills') {
    return { root, entries: [entry('reviewer-kit', '.agent-workspaces/alice/.claude/skills/reviewer-kit', 'symlink', {
      symlinkTarget: '/Users/mock/Library/Application Support/Kota/skills/reviewer-kit',
    })] };
  }
  if (relativePath === '.agent-workspaces/alice/project-files') {
    return {
      root,
      entries: [
        entry('.git', '.agent-workspaces/alice/project-files/.git', 'file'),
        entry('src', '.agent-workspaces/alice/project-files/src', 'folder'),
      ],
    };
  }
  if (relativePath === '.agent-workspaces/alice/project-files/src') {
    return { root, entries: [entry('FileTree.tsx', '.agent-workspaces/alice/project-files/src/FileTree.tsx', 'file', {
      changeOverview: { added: 0, modified: 1, deleted: 0, untracked: 0 },
    })] };
  }
  return { root, entries: [] };
}

function mockWorkspaceDiffChanges(scope: WorkspaceDiffScope): WorkspaceDiffChangeEntry[] {
  const rootPath = '/tmp/kota-dev';
  const entries: WorkspaceDiffChangeEntry[] = [
    {
      path: 'agent-only.txt',
      absolutePath: `${rootPath}/agent-only.txt`,
      fileChange: {
        status: 'added',
        addedLines: 4,
        deletedLines: 0,
        participants: [{
          actorId: 'alice',
          displayName: 'Dr. Alice Liddell',
          aka: 'Dr. Alice',
          kind: 'agent',
          provider: 'claude',
          avatarId: 'claude',
          status: 'added',
          addedLines: 4,
          deletedLines: 0,
        }],
      },
    },
    {
      path: 'app-v2/package.json',
      absolutePath: `${rootPath}/app-v2/package.json`,
      fileChange: {
        status: 'modified',
        addedLines: 18,
        deletedLines: 7,
        participants: [
          {
            actorId: 'human',
            displayName: 'Human',
            aka: 'Human',
            kind: 'human',
            status: 'modified',
            addedLines: 8,
            deletedLines: 2,
          },
          {
            actorId: 'alice',
            displayName: 'Dr. Alice Liddell',
            aka: 'Dr. Alice',
            kind: 'agent',
            provider: 'claude',
            avatarId: 'claude',
            status: 'modified',
            addedLines: 10,
            deletedLines: 5,
          },
        ],
      },
    },
    {
      path: 'app-v2/src/chrome/old-panel.tsx',
      absolutePath: `${rootPath}/app-v2/src/chrome/old-panel.tsx`,
      fileChange: {
        status: 'deleted',
        addedLines: 0,
        deletedLines: 44,
        participants: [{
          actorId: 'human',
          displayName: 'Human',
          aka: 'Human',
          kind: 'human',
          status: 'deleted',
          addedLines: 0,
          deletedLines: 44,
        }],
      },
    },
    {
      path: 'project-memory/scratch/diff-medal-mock.html',
      absolutePath: `${rootPath}/project-memory/scratch/diff-medal-mock.html`,
      fileChange: {
        status: 'untracked',
        participants: [{
          actorId: 'alice',
          displayName: 'Dr. Alice Liddell',
          aka: 'Dr. Alice',
          kind: 'agent',
          provider: 'claude',
          avatarId: 'claude',
          status: 'untracked',
        }],
      },
    },
  ];
  if (scope.type === 'all') return entries;
  if (scope.type === 'file') return entries.filter((entry) => entry.path === scope.path);
  const prefix = scope.prefix.replace(/^\/+|\/+$/g, '');
  if (!prefix) return entries;
  return entries.filter((entry) => entry.path === prefix || entry.path.startsWith(`${prefix}/`));
}

function mockWorkspaceFileDiff(request: WorkspaceFileDiffRequest): WorkspaceFileDiffResult {
  const entry = mockWorkspaceDiffChanges({ type: 'file', path: request.relativePath })[0];
  const participants = entry?.fileChange.participants ?? [];
  return {
    path: request.relativePath,
    segments: participants
      .filter((participant) => !request.actorId || participant.actorId === request.actorId)
      .map((participant) => ({
        actorId: participant.actorId,
        displayName: participant.displayName,
        aka: participant.aka,
        kind: participant.kind,
        provider: participant.provider,
        avatarId: participant.avatarId,
        status: participant.status,
        addedLines: participant.addedLines,
        deletedLines: participant.deletedLines,
        binary: false,
        truncated: false,
        hunks: [{
          header: '@@ mock diff @@',
          omittedLines: 0,
          lines: participant.status === 'deleted'
            ? [{ kind: 'del', text: '  old mock line' }]
            : participant.status === 'untracked'
              ? [{ kind: 'add', text: '<section>untracked mock content</section>' }]
              : [
                { kind: 'ctx', text: '  "scripts": {' },
                { kind: 'del', text: '    "old": "vite --host 0.0.0.0",' },
                { kind: 'add', text: '    "test": "vitest run",' },
                { kind: 'ctx', text: '  }' },
              ],
        }],
      })),
  };
}

export async function syncVioletRoom(
  request: VioletRoomRequest = {},
): Promise<VioletRoomState> {
  if (!useTauriRuntime()) {
    const now = new Date().toISOString();
    const state = {
      messages: [
        {
          id: 'mock-violet-user',
          sessionId: 'mock-session',
          agentId: 'user',
          shell: 'mock',
          role: 'user',
          kind: 'message',
          timestamp: now,
          text: 'Mock Violet room message. Real builds read provider native logs.',
          sourcePath: null,
          nativeEventId: null,
        },
      ],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${request.projectRoot || '/tmp/kota-dev'}/project-memory/raw_logs`,
      chathistoryDir: `${request.projectRoot || '/tmp/kota-dev'}/project-memory/chathistory`,
      syncedAt: now,
    };
    emitVioletRoomSynced(request, state);
    return state;
  }
  const state = await invoke<VioletRoomState>('violet_room_sync', { request });
  emitVioletWorkEvents(state);
  emitVioletRoomSynced(request, state);
  return state;
}

export async function readVioletRoomCache(
  request: VioletRoomRequest = {},
): Promise<VioletRoomState> {
  if (!useTauriRuntime()) {
    return syncVioletRoom(request);
  }
  const state = await invoke<VioletRoomState>('violet_room_read_cache', { request });
  return state;
}

export async function onVioletRoomSynced(
  listener: (payload: VioletRoomSyncedEvent) => void,
): Promise<UnlistenFn> {
  if (typeof window === 'undefined') return async () => {};
  const handler = (event: globalThis.Event) => {
    listener((event as CustomEvent<VioletRoomSyncedEvent>).detail);
  };
  window.addEventListener(VIOLET_ROOM_SYNCED_EVENT, handler);
  return async () => window.removeEventListener(VIOLET_ROOM_SYNCED_EVENT, handler);
}

export async function onVioletRoomChanged(
  listener: (payload: VioletRoomChangedEvent) => void,
): Promise<UnlistenFn> {
  if (!useTauriRuntime()) return async () => {};
  return listen<VioletRoomChangedEvent>(
    VIOLET_ROOM_CHANGED_EVENT,
    (event: Event<VioletRoomChangedEvent>) => {
      listener(event.payload);
    },
  );
}

export async function setVioletPrivacy(
  request: VioletPrivacyRequest,
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('violet_privacy_set', { request });
}

export async function readVioletSummary(
  request: VioletSummaryRequest = {},
): Promise<VioletSummaryState> {
  if (!useTauriRuntime()) {
    const now = new Date().toISOString();
    const root = request.projectRoot || '/tmp/kota-dev';
    return {
      latest: null,
      history: [],
      outstanding: {
        sinceTs: null,
        messageCount: 0,
      },
      logPath: `${root}/project-memory/chathistory/summaries/recent.json`,
      promptPath: '$KOTA_HOME/heroes/system-violet/violet-summary.md',
      updatedAt: now,
    };
  }
  return invoke<VioletSummaryState>('violet_summary_status', { request });
}

export async function summarizeVioletNow(
  request: VioletSummaryRequest,
): Promise<VioletSummaryState> {
  if (!useTauriRuntime()) {
    return readVioletSummary(request);
  }
  return invoke<VioletSummaryState>('violet_summary_now', { request });
}

export async function summarizeVioletAuto(
  request: VioletSummaryRequest,
): Promise<VioletSummaryState> {
  if (!useTauriRuntime()) {
    return readVioletSummary(request);
  }
  return invoke<VioletSummaryState>('violet_summary_auto_run', { request });
}

export async function revealTavernHeroFile(
  request: TavernHeroFileRequest,
): Promise<TavernHeroFileResult> {
  if (!useTauriRuntime()) {
    return {
      path: `~/Kota/heroes/${request.heroId}/${request.fileName}`,
    };
  }
  return invoke<TavernHeroFileResult>('tavern_write_and_reveal_hero_file', { request });
}

export async function readSystemPrompt(
  request: SystemPromptReadRequest,
  fallback: string,
): Promise<SystemPromptReadResult> {
  if (!useTauriRuntime()) {
    return {
      path: request.path,
      content: fallback.trimEnd(),
    };
  }
  return invoke<SystemPromptReadResult>('system_prompt_read', { request });
}

export async function resetSystemPrompt(
  request: SystemPromptReadRequest,
  fallback: string,
): Promise<SystemPromptReadResult> {
  if (!useTauriRuntime()) {
    return {
      path: request.path,
      content: fallback.trimEnd(),
    };
  }
  return invoke<SystemPromptReadResult>('system_prompt_reset', { request });
}

export async function saveTavernHeroProfiles(
  heroes: TavernHeroProfileDraft[],
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('tavern_save_hero_profiles', { request: { heroes } });
}

export async function deleteTavernHero(
  request: TavernHeroDeleteRequest,
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('tavern_delete_hero', { request });
}

export async function loadTavernHeroProfiles(): Promise<TavernHeroProfileDraft[]> {
  if (!useTauriRuntime()) return [];
  return invoke<TavernHeroProfileDraft[]>('tavern_load_hero_profiles');
}

export async function loadAccountUserIdentity(): Promise<AccountUserIdentity> {
  if (!useTauriRuntime()) return { name: 'User', avatarId: 'user-default' };
  return invoke<AccountUserIdentity>('account_user_identity_load');
}

export async function saveAccountUserIdentity(identity: AccountUserIdentity): Promise<AccountUserIdentity> {
  if (!useTauriRuntime()) return identity;
  return invoke<AccountUserIdentity>('account_user_identity_save', { identity });
}

export async function listAccountRules(): Promise<AccountRuleDraft[]> {
  if (!useTauriRuntime()) {
    return [
      {
        fileName: 'account-language-always.md',
        title: 'Account Language And Delivery',
        loadPolicy: 'always',
        taskTrigger: '',
        body: '# Account Language And Delivery\n\n- Reply in the same language as the user prompt.',
        path: '~/Kota/rules/account-language-always.md',
        bundledDefault: true,
        modified: false,
      },
      {
        fileName: 'rules-for-coding.md',
        title: 'Rules For Coding',
        loadPolicy: 'on-demand',
        taskTrigger: 'coding, debugging, refactoring, testing, reviewing, implementing, or modifying code',
        body: "Coding taste adapted from Andrej Karpathy's public coding guidance.\n\n- Keep changes narrow and maintainable.",
        path: '~/Kota/rules/rules-for-coding.md',
        bundledDefault: true,
        modified: false,
      },
    ];
  }
  return invoke<AccountRuleDraft[]>('account_rules_list');
}

export async function saveAccountRule(
  request: AccountRuleSaveRequest,
): Promise<AccountRuleDraft[]> {
  if (!useTauriRuntime()) return listAccountRules();
  return invoke<AccountRuleDraft[]>('account_rule_save', { request });
}

export async function deleteAccountRule(fileName: string): Promise<AccountRuleDraft[]> {
  if (!useTauriRuntime()) return listAccountRules();
  return invoke<AccountRuleDraft[]>('account_rule_delete', { request: { fileName } });
}

export async function resetDefaultAccountRules(): Promise<AccountRuleDraft[]> {
  if (!useTauriRuntime()) return listAccountRules();
  return invoke<AccountRuleDraft[]>('account_rules_reset_defaults');
}

let mockProjectRules: ProjectRuleDraft[] = [
  {
    fileName: 'project-context.md',
    title: 'Project Context',
    loadPolicy: 'always',
    taskTrigger: '',
    body: '- Project-specific guidance lives in project-rules.',
    path: '/tmp/kota-dev/project-rules/project-context.md',
    bundledDefault: false,
    modified: false,
  },
];

function mockProjectRulesDir(request: ProjectRulesRequest): string {
  return request.rulesDir?.trim() || `${request.projectRoot?.trim() || '/tmp/kota-dev'}/project-rules`;
}

function mockRuleFileName(title: string): string {
  const slug = title
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '') || 'project-rule';
  let fileName = `${slug}.md`;
  let suffix = 2;
  const existing = new Set(mockProjectRules.map((rule) => rule.fileName));
  while (existing.has(fileName)) {
    fileName = `${slug}-${suffix}.md`;
    suffix += 1;
  }
  return fileName;
}

export async function listProjectRules(request: ProjectRulesRequest): Promise<ProjectRuleDraft[]> {
  if (!useTauriRuntime()) {
    const rulesDir = mockProjectRulesDir(request);
    return mockProjectRules.map((rule) => ({ ...rule, path: `${rulesDir}/${rule.fileName}` }));
  }
  return invoke<ProjectRuleDraft[]>('project_rules_list', { request });
}

export async function saveProjectRule(request: ProjectRuleSaveRequest): Promise<ProjectRuleDraft[]> {
  if (!useTauriRuntime()) {
    const rulesDir = mockProjectRulesDir(request);
    const fileName = request.fileName || mockRuleFileName(request.title);
    const next: ProjectRuleDraft = {
      fileName,
      title: request.title,
      loadPolicy: request.loadPolicy,
      taskTrigger: request.loadPolicy === 'on-demand' ? request.taskTrigger?.trim() ?? '' : '',
      body: request.body,
      path: `${rulesDir}/${fileName}`,
      bundledDefault: false,
      modified: false,
    };
    mockProjectRules = [
      ...mockProjectRules.filter((rule) => rule.fileName !== fileName),
      next,
    ].sort((a, b) => a.title.localeCompare(b.title));
    return listProjectRules(request);
  }
  return invoke<ProjectRuleDraft[]>('project_rule_save', { request });
}

export async function deleteProjectRule(
  request: ProjectRulesRequest & { fileName: string },
): Promise<ProjectRuleDraft[]> {
  if (!useTauriRuntime()) {
    mockProjectRules = mockProjectRules.filter((rule) => rule.fileName !== request.fileName);
    return listProjectRules(request);
  }
  return invoke<ProjectRuleDraft[]>('project_rule_delete', { request });
}

const MOCK_ACCOUNT_SKILLS: AccountSkillDraft[] = [
  {
    id: 'frontend-design',
    name: 'frontend-design',
    description: 'Create distinctive, production-grade frontend interfaces with high design quality.',
    path: '~/Kota/skills/frontend-design',
    kind: 'builtin',
    bundledDefault: true,
    valid: true,
    createdAt: '2026-05-27T00:00:00Z',
  },
  {
    id: 'github',
    name: 'github',
    description: 'Triage and orient GitHub repository, pull request, and issue work through the connected GitHub app.',
    path: '~/Kota/skills/github',
    kind: 'builtin',
    bundledDefault: true,
    valid: true,
    createdAt: '2026-05-27T00:00:00Z',
  },
  {
    id: 'skill-creator',
    name: 'skill-creator',
    description: 'Create new skills, modify and improve existing skills, and measure skill performance.',
    path: '~/Kota/skills/skill-creator',
    kind: 'builtin',
    bundledDefault: true,
    valid: true,
    createdAt: '2026-05-27T00:00:00Z',
  },
  {
    id: 'test-app',
    name: 'test-app',
    description: 'Build a debug .app bundle of a Tauri 2 app and launch it for user testing.',
    path: '~/Kota/skills/test-app',
    kind: 'manual',
    bundledDefault: false,
    valid: true,
    createdAt: '2026-05-28T00:00:00Z',
  },
  {
    id: 'create-product-hub-ticket',
    name: 'create-product-hub-ticket',
    description: 'Create, query, search, update, delete, and attach files to Jira issues in Product Hub.',
    path: '~/Kota/skills/create-product-hub-ticket',
    kind: 'manual',
    bundledDefault: false,
    valid: true,
    createdAt: '2026-05-29T00:00:00Z',
  },
];

function sortAccountSkillsByCreatedAt(skills: AccountSkillDraft[]): AccountSkillDraft[] {
  return [...skills].sort((left, right) => (
    right.createdAt.localeCompare(left.createdAt)
    || left.name.localeCompare(right.name)
    || left.id.localeCompare(right.id)
  ));
}

export async function listAccountSkills(): Promise<AccountSkillDraft[]> {
  if (!useTauriRuntime()) return sortAccountSkillsByCreatedAt(MOCK_ACCOUNT_SKILLS);
  return invoke<AccountSkillDraft[]>('account_skills_list');
}

export async function deleteAccountSkill(skillId: string): Promise<AccountSkillDraft[]> {
  if (!useTauriRuntime()) {
    return sortAccountSkillsByCreatedAt(MOCK_ACCOUNT_SKILLS.filter((skill) => skill.id !== skillId));
  }
  return invoke<AccountSkillDraft[]>('account_skill_delete', { request: { skillId } });
}

export async function importAccountSkillArchive(file: File): Promise<AccountSkillImportResult> {
  if (!useTauriRuntime()) {
    const id = skillIdFromFileName(file.name);
    const imported: AccountSkillDraft = {
      id,
      name: id,
      description: 'Imported skill archive.',
      path: `~/Kota/skills/${id}`,
      kind: 'manual',
      bundledDefault: false,
      valid: true,
      createdAt: new Date().toISOString(),
    };
    return {
      skills: sortAccountSkillsByCreatedAt([
        ...MOCK_ACCOUNT_SKILLS,
        imported,
      ]),
      imported,
      message: `Imported skill "${imported.name}" (${imported.id}) into $KOTA_HOME/skills.`,
    };
  }
  const dataBase64 = await readFileAsBase64(file);
  return invoke<AccountSkillImportResult>('account_skill_import_archive', {
    request: {
      fileName: file.name,
      dataBase64,
    } satisfies AccountSkillImportArchiveRequest,
  });
}

export async function importAccountSkillFolder(files: File[]): Promise<AccountSkillImportResult> {
  if (files.length === 0) throw new Error('Choose a folder with a SKILL.md file.');
  const folderName = skillFolderName(files);
  const id = skillIdFromFileName(folderName);
  if (!useTauriRuntime()) {
    const imported: AccountSkillDraft = {
      id,
      name: id,
      description: 'Imported skill folder.',
      path: `~/Kota/skills/${id}`,
      kind: 'manual',
      bundledDefault: false,
      valid: true,
      createdAt: new Date().toISOString(),
    };
    return {
      skills: sortAccountSkillsByCreatedAt([
        ...MOCK_ACCOUNT_SKILLS,
        imported,
      ]),
      imported,
      message: `Imported skill "${imported.name}" (${imported.id}) into $KOTA_HOME/skills.`,
    };
  }
  const payloadFiles = await Promise.all(
    files.map(async (file) => ({
      relativePath: file.webkitRelativePath || file.name,
      dataBase64: await readFileAsBase64(file),
    } satisfies AccountSkillImportFolderFile)),
  );
  return invoke<AccountSkillImportResult>('account_skill_import_folder', {
    request: {
      folderName,
      files: payloadFiles,
    } satisfies AccountSkillImportFolderRequest,
  });
}

export async function importAccountSkillFromPicker(): Promise<AccountSkillImportResult | null> {
  if (!useTauriRuntime()) return null;
  const response = await invoke<AccountSkillImportPickerResult>('account_skill_import_from_picker');
  return response.result ?? null;
}

export async function openAccountSkillsFolder(): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('account_skills_open_folder');
}

export async function openAccountSkillFolder(skillId: string): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('account_skill_open_folder', { request: { skillId } });
}

export async function incarnateTavernHero(
  request: TavernIncarnateHeroRequest,
): Promise<TavernIncarnateHeroResult> {
  if (!useTauriRuntime()) {
    const projectRoot = request.projectRoot?.trim() || '/tmp/kota-dev';
    const mockSkillIds = new Set(MOCK_ACCOUNT_SKILLS.filter((skill) => skill.valid).map((skill) => skill.id));
    const matchedSkills = request.profile.skills.filter((skill) => mockSkillIds.has(skill));
    const missingSkills = request.profile.skills.filter((skill) => !mockSkillIds.has(skill));
    return {
      request: {
        agentId: request.agentId,
        cli: request.profile.provider as AgentSpawnRequest['cli'],
        cwd: `${projectRoot}/.agent-workspaces/${request.agentId}`,
        projectRoot,
        worktreeRoot: `${projectRoot}/.agent-workspaces/${request.agentId}/project-files`,
        sharedDir: `${projectRoot}/project-memory`,
        rulesDir: `${projectRoot}/project-rules`,
        adapterPath: `${projectRoot}/.agent-workspaces/${request.agentId}/AGENTS.md`,
        args: shellArgsFromText(request.profile.shell),
      },
      adapterPath: `${projectRoot}/.agent-workspaces/${request.agentId}/AGENTS.md`,
      shellPath: `${projectRoot}/.agent-workspaces/${request.agentId}/SHELL.yaml`,
      matchedSkills,
      missingSkills,
      projectRoot,
    };
  }
  return invoke<TavernIncarnateHeroResult>('tavern_incarnate_hero', { request });
}

export async function loadProjectAgentDetail(
  request: ProjectAgentRequest,
): Promise<ProjectAgentDetail> {
  if (!useTauriRuntime()) return mockProjectAgentDetail(request.agentId, request.projectRoot);
  return invoke<ProjectAgentDetail>('project_agent_load_detail', { request });
}

export async function commendProjectAgent(
  request: ProjectAgentCommendRequest,
): Promise<ProjectAgentRecord> {
  if (!useTauriRuntime()) {
    const key = `kota-v2.mock.commends.${request.projectRoot ?? 'default'}.${request.agentId}`;
    const next = Number(window.localStorage.getItem(key) ?? '0') + 1;
    window.localStorage.setItem(key, String(next));
    return {
      turns: 0,
      incarnations: 1,
      estimatedTokens: 0,
      commends: next,
      lastActiveAt: new Date().toISOString(),
    };
  }
  return invoke<ProjectAgentRecord>('project_agent_commend', { request });
}

export async function resolveProjectAgentLaunch(
  request: ProjectAgentRequest,
): Promise<ProjectAgentLaunchResolution> {
  if (!useTauriRuntime()) {
    const detail = await mockProjectAgentDetail(request.agentId, request.projectRoot);
    const projectRoot = request.projectRoot || '/tmp/kota-dev';
    return {
      status: 'ready',
      request: {
        agentId: detail.agentId,
        cli: detail.cli,
        cwd: `${projectRoot}/.agent-workspaces/${detail.agentId}`,
        projectRoot,
        worktreeRoot: `${projectRoot}/.agent-workspaces/${detail.agentId}/project-files`,
        sharedDir: `${projectRoot}/project-memory`,
        rulesDir: `${projectRoot}/project-rules`,
        adapterPath: detail.adapterPath,
        args: detail.args,
        sessionId: detail.sessionId,
      },
    };
  }
  return invoke<ProjectAgentLaunchResolution>('project_agent_resolve_launch', { request });
}

export async function startFreshProjectAgentSession(
  request: ProjectAgentRequest,
): Promise<ProjectAgentFreshSessionResult> {
  if (!useTauriRuntime()) {
    const detail = await mockProjectAgentDetail(request.agentId, request.projectRoot);
    const projectRoot = request.projectRoot || '/tmp/kota-dev';
    return {
      detail: { ...detail, sessionId: null, sessionSource: null },
      request: {
        agentId: detail.agentId,
        cli: detail.cli,
        cwd: `${projectRoot}/.agent-workspaces/${detail.agentId}`,
        projectRoot,
        worktreeRoot: `${projectRoot}/.agent-workspaces/${detail.agentId}/project-files`,
        sharedDir: `${projectRoot}/project-memory`,
        rulesDir: `${projectRoot}/project-rules`,
        adapterPath: detail.adapterPath,
        args: detail.args,
        sessionId: null,
      },
    };
  }
  return invoke<ProjectAgentFreshSessionResult>('project_agent_start_fresh_session', { request });
}

export async function clearProjectAgentSessionMetadata(
  request: ProjectAgentRequest,
): Promise<ProjectAgentDetail> {
  if (!useTauriRuntime()) {
    const detail = await mockProjectAgentDetail(request.agentId, request.projectRoot);
    return { ...detail, sessionId: null, sessionSource: null };
  }
  return invoke<ProjectAgentDetail>('project_agent_clear_session_metadata', { request });
}

export async function saveProjectAgentDetail(
  request: ProjectAgentSaveRequest,
): Promise<ProjectAgentDetail> {
  if (!useTauriRuntime()) return mockProjectAgentDetail(request.agentId, request.projectRoot, request);
  return invoke<ProjectAgentDetail>('project_agent_save_detail', { request });
}

export async function archiveProjectAgent(
  request: ProjectAgentLifecycleRequest,
): Promise<ProjectAgentLifecycleResult> {
  if (!useTauriRuntime()) {
    return { ok: true, dirty: false, dirtySummary: '', detail: await mockProjectAgentDetail(request.agentId, request.projectRoot) };
  }
  return invoke<ProjectAgentLifecycleResult>('project_agent_archive', { request });
}

export async function callBackProjectAgent(
  request: ProjectAgentRequest,
): Promise<ProjectAgentDetail> {
  if (!useTauriRuntime()) return mockProjectAgentDetail(request.agentId, request.projectRoot);
  return invoke<ProjectAgentDetail>('project_agent_call_back', { request });
}

export async function dismissProjectAgent(
  request: ProjectAgentLifecycleRequest,
): Promise<ProjectAgentLifecycleResult> {
  if (!useTauriRuntime()) return { ok: true, dirty: false, dirtySummary: '', detail: null };
  return invoke<ProjectAgentLifecycleResult>('project_agent_dismiss', { request });
}

export async function listArchivedProjectAgents(
  projectRoot?: string | null,
): Promise<ProjectAgentDetail[]> {
  if (!useTauriRuntime()) return [];
  return invoke<ProjectAgentDetail[]>('project_agent_list_archived', { projectRoot: projectRoot ?? null });
}

export async function listProjectAgentIdentities(
  projectRoot?: string | null,
): Promise<ProjectAgentIdentity[]> {
  return (await inspectProjectAgentIdentities(projectRoot)).identities;
}

export async function inspectProjectAgentIdentities(
  projectRoot?: string | null,
): Promise<ProjectAgentIdentityListing> {
  if (!useTauriRuntime()) return { identities: [], workspaceEntryCount: 0 };
  return invoke<ProjectAgentIdentityListing>('project_agent_list_identities', {
    projectRoot: projectRoot ?? null,
  });
}

export interface ProjectAgentLayoutFile {
  version: number;
  projectRoot: string;
  updatedAt: string;
  tableSlots: (string | null)[];
}

export async function loadProjectAgentLayoutFile(
  projectRoot?: string | null,
): Promise<ProjectAgentLayoutFile | null> {
  if (!useTauriRuntime()) return null;
  return invoke<ProjectAgentLayoutFile | null>('project_agent_layout_load', { projectRoot: projectRoot ?? null });
}

export async function saveProjectAgentLayoutFile(
  projectRoot: string | null,
  tableSlots: readonly (string | null)[],
): Promise<void> {
  if (!useTauriRuntime()) return;
  await invoke('project_agent_layout_save', { projectRoot, tableSlots: [...tableSlots] });
}

export async function inviteProjectAgentToTavern(
  request: ProjectAgentInviteRequest,
): Promise<ProjectAgentInviteResult> {
  if (!useTauriRuntime()) {
    const detail = await mockProjectAgentDetail(request.agentId, request.projectRoot);
    return {
      heroId: detail.inviteEligibility.proposedHeroId,
      displayName: request.displayName || detail.inviteEligibility.proposedDisplayName,
      path: `~/Kota/heroes/${detail.inviteEligibility.proposedHeroId}`,
      duplicateHeroId: null,
    };
  }
  return invoke<ProjectAgentInviteResult>('project_agent_invite_to_tavern', { request });
}

export async function kageBunshinProjectAgent(
  request: ProjectAgentRequest,
): Promise<ProjectAgentBunshinResult> {
  if (!useTauriRuntime()) {
    const detail = await mockProjectAgentDetail(`${request.agentId}-bunshin`, request.projectRoot);
    return {
      detail,
      request: {
        agentId: detail.agentId,
        cli: detail.cli,
        cwd: `${request.projectRoot || '/tmp/kota-dev'}/.agent-workspaces/${detail.agentId}`,
        projectRoot: request.projectRoot || '/tmp/kota-dev',
        args: [],
      },
    };
  }
  return invoke<ProjectAgentBunshinResult>('project_agent_kage_bunshin', { request });
}

function shellArgsFromText(shell: string): string[] {
  const args: string[] = [];
  const lines = shell.split(/\r?\n/);
  const start = lines.findIndex((line) => line.trim() === 'args:');
  if (start < 0) return args;
  for (const line of lines.slice(start + 1)) {
    if (!/^\s+-\s+/.test(line)) {
      if (/^\S/.test(line)) break;
      continue;
    }
    const raw = line.replace(/^\s+-\s+/, '').trim();
    try {
      args.push(JSON.parse(raw));
    } catch {
      args.push(raw.replace(/^['"]|['"]$/g, ''));
    }
  }
  return args;
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error ?? new Error('Unable to read file.'));
    reader.onload = () => {
      const result = typeof reader.result === 'string' ? reader.result : '';
      const comma = result.indexOf(',');
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.readAsDataURL(file);
  });
}

function skillIdFromFileName(fileName: string): string {
  const stem = archiveStem(fileName);
  const id = stem
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '');
  return id || 'imported-skill';
}

function archiveStem(fileName: string): string {
  const lower = fileName.toLowerCase();
  if (lower.endsWith('.tar.gz')) return fileName.slice(0, -7).trim();
  if (lower.endsWith('.tgz')) return fileName.slice(0, -4).trim();
  return fileName.replace(/\.[^.]+$/, '').trim();
}

function skillFolderName(files: File[]): string {
  const firstPath = files[0]?.webkitRelativePath;
  const firstSegment = firstPath?.split('/').find(Boolean);
  return firstSegment || files[0]?.name || 'imported-skill';
}

function mockProjectAgentDetail(
  agentId: string,
  projectRoot?: string | null,
  patch?: Partial<ProjectAgentSaveRequest>,
): ProjectAgentDetail {
  const root = projectRoot || '/tmp/kota-dev';
  const baseName = patch?.displayName || agentId;
  return {
    agentId,
    displayName: baseName,
    nameFields: patch?.nameFields ?? null,
    sourceHeroId: 'hero-dex',
    sourceHeroName: baseName.split(' v. ')[0] || baseName,
    projectId: 'mock-project',
    projectName: 'MockProject',
    cli: 'codex',
    provider: 'codex',
    model: patch?.model || 'gpt-5.5',
    effort: patch?.effort || 'xhigh',
    avatarId: patch?.avatarId || 'codex',
    skills: patch?.skills || ['frontend-design', 'test-app'],
    args: ['--model', patch?.model || 'gpt-5.5'],
    ghost: patch?.ghost || `# ${baseName}\n\nMock project incarnation.`,
    adapterPath: `${root}/.agent-workspaces/${agentId}/AGENTS.md`,
    shellPath: `${root}/.agent-workspaces/${agentId}/SHELL.yaml`,
    agentYamlPath: `${root}/.agent-workspaces/${agentId}/agent.yaml`,
    status: 'active',
    archivedAt: null,
    inviteEligibility: {
      eligible: true,
      proposedHeroId: `${agentId}-v-mockproject`,
      proposedDisplayName: `${baseName.split(' v. ')[0] || baseName} v. MockProject`,
    },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0, commends: 0, lastActiveAt: null },
    sessionId: null,
    forkable: false,
    sessionSource: null,
    dirty: false,
    dirtySummary: '',
  };
}

// ─────────────────────────────────────────── Composer attachments ─

export interface ComposerAttachmentMaterializeOptions {
  projectRoot?: string | null;
}

export interface WhiteboardCanvasLoadResult {
  canvasDir: string;
  manifestPath: string;
  pagePath: string;
  pageId: string;
  pages: Array<{
    id: string;
    title: string;
    path: string;
    modifiedMs: number;
  }>;
  dataJson: string;
  modifiedMs: number;
}

export interface WhiteboardCanvasSnapshotResult {
  pagePath: string;
  snapshotPath: string;
}

function devComposerAttachmentPath(
  name: string,
  options: ComposerAttachmentMaterializeOptions = {},
): string {
  const root = (options.projectRoot || '/tmp/kota-project').replace(/\/+$/, '');
  return `${root}/project-memory/attachments/composer/dev/${name || 'attachment'}`;
}

export async function saveComposerClipboardImage(
  file: File,
  options: ComposerAttachmentMaterializeOptions = {},
): Promise<string | null> {
  const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
  if (bytes.length === 0) return null;
  if (!useTauriRuntime()) {
    return devComposerAttachmentPath(file.name || 'clipboard-image.png', options);
  }
  return invoke<string>('save_composer_clipboard_image', {
    projectRoot: options.projectRoot ?? null,
    fileName: file.name || null,
    mime: file.type || null,
    bytes,
  });
}

export async function materializeComposerAttachmentPath(
  path: string,
  options: ComposerAttachmentMaterializeOptions = {},
): Promise<string> {
  if (!path) return path;
  if (!useTauriRuntime()) {
    const name = path.split('/').pop() || 'attachment';
    return devComposerAttachmentPath(name, options);
  }
  return invoke<string>('materialize_composer_attachment_path', {
    projectRoot: options.projectRoot ?? null,
    sourcePath: path,
  });
}

function devWhiteboardCanvas(projectRoot?: string | null): WhiteboardCanvasLoadResult {
  const root = (projectRoot || '/tmp/kota-project').replace(/\/+$/, '');
  return {
    canvasDir: `${root}/project-memory/canvas`,
    manifestPath: `${root}/project-memory/canvas/manifest.json`,
    pagePath: `${root}/project-memory/canvas/pages/page-001.excalidraw`,
    pageId: 'page-001',
    pages: [{
      id: 'page-001',
      title: 'Page 1',
      path: `${root}/project-memory/canvas/pages/page-001.excalidraw`,
      modifiedMs: Date.now(),
    }],
    dataJson: JSON.stringify({
      type: 'excalidraw',
      version: 2,
      source: 'https://kota.local',
      elements: [],
      appState: { viewBackgroundColor: '#f5f0e5', gridSize: null },
      files: {},
    }),
    modifiedMs: Date.now(),
  };
}

export async function loadWhiteboardCanvas(
  projectRoot?: string | null,
  pagePath?: string | null,
): Promise<WhiteboardCanvasLoadResult> {
  if (!useTauriRuntime()) {
    return devWhiteboardCanvas(projectRoot);
  }
  return invoke<WhiteboardCanvasLoadResult>('whiteboard_canvas_load', {
    projectRoot: projectRoot ?? null,
    pagePath: pagePath ?? null,
  });
}

export async function saveWhiteboardCanvas(
  request: {
    projectRoot?: string | null;
    pagePath: string;
    dataJson: string;
  },
): Promise<WhiteboardCanvasLoadResult> {
  if (!useTauriRuntime()) {
    return {
      ...devWhiteboardCanvas(request.projectRoot),
      pagePath: request.pagePath,
      dataJson: request.dataJson,
      modifiedMs: Date.now(),
    };
  }
  return invoke<WhiteboardCanvasLoadResult>('whiteboard_canvas_save', {
    projectRoot: request.projectRoot ?? null,
    pagePath: request.pagePath,
    dataJson: request.dataJson,
  });
}

export async function renameWhiteboardCanvasPage(
  request: {
    projectRoot?: string | null;
    pagePath: string;
    title: string;
  },
): Promise<WhiteboardCanvasLoadResult> {
  if (!useTauriRuntime()) {
    const canvas = devWhiteboardCanvas(request.projectRoot);
    return {
      ...canvas,
      pagePath: request.pagePath,
      pages: canvas.pages.map((page) => (
        page.path === request.pagePath ? { ...page, title: request.title.trim() || page.title } : page
      )),
      modifiedMs: Date.now(),
    };
  }
  return invoke<WhiteboardCanvasLoadResult>('whiteboard_canvas_rename_page', {
    projectRoot: request.projectRoot ?? null,
    pagePath: request.pagePath,
    title: request.title,
  });
}

export async function saveWhiteboardCanvasSnapshot(
  request: {
    projectRoot?: string | null;
    pagePath: string;
    pngBytes: number[];
  },
): Promise<WhiteboardCanvasSnapshotResult> {
  if (!useTauriRuntime()) {
    const base = request.pagePath.replace(/\/pages\/[^/]+$/, '/snapshots');
    return {
      pagePath: request.pagePath,
      snapshotPath: `${base}/snap_dev_${Date.now().toString(36)}.png`,
    };
  }
  return invoke<WhiteboardCanvasSnapshotResult>('whiteboard_canvas_snapshot', {
    projectRoot: request.projectRoot ?? null,
    pagePath: request.pagePath,
    pngBytes: request.pngBytes,
  });
}
