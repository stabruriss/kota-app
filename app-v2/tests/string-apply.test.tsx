import { useState } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import * as client from '../src/pty-client';
import { StringRoutingEditor } from '../src/chrome/StringRoutingEditor';
import { stringGroupToDraft, type AutoRouteGroup } from '../src/lib/string-routing';

const group: AutoRouteGroup = { threshold: .5, defaultModel: 'model-1', models: [
  { id: 'model-1', providerId: 'openai', modelId: 'sol', thinkingLevel: 'high', intent: 'Hard work' },
  { id: 'model-2', providerId: 'openai', modelId: 'luna', thinkingLevel: 'off', intent: 'Quick work' },
] };
const project = (localRoot: string): client.WorkspaceProject => ({ projectId: localRoot, localRoot, repoFullName: localRoot, remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'HEAD', localRootBytes: 0, sourceDir: '', sourceDirBytes: 0, sharedDir: '', rulesDir: '', agents: [] });
const agent = (agentId: string, provider = 'kota-local', model = 'kota/auto'): client.ProjectAgentDetail => ({ agentId, displayName: agentId,
  sourceHeroId: 'Hero-Fixture', sourceHeroName: 'Fixture', projectId: 'one', projectName: 'One', cli: 'kota-local', provider, model,
  effort: 'off', autoRoute: structuredClone(group), avatarId: 'codex', skills: ['retain'], args: [], ghost: 'target ghost', adapterPath: '',
  shellPath: '', shellRevision: 'target-revision', agentYamlPath: '', status: 'active', record: { turns: 0, incarnations: 1, estimatedTokens: 0 },
  inviteEligibility: { eligible: false, proposedHeroId: 'Hero-Fixture', proposedDisplayName: agentId }, forkable: false, dirty: false, dirtySummary: '' });
const details = [agent('Ada'), agent('Milo'), agent('Concrete', 'kota-local', 'openai/sol'), agent('Terminal', 'codex', 'sol')];
beforeEach(() => {
  vi.spyOn(client, 'listWorkspaceProjects').mockResolvedValue([project('/one'), project('/two')]);
  vi.spyOn(client, 'listArchivedWorkspaceProjects').mockResolvedValue([project('/archived')]);
  vi.spyOn(client, 'listProjectAgentIdentities').mockImplementation(async root => (root === '/one' ? details : [agent('Mira')]).map(item => ({ agentId: item.agentId, displayName: item.displayName, status: item.status, provider: item.provider, avatarId: item.avatarId, sourceHeroId: item.sourceHeroId })));
  vi.spyOn(client, 'loadProjectAgentDetail').mockImplementation(async ({ agentId }) => details.find(item => item.agentId === agentId) ?? agent(agentId));
  vi.spyOn(client, 'saveProjectAgentDetail').mockImplementation(async request => ({ ...agent(request.agentId), ...request, autoRoute: request.autoRoute ?? undefined }));
});
afterEach(() => vi.restoreAllMocks());
function Harness({ surface = 'hero', ready = true }: { surface?: 'workbench' | 'profile' | 'hero' | 'tavern'; ready?: boolean }) {
  const [draft, setDraft] = useState(stringGroupToDraft(group));
  return <StringRoutingEditor owner={surface === 'hero' ? 'Analyst' : 'Ada'} surface={surface} draft={draft} catalog={null}
    enabled={ready} onChange={setDraft} onClose={vi.fn()} applySource={surface === 'hero' ? undefined : { projectRoot: '/one', agentId: 'Ada' }} />;
}

