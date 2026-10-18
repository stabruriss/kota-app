import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MarkdownText } from '../src/chrome/VioletRoomPanel';
import { readMermaidTheme, requestMermaidPreview } from '../src/lib/mermaid-preview';

const engine = vi.hoisted(() => ({ initialize: vi.fn(), render: vi.fn() }));
vi.mock('mermaid', () => ({ default: engine }));
vi.mock('mermaid/dist/mermaid.esm.min.mjs', async () => ({ default: (await import('mermaid')).default }));
const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 200" style="max-width:800px"><text>Diagram</text></svg>';
const fence = (source: string) => '```mermaid\n' + source + '\n```';
const theme = { background: '#191a1c', surface: '#2b2c2f', text: '#e5e1d8', muted: '#b2aea6', border: '#625f59', accent: '#9d735f', font: 'sans-serif' };

beforeEach(() => {
  engine.render.mockReset().mockResolvedValue({ svg });
  engine.initialize.mockReset();
  vi.stubGlobal('IntersectionObserver', undefined);
});
afterEach(() => vi.unstubAllGlobals());

describe('Mermaid in shared Markdown', () => {
  it('waits for a closed fence and keeps the rendered block when later text streams in', async () => {
    const source = 'flowchart LR\nA[Stream] --> B';
    const view = render(<MarkdownText text={'```mermaid\n' + source} />);
    expect(view.container.querySelector('pre code')?.textContent).toBe(source);
    expect(engine.render).not.toHaveBeenCalled();
    view.rerender(<MarkdownText text={fence(source)} />);
    const image = await screen.findByRole('img', { name: 'Mermaid diagram' });
    expect(image).toHaveAttribute('width', '800');
    expect(image).toHaveAttribute('height', '200');
    expect(image).toHaveAttribute('src', expect.stringMatching(/^data:image\/svg\+xml/));
    expect(view.container.querySelector('details code')?.textContent).toBe(source);
    view.rerender(<MarkdownText text={fence(source) + '\n\nMore text'} />);
    expect(screen.getByRole('img')).toBe(image);
    expect(engine.render).toHaveBeenCalledTimes(1);
    expect(engine.initialize).toHaveBeenCalledWith(expect.objectContaining({ securityLevel: 'strict', htmlLabels: false, startOnLoad: false }));
    expect(view.container.querySelector('svg')).toBeNull();
  });

  it('does not queue offscreen diagrams; entering the viewport starts rendering', async () => {
    let notify!: IntersectionObserverCallback;
    vi.stubGlobal('IntersectionObserver', class {
      constructor(callback: IntersectionObserverCallback) { notify = callback; }
      observe() {}
      disconnect() {}
    });
    render(<MarkdownText text={fence('flowchart LR\nA[Visible] --> B')} />);
    await act(async () => {});
    expect(engine.render).not.toHaveBeenCalled();
    act(() => notify([{ isIntersecting: true } as IntersectionObserverEntry], {} as IntersectionObserver));
    expect(await screen.findByRole('img')).toBeInTheDocument();
  });

  it('falls back to the unchanged ordinary code block on parse/render failure and can recover after an edit', async () => {
    const source = 'flowchart LR\nA[invalid <&';
    engine.render.mockRejectedValueOnce(new Error('parse error'));
    const view = render(<MarkdownText text={fence(source)} />);
    await waitFor(() => expect(view.container.querySelector('details')).toBeNull());
    expect(view.container.querySelector('pre')?.className).toBe('violet-md-code');
    expect(view.container.querySelector('code')?.textContent).toBe(source);
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    view.rerender(<MarkdownText text={fence('flowchart LR\nA[Corrected] --> B')} />);
    expect(await screen.findByRole('img')).toBeInTheDocument();
  });

  it('also falls back if the SVG image cannot be decoded', async () => {
    const source = 'flowchart LR\nA[Decode] --> B';
    const view = render(<MarkdownText text={fence(source)} />);
    fireEvent.error(await screen.findByRole('img'));
    expect(view.container.querySelector('pre code')?.textContent).toBe(source);
    expect(view.container.querySelector('details')).toBeNull();
  });

  it('leaves HTML (including unfinished fences) and other code blocks unchanged', () => {
    const html = '<b>Drawing</b>';
    const view = render(<MarkdownText text={'```html\n' + html} />);
    expect(view.container.querySelector('iframe')).toHaveAttribute('sandbox', 'allow-same-origin');
    expect(view.container.querySelector('iframe')?.getAttribute('srcdoc')).toContain(html);
    view.rerender(<MarkdownText text={'```typescript\nconst n = 1;\n```'} />);
    expect(view.container.querySelector('pre code')?.textContent).toBe('const n = 1;');
    expect(engine.render).not.toHaveBeenCalled();
  });

  it('drops a late render after the message changed', async () => {
    let finish!: (result: { svg: string }) => void;
    engine.render.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const view = render(<MarkdownText text={fence('flowchart LR\nA[Old] --> B')} />);
    await waitFor(() => expect(engine.render).toHaveBeenCalledTimes(1));
    view.rerender(<MarkdownText text="Replacement text" />);
    await act(async () => finish({ svg }));
    expect(screen.queryByRole('img')).not.toBeInTheDocument();
    expect(screen.getByText('Replacement text')).toBeInTheDocument();
  });

  it('reads the local CSS palette and rerenders if an ancestor changes its tokens', async () => {
    // Happy DOM does not resolve inherited custom properties. Supply the
    // browser's computed palette; the real inheritance is checked in WebKit.
    const computedStyle = window.getComputedStyle.bind(window);
    vi.stubGlobal('getComputedStyle', (element: Element) => computedStyle(element.closest('[data-test-palette]') ?? element));
    const view = render(<div data-test-palette style={{ '--text': '#123456' } as React.CSSProperties}>
      <MarkdownText text={fence('flowchart LR\nA[Theme] --> B')} />
    </div>);
    await screen.findByRole('img');
    expect(engine.initialize.mock.calls.at(-1)?.[0].themeVariables.primaryTextColor).toBe('#123456');
    view.rerender(<div data-test-palette style={{ '--text': '#654321' } as React.CSSProperties}>
      <MarkdownText text={fence('flowchart LR\nA[Theme] --> B')} />
    </div>);
    await waitFor(() => expect(engine.render).toHaveBeenCalledTimes(2));
    expect(engine.initialize.mock.calls.at(-1)?.[0].themeVariables.primaryTextColor).toBe('#654321');
    expect(readMermaidTheme(view.container.firstElementChild as HTMLElement).text).toBe('#654321');
  });
});

