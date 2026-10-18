import { act, cleanup, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TavernModal } from '../src/chrome/TavernModal';
import { useAgentRuntime } from '../src/hooks/useAgentRuntime';
import * as pty from '../src/pty-client';
import * as local from '../src/lib/local-rpc';

const transport = vi.hoisted(() => ({
  enabled: false,
  calls: [] as Array<{ command: string; args?: unknown }>,
  replies: new Map<string, unknown>(),
}));
vi.mock('@tauri-apps/api/core', async (original) => ({
  ...await original<typeof import('@tauri-apps/api/core')>(),
  isTauri: () => transport.enabled,
  invoke: async (command: string, args?: unknown) => {
    transport.calls.push({ command, args });
    if (!transport.replies.has(command)) throw new Error(`Unexpected fixture IPC: ${command}`);
    return transport.replies.get(command);
  },
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(), KOTA_LOCAL_ENABLED: false,
}));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it('opens Tavern and launches a terminal shell with Local disabled without any Local probe or process', async () => {
  const previous = Object.getOwnPropertyDescriptor(window, 'localStorage');
  const values = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  const discover = vi.spyOn(local, 'discoverLocal');
  const snapshot = vi.spyOn(local, 'localSnapshot');
  const notice = vi.spyOn(local, 'onLocalNotice');
  const submit = vi.spyOn(local, 'submitLocal');
  const shells = vi.spyOn(pty, 'supportedShellsStatus');
  const spawn = vi.spyOn(pty, 'spawnAgentPty');
  try {
    // Collect the existing browser fixtures, then exercise the actual native
    // IPC branches without starting an account's real shell or changing files.
    transport.replies = new Map<string, unknown>([
      ['supported_shells_status', await pty.supportedShellsStatus()],
      ['auth_config_status', await pty.authConfigStatus()],
      ['storage_measure_status', await pty.storageMeasureStatus()],
      ['account_user_identity_load', { name: 'Alex', avatarId: null }],
      ['tavern_load_hero_profiles', []], ['tavern_save_hero_profiles', null],
      ['system_prompt_read', { path: '/fixture/prompt.md', content: 'Synthetic prompt.' }],
      ['pty_agent_spawn', { agentId: 'agent-fixture-terminal' }],
    ]);
    shells.mockClear(); transport.calls = []; transport.enabled = true;
    const view = render(<TavernModal open onClose={() => {}} />);
    await waitFor(() => expect(shells).toHaveBeenCalled());
    expect(screen.queryByText('Kota Local')).not.toBeInTheDocument();
    const { result, unmount } = renderHook(() => useAgentRuntime());
    const request = { agentId: 'agent-fixture-terminal', cli: 'codex' as const,
      cwd: '/fixture/copper', projectRoot: '/fixture/project', args: ['--model', 'default'] };
    await act(async () => { await result.current.recruit(request); });
    expect(spawn).toHaveBeenCalledExactlyOnceWith(request);
    expect(discover).not.toHaveBeenCalled(); expect(snapshot).not.toHaveBeenCalled();
    expect(notice).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled();
    expect(transport.calls.some(({ command }) => command === 'supported_shells_status')).toBe(true);
    expect(transport.calls.filter(({ command }) => command === 'pty_agent_spawn'))
      .toEqual([{ command: 'pty_agent_spawn', args: { request } }]);
    expect(transport.calls.filter(({ command }) => command.startsWith('local_rpc_'))).toEqual([]);
    console.log('LOCAL_DISABLED_IPC_TRACE=' + JSON.stringify(transport.calls));
    unmount(); view.unmount();
  } finally {
    transport.enabled = false;
    if (previous) Object.defineProperty(window, 'localStorage', previous);
    else Reflect.deleteProperty(window, 'localStorage');
  }
});
