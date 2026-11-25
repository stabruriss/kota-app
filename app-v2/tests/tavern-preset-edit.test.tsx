import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TavernModal, loadTavernHeroIncarnationProfile } from '../src/chrome/TavernModal';
import * as pty from '../src/pty-client';

let storageDescriptor: PropertyDescriptor | undefined;
beforeEach(() => {
  const values = new Map<string, string>();
  storageDescriptor = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
});
afterEach(() => {
  cleanup(); vi.restoreAllMocks();
  if (storageDescriptor) Object.defineProperty(window, 'localStorage', storageDescriptor);
});

it.each([
  ['hero-cc', 'claude', 'CC'], ['hero-dex', 'codex', 'Dex'], ['hero-gem', 'antigravity', 'Gem'],
  ['hero-op', 'opencode', 'Op'], ['hero-pi', 'pi', 'Pi'], ['hero-kimi', 'kimi', 'Kimi'],
])('%s stays editable after explicit reset: save, reopen, clear, and incarnate', async (heroId, provider, name) => {
  const factory = loadTavernHeroIncarnationProfile(heroId)!;
  let disk: pty.TavernHeroProfileDraft[] = [{ heroId, name, provider, model: 'default', effort: null,
    skills: ['frontend-design'], ghost: 'Old persona', shell: factory.shell, kind: 'custom' }];
  const load = vi.spyOn(pty, 'loadTavernHeroProfiles').mockImplementation(async () => structuredClone(disk));
  const save = vi.spyOn(pty, 'saveTavernHeroProfiles').mockImplementation(async (profiles) => { disk = structuredClone(profiles); });
  const status = await pty.supportedShellsStatus();
  vi.spyOn(pty, 'supportedShellsStatus').mockResolvedValue(status.map((entry) => ({ ...entry, installed: true,
    // Also cover a cached provider catalogue that doesn't know the CLI's default alias.
    modelOptions: [{ id: 'custom/model', label: 'custom/model', source: 'fixture' }],
  })));
  const spawn = vi.spyOn(pty, 'spawnAgentPty');
  const onClose = vi.fn();
  const view = render(<TavernModal open onClose={onClose} />);
  await waitFor(() => expect(load).toHaveBeenCalled());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Reset All Heroes' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Reset All Heroes' }));
  await userEvent.click(within(screen.getByRole('dialog', { name: 'Reset All Heroes?' })).getByRole('button', { name: 'Reset All Heroes' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Reset All Heroes?' })).not.toBeInTheDocument());
  await userEvent.click(await screen.findByRole('button', { name: new RegExp(`^${name}\\b`) }));
  const dialog = () => screen.getByRole('dialog', { name: `${name} profile` });
  const edit = async () => {
    await userEvent.click(within(dialog()).getByRole('button', { name: 'Edit' }));
    return within(dialog()).getAllByRole('combobox');
  };
  let fields = await edit();
  expect(fields[0]).toHaveValue('default');
  if (fields[1]) expect(fields[1]).toHaveValue(''); // Never show the first effort option as selected.
  await userEvent.click(fields[0]);
  expect(within(dialog()).getByRole('option', { name: /default/ })).toBeInTheDocument();
  await userEvent.clear(fields[0]);
  await userEvent.type(fields[0], 'chosen/custom-model');
  if (fields[1]) await userEvent.type(fields[1], 'high');
  await userEvent.click(within(dialog()).getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(disk.find((p) => p.heroId === heroId)).toMatchObject({
    model: 'chosen/custom-model', effort: fields[1] ? 'high' : null, ghost: factory.ghost, skills: ['frontend-design'],
  }));
  expect(save).toHaveBeenCalled();
  // Reload from the saved disk response, not merely local component state.
  view.rerender(<TavernModal open={false} onClose={onClose} />);
  view.rerender(<TavernModal open onClose={onClose} />);
  await waitFor(() => expect(load.mock.calls.length).toBeGreaterThan(1));
  fields = await edit();
  expect(fields[0]).toHaveValue('chosen/custom-model');
  if (fields[1]) expect(fields[1]).toHaveValue('high');
  await userEvent.clear(fields[0]);
  await userEvent.type(fields[0], 'default');
  if (fields[1]) await userEvent.clear(fields[1]);
  await userEvent.click(within(dialog()).getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(disk.find((p) => p.heroId === heroId)).toMatchObject({ model: 'default', effort: null }));
  view.rerender(<TavernModal open={false} onClose={onClose} />);
  view.rerender(<TavernModal open onClose={onClose} />);
  fields = await edit();
  expect(fields[0]).toHaveValue('default');
  if (fields[1]) expect(fields[1]).toHaveValue('');
  const next = loadTavernHeroIncarnationProfile(heroId)!;
  expect(next.model).toBe('default');
  expect(next.effort).toBeUndefined();
  expect(next.shell).not.toMatch(/effort:|--effort|--model|--thinking|model_reasoning_effort/);
  expect(next.args.join(' ')).not.toMatch(/--model|--effort|--thinking|model_reasoning_effort/);
  expect(spawn).not.toHaveBeenCalled(); // Template edits don't restart any live session.
});
