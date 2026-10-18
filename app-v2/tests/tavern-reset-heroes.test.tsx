import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { MockInstance } from 'vitest';
import {
  TavernModal, loadTavernHeroIncarnationProfile, loadTavernWorkingHeroes,
  TAVERN_PROFILE_CHANGED_EVENT,
} from '../src/chrome/TavernModal';
import * as pty from '../src/pty-client';

const ids = ['hero-cc', 'hero-dex', 'hero-gem', 'hero-op', 'hero-pi', 'hero-kimi'];
let disk: pty.TavernHeroProfileDraft[];
let storageDescriptor: PropertyDescriptor | undefined;
let load: MockInstance<typeof pty.loadTavernHeroProfiles>;
let save: MockInstance<typeof pty.saveTavernHeroProfiles>;

beforeEach(() => {
  const values = new Map<string, string>();
  storageDescriptor = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  disk = [];
  load = vi.spyOn(pty, 'loadTavernHeroProfiles').mockImplementation(async () => structuredClone(disk));
  save = vi.spyOn(pty, 'saveTavernHeroProfiles').mockImplementation(async (next) => { disk = structuredClone(next); });
  vi.spyOn(pty, 'hasTauriRuntime').mockReturnValue(true);
  vi.spyOn(pty, 'deleteTavernHero');
  vi.spyOn(pty, 'spawnAgentPty');
});
afterEach(() => {
  cleanup(); vi.restoreAllMocks();
  if (storageDescriptor) Object.defineProperty(window, 'localStorage', storageDescriptor);
});

function fresh(heroId: string): pty.TavernHeroProfileDraft {
  const p = loadTavernHeroIncarnationProfile(heroId)!;
  return { heroId, name: p.name, nameFields: p.nameFields, provider: p.provider, model: p.model,
    effort: p.effort ?? null, avatarId: p.avatarId, skills: p.skills, ghost: p.ghost, shell: p.shell,
    kind: p.kind, archived: false, dismissed: false };
}
function custom(heroId: string, name: string, patch: Partial<pty.TavernHeroProfileDraft> = {}): pty.TavernHeroProfileDraft {
  return { ...fresh('hero-dex'), heroId, name, nameFields: { titleId: null, given: name, middle: '', surname: '' },
    ghost: 'Personal Ghost', shell: 'provider: codex\nmodel: custom-model\nargs: []', model: 'custom-model',
    skills: ['personal-skill'], kind: 'invited', ...patch };
}
async function open() {
  const view = render(<TavernModal open onClose={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Reset All Heroes' })).toBeEnabled());
  return view;
}
async function confirmReset() {
  await userEvent.click(screen.getByRole('button', { name: 'Reset All Heroes' }));
  const dialog = screen.getByRole('dialog', { name: 'Reset All Heroes?' });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Reset All Heroes' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Reset All Heroes?' })).not.toBeInTheDocument());
}
function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => { resolve = done; });
  return { promise, resolve };
}

it('asks for confirmation, with exact copy, and Cancel/Escape never save', async () => {
  disk = [custom('mine', 'Custom')];
  await open();
  const button = screen.getByRole('button', { name: 'Reset All Heroes' });
  expect(button.closest('.tavern-reset-heroes')).toBeInTheDocument();
  expect(button.closest('.tavern-reset-heroes')?.previousElementSibling).toHaveClass('tavern-hero-gathering');
  expect(button.closest('.tavern-reset-heroes')?.nextElementSibling).toHaveClass('tavern-provider-status');
  await userEvent.click(button);
  const dialog = screen.getByRole('dialog', { name: 'Reset All Heroes?' });
  expect(within(dialog).getByText("Reset ALL Hero cards to factory defaults. This CAN'T be undone.")).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Cancel' })).toHaveFocus();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await userEvent.click(button);
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('dialog', { name: 'Reset All Heroes?' })).not.toBeInTheDocument();
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(save).not.toHaveBeenCalled();
  expect(disk[0].name).toBe('Custom');
});

