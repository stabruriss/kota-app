import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ShortcutRecruitModal } from '../src/chrome/ShortcutRecruitModal';
import * as pty from '../src/pty-client';
import * as reincarnation from '../src/lib/reincarnation';
import type { WorkingHero } from '../src/types/agentbar';

const heroes: WorkingHero[] = Array.from({ length: 7 }, (_, i) => ({ id: `hero-${i}`, name: `Hero ${i + 1}`, cli: 'codex', record: '' }));
const project = (id: string, archived = false) => ({ projectId: id, repoFullName: id, localRoot: `/projects/${id}`, archived } as pty.WorkspaceProject);
const identity = (id: string, status = 'active'): pty.ProjectAgentIdentity => ({ agentId: id, displayName: id, status, sourceHeroId: 'hero-source', provider: 'codex' });
const preview = (state: pty.TavernHeroSourceState = 'linked'): reincarnation.RecruitPreview => ({ state, heroName: 'Rowan', sourceAgentName: 'Rowan', sourceProjectName: 'Atlas', marks: 8, episodes: 124 });
const detail = { agentId: 'Rowan', displayName: 'Rowan', status: 'active', cli: 'codex', nameFields: { given: 'Rowan' } } as pty.ProjectAgentDetail;
beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(pty, 'listWorkspaceProjects').mockResolvedValue([project('Current'), project('Atlas'), project('Archived', true)]);
  vi.spyOn(pty, 'listProjectAgentIdentities').mockResolvedValue([identity('Rowan'), identity('Old', 'archived')]);
  vi.spyOn(pty, 'loadProjectAgentDetail').mockResolvedValue(detail);
  vi.spyOn(reincarnation, 'readRecruitPreview').mockResolvedValue(preview());
});
function setup() {
  const onSelect = vi.fn(), onSelectIncarnation = vi.fn(), onDismiss = vi.fn();
  const result = render(<ShortcutRecruitModal seatNumber={3} heroes={heroes} projectRoot="/projects/Current" onSelect={onSelect} onSelectIncarnation={onSelectIncarnation} onDismiss={onDismiss} />);
  return { ...result, onSelect, onSelectIncarnation, onDismiss };
}

it('shows four Tavern choices, folds both groups, and never hotkeys a hidden fifth hero', async () => {
  setup();
  expect(screen.getAllByRole('button', { name: /^Incarnate Hero/ })).toHaveLength(4);
  fireEvent.keyDown(window, { key: '5' });
  expect(reincarnation.readRecruitPreview).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Show all 7' }));
  expect(screen.getAllByRole('button', { name: /^Incarnate Hero/ })).toHaveLength(7);
  await userEvent.click(screen.getByRole('button', { name: /^From Tavern/ }));
  expect(screen.queryByRole('button', { name: /^Incarnate Hero/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: /^From other project/ }));
  expect(screen.queryByRole('button', { name: /^Atlas/ })).not.toBeInTheDocument();
});

it.each(['linked', 'relic', 'dangling'] as const)('confirms %s without displaying counts, and No cancels the whole recruitment', async state => {
  vi.mocked(reincarnation.readRecruitPreview).mockResolvedValue({ ...preview(state), reason: state === 'dangling' ? 'Source incarnation is no longer reachable. Recruiting uses the invitation snapshot without its under-table.' : undefined });
  const { onSelect, onDismiss } = setup();
  await userEvent.click(screen.getByRole('button', { name: 'Incarnate Hero 1' }));
  const confirmation = await screen.findByRole('alertdialog', { name: 'Reincarnate Rowan?' });
  expect(confirmation).toHaveTextContent('The original agent will not be removed.');
  expect(confirmation).toHaveTextContent('The invited agent may bring information from project Atlas.');
  expect(confirmation).not.toHaveTextContent('8 marks'); expect(confirmation).not.toHaveTextContent('124 episodes');
  if (state === 'dangling') expect(confirmation).toHaveTextContent('without its under-table');
  expect(onSelect).not.toHaveBeenCalled();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'No' }));
  await userEvent.click(screen.getByRole('button', { name: 'No' }));
  expect(onDismiss).toHaveBeenCalledOnce(); expect(onSelect).not.toHaveBeenCalled();
});

