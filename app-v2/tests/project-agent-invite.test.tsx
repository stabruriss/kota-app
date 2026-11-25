import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import { loadTavernWorkingHeroes, TAVERN_PROFILE_CHANGED_EVENT } from '../src/chrome/TavernModal';
import * as client from '../src/pty-client';

function detail(): client.ProjectAgentDetail {
  return {
    agentId: 'agent-piner', displayName: '颦儿 v. Kota',
    sourceHeroId: 'hero-dex', sourceHeroName: 'Dex',
    projectId: 'project-kota', projectName: 'Kota', cli: 'codex', provider: 'codex',
    model: 'default', skills: [], args: [], ghost: 'Personal ghost', status: 'active',
    adapterPath: '/fixture/AGENTS.md', shellPath: '/fixture/SHELL.yaml', agentYamlPath: '/fixture/agent.yaml',
    inviteEligibility: {
      eligible: true, proposedHeroId: 'hero-invited-one', proposedDisplayName: '颦儿 v. Kota',
    },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 },
    forkable: false, dirty: false, dirtySummary: '',
  };
}

function setup() {
  const load = vi.spyOn(client, 'loadProjectAgentDetail').mockResolvedValue(detail());
  vi.spyOn(client, 'listAccountSkills').mockResolvedValue([]);
  vi.spyOn(client, 'supportedShellsStatus').mockResolvedValue([]);
  vi.spyOn(client, 'hasTauriRuntime').mockReturnValue(true);
  vi.spyOn(client, 'loadTavernHeroProfiles').mockResolvedValue([]);
  return load;
}

function overlay() {
  return render(<ProjectAgentProfileOverlay agentId="agent-piner" projectRoot="/fixture/project"
    existingNames={[]} onClose={vi.fn()} onSaved={vi.fn()} onRemoveFromProject={vi.fn()} />);
}

const result: client.ProjectAgentInviteResult = {
  heroId: 'hero-invited-one', displayName: '颦儿 v. Kota', path: '/fixture/heroes/hero-invited-one',
};

afterEach(() => {
  vi.restoreAllMocks();
  window.localStorage.removeItem('kota-v2.tavern.hero-profiles');
  window.localStorage.removeItem('kota-v2.tavern.custom-heroes');
});

