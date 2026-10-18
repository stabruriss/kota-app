import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DanceSettingsEditor, PuppeteerDanceCard } from '../src/chrome/PuppeteerDanceCard';
import { AgentRibbon } from '../src/chrome/AgentRibbon';
import { Stage, type StageProps } from '../src/chrome/Stage';
import { danceSettingsIssue, useDanceSettings } from '../src/lib/dance-settings';
import * as api from '../src/lib/puppeteer-dance';
import * as jev from '../src/lib/jev';
import * as auth from '../src/lib/local-auth';
import * as events from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import '../src/styles/string-routing.css';
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn() }));
const agents = [{ id: 'agent-a', name: 'Ada', avatarId: 'codex' }, { id: 'agent-b', name: 'Babbage', avatarId: 'claude' }, { id: 'agent-c', name: 'Copper', avatarId: 'pi' }];
const settings = (patch: Partial<api.DanceSettings> = {}): api.DanceSettings => ({ schema: 1, enabled: false, sentinel: agents[0].id, strategy: 'context', threshold: .75, conditions: {}, ...patch });
const snapshot = (value = settings(), sourceRevision = 'read-1'): api.DanceSettingsSnapshot => ({ settings: value, sourceRevision, currentSentinel: value.enabled ? value.sentinel : null });
beforeEach(() => { vi.spyOn(jev, 'readJevSettings').mockResolvedValue({ schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] });
  vi.mocked(invoke).mockImplementation(async () => await jev.readJevSettings() as never);
});
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });
it('validates all nineteen threshold steps for both Dance strategies and rejects out-of-range or off-grid values', () => {
  for (const strategy of ['context', 'rule'] as const) {
    const value = settings({ strategy, conditions: { 'agent-a': 'Build', 'agent-b': 'Review' } });
    for (let step = 1; step <= 19; step++) expect(danceSettingsIssue({ ...value, threshold: step / 20 }, agents)).toBeNull();
    for (const threshold of [-.05, 0, .025, .049, .051, .49, .96, 1, NaN, Infinity, -Infinity]) {
      expect(danceSettingsIssue({ ...value, threshold }, agents)).toBe('Choose a valid probability threshold.');
    }
  }
});
it.each(['context', 'rule'] as const)('stages and saves 0.05 with the widened slider fill in %s mode', async strategy => {
  const initial = settings({ strategy, threshold: .85, conditions: { 'agent-a': 'Build', 'agent-b': 'Review' } });
  const save = vi.fn(async (draft: api.DanceSettings) => draft), close = vi.fn();
  render(<DanceSettingsEditor settings={initial} agents={agents} owner="Project A" busy={false} error={null} onSave={save} onClose={close} />);
  await screen.findByText('Verified');
  const slider = screen.getByRole('slider', { name: 'Dance probability threshold' });
  expect(slider).toHaveAttribute('min', '0.05'); expect(slider).toHaveAttribute('max', '0.95'); expect(slider).toHaveAttribute('step', '0.05');
  expect(slider).toHaveValue('0.85');
  expect(parseFloat(slider.style.getPropertyValue('--string-threshold-fill'))).toBeCloseTo(800 / 9);
  for (const [threshold, fill] of [[.95, 100], [.5, 50], [.05, 0]]) {
    fireEvent.change(slider, { target: { value: String(threshold) } });
    expect(slider).toHaveValue(String(threshold));
    expect(parseFloat(slider.style.getPropertyValue('--string-threshold-fill'))).toBeCloseTo(fill);
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
  }
  expect(save).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(save).toHaveBeenCalledExactlyOnceWith({ ...initial, threshold: .05 }); expect(close).toHaveBeenCalledOnce();
});
it('stages mode/threshold, puts threshold before segmented mode and needs two nonempty rules to Save', async () => {
  const save = vi.fn(async (draft: api.DanceSettings) => draft); const close = vi.fn();
  render(<DanceSettingsEditor settings={settings()} agents={agents} owner="Project A" busy={false} error={null} onSave={save} onClose={close} />);
  const slider = screen.getByRole('slider', { name: 'Dance probability threshold' });
  expect(slider.compareDocumentPosition(screen.getByRole('group', { name: 'Dance mode' })) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.getByText('No changes.')).toHaveAttribute('role', 'status');
  expect(screen.getByRole('button', { name: 'OK' })).toBeEnabled();
  await userEvent.click(screen.getByRole('button', { name: 'Rule based' }));
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(screen.getByText('Add routing conditions for at least 2 agents.')).toHaveAttribute('role', 'status');
  fireEvent.change(screen.getByRole('textbox', { name: 'Routing condition for Ada' }), { target: { value: 'Build reliable code' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Routing condition for Babbage' }), { target: { value: '   ' } });
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.change(screen.getByRole('textbox', { name: 'Routing condition for Babbage' }), { target: { value: 'Review the change' } });
  fireEvent.change(slider, { target: { value: '0.8' } });
  expect(save).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(save).toHaveBeenCalledWith(settings({ strategy: 'rule', threshold: .8, conditions: { 'agent-a': 'Build reliable code', 'agent-b': 'Review the change' } }));
  expect(close).toHaveBeenCalledOnce();
});
it('retains the dialog/draft on save failure and caps Unicode conditions at 200 characters', async () => {
  const close = vi.fn();
  render(<DanceSettingsEditor settings={settings({ strategy: 'rule', conditions: { 'agent-a': 'Code', 'agent-b': 'Review' } })} agents={agents} owner="Project A" busy={false} error="Dance unavailable. Try again." onSave={vi.fn(async () => null)} onClose={close} />);
  fireEvent.change(screen.getByRole('textbox', { name: 'Routing condition for Ada' }), { target: { value: '😀'.repeat(210) } });
  expect(Array.from((screen.getByRole('textbox', { name: 'Routing condition for Ada' }) as HTMLTextAreaElement).value)).toHaveLength(200);
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(close).not.toHaveBeenCalled();
});
it.each([
  ['context', settings()],
  ['rule', settings({ strategy: 'rule', conditions: { 'agent-a': 'Code', 'agent-b': 'Review' } })],
] as const)('offers OK that just closes when %s settings have nothing to save, and Save once edited', async (_strategy, value) => {
  const save = vi.fn(async (draft: api.DanceSettings) => draft); const close = vi.fn();
  const view = render(<DanceSettingsEditor settings={value} agents={agents} owner="A" busy={false} error={null} onSave={save} onClose={close} />);
  await screen.findByText('Verified');
  expect(screen.getByText('No changes.')).toHaveAttribute('role', 'status');
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  fireEvent.change(screen.getByRole('slider'), { target: { value: '0.8' } });
  expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled(); expect(screen.getByText('Unsaved changes')).toBeInTheDocument();
  fireEvent.change(screen.getByRole('slider'), { target: { value: String(value.threshold) } });
  await userEvent.click(screen.getByRole('button', { name: 'OK' }));
  expect(close).toHaveBeenCalledOnce(); expect(save).not.toHaveBeenCalled();
  view.rerender(<DanceSettingsEditor settings={value} agents={agents} owner="A" busy error={null} onSave={save} onClose={close} />);
  expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();
});
it('still offers OK when unchanged settings are invalid, keeping the issue visible', async () => {
  const save = vi.fn(); const close = vi.fn();
  render(<DanceSettingsEditor settings={settings({ strategy: 'rule', conditions: { 'agent-a': 'Code' } })} agents={agents} owner="A" busy={false} error={null} onSave={save} onClose={close} />);
  await screen.findByText('Verified');
  expect(screen.getByText('Add routing conditions for at least 2 agents.')).toHaveAttribute('role', 'status');
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'OK' }));
  expect(close).toHaveBeenCalledOnce(); expect(save).not.toHaveBeenCalled();
});
it('shows the runtime sentinel read-only in settings, explains it is automatic and never stages it', async () => {
  const save = vi.fn(async (draft: api.DanceSettings) => draft);
  const view = render(<DanceSettingsEditor settings={settings({ strategy: 'rule', conditions: { 'agent-a': 'Code', 'agent-b': 'Review' } })} agents={agents} owner="A" busy={false} error={null} onSave={save} onClose={vi.fn()} sentinel={agents[1]} />);
  const readout = screen.getByRole('status', { name: 'Current sentinel' });
  expect(readout).toHaveTextContent('Babbage'); expect(readout.querySelector('.tavern-avatar-art')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Sentinel' })).not.toBeInTheDocument(); expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  expect(within(readout).getByText('Auto')).toBeInTheDocument();
  expect(screen.getByText('If the recipient cannot be decided, the sentinel receives the message.')).toBeInTheDocument();
  // An empty condition is simply not matched; no agent is singled out as the fallback.
  expect(screen.getByText('Not matched.', { exact: true })).toBeInTheDocument(); expect(screen.queryByText('Still the sentinel for fallback.')).not.toBeInTheDocument();
  view.rerender(<DanceSettingsEditor key="fresh" settings={settings({ sentinel: null })} agents={agents} owner="A" busy={false} error={null} onSave={save} onClose={vi.fn()} />);
  expect(screen.getByRole('status', { name: 'Current sentinel' })).toHaveTextContent('Follows the last recipient');
  fireEvent.change(screen.getByRole('slider'), { target: { value: '0.8' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(save).toHaveBeenCalledWith(settings({ sentinel: null, threshold: .8 }));
});
it('switches a project directly without confirmation, and allows Off even when the shared Jev key is rejected', async () => {
  vi.spyOn(jev, 'readJevSettings').mockResolvedValue({ schema: 1, provider: 'openrouter', ready: false, keys: [{ providerId: 'openrouter', status: 'rejected' }] });
  const save = vi.spyOn(api, 'saveDance').mockImplementation(async (_root, next) => snapshot(next, 'saved-2'));
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot(settings({ enabled: true })));
  function Host() { const controller = useDanceSettings('/project-a'); return <PuppeteerDanceCard owner="Project A" controller={controller} agents={agents} />; }
  render(<Host />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Turn Dance off' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance off' }));
  expect(save).toHaveBeenCalledWith('/project-a', settings({ enabled: false }), 'read-1');
  // Missing prerequisites now explain themselves on Start instead of disabling it.
  await waitFor(() => expect(screen.getByRole('button', { name: 'Turn Dance on' })).toBeEnabled());
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});
it('never accepts a late settings response or another project’s changed event into the current project', async () => {
  let finish!: (value: api.DanceSettingsSnapshot) => void;
  vi.spyOn(api, 'readDance').mockImplementation(root => root === '/a' ? new Promise(done => { finish = done; }) : Promise.resolve(snapshot()));
  let listener!: (event: events.Event<api.DanceSettingsSnapshot & { projectRoot: string }>) => void;
  vi.mocked(events.listen).mockImplementation(async (_name, next) => { listener = next as typeof listener; return () => {}; });
  function Host({ root }: { root: string }) { const controller = useDanceSettings(root); return <span>{controller.settings?.enabled ? 'On' : controller.loading ? 'Loading' : 'Off'}</span>; }
  const view = render(<Host root="/a" />); await waitFor(() => expect(api.readDance).toHaveBeenCalledWith('/a'));
  view.rerender(<Host root="/b" />); await screen.findByText('Off');
  await act(async () => { finish(snapshot(settings({ enabled: true }))); listener({ event: '', id: 1, payload: { projectRoot: '/a', ...snapshot(settings({ enabled: true })) } }); });
  expect(screen.getByText('Off')).toBeInTheDocument();
});
it('keeps filter/Add/table-full outside Dance target actions, while double click and right click cancel a pending single click', async () => {
  vi.useFakeTimers(); const open = vi.fn(), double = vi.fn(), menu = vi.fn(), add = vi.fn();
  const props = { onTable: agents.map(agent => ({ id: agent.id, state: 'idle' as const })), offTable: [], targetAgent: 'agent-a', danceActive: true,
    shortcutOrder: agents.map(agent => agent.id), agentMeta: Object.fromEntries(agents.map(agent => [agent.id, { name: agent.name, emoji: '', role: '', hue: '', avatarId: agent.avatarId }])), onOpenAgent: open, onDblClickAgent: double, onAgentContextMenu: menu, onAddAgent: add };
  const view = render(<AgentRibbon {...props} />);
  expect(screen.getByTestId('agent-ribbon')).toHaveClass('dance-active');
  const mask = view.container.querySelector('.ribbon-dance-label')!;
  expect(mask).toHaveTextContent(/^PUPPETEER DANCING$/); expect(mask.querySelector('.tavern-avatar-art')).not.toBeInTheDocument();
  fireEvent.click(screen.getByTestId('ribbon-filter-clear'));
  fireEvent.click(screen.getByTestId('ribbon-add')); await act(async () => { vi.advanceTimersByTime(300); });
  expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTestId('chip-agent-b')); fireEvent.click(screen.getByTestId('chip-agent-b')); fireEvent.doubleClick(screen.getByTestId('chip-agent-b'));
  await act(async () => { vi.advanceTimersByTime(300); }); expect(double).toHaveBeenCalledWith('agent-b'); expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTestId('chip-agent-b')); fireEvent.contextMenu(screen.getByTestId('chip-agent-b'));
  await act(async () => { vi.advanceTimersByTime(300); }); expect(menu).toHaveBeenCalled(); expect(open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTestId('chip-agent-c')); await act(async () => { vi.advanceTimersByTime(300); }); expect(open).toHaveBeenCalledExactlyOnceWith('agent-c');
  view.rerender(<AgentRibbon {...props} tableFull />); fireEvent.click(screen.getByTestId('ribbon-table-full'));
  await act(async () => { vi.advanceTimersByTime(300); }); expect(open).toHaveBeenCalledOnce();
});
it('holds a Dance desktop seat’s single click for 220 ms, while double/right clicks and non-Dance clicks keep their behavior', async () => {
  vi.useFakeTimers(); const open = vi.fn(), double = vi.fn(), menu = vi.fn();
  const props: StageProps = { sceneKey: 'conversation', liveAgents: new Set(), targetAgent: null, danceActive: true,
    tableSlots: agents.map(agent => agent.id), onOpenAgent: open, onDblClickAgent: double, onAgentContextMenu: menu,
    centerpiece: 'fire', roomColor: '#2b2c2f', deskColor: '#6d5241', roomTheme: 'classic', deskTheme: 'warm',
    onChangeCenter: vi.fn(), onChangeRoom: vi.fn(), onChangeDesk: vi.fn(), onChangeRoomTheme: vi.fn(), onChangeDeskTheme: vi.fn(),
    agentMeta: Object.fromEntries(agents.map(agent => [agent.id, { name: agent.name, emoji: '', role: '', hue: '' }])),
  };
  const view = render(<Stage {...props} />);
  const seat = view.container.querySelector('.seat-ring [data-seat-index="0"]')!;
  fireEvent.click(seat); await act(async () => { vi.advanceTimersByTime(219); }); expect(open).not.toHaveBeenCalled();
  await act(async () => { vi.advanceTimersByTime(1); }); expect(open).toHaveBeenCalledExactlyOnceWith('agent-a'); open.mockClear();
  fireEvent.click(seat); fireEvent.click(seat); fireEvent.doubleClick(seat);
  await act(async () => { vi.advanceTimersByTime(300); }); expect(double).toHaveBeenCalledExactlyOnceWith('agent-a'); expect(open).not.toHaveBeenCalled();
  fireEvent.click(seat); fireEvent.contextMenu(seat);
  await act(async () => { vi.advanceTimersByTime(300); }); expect(menu).toHaveBeenCalledWith('agent-a', expect.anything(), 'table-card'); expect(open).not.toHaveBeenCalled();
  view.rerender(<Stage {...props} danceActive={false} />); fireEvent.click(seat); expect(open).toHaveBeenCalledExactlyOnceWith('agent-a');
});

