import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { PuppeteerMarkAndDreamSettings } from '../src/chrome/PuppeteerMarkAndDreamSettings';
import type { SoulPushSettingsSnapshot } from '../src/lib/soul-push';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
let disk: SoulPushSettingsSnapshot;
let revision: number;
const writes = () => vi.mocked(invoke).mock.calls.filter(([command]) => command === 'puppeteer_soul_save');
const setup = async () => {
  const onJevSettings = vi.fn();
  render(<PuppeteerMarkAndDreamSettings onJevSettings={onJevSettings} />);
  await screen.findByRole('switch', { name: 'Use marks and dreams' });
  return onJevSettings;
};
beforeEach(() => {
  vi.clearAllMocks(); revision = 1;
  disk = { sourceRevision: 'r1', jevReady: true, settings: { schema: 1, enabled: true, markThreshold: .85, dreamThreshold: .90,
    maxMarks: 3, maxDreams: 2, cooldownReplies: 10, minimumMessageLength: 60 } };
  vi.mocked(invoke).mockImplementation(async (command, args) => {
    if (command === 'puppeteer_soul_read') return structuredClone(disk) as never;
    if (command !== 'puppeteer_soul_save') throw new Error('unexpected command');
    const request = args as { settings: SoulPushSettingsSnapshot['settings']; sourceRevision: string };
    disk = { ...disk, sourceRevision: `r${++revision}`, settings: request.settings };
    return structuredClone(disk) as never;
  });
});

it('uses .05–.95 in .05 steps and saves only after release or a completed keyboard adjustment', async () => {
  await setup();
  const mark = screen.getByRole('slider', { name: /^Mark threshold/ });
  expect(mark).toHaveAttribute('min', '0.05'); expect(mark).toHaveAttribute('max', '0.95'); expect(mark).toHaveAttribute('step', '0.05');
  fireEvent.pointerDown(mark, { pointerId: 1 });
  fireEvent.change(mark, { target: { value: '.6' } }); fireEvent.change(mark, { target: { value: '.55' } });
  expect(writes()).toHaveLength(0); expect(mark).toHaveAttribute('aria-valuetext', '0.55');
  fireEvent.pointerUp(mark, { pointerId: 1 });
  await waitFor(() => expect(writes()).toHaveLength(1));
  expect(writes()[0][1]).toMatchObject({ sourceRevision: 'r1', settings: { markThreshold: .55 } });
  await waitFor(() => expect(mark).toBeEnabled());
  fireEvent.change(mark, { target: { value: '.5' } }); fireEvent.keyUp(mark, { key: 'ArrowLeft' });
  await waitFor(() => expect(writes()).toHaveLength(2));
  expect(writes()[1][1]).toMatchObject({ sourceRevision: 'r2', settings: { markThreshold: .5 } });
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
});

it('commits zero and step boundaries, while incomplete or invalid numeric drafts stay unsaved', async () => {
  await setup();
  const length = screen.getByRole('spinbutton', { name: 'Minimum message length' });
  fireEvent.change(length, { target: { value: '' } }); fireEvent.blur(length);
  expect(writes()).toHaveLength(0); expect(length).toHaveAttribute('aria-invalid', 'true');
  fireEvent.change(length, { target: { value: '65' } }); fireEvent.keyDown(length, { key: 'Enter' });
  expect(writes()).toHaveLength(0);
  fireEvent.change(length, { target: { value: '0' } }); fireEvent.keyDown(length, { key: 'Enter' });
  await waitFor(() => expect(length).toBeEnabled());
  expect(writes()[0][1]).toMatchObject({ settings: { minimumMessageLength: 0 } });
  expect(screen.getByRole('button', { name: 'Decrease minimum message length' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Increase minimum message length' }));
  await waitFor(() => expect(writes()).toHaveLength(2));
  expect(writes()[1][1]).toMatchObject({ settings: { minimumMessageLength: 10 } });
});

it('disables unavailable Jev without saving false, and takes the configured off state from the read', async () => {
  disk.jevReady = false;
  const openJev = await setup();
  expect(screen.getByRole('switch')).not.toBeChecked(); expect(screen.getByRole('switch')).toBeDisabled();
  expect(screen.getByRole('slider', { name: /^Dream threshold/ })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Set up Jev under String to enable.' }));
  expect(openJev).toHaveBeenCalledOnce(); expect(writes()).toHaveLength(0); expect(disk.settings.enabled).toBe(true);
});

it('saves the toggle immediately without changing stored values', async () => {
  disk.settings.enabled = false;
  await setup();
  expect(screen.getByRole('switch')).not.toBeChecked();
  expect(screen.getByRole('slider', { name: /^Mark threshold/ })).toBeDisabled();
  await userEvent.click(screen.getByRole('switch'));
  await waitFor(() => expect(screen.getByRole('slider', { name: /^Mark threshold/ })).toBeEnabled());
  expect(writes()).toHaveLength(1);
  expect(writes()[0][1]).toEqual({ sourceRevision: 'r1', settings: { ...disk.settings, enabled: true } });
});

it('locks controls for the single in-flight save and retains the value and safe error for an explicit Retry', async () => {
  await setup();
  let fail!: (error: unknown) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
  await userEvent.click(screen.getByRole('button', { name: 'Increase max marks per message' }));
  expect(screen.getByText('Saving…')).toBeInTheDocument(); expect(screen.getByRole('switch')).toBeDisabled();
  expect(screen.getByRole('spinbutton', { name: 'Max marks per message' })).toHaveValue(4);
  await act(async () => fail({ code: 'soul_settings_write_failed', message: 'PRIVATE_PATH' }));
  expect(screen.getByText('Could not save Mark and Dream settings.')).toBeInTheDocument();
  expect(screen.queryByText('PRIVATE_PATH')).not.toBeInTheDocument();
  expect(writes()).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(screen.queryByText('Could not save Mark and Dream settings.')).not.toBeInTheDocument());
  expect(writes()).toHaveLength(2); expect(writes()[1][1]).toMatchObject({ sourceRevision: 'r1', settings: { maxMarks: 4 } });
});

it('does not flash a missing-key hint or any default controls while the initial read is pending', async () => {
  let finish!: (value: SoulPushSettingsSnapshot) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  render(<PuppeteerMarkAndDreamSettings onJevSettings={vi.fn()} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledOnce());
  expect(screen.getByRole('status')).toHaveTextContent('Loading…');
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  expect(screen.queryByText('Set up Jev under String to enable.')).not.toBeInTheDocument();
  await act(async () => finish(disk));
  expect(screen.getByRole('switch')).toBeChecked(); expect(writes()).toHaveLength(0);
});
