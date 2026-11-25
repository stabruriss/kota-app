import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { TopBar } from '../src/chrome/TopBar';

describe('TopBar project metadata', () => {
  it('exposes real GitHub workspace paths and repo URL on the active tab', () => {
    render(
      <TopBar
        projects={[
          {
            id: 'owner-repo',
            name: 'owner/repo',
            path: '/Users/me/Kota/Projects/owner/repo',
            sourcePath: '/Users/me/Kota/Projects/owner/repo',
            accountPath: '/Users/me/Kota/Workspaces/owner-repo',
            githubUrl: 'https://github.com/owner/repo',
          },
        ]}
        activeProjectId="owner-repo"
        onSelectProject={vi.fn()}
        onNewProject={vi.fn()}
        onCloseProject={vi.fn()}
        onOpenTavern={vi.fn()}
      />,
    );

    const tab = screen.getByTestId('tab-owner-repo');
    expect(tab.querySelector('.tab-name')).toHaveTextContent('repo');
    const title = tab.getAttribute('title') ?? '';
    expect(title).toContain('GitHub: https://github.com/owner/repo');
    expect(title).toContain('Local: /Users/me/Kota/Projects/owner/repo');
    expect(title).toContain('Account: /Users/me/Kota/Workspaces/owner-repo');
  });

  it('turns the Tavern control into a project back button while Tavern is open', async () => {
    const onOpenTavern = vi.fn();
    render(
      <TopBar
        projects={[]}
        activeProjectId={null}
        onSelectProject={vi.fn()}
        onNewProject={vi.fn()}
        onCloseProject={vi.fn()}
        onOpenTavern={onOpenTavern}
        tavernOpen
      />,
    );

    const button = screen.getByTestId('tavern-btn');
    expect(button).toHaveAttribute('aria-label', 'Back to project');
    await userEvent.click(button);
    expect(onOpenTavern).toHaveBeenCalledTimes(1);
  });
});

const topProps={projects:[{id:'local_fixture',name:'Copper / Notes',displayName:'Copper / Notes'}],activeProjectId:'local_fixture',onSelectProject:vi.fn(),onNewProject:vi.fn(),onCloseProject:vi.fn(),onOpenTavern:vi.fn()};
it('preserves the explicit display name of a local project', () => {
  render(<TopBar {...topProps} />);
  expect(screen.getByTestId('tab-local_fixture').querySelector('.tab-name')).toHaveTextContent(/^Copper \/ Notes$/);
});

