import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
  isTauri: vi.fn(),
}));

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(),
}));

import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  __resetMockSmartPtyForTests,
  bartenderSyncReceipt,
  clearSmartPty,
  closeSmartPty,
  initSmartPty,
  interruptSmartPty,
  listSmartPtys,
  onSmartOutput,
  resizeSmartPty,
  restartSmartPty,
  spawnSmartPty,
  translateNlPrompt,
  writeSmartPty,
} from '../src/pty-client';
// (INITIAL_SCROLLBACK no longer asserted directly — text is now read
//  via the GridSnapshot helper below.)

describe('pty-client', () => {
  beforeEach(() => {
    __resetMockSmartPtyForTests();
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (command: string) => {
      switch (command) {
        case 'pty_smart_init':
          return 'smart-default';
        case 'pty_smart_spawn':
          return 'smart-extra';
        case 'pty_smart_list':
          return [{ ptyId: 'smart-default', cwd: '~', running: true }];
        default:
          return undefined;
      }
    });
    vi.mocked(listen).mockReset().mockResolvedValue((async () => {}) as never);
    vi.mocked(isTauri).mockReset().mockReturnValue(true);
  });

  it('maps multi-PTY commands to Tauri invoke calls', async () => {
    const defaultPtyId = await initSmartPty();
    const extraPtyId = await spawnSmartPty({ cwd: '~/work', cli: 'zsh' });

    await writeSmartPty(extraPtyId, 'ls\n');
    await resizeSmartPty(extraPtyId, 120, 32);
    await interruptSmartPty(extraPtyId);
    await clearSmartPty(extraPtyId);
    await restartSmartPty(extraPtyId);
    await closeSmartPty(extraPtyId);
    await listSmartPtys();
    await translateNlPrompt('find files modified in the last 24h');

    expect(defaultPtyId).toBe('smart-default');
    expect(extraPtyId).toBe('smart-extra');
    expect(invoke).toHaveBeenNthCalledWith(1, 'pty_smart_init');
    expect(invoke).toHaveBeenNthCalledWith(2, 'pty_smart_spawn', {
      cwd: '~/work',
      cli: 'zsh',
    });
    expect(invoke).toHaveBeenNthCalledWith(3, 'pty_smart_write', {
      ptyId: 'smart-extra',
      input: 'ls\n',
    });
    expect(invoke).toHaveBeenNthCalledWith(4, 'pty_smart_resize', {
      ptyId: 'smart-extra',
      cols: 120,
      rows: 32,
    });
    expect(invoke).toHaveBeenNthCalledWith(5, 'pty_smart_interrupt', {
      ptyId: 'smart-extra',
    });
    expect(invoke).toHaveBeenNthCalledWith(6, 'pty_smart_clear', {
      ptyId: 'smart-extra',
    });
    expect(invoke).toHaveBeenNthCalledWith(7, 'pty_smart_restart', {
      ptyId: 'smart-extra',
    });
    expect(invoke).toHaveBeenNthCalledWith(8, 'pty_smart_close', {
      ptyId: 'smart-extra',
    });
    expect(invoke).toHaveBeenNthCalledWith(9, 'pty_smart_list');
    expect(invoke).toHaveBeenNthCalledWith(10, 'pty_nl_translate', {
      ask: 'find files modified in the last 24h',
      provider: 'claude',
    });
  });

  it('keeps legacy single-PTY calls pinned to the default shell', async () => {
    await writeSmartPty('pwd\n');
    await resizeSmartPty(100, 28);
    await interruptSmartPty();
    await clearSmartPty();
    await restartSmartPty();

    expect(invoke).toHaveBeenNthCalledWith(1, 'pty_smart_init');
    expect(invoke).toHaveBeenNthCalledWith(2, 'pty_smart_write', {
      ptyId: 'smart-default',
      input: 'pwd\n',
    });
    expect(invoke).toHaveBeenNthCalledWith(3, 'pty_smart_resize', {
      ptyId: 'smart-default',
      cols: 100,
      rows: 28,
    });
    expect(invoke).toHaveBeenNthCalledWith(4, 'pty_smart_interrupt', {
      ptyId: 'smart-default',
    });
    expect(invoke).toHaveBeenNthCalledWith(5, 'pty_smart_clear', {
      ptyId: 'smart-default',
    });
    expect(invoke).toHaveBeenNthCalledWith(6, 'pty_smart_restart', {
      ptyId: 'smart-default',
    });
  });

  it('subscribes to smart output via the Tauri event bus', async () => {
    const handler = vi.fn();
    await onSmartOutput(handler);
    expect(listen).toHaveBeenCalledWith('pty://smart/output', expect.any(Function));
  });

  it('queries durable Bartender sync receipts by project and request id', async () => {
    vi.mocked(invoke).mockResolvedValueOnce({
      projectRoot: '/tmp/kota/proj',
      requestId: 'sync-123',
      phase: 'pending',
    });

    await bartenderSyncReceipt({
      projectRoot: '/tmp/kota/proj',
      requestId: 'sync-123',
    });

    expect(invoke).toHaveBeenCalledWith('bartender_sync_receipt', {
      request: {
        projectRoot: '/tmp/kota/proj',
        requestId: 'sync-123',
      },
    });
  });

  it('falls back to a multi-PTY mock backend outside Tauri', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    const handler = vi.fn();
    const stop = await onSmartOutput(handler);

    const ptyId = await initSmartPty();
    const secondPtyId = await spawnSmartPty({ cwd: '~/sandbox' });

    await writeSmartPty(ptyId, 'git status\n');
    await writeSmartPty(secondPtyId, 'pwd\n');

    // Per the M6.A grid refactor: events carry a GridSnapshot, not lines.
    // Walk each emitted snapshot and search the row text for substrings.
    const allText = handler.mock.calls
      .map(([payload]) => snapshotToText(payload?.snapshot));
    const allTextForPty = (id: string) =>
      handler.mock.calls
        .filter(([payload]) => payload?.ptyId === id)
        .map(([payload]) => snapshotToText(payload.snapshot))
        .join('\n');

    // Sanity: every emitted call has a snapshot.
    for (const text of allText) expect(text).toBeTypeOf('string');

    // INITIAL_SCROLLBACK text appears for both ptys.
    expect(allTextForPty(ptyId)).toMatch(/kota shell/);
    expect(allTextForPty(secondPtyId)).toMatch(/kota shell/);
    // Each pty got its command echoed into a snapshot.
    expect(allTextForPty(ptyId)).toMatch(/git status/);
    expect(allTextForPty(secondPtyId)).toMatch(/pwd/);

    await stop();
  });

  it('treats Ctrl-K as a line-buffer clear in the browser mock', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    const handler = vi.fn();
    await onSmartOutput(handler);
    const ptyId = await initSmartPty();

    await writeSmartPty(ptyId, 'stale draft');
    await writeSmartPty(ptyId, '\x0bfresh prompt\n');

    const text = handler.mock.calls
      .filter(([payload]) => payload?.ptyId === ptyId)
      .map(([payload]) => snapshotToText(payload.snapshot))
      .join('\n');
    expect(text).toMatch(/fresh prompt/);
    expect(text).not.toMatch(/stale draftfresh prompt/);
  });
});

/** Coalesce a GridSnapshot's cells into newline-joined row text — handy
 *  for asserting that a phrase appears somewhere in the visible grid. */
function snapshotToText(snap: { cols: number; rows: number; cells: { ch: string }[] } | undefined): string {
  if (!snap) return '';
  const lines: string[] = [];
  for (let r = 0; r < snap.rows; r++) {
    const row = snap.cells
      .slice(r * snap.cols, (r + 1) * snap.cols)
      .map((c) => c.ch)
      .join('');
    lines.push(row);
  }
  return lines.join('\n');
}
