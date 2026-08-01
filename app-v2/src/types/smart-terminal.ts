/** Smart Terminal types (M4 PTY mocked for now).
 *
 *  The Smart Terminal is the user's own persistent shell, unscoped to
 *  projects (see `product-design/Smart Terminal.md`). For this chrome-
 *  first landing pass the PTY is faked — scrollback lines and NL
 *  translations come from `app-v2/src/mock/smart-terminal.ts`. When M4
 *  (portable-pty + alacritty_terminal) lands we swap the mock out for
 *  real stdout streams without changing this type surface.           */

/** ANSI-ish line kinds used by the chrome. Maps 1:1 to CSS classes in
 *  canvas.css (`.sh-*`). Keep narrow — we colour by role, not raw ANSI.  */
export type ScrollbackKind =
  | 'prompt'   // `~/kota ›`
  | 'cmd'      // what the user typed
  | 'dim'      // faint informational (e.g. "Last login…")
  | 'ok'       // ✓ pass / success
  | 'err'      // error red
  | 'file'     // filenames / paths — buttercup
  | 'str'      // string literals — sky
  | 'ai'       // ● agent italic (claude-style)
  | 'path'     // absolute paths in brass
  | 'ask'      // `↳ ask: <the user's english>` muted italic
  | 'plain';   // everything else (ivory)

export interface ScrollbackLine {
  kind: ScrollbackKind;
  text: string;
  /** Optional running total if this row has multiple segments (prompt
   *  + command, say); the component splits on those boundaries. Right
   *  now the mock fixture uses a single `text` per line. */
}

/** M6.A grid refactor — replaces the line-stream payload. The full
 *  visible-grid snapshot from `pty/ansi.rs::GridSnapshot` is sent on
 *  every PTY read batch. Frontend renders cells via `<TerminalGrid>`.
 *  The legacy line-based fields are retained on the type strictly for
 *  Vitest fixtures that haven't been updated yet.                     */
export interface SmartOutputPayload {
  snapshot: import('./agent-pty').GridSnapshot;
}

export interface SmartOutputEvent extends SmartOutputPayload {
  ptyId: string;
}

export interface SmartStatusPayload {
  running: boolean;
  cwd: string;
}

export interface SmartStatusEvent extends SmartStatusPayload {
  ptyId: string;
}

export interface SmartExitPayload {
  code: number | null;
}

export interface SmartExitEvent extends SmartExitPayload {
  ptyId: string;
}

/** Fires when the controlling tty's foreground process group transitions
 *  back to the shell after a subprocess (claude / codex / ssh / make /
 *  …) yielded the tty. The frontend uses this to clear `tuiRunning` —
 *  `SmartExitEvent` only fires on shell-process death and misses
 *  subprocess exits like `/exit` inside claude code. */
export interface SmartTuiExitEvent {
  ptyId: string;
}

export interface SmartPtySummary {
  ptyId: string;
  cwd: string;
  running: boolean;
}

/** Smart command sub-state. Shell focus stays separate from this local
 *  prompt: clicking the bottom row enters typing, Enter translates,
 *  and command results are inserted into the PTY line without running. */
export type NLMode =
  | 'shell'
  | 'typing'
  | 'thinking'
  | 'prefilled'
  | 'escapeHatch';

export type ShellStatus = 'quiet' | 'live' | 'dead';

/** Magi can translate #ask prompts through Claude or Codex. The setting
 *  controls priority; if the primary translator fails, the other is tried. */
export type MagiProvider = 'claude' | 'codex';

export type FallbackCli = MagiProvider | 'agy' | 'opencode' | 'pi' | 'kimi';

export interface TranslateResult {
  kind: 'command' | 'escape';
  value: string;
  provider?: MagiProvider;
  /** Italic hint above the prompt row when `kind === 'escape'`. */
  hint?: string;
}