it('resets all six cards to fresh-install fields and argv, keeps added templates in Drifters, and refreshes Recruit', async () => {
  const defaults = ids.map(fresh);
  disk = defaults.map(p => ({ ...p, name: `${p.name} edited`, nameFields: null, avatarId: 'user:changed',
    provider: 'claude', model: 'changed-model', effort: 'max', ghost: 'Changed Ghost', skills: ['changed'],
    shell: 'changed shell', archived: true, dismissed: true }));
  const added = custom('invited', 'Personal');
  const archived = custom('drifter', 'Old', { archived: true });
  const dismissed = custom('dismissed', 'Gone', { dismissed: true });
  disk.push(added, archived, dismissed);
  const changed = vi.fn(); window.addEventListener(TAVERN_PROFILE_CHANGED_EVENT, changed);
  const view = await open();
  await confirmReset();
  expect(save).toHaveBeenCalledTimes(1);
  for (const expected of defaults) {
    expect(disk.find(p => p.heroId === expected.heroId)).toEqual(expected);
    const next = loadTavernHeroIncarnationProfile(expected.heroId)!;
    expect(next.shell).toBe(expected.shell);
    expect(next.args.join(' ')).not.toMatch(/--model|--effort|--thinking|model_reasoning_effort/);
  }
  expect(disk.find(p => p.heroId === added.heroId)).toEqual({ ...added, archived: true });
  expect(disk.find(p => p.heroId === archived.heroId)).toEqual(archived);
  expect(disk.find(p => p.heroId === dismissed.heroId)).toEqual(dismissed);
  expect(loadTavernWorkingHeroes().map(p => p.id)).toEqual(ids);
  expect(document.querySelector('.tavern-drifters')).toHaveTextContent('Personal');
  expect(changed).toHaveBeenCalled();
  expect(pty.deleteTavernHero).not.toHaveBeenCalled();
  expect(pty.spawnAgentPty).not.toHaveBeenCalled();
  view.rerender(<TavernModal open={false} onClose={vi.fn()} />);
  view.rerender(<TavernModal open onClose={vi.fn()} />);
  await waitFor(() => expect(load.mock.calls.length).toBeGreaterThan(3));
  expect(loadTavernWorkingHeroes().map(p => p.id)).toEqual(ids);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(save).toHaveBeenCalledTimes(1);
  window.removeEventListener(TAVERN_PROFILE_CHANGED_EVENT, changed);
});

it('reserves existing names and appends (1) repeatedly, without renaming Drifters on the next reset', async () => {
  disk = [custom('a', 'Dex'), custom('b', 'Dex(1)'), custom('c', 'Dex(1)(1)', { archived: true }),
    custom('d', ' cC ', { archived: true })];
  await open(); await confirmReset();
  const renamed = disk.find(p => p.heroId === 'a')!;
  expect(renamed.name).toBe('Dex(1)(1)(1)');
  expect(renamed.nameFields).toEqual({ titleId: null, given: renamed.name, middle: '', surname: '' });
  expect(disk.find(p => p.heroId === 'b')?.name).toBe('Dex(1)');
  expect(disk.find(p => p.heroId === 'c')?.name).toBe('Dex(1)(1)');
  expect(disk.find(p => p.heroId === 'd')?.name).toBe(' cC (1)');
  const first = structuredClone(disk);
  await confirmReset(); expect(disk).toEqual(first);
});

it('loads the current disk roster on confirmation rather than losing a newly invited template', async () => {
  await open();
  disk.push(custom('invited-late', 'Late arrival'));
  await confirmReset();
  expect(disk.find(p => p.heroId === 'invited-late')).toMatchObject({ name: 'Late arrival', archived: true });
});

it('single-flights confirmation and keeps the old roster until the write succeeds', async () => {
  disk = [custom('mine', 'Personal')];
  const block = deferred();
  save.mockImplementation(async next => { await block.promise; disk = structuredClone(next); });
  await open();
  await userEvent.click(screen.getByRole('button', { name: 'Reset All Heroes' }));
  const dialog = screen.getByRole('dialog', { name: 'Reset All Heroes?' });
  const confirm = within(dialog).getByRole('button', { name: 'Reset All Heroes' });
  fireEvent.click(confirm); fireEvent.click(confirm);
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(within(dialog).getByRole('button', { name: 'Cancel' })).toBeDisabled();
  expect(confirm).toBeDisabled();
  fireEvent.keyDown(document, { key: 'Escape' }); expect(dialog).toBeInTheDocument();
  expect(loadTavernWorkingHeroes().some(p => p.id === 'mine')).toBe(true);
  await act(async () => block.resolve());
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(loadTavernWorkingHeroes().some(p => p.id === 'mine')).toBe(false);
});

