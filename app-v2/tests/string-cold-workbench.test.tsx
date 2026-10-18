import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LocalAgentWorkbench } from '../src/chrome/LocalAgentWorkbench';
import * as rpc from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';
import * as startup from '../src/lib/local-startup';
import type { JevSettings } from '../src/lib/jev';
import type { AutoRouteGroup } from '../src/lib/string-routing';
import type { LocalWorkbenchView } from '../src/lib/local-rpc';

const ipc = vi.hoisted(() => ({ settings: null as JevSettings | null }));
vi.mock('@tauri-apps/api/core', async original => {
  const api = await original<typeof import('@tauri-apps/api/core')>();
  return { ...api, invoke: (...args: Parameters<typeof api.invoke>) => args[0] === 'local_rpc_jev_read'
    ? Promise.resolve(ipc.settings) : api.invoke(...args) };
});
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
const root = '/fixture/string-cold', agentId = 'agent-string-cold';
const group = (): AutoRouteGroup => ({ threshold: .85, defaultModel: 'model-1', models: [
  { id: 'model-1', providerId: 'openai', modelId: 'sol', thinkingLevel: 'high', intent: 'Hard tasks.' },
  { id: 'model-2', providerId: 'openai', modelId: 'luna', thinkingLevel: 'low', intent: 'Easy tasks.' },
] });
const props = () => ({ projectRoot: root, agentId, focusRequest: 1,
  agent: { name: 'Copper', emoji: '', role: '', hue: '', provider: 'kota-local' }, agents: {},
  onClose: vi.fn(), onDetails: vi.fn(), onFresh: vi.fn(), onResume: vi.fn(() => new Promise(() => {})),
});
beforeEach(() => {
  vi.restoreAllMocks();
  ipc.settings = { schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] };
  vi.spyOn(rpc, 'readLocalWorkbench').mockResolvedValue(null);
  vi.spyOn(rpc, 'controlLocalWorkbench');
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'not_ready', providers: [
    { providerId: 'openai', status: 'not_signed_in' },
  ] });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [{ providerId: 'openai', linked: false, models: [
    { modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'high'] },
    { modelId: 'luna', name: 'Luna', thinkingLevels: ['off', 'low'] },
  ] }] });
  vi.spyOn(startup, 'onLocalStartup').mockResolvedValue(() => {});
  vi.spyOn(startup, 'localLaunchAction').mockImplementation(async (_root, _id, action) => {
    if (action === 'configuration') return { model: 'kota/auto', thinkingLevel: 'off', autoRoute: group() } as never;
    if (action === 'begin') return { reused: false, view: { projectRoot: root, agentId, attemptId: 'cold-start', revision: 1,
      state: 'running', selection: null, saved: false, error: null, steps: [] } } as never;
    return null as never;
  });
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

it('retains the editor and sends a debounced offline edit to the session started by the composer', async () => {
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  let snapshot: LocalWorkbenchView | null = null;
  vi.mocked(rpc.readLocalWorkbench).mockImplementation(async () => snapshot);
  vi.spyOn(rpc, 'setOfflineLocalModel').mockRejectedValue({ code: 'unexpected_offline_save' });
  vi.spyOn(rpc, 'readLocalAuth').mockResolvedValue({ provider: 'kota', status: 'ready', authType: null });
  const save = vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) => {
    if (request.type === 'get_state') return { model: snapshot?.model, thinkingLevel: 'off' } as never;
    if (request.type === 'get_available_models') return { models: [{ provider: 'kota', id: 'auto' }] } as never;
    if (request.type === 'get_available_thinking_levels') return { levels: ['off'] } as never;
    if (request.type === 'set_model') return { autoRoute: request.autoRoute } as never;
    return {} as never;
  });
  const p = props(), mounted = render(<LocalAgentWorkbench {...p} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start session' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Edit routing' }));
  const editor = screen.getByRole('dialog', { name: 'Auto String' });
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  fireEvent.change(screen.getByRole('textbox', { name: 'Model 2 intent' }), { target: { value: '' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Model 2 intent' }), { target: { value: 'Completed while offline.' } });
  const scope = { projectRoot: root, agentId, sessionId: 'composer-started-session', epoch: 1 };
  snapshot = { scope, state: 'ready', queued: 0, compacting: false, childPid: 42,
    model: { provider: 'kota', id: 'auto' }, thinkingLevel: 'off', error: null,
    revision: 1, runId: 0, startedAt: null, phase: 'Idle', statsRevision: 0,
    dropped: 0, queueTotal: 0, queueRevision: 0, blocks: [], queue: [] };
  // The existing visibility refresh reads the same snapshot a composer-started
  // session would publish, before the 250 ms editor debounce can fire.
  await act(async () => { fireEvent(document, new Event('visibilitychange')); });
  expect(screen.getByRole('dialog', { name: 'Auto String' })).toBe(editor);
  expect(screen.getByRole('textbox', { name: 'Model 2 intent' })).toHaveValue('Completed while offline.');
  expect(screen.getByRole('slider')).toBeEnabled();
  expect(save.mock.calls.filter(([, request]) => request.type === 'set_model')).toHaveLength(0);
  await act(async () => { await vi.advanceTimersByTimeAsync(250); });
  const savedGroup = group(); savedGroup.models[1].intent = 'Completed while offline.';
  expect(save).toHaveBeenCalledWith(scope, { type: 'set_model', provider: 'kota', modelId: 'auto', autoRoute: savedGroup });
  expect(rpc.setOfflineLocalModel).not.toHaveBeenCalled();

  // A later epoch change also preserves incomplete edits instead of remounting.
  fireEvent.change(screen.getByRole('textbox', { name: 'Model 1 intent' }), { target: { value: '' } });
  snapshot = { ...snapshot, scope: { ...scope, sessionId: 'restarted-session', epoch: 2 } };
  await act(async () => { fireEvent(document, new Event('visibilitychange')); });
  expect(screen.getByRole('dialog', { name: 'Auto String' })).toBe(editor);
  expect(screen.getByRole('textbox', { name: 'Model 1 intent' })).toHaveValue('');
  expect(screen.getByRole('textbox', { name: 'Model 2 intent' })).toHaveValue('Completed while offline.');
  expect(screen.getByRole('button', { name: 'OK' })).toBeDisabled();
  expect(p.onResume).not.toHaveBeenCalled();
  mounted.unmount();
});

it('lets an existing valid Auto String reach the backend launch gate without treating kota as a signed-out physical provider', async () => {
  const p = props(); render(<LocalAgentWorkbench {...p} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start session' })).toBeEnabled());
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Auto String');
  expect(screen.getByLabelText('Thinking')).toHaveDisplayValue('By Auto String');
  expect(screen.getByLabelText('Thinking')).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Sign in' })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Start session' }));
  await waitFor(() => expect(p.onResume).toHaveBeenCalledWith(agentId, { attemptId: 'cold-start' }));
  expect(startup.localLaunchAction).toHaveBeenCalledWith(root, agentId, 'begin', { selection: undefined });
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
});

