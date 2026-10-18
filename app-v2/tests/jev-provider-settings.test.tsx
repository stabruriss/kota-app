import { act, render, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { JevProviderSettings } from '../src/chrome/JevProviderSettings';
import { useJevSettings, type JevSettings } from '../src/lib/jev';
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn(), stop: vi.fn(), changed: null as null | ((event: { payload: JevSettings }) => void) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/event', () => ({ listen: mocks.listen }));
const settings = (status: 'none' | 'verified' | 'rejected' = 'none', provider: JevSettings['provider'] = 'openrouter'): JevSettings => ({
  schema: 1, provider, ready: status === 'verified', keys: [
    { providerId: 'openrouter', status: provider === 'openrouter' ? status : 'verified' },
    { providerId: 'vercel-ai-gateway', status: provider === 'vercel-ai-gateway' ? status : 'none' },
    { providerId: 'opencode', status: 'none' }, { providerId: 'typesafe', status: 'none' },
  ],
});
const deferred = <T,>() => { let resolve!: (value: T) => void; let reject!: (value: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
beforeEach(() => {
  mocks.changed = null; mocks.stop.mockReset(); mocks.invoke.mockReset(); mocks.listen.mockReset();
  mocks.listen.mockImplementation(async (_name, callback) => { mocks.changed = callback; return mocks.stop; });
  mocks.invoke.mockImplementation(async command => command === 'local_rpc_jev_read' ? settings() : undefined);
});
describe('shared Jev key settings', () => {
  it('shows loading when enabled before subscription or initial read completes', async () => {
    const subscription = deferred<() => void>(), read = deferred<JevSettings>();
    mocks.listen.mockReturnValue(subscription.promise);
    mocks.invoke.mockReturnValue(read.promise);
    const { result, rerender, unmount } = renderHook(({ enabled }) => useJevSettings(enabled), { initialProps: { enabled: false } });
    expect(result.current.loading).toBe(false);
    expect(mocks.listen).not.toHaveBeenCalled();
    rerender({ enabled: true });
    expect(result.current.loading).toBe(true);
    expect(mocks.invoke).not.toHaveBeenCalled();
    await act(async () => subscription.resolve(mocks.stop));
    expect(result.current.loading).toBe(true);
    expect(mocks.invoke).toHaveBeenCalledOnce();
    expect(mocks.invoke).toHaveBeenCalledWith('local_rpc_jev_read');
    await act(async () => read.resolve(settings('verified')));
    expect(result.current.loading).toBe(false);
    expect(result.current.settings?.ready).toBe(true);
    unmount();
    expect(mocks.stop).toHaveBeenCalledOnce();
  });
  it('uses selected channel state only and sends the exact provider argument', async () => {
    mocks.invoke.mockImplementation(async (command, args) => command === 'local_rpc_jev_read' ? settings('none', 'vercel-ai-gateway') : settings('none', args.providerId));
    render(<JevProviderSettings />);
    await screen.findByText('Not set');
    expect(screen.queryByText('Verified')).not.toBeInTheDocument(); // OpenRouter key cannot unlock Vercel
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Jev provider' }), 'typesafe');
    expect(mocks.invoke).toHaveBeenCalledWith('local_rpc_jev_select', { providerId: 'typesafe' });
    expect(screen.getByRole('combobox', { name: 'Jev provider' })).toHaveValue('typesafe');
  });
  it('verifies on Save, closes successful dialog, and never stores key in return data', async () => {
    mocks.invoke.mockImplementation(async command => command === 'local_rpc_jev_read' ? settings() : settings('verified'));
    render(<JevProviderSettings />);await screen.findByText('Not set');
    await userEvent.click(screen.getByRole('button', { name: 'Add key' }));
    await userEvent.type(screen.getByLabelText('Key'), 'fixture-only');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(mocks.invoke).toHaveBeenCalledWith('local_rpc_jev_key_save', { request: { requestId: expect.any(String), providerId: 'openrouter', key: 'fixture-only' } });
    expect(screen.getByText('Verified')).toBeVisible();
    expect(screen.queryByText('fixture-only')).not.toBeInTheDocument();
    expect(mocks.invoke).not.toHaveBeenCalledWith('local_rpc_jev_key_cancel', expect.anything());
  });
  it('failure retains input and original rejected state, never displays raw errors', async () => {
    mocks.invoke.mockImplementation(async command => { if (command === 'local_rpc_jev_read') return settings('rejected'); throw { code: 'jev_verification_failed', message: 'secret-like error not for UI' }; });
    render(<JevProviderSettings />);await screen.findByText('Rejected');
    expect(screen.getByText('This Jev key was rejected. Update it to resume routing.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Update key' }));
    await userEvent.type(screen.getByLabelText('Key'), 'fixture-only');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(await screen.findByText('Verification failed.')).toBeVisible();
    expect(screen.getByLabelText('Key')).toHaveValue('fixture-only');
    expect(screen.getByText('Rejected')).toBeVisible();
    expect(screen.queryByText('secret-like error not for UI')).not.toBeInTheDocument();
  });
  it('Cancel aborts the active request and ignores its late successful completion', async () => {
    const save = deferred<JevSettings>();
    mocks.invoke.mockImplementation(async command => command === 'local_rpc_jev_read' ? settings() : command === 'local_rpc_jev_key_save' ? save.promise : undefined);
    render(<JevProviderSettings />);await screen.findByText('Not set');
    await userEvent.click(screen.getByRole('button', { name: 'Add key' }));
    await userEvent.type(screen.getByLabelText('Key'), 'fixture-only');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(screen.getByRole('button', { name: 'Verifying…' })).toBeDisabled();
    const args = mocks.invoke.mock.calls.find(([command]) => command === 'local_rpc_jev_key_save')![1];
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(mocks.invoke).toHaveBeenCalledWith('local_rpc_jev_key_cancel', { requestId: args.request.requestId });
    await act(async () => save.resolve(settings('verified')));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.getByText('Not set')).toBeVisible();
  });
  it('shared change event wins over a stale read and removes listener on unmount', async () => {
    const read = deferred<JevSettings>();mocks.invoke.mockReturnValue(read.promise);
    const { unmount } = render(<JevProviderSettings />);
    await waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith('local_rpc_jev_read'));
    act(() => mocks.changed?.({ payload: settings('rejected') }));
    await act(async () => read.resolve(settings('verified')));
    expect(screen.getByText('Rejected')).toBeVisible();
    expect(screen.queryByText('Verified')).not.toBeInTheDocument();
    expect(mocks.invoke).toHaveBeenCalledTimes(1);
    unmount();expect(mocks.stop).toHaveBeenCalledOnce();
  });
  it('read failure is unavailable with explicit Retry, rather than missing key', async () => {
    mocks.invoke.mockRejectedValueOnce({ code: 'jev_store_invalid' }).mockResolvedValue(settings('verified'));
    render(<JevProviderSettings />);
    expect(await screen.findByText('Status unavailable')).toBeVisible();
    expect(screen.queryByText('Not set')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(await screen.findByText('Verified')).toBeVisible();
    expect(mocks.invoke).toHaveBeenCalledTimes(2);
  });
});
