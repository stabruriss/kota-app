import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import * as ipc from '../src/pty-client';
import * as puppeteer from '../src/lib/puppeteer-dance';
import type { DanceController } from '../src/lib/dance-settings';
import { RightColumn } from '../src/chrome/RightColumn';

// RightColumn renders this label once per render of its own, so it counts them.
const columnRenders = vi.hoisted(() => ({ count: 0 }));
vi.mock('../src/chrome/KotaVersionLabel', () => ({ KotaVersionLabel: () => { columnRenders.count += 1; return null; } }));

const workspace: ipc.WorkspaceProject = {
  projectId: 'proj', repoFullName: 'mock/proj', remoteUrl: 'https://github.com/mock/proj.git', githubHtmlUrl: 'https://github.com/mock/proj',
  defaultBranch: 'main', baseRef: 'origin/main', localRoot: '/tmp/kota/proj', localRootBytes: 0, sourceDir: '/tmp/kota/proj/source', sourceDirBytes: 0,
  sharedDir: '/tmp/kota/proj/project-memory', rulesDir: '/tmp/kota/proj/rules', agents: [],
};
const bartender: ipc.BartenderStatus = {
  projectId: 'proj', sourceDir: workspace.sourceDir, defaultBranch: 'main', roomChangeCount: 3, sourceChangeCount: 0, githubChangeCount: 2,
  githubBehindCount: 0, githubNeedsInitialPush: false, githubPushBranch: null, githubInitialPushCommitCount: 0, dirtyAgents: [],
  checkedAt: '2026-11-24T00:00:00Z', state: 'roomDiff', message: '3 changed files waiting.',
};
const danceSettings = { enabled: false, strategy: 'rule', threshold: 0.5, sentinel: null, conditions: {} } as unknown as puppeteer.DanceSettings;
const danceSnapshot = { settings: danceSettings, sourceRevision: 'r1', currentSentinel: null } as unknown as puppeteer.DanceSettingsSnapshot;
const dance = {
  projectRoot: '/tmp/kota/proj', settings: danceSettings, sourceRevision: 'r1', currentSentinel: null, loading: false, busy: false, error: null,
  currentError: () => null, save: vi.fn(), refresh: vi.fn(async () => danceSnapshot),
} as unknown as DanceController;

const STORE = 'kota-v2.right-column.card-piles';
const slot = (id: string) => document.querySelector<HTMLElement>(`.rail-slot[data-card="${id}"]`)!;
const face = (id: string) => slot(id).querySelector<HTMLElement>('.rail-raise')!.firstElementChild as HTMLElement;
const tops = () => [...document.querySelectorAll<HTMLElement>('.rail-slot.is-top')].sort((a, b) => Number(a.style.order) - Number(b.style.order));
/** What the column shows, pile by pile, top card first. */
const shown = () => {
  const piles: string[][] = [];
  for (const el of [...document.querySelectorAll<HTMLElement>('.rail-slot')].sort((a, b) => Number(a.style.order) - Number(b.style.order))) {
    if (el.classList.contains('is-top')) piles.push([]);
    piles.at(-1)!.push(el.dataset.card!);
  }
  return piles.map((pile) => pile.join(' '));
};
const save = (piles: string[][]) => window.localStorage.setItem(STORE, JSON.stringify(piles));
const saved = () => (JSON.parse(window.localStorage.getItem(STORE) ?? '[]') as string[][]).map((pile) => pile.join(' '));

function room(props: Partial<Parameters<typeof RightColumn>[0]> = {}) {
  const element = (extra: typeof props = {}) => (
    <RightColumn sceneKey="conversation" workspace={workspace} workingAgents={new Set()} onOpenHotMem={vi.fn()} danceController={dance}
      footerSlot={<button type="button">Archive</button>} {...props} {...extra} />
  );
  const view = render(element());
  return { ...view, update: (extra: typeof props) => view.rerender(element(extra)) };
}
async function settled(props?: Parameters<typeof room>[0]) {
  const view = room(props);
  await screen.findByRole('button', { name: 'Sync 3 changed files in room' });
  await act(async () => {});
  return view;
}

