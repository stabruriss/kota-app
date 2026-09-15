import { createHash } from 'node:crypto';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn(), isTauri: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { parseBbsRosterPage, readCompleteBbsRoster } from '../src/bbs-roster-client';
import { bbsRosterAvatarImages } from '../src/bbs-roster-avatars';
import { bbsMentionSelection, bbsMentionTargets } from '../src/bbs-mentions';
import { RightColumn } from '../src/chrome/RightColumn';
import type { BbsSnapshot, WorkspaceProject } from '../src/pty-client';
import fixture from './fixtures/bbs-roster-runtime.json';

// Unmodified BBS_ROSTER_FIXTURE from 74c12316b251edf4434d6a6a8be7c8e4e60bc7a2:
// roster/runtime/tests.rs::memory_pages_and_identity_free_local_avatar_use_actual_background_projection.
// SHA-256 b20fe5eb9db5f9fc0c506b9387cea2c82c18b82f0588ab1d483665eef80a96ef.
// This is one real identity-free local Page + read result, not a remote/multipage
// fixture. Only IPC delivery below is stubbed; roster client/hook/queue/UI are real.
const workspace: WorkspaceProject = {
  projectId: 'p', repoFullName: 'test/p', remoteUrl: '', githubHtmlUrl: '', defaultBranch: 'main', baseRef: 'main',
  localRoot: '/tmp/roster-fixture', localRootBytes: 0, sourceDir: '/tmp/roster-fixture/source', sourceDirBytes: 0,
  sharedDir: '/tmp/roster-fixture/memory', rulesDir: '/tmp/roster-fixture/rules', agents: [],
};
const board: BbsSnapshot = { projectId: 'p', projectDisplayName: 'Same Name', root: '/tmp/bbs-fixture', newCount: 0, threads: [] };
const imageRef = { deviceId: fixture.avatar.deviceId, sha256: fixture.avatar.sha256, ext: 'png', sizeBytes: 68 };
let rosterHint: () => void, stopRoster: ReturnType<typeof vi.fn>, rejectRoster: boolean;
const calls = (command: string) => vi.mocked(invoke).mock.calls.filter(call => call[0] === command);
const observers: VisibleObserver[] = [];
class VisibleObserver {
  target!: Element;
  constructor(private callback: IntersectionObserverCallback) { observers.push(this); }
  observe(target: Element) { this.target = target; }
  disconnect = vi.fn();
  show() { this.callback([{ target: this.target, isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver); }
}

beforeEach(() => {
  vi.restoreAllMocks(); vi.clearAllMocks(); window.localStorage.clear();
  // happy-dom has no layout-driven intersections. Explicitly enter the viewport;
  // the reader/queue remain real, and must not run before this signal.
  observers.length = 0; vi.stubGlobal('IntersectionObserver', VisibleObserver);
  vi.mocked(isTauri).mockReturnValue(true); rejectRoster = false; rosterHint = () => {}; stopRoster = vi.fn();
  bbsRosterAvatarImages.forget(imageRef);
  vi.mocked(listen).mockImplementation(async (event, handler) => {
    if (event === 'bbs-roster://changed') {
      rosterHint = () => handler({ event, id: 1, payload: null });
      return stopRoster;
    }
    return () => {};
  });
  vi.mocked(invoke).mockImplementation(async command => {
    switch (command) {
      case 'bbs_roster_read': if (rejectRoster) throw fixture.staleError; return fixture.local;
      case 'bbs_roster_avatar_read': return fixture.avatar.dataUrl;
      case 'bbs_snapshot': return board;
      case 'bbs_sync_status': return {
        protocolVersion: 1, device: { id: '', name: 'This device' }, worker: { configured: false, canCreateGroup: false },
        group: { id: null, name: null, role: null, members: [] }, invitation: 'none', invitationGeneration: null,
        sync: { phase: 'idle', completed: null, total: null, lastSuccessfulAt: null, error: null },
      };
      case 'account_user_identity_load': return { name: 'User', avatarId: 'user-default' };
      case 'hero_avatar_list': return [];
      case 'ember_schedule_state': return { drafts: [], schedules: [], history: [], appLastSeenAt: null };
      case 'lm_status': return null;
      case 'bartender_status': return { state: 'idle', message: '', dirtyAgents: [], roomChangeCount: 0, githubChangeCount: 0 };
      case 'violet_summary_status': return { latest: null, history: [], outstanding: { sinceTs: null, messageCount: 0 }, logPath: '', promptPath: '', updatedAt: '' };
      default: throw new Error(`No native operation in this fixture: ${command}`);
    }
  });
});
afterEach(() => { bbsRosterAvatarImages.forget(imageRef); vi.unstubAllGlobals(); });

describe('actual Rust roster projection through the BBS entry', () => {
  it('preserves null identity/local target and pairs the actual avatar bytes with its descriptor', async () => {
    const view = await readCompleteBbsRoster(async () => parseBbsRosterPage(fixture.local));
    expect(view.version).toBe(fixture.local.version);
    expect(view.devices).toHaveLength(1);
    const device = view.devices[0], project = device.projects![0], agent = project.agents[0];
    expect(device.deviceId).toBeNull(); expect(device.local).toBe(true);
    expect(agent.targetRef).toBe('local/p/a');
    expect(bbsMentionTargets([bbsMentionSelection(device, project, agent)])).toEqual([{ deviceId: 'local', projectId: 'p', agentId: 'a' }]);
    const bytes = Buffer.from(fixture.avatar.dataUrl.split(',')[1], 'base64');
    expect(bytes.byteLength).toBe(68);
    expect(createHash('sha256').update(bytes).digest('hex')).toBe(fixture.avatar.sha256);
    expect(agent.avatar).toEqual({ kind: 'image', sha256: fixture.avatar.sha256, ext: 'png', sizeBytes: bytes.byteLength, available: true });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('reads on opening, displays the real image and keeps the complete view/draft on the actual stale rejection', async () => {
    render(<RightColumn sceneKey="conversation" onOpenHotMem={() => {}} workspace={workspace} projectRoot={workspace.localRoot} />);
    expect(calls('bbs_roster_read')).toHaveLength(0); expect(calls('bbs_roster_avatar_read')).toHaveLength(0);
    await userEvent.click(screen.getByRole('button', { name: /^Post$/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Bulletin Board' });
    const editor = within(dialog).getByTestId('input-field'); await userEvent.type(editor, 'Keep the raw draft');
    expect(calls('bbs_roster_read')).toHaveLength(0);
    await userEvent.click(within(dialog).getByRole('button', { name: /^@ Agent/ }));
    const target = await within(dialog).findByRole('checkbox', { name: 'Photo, This device, Same Name' });
    expect(calls('bbs_roster_avatar_read')).toHaveLength(0);
    expect(observers).toHaveLength(1); act(() => observers[0].show());
    await waitFor(() => expect(target.querySelector('img')).toHaveAttribute('src', fixture.avatar.dataUrl));
    expect(calls('bbs_roster_read')).toEqual([['bbs_roster_read', { request: { version: null, after: null } }]]);
    expect(calls('bbs_roster_avatar_read')).toEqual([['bbs_roster_avatar_read', { request: { deviceId: 'local', sha256: fixture.avatar.sha256 } }]]);
    await userEvent.click(target);
    expect(within(dialog).getByText('1 agent · 1 project · 1 device')).toBeInTheDocument();
    const scans = calls('bbs_snapshot').length;
    rejectRoster = true; act(() => rosterHint());
    await within(dialog).findByText('The agent roster changed. Please retry.');
    await waitFor(() => expect(calls('bbs_roster_read')).toHaveLength(3), { timeout: 2000 }); // one bounded automatic retry
    expect(within(dialog).getByRole('checkbox', { name: 'Photo, This device, Same Name' })).toBe(target);
    expect(target).toHaveAttribute('aria-checked', 'true');
    expect(within(dialog).getByTestId('input-field')).toBe(editor); expect(editor).toHaveTextContent('Keep the raw draft');
    rejectRoster = false; await userEvent.click(within(dialog).getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(within(dialog).queryByText('The agent roster changed. Please retry.')).not.toBeInTheDocument());
    expect(calls('bbs_roster_read')).toHaveLength(4); expect(calls('bbs_roster_avatar_read')).toHaveLength(1);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Done' }));
    expect(stopRoster).toHaveBeenCalledOnce();
    expect(observers[0].disconnect).toHaveBeenCalledOnce();
    expect(calls('bbs_snapshot')).toHaveLength(scans);
    expect(within(dialog).getByRole('button', { name: 'Remove Photo, This device, Same Name' })).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Close Bulletin Board' }));
    expect(vi.mocked(invoke).mock.calls.filter(([command]) => /^(bbs_human_|agent_bus_send|bbs_sync_(invitation|join|start|cancel|disconnect)|bbs_sync_avatar_read)/.test(command))).toEqual([]);
  });
});
