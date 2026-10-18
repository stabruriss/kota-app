import { act, cleanup, fireEvent, render, renderHook, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import { useLocalWorkbenchLaunch } from '../src/hooks/useLocalWorkbenchLaunch';
import * as pty from '../src/pty-client';
import * as auth from '../src/lib/local-auth';
import * as startup from '../src/lib/local-startup';
import { localConfigurationSaveFailure, type LocalConfigurationChange } from '../src/lib/local-configuration';
import { stringErrorText } from '../src/lib/string-routing';

const events = vi.hoisted(() => new Map<string, Set<(event: { payload: LocalConfigurationChange }) => void>>());
vi.mock('@tauri-apps/api/event', () => ({ listen: async (name: string, callback: (event: { payload: LocalConfigurationChange }) => void) => {
  const listeners = events.get(name) ?? new Set(); events.set(name, listeners); listeners.add(callback);
  return () => { listeners.delete(callback); };
} }));
vi.mock('../src/lib/local-rpc', async original => ({
  ...await original<typeof import('../src/lib/local-rpc')>(), KOTA_LOCAL_ENABLED: true,
}));
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn(async () => null) }));
const root = '/fixture/configuration', agentId = 'agent-config';
const models: auth.LocalModelCatalog = { schema: 1, providers: [{ providerId: 'fable', linked: true, models: [
  { modelId: 'reason', name: 'Reason', thinkingLevels: ['low', 'high'] },
  { modelId: 'plain', name: 'Plain', thinkingLevels: ['off'] },
] }] };
let detail: pty.ProjectAgentDetail;
let configuration: { model: string; thinkingLevel: string };
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
async function emit(patch: Partial<LocalConfigurationChange> = {}) {
  await act(async () => { for (const callback of events.get(`local-rpc://agent/${agentId}/configuration`) ?? []) {
    callback({ payload: { projectRoot: root, agentId, revision: 'new-revision', ...patch } });
  } });
}
beforeEach(() => {
  vi.restoreAllMocks(); events.clear();
  configuration = { model: 'fable/reason', thinkingLevel: 'low' };
  detail = { agentId, displayName: 'Fixture', provider: 'kota-local', cli: 'kota-local', model: 'fable/reason', effort: 'low',
    shellRevision: 'opening-revision', skills: [], ghost: 'Original ghost', args: [], projectName: 'fixture',
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, status: 'active', sourceHeroId: 'fixture', sourceHeroName: 'Fixture',
    projectId: 'fixture', avatarId: null, shellPath: `${root}/SHELL.yaml`, adapterPath: `${root}/AGENTS.md`, agentYamlPath: `${root}/agent.yaml`,
    forkable: false, dirty: false, dirtySummary: '', inviteEligibility: { eligible: false, reason: 'fixture', proposedHeroId: 'fixture', proposedDisplayName: 'Fixture' } };
  vi.spyOn(pty, 'loadProjectAgentDetail').mockImplementation(async () => structuredClone(detail));
  vi.spyOn(pty, 'saveProjectAgentDetail').mockImplementation(async request => ({ ...detail, model: request.model, effort: request.effort }));
  vi.spyOn(pty, 'listAccountSkills').mockResolvedValue([{ id: 'fixture-skill', name: 'Fixture skill', description: 'fixture', path: '/fixture/skill',
    kind: 'manual', bundledDefault: false, valid: true, createdAt: '' }]);
  vi.spyOn(pty, 'supportedShellsStatus').mockResolvedValue([]);
  vi.spyOn(pty, 'refreshProviderModelOptions').mockResolvedValue([]);
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue(models);
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'ready', providers: [{ providerId: 'fable', status: 'signed_in', authType: 'api_key' }] });
  vi.spyOn(startup, 'onLocalStartup').mockResolvedValue(() => {});
  vi.spyOn(startup, 'localLaunchAction').mockImplementation(async (_root, _agent, action) => {
    if (action === 'configuration') return structuredClone(configuration) as never;
    if (action === 'begin') return { reused: false, view: { projectRoot: root, agentId, attemptId: 'start', revision: 1,
      state: 'running', selection: null, saved: false, error: null, steps: [] } } as never;
    return null as never;
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });
function mountProfile() {
  const onSaved = vi.fn();
  const view = render(<ProjectAgentProfileOverlay agentId={agentId} projectRoot={root} existingNames={[]}
    onClose={() => {}} onSaved={onSaved} onRemoveFromProject={() => {}} />);
  return { ...view, onSaved };
}