/** The column as it lays out: 112 px faces, 8 px gaps, 10 px per visible edge. Transforms are ignored. */
function layout() {
  const box = (left: number, top: number, width: number, height: number) => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON: () => ({}) });
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const own = this.closest<HTMLElement>('.rail-slot');
    if (own) {
      let top = 60;
      for (const el of tops()) {
        const depth = Number(el.style.getPropertyValue('--depth')) * 10;
        if (Math.floor(Number(el.style.order) / 10) === Math.floor(Number(own.style.order) / 10)) return box(1012, top, 275, 112 + (this === own ? depth : 0));
        top += 112 + depth + 8;
      }
    }
    const total = tops().reduce((sum, el) => sum + 120 + Number(el.style.getPropertyValue('--depth')) * 10, 0);
    if (this.classList.contains('sr-footer-slot')) return box(1012, 60 + total, 275, 30);
    if (this.classList.contains('system-agent-stack')) return box(1012, 60, 275, total + 30);
    return box(1000, 0, 300, 900);
  });
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(112);
}

const press = (el: Element, y: number) => fireEvent.pointerDown(el, { pointerId: 1, button: 0, buttons: 1, clientX: 1100, clientY: y });
/** One move of the hand and the frame that follows it. */
const moveTo = (y: number, x = 1100) => {
  fireEvent.pointerMove(window, { pointerId: 1, buttons: 1, clientX: x, clientY: y });
  act(() => { vi.advanceTimersByTime(20); });
};
const release = (y: number, x = 1100) => fireEvent.pointerUp(window, { pointerId: 1, buttons: 0, clientX: x, clientY: y });
/** Presses the top edge of a card and carries it `dy` px, as a hand does: in several moves. */
function carry(id: string, dy: number) {
  const y = face(id).getBoundingClientRect().top + 10;
  press(face(id), y);
  for (let i = 1; i <= 8; i += 1) moveTo(y + (dy * i) / 8);
  return y + dy;
}
function dragBy(id: string, dy: number) {
  const el = face(id);
  release(carry(id, dy));
  // The browser follows the release with a click on what it was pressed on.
  fireEvent.click(el);
  act(() => { vi.advanceTimersByTime(1); });
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  window.localStorage.clear();
  columnRenders.count = 0;
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.animate = vi.fn(() => ({ cancel() {}, finished: Promise.resolve() }) as unknown as Animation);
  vi.spyOn(ipc, 'githubAccountsRead').mockResolvedValue({ defaultLogin: 'mock', accounts: [{ login: 'mock', default: true, state: 'ok', source: 'gh-cli' }] });
  vi.spyOn(ipc, 'projectConnectionStatus').mockResolvedValue({ kind: 'github', accountLogin: 'mock', accountState: 'ok' });
  vi.spyOn(ipc, 'bartenderStatus').mockResolvedValue(bartender);
  layout();
});
afterEach(() => {
  vi.advanceTimersByTime(10);   // a drag blocks the click that follows its release until the next task
  vi.useRealTimers();
  vi.restoreAllMocks();
  // Assigned, not spied on, so nothing else takes them back: files sharing a worker would inherit them.
  Reflect.deleteProperty(Element.prototype, 'setPointerCapture');
  Reflect.deleteProperty(Element.prototype, 'animate');
});

