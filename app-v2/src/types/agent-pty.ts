/** Agent PTY types — mirror of `pty/agent.rs` Rust structs.
 *
 *  Per ARCHITECTURE-INVARIANTS:
 *    I-4 same alacritty/portable-pty infra as Smart Terminal
 *    I-5 zero protocol parsing — frontend renders ANSI ScrollbackLines as-is
 *    I-15 backend injects 4 KOTA path env vars + git author identity
 *
 *  The seat on the round table subscribes to `pty://agent/{id}/{output|exit|status}`
 *  Tauri events; this module is the single source of those topic names.   */

/** A single styled cell in the terminal grid. Mirrors `pty/ansi.rs::GridCell`.
 *  `fg` / `bg` are 0xRRGGBB ints; `undefined` = renderer default.
 *  `attrs` is a bitmask: 1=bold 2=italic 4=underline 8=dim 16=inverse 32=strikeout. */
export interface GridCell {
  ch: string;
  fg?: number;
  bg?: number;
  attrs?: number;
}

export const ATTR_BOLD = 1 << 0;
export const ATTR_ITALIC = 1 << 1;
export const ATTR_UNDERLINE = 1 << 2;
export const ATTR_DIM = 1 << 3;
export const ATTR_INVERSE = 1 << 4;
export const ATTR_STRIKEOUT = 1 << 5;
/** Wide-glyph cell (CJK / fullwidth) — occupies 2 grid columns. The
 *  next cell is alacritty's WIDE_CHAR_SPACER and arrives as `ch: ""`.
 *  Renderer pins these cells to `2 * cellWidth` via inline-block so the
 *  monospace + CJK-fallback font's natural width drift can't accumulate. */
export const ATTR_WIDE = 1 << 6;

export interface GridSnapshot {
  cols: number;
  rows: number;
  /** row-major; length = cols * rows */
  cells: GridCell[];
  cursorRow: number;
  cursorCol: number;
  cursorVisible: boolean;
  /** alacritty local scrollback offset; 0 means live bottom. */
  displayOffset?: number;
  /** Terminal app enabled mouse tracking, so wheel should be sent to PTY. */
  mouseMode?: boolean;
  /** Mouse tracking expects SGR coordinates instead of legacy X10 bytes. */
  sgrMouse?: boolean;
}

/** Synthesize a GridSnapshot from a list of plain text lines, one per
 *  row. Used by the browser-dev mock paths and Vitest fixtures that
 *  predate the alacritty grid model. Real PTY sessions get snapshots
 *  straight from `pty/ansi.rs::AnsiLineDecoder::snapshot()`. */
export function linesToGridSnapshot(
  lines: { text: string }[],
  cols = 100,
  rows = 28,
): GridSnapshot {
  const cells: GridCell[] = new Array(cols * rows);
  // Take the LAST `rows` lines so newest content stays visible.
  const visible = lines.slice(-rows);
  for (let r = 0; r < rows; r++) {
    const row = visible[r]?.text ?? '';
    for (let c = 0; c < cols; c++) {
      cells[r * cols + c] = { ch: row[c] ?? ' ' };
    }
  }
  return {
    cols,
    rows,
    cells,
    cursorRow: Math.min(visible.length, rows - 1),
    cursorCol: Math.min((visible[visible.length - 1]?.text.length ?? 0), cols - 1),
    cursorVisible: false,
  };
}

/** Which CLI the spawned process is. Hero shell decides this; incarnation
 *  cannot change shell (per I-20.2). */
export type AgentCli = 'claude' | 'codex' | 'antigravity' | 'opencode' | 'pi';

export interface AgentSpawnRequest {
  agentId: string;
  cli: AgentCli;
  /** Absolute path to the incarnation workspace (.agent-workspaces/{id}/). */
  cwd: string;
  /** Legacy absolute path to the Kota project root — feeds KOTA_PROJECT_ROOT env. */
  projectRoot: string;
  /** Absolute path to the actual git worktree used as the agent CWD. */
  worktreeRoot?: string;
  /** Absolute path to project memory files materialized on this machine. */
  sharedDir?: string;
  /** Absolute path to Kota room/project rules for this workspace. */
  rulesDir?: string;
  /** Absolute path to CLAUDE.md / AGENTS.md used by this CLI. */
  adapterPath?: string;
  /** CLI startup args parsed from the incarnation's SHELL.yaml. */
  args?: string[];
  /** Provider-native session id when Kota owns or has discovered it. */
  sessionId?: string | null;
  projectId?: string;
  projectRemote?: string;
  projectBaseRef?: string;
  /** Confirmed user intent to end a foreign live session and resume here. */
  takeover?: boolean;
}

export interface AgentRoute {
  agentId: string;
  outputEvent: string;
  exitEvent: string;
  statusEvent: string;
  workEvent?: string;
}

export interface AgentSessionLeaseConflict {
  code: 'agent-session-lease-conflict';
  agentId: string;
  ownerPid: number;
  childPid?: number | null;
}

export interface AgentOutputEvent {
  agentId: string;
  /** Full visible-grid snapshot from `pty/agent.rs`. */
  snapshot: GridSnapshot;
}

export interface AgentStatusEvent {
  agentId: string;
  running: boolean;
  cli: AgentCli;
  cwd: string;
  phase?: 'spawned' | 'firstBytes' | 'terminalQuery' | 'exited';
  detail?: string;
  resolvedBin?: string;
  ttyName?: string;
  elapsedMs?: number;
  byteCount?: number;
  sessionId?: string;
  forkable?: boolean;
  sessionSource?: 'spawn-output' | 'native-log' | 'violet' | 'manual' | string;
}

export interface AgentExitEvent {
  agentId: string;
  code: number | null;
}

export type AgentWorkState = 'idle' | 'working' | 'maybeIdle' | 'failed' | 'interrupted';

export interface AgentWorkStateEvent {
  agentId: string;
  state: AgentWorkState;
  timestamp: string;
  startedAt?: string | null;
  lastActivityAt?: string | null;
  cli?: AgentCli | string;
  cwd?: string;
  sessionId?: string | null;
  turnId?: string | null;
  reason?: string | null;
  sourcePath?: string | null;
  nativeEventId?: string | null;
}

export interface AgentSummary {
  agentId: string;
  cli: AgentCli;
  cwd: string;
  projectRoot: string;
  projectId?: string | null;
  running: boolean;
}

/** Topic helpers — keep in sync with `pty/agent.rs::AgentRoute::new`. */
export function agentOutputTopic(agentId: string): string {
  return `pty://agent/${agentId}/output`;
}
export function agentExitTopic(agentId: string): string {
  return `pty://agent/${agentId}/exit`;
}
export function agentStatusTopic(agentId: string): string {
  return `pty://agent/${agentId}/status`;
}
export function agentWorkTopic(agentId: string): string {
  return `pty://agent/${agentId}/work`;
}