it.each(['none', 'rejected'] as const)('keeps a saved Auto String selected with a %s key and uses the existing choose-model flow', async status => {
  ipc.settings = { schema: 1, provider: 'openrouter', ready: false, keys: [{ providerId: 'openrouter', status }] };
  const p = props(); render(<LocalAgentWorkbench {...p} />);
  await screen.findByText('Choose a model to continue.');
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Auto String');
  expect(screen.queryByRole('button', { name: 'Start session' })).not.toBeInTheDocument();
  expect(p.onResume).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
  expect(screen.getByRole('option', { name: /^Auto String/ })).toBeDisabled();
  expect(screen.getAllByRole('button', { name: 'Jev settings' }).length).toBeGreaterThan(0);
  expect(startup.localLaunchAction).not.toHaveBeenCalledWith(root, agentId, 'begin', expect.anything());
});

it('holds Start and New session until the offline group save completes, including the debounce window', async () => {
  let finish!: (value: Awaited<ReturnType<typeof rpc.setOfflineLocalModel>>) => void;
  const save = vi.spyOn(rpc, 'setOfflineLocalModel').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const p = props(); render(<LocalAgentWorkbench {...p} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start session' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Edit routing' }));
  fireEvent.change(screen.getByRole('slider'), { target: { value: '0.9' } });
  expect(screen.getByRole('button', { name: 'Start session' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'New session' })).toBeDisabled();
  expect(screen.getByRole('slider')).toBeEnabled();
  await waitFor(() => expect(save).toHaveBeenCalledWith(root, agentId, 'kota', 'auto', { ...group(), threshold: .9 }));
  await userEvent.click(screen.getByRole('button', { name: 'OK' }));
  expect(screen.getByRole('button', { name: 'Start session' })).toBeDisabled();
  expect(p.onResume).not.toHaveBeenCalled();
  await act(async () => finish({ provider: 'kota', modelId: 'auto', thinkingLevel: 'off', autoRoute: { ...group(), threshold: .9 } }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start session' })).toBeEnabled());
  expect(screen.getByRole('button', { name: 'New session' })).toBeEnabled();
});

it('keeps a retained group out of an ordinary model launch request', async () => {
  vi.mocked(auth.readLocalAuthAvailability).mockResolvedValue({ schema: 1, summary: 'ready', providers: [
    { providerId: 'openai', status: 'signed_in', authType: 'api_key' },
  ] });
  vi.mocked(auth.readLocalModelCatalog).mockResolvedValue({ schema: 1, providers: [{ providerId: 'openai', linked: true, models: [
    { modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'high'] },
    { modelId: 'luna', name: 'Luna', thinkingLevels: ['off', 'low'] },
  ] }] });
  const p = props(); render(<LocalAgentWorkbench {...p} />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Start session' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
  await userEvent.click(screen.getByRole('option', { name: /Luna OpenAI/ }));
  await userEvent.click(screen.getByRole('button', { name: 'Start session' }));
  await waitFor(() => expect(startup.localLaunchAction).toHaveBeenCalledWith(root, agentId, 'begin', { selection: {
    providerId: 'openai', modelId: 'luna', thinkingLevel: 'off',
  } }));
});