describe('Mermaid work queue and bounded preview cache', () => {
  it('shares identical requests, serializes different diagrams, and skips cancelled queued work', async () => {
    let finish!: (value: { svg: string }) => void;
    engine.render.mockReturnValueOnce(new Promise(resolve => { finish = resolve; }));
    const a = requestMermaidPreview('flowchart LR\nA[Queue] --> B', theme);
    const duplicate = requestMermaidPreview('flowchart LR\nA[Queue] --> B', theme);
    const skipped = requestMermaidPreview('flowchart LR\nA[Cancelled] --> B', theme);
    const b = requestMermaidPreview('flowchart LR\nA[Next] --> B', theme);
    skipped.cancel();
    await waitFor(() => expect(engine.render).toHaveBeenCalledTimes(1));
    finish({ svg });
    expect(await a.promise).toEqual(expect.objectContaining({ width: 800, height: 200 }));
    expect(await a.promise).toEqual(await duplicate.promise);
    await b.promise;
    expect(engine.render).toHaveBeenCalledTimes(2);
    expect(engine.render.mock.calls.map(call => call[1])).not.toContain('flowchart LR\nA[Cancelled] --> B');
    expect(document.querySelector('[aria-hidden="true"]')).toBeNull();
  });

  it('bounds completed entries and keys previews by source and palette', async () => {
    const source = 'flowchart LR\nA[Cache] --> B';
    await requestMermaidPreview(source, theme).promise;
    await requestMermaidPreview(source, theme).promise;
    expect(engine.render).toHaveBeenCalledTimes(1);
    await requestMermaidPreview(source, { ...theme, text: '#fedcba' }).promise;
    expect(engine.render).toHaveBeenCalledTimes(2);
    for (let i = 0; i < 64; i++) await requestMermaidPreview(`flowchart LR\nA[Cache ${i}] --> B`, theme).promise;
    await requestMermaidPreview(source, theme).promise;
    expect(engine.render).toHaveBeenCalledTimes(67);
  });
});
