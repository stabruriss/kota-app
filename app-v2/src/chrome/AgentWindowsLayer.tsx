/** AgentWindowsLayer — floating-window layer for live agent terminals.
 *
 *  Mounted as a sibling of <Stage>, position:absolute over the
 *  workspace area. For each live agent, renders one <WindowFrame>
 *  containing <TerminalGrid>. Owns global z-stack + cascade
 *  positioning + per-agent geometry (via useWindowGeometry per id).
 *
 *  Per HANDOFF-W-FloatingWindows.md K3 / Q2:
 *    - Focused window ⟺ targetAgent (single source of truth)
 *    - Click anywhere on the frame → focus
 *    - When all windows are minimized, parent falls target back to
 *      Captain (handled at the App layer, not here)                    */

import { useCallback, useEffect, useImperativeHandle, useRef, useState, forwardRef, type KeyboardEvent, type CompositionEvent, type FormEvent, type ClipboardEvent, type WheelEvent, type DragEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { WindowFrame } from './WindowFrame';
import { AgentCommendButton } from './AgentCommendButton';
import { ProjectAgentName } from './ProjectAgentName';
import {
  TerminalGrid,
  TERMINAL_CELL_WIDTH,
  TERMINAL_FONT_FAMILY,
  TERMINAL_FONT_SIZE,
  TERMINAL_LINE_HEIGHT,
} from './TerminalGrid';
import {
  cascadePosition,
  useWindowGeometry,
  WINDOW_DEFAULT_H,
  WINDOW_DEFAULT_W,
} from '../hooks/useWindowGeometry';
import type { DragDropEvent as TauriDragDropPayload } from '@tauri-apps/api/webview';
import type { WindowGeometry } from '../hooks/useWindowGeometry';
import { AGENTS } from '../mock/fixtures';
import type { Agent, AgentId } from '../types/scene';
import type { GridSnapshot, AgentStatusEvent } from '../types/agent-pty';
import { resizeAgentPty, scrollAgentPty } from '../pty-client';
import type { ProjectAgentCommendSource, ProjectAgentRecord } from '../pty-client';
import iconCommends from '../assets/tavern/icons/commends.svg';
import iconTurns from '../assets/tavern/icons/turns.svg';
import { avatarClassForAgentFallback, avatarImageStyleForId } from '../lib/hero-avatars';

// TerminalGrid renders monospace cells at fontSize=12, cellWidth=7.2,
// lineHeight=ceil(fontSize*1.2)=15. We deliberately use a slightly
// LARGER cell width here for the resize math so the PTY cols come out
// a little smaller than the display can hold — that way text wraps
// cleanly inside the visible area rather than spilling a few pixels
// past the right edge.
const CELL_W_RESIZE = 7.7;
const CELL_H_RESIZE = 15;
const BODY_HPAD = 6; // small breathing room on each side of the grid

const Z_BASE = 100;

const RATATUI_AGENT_CLIS = new Set(['codex', 'antigravity', 'opencode', 'pi', 'kimi']);
const TERMINAL_BODY_SELECTOR = '[data-agent-terminal-body="true"]';

export interface AgentWindowsLayerProps {
  /** Stable order so cascade indexing is consistent across renders. */
  liveAgents: AgentId[];
  grids: Map<AgentId, GridSnapshot>;
  status?: Map<AgentId, AgentStatusEvent>;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  /** Currently focused agent (= targetAgent at App layer). */
  focusedAgent: AgentId | null;
  /** Set of agents whose windows are currently minimized. Owned by App
   *  so the AgentRibbon (taskbar) can also reflect this state. */
  minimized: ReadonlySet<AgentId>;
  /** Restore + bring-to-front + select-target. Parent should clear
   *  this id from `minimized` and set targetAgent inside. */
  onFocusAgent: (id: AgentId) => void;
  /** User clicked the window's minimize button. Parent adds to set. */
  onMinimizeAgent: (id: AgentId) => void;
  /** Forward a keystroke to that agent's PTY stdin. */
  onAgentKey: (id: AgentId, bytes: string) => void;
  /** Project id for storage scoping. */
  projectId: string;
  /** Human-readable project name for project-agent full names. */
  projectName?: string;
  /** Experimental visual layer inspired by Ghostty. Does not replace PTY state. */
  ghosttyTerminalEnhancement?: boolean;
  /** Open the project-agent detail/config surface for this incarnation. */
  onOpenAgentDetail?: (id: AgentId) => void;
  /** Open the shared project-agent context menu. */
  onAgentContextMenu?: (id: AgentId, point: { x: number; y: number }) => void;
  onCommendAgent?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  agentRecords?: Readonly<Record<AgentId, ProjectAgentRecord>>;
}

/** Imperative handle exposed so App's ⌘1-9 dispatcher can raise +
 *  focus a window without going through state. */
export interface AgentWindowsLayerHandle {
  bringToFront: (id: AgentId) => void;
}

export interface RegisteredTerminalBody {
  el: HTMLElement;
  focusInput: () => void;
}

export const AgentWindowsLayer = forwardRef<AgentWindowsLayerHandle, AgentWindowsLayerProps>(
  function AgentWindowsLayer(
    {
      liveAgents,
      grids,
      status,
      agentMeta,
      focusedAgent,
      minimized,
      onFocusAgent,
      onMinimizeAgent,
      onAgentKey,
      projectId,
      projectName,
      ghosttyTerminalEnhancement = false,
      onOpenAgentDetail,
      onAgentContextMenu,
      onCommendAgent,
      agentRecords,
    },
    ref,
  ) {
  const terminalBodiesRef = useRef<Map<AgentId, RegisteredTerminalBody>>(new Map());

  // Global z-stack: higher index in `zStack` = higher z. We re-derive
  // each agent's z from its position in this array on every render.
  const [zStack, setZStack] = useState<AgentId[]>(() => [...liveAgents]);
  const zStackRef = useRef(zStack);
  useEffect(() => { zStackRef.current = zStack; }, [zStack]);

  // Keep zStack in sync with liveAgents (add new ones to top, drop
  // agents that have been dismissed).
  useEffect(() => {
    setZStack((prev) => {
      const liveSet = new Set(liveAgents);
      const next = prev.filter((id) => liveSet.has(id));
      for (const id of liveAgents) {
        if (!next.includes(id)) next.push(id);
      }
      return next;
    });
  }, [liveAgents]);

  const raiseToFront = useCallback(
    (id: AgentId) => {
      setZStack((prev) => {
        if (prev[prev.length - 1] === id) return prev;
        return [...prev.filter((x) => x !== id), id];
      });
    },
    [],
  );

  const focusTerminalInput = useCallback((id: AgentId) => {
    terminalBodiesRef.current.get(id)?.focusInput();
  }, []);

  const bringToFront = useCallback(
    (id: AgentId) => {
      raiseToFront(id);
      focusTerminalInput(id);
    },
    [focusTerminalInput, raiseToFront],
  );

  useImperativeHandle(ref, () => ({ bringToFront }), [bringToFront]);

  const focusAgent = useCallback(
    (id: AgentId, options: { focusInput?: boolean } = {}) => {
      const shouldFocusInput = options.focusInput ?? true;
      if (shouldFocusInput) {
        bringToFront(id);
      } else {
        raiseToFront(id);
      }
      // Parent's onFocusAgent clears the minimized flag + sets target.
      onFocusAgent(id);
    },
    [bringToFront, onFocusAgent, raiseToFront],
  );
  const focusAgentRef = useRef(focusAgent);
  const onAgentKeyRef = useRef(onAgentKey);
  useEffect(() => { focusAgentRef.current = focusAgent; }, [focusAgent]);
  useEffect(() => { onAgentKeyRef.current = onAgentKey; }, [onAgentKey]);

  const minimizeAgent = useCallback(
    (id: AgentId) => {
      onMinimizeAgent(id);
    },
    [onMinimizeAgent],
  );

  const registerTerminalBody = useCallback(
    (id: AgentId, registration: RegisteredTerminalBody) => {
      terminalBodiesRef.current.set(id, registration);
      return () => {
        if (terminalBodiesRef.current.get(id) === registration) {
          terminalBodiesRef.current.delete(id);
        }
      };
    },
    [],
  );

  useEffect(() => {
    if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return;

    let cancelled = false;
    let unlisten: (() => void) | null = null;

    void import('@tauri-apps/api/webview')
      .then(({ getCurrentWebview }) =>
        getCurrentWebview().onDragDropEvent((event) => {
          if (event.payload.type !== 'drop') return;
          const hit = terminalDropTargetAtPosition(
            event.payload,
            terminalBodiesRef.current,
            zStackRef.current,
          );
          if (!hit) return;
          const text = formatDroppedTerminalPaths(event.payload.paths);
          if (!text) return;
          focusAgentRef.current(hit.agentId);
          onAgentKeyRef.current(hit.agentId, text);
          hit.terminal.focusInput();
        }),
      )
      .then((stopListening) => {
        if (cancelled) {
          stopListening?.();
        } else {
          unlisten = stopListening ?? null;
        }
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  return (
    <div
      className="agent-windows-layer"
      data-testid="agent-windows-layer"
      style={{
        position: 'absolute',
        inset: 0,
        pointerEvents: 'none',
        zIndex: Z_BASE,
      }}
    >
      {liveAgents.map((id, idx) => {
        const z = Z_BASE + zStack.indexOf(id);
        const cascade = cascadePosition(idx);
        return (
          <SingleAgentWindow
            key={id}
            agentId={id}
            projectId={projectId}
            grid={grids.get(id)}
            statusEvent={status?.get(id)}
            agentMeta={agentMeta}
            focused={focusedAgent === id}
            isMinimized={minimized.has(id)}
            defaultGeom={{
              x: cascade.x,
              y: cascade.y,
              w: WINDOW_DEFAULT_W,
              h: WINDOW_DEFAULT_H,
              z,
            }}
            zOverride={z}
            onFocus={() => focusAgent(id)}
            onMinimize={() => minimizeAgent(id)}
            onKey={(bytes) => onAgentKey(id, bytes)}
            onRegisterTerminalBody={(registration) => registerTerminalBody(id, registration)}
            ghosttyTerminalEnhancement={ghosttyTerminalEnhancement}
            onRaise={() => focusAgent(id, { focusInput: false })}
            onOpenAgentDetail={() => onOpenAgentDetail?.(id)}
            onAgentContextMenu={(point) => onAgentContextMenu?.(id, point)}
            onCommend={(source) => onCommendAgent?.(id, source)}
            record={agentRecords?.[id]}
            projectName={projectName ?? projectId}
          />
        );
      })}
    </div>
  );
});

interface SingleAgentWindowProps {
  agentId: AgentId;
  projectId: string;
  grid: GridSnapshot | undefined;
  statusEvent: AgentStatusEvent | undefined;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  focused: boolean;
  isMinimized: boolean;
  defaultGeom: Pick<WindowGeometry, 'x' | 'y' | 'w' | 'h' | 'z'>;
  zOverride: number;
  onFocus: () => void;
  onMinimize: () => void;
  onKey: (bytes: string) => void;
  onRegisterTerminalBody: (registration: RegisteredTerminalBody) => () => void;
  ghosttyTerminalEnhancement: boolean;
  onRaise: () => void;
  onOpenAgentDetail?: () => void;
  onAgentContextMenu?: (point: { x: number; y: number }) => void;
  onCommend?: (source: ProjectAgentCommendSource) => void;
  record?: ProjectAgentRecord;
  projectName: string;
}

function imeAnchor(
  grid: GridSnapshot | undefined,
  cli: AgentStatusEvent['cli'] | undefined,
): { top: number; left: number } {
  const cursorVisible = grid?.cursorVisible === true;
  const cursorRow = grid?.cursorRow ?? 0;
  const cursorCol = grid?.cursorCol ?? 0;
  const cursorMoved = cursorRow > 0 || cursorCol > 0;
  const ratatuiCursorMayBeFrozen = cli != null && RATATUI_AGENT_CLIS.has(cli);
  const useCursor = ratatuiCursorMayBeFrozen
    ? cursorVisible && cursorMoved
    : cursorVisible || cursorMoved;

  if (useCursor) {
    return {
      top: cursorRow * TERMINAL_LINE_HEIGHT,
      left: cursorCol * TERMINAL_CELL_WIDTH,
    };
  }

  // Ratatui TUIs can paint their own reverse-video cursor while leaving
  // alacritty's cursor hidden or frozen at (0,0). Anchor IME to the
  // bottom terminal row in that case; the input boxes for these CLIs live
  // there even when the terminal cursor signal is not useful.
  return {
    top: Math.max((grid?.rows ?? 1) - 1, 0) * TERMINAL_LINE_HEIGHT,
    left: 0,
  };
}

function wheelMagnitudePx(e: WheelEvent): number {
  if (e.deltaMode === 1) return Math.abs(e.deltaY) * TERMINAL_LINE_HEIGHT;
  if (e.deltaMode === 2) return Math.abs(e.deltaY) * TERMINAL_LINE_HEIGHT * 8;
  return Math.abs(e.deltaY);
}

function wheelScrollLines(e: WheelEvent): number {
  const steps = Math.max(1, Math.min(8, Math.ceil(wheelMagnitudePx(e) / TERMINAL_LINE_HEIGHT)));
  // Browser wheel: negative delta = user scrolls up/older. Alacritty
  // display offset uses positive numbers for older scrollback.
  return e.deltaY < 0 ? steps : -steps;
}

function wheelMouseBytes(
  e: WheelEvent,
  grid: GridSnapshot,
  host: HTMLElement,
): string {
  const rect = host.getBoundingClientRect();
  const x = e.clientX - rect.left - BODY_HPAD;
  const y = e.clientY - rect.top;
  const col = Math.max(1, Math.min(grid.cols, Math.floor(x / TERMINAL_CELL_WIDTH) + 1));
  const row = Math.max(1, Math.min(grid.rows, Math.floor(y / TERMINAL_LINE_HEIGHT) + 1));
  const button = e.deltaY < 0 ? 64 : 65;
  const count = Math.max(1, Math.min(4, Math.ceil(wheelMagnitudePx(e) / 80)));

  if (grid.sgrMouse !== false) {
    return Array.from({ length: count }, () => `\x1b[<${button};${col};${row}M`).join('');
  }

  const legacy = `\x1b[M${String.fromCharCode(32 + button)}${String.fromCharCode(32 + col)}${String.fromCharCode(32 + row)}`;
  return legacy.repeat(count);
}

function pathFromFileUri(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  try {
    const url = new URL(trimmed);
    if (url.protocol !== 'file:') return null;
    return decodeURIComponent(url.pathname);
  } catch {
    return trimmed.startsWith('/') ? trimmed : null;
  }
}

function droppedTerminalPaths(e: DragEvent<HTMLDivElement>): string[] {
  const paths: string[] = [];
  const seen = new Set<string>();
  const addPath = (path: string | null | undefined) => {
    if (!path || seen.has(path)) return;
    seen.add(path);
    paths.push(path);
  };

  for (const file of Array.from(e.dataTransfer.files ?? [])) {
    const withPath = file as File & { path?: string; webkitRelativePath?: string };
    addPath(withPath.path || withPath.webkitRelativePath);
  }

  for (const type of ['text/uri-list', 'text/plain']) {
    const data = e.dataTransfer.getData(type);
    if (!data) continue;
    for (const line of data.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed || trimmed.startsWith('#')) continue;
      addPath(pathFromFileUri(trimmed));
    }
  }

  return paths;
}

function escapeDroppedPath(path: string): string {
  return path.replace(/([\\\s"'`$&;|<>()[\]{}*?!#~])/g, '\\$1');
}

function formatDroppedTerminalPaths(paths: string[]): string {
  const unique: string[] = [];
  const seen = new Set<string>();
  for (const path of paths) {
    if (!path || seen.has(path)) continue;
    seen.add(path);
    unique.push(path);
  }
  return unique.map(escapeDroppedPath).join(' ');
}

function closestTerminalBody(el: Element | null): HTMLElement | null {
  return el?.closest<HTMLElement>(TERMINAL_BODY_SELECTOR) ?? null;
}

export function terminalDropPointCandidates(position: { x: number; y: number }): Array<{ x: number; y: number }> {
  const dpr = window.devicePixelRatio || 1;
  const logical = { x: position.x / dpr, y: position.y / dpr };
  const raw = { x: position.x, y: position.y };
  if (dpr === 1 || (logical.x === raw.x && logical.y === raw.y)) return [raw];

  const rawInViewport =
    raw.x >= 0 &&
    raw.x <= window.innerWidth &&
    raw.y >= 0 &&
    raw.y <= window.innerHeight;
  const logicalInViewport =
    logical.x >= 0 &&
    logical.x <= window.innerWidth &&
    logical.y >= 0 &&
    logical.y <= window.innerHeight;

  if (rawInViewport && !logicalInViewport) return [raw];
  if (!rawInViewport && logicalInViewport) return [logical];

  // Tauri types this as PhysicalPosition, but on macOS WebKit drops have
  // been observed as CSS-space coordinates. Prefer raw coordinates when
  // both are plausible; otherwise OpenCode/Antigravity drops can halve into the
  // Codex window and route to the wrong PTY.
  return [raw, logical];
}

export function terminalDropTargetAtPosition(
  payload: Extract<TauriDragDropPayload, { type: 'drop' }>,
  terminals: ReadonlyMap<AgentId, RegisteredTerminalBody>,
  zStack: readonly AgentId[],
): { agentId: AgentId; terminal: RegisteredTerminalBody } | null {
  const agentIds = new Set(terminals.keys());

  for (const point of terminalDropPointCandidates(payload.position)) {
    const directBody = closestTerminalBody(document.elementFromPoint(point.x, point.y));
    const directId = directBody?.dataset.agentId as AgentId | undefined;
    const direct = directId ? terminals.get(directId) : undefined;
    if (direct && agentIds.has(directId as AgentId)) {
      return { agentId: directId as AgentId, terminal: direct };
    }
  }

  const ordered = [...zStack].reverse();
  for (const point of terminalDropPointCandidates(payload.position)) {
    for (const agentId of ordered) {
      const terminal = terminals.get(agentId);
      if (!terminal) continue;
      const rect = terminal.el.getBoundingClientRect();
      if (
        point.x >= rect.left &&
        point.x <= rect.right &&
        point.y >= rect.top &&
        point.y <= rect.bottom
      ) {
        return { agentId, terminal };
      }
    }
  }

  return null;
}

function SingleAgentWindow({
  agentId,
  projectId,
  grid,
  statusEvent,
  agentMeta,
  focused,
  isMinimized,
  defaultGeom,
  zOverride,
  onFocus,
  onMinimize,
  onKey,
  onRegisterTerminalBody,
  ghosttyTerminalEnhancement,
  onRaise,
  onOpenAgentDetail,
  onAgentContextMenu,
  onCommend,
  record,
  projectName,
}: SingleAgentWindowProps) {
  const [geom, setGeom] = useWindowGeometry({
    storageKey: `kota-v2.window-geom.${projectId}.${agentId}`,
    defaultGeom,
  });

  // Sync external z-stack into geom (parent owns z order).
  useEffect(() => {
    if (geom.z !== zOverride) {
      setGeom({ z: zOverride });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zOverride]);

  // External minimize signal flows into geom too.
  useEffect(() => {
    if (geom.minimized !== isMinimized) {
      setGeom({ minimized: isMinimized });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isMinimized]);

  // Reflow the PTY whenever the window body's actual rendered size
  // changes — driven by ResizeObserver so we catch both pointerup-
  // committed geom updates AND inline-style mutations during drag
  // (the previous geom-driven effect only fired on commit, which
  // felt non-responsive while dragging a corner). Short 50ms debounce
  // keeps the IPC call rate sane while still feeling live.
  const bodyMeasureRef = useRef<HTMLDivElement | null>(null);
  const lastSizeRef = useRef<{ cols: number; rows: number } | null>(null);
  const liveRef = useRef(false);
  useEffect(() => { liveRef.current = !!statusEvent?.running; }, [statusEvent]);
  useEffect(() => {
    const el = bodyMeasureRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return;
    let timer: number | null = null;
    const ro = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (!entry) return;
      const innerW = entry.contentRect.width;
      const innerH = entry.contentRect.height;
      if (innerW < 40 || innerH < 40) return;
      const cols = Math.max(20, Math.floor(innerW / CELL_W_RESIZE));
      const rows = Math.max(5, Math.floor(innerH / CELL_H_RESIZE));
      const last = lastSizeRef.current;
      if (last && last.cols === cols && last.rows === rows) return;
      if (timer != null) window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        timer = null;
        // Skip if PTY isn't running yet — backend would return Err
        // and the IPC roundtrip is wasted. We'll catch up on the
        // next observed size delta after spawn completes.
        if (!liveRef.current) return;
        lastSizeRef.current = { cols, rows };
        void resizeAgentPty(agentId, cols, rows).catch(() => {});
      }, 50);
    });
    ro.observe(el);
    return () => {
      if (timer != null) window.clearTimeout(timer);
      ro.disconnect();
    };
  }, [agentId]);

  // Body input — a hidden textarea overlays the grid so we capture
  // both raw keydowns AND IME composition events (Chinese / Japanese
  // input pipelines emit nothing useful via keydown alone — the real
  // text arrives via compositionend). The grid below stays read-only.
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const composingRef = useRef(false);
  const pointerDownRef = useRef<{ x: number; y: number } | null>(null);
  const suppressTerminalAutoFocusRef = useRef(false);
  const focusInputSoon = useCallback(() => {
    if (geom.minimized) return;
    const focusInput = () => {
      inputRef.current?.focus({ preventScroll: true });
    };
    window.requestAnimationFrame(focusInput);
    window.setTimeout(focusInput, 0);
  }, [geom.minimized]);
  const focusWindowAndInput = useCallback(() => {
    onFocus();
    focusInputSoon();
  }, [focusInputSoon, onFocus]);

  useEffect(() => {
    const el = bodyMeasureRef.current;
    if (!el) return;
    return onRegisterTerminalBody({ el, focusInput: focusInputSoon });
  }, [focusInputSoon, onRegisterTerminalBody]);

  useEffect(() => {
    if (focused && !geom.minimized) {
      if (suppressTerminalAutoFocusRef.current) return;
      focusInputSoon();
    } else if (!focused) {
      // Reset stale composition state on blur — without this, an
      // orphaned composition (focus stolen mid-IME) leaves
      // composingRef stuck at true, which blocks onChange from
      // clearing the textarea on subsequent input. Codex hit this
      // when toggling focus between agent windows.
      composingRef.current = false;
      if (inputRef.current) inputRef.current.value = '';
    }
  }, [focused, geom.minimized, focusInputSoon]);

  useEffect(() => {
    if (!focused || geom.minimized) return;
    const onWindowFocus = () => focusInputSoon();
    const onVisibility = () => {
      if (document.visibilityState === 'visible') focusInputSoon();
    };
    window.addEventListener('focus', onWindowFocus);
    document.addEventListener('visibilitychange', onVisibility);
    return () => {
      window.removeEventListener('focus', onWindowFocus);
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, [focused, geom.minimized, focusInputSoon]);

  const sendText = useCallback(
    (text: string) => {
      if (!text) return;
      onKey(normalizeTerminalText(text));
    },
    [onKey],
  );

  const onInputKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // IME composition: defer to compositionend.
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    // Text is handled by beforeinput/input below. keydown is only for
    // controls that do not insert text: arrows, Enter, Backspace, Ctrl-C,
    // etc. This keeps keyboard layout / Option-symbol / dead-key output
    // on the browser's text-input path instead of guessing from e.key.
    if (!e.ctrlKey && !e.metaKey && !e.altKey && e.key.length === 1) return;
    const bytes = keyToPtyBytes(e, statusEvent?.cli);
    if (bytes != null) {
      e.preventDefault();
      e.stopPropagation();
      onKey(bytes);
    }
  };
  const onInputBeforeInput = (e: FormEvent<HTMLTextAreaElement>) => {
    if (composingRef.current) return;
    const native = e.nativeEvent as InputEvent;
    const inputType = native.inputType ?? '';
    const data = native.data ?? '';
    if (inputType === 'insertText' && data) {
      e.preventDefault();
      sendText(data);
      if (inputRef.current) inputRef.current.value = '';
    }
  };
  const onInputInput = (e: FormEvent<HTMLTextAreaElement>) => {
    if (composingRef.current) return;
    const el = e.currentTarget;
    const text = el.value;
    if (!text) return;
    sendText(text);
    el.value = '';
  };
  const onInputPaste = (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const text = e.clipboardData.getData('text');
    if (!text) return;
    e.preventDefault();
    sendText(text);
    if (inputRef.current) inputRef.current.value = '';
  };
  const onInputCompositionEnd = (e: CompositionEvent<HTMLTextAreaElement>) => {
    const text = e.data || inputRef.current?.value || '';
    composingRef.current = false;
    sendText(text);
    if (inputRef.current) inputRef.current.value = '';
  };
  const onInputCompositionStart = () => {
    composingRef.current = true;
  };
  const onTerminalWheel = (e: WheelEvent<HTMLDivElement>) => {
    if (!grid) return;
    e.preventDefault();
    e.stopPropagation();
    focusInputSoon();

    if (grid.mouseMode) {
      onKey(wheelMouseBytes(e, grid, e.currentTarget));
      return;
    }

    void scrollAgentPty(agentId, wheelScrollLines(e)).catch(() => {});
  };
  const onTerminalPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.stopPropagation();
    suppressTerminalAutoFocusRef.current = true;
    onRaise();
    pointerDownRef.current = { x: e.clientX, y: e.clientY };
  };
  const onTerminalPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    e.stopPropagation();
    const start = pointerDownRef.current;
    pointerDownRef.current = null;
    const moved =
      !start ||
      Math.abs(e.clientX - start.x) > 4 ||
      Math.abs(e.clientY - start.y) > 4;
    suppressTerminalAutoFocusRef.current = false;
    if (moved || window.getSelection()?.toString()) return;
    focusInputSoon();
  };
  const onTerminalDragOver = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    e.dataTransfer.dropEffect = 'copy';
  };
  const onTerminalDrop = (e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const paths = droppedTerminalPaths(e);
    if (paths.length === 0) return;
    onFocus();
    sendText(paths.map(escapeDroppedPath).join(' '));
    focusInputSoon();
  };

  const agent = agentMeta?.[agentId] ?? AGENTS[agentId];
  const anchor = imeAnchor(grid, statusEvent?.cli);

  return (
    <div style={{ pointerEvents: 'auto' }}>
      <WindowFrame
        windowId={agentId}
        geom={geom}
        focused={focused}
        title={
          <>
            <span className="agent-commend-host win-title-avatar-host">
              <button
                type="button"
                className="win-title-avatar-button"
                onClick={(event) => {
                  event.stopPropagation();
                  onOpenAgentDetail?.();
                }}
                onPointerDown={(event) => event.stopPropagation()}
                onContextMenu={(event) => {
                  if (!onAgentContextMenu) return;
                  event.preventDefault();
                  event.stopPropagation();
                  onAgentContextMenu({ x: event.clientX, y: event.clientY });
                }}
                aria-label={`Open ${agent?.name ?? agentId} detail`}
              >
                <span className="win-title-avatar-clip" aria-hidden>
                  <span
                    className={`win-title-agent-avatar tavern-avatar-art ${agent?.avatarClass ?? providerAvatarClass(statusEvent?.cli, agentId)}`}
                    style={avatarImageStyleForId(agent?.avatarId)}
                  >
                    <span />
                    <i />
                    <b />
                  </span>
                </span>
              </button>
              <AgentCommendButton
                agentId={agentId}
                agentName={agent?.name ?? agentId}
                source="terminal-header"
                count={record?.commends}
                onCommend={onCommend ? (_id, source) => onCommend(source) : undefined}
              />
            </span>
            <span className="win-title-name-bar">
              <ProjectAgentName name={agent?.name ?? agentId} projectName={projectName} compact />
            </span>
            <span className={`win-title-status ${statusEvent?.running ? 'live' : 'quiet'}`}>
              <span className="ths-dot" />
              {statusEvent?.running === false ? 'exited' : 'running'}
            </span>
            <span className="win-title-main">
              <span className="win-title-merits" aria-label="Agent record">
                <span className="win-title-merit" title="Turns">
                  <img src={iconTurns} alt="" aria-hidden />
                  <b>{record?.turns ?? 0}</b>
                </span>
                <span className="win-title-merit" title="Commends">
                  <img src={iconCommends} alt="" aria-hidden />
                  <b>{formatCompactRecordValue(record?.commends ?? 0)}</b>
                </span>
              </span>
            </span>
          </>
        }
        onGeomChange={(patch) => setGeom(patch)}
        onFocus={focusWindowAndInput}
        onMinimize={onMinimize}
        className={ghosttyTerminalEnhancement ? 'ghostty-enhanced-frame' : undefined}
      >
        <div
          ref={bodyMeasureRef}
          className={`win-terminal-body ${ghosttyTerminalEnhancement ? 'ghostty-enhanced-terminal' : ''}`}
          data-agent-terminal-body="true"
          data-agent-id={agentId}
          style={{
            position: 'relative',
            width: '100%',
            height: '100%',
            padding: `0 ${BODY_HPAD}px`,
            boxSizing: 'border-box',
            overflow: 'hidden',
          }}
          onPointerDown={onTerminalPointerDown}
          onPointerUp={onTerminalPointerUp}
          onWheel={onTerminalWheel}
          onDragOver={onTerminalDragOver}
          onDrop={onTerminalDrop}
        >
          <TerminalGrid snapshot={grid} enhanced={ghosttyTerminalEnhancement} />
          <textarea
            ref={inputRef}
            className="win-ime-capture"
            aria-label={`${agent?.name ?? agentId} terminal input`}
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            rows={1}
            wrap="off"
            onKeyDown={onInputKeyDown}
            onBeforeInput={onInputBeforeInput}
            onInput={onInputInput}
            onPaste={onInputPaste}
            onCompositionStart={onInputCompositionStart}
            onCompositionEnd={onInputCompositionEnd}
            // The OS anchors the IME candidate window to the focused
            // textarea's caret. Start this transparent capture surface at
            // the terminal anchor, but let it extend to the window edge so
            // real pinyin/marked-text composition does not immediately
            // scroll inside a tiny 1-line textarea.
            style={{
              position: 'absolute',
              top: anchor.top,
              left: anchor.left,
              right: 0,
              bottom: 0,
              minWidth: TERMINAL_CELL_WIDTH * 32,
              minHeight: TERMINAL_LINE_HEIGHT,
              opacity: 0,
              border: 'none',
              outline: 'none',
              resize: 'none',
              background: 'transparent',
              color: 'transparent',
              caretColor: 'transparent',
              padding: 0,
              margin: 0,
              fontFamily: TERMINAL_FONT_FAMILY,
              fontSize: TERMINAL_FONT_SIZE,
              lineHeight: `${TERMINAL_LINE_HEIGHT}px`,
              whiteSpace: 'pre',
              overflow: 'hidden',
              pointerEvents: 'none',
            }}
          />
        </div>
      </WindowFrame>
    </div>
  );
}

function providerAvatarClass(cli: AgentStatusEvent['cli'] | undefined, id: AgentId): string {
  return avatarClassForAgentFallback(cli, id);
}

function formatCompactRecordValue(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}m`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}k`;
  return String(value);
}

function keyToPtyBytes(e: KeyboardEvent, cli?: AgentStatusEvent['cli']): string | null {
  if (['Shift', 'Meta', 'Control', 'Alt', 'CapsLock'].includes(e.key)) return null;
  if (e.key === 'ArrowUp') return '\x1b[A';
  if (e.key === 'ArrowDown') return '\x1b[B';
  if (e.key === 'ArrowRight') return '\x1b[C';
  if (e.key === 'ArrowLeft') return '\x1b[D';
  if (e.key === 'Enter') return cli === 'kimi' ? '\x1b[13;1u' : '\r';
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