describe('cards stay clickable', () => {
  it.each([
    ['violet', 'Violet summary history', 'Open Violet summary history'],
    ['ember', 'Ember scheduled prompts', 'Open Ember schedules and drafts'],
    ['lm', 'Laughing Man settings', 'Laughing Man Telegram bridge'],
  ])('%s opens on a click with up to 4 px of slip; a drag opens nothing and the next click opens it', async (id, dialog, card) => {
    await settled();
    const el = screen.getByRole('button', { name: card });
    const y = el.getBoundingClientRect().top + 10;
    press(el, y); moveTo(y + 30); release(y + 30); fireEvent.click(el);
    act(() => { vi.advanceTimersByTime(1); });
    expect(screen.queryByRole('dialog', { name: dialog })).not.toBeInTheDocument();
    press(el, y); moveTo(y + 4); release(y + 4); fireEvent.click(el);
    expect(screen.getByRole('dialog', { name: dialog })).toBeInTheDocument();
    expect(shown()).toEqual(['violet', 'bartender', 'ember', 'dance', 'lm', 'bbs']);
    expect(face(id)).toBe(el);
  });

  it('opens Violet, Ember and Laughing Man from the keyboard', async () => {
    await settled();
    for (const name of ['Open Violet summary history', 'Open Ember schedules and drafts', 'Laughing Man Telegram bridge']) {
      fireEvent.keyDown(screen.getByRole('button', { name }), { key: 'Enter' });
    }
    expect(screen.getByRole('dialog', { name: 'Violet summary history' })).toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Ember scheduled prompts' })).toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Laughing Man settings' })).toBeInTheDocument();
  });

  it('a press on a control never becomes a drag, and its click still counts once', async () => {
    const sync = vi.spyOn(ipc, 'bartenderSyncLocal').mockResolvedValue({ ok: true, message: 'Done.', status: bartender, conflicts: [], snapshotCount: 0, publishedCommitCount: 0, publishedAgents: [] });
    await settled();
    const auto = screen.getByRole('switch', { name: 'Auto sync' });
    const controls = [auto, screen.getByRole('button', { name: 'Sync 3 changed files in room' }), screen.getByRole('button', { name: 'Set up Telegram Bot' }),
      screen.getByRole('button', { name: 'Turn Dance on' }), screen.getByRole('button', { name: 'Open' }), screen.getByRole('button', { name: 'Post' })];
    for (const control of controls) {
      press(control, 100); moveTo(230); release(230);
      expect(document.querySelector('[data-dragging]')).toBeNull();
    }
    expect(shown()).toEqual(['violet', 'bartender', 'ember', 'dance', 'lm', 'bbs']);
    fireEvent.click(auto);
    expect(auto).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(controls[1]);
    await waitFor(() => expect(sync).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: 'Open' }));
    expect(screen.getByRole('dialog', { name: 'Bulletin Board' })).toBeInTheDocument();
  });

  it('shows two Bartender actions in one row with short labels and full names', async () => {
    await settled();
    expect(screen.getByRole('button', { name: 'Sync 3 changed files in room' })).toHaveTextContent(/^Sync 3$/);
    expect(await screen.findByRole('button', { name: 'Push 2 changes to GitHub' })).toHaveTextContent(/^Push 2$/);
  });

  it('leaves dialogs opened by a card alone', async () => {
    await settled();
    fireEvent.click(screen.getByRole('button', { name: /^Dance settings/ }));
    const dialog = await screen.findByRole('dialog');
    const inside = within(dialog).getByRole('button', { name: 'Context based' }).parentElement!;
    press(inside, 100); moveTo(300); release(300);
    expect(document.querySelector('[data-dragging]')).toBeNull();
    expect(fireEvent.dragStart(inside)).toBe(true);
    // On a card the browser's own picture drag is refused, so it cannot take the mouse.
    const avatar = document.querySelector<HTMLImageElement>('.violet-card img')!;
    expect(avatar).toHaveAttribute('draggable', 'false');
    expect(fireEvent.dragStart(avatar)).toBe(false);
  });
});