describe('inviting a project incarnation to Tavern', () => {
  it('blocks rapid clicks and stays Already in Tavern after success and reopening', async () => {
    const load = setup();
    let finish!: (value: client.ProjectAgentInviteResult) => void;
    const invite = vi.spyOn(client, 'inviteProjectAgentToTavern')
      .mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const view = overlay();
    const button = await screen.findByRole('button', { name: 'Invite to Tavern' });
    await waitFor(() => expect(button).toBeEnabled());
    act(() => { fireEvent.click(button); fireEvent.click(button); });
    expect(invite).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Inviting' })).toBeDisabled();
    expect(invite).toHaveBeenCalledWith({ agentId: 'agent-piner', projectRoot: '/fixture/project' });
    await act(async () => finish(result));
    const completed = screen.getByRole('button', { name: 'Already in Tavern' });
    expect(completed).toBeDisabled();
    fireEvent.click(completed);
    expect(invite).toHaveBeenCalledTimes(1);
    expect(screen.getByText('Invited as 颦儿 v. Kota')).toBeInTheDocument();

    view.unmount();
    load.mockResolvedValue({ ...detail(), inviteEligibility: {
      eligible: false, duplicateHeroId: result.heroId,
      proposedHeroId: result.heroId, proposedDisplayName: result.displayName,
      reason: 'This incarnation is already in Tavern.',
    } });
    overlay();
    expect(await screen.findByRole('button', { name: 'Already in Tavern' })).toBeDisabled();
    expect(invite).toHaveBeenCalledTimes(1);
  });

  it('allows a deliberate retry after a failed request, then locks the completed invitation', async () => {
    setup();
    const invite = vi.spyOn(client, 'inviteProjectAgentToTavern')
      .mockRejectedValueOnce(new Error('Could not save invitation'))
      .mockResolvedValue(result);
    overlay();
    const button = await screen.findByRole('button', { name: 'Invite to Tavern' });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    await screen.findByText('Error: Could not save invitation');
    expect(button).toBeEnabled();
    fireEvent.click(button);
    expect(await screen.findByRole('button', { name: 'Already in Tavern' })).toBeDisabled();
    expect(invite).toHaveBeenCalledTimes(2);
  });

  it.each(['fades', 'stays'])('puts the invited note beside the button for a few seconds; a refresh failure %s in the status', async (outcome) => {
    setup();
    vi.spyOn(client, 'inviteProjectAgentToTavern').mockResolvedValue(result);
    if (outcome === 'stays') vi.mocked(client.loadTavernHeroProfiles).mockRejectedValue(new Error('directory unreadable'));
    const view = overlay();
    const button = await screen.findByRole('button', { name: 'Invite to Tavern' });
    await waitFor(() => expect(button).toBeEnabled());
    vi.useFakeTimers();
    try {
      fireEvent.click(button);
      await act(async () => { for (let tick = 0; tick < 50; tick += 1) await Promise.resolve(); });
      const done = screen.getByRole('button', { name: 'Already in Tavern' });
      const toast = view.container.querySelector('.profile-sheet-toast')!;
      expect(toast.parentElement).toContainElement(done);
      if (outcome === 'fades') {
        expect(toast).toHaveTextContent('Invited as 颦儿 v. Kota');
        expect(toast).toHaveAttribute('data-show', 'true');
        expect(view.container.querySelector('.profile-sheet-status')).toBeNull();
        act(() => { vi.advanceTimersByTime(2900); });
        expect(toast).toHaveAttribute('data-show', 'true');
        act(() => { vi.advanceTimersByTime(200); });
        expect(toast).toHaveAttribute('data-show', 'false');
      } else {
        // This one asks the user to act, so it does not fade.
        expect(toast).toHaveAttribute('data-show', 'false');
        const note = screen.getByText('Invited as 颦儿 v. Kota. Recruit list could not refresh; open Tavern to refresh it.');
        expect(note).toHaveClass('profile-sheet-status');
        act(() => { vi.advanceTimersByTime(60000); });
        expect(note).toBeInTheDocument();
      }
    } finally { vi.useRealTimers(); }
  });

  it('refreshes the cache only after invitation succeeds and guards clicks until refresh finishes', async () => {
    setup();
    let finishInvite!: (value: client.ProjectAgentInviteResult) => void;
    let finishRefresh!: (value: client.TavernHeroProfileDraft[]) => void;
    vi.spyOn(client, 'inviteProjectAgentToTavern')
      .mockReturnValue(new Promise((resolve) => { finishInvite = resolve; }));
    const profiles = vi.mocked(client.loadTavernHeroProfiles)
      .mockReturnValue(new Promise((resolve) => { finishRefresh = resolve; }));
    const changed = vi.fn(() => {
      expect(loadTavernWorkingHeroes().map((hero) => hero.id)).toContain(result.heroId);
    });
    window.addEventListener(TAVERN_PROFILE_CHANGED_EVENT, changed);
    try {
      overlay();
      const button = await screen.findByRole('button', { name: 'Invite to Tavern' });
      await waitFor(() => expect(button).toBeEnabled());
      fireEvent.click(button);
      expect(profiles).not.toHaveBeenCalled();
      expect(changed).not.toHaveBeenCalled();
      await act(async () => finishInvite(result));
      await waitFor(() => expect(profiles).toHaveBeenCalledTimes(1));
      expect(changed).not.toHaveBeenCalled();
      const pending = screen.getByRole('button', { name: 'Inviting' });
      expect(pending).toBeDisabled();
      fireEvent.click(pending);
      await act(async () => finishRefresh([{
        heroId: result.heroId, name: result.displayName, provider: 'codex', model: 'default',
        skills: [], ghost: 'Personal ghost', shell: '', kind: 'invited',
      }]));
      expect(changed).toHaveBeenCalledTimes(1);
      expect(screen.getByRole('button', { name: 'Already in Tavern' })).toBeDisabled();
      expect(client.inviteProjectAgentToTavern).toHaveBeenCalledTimes(1);
    } finally {
      window.removeEventListener(TAVERN_PROFILE_CHANGED_EVENT, changed);
    }
  });

  it('keeps a saved invitation locked and distinguishes a recruit-list refresh failure', async () => {
    setup();
    const invite = vi.spyOn(client, 'inviteProjectAgentToTavern').mockResolvedValue(result);
    vi.mocked(client.loadTavernHeroProfiles).mockRejectedValue(new Error('directory unreadable'));
    const cache = JSON.stringify([{ id: 'hero-existing', kind: 'invited', name: 'Existing', provider: 'codex' }]);
    window.localStorage.setItem('kota-v2.tavern.custom-heroes', cache);
    const changed = vi.fn();
    window.addEventListener(TAVERN_PROFILE_CHANGED_EVENT, changed);
    try {
      overlay();
      const button = await screen.findByRole('button', { name: 'Invite to Tavern' });
      await waitFor(() => expect(button).toBeEnabled());
      fireEvent.click(button);
      await screen.findByText('Invited as 颦儿 v. Kota. Recruit list could not refresh; open Tavern to refresh it.');
      const completed = screen.getByRole('button', { name: 'Already in Tavern' });
      expect(completed).toBeDisabled();
      fireEvent.click(completed);
      expect(invite).toHaveBeenCalledTimes(1);
      expect(changed).not.toHaveBeenCalled();
      expect(window.localStorage.getItem('kota-v2.tavern.custom-heroes')).toBe(cache);
      expect(screen.queryByText('Error: directory unreadable')).not.toBeInTheDocument();
    } finally {
      window.removeEventListener(TAVERN_PROFILE_CHANGED_EVENT, changed);
    }
  });

  it('does not read or replace the recruit cache when invitation itself fails', async () => {
    setup();
    vi.spyOn(client, 'inviteProjectAgentToTavern').mockRejectedValue(new Error('Could not save invitation'));
    const cache = JSON.stringify([{ id: 'hero-existing', kind: 'invited', name: 'Existing', provider: 'codex' }]);
    window.localStorage.setItem('kota-v2.tavern.custom-heroes', cache);
    overlay();
    const button = await screen.findByRole('button', { name: 'Invite to Tavern' });
    await waitFor(() => expect(button).toBeEnabled());
    fireEvent.click(button);
    await screen.findByText('Error: Could not save invitation');
    expect(button).toBeEnabled();
    expect(client.loadTavernHeroProfiles).not.toHaveBeenCalled();
    expect(window.localStorage.getItem('kota-v2.tavern.custom-heroes')).toBe(cache);
  });
});
