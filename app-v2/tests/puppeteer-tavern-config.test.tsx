import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import * as client from '../src/pty-client';
import * as auth from '../src/lib/local-auth';
import * as jev from '../src/lib/jev';
import * as dance from '../src/lib/puppeteer-dance';
import { applyAgentString, saveAgentString } from '../src/lib/puppeteer-projects';
import { PuppeteerTavernConfig } from '../src/chrome/PuppeteerTavernConfig';
import { PuppeteerPromptFile } from '../src/chrome/PuppeteerPromptFile';
import type { AutoRouteGroup } from '../src/lib/string-routing';
import { invoke } from '@tauri-apps/api/core';
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn() }));
const group: AutoRouteGroup = { threshold: .85, defaultModel: 'model-1', models: [
  { id: 'model-1', providerId: 'openai', modelId: 'sol', thinkingLevel: 'high', intent: 'Hard work' },
  { id: 'model-2', providerId: 'openai', modelId: 'luna', thinkingLevel: 'low', intent: 'Quick work' },
] };
const project = (localRoot: string, projectId = localRoot): client.WorkspaceProject => ({ projectId, localRoot, repoFullName: `mock/${projectId}`, remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'HEAD', localRootBytes: 0, sourceDir: `${localRoot}/source`, sourceDirBytes: 0, sharedDir: `${localRoot}/project-memory`, rulesDir: `${localRoot}/project-rules`, agents: [] });
const agent = (agentId: string, provider = 'kota-local', model = 'kota/auto'): client.ProjectAgentDetail => ({ agentId, displayName: agentId, sourceHeroId: 'Hero-ONE', sourceHeroName: 'Source', projectId: 'one', projectName: 'One', cli: 'kota-local', provider, model, effort: 'off', autoRoute: structuredClone(group), avatarId: 'codex', skills: ['keep-skill'], args: [], ghost: 'keep this ghost', adapterPath: '', shellPath: '', shellRevision: 'shell-before', agentYamlPath: '', status: 'active', record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, inviteEligibility: { eligible: true, proposedHeroId: 'Hero-ONE', proposedDisplayName: agentId }, forkable: false, dirty: false, dirtySummary: '' });
const identities = [agent('Ada'), agent('Babbage'), agent('Concrete', 'kota-local', 'openai/sol'), agent('Terminal', 'codex', 'default')];
beforeEach(() => {
  vi.spyOn(client, 'listWorkspaceProjects').mockResolvedValue([project('/one', 'One'), project('/two', 'Two')]);
  vi.spyOn(client, 'listArchivedWorkspaceProjects').mockResolvedValue([]);
  vi.spyOn(client, 'listProjectAgentIdentities').mockImplementation(async root => (root === '/one' ? identities : [agent('Copper')]).map(detail => ({ agentId: detail.agentId, displayName: detail.displayName, sourceHeroId: detail.sourceHeroId, status: detail.status, provider: detail.provider, avatarId: detail.avatarId })));
  vi.spyOn(client, 'loadProjectAgentDetail').mockImplementation(async ({ agentId }) => identities.find(detail => detail.agentId === agentId) ?? agent(agentId));
  vi.spyOn(client, 'loadProjectAgentLayoutFile').mockResolvedValue({ version: 1, projectRoot: '/one', updatedAt: '', tableSlots: ['Ada', 'Babbage'] });
  vi.spyOn(client, 'readSystemPrompt').mockResolvedValue({ path: '/account/heroes/system-puppeteer/puppeteer-prompts.json', content: '{"four":"questions"}' });
  vi.spyOn(jev, 'readJevSettings').mockResolvedValue({ schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] });
  vi.mocked(invoke).mockImplementation(async () => await jev.readJevSettings() as never);
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'ready', providers: [] });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [{ providerId: 'openai', linked: true, models: [
    { modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'high'] }, { modelId: 'luna', name: 'Luna', thinkingLevels: ['off', 'low'] },
  ] }] });
  vi.spyOn(client, 'saveProjectAgentDetail').mockImplementation(async request => ({ ...agent(request.agentId), ...request, autoRoute: request.autoRoute ?? undefined }));
  vi.spyOn(dance, 'readDance').mockResolvedValue({ settings: { schema: 1, enabled: false, sentinel: 'Ada', strategy: 'context', threshold: .75, conditions: {} }, sourceRevision: 'fixture-revision', currentSentinel: null });
});
afterEach(() => vi.restoreAllMocks());
it('groups only saved Local Auto String agents, opens their own editor directly and stages until OK', async () => {
  render(<PuppeteerTavernConfig />);
  expect(await screen.findByText('mock/One')).toBeInTheDocument();
  expect(screen.getByText('mock/Two')).toBeInTheDocument();
  expect(screen.queryByText('Concrete')).not.toBeInTheDocument(); expect(screen.queryByText('Terminal')).not.toBeInTheDocument();
  expect(auth.readLocalModelCatalog).not.toHaveBeenCalled();
  expect(screen.getAllByText('2 models · Threshold 0.85')).toHaveLength(3);
  await userEvent.click(screen.getByRole('button', { name: 'Edit Auto String for Ada in mock/One' }));
  const dialog = screen.getByRole('dialog', { name: 'Auto String' });
  await waitFor(() => expect(within(dialog).getByRole('button', { name: 'OK' })).toBeEnabled());
  expect(within(dialog).getByText('No changes.', { selector: 'p[role="status"]' })).toBeVisible();
  expect(within(dialog).getByText('Click OK to save.')).toBeVisible();
  fireEvent.change(within(dialog).getByRole('textbox', { name: 'Model 1 intent' }), { target: { value: 'Revised for Ada' } });
  expect(within(dialog).getByText('Unsaved changes', { selector: 'p[role="status"]' })).toBeVisible();
  expect(client.saveProjectAgentDetail).not.toHaveBeenCalled();
  expect(screen.queryByRole('dialog', { name: /profile|workbench/i })).not.toBeInTheDocument();
  await userEvent.click(within(dialog).getByRole('button', { name: 'OK' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Auto String' })).not.toBeInTheDocument());
  expect(client.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({ projectRoot: '/one', agentId: 'Ada', ghost: 'keep this ghost', skills: ['keep-skill'], autoRoute: { ...group, models: [{ ...group.models[0], intent: 'Revised for Ada' }, group.models[1]] } }));
});
it('tracks real Tavern dirty state through reverting, discarding, saving and reopening', async () => {
  render(<PuppeteerTavernConfig />); await screen.findByText('mock/One');
  const open = async () => {
    await userEvent.click(screen.getByRole('button', { name: 'Edit Auto String for Ada in mock/One' }));
    const dialog = within(screen.getByRole('dialog', { name: 'Auto String' }));
    await waitFor(() => expect(dialog.getByRole('button', { name: 'OK' })).toBeEnabled());
    return dialog;
  };
  let editor = await open();
  expect(editor.getByText('No changes.', { selector: 'p[role="status"]' })).toBeVisible();
  fireEvent.change(editor.getByRole('slider'), { target: { value: '.9' } });
  expect(editor.getByText('Unsaved changes', { selector: 'p[role="status"]' })).toBeVisible();
  fireEvent.change(editor.getByRole('slider'), { target: { value: '.85' } });
  expect(editor.getByText('No changes.', { selector: 'p[role="status"]' })).toBeVisible();
  fireEvent.change(editor.getByRole('slider'), { target: { value: '.9' } });
  await userEvent.keyboard('{Escape}');
  expect(client.saveProjectAgentDetail).not.toHaveBeenCalled();
  editor = await open();
  expect(editor.getByRole('slider')).toHaveValue('0.85');
  expect(editor.getByText('No changes.', { selector: 'p[role="status"]' })).toBeVisible();
  fireEvent.change(editor.getByRole('slider'), { target: { value: '.9' } });
  let finish!: (value: client.ProjectAgentDetail) => void;
  vi.mocked(client.saveProjectAgentDetail).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await userEvent.click(editor.getByRole('button', { name: 'OK' }));
  await waitFor(() => expect(client.saveProjectAgentDetail).toHaveBeenCalledOnce());
  expect(editor.getByText('Saving…', { selector: 'p[role="status"]' })).toBeVisible();
  const saved = { ...agent('Ada'), autoRoute: { ...group, threshold: .9 }, shellRevision: 'saved-revision' };
  await act(async () => finish(saved));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Auto String' })).not.toBeInTheDocument());
  vi.mocked(client.loadProjectAgentDetail).mockImplementation(async ({ agentId }) => agentId === 'Ada' ? saved : agent(agentId));
  editor = await open();
  expect(editor.getByRole('slider')).toHaveValue('0.9');
  expect(editor.getByText('No changes.', { selector: 'p[role="status"]' })).toBeVisible();
  expect(editor.getByText('Click OK to save.')).toBeVisible();
});
it('applies a complete group serially across projects, keeps partial successes and leaves source edits staged', async () => {
  let finish!: (detail: client.ProjectAgentDetail) => void;
  vi.mocked(client.saveProjectAgentDetail).mockImplementationOnce(() => new Promise(done => { finish = done; })).mockRejectedValueOnce({ secret: 'DO_NOT_RENDER' });
  render(<PuppeteerTavernConfig />); await screen.findByText('mock/One');
  await userEvent.click(screen.getByRole('button', { name: 'Edit Auto String for Ada in mock/One' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Apply to' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Apply to' }));
  const apply = screen.getByRole('dialog', { name: 'Apply Auto String settings' });
  const boxes = await within(apply).findAllByRole('checkbox'); expect(boxes).toHaveLength(2);
  await userEvent.click(boxes[0]); await userEvent.click(boxes[1]); await userEvent.click(within(apply).getByRole('button', { name: 'Apply' }));
  await waitFor(() => expect(client.saveProjectAgentDetail).toHaveBeenCalledOnce());
  expect(within(apply).getByRole('button', { name: 'Applying…' })).toBeDisabled();
  await act(async () => { finish(agent('Babbage')); });
  await waitFor(() => expect(within(apply).getByRole('status')).toHaveTextContent('1 applied. 1 failed.'));
  expect(boxes[0]).not.toBeChecked(); expect(boxes[1]).toBeChecked();
  expect(client.saveProjectAgentDetail).toHaveBeenNthCalledWith(2, expect.objectContaining({ projectRoot: '/two', agentId: 'Copper', autoRoute: group }));
  expect(screen.queryByText('DO_NOT_RENDER')).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name: 'Apply Auto String settings' })).not.toBeInTheDocument();
  expect(screen.getByRole('dialog', { name: 'Auto String' })).toBeInTheDocument();
  expect(client.saveProjectAgentDetail).not.toHaveBeenCalledWith(expect.objectContaining({ agentId: 'Ada' }));
});
it('rereads each target before Apply and refuses a target that stopped using String without modifying another field', async () => {
  vi.mocked(client.loadProjectAgentDetail).mockResolvedValueOnce(agent('Concrete', 'kota-local', 'openai/sol'));
  await expect(saveAgentString('/one', 'Concrete', group)).rejects.toEqual({ code: 'string_agent_changed' });
  expect(client.saveProjectAgentDetail).not.toHaveBeenCalled();
  const result = vi.fn(); await applyAgentString([{ projectRoot: '/one', agentId: 'Babbage' }, { projectRoot: '/two', agentId: 'Copper' }], group, result);
  expect(result).toHaveBeenCalledTimes(2); expect(client.saveProjectAgentDetail).toHaveBeenCalledTimes(2);
});
it('rejects a stale String file baseline before calling the shared profile writer', async () => {
  vi.mocked(client.loadProjectAgentDetail).mockResolvedValue({ ...agent('Ada'), shellRevision: 'agent-edited' });
  await expect(saveAgentString('/one', 'Ada', group, 'shell-before')).rejects.toEqual({ code: 'local_config_changed' });
  expect(client.loadProjectAgentDetail).toHaveBeenCalledWith({ projectRoot: '/one', agentId: 'Ada' }, { readOnly: true });
  expect(client.saveProjectAgentDetail).not.toHaveBeenCalled();
  await saveAgentString('/one', 'Ada', group, 'agent-edited');
  expect(client.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({ shellRevision: 'agent-edited', ghost: 'keep this ghost' }));
});
it('shows all project Dance cards with seated names, without loading agent SHELL/model details or changing projects', async () => {
  render(<PuppeteerTavernConfig />); await screen.findByText('mock/One');
  vi.mocked(client.loadProjectAgentDetail).mockClear();
  await userEvent.click(screen.getByRole('tab', { name: 'Dance' }));
  await screen.findByRole('region', { name: 'Puppeteer Dance · mock/One' });
  await waitFor(() => expect(dance.readDance).toHaveBeenCalledWith('/two'));
  expect(screen.getAllByRole('button', { name: 'Turn Dance on' })).toHaveLength(2);
  expect(client.loadProjectAgentDetail).not.toHaveBeenCalled(); expect(auth.readLocalModelCatalog).not.toHaveBeenCalled();
});
it('does not list or inspect archived projects in either tab or the Apply targets', async () => {
  vi.mocked(client.listArchivedWorkspaceProjects).mockResolvedValue([project('/archived', 'Archived')]);
  render(<PuppeteerTavernConfig />); await screen.findByText('mock/One');
  expect(screen.queryByText('mock/Archived')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Edit Auto String for Ada in mock/One' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Apply to' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Apply to' }));
  expect(await within(screen.getByRole('dialog', { name: 'Apply Auto String settings' })).findAllByRole('checkbox')).toHaveLength(2);
  await userEvent.keyboard('{Escape}{Escape}');
  await userEvent.click(screen.getByRole('tab', { name: 'Dance' }));
  await screen.findByRole('region', { name: 'Puppeteer Dance · mock/One' });
  expect(screen.queryByRole('region', { name: /Archived/ })).not.toBeInTheDocument();
  expect(client.listArchivedWorkspaceProjects).not.toHaveBeenCalled();
  expect(client.listProjectAgentIdentities).not.toHaveBeenCalledWith('/archived');
  expect(dance.readDance).not.toHaveBeenCalledWith('/archived');
});
it('reads account-wide Mark and Dream lazily, without inspecting projects or losing it on tab switches', async () => {
  const soul = { sourceRevision: 'soul-before', jevReady: true, settings: { schema: 1, enabled: false,
    markThreshold: .85, dreamThreshold: .90, maxMarks: 3, maxDreams: 2, cooldownReplies: 10, minimumMessageLength: 60 } };
  vi.mocked(invoke).mockImplementation(async command => (command === 'puppeteer_soul_read' ? soul : await jev.readJevSettings()) as never);
  render(<PuppeteerTavernConfig />); await screen.findByText('mock/One');
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'puppeteer_soul_read')).toBe(false);
  vi.mocked(client.listWorkspaceProjects).mockClear(); vi.mocked(client.listProjectAgentIdentities).mockClear();
  await userEvent.click(screen.getByRole('tab', { name: 'Mark and Dream' }));
  expect(await screen.findByRole('switch', { name: 'Use marks and dreams' })).not.toBeChecked();
  expect(client.listWorkspaceProjects).not.toHaveBeenCalled(); expect(client.listProjectAgentIdentities).not.toHaveBeenCalled();
  expect(auth.readLocalModelCatalog).not.toHaveBeenCalled();
  await userEvent.keyboard('{Home}');
  expect(screen.getByRole('tab', { name: 'String' })).toHaveFocus();
  await userEvent.keyboard('{End}');
  expect(screen.getByRole('tab', { name: 'Mark and Dream' })).toHaveFocus();
  expect(vi.mocked(invoke).mock.calls.filter(([command]) => command === 'puppeteer_soul_read')).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
});
it('routes the missing-Jev guide to String and focuses the existing shared Jev settings', async () => {
  const soul = { sourceRevision: 'soul-before', jevReady: false, settings: { schema: 1, enabled: true,
    markThreshold: .85, dreamThreshold: .90, maxMarks: 3, maxDreams: 2, cooldownReplies: 10, minimumMessageLength: 60 } };
  vi.mocked(invoke).mockImplementation(async command => (command === 'puppeteer_soul_read' ? soul : await jev.readJevSettings()) as never);
  render(<PuppeteerTavernConfig />); await screen.findByText('mock/One');
  await userEvent.click(screen.getByRole('tab', { name: 'Mark and Dream' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Set up Jev under String to enable.' }));
  expect(screen.getByRole('tab', { name: 'String' })).toHaveAttribute('aria-selected', 'true');
  await waitFor(() => expect(document.getElementById('puppeteer-jev')).toHaveFocus());
  expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'puppeteer_soul_save')).toBe(false);
});
it.each(['Puppeteer prompt file is invalid. Using bundled defaults.', 'Could not read the Puppeteer prompt file. Using bundled defaults.'])('previews full effective JSON and exact editable file path/warning (%s)', async warning => {
  const content = '{"stringFirst":{"instructions":"user copy"},"danceRule":{}}';
  vi.mocked(client.readSystemPrompt).mockResolvedValue({ path: '/account/heroes/system-puppeteer/puppeteer-prompts.json', content, warning });
  render(<PuppeteerPromptFile />);
  await screen.findByText(warning);
  expect(screen.getByLabelText('Puppeteer prompt preview')).toHaveTextContent(content);
  expect(screen.getByText('/account/heroes/system-puppeteer/puppeteer-prompts.json')).toBeInTheDocument();
  expect(client.readSystemPrompt).toHaveBeenCalledWith({ path: 'puppeteer-prompts.json' }, expect.any(String));
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument(); expect(screen.queryByRole('button', { name: /Save|Reset/ })).not.toBeInTheDocument();
});