describe('project tab strip', () => {
  const WIDTH = 600, TAB = 150, FIRST = 12;
  let scrollTo: ReturnType<typeof vi.fn>;
  let resized: () => void;
  const strip = () => document.querySelector<HTMLElement>('.tb-tabs')!;
  const at = (tab: Element) => FIRST + [...tab.parentElement!.children].indexOf(tab) * (TAB + 1);
  /** A 600px strip of 150px tabs; happy-dom has no layout. */
  const layOut = () => {
    scrollTo = vi.fn();
    vi.spyOn(Element.prototype, 'scrollTo').mockImplementation(scrollTo as Element['scrollTo']);
    vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(WIDTH);
    vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(TAB);
    vi.spyOn(HTMLElement.prototype, 'offsetLeft', 'get').mockImplementation(function (this: HTMLElement) { return this.classList.contains('tab') ? at(this) : 0; });
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
      const left = this.classList.contains('tab') ? at(this) - strip().scrollLeft : 0;
      return new DOMRect(left, 0, this.classList.contains('tab') ? TAB : WIDTH, 48);
    });
    vi.stubGlobal('ResizeObserver', class { constructor(callback: () => void) { resized = callback; } observe() {} disconnect() {} });
  };
  const scrolled = (left: number) => act(() => { strip().scrollLeft = left; fireEvent.scroll(strip()); });
  const bar = (count: number, active: number | null, more: Partial<Parameters<typeof TopBar>[0]> = {}) => <TopBar
    projects={Array.from({ length: count }, (_, i) => ({ id: `p${i}`, name: `p${i}` }))} activeProjectId={active == null ? null : `p${active}`}
    onSelectProject={vi.fn()} onNewProject={vi.fn()} onCloseProject={vi.fn()} onOpenTavern={vi.fn()} {...more} />;
  const edges = () => ['left', 'right'].filter(side => document.querySelector(`.tb-edge.${side}`)!.classList.contains('on'));
  afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

  it('shows an edge exactly on the sides that hide a tab', () => {
    layOut();
    render(bar(8, 0)); // 8 tabs end at 1219
    expect(edges()).toEqual(['right']);
    scrolled(300);
    expect(edges()).toEqual(['left', 'right']);
    scrolled(1219 - WIDTH);
    expect(edges()).toEqual(['left']);
    expect(screen.getByRole('button', { name: 'More projects' })).toBeDisabled();
    scrolled(FIRST); // only the space before the first tab is out
    expect(edges()).toEqual(['right']);
  });

  it('follows tabs that come and go, and the window', () => {
    layOut();
    const { rerender } = render(bar(3, 0));
    expect(edges()).toEqual([]);
    rerender(bar(5, 0));
    expect(edges()).toEqual(['right']);
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue(new DOMRect(0, 0, 100, 48));
    act(() => resized());
    expect(edges()).toEqual([]);
    rerender(bar(0, null));
    expect(screen.getByRole('button', { name: 'Open project' })).toBeInTheDocument();
  });

  it('brings the active tab into view with its move buttons clear of the edges', () => {
    layOut();
    const { rerender } = render(bar(8, 7));
    expect(scrollTo).toHaveBeenLastCalledWith({ left: at(screen.getByTestId('tab-p7')) + TAB + 76 - WIDTH, behavior: 'auto' }); // no gliding when it first appears
    strip().scrollLeft = 619;
    rerender(bar(8, 2));
    expect(scrollTo).toHaveBeenLastCalledWith({ left: at(screen.getByTestId('tab-p2')) - 76, behavior: 'smooth' });
    strip().scrollLeft = 238;
    scrollTo.mockClear();
    rerender(bar(8, 3)); // already clear of both edges
    expect(scrollTo).not.toHaveBeenCalled();
    act(() => resized()); // the window must not take a place the user scrolled to
    expect(scrollTo).not.toHaveBeenCalled();
    rerender(bar(8, 3, { projects: ['p0', 'p1', 'p2', 'p4', 'p5', 'p6', 'p3', 'p7'].map(id => ({ id, name: id })) }));
    expect(scrollTo).toHaveBeenLastCalledWith({ left: FIRST + 6 * 151 + TAB + 76 - WIDTH, behavior: 'smooth' });
  });

  it('does not glide when motion is reduced', () => {
    layOut();
    vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: true } as MediaQueryList);
    const { rerender } = render(bar(8, 0));
    rerender(bar(8, 7));
    expect(scrollTo).toHaveBeenLastCalledWith(expect.objectContaining({ behavior: 'auto' }));
  });

  it('pages with the arrows, by what the edges leave visible', () => {
    layOut();
    render(bar(8, 0));
    scrolled(300);
    fireEvent.click(screen.getByRole('button', { name: 'More projects' }));
    expect(scrollTo).toHaveBeenLastCalledWith({ left: 300 + WIDTH - 128, behavior: 'smooth' });
    fireEvent.click(screen.getByRole('button', { name: 'Earlier projects' }));
    expect(scrollTo).toHaveBeenLastCalledWith({ left: 300 - (WIDTH - 128), behavior: 'smooth' });
  });

  it('keeps + outside the strip, and nothing below it or masking it', async () => {
    render(bar(8, 0));
    expect(strip().querySelector('.tab-plus')).toBeNull();
    const { readFile } = await import('node:fs/promises');
    const css = await readFile('src/styles/canvas.css', 'utf8');
    const rule = (selector: string) => css.slice(css.indexOf(`\n${selector} {`)).split('}')[0]!;
    expect(rule('.tb-tabs')).toContain('overflow-y: hidden');
    expect(rule('.tb-tabs')).not.toContain('mask');
    expect(rule('.tab')).not.toContain('margin-bottom');
    expect(rule('.tab.active::after')).toContain('bottom: 0;');
  });
});