it('adopts saved configuration over an offline draft, scopes events, and cannot Start with the stale draft while reloading', async () => {
  const { result } = renderHook(() => useLocalWorkbenchLaunch({ projectRoot: root, agentId, connected: false, loaded: true,
    focusRequest: 0, onResume: vi.fn(async () => {}), onStarted: vi.fn() }));
  await waitFor(() => expect(result.current.saved?.id).toBe('reason'));
  await waitFor(() => expect(result.current.catalog).not.toBeNull());
  act(() => result.current.selectModel('fable', 'plain'));
  expect(result.current.saved?.id).toBe('plain');
  const reads = vi.mocked(startup.localLaunchAction).mock.calls.length;
  await emit({ projectRoot: '/other' }); await emit({ agentId: 'other-agent' });
  expect(startup.localLaunchAction).toHaveBeenCalledTimes(reads); expect(result.current.saved?.id).toBe('plain');
  const pending = deferred<typeof configuration>();
  vi.mocked(startup.localLaunchAction).mockImplementation(async (_root, _agent, action) => action === 'configuration' ? await pending.promise as never
    : action === 'begin' ? { reused: false, view: { projectRoot: root, agentId, attemptId: 'start', revision: 1,
      state: 'running', selection: null, saved: false, error: null, steps: [] } } as never : null as never);
  await emit();
  expect(result.current.saved?.id).toBe('reason');
  await act(async () => { await result.current.start(); });
  expect(startup.localLaunchAction).toHaveBeenCalledWith(root, agentId, 'begin', { selection: undefined });
  await act(async () => pending.resolve({ model: 'fable/reason', thinkingLevel: 'high' }));
  expect(result.current.saved?.thinking).toBe('high');
});

it('refreshes an open clean Detail without replacing name, Ghost or skills drafts', async () => {
  const view = mountProfile();
  const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Current SHELL settings')).toHaveTextContent(/reason/i));
  await userEvent.click(view.container.querySelector<HTMLButtonElement>('.profile-name-button')!);
  fireEvent.change(screen.getByLabelText('Given name'), { target: { value: 'Unsubmitted name' } });
  await userEvent.click(screen.getByRole('button', { name: 'Edit GHOST' }));
  fireEvent.change(view.container.querySelector<HTMLTextAreaElement>('textarea')!, { target: { value: 'Unsubmitted ghost' } });
  await userEvent.click(screen.getByRole('checkbox', { name: 'Fixture skill skill' }));
  detail = { ...detail, model: 'fable/plain', effort: 'off', shellRevision: 'new-revision', displayName: 'External name', ghost: 'External ghost' };
  await emit();
  await waitFor(() => expect(shell.getByLabelText('Current SHELL settings')).toHaveTextContent(/plain.*off/i));
  expect(screen.getByLabelText('Given name')).toHaveValue('Unsubmitted name');
  expect(view.container.querySelector('textarea')).toHaveValue('Unsubmitted ghost');
  expect(screen.getByRole('checkbox', { name: 'Fixture skill skill' })).toBeChecked();
  expect(pty.loadProjectAgentDetail).toHaveBeenLastCalledWith({ projectRoot: root, agentId }, { readOnly: true });
  expect(pty.saveProjectAgentDetail).not.toHaveBeenCalled();
});

it('does not overwrite an editing Detail and retains its opening revision for the existing Reload protection', async () => {
  mountProfile(); const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Thinking')).toBeEnabled());
  await userEvent.selectOptions(shell.getByLabelText('Thinking'), 'high');
  const reads = vi.mocked(pty.loadProjectAgentDetail).mock.calls.length;
  detail = { ...detail, model: 'fable/plain', effort: 'off', shellRevision: 'external-revision' };
  await emit(); expect(pty.loadProjectAgentDetail).toHaveBeenCalledTimes(reads);
  expect(shell.getByLabelText('Thinking')).toHaveValue('high');
  vi.mocked(pty.saveProjectAgentDetail).mockRejectedValue('local_config_changed');
  await userEvent.click(shell.getByRole('button', { name: 'Save SHELL' }));
  await screen.findByText('Settings changed outside this editor. Reload before saving.');
  expect(pty.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({ shellRevision: 'opening-revision' }));
  await userEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await waitFor(() => expect(shell.getByLabelText('Current SHELL settings')).toHaveTextContent(/plain.*off/i));
});

it.each([true, false])('shows actual saved thinking and honest appliedToSession=%s feedback', async appliedToSession => {
  const view = mountProfile(); const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Thinking')).toBeEnabled());
  await userEvent.selectOptions(shell.getByLabelText('Thinking'), 'high');
  const pending = deferred<pty.ProjectAgentDetail>();
  vi.mocked(pty.saveProjectAgentDetail).mockReturnValue(pending.promise);
  await userEvent.click(shell.getByRole('button', { name: 'Save SHELL' }));
  const reads = vi.mocked(pty.loadProjectAgentDetail).mock.calls.length;
  await emit(); expect(pty.loadProjectAgentDetail).toHaveBeenCalledTimes(reads);
  const next = { ...detail, effort: appliedToSession ? 'low' : 'high', shellRevision: 'saved-revision', appliedToSession };
  await act(async () => pending.resolve(next));
  await screen.findByText(appliedToSession ? 'Saved and applied to the running session.' : 'Saved. Applies on next launch.');
  expect(view.onSaved).toHaveBeenCalledWith(next, 'kota-local');
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  expect(shell.getByLabelText('Thinking')).toHaveValue(next.effort);
});

