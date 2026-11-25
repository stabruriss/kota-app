import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ComposerModelControl, type ComposerModelMemory } from '../src/chrome/ComposerModelControl';
import { useLocalWorkbenchLaunch } from '../src/hooks/useLocalWorkbenchLaunch';
import { InputBar, type InputBarProps } from '../src/chrome/InputBar';
import * as configuration from '../src/lib/local-configuration';
import * as rpc from '../src/lib/local-rpc';
import * as startup from '../src/lib/local-startup';
import * as auth from '../src/lib/local-auth';
import * as magpie from '../src/lib/local-magpie';
import type { AutoRouteGroup } from '../src/lib/string-routing';
import composerStyles from '../src/styles/composer-model.css?inline';

const ipc = vi.hoisted(() => ({ ready: true }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async command => {
  if (command === 'local_rpc_jev_read') return { schema: 1, provider: 'openrouter', ready: ipc.ready, keys: [] };
  throw Error(`Unexpected IPC: ${command}`);
}) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
const root = '/fixture/composer-model', id = 'agent-copper';
const group: AutoRouteGroup = { threshold: .5, defaultModel: 'model-1', models: [
  { id: 'model-1', providerId: 'fixture', modelId: 'base', thinkingLevel: 'high', intent: 'Complex work.' },
  { id: 'model-2', providerId: 'magpie', modelId: 'upstream/long-model', thinkingLevel: 'low', intent: 'Light work.' },
] };
let saved: { model: string; thinkingLevel: string; autoRoute: AutoRouteGroup }, memory: ComposerModelMemory;
let changed: (() => void) | undefined;
beforeEach(() => {
  ipc.ready = true; changed = undefined;
  saved = { model: 'kota/auto', thinkingLevel: 'off', autoRoute: structuredClone(group) };
  memory = { choices: new Map(), current: null };
  vi.spyOn(configuration, 'onLocalConfiguration').mockImplementation(async (_root, _id, callback) => {
    changed = () => callback({ projectRoot: root, agentId: id, revision: 'new' }); return () => { changed = undefined; };
  });
  vi.spyOn(startup, 'localLaunchAction').mockImplementation(async (_root, _id, action) => action === 'configuration' ? structuredClone(saved) as never : null as never);
  vi.spyOn(startup, 'onLocalStartup').mockResolvedValue(() => {});
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'ready', providers: [] });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [
    { providerId: 'fixture', linked: true, models: [{ modelId: 'base', name: 'Base', thinkingLevels: ['off', 'high'] }] },
    { providerId: 'magpie', linked: true, models: [{ modelId: 'upstream/long-model', name: 'Long model', thinkingLevels: ['off', 'low'] }] },
  ] });
  vi.spyOn(magpie, 'probeMagpie').mockResolvedValue({ settings: { mode: 'local', ip: '', port: 0 }, linked: false, keySaved: false, modelCount: 0, defaultKey: 'fixture' });
  vi.spyOn(rpc, 'localSnapshot').mockRejectedValue('offline');
  vi.spyOn(rpc, 'setOfflineLocalModel').mockImplementation(async (_root, _id, provider, modelId, autoRoute) => {
    saved = { model: `${provider}/${modelId}`, thinkingLevel: 'off', autoRoute: autoRoute ?? saved.autoRoute };
    changed?.(); return { provider, modelId, thinkingLevel: 'off', autoRoute: saved.autoRoute };
  });
  vi.spyOn(rpc, 'controlLocalWorkbench').mockResolvedValue({} as never);
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.restoreAllMocks(); });
function control(agentId = id, projectRoot = root) {
  return <ComposerModelControl key={`${projectRoot}:${agentId}`} projectRoot={projectRoot} agentId={agentId} name="Copper" memory={memory} />;
}
async function ready() {
  await waitFor(() => expect(screen.getByRole('button', { name: 'Auto String routing: Auto' })).toBeEnabled());
}
async function chooseOverride() {
  await userEvent.click(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'magpie / upstream/long-model • low' }));
}
function WorkbenchConfiguration() {
  const launch = useLocalWorkbenchLaunch({ projectRoot: root, agentId: id, connected: false, loaded: true, focusRequest: 1,
    onResume: vi.fn(), onStarted: vi.fn() });
  return <><output aria-label="Workbench model">{launch.saved?.provider}/{launch.saved?.id}</output>
    <button onClick={() => launch.selectModel('fixture', 'unsaved')}>Stage offline model</button></>;
}

