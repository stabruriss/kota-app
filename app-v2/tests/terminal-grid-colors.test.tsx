import type { ReactElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { TerminalGrid } from '../src/chrome/TerminalGrid';
import {
  ATTR_BOLD, ATTR_DIM, ATTR_INVERSE, ATTR_ITALIC, ATTR_STRIKEOUT, ATTR_UNDERLINE, ATTR_WIDE,
  type GridCell, type GridSnapshot,
} from '../src/types/agent-pty';

const foreground = 'var(--terminal-fg, var(--ivory, #ece6d6))';
const background = 'var(--terminal-bg, transparent)';
const reverseForeground = 'var(--term-bg, #090A0B)';

function snapshot(cells: GridCell[]): GridSnapshot {
  return { cols: cells.length, rows: 1, cells, cursorRow: 0, cursorCol: 0, cursorVisible: false };
}

function renderGrid(element: ReactElement): HTMLElement {
  const container = document.createElement('div');
  container.innerHTML = renderToStaticMarkup(element);
  return container;
}

function firstSpan(container: HTMLElement): HTMLSpanElement {
  const span = container.querySelector('.term-grid > div > span');
  expect(span).not.toBeNull();
  return span as HTMLSpanElement;
}

// Happy DOM drops CSS-variable colors assigned by React and does not resolve
// them. Check the actual component's serialized CSS without pretending this is
// browser paint verification; computed colors are covered by the WebKit probe.
function style(element: HTMLElement, property: string): string | undefined {
  return element.getAttribute('style')?.split(';').map(part => part.trim())
    .find(part => part.startsWith(`${property}:`))?.slice(property.length + 1).trim();
}

describe('TerminalGrid · reverse-video colors', () => {
  it.each([false, true])('keeps default reverse-video text opaque (enhanced=%s)', (enhanced) => {
    const container = renderGrid(<TerminalGrid enhanced={enhanced} snapshot={snapshot(
      [...'/model'].map(ch => ({ ch, attrs: ATTR_INVERSE })),
    )} />);
    const span = firstSpan(container);
    expect(span.textContent).toBe('/model');
    // Drawing a transparent terminal background is valid; using it as text is not.
    expect(style(span, 'color')).toBe(reverseForeground);
    expect(style(span, 'background')).toBe(foreground);
    expect(container.querySelectorAll('.term-grid > div > span')).toHaveLength(1);
    expect(container.querySelector('.term-grid')?.classList.contains('ghostty-enhanced-grid')).toBe(enhanced);
  });

  it.each([false, true])('preserves ordinary default text and transparent backgrounds (enhanced=%s)', (enhanced) => {
    const container = renderGrid(<TerminalGrid enhanced={enhanced} snapshot={snapshot([{ ch: 'x' }])} />);
    const span = firstSpan(container);
    expect(style(span, 'color')).toBe(foreground);
    expect(style(span, 'background')).toBe(background);
  });

  it.each([
    [0x112233, '#112233'],
    [0x000000, '#000000'],
  ] as const)('preserves explicit reverse-video backgrounds, including RGB zero (%s)', (bg, color) => {
    const container = renderGrid(<TerminalGrid snapshot={snapshot([
      { ch: 'x', attrs: ATTR_INVERSE, fg: 0xabcdef, bg },
    ])} />);
    const span = firstSpan(container);
    expect(style(span, 'color')).toBe(color);
    expect(style(span, 'background')).toBe('#abcdef');
  });

  it('uses explicit ANSI foreground as the reverse highlight while keeping default reverse text opaque', () => {
    const container = renderGrid(<TerminalGrid snapshot={snapshot([
      { ch: 'x', attrs: ATTR_INVERSE, fg: 0x11a8cd },
    ])} />);
    const span = firstSpan(container);
    expect(style(span, 'color')).toBe(reverseForeground);
    expect(style(span, 'background')).toBe('#11a8cd');
  });

  it('leaves explicit non-reverse ANSI colors unchanged', () => {
    const container = renderGrid(<TerminalGrid snapshot={snapshot([
      { ch: 'x', fg: 0x112233, bg: 0x445566 },
    ])} />);
    const span = firstSpan(container);
    expect(style(span, 'color')).toBe('#112233');
    expect(style(span, 'background')).toBe('#445566');
  });

  it('preserves bold, dim, italic, underline and strikeout on reverse-video text', () => {
    const container = renderGrid(<TerminalGrid snapshot={snapshot([
      { ch: 'x', attrs: ATTR_INVERSE | ATTR_BOLD | ATTR_DIM | ATTR_ITALIC | ATTR_UNDERLINE | ATTR_STRIKEOUT },
    ])} />);
    const span = firstSpan(container);
    expect(style(span, 'color')).toBe(reverseForeground);
    expect(style(span, 'font-weight')).toBe('700');
    expect(style(span, 'opacity')).toBe('0.6');
    expect(style(span, 'font-style')).toBe('italic');
    expect(style(span, 'text-decoration')).toBe('underline line-through');
  });

  it('keeps reverse wide glyphs at two cells and drops their spacers', () => {
    const container = renderGrid(<TerminalGrid cellWidth={10} snapshot={snapshot([
      { ch: '中', attrs: ATTR_INVERSE | ATTR_WIDE }, { ch: '' },
    ])} />);
    const span = firstSpan(container);
    expect(span.textContent).toBe('中');
    expect(style(span, 'color')).toBe(reverseForeground);
    expect(style(span, 'display')).toBe('inline-block');
    expect(style(span, 'width')).toBe('20px');
    expect(container.querySelectorAll('.term-grid > div > span')).toHaveLength(1);
  });

  it('keeps reverse runs separate from ordinary text without changing row text', () => {
    const container = renderGrid(<TerminalGrid snapshot={snapshot([
      { ch: 'a', attrs: ATTR_INVERSE }, { ch: 'b', attrs: ATTR_INVERSE }, { ch: 'c' },
    ])} />);
    const spans = container.querySelectorAll<HTMLSpanElement>('.term-grid > div > span');
    expect(spans).toHaveLength(2);
    expect(spans[0].textContent).toBe('ab');
    expect(spans[1].textContent).toBe('c');
    expect(style(spans[1], 'color')).toBe(foreground);
    expect(style(spans[1], 'background')).toBe(background);
  });

  it('does not change the cursor overlay or positioning', () => {
    const container = renderGrid(<TerminalGrid cellWidth={10} fontSize={12} snapshot={{
      ...snapshot([{ ch: 'x', attrs: ATTR_INVERSE }, { ch: ' ' }]),
      cursorCol: 1, cursorVisible: true,
    }} />);
    const cursor = container.querySelector<HTMLElement>('[data-testid="term-grid-cursor"]');
    expect(cursor).not.toBeNull();
    expect(style(cursor!, 'left')).toBe('10px');
    expect(style(cursor!, 'top')).toBe('0');
    expect(style(cursor!, 'width')).toBe('10px');
    expect(style(cursor!, 'height')).toBe('14px');
    expect(style(cursor!, 'background')).toBe('var(--terminal-cursor-bg, var(--brass-bright, #d6a76b))');
    expect(style(cursor!, 'mix-blend-mode')).toBe('difference');
  });
});