it.each([false, true])('queues an exit behind an earlier settings save, including when it fails (failure=%s)', async failure => {
  let controller!: ReturnType<typeof useDanceSettings>; let finish!: (next: api.DanceSettingsSnapshot) => void; let fail!: (error: Error) => void;
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot(settings({ enabled: true })));
  const save = vi.spyOn(api, 'saveDance').mockImplementationOnce(() => new Promise((resolve, reject) => { finish = resolve; fail = reject; }))
    .mockImplementation(async (_root, next) => snapshot(next, 'saved-2'));
  function Host() { controller = useDanceSettings('/a'); return <span>{controller.busy ? 'Busy' : 'Idle'}</span>; }
  render(<Host />); await waitFor(() => expect(controller.settings).not.toBeNull());
  let first!: ReturnType<typeof controller.save>; let second!: ReturnType<typeof controller.save>;
  act(() => { first = controller.save(settings({ enabled: true, sentinel: 'agent-b', threshold: .8 })); second = controller.save(current => ({ ...current, enabled: false })); });
  await waitFor(() => expect(save).toHaveBeenCalledOnce()); expect(screen.getByText('Busy')).toBeInTheDocument();
  await act(async () => { if (failure) fail(new Error('fixture save failed')); else finish(snapshot(save.mock.calls[0][1], 'saved-2')); await first; await second; });
  expect(save).toHaveBeenCalledTimes(2);
  expect(save.mock.calls[1][1]).toEqual(settings({ enabled: false, sentinel: failure ? 'agent-a' : 'agent-b', threshold: failure ? .75 : .8 }));
  expect(save.mock.calls[1][2]).toBe(failure ? 'read-1' : 'saved-2');
  expect(controller.settings?.enabled).toBe(false); expect(screen.getByText('Idle')).toBeInTheDocument();
});

