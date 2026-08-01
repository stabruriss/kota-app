/** Smart Terminal — user's persistent shell stripe at the bottom of
 *  the app, crossing all three workspace columns.
 *
 *  Multi-tab (per `06-smart-terminal-tabs.md`): each tab is an
 *  independent PTY (via `pty-client.ts` multi-PTY API). Tab record
 *  carries its own scrollback, input, NL state, status, TUI flag,
 *  and unread counter. Switching tabs swaps which record drives the
 *  visible scrollback / prompt / header CWD.
 *
 *  Phase-2 deferrals (not implemented here): tab rename, drag-to-
 *  reorder. Everything else from the CD spec is in.
 *
 *  Design source:
 *    product-design/Smart Terminal.md (canonical spec)
 *    .context/design-brief/06-smart-terminal-tabs.md (multi-tab CD brief)
 *    Kota UI-handoff-v1.zip / shells/smart-terminal-tabs.html         */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CompositionEvent as ReactCompositionEvent,
  type ClipboardEvent as ReactClipboardEvent,
  type FormEvent as ReactFormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type WheelEvent as ReactWheelEvent,
} from 'react';
import { flushSync } from 'react-dom';
import {
  clearSmartPty,
  closeSmartPty,
  interruptSmartPty,
  onSmartExit,
  onSmartOutput,
  onSmartStatus,
  onSmartTuiExit,
  resizeSmartPty,
  resolveCli,
  restartSmartPty,
  scrollSmartPty,
  spawnSmartPty,
  translateNlPrompt,
  writeSmartPty,
} from '../pty-client';
import {
  loadMagiProvider,
  magiHandoffArgs,
  normalizeMagiProvider,
} from '../magi-config';
import type {
  FallbackCli,
  MagiProvider,
  NLMode,
  ScrollbackLine,
  ShellStatus,
} from '../types/smart-terminal';
import { TerminalGrid } from './TerminalGrid';
import type { GridSnapshot } from '../types/agent-pty';
import avatarMagi from '../assets/tavern/optimized/avatars/magi.webp';

// ── localStorage keys ────────────────────────────────────────────
const LS_EXPANDED = 'kota-v2.st.expanded';
const LS_HEIGHT = 'kota-v2.st.height';
const LS_TABS = 'kota-v2.st.tabs';
const MIN_HEIGHT = 180;
const MAX_HEIGHT_VH = 0.82;
const DEFAULT_HEIGHT = 380;
const DEFAULT_CWD = '~';
const MAX_SCROLLBACK_LINES = 5000;
const TERM_WHEEL_LINE_HEIGHT = 15;
const TERM_WHEEL_CELL_WIDTH = 7.2;

const isToggleShortcut = (e: KeyboardEvent) =>
  (e.metaKey || e.ctrlKey) && e.key === '`';

function wheelMagnitudePx(e: ReactWheelEvent): number {
  if (e.deltaMode === 1) return Math.abs(e.deltaY) * TERM_WHEEL_LINE_HEIGHT;
  if (e.deltaMode === 2) return Math.abs(e.deltaY) * TERM_WHEEL_LINE_HEIGHT * 8;
  return Math.abs(e.deltaY);
}

function wheelScrollLines(e: ReactWheelEvent): number {
  const steps = Math.max(1, Math.min(8, Math.ceil(wheelMagnitudePx(e) / TERM_WHEEL_LINE_HEIGHT)));
  return e.deltaY < 0 ? steps : -steps;
}

function wheelMouseBytes(e: ReactWheelEvent, grid: GridSnapshot, host: HTMLElement): string {
  const rect = host.getBoundingClientRect();
  const style = window.getComputedStyle(host);
  const padLeft = Number.parseFloat(style.paddingLeft || '0') || 0;
  const padTop = Number.parseFloat(style.paddingTop || '0') || 0;
  const col = Math.max(
    1,
    Math.min(grid.cols, Math.floor((e.clientX - rect.left - padLeft) / TERM_WHEEL_CELL_WIDTH) + 1),
  );
  const row = Math.max(
    1,
    Math.min(grid.rows, Math.floor((e.clientY - rect.top - padTop) / TERM_WHEEL_LINE_HEIGHT) + 1),
  );
  const button = e.deltaY < 0 ? 64 : 65;
  const count = Math.max(1, Math.min(4, Math.ceil(wheelMagnitudePx(e) / 80)));

  if (grid.sgrMouse !== false) {
    return Array.from({ length: count }, () => `\x1b[<${button};${col};${row}M`).join('');
  }

  const legacy = `\x1b[M${String.fromCharCode(32 + button)}${String.fromCharCode(32 + col)}${String.fromCharCode(32 + row)}`;
  return legacy.repeat(count);
}

interface PersistedTab {
  tabId: string;
  cwd: string;
  /** Optional user-chosen label (Phase 2). For now derived from cwd + tuiRunning. */
  label?: string;
}

type PromptFocusMode = 'shell' | 'smart';

interface TabRuntime {
  tabId: string;
  ptyId: string;
  cwd: string;
  customLabel: string | null;
  /** Latest GridSnapshot from `pty/smart.rs` (alacritty Term grid). */
  grid: GridSnapshot | null;
  /** Mock-only fallback used by Vitest fixtures and the initial chrome
   *  boot before the first PTY snapshot arrives. Real PTY sessions
   *  ignore this path and render via `grid` + TerminalGrid.            */
  scrollback: ScrollbackLine[];
  /** Natural-language text in the bottom Smart command row. */
  input: string;
  /** Best-effort copy of the shell readline buffer while Kota owns the
   *  keystream. It lets us detect direct `claude` / `codex` launches
   *  before the line is submitted, while still leaving the visible
   *  cursor and echo in the real PTY. */
  shellDraft: string;
  focusMode: PromptFocusMode;
  nlMode: NLMode;
  escapeHint: string | null;
  shellStatus: ShellStatus;
  tuiRunning: FallbackCli | null;
  unread: number;
  prefillSelected: boolean;
  sweep: boolean;
  /** Original Smart command prompt waiting to be inserted into the agent
   *  CLI (claude / codex) after it finishes spawning. Settled by
   *  watching grid stillness — once the grid stops changing for a
   *  second the agent has cleared its trust-directory + OS-auth
   *  prompts and rendered its own input box, so we paste the user's
   *  original ask there. Cancelled if the user starts typing first. */
  pendingHandoffAsk: string | null;
}

interface ContextMenuState {
  tabId: string;
  x: number;
  y: number;
}

