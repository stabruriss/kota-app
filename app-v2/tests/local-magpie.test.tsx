import { StrictMode } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { KotaLocalMagpieRow, MagpieSettingsDialog } from '../src/chrome/KotaLocalMagpieRow';
import { ShellComboBox } from '../src/chrome/ShellComboBox';
import { localModelLabel, localModelOption, probeMagpie, saveMagpie, type MagpieState } from '../src/lib/local-magpie';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: ipc.invoke }));
const initial = (): MagpieState => ({ settings: { mode: 'local', ip: '', port: 3425 }, linked: false,
  keySaved: false, modelCount: 0, defaultKey: 'fixture-local-default' });
let saved: MagpieState;
const calls = (command: string) => ipc.invoke.mock.calls.filter(([name]) => name === command);
const settle = () => act(async () => { await Promise.resolve(); await Promise.resolve(); });
beforeEach(() => {
  saved = initial(); ipc.invoke.mockReset();
  ipc.invoke.mockImplementation(async (command, args) => {
    if (command === 'local_rpc_magpie_read') return { ...saved, linked: false };
    if (command === 'local_rpc_magpie_probe') return { ...saved, linked: true };
    if (command === 'local_rpc_magpie_save') return saved = { ...saved, settings: args.request.settings, linked: true, keySaved: true };
    throw new Error(`Unexpected fixture command: ${command}`);
  });
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it('uses a normal provider row, official mark, two statuses and only Settings', async () => {
  render(<KotaLocalMagpieRow onLinked={vi.fn()} />);
  const row = screen.getByRole('article', { name: 'Magpie' });
  expect(row).toHaveClass('local-auth-row');
  expect(row.querySelector('img')).toHaveAttribute('src', expect.stringContaining('magpie.svg'));
  expect(within(row).getByText('Not connected')).toBeInTheDocument();
  await screen.findByText('Linked');
  expect(within(row).getAllByRole('button').map(button => button.textContent)).toEqual(['Settings']);
  expect(row).not.toHaveTextContent(/Sign in|Sign out|Subscription|Use Credit|Refresh|Disconnect|3425|API key/);
  expect(calls('local_rpc_magpie_probe')).toHaveLength(1);
});
it('probes once on Link mount, not re-render, window focus, visibility or Settings/Cancel', async () => {
  const changed = vi.fn();
  const view = render(<StrictMode><KotaLocalMagpieRow onLinked={changed} /></StrictMode>);
  await screen.findByText('Linked');
  view.rerender(<StrictMode><KotaLocalMagpieRow onLinked={changed} /></StrictMode>);
  fireEvent.focus(window); document.dispatchEvent(new Event('visibilitychange'));
  fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
  await screen.findByRole('dialog', { name: 'Magpie settings' });
  fireEvent.change(screen.getByLabelText('Port'), { target: { value: '4567' } });
  fireEvent.click(screen.getByRole('radio', { name: 'LAN' }));
  fireEvent.change(screen.getByLabelText('IP'), { target: { value: '192.0.2.40' } });
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await settle();
  expect(calls('local_rpc_magpie_probe')).toHaveLength(1);
  expect(calls('local_rpc_magpie_save')).toHaveLength(0);
});
it('does not schedule background discovery while Link stays open', async () => {
  vi.useFakeTimers();
  try {
    render(<KotaLocalMagpieRow onLinked={vi.fn()} />); await settle();
    await act(async () => { await vi.advanceTimersByTimeAsync(120000); });
    expect(calls('local_rpc_magpie_probe')).toHaveLength(1);
  } finally { vi.useRealTimers(); }
});
it('Local uses the Rust default key and fixed IP; Mode shares exactly one Port', () => {
  render(<MagpieSettingsDialog initial={saved} onClose={vi.fn()} />);
  expect(screen.getByLabelText('IP')).toHaveValue('127.0.0.1');
  expect(screen.getByLabelText('IP')).toHaveAttribute('readonly');
  expect(screen.getByLabelText('IP')).not.toHaveAttribute('placeholder');
  expect(screen.getByLabelText('Port')).toHaveAttribute('type', 'text');
  expect(screen.getByLabelText('Port')).toHaveAttribute('inputmode', 'numeric');
  expect(screen.queryByRole('spinbutton')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Key')).toHaveValue('fixture-local-default');
  fireEvent.change(screen.getByLabelText('Port'), { target: { value: '4321' } });
  fireEvent.click(screen.getByRole('radio', { name: 'LAN' }));
  expect(screen.getByLabelText('IP')).not.toHaveAttribute('readonly');
  expect(screen.getByLabelText('IP')).toHaveValue('');
  expect(screen.getByLabelText('IP')).toHaveAttribute('placeholder', 'Magpie LAN Gateway IP, e.g. 192.168.1.101');
  expect(screen.getByLabelText('Port')).toHaveValue('4321');
  expect(screen.getByLabelText('Key')).toHaveValue('');
  expect(screen.getByLabelText('Key')).toBeRequired();
  fireEvent.click(screen.getByRole('radio', { name: 'Local' }));
  expect(screen.getByLabelText('Port')).toHaveValue('4321');
  expect(screen.getByLabelText('IP')).toHaveValue('127.0.0.1');
  expect(screen.getByLabelText('Key')).toHaveValue('fixture-local-default');
  expect(ipc.invoke).not.toHaveBeenCalled();
});
it('validates text ports as integers from 1 to 65535 before any Save IPC', async () => {
  const close = vi.fn(); render(<MagpieSettingsDialog initial={saved} onClose={close} />);
  const port = screen.getByLabelText('Port');
  for (const value of ['', '0', '-1', '3425.5', '65536', 'not-a-port']) {
    fireEvent.change(port, { target: { value } });
    expect(port).toBeInvalid();
    fireEvent.submit(screen.getByRole('dialog'));
    expect(ipc.invoke).not.toHaveBeenCalled(); expect(close).not.toHaveBeenCalled();
  }
  for (const value of ['1', '65535']) {
    fireEvent.change(port, { target: { value } }); expect(port).toBeValid();
  }
  fireEvent.submit(screen.getByRole('dialog')); await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(calls('local_rpc_magpie_save')[0][1].request.settings.port).toBe(65535);
  expect(calls('local_rpc_magpie_probe')).toHaveLength(0);
});
it('requires a LAN key; no prefix or length eligibility checks; saves through one IPC', async () => {
  const close = vi.fn(); render(<MagpieSettingsDialog initial={saved} onClose={close} />);
  fireEvent.click(screen.getByRole('radio', { name: 'LAN' }));
  fireEvent.change(screen.getByLabelText('IP'), { target: { value: '192.0.2.40' } });
  fireEvent.submit(screen.getByRole('dialog'));
  expect(ipc.invoke).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText('Key'), { target: { value: 'fixture-key' } });
  fireEvent.submit(screen.getByRole('dialog'));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  expect(calls('local_rpc_magpie_save')).toEqual([['local_rpc_magpie_save', { request: {
    settings: { mode: 'lan', ip: '192.0.2.40', port: 3425 }, key: 'fixture-key',
  } }]]);
  expect(calls('local_rpc_magpie_probe')).toHaveLength(0);
  expect(screen.getByLabelText('Key')).toHaveValue('');
});
it('never reads back a LAN key, and reuses it only for the same IP and Port', () => {
  const state = { ...saved, settings: { mode: 'lan' as const, ip: '192.0.2.40', port: 4567 }, keySaved: true };
  render(<MagpieSettingsDialog initial={state} onClose={vi.fn()} />);
  expect(screen.getByLabelText('IP')).toHaveValue('192.0.2.40');
  const key = screen.getByLabelText('Key');
  expect(key).toHaveAttribute('type', 'password'); expect(key).toHaveValue(''); expect(key).not.toBeRequired();
  fireEvent.change(screen.getByLabelText('Port'), { target: { value: '5678' } });
  expect(key).toBeRequired();
  fireEvent.change(screen.getByLabelText('Port'), { target: { value: '4567' } });
  fireEvent.change(screen.getByLabelText('IP'), { target: { value: '192.0.2.41' } });
  expect(key).toBeRequired(); expect(ipc.invoke).not.toHaveBeenCalled();
});
it('Escape cancels the draft without persisting a key or starting a probe', async () => {
  const close = vi.fn(); const storage = vi.spyOn(window.localStorage, 'setItem');
  render(<MagpieSettingsDialog initial={saved} onClose={close} />);
  fireEvent.change(screen.getByLabelText('Key'), { target: { value: 'fixture-never-store' } });
  fireEvent.keyDown(window, { key: 'Escape' });
  expect(close).toHaveBeenCalledOnce(); expect(storage).not.toHaveBeenCalled(); expect(ipc.invoke).not.toHaveBeenCalled();
});
it('offline Save keeps the target, closes Settings and reports Not connected, without a second probe', async () => {
  render(<KotaLocalMagpieRow onLinked={vi.fn()} />); await screen.findByText('Linked');
  fireEvent.click(screen.getByRole('button', { name: 'Settings' })); await screen.findByRole('dialog');
  fireEvent.change(screen.getByLabelText('Port'), { target: { value: '4567' } });
  ipc.invoke.mockImplementationOnce(async (_command, args) => saved = { ...saved, settings: args.request.settings, linked: false, error: 'unreachable' });
  fireEvent.submit(screen.getByRole('dialog'));
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(screen.getByText('Not connected')).toBeInTheDocument(); expect(saved.settings.port).toBe(4567);
  expect(calls('local_rpc_magpie_probe')).toHaveLength(1); expect(calls('local_rpc_magpie_save')).toHaveLength(1);
});
it('does not discard an invalid target that the backend could not save', async () => {
  ipc.invoke.mockResolvedValue({ ...saved, error: 'unreachable' });
  const close = vi.fn(); render(<MagpieSettingsDialog initial={saved} onClose={close} />);
  fireEvent.click(screen.getByRole('radio', { name: 'LAN' }));
  fireEvent.change(screen.getByLabelText('IP'), { target: { value: 'invalid host' } });
  fireEvent.change(screen.getByLabelText('Key'), { target: { value: 'fixture-key' } });
  fireEvent.submit(screen.getByRole('dialog')); await screen.findByRole('alert');
  expect(close).not.toHaveBeenCalled(); expect(screen.getByLabelText('IP')).toHaveValue('invalid host');
});
it.each(['key_rejected', 'save_failed'])('keeps the dialog open for %s with controlled copy, not raw errors', async error => {
  ipc.invoke.mockResolvedValue({ ...saved, error });
  const close = vi.fn(); render(<MagpieSettingsDialog initial={saved} onClose={close} />);
  fireEvent.submit(screen.getByRole('dialog')); await screen.findByRole('alert');
  expect(close).not.toHaveBeenCalled(); expect(screen.getByRole('alert')).not.toHaveTextContent(/fixture|http:/);
});
it('coalesces simultaneous openings and serializes a Settings save behind an earlier probe', async () => {
  let finish!: (state: MagpieState) => void;
  ipc.invoke.mockImplementationOnce(() => new Promise<MagpieState>(resolve => { finish = resolve; }));
  const first = probeMagpie(); const second = probeMagpie();
  const save = saveMagpie({ mode: 'local', ip: '', port: 4567 });
  expect(calls('local_rpc_magpie_probe')).toHaveLength(1); expect(calls('local_rpc_magpie_save')).toHaveLength(0);
  finish(saved); await Promise.all([first, second, save]);
  expect(calls('local_rpc_magpie_save')).toHaveLength(1);
});

describe('Local model menu labels and opening boundary', () => {
  it('preserves every inner slash and distinguishes identical upstream model names', () => {
    expect(localModelLabel('magpie', 'copilot/gpt-5.5', 'GPT-5.5')).toBe('Magpie / copilot / GPT-5.5');
    expect(localModelLabel('magpie', 'codex/gpt-5.5', 'GPT-5.5')).toBe('Magpie / codex / GPT-5.5');
    expect(localModelLabel('magpie', 'vendor/family/model')).toBe('Magpie / vendor / family/model');
    expect(localModelOption('magpie', 'vendor/family/model', 'Pretty')).toMatchObject({ id: 'magpie/vendor/family/model', displayLabel: 'Magpie / vendor / Pretty' });
    expect(localModelOption('openai', 'gpt', 'GPT')).toEqual({ id: 'openai/gpt', label: 'GPT', source: 'provider' });
    expect(localModelLabel('anthropic', 'claude', 'Claude')).toBe('anthropic / Claude');
  });
  it('invokes the menu-open callback once, not on re-render, search or model selection', async () => {
    const open = vi.fn(); const change = vi.fn();
    const options = [localModelOption('magpie', 'vendor/family/model', 'Pretty')];
    const view = render(<ShellComboBox value="" options={options} placeholder="Choose a model" onChange={change} onOpen={open} />);
    expect(open).not.toHaveBeenCalled(); fireEvent.focus(screen.getByRole('combobox')); await settle(); expect(open).toHaveBeenCalledTimes(1);
    view.rerender(<ShellComboBox value="" options={options} placeholder="Choose a model" onChange={change} onOpen={() => open()} />);
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'vendor' } }); await settle(); expect(open).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('option', { name: /Magpie \/ vendor \/ Pretty/ }));
    expect(change).toHaveBeenLastCalledWith('magpie/vendor/family/model'); expect(open).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole('combobox')); await settle(); expect(open).toHaveBeenCalledTimes(2);
  });
  it('shows the human label when closed, editing the intact ID only when the model menu is opened', () => {
    const option = localModelOption('magpie', 'vendor/family/model', 'Pretty');
    render(<ShellComboBox value={option.id} options={[option]} placeholder="Choose a model" onChange={vi.fn()} onOpen={vi.fn()} />);
    expect(screen.getByRole('combobox')).toHaveValue('Magpie / vendor / Pretty');
    fireEvent.focus(screen.getByRole('combobox'));
    expect(screen.getByRole('combobox')).toHaveValue('magpie/vendor/family/model');
  });
  it('leaves ordinary non-Local combo boxes and effort menus free of Magpie discovery', async () => {
    render(<ShellComboBox value="" options={[{ id: 'high', label: 'High' }]} placeholder="Effort" onChange={vi.fn()} />);
    fireEvent.focus(screen.getByRole('combobox')); fireEvent.click(screen.getByRole('option', { name: 'high' })); await settle();
    expect(ipc.invoke).not.toHaveBeenCalled();
  });
});