it('waits for a prior in-flight autosave so it cannot restore an old card after reset', async () => {
  disk = [custom('mine', 'Personal')];
  const block = deferred();
  save.mockImplementationOnce(async next => { await block.promise; disk = structuredClone(next); });
  await open();
  await userEvent.click(screen.getByRole('button', { name: 'New Hero create template' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  fireEvent.keyDown(document, { key: 'Escape' });
  // The profile overlay handles its own close; use its existing back control.
  const profile = screen.queryByRole('dialog', { name: 'New Hero profile' });
  if (profile) await userEvent.click(within(profile).getByRole('button', { name: 'Back' }));
  await userEvent.click(screen.getByRole('button', { name: 'Reset All Heroes' }));
  const dialog = screen.getByRole('dialog', { name: 'Reset All Heroes?' });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Reset All Heroes' }));
  expect(save).toHaveBeenCalledTimes(1);
  await act(async () => block.resolve());
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(save).toHaveBeenCalledTimes(2);
  expect(disk.filter(p => !ids.includes(p.heroId)).every(p => p.archived)).toBe(true);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(save).toHaveBeenCalledTimes(2);
});

it('flushes a newly added card still waiting for autosave before moving it into Drifters', async () => {
  await open();
  const setTimeout = window.setTimeout.bind(window);
  vi.spyOn(window, 'setTimeout').mockImplementation(((handler: TimerHandler, timeout?: number, ...args: unknown[]) =>
    setTimeout(handler, timeout === 250 ? 10_000 : timeout, ...args)) as typeof window.setTimeout);
  fireEvent.click(screen.getByRole('button', { name: 'New Hero create template' }));
  const profile = screen.getByRole('dialog', { name: 'New Hero profile' });
  fireEvent.click(within(profile).getByRole('button', { name: 'Back' }));
  expect(save).not.toHaveBeenCalled();
  await confirmReset();
  expect(save).toHaveBeenCalledTimes(2);
  const added = disk.filter(p => !ids.includes(p.heroId));
  expect(added).toHaveLength(1);
  expect(added[0]).toMatchObject({ name: 'New Hero', archived: true, dismissed: false });
  expect(save.mock.calls[0][0].find(p => p.heroId === added[0].heroId)?.archived).toBe(false);
  expect(pty.deleteTavernHero).not.toHaveBeenCalled();
});

it('clears an earlier autosave error when the explicit reset really succeeds', async () => {
  disk = [custom('mine', 'Personal')];
  const block = deferred();
  save.mockImplementationOnce(async () => {
    await block.promise;
    throw new Error('fixture earlier save failed');
  });
  await open();
  await userEvent.click(screen.getByRole('button', { name: 'New Hero create template' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  await userEvent.click(within(screen.getByRole('dialog', { name: 'New Hero profile' })).getByRole('button', { name: 'Back' }));
  await userEvent.click(screen.getByRole('button', { name: 'Reset All Heroes' }));
  const dialog = screen.getByRole('dialog', { name: 'Reset All Heroes?' });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Reset All Heroes' }));
  await act(async () => block.resolve());
  await waitFor(() => expect(dialog).not.toBeInTheDocument());
  expect(save).toHaveBeenCalledTimes(2);
  expect(screen.queryByText(/fixture earlier save failed/)).not.toBeInTheDocument();
  expect(disk.find(p => p.heroId === 'mine')?.archived).toBe(true);
});

it('keeps an archived template callable back with the same content and ID', async () => {
  const original = custom('personal-id', 'Personal');
  disk = [original];
  await open(); await confirmReset();
  const drifter = screen.getByText('Personal').closest('.tavern-drifter') as HTMLElement;
  await userEvent.click(within(drifter).getByRole('button', { name: 'Call Back' }));
  await waitFor(() => expect(disk.find(p => p.heroId === 'personal-id')?.archived).toBe(false));
  expect(disk.find(p => p.heroId === 'personal-id')).toMatchObject({
    heroId: original.heroId, name: original.name, nameFields: original.nameFields,
    ghost: original.ghost, shell: original.shell, skills: original.skills,
  });
  expect(loadTavernWorkingHeroes().some(p => p.id === original.heroId)).toBe(true);
  expect(pty.spawnAgentPty).not.toHaveBeenCalled();
});

it('reports a partial write failure and projects actual disk state without retrying or deleting', async () => {
  disk = [custom('mine', 'Personal')];
  save.mockImplementationOnce(async next => {
    disk = [next[0], ...disk];
    throw new Error('fixture disk write failed');
  });
  await open(); await confirmReset();
  expect(await screen.findByText(/Could not reset all Heroes.*fixture disk write failed/)).toBeInTheDocument();
  expect(loadTavernWorkingHeroes().some(p => p.id === 'mine')).toBe(true);
  expect(disk).toHaveLength(2);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(save).toHaveBeenCalledTimes(1);
  expect(pty.deleteTavernHero).not.toHaveBeenCalled();
});

it('does not overwrite with stale UI if read-back also fails; reopening reloads the saved result', async () => {
  const rendered = await open();
  load.mockResolvedValueOnce([]).mockRejectedValueOnce(new Error('fixture read-back failed'));
  const view = screen.getByTestId('tavern-page');
  await confirmReset();
  expect(view).toHaveTextContent(/could not refresh/i);
  expect(save).toHaveBeenCalledTimes(1);
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 300)); });
  expect(save).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Reset All Heroes' })).toBeDisabled();
  rendered.rerender(<TavernModal open={false} onClose={vi.fn()} />);
  rendered.rerender(<TavernModal open onClose={vi.fn()} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Reset All Heroes' })).toBeEnabled());
  expect(disk).toHaveLength(6);
  expect(loadTavernWorkingHeroes().map(p => p.id)).toEqual(ids);
  expect(save).toHaveBeenCalledTimes(1);
});