// ── helpers ──────────────────────────────────────────────────────
function loadBool(key: string, fallback: boolean): boolean {
  try {
    const raw = localStorage.getItem(key);
    return raw === 'true' ? true : raw === 'false' ? false : fallback;
  } catch { return fallback; }
}
function loadNum(key: string, fallback: number): number {
  try {
    const raw = localStorage.getItem(key);
    const n = raw ? Number(raw) : NaN;
    return Number.isFinite(n) ? n : fallback;
  } catch { return fallback; }
}
function loadPersistedTabs(): PersistedTab[] {
  try {
    const raw = localStorage.getItem(LS_TABS);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((t): t is PersistedTab =>
        t && typeof t.tabId === 'string' && typeof t.cwd === 'string')
      .slice(0, 32);
  } catch { return []; }
}
function persistTabs(tabs: TabRuntime[]) {
  try {
    const slim: PersistedTab[] = tabs.map((t) => ({
      tabId: t.tabId,
      cwd: t.cwd,
      label: t.customLabel ?? undefined,
    }));
    localStorage.setItem(LS_TABS, JSON.stringify(slim));
  } catch { /* ignore */ }
}
function makeTabId(): string {
  if (typeof crypto !== 'undefined' && 'randomUUID' in crypto) {
    return crypto.randomUUID();
  }
  return `tab-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
function trimScrollback(lines: ScrollbackLine[]): ScrollbackLine[] {
  if (lines.length <= MAX_SCROLLBACK_LINES) return lines;
  return lines.slice(lines.length - MAX_SCROLLBACK_LINES);
}
/** Map a key event from the smart-terminal input to terminal bytes for
 *  TUI passthrough. Same translation table as InputBar.keyEventToBytes. */
function inputKeyToBytes(
  e: ReactKeyboardEvent<HTMLInputElement>,
  activeCli: FallbackCli | null,
): string | null {
  if (['Shift', 'Meta', 'Control', 'Alt', 'CapsLock'].includes(e.key)) return null;
  if (e.key === 'ArrowUp') return '\x1b[A';
  if (e.key === 'ArrowDown') return '\x1b[B';
  if (e.key === 'ArrowRight') return '\x1b[C';
  if (e.key === 'ArrowLeft') return '\x1b[D';
  if (e.key === 'Enter') return activeCli === 'kimi' ? '\x1b[13;1u' : '\r';
  if (e.key === 'Tab') return '\t';
  if (e.key === 'Backspace') return '\x7f';
  if (e.key === 'Delete') return '\x1b[3~';
  if (e.key === 'Escape') return '\x1b';
  if (e.key === 'Home') return '\x1b[H';
  if (e.key === 'End') return '\x1b[F';
  if (e.key === 'PageUp') return '\x1b[5~';
  if (e.key === 'PageDown') return '\x1b[6~';
  if (e.ctrlKey && e.key.length === 1) {
    const c = e.key.toLowerCase().charCodeAt(0);
    if (c >= 97 && c <= 122) return String.fromCharCode(c - 96);
  }
  if (!e.metaKey && !e.ctrlKey && !e.altKey && e.key.length === 1) return e.key;
  return null;
}
function normalizeTerminalText(text: string): string {
  return text.replace(/\r\n/g, '\r').replace(/\n/g, '\r');
}
/** Tail of cwd, suitable for tab label. `~/Projects/quito` → `~/quito` is
 *  too aggressive; just return the last two segments unless the path is
 *  short. */
function cwdTail(cwd: string): string {
  if (cwd.length <= 18) return cwd;
  const parts = cwd.split('/');
  if (parts.length >= 2) {
    const tail = `…/${parts.slice(-2).join('/')}`;
    return tail.length < cwd.length ? tail : cwd;
  }
  return cwd;
}
function deriveLabel(tab: TabRuntime): string {
  if (tab.customLabel) return tab.customLabel;
  if (tab.tuiRunning) return `${cwdTail(tab.cwd)} · ${tab.tuiRunning}`;
  return cwdTail(tab.cwd);
}
function tuiDisplayName(cli: FallbackCli): string {
  if (cli === 'claude') return 'Claude Code';
  if (cli === 'codex') return 'Codex';
  if (cli === 'agy') return 'Antigravity';
  if (cli === 'opencode') return 'OpenCode';
  if (cli === 'pi') return 'Pi';
  if (cli === 'kimi') return 'Kimi Code';
  return cli;
}
function nextShellDraftAfterText(draft: string, text: string): string {
  const normalized = normalizeTerminalText(text);
  const parts = normalized.split('\r');
  if (parts.length > 1) return parts[parts.length - 1] ?? '';
  return draft + normalized;
}

// ─────────────────────────────────────────────────────────────────
//  Component
// ─────────────────────────────────────────────────────────────────
export function SmartTerminal() {
  const [expanded, setExpanded] = useState<boolean>(() => loadBool(LS_EXPANDED, false));
  const [height, setHeight] = useState<number>(() => loadNum(LS_HEIGHT, DEFAULT_HEIGHT));
  const [tabs, setTabs] = useState<TabRuntime[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [confirmCloseTabId, setConfirmCloseTabId] = useState<string | null>(null);
  const [confirmAnchor, setConfirmAnchor] = useState<{ right: number; bottom: number } | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  // Session-scoped flag: ⇧-click on × bypasses subsequent confirms.
  const [skipCloseConfirm, setSkipCloseConfirm] = useState(false);

  const smartInputRef = useRef<HTMLInputElement | null>(null);
  const shellInputRef = useRef<HTMLInputElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const scrollbackRef = useRef<HTMLDivElement | null>(null);
  const tabsRef = useRef<TabRuntime[]>(tabs);
  const activeIdRef = useRef<string | null>(activeTabId);
  const expandedRef = useRef(expanded);

  useEffect(() => { tabsRef.current = tabs; }, [tabs]);
  useEffect(() => { activeIdRef.current = activeTabId; }, [activeTabId]);
  useEffect(() => { expandedRef.current = expanded; }, [expanded]);

  // Persist panel-level state.
  useEffect(() => {
    try { localStorage.setItem(LS_EXPANDED, String(expanded)); } catch { /* ignore */ }
  }, [expanded]);
  useEffect(() => {
    try { localStorage.setItem(LS_HEIGHT, String(height)); } catch { /* ignore */ }
  }, [height]);
  // Persist tab metadata when tabs change (slim — not scrollback).
  useEffect(() => { persistTabs(tabs); }, [tabs]);

  // ── tab mutators ────────────────────────────────────────────────
  const updateTab = useCallback(
    (id: string, mutator: (t: TabRuntime) => Partial<TabRuntime>) => {
      setTabs((prev) =>
        prev.map((t) => (t.tabId === id ? { ...t, ...mutator(t) } : t)));
    },
    [],
  );
  const updateTabByPty = useCallback(
    (ptyId: string, mutator: (t: TabRuntime) => Partial<TabRuntime>) => {
      setTabs((prev) =>
        prev.map((t) => (t.ptyId === ptyId ? { ...t, ...mutator(t) } : t)));
    },
    [],
  );

  // ── pty event subscriptions (one window-level listener pattern) ─
  useEffect(() => {
    const unlisten: Array<() => void | Promise<void>> = [];
    let mounted = true;

    const attach = async () => {
      unlisten.push(await onSmartOutput((evt) => {
        if (!mounted) return;
        updateTabByPty(evt.ptyId, (t) => {
          // Per M6.A grid refactor: snapshot REPLACES whatever was on
          // the screen (alacritty Term is the SoT). Unread count goes
          // up on each fresh snapshot when this tab isn't visible —
          // crude heuristic but enough for "something happened" badge.
          const isActive = activeIdRef.current === t.tabId && expandedRef.current;
          const unreadDelta = isActive ? 0 : 1;
          return {
            grid: evt.snapshot,
            unread: t.unread + unreadDelta,
          };
        });
      }));

      unlisten.push(await onSmartExit((evt) => {
        if (!mounted) return;
        updateTabByPty(evt.ptyId, () => ({
          shellStatus: 'dead',
          tuiRunning: null,
          pendingHandoffAsk: null,
        }));
      }));

      unlisten.push(await onSmartStatus((evt) => {
        if (!mounted) return;
        updateTabByPty(evt.ptyId, (t) => ({
          cwd: evt.cwd,
          shellStatus: evt.running
            ? (t.tuiRunning ? 'live' : 'quiet')
            : 'dead',
        }));
      }));

      // Foreground process group reclaim — fires when the user `/exit`s
      // a TUI agent (claude / codex) and control returns to the parent
      // shell. The shell itself isn't dead so onSmartExit doesn't fire;
      // only this signal lets us drop `tuiRunning` and restore the
      // bottom smart-input. Computed in the Rust emitter via
      // tcgetpgrp on the master fd — see `SMART_TUI_EXIT_EVENT`.
      unlisten.push(await onSmartTuiExit((evt) => {
        if (!mounted) return;
        updateTabByPty(evt.ptyId, (t) => {
          if (!t.tuiRunning) return {};
          return {
            tuiRunning: null,
            pendingHandoffAsk: null,
            shellStatus: 'quiet',
          };
        });
      }));
    };

    void attach();
    return () => {
      mounted = false;
      for (const stop of unlisten) void stop();
    };
  }, [updateTabByPty]);

  // ── initial spawn (rehydrate persisted tabs, or create one default) ─
  const spawnedRef = useRef(false);
  useEffect(() => {
    if (spawnedRef.current) return;
    spawnedRef.current = true;

    const persisted = loadPersistedTabs();
    const records = persisted.length > 0
      ? persisted
      : [{ tabId: makeTabId(), cwd: DEFAULT_CWD }];

    void Promise.all(records.map(async (rec) => {
      const ptyId = await spawnSmartPty({ cwd: rec.cwd === '~' ? undefined : rec.cwd });
      const tab: TabRuntime = {
        tabId: rec.tabId,
        ptyId,
        cwd: rec.cwd,
        customLabel: rec.label ?? null,
        scrollback: [],
        grid: null,
        input: '',
        shellDraft: '',
        focusMode: 'shell',
        nlMode: 'shell',
        escapeHint: null,
        shellStatus: 'quiet',
        tuiRunning: null,
        unread: 0,
        prefillSelected: false,
        sweep: false,
        pendingHandoffAsk: null,
      };
      return tab;
    })).then((newTabs) => {
      setTabs(newTabs);
      if (newTabs.length > 0) setActiveTabId(newTabs[0]!.tabId);
    });
  }, []);

  const activeTab = useMemo(
    () => tabs.find((t) => t.tabId === activeTabId) ?? null,
    [tabs, activeTabId],
  );

  // ── Pin scrollback to bottom on update (active tab only). ──────
  useEffect(() => {
    const el = scrollbackRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [activeTab?.scrollback, activeTab?.tabId, expanded]);

  // Clear unread when tab becomes active + panel expanded.
  useEffect(() => {
    if (!expanded || !activeTabId) return;
    updateTab(activeTabId, () => ({ unread: 0 }));
  }, [expanded, activeTabId, updateTab]);

  const focusShellCapture = useCallback(() => {
    const focus = () => shellInputRef.current?.focus();
    focus();
    window.requestAnimationFrame(focus);
  }, []);

  const focusSmartInput = useCallback(() => {
    const focus = () => smartInputRef.current?.focus();
    focus();
    window.requestAnimationFrame(focus);
  }, []);

  const activateShellFocus = useCallback((tabId: string) => {
    updateTab(tabId, (t) => ({
      focusMode: 'shell',
      nlMode: t.nlMode === 'thinking' ? t.nlMode : 'shell',
      escapeHint: null,
      prefillSelected: false,
    }));
    focusShellCapture();
  }, [focusShellCapture, updateTab]);

  const activateSmartFocus = useCallback((tabId: string) => {
    updateTab(tabId, (t) => {
      if (t.tuiRunning || t.nlMode === 'thinking') return {};
      return {
        focusMode: 'smart',
        nlMode: 'typing',
        escapeHint: null,
        prefillSelected: false,
      };
    });
    focusSmartInput();
  }, [focusSmartInput, updateTab]);

  // Focus the active input on tab switch / expand / TUI toggle. The
  // default focus is the real shell capture; the bottom Smart command
  // row only receives focus after an explicit click.
  useEffect(() => {
    if (!expanded || !activeTab) return;
    const target = activeTab.tuiRunning || activeTab.focusMode === 'shell'
      ? shellInputRef.current
      : smartInputRef.current;
    target?.focus();
    const raf = window.requestAnimationFrame(() => {
      target?.focus();
    });
    return () => window.cancelAnimationFrame(raf);
  }, [expanded, activeTab?.tabId, activeTab?.focusMode, activeTab?.tuiRunning]);

  // Select pre-filled translation result.
  useEffect(() => {
    if (activeTab?.prefillSelected && smartInputRef.current) {
      smartInputRef.current.select();
    }
  }, [activeTab?.prefillSelected, activeTab?.input]);

  // Brass underline sweep for ~1s after appearance.
  useEffect(() => {
    if (!activeTab?.sweep) return;
    const aid = activeTab.tabId;
    const t = window.setTimeout(() => {
      updateTab(aid, () => ({ sweep: false }));
    }, 1000);
    return () => window.clearTimeout(t);
  }, [activeTab?.sweep, activeTab?.tabId, updateTab]);

  // ── tab actions ────────────────────────────────────────────────
  const newTab = useCallback(async (cwd?: string) => {
    const tabId = makeTabId();
    const ptyId = await spawnSmartPty({ cwd });
    const tab: TabRuntime = {
      tabId,
      ptyId,
      cwd: cwd ?? DEFAULT_CWD,
      customLabel: null,
      scrollback: [],
      grid: null,
      input: '',
      shellDraft: '',
      focusMode: 'shell',
      nlMode: 'shell',
      escapeHint: null,
      shellStatus: 'quiet',
      tuiRunning: null,
      unread: 0,
      prefillSelected: false,
      sweep: false,
      pendingHandoffAsk: null,
    };
    setTabs((prev) => [...prev, tab]);
    setActiveTabId(tabId);
    setExpanded(true);
    return ptyId;
  }, []);

  // M6.B Step 2 — external "open + run a command" trigger. App.tsx
  // fires `kota:smart-run` when the user clicks the GitHub CLI setup
  // pill so we drop them straight into a new tab running the browser
  // login flow.
  // Detail: { command: string, cwd?: string }.
  useEffect(() => {
    const onRun = (e: Event) => {
      const detail = (e as CustomEvent).detail as { command?: string; cwd?: string } | undefined;
      const cmd = detail?.command;
      if (!cmd) return;
      void newTab(detail?.cwd).then((ptyId) => {
        // Tiny tick so the PTY has its prompt before we write —
        // otherwise zsh swallows the line as part of its init banner.
        setTimeout(() => { void writeSmartPty(ptyId, `${cmd}\r`); }, 250);
      });
    };
    window.addEventListener('kota:smart-run', onRun);
    return () => window.removeEventListener('kota:smart-run', onRun);
  }, [newTab]);

  const removeTabLocal = useCallback((tabId: string) => {
    setTabs((prev) => {
      const idx = prev.findIndex((t) => t.tabId === tabId);
      if (idx === -1) return prev;
      const next = prev.filter((t) => t.tabId !== tabId);
      // Switch active to a neighbor if needed.
      if (activeIdRef.current === tabId) {
        const newActive = next[Math.min(idx, next.length - 1)]?.tabId ?? null;
        setActiveTabId(newActive);
        // If no tabs remain, auto-collapse (Q6 = A).
        if (newActive === null) setExpanded(false);
      }
      return next;
    });
  }, []);

  const closeTab = useCallback(async (
    tabId: string,
    force = false,
    anchor?: { right: number; bottom: number },
  ) => {
    const tab = tabsRef.current.find((t) => t.tabId === tabId);
    if (!tab) return;
    const isRunning = tab.shellStatus === 'live' || tab.tuiRunning !== null;
    if (isRunning && !force && !skipCloseConfirm) {
      setConfirmCloseTabId(tabId);
      if (anchor) setConfirmAnchor(anchor);
      return;
    }
    setConfirmCloseTabId(null);
    setConfirmAnchor(null);
    try { await closeSmartPty(tab.ptyId); } catch { /* ignore */ }
    removeTabLocal(tabId);
  }, [skipCloseConfirm, removeTabLocal]);

  const switchTab = useCallback((tabId: string) => {
    setActiveTabId(tabId);
    setContextMenu(null);
    setConfirmCloseTabId(null);
  }, []);

  const cloneActiveCwd = useCallback(() => {
    const a = tabsRef.current.find((t) => t.tabId === activeIdRef.current);
    void newTab(a?.cwd === '~' ? undefined : a?.cwd);
  }, [newTab]);

  // ── input + NL state machine (per active tab) ───────────────────
  const onChange = (v: string) => {
    if (!activeTab) return;
    updateTab(activeTab.tabId, (t) => ({
      input: v,
      focusMode: 'smart',
      nlMode: t.nlMode === 'thinking' ? 'thinking' : 'typing',
      prefillSelected: false,
    }));
  };

  /** Auto-handoff: spawn the Magi-selected agent CLI in the
   *  current smart PTY, log a dim banner explaining the handoff, then
   *  show the original ask in a pill so the user can insert/copy it
   *  once the agent prompt is ready. */
  const handoffToProvider = useCallback((
    tabId: string,
    ask: string,
    reason: string,
    provider: MagiProvider,
  ) => {
    const tab = tabsRef.current.find((t) => t.tabId === tabId);
    if (!tab) return;
    const ptyId = tab.ptyId;
    const handoffArgs = magiHandoffArgs(provider);
    const handoffSuffix = handoffArgs.length > 0 ? ` ${handoffArgs.join(' ')}` : '';

    setTabs((prev) =>
      prev.map((t) =>
        t.tabId === tabId
          ? {
              ...t,
              scrollback: trimScrollback([
                ...t.scrollback,
                { kind: 'dim', text: `↳ ${reason} — handing off to ${provider}` },
              ]),
            }
          : t,
      ));

    updateTab(tabId, () => ({
      input: '',
      shellDraft: '',
      focusMode: 'shell',
      nlMode: 'shell',
      escapeHint: null,
      prefillSelected: false,
      sweep: false,
      tuiRunning: provider,
      shellStatus: 'live',
      // Stash the original ask for the handoff pill. The user inserts
      // it manually after Claude finishes any trust / auth prompts.
      pendingHandoffAsk: ask,
    }));

    // Resolve the CLI to an absolute path via Kota's augmented PATH.
    // The user's interactive ~/.zshrc may have rebuilt $PATH (conda /
    // nvm / pyenv hooks routinely overwrite it), so a bare `claude`
    // can land in a "command not found" state even when the binary
    // exists. Resolution uses path_env's FALLBACK_PATH_DIRS (homebrew,
    // /usr/local/bin, ~/.bun/bin, etc) plus the inherited PATH.
    void resolveCli(provider).then(async (agentBin) => {
      const stillThere = tabsRef.current.find((t) => t.tabId === tabId);
      if (!stillThere || stillThere.ptyId !== ptyId) return;
      await writeSmartPty(ptyId, `\x15${agentBin}${handoffSuffix}\r`);
    }).catch((err) => {
      console.warn(`[smart-shell] ${provider} handoff failed`, err);
      updateTab(tabId, (t) => {
        if (t.ptyId !== ptyId) return {};
        return {
          tuiRunning: null,
          pendingHandoffAsk: null,
          shellStatus: t.shellStatus === 'dead' ? 'dead' : 'quiet',
          scrollback: trimScrollback([
            ...t.scrollback,
            { kind: 'err', text: `↳ handoff failed — could not launch ${provider}` },
          ]),
        };
      });
    });
  }, [updateTab]);

  const translateNL = useCallback((tabId: string, ask: string) => {
    const requestedProvider = loadMagiProvider();
    // Force a synchronous React commit so the spinner appears IMMEDIATELY.
    flushSync(() => {
      updateTab(tabId, () => ({ nlMode: 'thinking', focusMode: 'smart' }));
    });
    setTabs((prev) =>
      prev.map((t) =>
        t.tabId === tabId
          ? { ...t, scrollback: trimScrollback([...t.scrollback, { kind: 'ask', text: `↳ ask: ${ask}` }]) }
          : t,
      ));
    const start = Date.now();
    // Yield to the browser so it can actually PAINT the spinner before
    // we kick off the slow LLM round-trip. flushSync commits the React
    // tree synchronously but the browser still doesn't paint until the
    // next animation frame; without this setTimeout(0) the JS event
    // loop is busy on the IPC await and the spinner only flashes after
    // the response has arrived. requestAnimationFrame would be even
    // tighter but setTimeout(0) is enough and safer (rAF can be
    // throttled when the window isn't focused).
    window.setTimeout(() => {
      void translateNlPrompt(ask, requestedProvider)
        .catch((err) => {
          updateTab(tabId, (t) => ({
            input: ask,
            focusMode: 'smart',
            nlMode: 'typing',
            escapeHint: null,
            prefillSelected: false,
            sweep: false,
            scrollback: trimScrollback([
              ...t.scrollback,
              { kind: 'err', text: `↳ translator unavailable: ${String(err)}` },
            ]),
          }));
          return null;
        })
        .then((res) => {
          if (!res) return;
          const remaining = Math.max(0, 120 - (Date.now() - start));
          window.setTimeout(() => {
            // Escape → auto-handoff: launch the target CLI in
            // provider-specific bypass mode and stash the user's original
            // ask for manual insert once the TUI is ready.
            if (res.kind === 'escape') {
              const provider = normalizeMagiProvider(res.provider ?? res.value);
              handoffToProvider(tabId, ask, res.hint ?? 'too complex for one command', provider);
              return;
            }
            const latest = tabsRef.current.find((t) => t.tabId === tabId);
            if (!latest) return;
            void writeSmartPty(latest.ptyId, res.value);
            updateTab(tabId, (t) => ({
              input: '',
              shellDraft: nextShellDraftAfterText(t.shellDraft, res.value),
              focusMode: 'shell',
              nlMode: 'shell',
              escapeHint: null,
              prefillSelected: false,
              sweep: true,
            }));
            focusShellCapture();
          }, remaining);
        });
    }, 0);
  }, [focusShellCapture, updateTab, handoffToProvider]);

  const commitShellDraft = useCallback((tabId: string) => {
    const tab = tabsRef.current.find((t) => t.tabId === tabId);
    if (!tab) return;
    const cmd = tab.shellDraft;
    console.log(
      `[smart-shell:${tabId}] commit shell cmd=${JSON.stringify(cmd)} tuiRunning=${tab.tuiRunning}`,
    );
    updateTab(tabId, () => ({
      shellDraft: '',
      focusMode: 'shell',
      nlMode: 'shell',
      escapeHint: null,
      prefillSelected: false,
    }));
    const trimmed = cmd.trim();
    if (trimmed === 'clear' || trimmed === 'reset') {
      void writeSmartPty(tab.ptyId, '\x15');
      void clearSmartPty(tab.ptyId);
      updateTab(tabId, () => ({ scrollback: [] }));
      return;
    }
    const cliMatch = trimmed.match(/^(claude|cc|codex|agy|opencode|pi|kimi)(\s.*)?$/);
    if (cliMatch) {
      const cliName = (cliMatch[1] === 'cc' ? 'claude' : cliMatch[1]) as FallbackCli;
      const restArgs = cliMatch[2] ?? '';
      updateTab(tabId, () => ({
        tuiRunning: cliName,
        shellStatus: 'live',
        shellDraft: '',
      }));
      // Resolve to absolute path so the launch survives a user
      // ~/.zshrc that conda / nvm / pyenv hooks have stripped of
      // /opt/homebrew/bin etc. \x15 wipes whatever was mirrored to
      // the readline buffer, then we send the resolved absolute
      // path + the original args + CR.
      void resolveCli(cliName).then((absBin) => {
        void writeSmartPty(tab.ptyId, `\x15${absBin}${restArgs}\r`);
      });
      return;
    }
    if (trimmed === 'exit' && tab.tuiRunning !== null) {
      updateTab(tabId, () => ({ tuiRunning: null, shellStatus: 'quiet', pendingHandoffAsk: null }));
    }
    void writeSmartPty(tab.ptyId, '\r');
  }, [updateTab]);

  const onInputKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (!activeTab) return;
    const composing = e.nativeEvent.isComposing || (e as unknown as { keyCode?: number }).keyCode === 229;

    // Shell/TUI focus: every key goes straight to the PTY. The bottom
    // Smart command row is not involved until the user explicitly
    // clicks it.
    if (activeTab.tuiRunning || activeTab.focusMode === 'shell') {
      if (e.nativeEvent.isComposing || (e as unknown as { keyCode?: number }).keyCode === 229) {
        return;
      }
      if (['Shift', 'Meta', 'Control', 'Alt', 'CapsLock'].includes(e.key)) return;

      if (!activeTab.tuiRunning) {
        if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
          e.preventDefault();
          void clearSmartPty(activeTab.ptyId);
          updateTab(activeTab.tabId, () => ({ scrollback: [] }));
          return;
        }
        if (e.ctrlKey && !e.metaKey && (e.key === 'c' || e.key === 'C') && activeTab.shellDraft.length === 0) {
          e.preventDefault();
          void interruptSmartPty(activeTab.ptyId);
          updateTab(activeTab.tabId, () => ({
            shellDraft: '',
            tuiRunning: null,
            shellStatus: 'quiet',
            pendingHandoffAsk: null,
          }));
          return;
        }
        if (e.key === 'Enter') {
          e.preventDefault();
          commitShellDraft(activeTab.tabId);
          return;
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          void writeSmartPty(activeTab.ptyId, '\x15');
          updateTab(activeTab.tabId, () => ({ shellDraft: '', input: '', nlMode: 'shell' }));
          return;
        }
      }

      // Text insertion is handled by the browser input event below so
      // keyboard layouts, Option symbols, dead keys, and IME commits are
      // preserved exactly.
      if (e.key.length === 1 && !(e.ctrlKey && !e.metaKey)) return;
      const bytes = inputKeyToBytes(e, activeTab.tuiRunning);
      if (bytes != null && !e.metaKey) {
        e.preventDefault();
        if (!activeTab.tuiRunning) {
          if (e.key === 'Backspace') {
            updateTab(activeTab.tabId, (t) => ({ shellDraft: t.shellDraft.slice(0, -1) }));
          } else if (bytes === '\x15' || bytes === '\x03') {
            updateTab(activeTab.tabId, () => ({ shellDraft: '' }));
          }
        }
        void writeSmartPty(activeTab.ptyId, bytes);
      }
      return;
    }

    // Smart focus: the user types natural language. Enter translates to
    // a shell command, inserts that command at the PTY cursor, then moves
    // focus back to the real shell so the user can press Enter to run.
    if (e.key === 'Enter') {
      if (composing) return;
      e.preventDefault();
      const ask = activeTab.input.trim();
      if (!ask) return;
      translateNL(activeTab.tabId, ask);
      return;
    }
    if (e.key === 'Escape') {
      e.preventDefault();
      updateTab(activeTab.tabId, () => ({
        input: '',
        focusMode: 'shell',
        nlMode: 'shell',
        escapeHint: null,
        prefillSelected: false,
      }));
      focusShellCapture();
      return;
    }
    if ((e.metaKey || e.ctrlKey) && (e.key === 'k' || e.key === 'K')) {
      e.preventDefault();
      void clearSmartPty(activeTab.ptyId);
      updateTab(activeTab.tabId, () => ({ scrollback: [] }));
    }
  };

  const onTerminalTextInput = (e: ReactFormEvent<HTMLInputElement>) => {
    if (!activeTab) return;
    const native = e.nativeEvent as InputEvent;
    const inputType = native.inputType ?? '';
    const text = native.data ?? '';
    if (!text) return;
    if (activeTab.tuiRunning || activeTab.focusMode === 'shell') {
      if (inputType !== 'insertText') return;
      void writeSmartPty(activeTab.ptyId, normalizeTerminalText(text));
      if (!activeTab.tuiRunning) {
        updateTab(activeTab.tabId, (t) => ({
          shellDraft: nextShellDraftAfterText(t.shellDraft, text),
        }));
      }
      e.currentTarget.value = '';
    }
  };

  const onTerminalPaste = (e: ReactClipboardEvent<HTMLInputElement>) => {
    if (!activeTab) return;
    const text = e.clipboardData.getData('text');
    if (!text) return;
    if (activeTab.tuiRunning || activeTab.focusMode === 'shell') {
      e.preventDefault();
      void writeSmartPty(activeTab.ptyId, normalizeTerminalText(text));
      if (!activeTab.tuiRunning) {
        updateTab(activeTab.tabId, (t) => ({
          shellDraft: nextShellDraftAfterText(t.shellDraft, text),
        }));
      }
      e.currentTarget.value = '';
    }
  };

  /** Shell/TUI capture composition flush — when an IME finishes composing (e.g.
   *  Chinese pinyin → 中文 on space), forward the resolved text to the
   *  PTY in one chunk and clear the native input value so the next
   *  composition starts fresh. Pre-composition keystrokes (isComposing)
   *  are skipped by onInputKeyDown so the IME can compose locally. */
  const onShellCaptureCompositionEnd = (e: ReactCompositionEvent<HTMLInputElement>) => {
    if (!activeTab?.tuiRunning && activeTab?.focusMode !== 'shell') return;
    const data = e.data ?? '';
    if (data) {
      void writeSmartPty(activeTab.ptyId, data);
      if (!activeTab.tuiRunning) {
        updateTab(activeTab.tabId, (t) => ({
          shellDraft: nextShellDraftAfterText(t.shellDraft, data),
        }));
      }
    }
    if (e.currentTarget) e.currentTarget.value = '';
  };

  const restartActiveShell = () => {
    if (!activeTab) return;
    const id = activeTab.tabId;
    updateTab(id, () => ({
      shellStatus: 'quiet',
      tuiRunning: null,
      input: '',
      shellDraft: '',
      focusMode: 'shell',
      nlMode: 'shell',
      escapeHint: null,
      prefillSelected: false,
      scrollback: [],
      grid: null,
      pendingHandoffAsk: null,
    }));
    void restartSmartPty(activeTab.ptyId);
  };

  // ── Window-level keyboard shortcuts ────────────────────────────
  // W4 / K1 — Smart Terminal owns ONLY the expand-toggle (⌘`). All
  // ⌘T / ⌘W / ⌘1-9 / ⌘⇧[/] are reserved for agent windows.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (isToggleShortcut(e)) {
        e.preventDefault();
        setExpanded((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // ── PTY size sync ──────────────────────────────────────────────
  const syncPtySize = useCallback(() => {
    const panel = panelRef.current;
    if (!panel || !expandedRef.current) return;
    const a = tabsRef.current.find((t) => t.tabId === activeIdRef.current);
    if (!a) return;
    const rect = panel.getBoundingClientRect();
    const cols = Math.max(40, Math.floor((rect.width - 28) / 8));
    const rows = Math.max(8, Math.floor((rect.height - 100) / 18));
    void resizeSmartPty(a.ptyId, cols, rows);
  }, []);
  useEffect(() => {
    if (!expanded) return;
    syncPtySize();
    const onResize = () => syncPtySize();
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, [expanded, height, activeTabId, syncPtySize]);

  // ── resize drag ────────────────────────────────────────────────
  const resizeDrag = useRef<{ y: number; startH: number } | null>(null);
  const onResizeStart = (e: ReactMouseEvent) => {
    resizeDrag.current = { y: e.clientY, startH: height };
    const onMove = (ev: MouseEvent) => {
      if (!resizeDrag.current) return;
      const dy = resizeDrag.current.y - ev.clientY;
      const next = Math.min(
        Math.max(MIN_HEIGHT, resizeDrag.current.startH + dy),
        Math.floor(window.innerHeight * MAX_HEIGHT_VH),
      );
      setHeight(next);
    };
    const onUp = () => {
      resizeDrag.current = null;
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  // ── derived collapsed-bar state ────────────────────────────────
  const totalUnread = useMemo(
    () => tabs.reduce((sum, t) => sum + t.unread, 0),
    [tabs],
  );
  const anyLive = useMemo(
    () => tabs.some((t) => t.shellStatus === 'live' || t.tuiRunning),
    [tabs],
  );

  // ── close confirm: keyboard hooks while tooltip is open ────────
  useEffect(() => {
    if (!confirmCloseTabId) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        setConfirmCloseTabId(null);
        setConfirmAnchor(null);
      } else if (e.key === 'Enter') {
        e.preventDefault();
        void closeTab(confirmCloseTabId, true);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [confirmCloseTabId, closeTab]);

  // Click-outside to dismiss context menu / confirm.
  useEffect(() => {
    if (!contextMenu && !confirmCloseTabId) return;
    const onClick = () => {
      setContextMenu(null);
      // Don't dismiss confirm on every click — only on click-outside the
      // tooltip, which we approximate via Esc + explicit cancel button.
    };
    window.addEventListener('mousedown', onClick);
    return () => window.removeEventListener('mousedown', onClick);
  }, [contextMenu, confirmCloseTabId]);

  // ── render ─────────────────────────────────────────────────────

  if (!expanded) {
    return (
      <div
        className="st-collapsed"
        data-testid="smart-terminal"
        data-state="collapsed"
        role="button"
        aria-label="Expand Smart Terminal"
        onClick={() => setExpanded(true)}
      >
        <span className="st-bar-ico" aria-hidden>⌘</span>
        <span className="st-bar-label">Shell</span>
        {tabs.length > 0 && (
          <span
            className={`st-shells-chip ${totalUnread > 0 ? 'unread' : ''}`}
            data-testid="st-shells-chip"
            aria-label={`${tabs.length} shell${tabs.length === 1 ? '' : 's'}`}
          >
            {totalUnread > 0 && <span className="st-shells-dot" aria-hidden />}
            <span className="st-shells-count">{tabs.length}</span>
            shell{tabs.length === 1 ? '' : 's'}
          </span>
        )}
        <CwdChip cwd={activeTab?.cwd ?? DEFAULT_CWD} />
        <span className="st-bar-grow" />
        <span className="st-bar-right">
          <span className="st-bar-status">
            <span
              className={`st-dot ${totalUnread > 0 ? 'unread' : anyLive ? 'live' : ''}`}
              aria-hidden
            />
            {totalUnread > 0
              ? `${totalUnread} new line${totalUnread === 1 ? '' : 's'}`
              : anyLive ? 'running' : 'quiet'}
          </span>
          <span className="st-bar-kbd">⌘`</span>
          <span className="st-bar-chev" aria-hidden>
            <svg viewBox="0 0 16 16" fill="none"
                 stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <path d="M4 10 Q6 8.5 8 7 Q10 8.5 12 10" />
            </svg>
          </span>
        </span>
      </div>
    );
  }

  return (
    <div
      ref={panelRef}
      className="st-panel"
      data-testid="smart-terminal"
      data-state="expanded"
      style={{ height }}
      role="region"
      aria-label="Smart Terminal"
    >
      <div
        className="st-resize-handle"
        onMouseDown={onResizeStart}
        data-testid="st-resize-handle"
        aria-hidden
      />
      <button
        className="st-panel-minimize"
        aria-label="Minimize Smart Shell"
        title="Minimize Smart Shell"
        data-testid="st-collapse-btn"
        onClick={() => setExpanded(false)}
      >
        <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
             strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
          <path d="M4 6l4 4 4-4" />
        </svg>
      </button>
      <span className="st-panel-minimize-hint" aria-hidden>
        ⌘`
      </span>

      {/* Tab strip */}
      <div className="st-tabstrip" data-testid="st-tabstrip">
        <div className={`st-tab-track ${tabs.length > 4 ? 'overflow' : ''}`}>
          {tabs.map((t) => (
            <TabChip
              key={t.tabId}
              tab={t}
              active={t.tabId === activeTabId}
              onClick={() => switchTab(t.tabId)}
              onMiddleClose={() => void closeTab(t.tabId, false)}
              onCloseClick={(e) => {
                e.stopPropagation();
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
                const anchor = { right: r.right, bottom: r.bottom };
                if (e.shiftKey) {
                  setSkipCloseConfirm(true);
                  void closeTab(t.tabId, true, anchor);
                } else {
                  void closeTab(t.tabId, false, anchor);
                }
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setContextMenu({ tabId: t.tabId, x: e.clientX, y: e.clientY });
              }}
            />
          ))}
          <button
            className="st-plus"
            onClick={(e) => {
              e.stopPropagation();
              if (e.altKey) cloneActiveCwd();
              else void newTab();
            }}
            aria-label="New shell tab"
            title="New tab · ⌥-click to clone active CWD"
            data-testid="st-plus"
          >
            +
          </button>
        </div>
      </div>

      {/* Header (CWD chip reflects active tab) */}
      <div className="st-head">
        <span className="st-head-label">Shell</span>
        {activeTab && <CwdChip cwd={activeTab.cwd} />}
        {activeTab?.tuiRunning && (
          <span className="st-tui-pill" data-testid="st-tui-pill">
            ▶ {tuiDisplayName(activeTab.tuiRunning)}
          </span>
        )}
        <span className="st-head-spacer" />
        <span className="st-head-shortcut" aria-label="Command K clears the Smart Shell screen">
          <kbd>⌘K</kbd>
          <span>Clear screen</span>
        </span>
      </div>

      {/* Active tab body — alacritty grid (real PTY) or mock fallback.
          When a TUI agent is running the bottom prompt row collapses
          to a 0px-wide invisible capture input. Click the grid area
          (e.g. to read claude's output) and we re-grab focus so the
          next keystroke still reaches the PTY — without this hook,
          one click on the grid silently disables typing for the rest
          of the session. */}
      <div
        className="st-scrollback"
        ref={scrollbackRef}
        onWheel={(e) => {
          if (!activeTab?.grid) return;
          e.preventDefault();
          e.stopPropagation();
          if (activeTab.grid.mouseMode) {
            void writeSmartPty(
              activeTab.ptyId,
              wheelMouseBytes(e, activeTab.grid, e.currentTarget),
            );
            return;
          }
          void scrollSmartPty(activeTab.ptyId, wheelScrollLines(e));
        }}
        onMouseUp={() => {
          if (!activeTab) return;
          // Don't yank focus mid-text-selection — only refocus when the
          // click was a plain click (no selection).
          if (window.getSelection()?.toString()) return;
          activateShellFocus(activeTab.tabId);
        }}
      >
        {activeTab?.grid ? (
          <TerminalGrid snapshot={activeTab.grid} />
        ) : (
          activeTab?.scrollback.map((line, i) => (
            <div key={i} className={`st-line sh-${line.kind}`}>{line.text}</div>
          ))
        )}
        {activeTab?.shellStatus === 'live' && !activeTab.tuiRunning && (
          <div className="st-running-chip" data-testid="st-running-chip">
            running · PTY
          </div>
        )}
      </div>

      {activeTab?.shellStatus === 'dead' ? (
        <div className="st-restart">
          <div className="st-restart-msg">shell ended</div>
          <button
            className="st-restart-btn"
            onClick={restartActiveShell}
            data-testid="st-restart-btn"
          >
            ⟳ Restart shell
          </button>
        </div>
      ) : activeTab ? (
        activeTab.tuiRunning ? (
          // TUI mode: bottom smart-input is replaced with a thin hint
          // bar + an invisible focusable input that captures every key
          // (including IME composition) and forwards it to the PTY.
          // The user types directly into the agent's own prompt up in
          // the grid. To leave, use the agent's own `/exit` / Ctrl-D —
          // no separate Kota-side shortcut.
          <>
          {activeTab.pendingHandoffAsk && (
            // Pending Smart command handoff — show the user's original question
            // boxed up with three explicit actions instead of trying to
            // auto-detect when claude's input box is "ready". User
            // clicks Insert when they see the prompt is up, Copy if
            // they want to paste manually elsewhere, or Close to
            // dismiss without sending.
            <div className="st-handoff-pill" data-testid="st-handoff-pill">
              <div className="st-handoff-text">
                <span className="st-handoff-label">↳ ask:</span>
                <span className="st-handoff-body">{activeTab.pendingHandoffAsk}</span>
              </div>
              <div className="st-handoff-actions">
                <button
                  type="button"
                  className="st-handoff-btn"
                  onClick={() => {
                    if (!activeTab.pendingHandoffAsk) return;
                    void navigator.clipboard?.writeText(activeTab.pendingHandoffAsk);
                  }}
                  data-testid="st-handoff-copy"
                  aria-label="Copy original ask to clipboard"
                  title="复制"
                >
                  <svg
                    className="st-handoff-icon"
                    viewBox="0 0 24 24"
                    fill="none"
                    aria-hidden="true"
                  >
                    <path
                      d="M6 11C6 8.17157 6 6.75736 6.87868 5.87868C7.75736 5 9.17157 5 12 5H15C17.8284 5 19.2426 5 20.1213 5.87868C21 6.75736 21 8.17157 21 11V16C21 18.8284 21 20.2426 20.1213 21.1213C19.2426 22 17.8284 22 15 22H12C9.17157 22 7.75736 22 6.87868 21.1213C6 20.2426 6 18.8284 6 16V11Z"
                      stroke="currentColor"
                      strokeWidth="1.5"
                    />
                    <path
                      opacity="0.5"
                      d="M6 19C4.34315 19 3 17.6569 3 16V10C3 6.22876 3 4.34315 4.17157 3.17157C5.34315 2 7.22876 2 11 2H15C16.6569 2 18 3.34315 18 5"
                      stroke="currentColor"
                      strokeWidth="1.5"
                    />
                  </svg>
                </button>
                <button
                  type="button"
                  className="st-handoff-btn primary"
                  onClick={() => {
                    const ask = activeTab.pendingHandoffAsk;
                    if (!ask) return;
                    void writeSmartPty(activeTab.ptyId, ask);
                    updateTab(activeTab.tabId, () => ({ pendingHandoffAsk: null }));
                    shellInputRef.current?.focus();
                  }}
                  data-testid="st-handoff-insert"
                  aria-label="Insert original ask into agent input"
                  title="插入"
                >
                  <svg
                    className="st-handoff-icon"
                    viewBox="0 0 24 24"
                    fill="none"
                    aria-hidden="true"
                  >
                    <path
                      d="M7 9.5L12 4.5L17 9.5"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                    <path
                      opacity="0.5"
                      d="M12 4.5C12 4.5 12 12.8333 12 14.5C12 16.1667 13 19.5 17 19.5"
                      stroke="currentColor"
                      strokeWidth="1.5"
                      strokeLinecap="round"
                    />
                  </svg>
                </button>
                <button
                  type="button"
                  className="st-handoff-btn ghost"
                  onClick={() => {
                    updateTab(activeTab.tabId, () => ({ pendingHandoffAsk: null }));
                  }}
                  data-testid="st-handoff-close"
                  aria-label="Close pending ask"
                >
                  ×
                </button>
              </div>
            </div>
          )}
          <div
            className="st-prompt-row st-tui-row"
            data-tui={activeTab.tuiRunning}
            onMouseDown={(e) => {
              // Click anywhere on the row → focus the invisible capture
              // input. Without this, the row's gutter / hint / padding
              // is dead space that swallows clicks and leaves the user
              // with no keyboard focus and no visible cue to fix it.
              if (e.target !== shellInputRef.current) {
                e.preventDefault();
                shellInputRef.current?.focus();
              }
            }}
          >
            <span className="st-prompt-gutter">↳ in {tuiDisplayName(activeTab.tuiRunning)}</span>
            <input
              ref={shellInputRef}
              className="st-tui-capture"
              onKeyDown={onInputKeyDown}
              onInput={onTerminalTextInput}
              onPaste={onTerminalPaste}
              onCompositionEnd={onShellCaptureCompositionEnd}
              autoFocus
              data-testid="st-input"
              aria-label={`${tuiDisplayName(activeTab.tuiRunning)} input`}
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
            />
          </div>
          </>
        ) : (
          <>
            <input
              ref={shellInputRef}
              className="st-shell-capture"
              onKeyDown={onInputKeyDown}
              onInput={onTerminalTextInput}
              onPaste={onTerminalPaste}
              onCompositionEnd={onShellCaptureCompositionEnd}
              data-testid="st-shell-capture"
              aria-label="Shell input"
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
            />
            <div
              className={`st-prompt-row st-smart-row ${activeTab.focusMode === 'smart' || activeTab.nlMode === 'thinking' ? 'nl' : 'inactive'}`}
              data-nl-mode={activeTab.nlMode}
              data-focus-mode={activeTab.focusMode}
              onMouseDown={(e) => {
                if (activeTab.nlMode === 'thinking') return;
                e.preventDefault();
                activateSmartFocus(activeTab.tabId);
              }}
            >
              <img
                className="st-prompt-avatar"
                src={avatarMagi}
                alt=""
                aria-hidden="true"
                draggable={false}
              />
              <span className="st-prompt-gutter">
                {activeTab.nlMode === 'thinking'
                  ? 'Magi ›'
                  : activeTab.focusMode === 'smart'
                    ? 'Magi ›'
                    : 'Magi'}
              </span>
              {/* Indicators sit BEFORE the input — visually right next to
                  the gutter where the user's eye already is. The input's
                  flex:1 would push them to the far right of the panel
                  if placed after. */}
              {activeTab.nlMode === 'thinking' && (
                <span className="st-nl-indicator" aria-hidden>
                  <span className="st-nl-spin">
                    <span /><span /><span />
                  </span>
                  <span className="st-nl-hint">thinking…</span>
                </span>
              )}
              {activeTab.nlMode === 'typing' && (
                <span className="st-nl-indicator" aria-hidden>
                  <span className="st-nl-hint">enter to insert</span>
                </span>
              )}
              {activeTab.focusMode === 'shell' && activeTab.nlMode !== 'thinking' && (
                <span className="st-nl-indicator" aria-hidden>
                  <span className="st-nl-hint">Click and describe a command</span>
                </span>
              )}
              <span className={`st-prompt-input-wrap ${activeTab.sweep ? 'sweep' : ''}`}>
                <input
                  ref={smartInputRef}
                  className="st-prompt-input"
                  value={activeTab.input}
                  onChange={(e) => onChange(e.target.value)}
                  onKeyDown={onInputKeyDown}
                  onFocus={() => activateSmartFocus(activeTab.tabId)}
                  readOnly={activeTab.nlMode === 'thinking'}
                  spellCheck={false}
                  autoComplete="off"
                  data-testid="st-input"
                  aria-label="Smart command input"
                  placeholder={activeTab.focusMode === 'smart' ? 'Describe the command…' : ''}
                />
              </span>
            </div>
          </>
        )
      ) : null}

      {/* Close-confirm tooltip — rendered at panel level so it escapes
          the .st-tab-track's overflow clip. Anchored to the close
          button's bounding rect captured at click time. */}
      {confirmCloseTabId && confirmAnchor && (() => {
        // Tooltip is 240 px wide; anchor its right edge to the close
        // button's right but clamp to stay on screen.
        const TIP_W = 240;
        const left = Math.max(8, Math.min(
          confirmAnchor.right - TIP_W,
          window.innerWidth - TIP_W - 8,
        ));
        return (
        <div
          className="st-confirm-tip"
          role="dialog"
          aria-label="Confirm close"
          data-testid="st-confirm-tip"
          style={{
            position: 'fixed',
            top: confirmAnchor.bottom + 6,
            left,
            width: TIP_W,
          }}
          onClick={(e) => e.stopPropagation()}
          onMouseDown={(e) => e.stopPropagation()}
        >
          <div className="st-confirm-head">
            This shell is running. Close the tab and kill the process?
          </div>
          <div className="st-confirm-row">
            <button
              className="st-confirm-btn kill"
              onClick={() => void closeTab(confirmCloseTabId, true)}
            >
              Kill &amp; close
            </button>
            <button
              className="st-confirm-btn cancel"
              onClick={() => { setConfirmCloseTabId(null); setConfirmAnchor(null); }}
            >
              Cancel
            </button>
          </div>
          <div className="st-confirm-kbd">⏎ confirm · esc cancel · ⇧-click to skip next time</div>
        </div>
        );
      })()}

      {/* Right-click context menu */}
      {contextMenu && (
        <div
          className="st-ctx-menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          data-testid="st-ctx-menu"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <button
            className="st-ctx-row"
            onClick={() => {
              const a = tabsRef.current.find((t) => t.tabId === contextMenu.tabId);
              if (a) void newTab(a.cwd === '~' ? undefined : a.cwd);
              setContextMenu(null);
            }}
          >
            <span>Duplicate (same CWD)</span>
            <span className="st-ctx-kbd">⌥+</span>
          </button>
          <button
            className="st-ctx-row"
            onClick={() => {
              const a = tabsRef.current.find((t) => t.tabId === contextMenu.tabId);
              if (a) {
                void clearSmartPty(a.ptyId);
                updateTab(a.tabId, () => ({ scrollback: [] }));
              }
              setContextMenu(null);
            }}
          >
            <span>Clear scrollback</span>
          </button>
          <button
            className="st-ctx-row"
            onClick={() => {
              const a = tabsRef.current.find((t) => t.tabId === contextMenu.tabId);
              if (a) void interruptSmartPty(a.ptyId);
              setContextMenu(null);
            }}
          >
            <span>Send SIGINT</span>
            <span className="st-ctx-kbd">⌃C</span>
          </button>
          <div className="st-ctx-divider" />
          <button
            className="st-ctx-row danger"
            onClick={() => {
              const others = tabsRef.current.filter((t) => t.tabId !== contextMenu.tabId);
              for (const t of others) void closeTab(t.tabId, true);
              setContextMenu(null);
            }}
          >
            <span>Close other tabs</span>
          </button>
          <button
            className="st-ctx-row danger"
            onClick={() => {
              void closeTab(contextMenu.tabId, true);
              setContextMenu(null);
            }}
          >
            <span>Close tab &amp; kill</span>
          </button>
        </div>
      )}
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────
//  Tab chip
// ─────────────────────────────────────────────────────────────────
function TabChip({
  tab,
  active,
  onClick,
  onCloseClick,
  onMiddleClose,
  onContextMenu,
}: {
  tab: TabRuntime;
  active: boolean;
  onClick: () => void;
  onCloseClick: (e: ReactMouseEvent<HTMLButtonElement>) => void;
  onMiddleClose: () => void;
  onContextMenu: (e: ReactMouseEvent) => void;
}) {
  const isRunning = tab.shellStatus === 'live' || tab.tuiRunning !== null;
  const dotCls =
    tab.tuiRunning ? '' :
    tab.shellStatus === 'dead' ? 'err' :
    tab.shellStatus === 'live' ? 'live' :
    tab.unread > 0 ? 'unread' :
    'quiet';
  return (
    <div
      className={`st-tab ${active ? 'active' : ''} ${isRunning ? 'live' : ''}`}
      role="tab"
      aria-selected={active}
      data-testid={`st-tab-${tab.tabId}`}
      onClick={onClick}
      onMouseDown={(e) => {
        if (e.button === 1) {
          e.preventDefault();
          onMiddleClose();
        }
      }}
      onContextMenu={onContextMenu}
      title={tab.cwd}
    >
      {tab.tuiRunning ? (
        <span className="st-tab-tui" aria-hidden>▶</span>
      ) : (
        <span className={`st-tab-dot ${dotCls}`} aria-hidden />
      )}
      <span className="st-tab-label">{deriveLabel(tab)}</span>
      <button
        type="button"
        className="st-tab-close"
        aria-label={`Close ${deriveLabel(tab)}`}
        onClick={onCloseClick}
        data-testid={`st-tab-close-${tab.tabId}`}
      >
        ×
      </button>
    </div>
  );
}

// ─────────────────────────────────────────────────────────────────
//  CWD chip
// ─────────────────────────────────────────────────────────────────
function CwdChip({ cwd }: { cwd: string }) {
  return (
    <span className="st-cwd-chip" data-testid="st-cwd-chip" title={cwd}>
      <span className="st-cwd-ico" aria-hidden>⌂</span>
      {cwd}
    </span>
  );
}
