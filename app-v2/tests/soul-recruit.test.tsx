import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { App } from '../src/App';
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

it('shows four Tavern choices, switches across three From tabs, and never hotkeys a hidden fifth hero', async () => {
  setup();
  expect(screen.getAllByRole('tab').map(tab => tab.textContent)).toEqual(['Tavern', 'This Project', 'Other Project']);
  expect(screen.getByRole('tab', { name: 'Tavern' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.getAllByRole('button', { name: /^Incarnate Hero/ })).toHaveLength(4);
  fireEvent.keyDown(window, { key: '5' });
  expect(reincarnation.readRecruitPreview).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Show all 7' }));
  expect(screen.getAllByRole('button', { name: /^Incarnate Hero/ })).toHaveLength(7);

  const tavernTab = screen.getByRole('tab', { name: 'Tavern' });
  fireEvent.keyDown(tavernTab, { key: 'ArrowRight' });
  expect(screen.getByRole('tab', { name: 'This Project' })).toHaveAttribute('aria-selected', 'true');
  expect(screen.queryByRole('button', { name: /^Incarnate Hero/ })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole('tab', { name: 'This Project' }), { key: 'ArrowRight' });
  expect(screen.getByRole('tab', { name: 'Other Project' })).toHaveAttribute('aria-selected', 'true');
  fireEvent.keyDown(window, { key: '1' });
  expect(reincarnation.readRecruitPreview).not.toHaveBeenCalled();
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

it('recruits from This Project with the same pill visual, 1-9 hotkeys, and same-project confirmation copy', async () => {
  vi.mocked(reincarnation.readRecruitPreview).mockResolvedValue({ ...preview(), sourceProjectName: 'Current' });
  const { onSelect, onSelectIncarnation } = setup();
  expect(pty.listProjectAgentIdentities).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('tab', { name: 'This Project' }));
  const rowan = await screen.findByRole('button', { name: 'Rowan' });
  expect(rowan).toHaveClass('working-hero-pill');
  expect(rowan.querySelector('.working-hero-hotkey')?.textContent).toBe('1');
  expect(screen.queryByRole('button', { name: 'Old' })).not.toBeInTheDocument();
  expect(pty.listProjectAgentIdentities).toHaveBeenCalledExactlyOnceWith('/projects/Current');

  fireEvent.keyDown(window, { key: '2' });
  expect(pty.loadProjectAgentDetail).not.toHaveBeenCalled();
  fireEvent.keyDown(window, { key: '1' });
  await waitFor(() => expect(pty.loadProjectAgentDetail).toHaveBeenCalledWith({ projectRoot: '/projects/Current', agentId: 'Rowan' }, { readOnly: true }));
  expect(reincarnation.readRecruitPreview).toHaveBeenCalledWith({ sourceProjectRoot: '/projects/Current', sourceAgentId: 'Rowan' });
  const confirmation = await screen.findByRole('alertdialog', { name: 'Reincarnate Rowan?' });
  expect(confirmation).toHaveTextContent('from this project');
  expect(confirmation).toHaveTextContent('The original agent will not be removed.');
  expect(confirmation).toHaveTextContent('The new agent starts with the same marks and episodes, then goes its own way.');
  expect(confirmation).not.toHaveTextContent('from project Current');

  await userEvent.click(screen.getByRole('button', { name: 'Yes' }));
  expect(onSelectIncarnation).toHaveBeenCalledExactlyOnceWith({ projectRoot: '/projects/Current', agent: detail });
  expect(onSelect).not.toHaveBeenCalled();
});

it('shows an empty message when This Project has no active agents yet', async () => {
  vi.mocked(pty.listProjectAgentIdentities).mockResolvedValue([identity('Old', 'archived')]);
  setup();
  await userEvent.click(screen.getByRole('tab', { name: 'This Project' }));
  expect(await screen.findByText('No agents in this project yet')).toBeInTheDocument();
});

it('lists only other current projects in Other Project tab, loads an expanded roster, and passes the exact source after confirmation', async () => {
  const { onSelect, onSelectIncarnation } = setup();
  await userEvent.click(screen.getByRole('tab', { name: 'Other Project' }));
  const atlas = await screen.findByRole('button', { name: /^Atlas/ });
  expect(screen.queryByRole('button', { name: /^Current/ })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /^Archived/ })).not.toBeInTheDocument();
  expect(pty.listProjectAgentIdentities).not.toHaveBeenCalled();
  expect(pty.loadProjectAgentDetail).not.toHaveBeenCalled();
  await userEvent.click(atlas);
  const rowan = await screen.findByRole('button', { name: 'Rowan' });
  expect(rowan).toHaveClass('working-hero-pill');
  expect(rowan.querySelector('.working-hero-hotkey')).toBeNull();
  expect(screen.queryByRole('button', { name: 'Old' })).not.toBeInTheDocument();
  await userEvent.click(rowan);
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

it.each([
  { label: 'legacy name without a surname', existing: '紫鹃', incoming: '紫鹃', status: 'active', sameProject: false, expected: '紫鹃 II v. Cedar' },
  { label: 'title and project surname on the existing name', existing: 'His H. 郭子仪 v. Cedar', incoming: '郭子仪', status: 'active', sameProject: false, expected: '郭子仪 II v. Cedar' },
  { label: 'same-project recruitment still starting at II', existing: '紫鹃', incoming: '紫鹃', status: 'active', sameProject: true, expected: '紫鹃 II v. Cedar' },
  { label: 'a different base without a suffix', existing: 'Maple', incoming: 'Rowan', status: 'active', sameProject: false, expected: 'Rowan v. Cedar' },
  { label: 'an archived name remaining reserved', existing: 'Rowan', incoming: 'Rowan', status: 'archived', sameProject: false, expected: 'Rowan II v. Cedar' },
  { label: 'case-insensitive base matching', existing: 'rOwAn v. Cedar', incoming: 'Rowan', status: 'active', sameProject: false, expected: 'Rowan II v. Cedar' },
])('names an incarnation by its occupied base: $label', async ({ existing, incoming, status, sameProject, expected }) => {
  window.localStorage.clear();
  pty.__resetMockSmartPtyForTests();
  const current: pty.WorkspaceProject = {
    ...project('Cedar'), repoFullName: 'mock/Cedar', remoteUrl: 'https://example.invalid/cedar.git',
    githubHtmlUrl: 'https://example.invalid/cedar', defaultBranch: 'main', baseRef: 'HEAD',
    localRootBytes: 0, sourceDir: '/projects/Cedar/source', sourceDirBytes: 0,
    sharedDir: '/projects/Cedar/project-memory', rulesDir: '/projects/Cedar/project-rules', agents: [],
  };
  const localAgent = { ...identity('agent-existing', status), displayName: existing };
  const sourceAgent = { ...identity('agent-source'), displayName: incoming };
  const sourceRoot = sameProject ? current.localRoot : '/projects/Atlas';
  const sourceId = sameProject ? localAgent.agentId : sourceAgent.agentId;
  vi.spyOn(pty, 'hasTauriRuntime').mockReturnValue(true);
  vi.spyOn(pty, 'workspaceStatus').mockResolvedValue({ active: current });
  vi.mocked(pty.listWorkspaceProjects).mockResolvedValue([current, project('Atlas')]);
  vi.spyOn(pty, 'inspectProjectAgentIdentities').mockResolvedValue({ identities: [localAgent], workspaceEntryCount: 1 });
  vi.mocked(pty.listProjectAgentIdentities).mockImplementation(async root => root === '/projects/Atlas' ? [sourceAgent] : [localAgent]);
  vi.mocked(pty.loadProjectAgentDetail).mockImplementation(async ({ agentId, projectRoot }) => {
    const displayName = agentId === localAgent.agentId ? existing : incoming;
    const cwd = `${projectRoot ?? current.localRoot}/.agent-workspaces/${agentId}`;
    return {
      agentId, displayName, sourceHeroId: 'hero-source', sourceHeroName: displayName,
      projectId: 'Cedar', projectName: 'Cedar', cli: 'codex', provider: 'codex', model: 'default',
      skills: [], args: [], ghost: '', adapterPath: `${cwd}/AGENTS.md`, shellPath: `${cwd}/SHELL.yaml`,
      agentYamlPath: `${cwd}/agent.yaml`, status: agentId === localAgent.agentId ? status : 'active',
      inviteEligibility: { eligible: true, proposedHeroId: 'hero-source', proposedDisplayName: displayName },
      record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, forkable: false, dirty: false, dirtySummary: '',
    };
  });
  vi.mocked(reincarnation.readRecruitPreview).mockResolvedValue({ ...preview(), heroName: incoming, sourceAgentName: incoming });
  const recruit = vi.spyOn(reincarnation, 'recruitFromIncarnation').mockImplementation(async ({ agentId }) => {
    const cwd = `${current.localRoot}/.agent-workspaces/${agentId}`;
    return {
      request: { agentId, cli: 'codex', cwd, projectRoot: current.localRoot, args: [] },
      projectRoot: current.localRoot, adapterPath: `${cwd}/AGENTS.md`, shellPath: `${cwd}/SHELL.yaml`,
      matchedSkills: [], missingSkills: [],
    };
  });
  vi.spyOn(pty, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
  vi.spyOn(pty, 'saveProjectAgentLayoutFile').mockResolvedValue();
  vi.spyOn(pty, 'listAgentPtys').mockResolvedValue([]);
  const { container } = render(<App />);
  await waitFor(() => expect(pty.inspectProjectAgentIdentities).toHaveBeenCalledWith(current.localRoot));
  await waitFor(() => expect(container.querySelector('.stage')).not.toHaveAttribute('aria-busy'));
  await userEvent.click(screen.getByTestId('ribbon-add'));
  await userEvent.click(screen.getByRole('tab', { name: sameProject ? 'This Project' : 'Other Project' }));
  if (!sameProject) await userEvent.click(await screen.findByRole('button', { name: /^Atlas/ }));
  await userEvent.click(await screen.findByRole('button', { name: incoming }));
  await userEvent.click(await screen.findByRole('button', { name: 'Yes' }));
  await waitFor(() => expect(recruit).toHaveBeenCalledOnce());
  expect(recruit.mock.calls[0][0]).toMatchObject({ sourceProjectRoot: sourceRoot, sourceAgentId: sourceId, displayName: expected });
  await screen.findByTestId(`chip-${recruit.mock.calls[0][0].agentId}`);
});