it('does not execute an old project’s queued exit or accept its late save into a new project', async () => {
  let controller!: ReturnType<typeof useDanceSettings>; let finish!: (next: api.DanceSettingsSnapshot) => void;
  vi.spyOn(api, 'readDance').mockImplementation(async root => snapshot(settings({ enabled: root === '/a' })));
  const save = vi.spyOn(api, 'saveDance').mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  function Host({ root }: { root: string }) { controller = useDanceSettings(root); return <span>{controller.settings?.enabled ? 'On' : 'Off'}</span>; }
  const view = render(<Host root="/a" />); await screen.findByText('On');
  let first!: ReturnType<typeof controller.save>; let exit!: ReturnType<typeof controller.save>;
  act(() => { first = controller.save(settings({ enabled: true, sentinel: 'agent-b' })); exit = controller.save(current => ({ ...current, enabled: false })); });
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  view.rerender(<Host root="/b" />); await waitFor(() => expect(api.readDance).toHaveBeenCalledWith('/b'));
  await act(async () => { finish(snapshot(save.mock.calls[0][1], 'saved-2')); expect(await first).toBeNull(); expect(await exit).toBeNull(); });
  expect(save).toHaveBeenCalledOnce(); expect(controller.settings?.sentinel).toBe('agent-a'); expect(controller.settings?.enabled).toBe(false);
});
it('follows sentinel events on the card, and a late read does not roll a pushed sentinel back', async () => {
  let finish!: (value: api.DanceSettingsSnapshot) => void;
  const read = vi.spyOn(api, 'readDance').mockResolvedValueOnce(snapshot(settings({ enabled: true })));
  const listeners = new Map<string, (event: events.Event<unknown>) => void>();
  vi.mocked(events.listen).mockImplementation(async (name, next) => { listeners.set(name, next as (event: events.Event<unknown>) => void); return () => {}; });
  const push = (projectRoot: string, currentSentinel: string | null) => act(async () => listeners.get('puppeteer-dance://sentinel')!({ event: '', id: 1, payload: { projectRoot, currentSentinel } }));
  render(<CardHost />);
  const readout = await screen.findByRole('status', { name: 'Current sentinel' });
  await waitFor(() => expect(readout).toHaveTextContent('Ada'));
  expect(within(readout).getByText('Sentinel').nextElementSibling).toHaveClass('dance-agent-avatar');
  await push('/project-b', 'agent-c'); expect(readout).toHaveTextContent('Ada');
  await push('/project-a', 'agent-b'); expect(readout).toHaveTextContent('Babbage');
  // A settings read started before the next push answers with the older sentinel afterwards.
  read.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  await userEvent.click(screen.getByRole('button', { name: 'Dance settings · Project A' }));
  await push('/project-a', 'agent-c');
  await act(async () => finish({ ...snapshot(settings({ enabled: true, threshold: .9 }), 'read-2'), currentSentinel: 'agent-b' }));
  const editor = await screen.findByRole('dialog', { name: 'Puppeteer Setting' });
  expect(within(editor).getByRole('status', { name: 'Current sentinel' })).toHaveTextContent('Copper');
  expect(screen.getByRole('region', { name: 'Puppeteer Dance · Project A' }).querySelector('.dance-sentinel-compact')).toHaveTextContent('Copper');
  await push('/project-a', 'agent-gone'); expect(within(editor).getByRole('status', { name: 'Current sentinel' })).toHaveTextContent('Follows the last recipient');
});