describe('dragging', () => {
  it('stacks, reorders and moves a whole pile, saves each result, and keeps the same nodes', async () => {
    await settled();
    const nodes = ['violet', 'bartender', 'ember', 'dance', 'lm', 'bbs'].map((id) => [face(id), face(id).parentElement, slot(id).parentElement]);
    dragBy('violet', 70);
    expect(shown()).toEqual(['violet bartender', 'ember', 'dance', 'lm', 'bbs']);
    expect(saved()).toEqual(shown());
    dragBy('violet', 170);
    expect(shown()).toEqual(['ember', 'violet bartender', 'dance', 'lm', 'bbs']);
    dragBy('bbs', -160);
    expect(shown()).toEqual(['ember', 'violet bartender', 'dance', 'bbs', 'lm']);
    expect(saved()).toEqual(shown());
    expect(['violet', 'bartender', 'ember', 'dance', 'lm', 'bbs'].map((id) => [face(id), face(id).parentElement, slot(id).parentElement])).toEqual(nodes);
  });

  it('never renders the column itself, and shows the outcome before release', async () => {
    await settled();
    columnRenders.count = 0;
    const cue = document.querySelector('.rail-drop-cue')!;
    expect(cue).toHaveAttribute('hidden');
    const y = carry('violet', 70);
    expect(cue).toHaveTextContent('Release to stack on Bartender');
    expect(slot('bartender')).toHaveAttribute('data-target');
    moveTo(y + 100);
    moveTo(y + 100);
    expect(cue).toHaveTextContent('Release to place below Bartender');
    expect(shown()[0]).toBe('bartender');
    release(y + 100);
    expect(shown()).toEqual(['bartender', 'violet', 'ember', 'dance', 'lm', 'bbs']);
    expect(cue).toHaveAttribute('hidden');
    expect(columnRenders.count).toBe(0);
  });

  it.each([
    ['Escape', () => fireEvent.keyDown(window, { key: 'Escape' })],
    ['a lost pointer', () => fireEvent.pointerCancel(window, { pointerId: 1 })],
    ['the window losing focus', () => fireEvent.blur(window)],
    ['a release far outside the column', () => release(400, 300)],
  ])('%s puts the pile back and saves nothing', async (_name, stop) => {
    await settled();
    carry('violet', 200);
    expect(shown()[0]).toBe('bartender');
    stop();
    expect(shown()).toEqual(['violet', 'bartender', 'ember', 'dance', 'lm', 'bbs']);
    expect(window.localStorage.getItem(STORE)).toBeNull();
    expect(document.querySelector('[data-dragging], [data-target], [data-carrying]')).toBeNull();
    expect(document.body.style.cursor).toBe('');
  });

  it('swallows the click that follows an Escape', async () => {
    await settled();
    const y = carry('violet', 20);
    fireEvent.keyDown(window, { key: 'Escape' });
    release(y);
    fireEvent.click(face('violet'));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('listens to the window only while a press lasts', async () => {
    await settled();
    const added = vi.spyOn(window, 'addEventListener'), removed = vi.spyOn(window, 'removeEventListener');
    const names = (spy: typeof added | typeof removed) => spy.mock.calls.map(([name]) => name as string).filter((name) => /^(pointer|blur|keydown)/.test(name)).sort();
    fireEvent.pointerOver(face('violet'));
    expect(names(added)).toEqual([]);
    press(face('violet'), 70);
    expect(names(added)).toEqual(['blur', 'keydown', 'pointercancel', 'pointermove', 'pointerup']);
    release(70);
    expect(names(removed)).toEqual(names(added));
  });

  it('is not disturbed by focus leaving the composer', async () => {
    await settled();
    const field = document.body.appendChild(document.createElement('textarea'));
    field.focus();
    press(face('violet'), 70);
    field.blur();
    moveTo(100); moveTo(140);
    expect(slot('violet')).toHaveAttribute('data-dragging');
    release(140);
    field.remove();
  });

  it('drops a press whose release never arrived', async () => {
    await settled();
    press(face('violet'), 70);
    fireEvent.pointerMove(window, { pointerId: 1, buttons: 0, clientX: 1100, clientY: 400 });
    expect(document.querySelector('[data-dragging]')).toBeNull();
  });
});

describe('piles', () => {
  it('shows its two knobs only on the top card and hides buried cards from the keyboard', async () => {
    save([['ember', 'lm', 'bbs'], ['violet'], ['bartender'], ['dance']]);
    await settled();
    expect(screen.getAllByRole('button', { name: 'Flip stack' })).toHaveLength(1);
    expect(within(slot('ember')).getByRole('button', { name: 'Deal top card below stack' })).toBeInTheDocument();
    expect(slot('lm')).toHaveAttribute('inert');
    expect(slot('violet')).not.toHaveAttribute('inert');
    expect(within(slot('ember')).getByRole('tooltip').textContent).toBe('Next: Laughing Man · 3 cards');
  });

  it('flips at one spot as often as it is clicked', async () => {
    save([['ember', 'lm', 'bbs'], ['violet'], ['bartender'], ['dance']]);
    await settled();
    fireEvent.pointerOver(face('ember'));
    for (const next of ['lm bbs ember', 'bbs ember lm', 'ember lm bbs']) {
      fireEvent.click(screen.getByRole('button', { name: 'Flip stack' }), { detail: 1 });
      expect(shown()[0]).toBe(next);
      expect(slot(next.split(' ')[0])).toHaveAttribute('data-hot');
    }
    expect(saved()[0]).toBe('ember lm bbs');
  });

  it('deals below the pile; a stray second click opens nothing, a button on the card still works, and later clicks open it', async () => {
    save([['ember', 'violet'], ['bartender'], ['dance'], ['lm'], ['bbs']]);
    await settled();
    fireEvent.click(screen.getByRole('button', { name: 'Deal top card below stack' }), { detail: 1 });
    expect(shown()).toEqual(['violet', 'ember', 'bartender', 'dance', 'lm', 'bbs']);
    expect(saved()).toEqual(shown());
    expect(screen.queryByRole('button', { name: 'Flip stack' })).not.toBeInTheDocument();
    fireEvent.click(face('violet'));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('switch', { name: 'Auto sync' }));
    expect(screen.getByRole('switch', { name: 'Auto sync' })).toHaveAttribute('aria-checked', 'true');
    act(() => { vi.advanceTimersByTime(450); });
    fireEvent.click(face('violet'));
    expect(screen.getByRole('dialog', { name: 'Violet summary history' })).toBeInTheDocument();
  });

  it('keeps keyboard focus with the pile, and a held key does not open the card it lands on', async () => {
    save([['ember', 'violet', 'bbs'], ['bartender'], ['dance'], ['lm']]);
    await settled();
    fireEvent.click(screen.getByRole('button', { name: 'Flip stack' }));
    expect(document.activeElement).toBe(within(slot('violet')).getByRole('button', { name: 'Flip stack' }));
    fireEvent.click(screen.getByRole('button', { name: 'Deal top card below stack' }));
    expect(document.activeElement).toBe(within(slot('bbs')).getByRole('button', { name: 'Deal top card below stack' }));
    fireEvent.click(screen.getByRole('button', { name: 'Deal top card below stack' }));
    expect(document.activeElement).toBe(face('ember'));
    fireEvent.keyDown(face('ember'), { key: 'Enter', repeat: true });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    fireEvent.keyDown(face('ember'), { key: 'Enter' });
    expect(screen.getByRole('dialog', { name: 'Ember scheduled prompts' })).toBeInTheDocument();
  });

  it('lights a lamp on a buried card from the reminders the card already has', async () => {
    vi.spyOn(ipc, 'lmStatus').mockResolvedValue({ configured: true, enabled: true, running: false, botUsername: 'bot', ownerUserId: 1, selected: null, latest: null, lastError: 'poll failed', standby: null } as unknown as ipc.LmStatus);
    save([['ember', 'lm'], ['violet', 'bartender'], ['dance'], ['bbs']]);
    await settled();
    await waitFor(() => expect(slot('lm').querySelector('.rail-lamp')).toHaveClass('fault'));
    expect(slot('bartender').querySelector('.rail-lamp')).toHaveClass('note');
    expect(slot('violet').querySelector('.rail-lamp')).toBeNull();
    expect(within(slot('ember')).getByRole('tooltip').textContent).toBe('Next: Laughing Man · 2 cards\nLaughing Man · Connection error');
  });

  it('keeps the place of a card that is not rendered', async () => {
    save([['violet'], ['ember', 'dance', 'lm'], ['bartender'], ['bbs']]);
    const view = await settled({ danceController: undefined });
    expect(shown()).toEqual(['violet', 'ember lm', 'bartender', 'bbs']);
    dragBy('violet', 180);
    expect(shown()).toEqual(['ember lm', 'violet', 'bartender', 'bbs']);
    view.update({ danceController: dance });
    expect(shown()).toEqual(['ember dance lm', 'violet', 'bartender', 'bbs']);
    view.update({ danceController: undefined });
    expect(shown()).toEqual(['ember lm', 'violet', 'bartender', 'bbs']);
  });

  it('lets go when a card disappears mid-drag', async () => {
    const view = await settled();
    carry('violet', 200);
    expect(shown()[0]).toBe('bartender');
    view.update({ danceController: undefined });
    expect(shown()).toEqual(['violet', 'bartender', 'ember', 'lm', 'bbs']);
    expect(document.querySelector('[data-dragging]')).toBeNull();
    release(300);
  });
});

describe('the two cards that grow', () => {
  it('Violet grows while its error is open and keeps it open through a flip', async () => {
    vi.spyOn(ipc, 'readVioletSummary').mockResolvedValue({ latest: null, history: [], outstanding: { messageCount: 0 }, logPath: '', promptPath: '', updatedAt: '', error: 'codex summary CLI exited: boom' });
    save([['violet', 'ember'], ['bartender'], ['dance'], ['lm'], ['bbs']]);
    await settled({ projectRoot: workspace.localRoot });
    const line = await screen.findByRole('button', { name: /^Show CLI error/ });
    press(line, 100); moveTo(300); release(300);
    expect(document.querySelector('[data-dragging]')).toBeNull();
    expect(slot('violet')).not.toHaveAttribute('data-expanded');
    fireEvent.click(line);
    expect(slot('violet')).toHaveAttribute('data-expanded');
    const text = document.querySelector('.violet-summary-fault-text');
    fireEvent.click(screen.getByRole('button', { name: 'Flip stack' }), { detail: 1 });
    fireEvent.click(screen.getByRole('button', { name: 'Flip stack' }), { detail: 1 });
    expect(document.querySelector('.violet-summary-fault-text')).toBe(text);
    expect(slot('violet')).toHaveAttribute('data-expanded');
    fireEvent.click(screen.getByRole('button', { name: /^Hide CLI error/ }));
    expect(slot('violet')).not.toHaveAttribute('data-expanded');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('Bartender grows while a pull conflict waits for an agent', async () => {
    const behind = { ...bartender, roomChangeCount: 0, githubChangeCount: 0, githubBehindCount: 2, state: 'githubBehind' } as ipc.BartenderStatus;
    vi.spyOn(ipc, 'bartenderStatus').mockResolvedValue(behind);
    vi.spyOn(ipc, 'bartenderPullFromGithub').mockResolvedValue({ ok: false, needsHumanPick: true, message: 'Conflict.', conflict: { message: '', sourceHead: '', upstream: '', upstreamHead: '', defaultBranch: 'main' }, status: behind, pulledCommitCount: 0 });
    const route = vi.spyOn(ipc, 'bartenderRoutePullConflict').mockResolvedValue({ ok: true, message: 'Routed.', status: behind } as never);
    room({ roomAgents: ['alice' as never], agentMeta: { alice: { name: 'Alice' } } as never, workspace: { ...workspace, agents: [{ agentId: 'alice', cli: 'codex', cwd: '', projectRoot: '', worktreeRoot: '', sharedDir: '', rulesDir: '', adapterPath: '', args: [], projectId: 'proj', projectBaseRef: '' }] } });
    fireEvent.click(await screen.findByRole('button', { name: 'Pull 2 changes from GitHub' }));
    const pick = await screen.findByRole('button', { name: 'Alice' });
    expect(slot('bartender')).toHaveAttribute('data-expanded');
    press(screen.getByText('GitHub pull conflicted with the local version.'), 100); moveTo(300); release(300);
    expect(document.querySelector('[data-dragging]')).toBeNull();
    fireEvent.click(pick);
    await waitFor(() => expect(slot('bartender')).not.toHaveAttribute('data-expanded'));
    expect(route).toHaveBeenCalledTimes(1);
  });
});