it('shares one SHELL read/subscription with the workbench, adopts external saves, and drops an old offline draft', async () => {
  render(<>{control()}<WorkbenchConfiguration /></>); await ready();
  expect(screen.getByRole('button', { name: 'Local model' })).not.toHaveTextContent('•');
  expect(configuration.onLocalConfiguration).toHaveBeenCalledTimes(1);
  expect(vi.mocked(startup.localLaunchAction).mock.calls.filter(([, , action]) => action === 'configuration')).toHaveLength(1);
  await userEvent.click(screen.getByText('Stage offline model'));
  expect(screen.getByLabelText('Workbench model')).toHaveTextContent('fixture/unsaved');
  saved.model = 'fixture/base'; saved.thinkingLevel = 'high'; await act(async () => changed?.());
  await waitFor(() => expect(screen.getByLabelText('Workbench model')).toHaveTextContent('fixture/base'));
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Base • high');
  expect(screen.queryByRole('button', { name: 'Auto String routing: Auto' })).not.toBeInTheDocument();
  expect(vi.mocked(startup.localLaunchAction).mock.calls.filter(([, , action]) => action === 'configuration')).toHaveLength(2);
  saved.thinkingLevel = 'off'; await act(async () => changed?.());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Base • off'));
  expect(rpc.setOfflineLocalModel).not.toHaveBeenCalled();
  expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  expect(magpie.probeMagpie).not.toHaveBeenCalled();
});

it('does not let a historical startup snapshot undo the current configuration when the workbench opens', async () => {
  const view = render(control()); await ready();
  saved.model = 'fixture/base'; await act(async () => changed?.());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Base'));
  vi.mocked(startup.localLaunchAction).mockImplementation(async (_root, _id, action) => action === 'configuration' ? structuredClone(saved) as never
    : { projectRoot: root, agentId: id, attemptId: 'old', revision: 4, state: 'ready', saved: true,
      selection: { providerId: 'kota', modelId: 'auto', thinkingLevel: 'off', autoRoute: group }, steps: [] } as never);
  view.rerender(<>{control()}<WorkbenchConfiguration /></>);
  await waitFor(() => expect(screen.getByLabelText('Workbench model')).toHaveTextContent('fixture/base'));
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Base');
});

it('discards a late initial configuration read after an event starts a fresher read', async () => {
  let resolveOld!: (value: unknown) => void;
  vi.mocked(startup.localLaunchAction).mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve; }) as never);
  render(control()); await waitFor(() => expect(resolveOld).toBeTypeOf('function'));
  saved.model = 'fixture/base'; await act(async () => changed?.());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Base'));
  await act(async () => resolveOld({ model: 'kota/auto', thinkingLevel: 'off', autoRoute: group }));
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Base');
  expect(screen.queryByRole('button', { name: 'Auto String routing: Auto' })).not.toBeInTheDocument();
});