it.each(['local_model_apply_unconfirmed', 'local_model_session_replaced', 'local_model_session_closed',
  'local_model_applied_save_failed', 'local_model_applied_metadata_failed'])('reports %s without claiming a clean no-op, and rereads saved SHELL only', async code => {
  const view = mountProfile(); const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Thinking')).toBeEnabled());
  await userEvent.selectOptions(shell.getByLabelText('Thinking'), 'high');
  vi.mocked(pty.saveProjectAgentDetail).mockRejectedValue(code);
  detail = { ...detail, model: 'fable/plain', effort: 'off', shellRevision: 'disk-revision' };
  await userEvent.click(shell.getByRole('button', { name: 'Save SHELL' }));
  const copy = localConfigurationSaveFailure(code)!.message;
  await screen.findByText(copy);
  expect(pty.loadProjectAgentDetail).toHaveBeenLastCalledWith({ projectRoot: root, agentId }, { readOnly: true });
  expect(shell.getByLabelText('Thinking')).toHaveValue('off');
  expect(view.onSaved).not.toHaveBeenCalled();
  expect(stringErrorText(code)).toBe(copy); // Tavern's shared save command has the same failure semantics.
  expect(copy).not.toContain('session is unchanged');
});

it('keeps an old offline draft discarded after a failed read, and ignores a stale failure after a newer successful read', async () => {
  const { result } = renderHook(() => useLocalWorkbenchLaunch({ projectRoot: root, agentId, connected: false, loaded: true,
    focusRequest: 0, onResume: vi.fn(async () => {}), onStarted: vi.fn() }));
  await waitFor(() => expect(result.current.saved?.id).toBe('reason'));
  act(() => result.current.selectModel('fable', 'plain'));
  vi.mocked(startup.localLaunchAction).mockRejectedValueOnce({ code: 'local_config_unavailable' });
  await emit();
  expect(result.current.saved?.id).toBe('reason');
  expect(result.current.requestError).toEqual({ code: 'local_config_unavailable', stage: 'configuration_read' });
  const late = deferred<typeof configuration>();
  vi.mocked(startup.localLaunchAction).mockReturnValueOnce(late.promise);
  await emit();
  configuration = { model: 'fable/reason', thinkingLevel: 'high' };
  await emit();
  expect(result.current.saved?.thinking).toBe('high');
  expect(result.current.requestError).toBeNull();
  await act(async () => late.reject({ code: 'local_config_unavailable' }));
  expect(result.current.requestError).toBeNull();
});

it('discards a late clean-Detail refresh if editing began, and unsubscribes on close', async () => {
  const view = mountProfile(); const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await waitFor(() => expect(shell.getByRole('button', { name: 'Edit SHELL' })).toBeEnabled());
  const late = deferred<pty.ProjectAgentDetail>();
  vi.mocked(pty.loadProjectAgentDetail).mockReturnValueOnce(late.promise);
  await emit();
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Thinking')).toBeEnabled());
  await userEvent.selectOptions(shell.getByLabelText('Thinking'), 'high');
  await act(async () => late.resolve({ ...detail, model: 'fable/plain', effort: 'off', shellRevision: 'late-revision' }));
  expect(shell.getByLabelText('Thinking')).toHaveValue('high');
  expect(shell.getByRole('button', { name: 'Local model' })).toHaveTextContent('Reason');
  view.unmount(); expect(events.get(`local-rpc://agent/${agentId}/configuration`)?.size).toBe(0);
});

it('never claims a failed saved-settings reload succeeded after an applied-but-unsaved error', async () => {
  mountProfile(); const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await waitFor(() => expect(shell.getByRole('button', { name: 'Edit SHELL' })).toBeEnabled());
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Thinking')).toBeEnabled());
  await userEvent.selectOptions(shell.getByLabelText('Thinking'), 'high');
  vi.mocked(pty.saveProjectAgentDetail).mockRejectedValue('local_model_applied_save_failed');
  vi.mocked(pty.loadProjectAgentDetail).mockRejectedValueOnce('fixture read unavailable');
  await userEvent.click(shell.getByRole('button', { name: 'Save SHELL' }));
  await screen.findByText(`${localConfigurationSaveFailure('local_model_applied_save_failed')!.message} Could not reload saved settings. Reopen this profile.`);
  expect(screen.queryByText(/have been reloaded/)).not.toBeInTheDocument();
});
