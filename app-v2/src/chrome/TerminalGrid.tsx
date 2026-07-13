/** TerminalGrid — renders a `GridSnapshot` from `pty/ansi.rs`.
 *
 *  Per ARCHITECTURE-INVARIANTS:
 *    I-4 alacritty Term is the SoT; this is the dumb rendering side.
 *    I-5 zero protocol parsing — we just paint cells with attrs / colors.
 *
 *  Each row coalesces consecutive same-style cells into a single span
 *  (run-length compression). Massive DOM-node reduction (typical: 2-5
 *  spans per row vs 100+) and lets `findByText` match line text
 *  through the standard textContent traversal. */

import {
  ATTR_BOLD,
  ATTR_DIM,
  ATTR_INVERSE,
  ATTR_ITALIC,
  ATTR_STRIKEOUT,
  ATTR_UNDERLINE,
  ATTR_WIDE,
  type GridCell,
  type GridSnapshot,
} from '../types/agent-pty';

const DEFAULT_FG = 'var(--terminal-fg, var(--ivory, #ece6d6))';
const DEFAULT_BG = 'var(--terminal-bg, transparent)';
const CURSOR_BG = 'var(--terminal-cursor-bg, var(--brass-bright, #d6a76b))';

/** Default cell metrics. Exported so the IME-capture textarea over the
 *  grid (in AgentWindowsLayer) can be positioned at the same pixel
 *  coordinates as the rendered cursor — the OS anchors the IME
 *  candidate window to the textarea's caret, so the two must agree on
 *  cell size or the candidate box drifts. */
export const TERMINAL_FONT_SIZE = 12;
export const TERMINAL_CELL_WIDTH = 7.2;
export const TERMINAL_LINE_HEIGHT = Math.round(TERMINAL_FONT_SIZE * 1.2);
export const TERMINAL_FONT_FAMILY =
  'var(--font-mono, ui-monospace, SFMono-Regular, Menlo, Consolas, monospace)';

function rgbHex(n: number | undefined): string | undefined {
  if (n == null) return undefined;
  return '#' + n.toString(16).padStart(6, '0');
}

function sameStyle(a: GridCell, b: GridCell): boolean {
  return a.fg === b.fg && a.bg === b.bg && (a.attrs ?? 0) === (b.attrs ?? 0);
}

function styleOf(cell: GridCell): React.CSSProperties {
  const a = cell.attrs ?? 0;
  const inverse = (a & ATTR_INVERSE) !== 0;
  const fgRaw = rgbHex(cell.fg);
  const bgRaw = rgbHex(cell.bg);
  return {
    color: inverse ? bgRaw ?? DEFAULT_BG : fgRaw ?? DEFAULT_FG,
    background: inverse ? fgRaw ?? DEFAULT_FG : bgRaw ?? DEFAULT_BG,
    fontWeight: a & ATTR_BOLD ? 700 : 400,
    fontStyle: a & ATTR_ITALIC ? 'italic' : 'normal',
    textDecoration:
      (a & ATTR_UNDERLINE ? 'underline ' : '') +
      (a & ATTR_STRIKEOUT ? 'line-through' : ''),
    opacity: a & ATTR_DIM ? 0.6 : 1,
  };
}

function RowSpans({ row, cellWidth }: { row: GridCell[]; cellWidth: number }) {
  if (row.length === 0) return null;
  // Two-pass emission rather than pure run-length compression because
  // wide cells need their own inline-block span pinned to 2*cellWidth
  // (the browser's CJK font fallback paints slightly under or over a
  // pure 2x width and the drift accumulates into "cursor floats away
  // from end of typed text"). Spacer cells (alacritty's WIDE_CHAR_SPACER,
  // emitted by ansi.rs as `ch: ""`) are dropped — the wide cell to their
  // left already covers the visual span.
  const out: React.ReactNode[] = [];
  let pending: { cell: GridCell; text: string } | null = null;
  const flush = () => {
    if (pending) {
      out.push(
        <span key={out.length} style={styleOf(pending.cell)}>
          {pending.text}
        </span>,
      );
      pending = null;
    }
  };
  for (let i = 0; i < row.length; i++) {
    const cell = row[i]!;
    const attrs = cell.attrs ?? 0;
    if ((attrs & ATTR_WIDE) !== 0) {
      flush();
      out.push(
        <span
          key={out.length}
          style={{
            ...styleOf(cell),
            display: 'inline-block',
            width: cellWidth * 2,
          }}
        >
          {cell.ch || ' '}
        </span>,
      );
      continue;
    }
    if (cell.ch === '') continue; // spacer to the right of a wide char
    const ch = cell.ch ?? ' ';
    if (pending && sameStyle(cell, pending.cell)) {
      pending.text += ch;
    } else {
      flush();
      pending = { cell, text: ch };
    }
  }
  flush();
  return <>{out}</>;
}

export interface TerminalGridProps {
  snapshot: GridSnapshot | undefined;
  /** px size for the cell — height inferred at 1.2× line. */
  cellWidth?: number;
  fontSize?: number;
  enhanced?: boolean;
}

export function TerminalGrid({
  snapshot,
  cellWidth = TERMINAL_CELL_WIDTH,
  fontSize = TERMINAL_FONT_SIZE,
  enhanced = false,
}: TerminalGridProps) {
  if (!snapshot) {
    return (
      <div
        className={`term-grid term-grid-empty ${enhanced ? 'ghostty-enhanced-grid' : ''}`}
        data-testid="term-grid-empty"
      />
    );
  }

  const { cols, rows, cells, cursorRow, cursorCol, cursorVisible } = snapshot;
  const lineHeight = Math.round(fontSize * 1.2);

  // Slice cells row-by-row.
  const rowSlices: GridCell[][] = [];
  for (let r = 0; r < rows; r++) {
    rowSlices.push(cells.slice(r * cols, (r + 1) * cols));
  }

  return (
    <div
      className={`term-grid ${enhanced ? 'ghostty-enhanced-grid' : ''}`}
      data-testid="term-grid"
      style={{
        position: 'relative',
        fontFamily: TERMINAL_FONT_FAMILY,
        fontSize,
        lineHeight: `${lineHeight}px`,
        whiteSpace: 'pre',
      }}
    >
      {rowSlices.map((row, ri) => (
        <div key={ri} style={{ height: lineHeight }}>
          <RowSpans row={row} cellWidth={cellWidth} />
        </div>
      ))}
      {cursorVisible && (
        <div
          aria-hidden
          data-testid="term-grid-cursor"
          style={{
            position: 'absolute',
            left: cursorCol * cellWidth,
            top: cursorRow * lineHeight,
            width: cellWidth,
            height: lineHeight,
            background: CURSOR_BG,
            mixBlendMode: 'difference',
            pointerEvents: 'none',
          }}
        />
      )}
    </div>
  );
}