it('keeps manual choice across unmounts but isolates owners and clears a remapped group id', async () => {
  const view = render(control()); await ready(); await chooseOverride();
  expect(memory.current).toEqual({ projectRoot: root, agentId: id, routeModel: 'model-2' });
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Auto Off');
  expect(screen.getByRole('button', { name: 'Local model' })).not.toHaveTextContent('•');
  expect(screen.getByRole('button', { name: 'Manual model: long-model • low' })).not.toHaveTextContent('upstream/');
  expect(rpc.setOfflineLocalModel).not.toHaveBeenCalled(); expect(rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  view.rerender(control('agent-other')); await ready();
  expect(memory.current?.routeModel).toBeUndefined();
  view.rerender(control()); await screen.findByRole('button', { name: 'Manual model: long-model • low' });
  saved.autoRoute.models[1].modelId = 'replacement'; await act(async () => changed?.()); await ready();
  expect(memory.current?.routeModel).toBeUndefined(); expect(memory.choices.size).toBe(0);
});

it('opens a lazy portal on click or keyboard only, keeps it on pointer departure, and does not discover providers', async () => {
  const view = render(control()); await ready();
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  const trigger = screen.getByRole('button', { name: 'Auto String routing: Auto' });
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  fireEvent.pointerEnter(trigger); act(() => { vi.advanceTimersByTime(1000); });
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  vi.useRealTimers();
  fireEvent.keyDown(trigger, { key: 'ArrowDown' });
  const menu = screen.getByRole('menu'); expect(view.container.contains(menu)).toBe(false);
  expect(screen.getByRole('menuitemradio', { name: 'fixture / base • high' })).toHaveFocus();
  fireEvent.keyDown(menu, { key: 'End' }); expect(screen.getByRole('menuitemradio', { name: 'Auto' })).toHaveFocus();
  fireEvent.keyDown(menu, { key: 'Escape' }); expect(trigger).toHaveFocus();
  expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  fireEvent.click(trigger);
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  fireEvent.pointerLeave(trigger); fireEvent.pointerLeave(screen.getByRole('menu'));
  act(() => { vi.advanceTimersByTime(1000); }); expect(screen.getByRole('menu')).toBeInTheDocument();
  fireEvent.click(trigger); expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  vi.useRealTimers(); await chooseOverride();
  await userEvent.click(screen.getByRole('button', { name: 'Manual model: long-model • low' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Auto' }));
  expect(memory.current?.routeModel).toBeUndefined(); expect(memory.choices.size).toBe(0);
  expect(magpie.probeMagpie).not.toHaveBeenCalled();
});

it.each(['{Enter}', '[Space]'])('toggles with %s and keeps aria-expanded and Escape focus in sync', async key => {
  render(control()); await ready();
  const trigger = screen.getByRole('button', { name: 'Auto String routing: Auto' });
  expect(trigger).toHaveAttribute('aria-expanded', 'false');
  act(() => trigger.focus()); await userEvent.keyboard(key);
  expect(trigger).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByRole('menuitemradio', { name: 'fixture / base • high' })).toHaveFocus();
  act(() => trigger.focus()); await userEvent.keyboard(key);
  expect(trigger).toHaveAttribute('aria-expanded', 'false'); expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  await userEvent.keyboard(key); await userEvent.keyboard('{Escape}');
  expect(trigger).toHaveFocus(); expect(trigger).toHaveAttribute('aria-expanded', 'false');
});

it('closes on an outside click while delivering that click and focus to the original target', async () => {
  const clicked = vi.fn();
  render(<>{control()}<input aria-label="Composer draft" onClick={clicked} /></>); await ready();
  await userEvent.click(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
  const input = screen.getByRole('textbox', { name: 'Composer draft' });
  await userEvent.click(input);
  expect(screen.queryByRole('menu')).not.toBeInTheDocument(); expect(input).toHaveFocus(); expect(clicked).toHaveBeenCalledTimes(1);
});

it('keeps the fan open when an unrelated scroll region moves', async () => {
  render(<><div data-testid="chat-scroll">Messages</div>{control()}</>); await ready();
  const trigger = screen.getByRole('button', { name: 'Auto String routing: Auto' });
  await userEvent.click(trigger);
  fireEvent.scroll(screen.getByTestId('chat-scroll'));
  expect(screen.getByRole('menu')).toBeInTheDocument(); expect(trigger).toHaveAttribute('aria-expanded', 'true');
});

it('closes the fan when the window is resized', async () => {
  render(control()); await ready();
  const trigger = screen.getByRole('button', { name: 'Auto String routing: Auto' });
  await userEvent.click(trigger); fireEvent.resize(window);
  expect(screen.queryByRole('menu')).not.toBeInTheDocument(); expect(trigger).toHaveAttribute('aria-expanded', 'false');
});

it('keeps the menu mounted when WebKit clears button focus with a null relatedTarget', async () => {
  render(control()); await ready();
  await userEvent.click(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
  const first = screen.getByRole('menuitemradio', { name: 'fixture / base • high' });
  const blurred = vi.fn(); first.addEventListener('blur', blurred);
  act(() => first.blur());
  expect(blurred).toHaveBeenCalledOnce(); expect(blurred.mock.calls[0][0].relatedTarget).toBeNull();
  expect(document.body).toHaveFocus(); expect(screen.getByRole('menu')).toBeInTheDocument();
});

it('closes once when the trigger is pressed after WebKit clears menu focus', async () => {
  render(control()); await ready();
  const trigger = screen.getByRole('button', { name: 'Auto String routing: Auto' });
  await userEvent.click(trigger);
  const first = screen.getByRole('menuitemradio', { name: 'fixture / base • high' });
  fireEvent.pointerDown(trigger); fireEvent.mouseDown(trigger); act(() => first.blur());
  expect(screen.getByRole('menu')).toBeInTheDocument();
  fireEvent.mouseUp(trigger); fireEvent.click(trigger);
  expect(screen.queryByRole('menu')).not.toBeInTheDocument(); expect(trigger).toHaveAttribute('aria-expanded', 'false');
});

it.each([false, true])('closes on Tab, restores trigger focus, and leaves native navigation unblocked (reverse=%s)', async shift => {
  render(control()); await ready();
  const trigger = screen.getByRole('button', { name: 'Auto String routing: Auto' });
  await userEvent.click(trigger);
  // user-event tabs from its original event target, even after that node is removed.
  // Check native navigation in WebKit; here check the handoff without cancelling Tab.
  expect(fireEvent.keyDown(document.activeElement!, { key: 'Tab', code: 'Tab', shiftKey: shift })).toBe(true);
  expect(screen.queryByRole('menu')).not.toBeInTheDocument(); expect(trigger).toHaveAttribute('aria-expanded', 'false');
  expect(trigger).toHaveFocus();
});

it('keeps the Auto pill visible on hover and keyboard focus, selects it, and permits Tab to leave', async () => {
  const style = document.createElement('style'); style.textContent = composerStyles; document.head.appendChild(style);
  try {
    render(control()); await ready(); await chooseOverride();
    await userEvent.click(screen.getByRole('button', { name: 'Manual model: long-model • low' }));
    const auto = screen.getByRole('menuitemradio', { name: 'Auto' }), pill = auto.querySelector('.composer-fan-short')!;
    fireEvent.pointerEnter(auto); expect(pill).toBeVisible();
    await userEvent.keyboard('{End}'); expect(auto).toHaveFocus(); expect(pill).toBeVisible();
    await userEvent.click(pill);
    expect(screen.queryByRole('menu')).not.toBeInTheDocument(); expect(memory.current?.routeModel).toBeUndefined();
    await userEvent.click(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
    await userEvent.keyboard('{End}'); await userEvent.tab();
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  } finally { style.remove(); }
});

it('keeps another item’s expanded label when WebKit clears focus and selects by clicking that label', async () => {
  render(control()); await ready();
  await userEvent.click(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
  const first = screen.getByRole('menuitemradio', { name: 'fixture / base • high' });
  const next = screen.getByRole('menuitemradio', { name: 'magpie / upstream/long-model • low' });
  expect(first).toHaveFocus(); fireEvent.pointerEnter(next);
  const full = screen.getByText('upstream/long-model • low', { selector: '.composer-fan-full' });
  fireEvent.pointerDown(full); act(() => first.blur()); expect(document.body).toHaveFocus();
  // The old focused item's blur must not detach the pointer's click target.
  expect(full).toBeInTheDocument(); fireEvent.click(full);
  expect(memory.current?.routeModel).toBe('model-2'); expect(screen.queryByRole('menu')).not.toBeInTheDocument();
});

it.each([0, 1])('keeps the pointed full label when its own item loses focus (item=%s)', async index => {
  render(control()); await ready();
  await userEvent.click(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
  if (index) await userEvent.keyboard('{ArrowDown}');
  const item = screen.getAllByRole('menuitemradio')[index];
  expect(item).toHaveFocus(); fireEvent.pointerEnter(item);
  const full = item.querySelector('.composer-fan-full')!;
  fireEvent.pointerDown(full); act(() => item.blur());
  expect(document.body).toHaveFocus(); expect(full).toBeInTheDocument();
  fireEvent.click(full);
  expect(memory.current?.routeModel).toBe(group.models[index].id); expect(screen.queryByRole('menu')).not.toBeInTheDocument();
});

it.each([false, true])('permanently selects a normal model through the %s online path and clears the override', async online => {
  if (online) {
    const scope = { projectRoot: root, agentId: id, sessionId: 'current', epoch: 2 };
    vi.mocked(rpc.localSnapshot).mockResolvedValue({ scope, state: 'ready', model: null, thinkingLevel: null, queued: 0, compacting: false, childPid: 1, error: null });
    vi.mocked(rpc.controlLocalWorkbench).mockImplementation(async (_scope, request) => {
      saved.model = `${request.provider}/${request.modelId}`; changed?.(); return {} as never;
    });
  }
  render(control()); await ready(); await chooseOverride();
  await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
  await userEvent.click(screen.getByRole('option', { name: 'Base fixture' }));
  await waitFor(() => expect(saved.model).toBe('fixture/base'));
  await waitFor(() => expect(memory.current?.routeModel).toBeUndefined());
  expect(online ? rpc.controlLocalWorkbench : rpc.setOfflineLocalModel).toHaveBeenCalledTimes(1);
  expect(online ? rpc.setOfflineLocalModel : rpc.controlLocalWorkbench).not.toHaveBeenCalled();
  expect(magpie.probeMagpie).toHaveBeenCalledTimes(1);
});

it('leaves the selection intact after a failed permanent change and disables Auto String without Jev', async () => {
  ipc.ready = false;
  vi.mocked(rpc.setOfflineLocalModel).mockRejectedValue({ code: 'local_model_update_failed' });
  render(control());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Local model' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
  expect(screen.getByRole('option', { name: /Auto String/ })).toBeDisabled();
  await userEvent.click(screen.getByRole('option', { name: 'Base fixture' }));
  await waitFor(() => expect(rpc.setOfflineLocalModel).toHaveBeenCalledTimes(1));
  expect(screen.getByRole('button', { name: 'Local model' })).toHaveTextContent('Auto String');
  expect(screen.getByRole('button', { name: 'Auto String routing: Auto' })).toBeDisabled();
});

it('retains the original toolbar tools and snapshots the override only for a direct composer send', async () => {
  let selected: string | undefined = 'model-2';
  const onSend = vi.fn<NonNullable<InputBarProps['onSend']>>(async () => {}), whiteboard = vi.fn();
  const props = { value: '', onChange: vi.fn(), onSend, onWhiteboardOpen: whiteboard, targetAgent: id,
    agentMeta: { [id]: { name: 'Copper', emoji: '', hue: '', role: '' } }, getRouteModel: () => selected };
  const view = render(<InputBar {...props} />);
  const tools = screen.getByTestId('ib-whiteboard-tool').parentElement!, original = tools.outerHTML;
  view.rerender(<InputBar {...props} modelControls={<span>Models</span>} />);
  expect(tools.outerHTML).toBe(original); expect(tools.parentElement?.previousElementSibling).toHaveTextContent('Models');
  await userEvent.click(screen.getByTestId('ib-whiteboard-tool')); expect(whiteboard).toHaveBeenCalledTimes(1);
  await userEvent.type(screen.getByTestId('input-field'), 'First'); await userEvent.click(screen.getByTestId('ib-send'));
  selected = undefined; expect(onSend.mock.calls[0][2]).toMatchObject({ routeModel: 'model-2' });
  await userEvent.type(screen.getByTestId('input-field'), 'Auto'); await userEvent.click(screen.getByTestId('ib-send'));
  expect(onSend.mock.calls[1][2]).not.toHaveProperty('routeModel');
  view.rerender(<InputBar {...props} broadcastMode broadcastRecipientCount={1} />); selected = 'model-1';
  await userEvent.type(screen.getByTestId('input-field'), 'Broadcast'); await userEvent.click(screen.getByTestId('ib-send'));
  expect(onSend.mock.calls[2][2]).not.toHaveProperty('routeModel');
});