it.each(['workbench', 'profile', 'hero', 'tavern'] as const)('%s shares lazy Apply and excludes only its source (none for Hero)', async surface => {
  render(<Harness surface={surface} />);
  for (const name of ['Import', 'Save as template', 'Apply to']) expect(screen.getByRole('button', { name })).toHaveClass('local-model-text');
  expect(client.listWorkspaceProjects).not.toHaveBeenCalled(); expect(client.loadProjectAgentDetail).not.toHaveBeenCalled();
  // WebKit pointer activation does not focus buttons by default.
  screen.getByRole('dialog', { name: 'Auto String' }).focus();
  fireEvent.click(screen.getByRole('button', { name: 'Apply to' }));
  const panel = screen.getByRole('dialog', { name: 'Apply Auto String settings' });
  expect(within(panel).getByText(surface === 'hero' ? 'From Hero Analyst' : 'From Ada')).toBeVisible();
  const boxes = await within(panel).findAllByRole('checkbox'); expect(boxes).toHaveLength(surface === 'hero' ? 3 : 2);
  expect(within(panel).queryByText('Concrete')).not.toBeInTheDocument(); expect(within(panel).queryByText('Terminal')).not.toBeInTheDocument();
  expect(!!within(panel).queryByText('Ada')).toBe(surface === 'hero');
  expect(client.listArchivedWorkspaceProjects).not.toHaveBeenCalled();
  expect(client.saveProjectAgentDetail).not.toHaveBeenCalled();
  await userEvent.keyboard('{Escape}'); expect(screen.queryByRole('dialog', { name: 'Apply Auto String settings' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Apply to' })).toHaveFocus();
});

it('copies the current edited Hero draft serially; retry sends failed targets only and never saves the source', async () => {
  let finish!: (detail: client.ProjectAgentDetail) => void;
  vi.mocked(client.saveProjectAgentDetail).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; })).mockRejectedValueOnce({ code: 'fixture', secret: 'DO_NOT_RENDER' });
  render(<Harness />);
  fireEvent.change(screen.getByRole('slider'), { target: { value: '.75' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Model 1 intent' }), { target: { value: 'Edited on screen' } });
  await userEvent.click(screen.getByRole('button', { name: 'Apply to' }));
  const panel = screen.getByRole('dialog', { name: 'Apply Auto String settings' });
  await within(panel).findAllByRole('checkbox');
  await userEvent.click(within(panel).getByRole('checkbox', { name: 'Milo' }));
  await userEvent.click(within(panel).getByRole('checkbox', { name: 'Mira' }));
  await userEvent.click(within(panel).getByRole('button', { name: 'Apply' }));
  await waitFor(() => expect(client.saveProjectAgentDetail).toHaveBeenCalledOnce());
  await userEvent.keyboard('{Escape}'); expect(panel).toBeVisible();
  expect(within(panel).getByRole('button', { name: 'Applying…' })).toBeDisabled();
  await act(async () => finish(agent('Milo')));
  await within(panel).findByText('1 applied. 1 failed. Select them to try again.');
  expect(within(panel).getByRole('checkbox', { name: /Milo/ })).not.toBeChecked();
  const failed = within(panel).getByRole('checkbox', { name: /Mira/ }); expect(failed).toBeChecked();
  await userEvent.click(failed); await userEvent.click(failed);
  await userEvent.click(within(panel).getByRole('button', { name: 'Apply' }));
  await within(panel).findByText('2 applied.');
  expect(client.saveProjectAgentDetail).toHaveBeenCalledTimes(3);
  for (const [request] of vi.mocked(client.saveProjectAgentDetail).mock.calls) {
    expect(request.agentId).not.toBe('Ada'); expect(request.autoRoute).toMatchObject({ threshold: .75, models: [{ ...group.models[0], intent: 'Edited on screen' }, group.models[1]] });
    expect(request.ghost).toBe('target ghost'); expect(request.skills).toEqual(['retain']);
  }
  expect(vi.mocked(client.saveProjectAgentDetail).mock.lastCall![0].agentId).toBe('Mira');
  expect(screen.getByRole('textbox', { name: 'Model 1 intent' })).toHaveValue('Edited on screen');
  expect(screen.queryByText('DO_NOT_RENDER')).not.toBeInTheDocument();
});

it('disables Apply when Jev is no longer ready while preserving template actions', async () => {
  const view = render(<Harness />); await userEvent.click(screen.getByRole('button', { name: 'Apply to' }));
  const panel = screen.getByRole('dialog', { name: 'Apply Auto String settings' });
  await within(panel).findAllByRole('checkbox'); await userEvent.click(within(panel).getByRole('checkbox', { name: 'Milo' }));
  view.rerender(<Harness ready={false} />);
  expect(within(panel).getByRole('button', { name: 'Apply' })).toBeDisabled(); expect(client.saveProjectAgentDetail).not.toHaveBeenCalled();
});

it('does not show failed discovery as an empty list and reloads only explicitly', async () => {
  vi.mocked(client.listWorkspaceProjects).mockRejectedValueOnce({ message: 'private' });
  render(<Harness />); await userEvent.click(screen.getByRole('button', { name: 'Apply to' }));
  const panel = screen.getByRole('dialog', { name: 'Apply Auto String settings' });
  await within(panel).findByText('Agent roster unavailable.');
  expect(within(panel).queryByText('No other Local agents are using Auto String.')).not.toBeInTheDocument();
  expect(client.listWorkspaceProjects).toHaveBeenCalledOnce();
  await userEvent.click(within(panel).getByRole('button', { name: 'Retry' }));
  expect(await within(panel).findAllByRole('checkbox')).toHaveLength(3);
});