function CardHost({ seedTarget, seats = agents }: { seedTarget?: string | null; seats?: typeof agents }) {
  const controller = useDanceSettings('/project-a');
  return <PuppeteerDanceCard owner="Project A" controller={controller} agents={seats} seedTarget={seedTarget} />;
}
it('shows the Sentinel wordmark read-only, seeds the lowest seat without a composer target and lights it only after a confirmed enable', async () => {
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot(settings({ sentinel: null })));
  let finish!: (value: api.DanceSettingsSnapshot) => void;
  const save = vi.spyOn(api, 'saveDance').mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }))
    .mockImplementation(async (_root, next) => snapshot(next, 'saved-off'));
  render(<CardHost />);
  const card = screen.getByRole('region', { name: 'Puppeteer Dance · Project A' });
  const readout = screen.getByRole('status', { name: 'Current sentinel' });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Turn Dance on' })).toBeEnabled());
  expect(screen.queryByRole('button', { name: 'Sentinel' })).not.toBeInTheDocument();
  const label = within(readout).getByText('Sentinel');
  expect(label).toHaveAttribute('title', "The sentinel follows whoever received your last message. If the recipient can't be decided, the sentinel receives the message.");
  expect(readout).toHaveTextContent('Follows the last recipient'); expect(within(readout).queryByText('Auto')).not.toBeInTheDocument();
  expect(card).toHaveAttribute('data-dance-enabled', 'false');
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  await waitFor(() => expect(save).toHaveBeenCalledOnce());
  expect(save).toHaveBeenCalledWith('/project-a', settings({ enabled: true, sentinel: 'agent-a' }), 'read-1');
  expect(card).toHaveAttribute('data-dance-enabled', 'false');
  await act(async () => finish(snapshot(settings({ enabled: true }), 'saved-on')));
  expect(card).toHaveAttribute('data-dance-enabled', 'true'); expect(readout).toHaveTextContent('Ada');
  expect(label.nextElementSibling).toHaveClass('dance-agent-avatar');
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance off' }));
  await waitFor(() => expect(card).toHaveAttribute('data-dance-enabled', 'false'));
  expect(readout).toHaveTextContent('Follows the last recipient');
});
it('does not light the Sentinel wordmark when enabling Dance fails', async () => {
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot());
  vi.spyOn(api, 'saveDance').mockRejectedValue({ code: 'dance_settings_conflict' });
  render(<CardHost />);
  await waitFor(() => expect(screen.getByRole('button', { name: 'Turn Dance on' })).toBeEnabled());
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  await screen.findByRole('dialog', { name: 'Before you start Dance' });
  expect(screen.getByRole('region', { name: 'Puppeteer Dance · Project A' })).toHaveAttribute('data-dance-enabled', 'false');
});
it.each([['agent-c', 'agent-c'], [null, 'agent-a'], ['agent-gone', 'agent-a']] as const)('starts with composer target %s as the seed sentinel (seeded: %s)', async (seedTarget, seeded) => {
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot(settings({ sentinel: 'agent-b' })));
  const save = vi.spyOn(api, 'saveDance').mockImplementation(async (_root, next) => snapshot(next, 'saved'));
  render(<CardHost seedTarget={seedTarget} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Turn Dance on' }));
  await screen.findByRole('button', { name: 'Turn Dance off' });
  expect(save).toHaveBeenCalledExactlyOnceWith('/project-a', settings({ enabled: true, sentinel: seeded }), 'read-1');
});
it('explains an empty table on Start with a single OK and never saves', async () => {
  const read = vi.spyOn(api, 'readDance').mockResolvedValue(snapshot(settings({ sentinel: null })));
  const save = vi.spyOn(api, 'saveDance');
  render(<CardHost seats={[]} />);
  await waitFor(() => expect(read).toHaveBeenCalledOnce());
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  const dialog = await screen.findByRole('dialog', { name: 'Before you start Dance' });
  expect(within(dialog).getByRole('status')).toHaveTextContent('Seat at least one agent at the table before starting Dance.');
  expect(within(dialog).queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  await userEvent.click(within(dialog).getByRole('button', { name: 'OK' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument(); expect(save).not.toHaveBeenCalled(); expect(read).toHaveBeenCalledTimes(2);
});
it('maps a seed that left before the enabling save to the empty-table explanation', async () => {
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot());
  vi.spyOn(api, 'saveDance').mockRejectedValue({ code: 'dance_sentinel_required' });
  render(<CardHost />);
  await userEvent.click(await screen.findByRole('button', { name: 'Turn Dance on' }));
  const dialog = await screen.findByRole('dialog', { name: 'Before you start Dance' });
  expect(within(dialog).getByRole('status')).toHaveTextContent('Seat at least one agent at the table before starting Dance.');
});
it.each(['none', 'rejected'] as const)('checks fresh Jev %s on Start and closes the check before requesting its settings', async status => {
  vi.spyOn(api, 'readDance').mockResolvedValue(snapshot()); const save = vi.spyOn(api, 'saveDance');
  const navigate = vi.spyOn(auth, 'requestKotaLocalLink').mockImplementation(() => {
    expect(screen.queryByRole('dialog', { name: 'Before you start Dance' })).not.toBeInTheDocument();
  });
  render(<CardHost />); await waitFor(() => expect(jev.readJevSettings).toHaveBeenCalled());
  // The card previously saw Ready; Start must re-read instead of trusting it.
  vi.mocked(jev.readJevSettings).mockResolvedValue({ schema: 1, provider: 'openrouter', ready: false, keys: [{ providerId: 'openrouter', status }] });
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  const dialog = await screen.findByRole('dialog', { name: 'Before you start Dance' });
  expect(within(dialog).getByRole('status')).toHaveTextContent(status === 'rejected' ? 'Jev key rejected.' : 'Set up Jev');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Set up Jev' }));
  expect(navigate).toHaveBeenCalledExactlyOnceWith('puppeteer-jev'); expect(save).not.toHaveBeenCalled();
});
it('re-reads rules on Start, counts only seated agents and sends an incomplete set to the editor', async () => {
  const read = vi.spyOn(api, 'readDance').mockResolvedValue(snapshot()); const save = vi.spyOn(api, 'saveDance');
  render(<CardHost />); await waitFor(() => expect(read).toHaveBeenCalledOnce());
  read.mockResolvedValue(snapshot(settings({ strategy: 'rule', conditions: { 'agent-a': 'Code', 'agent-gone': 'Review' } }), 'external'));
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  await screen.findByText('Add routing conditions for at least 2 agents.');
  await userEvent.click(screen.getByRole('button', { name: 'Edit rules' }));
  await screen.findByRole('dialog', { name: 'Puppeteer Setting' });
  expect(screen.getByRole('textbox', { name: 'Routing condition for Ada' })).toHaveValue('Code');
  expect(save).not.toHaveBeenCalled();
});
it('unlocks a failed settings read and Retry runs fresh checks before enabling Dance', async () => {
  const read = vi.spyOn(api, 'readDance').mockRejectedValue({ code: 'dance_settings_read_failed' });
  const save = vi.spyOn(api, 'saveDance').mockImplementation(async (_root, next) => snapshot(next, 'saved'));
  render(<CardHost />); await waitFor(() => expect(read).toHaveBeenCalledOnce());
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  await screen.findByRole('dialog', { name: 'Before you start Dance' });
  expect(screen.getByRole('button', { name: 'Turn Dance on' })).toBeEnabled();
  read.mockResolvedValue(snapshot(settings(), 'recovered'));
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await screen.findByRole('button', { name: 'Turn Dance off' });
  expect(save).toHaveBeenCalledExactlyOnceWith('/project-a', settings({ enabled: true }), 'recovered');
});
it('retains an editor baseline through external events and offers Reload after a conflict', async () => {
  const read = vi.spyOn(api, 'readDance').mockResolvedValue(snapshot());
  const save = vi.spyOn(api, 'saveDance').mockRejectedValue({ code: 'dance_settings_conflict' });
  let changed!: (event: events.Event<api.DanceSettingsSnapshot & { projectRoot: string }>) => void;
  vi.mocked(events.listen).mockImplementation(async (name, listener) => { if (name === 'puppeteer-dance://changed') changed = listener as typeof changed; return () => {}; });
  render(<CardHost />); await waitFor(() => expect(read).toHaveBeenCalled());
  await userEvent.click(screen.getByRole('button', { name: 'Dance settings · Project A' }));
  await screen.findByRole('dialog', { name: 'Puppeteer Setting' });
  fireEvent.change(screen.getByRole('slider'), { target: { value: '.9' } });
  const external = snapshot(settings({ threshold: .8 }), 'external');
  await act(async () => changed({ event: '', id: 1, payload: { projectRoot: '/project-a', ...external } }));
  expect(screen.getByRole('slider')).toHaveValue('0.9');
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(save).toHaveBeenCalledExactlyOnceWith('/project-a', settings({ threshold: .9 }), 'read-1');
  await screen.findByText('Settings changed outside this editor. Reload before saving.');
  read.mockResolvedValue(external);
  await userEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await waitFor(() => expect(screen.getByRole('slider')).toHaveValue('0.8'));
});
