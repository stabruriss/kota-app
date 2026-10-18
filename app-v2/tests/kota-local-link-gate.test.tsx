import { act, cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { TavernModal } from '../src/chrome/TavernModal';
import { requestKotaLocalLink } from '../src/lib/local-auth';
import * as pty from '../src/pty-client';

const calls = vi.hoisted(() => [] as string[]);
vi.mock('@tauri-apps/api/core', async (original) => ({
  ...await original<typeof import('@tauri-apps/api/core')>(),
  invoke: async (command: string) => {
    calls.push(command);
    throw new Error(`Unexpected fixture IPC: ${command}`);
  },
}));
const listened = vi.hoisted(() => [] as string[]);
vi.mock('@tauri-apps/api/event', () => ({ listen: async (name: string) => { listened.push(name); return () => {}; } }));
vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(), KOTA_LOCAL_ENABLED: false,
}));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it('shows no Kota Local card, makes no Local IPC and ignores Local links with Local disabled', async () => {
  const previous = Object.getOwnPropertyDescriptor(window, 'localStorage');
  const values = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  try {
    vi.spyOn(pty, 'loadTavernHeroProfiles').mockResolvedValue([]);
    vi.spyOn(pty, 'saveTavernHeroProfiles').mockResolvedValue();
    render(<TavernModal open onClose={() => {}} />);
    act(() => requestKotaLocalLink('kota-local-jev'));
    expect(screen.getByRole('button', { name: 'Hero' })).toHaveClass('active');
    await userEvent.click(screen.getByRole('button', { name: 'Link' }));
    expect(await screen.findByText('GitHub CLI')).toBeInTheDocument();
    expect(screen.queryByTestId('tavern-kota-local')).not.toBeInTheDocument();
    expect(screen.queryByText('Kota Local')).not.toBeInTheDocument();
    expect(calls.filter((command) => command.startsWith('local_rpc_'))).toEqual([]);
    expect(listened).not.toContain('local-auth-event');
  } finally {
    if (previous) Object.defineProperty(window, 'localStorage', previous);
    else Reflect.deleteProperty(window, 'localStorage');
  }
});