it('Yes passes the selected hero and fresh preview once; native heroes keep the direct flow', async () => {
  let finish!: () => void;
  const { onSelect } = setup();
  onSelect.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  await userEvent.click(screen.getByRole('button', { name: 'Incarnate Hero 1' }));
  await userEvent.dblClick(await screen.findByRole('button', { name: 'Yes' }));
  expect(onSelect).toHaveBeenCalledExactlyOnceWith(heroes[0], preview());
  expect(screen.getByRole('button', { name: 'No' })).toBeDisabled();
  await act(async () => finish());
});

it('native heroes do not show an extra confirmation', async () => {
  vi.mocked(reincarnation.readRecruitPreview).mockResolvedValue(preview('native'));
  const { onSelect } = setup();
  fireEvent.keyDown(window, { key: '1' });
  await waitFor(() => expect(onSelect).toHaveBeenCalledOnce());
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
});

it('lists only other current projects, loads an expanded roster, and passes the exact source after confirmation', async () => {
  const { onSelect, onSelectIncarnation } = setup();
  const atlas = await screen.findByRole('button', { name: /^Atlas/ });
  expect(screen.queryByRole('button', { name: /^Current/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /^Archived/ })).not.toBeInTheDocument();
  expect(pty.listProjectAgentIdentities).not.toHaveBeenCalled();
  expect(pty.loadProjectAgentDetail).not.toHaveBeenCalled();
  await userEvent.click(atlas);
  await userEvent.click(await screen.findByRole('button', { name: 'Rowan' }));
  expect(screen.queryByRole('button', { name: 'Old' })).not.toBeInTheDocument();
  expect(pty.loadProjectAgentDetail).toHaveBeenCalledWith({ projectRoot: '/projects/Atlas', agentId: 'Rowan' }, { readOnly: true });
  expect(reincarnation.readRecruitPreview).toHaveBeenCalledWith({ sourceProjectRoot: '/projects/Atlas', sourceAgentId: 'Rowan' });
  expect(onSelectIncarnation).not.toHaveBeenCalled();
  await userEvent.click(await screen.findByRole('button', { name: 'Yes' }));
  expect(onSelectIncarnation).toHaveBeenCalledExactlyOnceWith({ projectRoot: '/projects/Atlas', agent: detail });
  expect(onSelect).not.toHaveBeenCalled();
});

it('shows preview failures without recruiting, and a later click retries the read', async () => {
  vi.mocked(reincarnation.readRecruitPreview).mockRejectedValueOnce(new Error('Source is unavailable.'));
  const { onSelect } = setup();
  await userEvent.click(screen.getByRole('button', { name: 'Incarnate Hero 1' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('Source is unavailable.');
  expect(onSelect).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Incarnate Hero 1' }));
  expect(await screen.findByRole('alertdialog')).toBeInTheDocument();
});

it('ignores late preview replies after closing and restores the original focus', async () => {
  const trigger = document.createElement('button'); document.body.append(trigger); trigger.focus();
  let finish!: (value: reincarnation.RecruitPreview) => void;
  vi.mocked(reincarnation.readRecruitPreview).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const { onSelect, onDismiss, unmount } = setup();
  await userEvent.click(screen.getByRole('button', { name: 'Incarnate Hero 1' }));
  fireEvent.keyDown(window, { key: 'Escape' });
  expect(onDismiss).toHaveBeenCalledOnce(); unmount();
  await act(async () => finish(preview('native')));
  expect(onSelect).not.toHaveBeenCalled(); expect(document.activeElement).toBe(trigger); trigger.remove();
});

it('traps confirmation focus and Escape cancels without returning to the underlying picker', async () => {
  const { onSelect, onDismiss } = setup();
  await userEvent.click(screen.getByRole('button', { name: 'Incarnate Hero 1' }));
  const dialog = await screen.findByRole('alertdialog');
  fireEvent.keyDown(within(dialog).getByRole('button', { name: 'No' }), { key: 'Tab', shiftKey: true });
  expect(document.activeElement).toBe(within(dialog).getByRole('button', { name: 'Yes' }));
  fireEvent.keyDown(window, { key: 'Escape' });
  expect(onDismiss).toHaveBeenCalledOnce(); expect(onSelect).not.toHaveBeenCalled();
});
