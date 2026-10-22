import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../src/App';
import * as pty from '../src/pty-client';
import * as local from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';
import * as startup from '../src/lib/local-startup';
import * as core from '@tauri-apps/api/core';
import * as danceApi from '../src/lib/puppeteer-dance';
import * as jev from '../src/lib/jev';
import type { AgentCli } from '../src/types/agent-pty';
import type { AutoRouteGroup } from '../src/lib/string-routing';
import { formatAgentPromptInput } from '../src/lib/agent-prompt';
import { __clearVioletComposerSentHistoryForTests, emitVioletComposerSent, violetComposerSentHistory } from '../src/chrome/violet-room-events';

vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(), KOTA_LOCAL_ENABLED: true,
}));
vi.mock('@tauri-apps/api/core', async (original) => {
  const api = await original<typeof import('@tauri-apps/api/core')>();
  return { ...api, invoke: vi.fn(api.invoke) };
});
const originalInvoke = vi.mocked(core.invoke).getMockImplementation()!;

const root = '/tmp/kota-fixture-local-composer';
const localId = 'agent-fixture-copper';
const terminalId = 'agent-fixture-silver';
const names: Record<string, string> = { [localId]: 'Copper', [terminalId]: 'Silver' };
const cachedRoom = (messages: pty.VioletChatMessage[] = []): pty.VioletRoomState => ({ messages, sources: [], workEvents: [], agentBusReceipts: [],
  syncedAt: '2026-10-10T00:00:00Z', rawLogDir: root + '/project-memory/raw_logs', chathistoryDir: root + '/project-memory/chathistory' });
function agent(agentId: string): pty.WorkspaceProject['agents'][number] & { cli: AgentCli } {
  return { agentId, cli: agentId === localId ? 'kota-local' : 'codex', cwd: `${root}/.agent-workspaces/${agentId}`,
    projectRoot: root, worktreeRoot: `${root}/.agent-workspaces/${agentId}/project-files`,
    sharedDir: `${root}/project-memory`, rulesDir: `${root}/project-rules`,
    adapterPath: `${root}/.agent-workspaces/${agentId}/AGENTS.md`, args: [], projectId: 'fixture-local-composer', projectBaseRef: 'HEAD' };
}
function detail(agentId: string): pty.ProjectAgentDetail {
  const spec = agent(agentId);
  return { agentId, displayName: names[agentId], sourceHeroId: 'Hero-FABLE123', sourceHeroName: names[agentId],
    projectId: 'fixture-local-composer', projectName: 'Fixture', cli: spec.cli, provider: spec.cli,
    model: agentId === localId ? 'fixture/base' : 'default', effort: agentId === localId ? 'off' : null, avatarId: null, skills: [], args: [], ghost: '',
    adapterPath: spec.adapterPath!, shellPath: `${spec.cwd}/SHELL.yaml`, agentYamlPath: `${spec.cwd}/agent.yaml`,
    status: 'active', inviteEligibility: { eligible: true, proposedHeroId: 'Hero-FABLE123', proposedDisplayName: names[agentId] },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, forkable: false, dirty: false, dirtySummary: '' };
}

let startupView: startup.LocalStartView | null = null;
let startupListeners = new Set<(view: startup.LocalStartView) => void>();
function finishStartup(state: startup.LocalStartView['state'], error: startup.LocalStartError | null = null) {
  if (!startupView || startupView.state !== 'running') return;
  startupView = {...startupView,state,error,revision:startupView.revision+1};
  for (const listener of startupListeners) listener(structuredClone(startupView));
}
beforeEach(() => { vi.restoreAllMocks(); window.localStorage.clear(); __clearVioletComposerSentHistoryForTests(); startupView=null; startupListeners=new Set();
  vi.spyOn(danceApi, 'readDance').mockResolvedValue({ settings: { schema: 1, enabled: false, sentinel: localId, strategy: 'context', threshold: .75, conditions: {} }, sourceRevision: 'fixture-revision', currentSentinel: null });
  vi.spyOn(jev, 'readJevSettings').mockResolvedValue({ schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] });
  vi.mocked(core.invoke).mockImplementation((command, args) => command === 'local_rpc_jev_read' ? Promise.resolve({ schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] }) as never : originalInvoke(command, args));
});
afterEach(() => vi.restoreAllMocks());

async function mount(ids = [localId], extraWorkspaces: pty.WorkspaceProject[] = [], modelConfig: { model: string; thinkingLevel: string; autoRoute?: AutoRouteGroup } = { model: 'fixture/base', thinkingLevel: 'off' }) {
  const workspace: pty.WorkspaceProject = { projectId: 'fixture-local-composer', repoFullName: 'mock/fixture-local-composer',
    remoteUrl: 'https://example.invalid/fixture.git', githubHtmlUrl: 'https://example.invalid/fixture', defaultBranch: 'main', baseRef: 'HEAD',
    localRoot: root, localRootBytes: 0, sourceDir: `${root}/source`, sourceDirBytes: 0,
    sharedDir: `${root}/project-memory`, rulesDir: `${root}/project-rules`, agents: ids.map(agent) };
  vi.spyOn(pty, 'hasTauriRuntime').mockReturnValue(true);
  vi.spyOn(pty, 'workspaceStatus').mockResolvedValue({ active: workspace });
  vi.spyOn(pty, 'listWorkspaceProjects').mockResolvedValue([workspace, ...extraWorkspaces]);
  vi.spyOn(pty, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
  vi.spyOn(pty, 'saveProjectAgentLayoutFile').mockResolvedValue();
  vi.spyOn(pty, 'inspectProjectAgentIdentities').mockResolvedValue({ identities: ids.map((agentId) => ({ agentId,
    displayName: names[agentId], sourceHeroId: 'Hero-FABLE123', status: 'active', provider: agent(agentId).cli, avatarId: null })), workspaceEntryCount: ids.length });
  vi.spyOn(pty, 'loadProjectAgentDetail').mockImplementation(async ({ agentId }) => detail(agentId));
  if (!vi.isMockFunction(pty.peekVioletRoomMessages)) vi.spyOn(pty, 'peekVioletRoomMessages').mockReturnValue([]);
  if (!vi.isMockFunction(pty.readVioletRoomCache)) vi.spyOn(pty, 'readVioletRoomCache').mockResolvedValue(cachedRoom());
  vi.spyOn(pty, 'listAgentPtys').mockResolvedValue([]);
  vi.spyOn(pty, 'resolveProjectAgentLaunch').mockImplementation(async ({ agentId }) => ({ status: 'ready', request: agent(agentId) }));
  const check = vi.spyOn(pty, 'checkProjectAgentSession').mockResolvedValue(true);
  vi.spyOn(local, 'localSnapshot').mockResolvedValue({ scope: { projectRoot: root, agentId: localId, sessionId: 'fixture-session', epoch: 1 },
    state: 'ready', queued: 0, compacting: false, childPid: 42, model: null, thinkingLevel: null, error: null });
  vi.spyOn(local, 'onLocalNotice').mockResolvedValue(() => {});
  vi.spyOn(local, 'checkLocalLaunch').mockResolvedValue({ status: 'ok' });
  vi.spyOn(startup,'onLocalStartup').mockImplementation(async (_agent,callback) => {startupListeners.add(callback);return () => {startupListeners.delete(callback);};});
  vi.spyOn(startup,'localLaunchAction').mockImplementation(async (_root,_agent,action) => {
    if(action === 'configuration') return structuredClone(modelConfig) as never;
    if(action === 'begin') {
      const reused = startupView?.state === 'running';
      if(!reused) startupView = {projectRoot:root,agentId:localId,attemptId:crypto.randomUUID(),revision:1,state:'running',selection:null,saved:false,error:null,
        steps:[{stage:'configuration_read',startedAt:new Date().toISOString(),finishedAt:null,elapsedMs:0,error:null}]};
      return {view:structuredClone(startupView),reused} as never;
    }
    if(action === 'cancel') finishStartup('cancelled',{code:'startup_cancelled'});
    if(action === 'failed') finishStartup('failed',{code:'local_start_failed'});
    return structuredClone(startupView) as never;
  });
  const workbenchStart=vi.spyOn(startup,'startLocalWorkbench').mockImplementation(async request => {
    finishStartup('ready');
    return {agentId:request.agentId,outputEvent:'fixture/output',exitEvent:'fixture/exit',statusEvent:'fixture/status',workEvent:'fixture/work'};
  });
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({ schema: 1, summary: 'ready', providers: [{ providerId: 'fixture', status: 'signed_in', authType: 'api_key' }] });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [{ providerId: 'fixture', linked: true, models: [{ modelId: 'base', name: 'Base', thinkingLevels: ['off'] }] }] });
  const submit = vi.spyOn(local, 'submitLocal').mockResolvedValue();
  const targets = vi.spyOn(local, 'localInputTargets').mockResolvedValue(ids.filter((id) => id === localId));
  const spawn = vi.spyOn(pty, 'spawnAgentPty');
  const terminalSubmit = vi.spyOn(pty, 'submitAgentPromptPty').mockResolvedValue();
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  const view = render(<App />);
  await screen.findByTestId(`chip-${localId}`);
  await waitFor(() => expect(view.container.querySelector('.stage')).not.toHaveAttribute('aria-busy'));
  await userEvent.click(screen.getByTestId(`chip-${localId}`));
  return { submit, spawn, workbenchStart, terminalSubmit, targets, check };
}
async function send(text = 'Keep this exact original text.') {
  await userEvent.type(screen.getByTestId('input-field'), text);
  await userEvent.click(screen.getByTestId('ib-send'));
}
const conflict = { code: 'agent-session-lease-conflict' as const, agentId: localId, ownerPid: 456, childPid: 789 };
const readyView: local.LocalWorkbenchView = {
  scope: { projectRoot: root, agentId: localId, sessionId: 'fixture-session', epoch: 1 },
  state: 'ready', queued: 0, compacting: false, childPid: 42, model: null, thinkingLevel: null, error: null,
  revision: 1, runId: 0, startedAt: null, phase: 'Online', statsRevision: 0, dropped: 0,
  queueTotal: 0, queueRevision: 0, blocks: [], queue: [],
};

async function enableDance() {
  const save = vi.spyOn(danceApi, 'saveDance').mockImplementation(async (_root, settings) => ({ settings, sourceRevision: 'saved-revision', currentSentinel: settings.enabled ? settings.sentinel : null }));
  await userEvent.click(await screen.findByRole('button', { name: 'Turn Dance on' }));
  await screen.findByRole('button', { name: 'Turn Dance off' });
  return save;
}
it('Dance reuses the original route/prepared/identity for Retry, including after Dance is turned off', async () => {
  const { submit, check } = await mount([localId, terminalId]); const save = await enableDance();
  const gap = { currentTime: '2026-10-10T00:00:00Z', prompt: '<KOTA_TEMPORAL_GAP>\noriginal gap\n</KOTA_TEMPORAL_GAP>\n', targetAgentIds: [localId] };
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId, targetAgentId: localId, fallback: false, recorded: true,
    prepared: { targetAgentId: localId, payload: gap.prompt + request.payload, temporalGap: gap } }));
  const ordinary = vi.spyOn(pty, 'prepareComposerTemporalContext');
  submit.mockRejectedValueOnce(new Error('delivery failed'));
  await send('Keep this routed body');
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  const original = submit.mock.calls[0][0];
  expect(original).toMatchObject({ text: gap.prompt + 'Keep this routed body', projectRoot: root, agentId: localId });
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(screen.getByTestId('input-field')).toHaveTextContent('');
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance off' }));
  await userEvent.click(screen.getByTestId(`chip-${terminalId}`));
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  const retry = await screen.findByRole('button', { name: 'Queued message — try sending now' });
  const bubble = document.querySelector(`[data-violet-message-id="${original.messageId}"]`)!;
  expect(bubble).toHaveClass('agent', 'puppeteer-origin');
  expect(bubble.querySelector('.violet-msg-avatar')).toHaveClass('system-puppeteer');
  expect(bubble.querySelector('.violet-msg-avatar')).toHaveAttribute('title', 'Puppeteer');
  await userEvent.click(retry);
  await waitFor(() => expect(submit).toHaveBeenCalledTimes(2));
  expect(submit.mock.calls[1][0]).toEqual(original);
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(save).toHaveBeenLastCalledWith(root, expect.objectContaining({ enabled: false }), expect.any(String));
  expect(violetComposerSentHistory(root)).toHaveLength(1);
  expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'false');
  expect(check).not.toHaveBeenCalled(); // Local ownership stays on its existing submit/lease path.
});

async function terminalDance() {
  const env = await mount([localId, terminalId]);
  await userEvent.dblClick(screen.getByTestId(`chip-${terminalId}`));
  await waitFor(() => expect(env.spawn).toHaveBeenCalledOnce());
  await userEvent.click(screen.getByTestId(`chip-${localId}`));
  await enableDance();
  env.check.mockClear();
  const gap = '<KOTA_TEMPORAL_GAP>\nfirst routing gap\n</KOTA_TEMPORAL_GAP>\n';
  const temporalGap = { currentTime: '2026-10-10T00:00:00Z', prompt: gap, targetAgentIds: [terminalId] };
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(async (_root, request) => ({
    messageId: request.messageId, targetAgentId: terminalId, fallback: false, recorded: true,
    prepared: { targetAgentId: terminalId, payload: gap + request.payload, temporalGap },
  }));
  const ordinary = vi.spyOn(pty, 'prepareComposerTemporalContext');
  return { ...env, route, gap, ordinary };
}

it.each(['all', 'selected'] as const)('Dance follows the routed recipient under its overlay with the %s message filter', async filter => {
  const messages: pty.VioletChatMessage[] = [localId, terminalId].map((agentId, index) => ({
    id: `follow-reply-${index}`, agentId, sessionId: 'fixture-follow', shell: agent(agentId).cli,
    role: 'assistant', kind: 'message', timestamp: `2026-10-10T00:00:0${index}Z`, text: `${names[agentId]} previous reply`,
  }));
  vi.spyOn(pty, 'readVioletRoomCache').mockResolvedValue(cachedRoom(messages));
  const { route, terminalSubmit, submit } = await terminalDance();
  const save = vi.mocked(danceApi.saveDance);
  if (filter === 'selected') await userEvent.click(screen.getByTestId('ribbon-filter-clear'));
  else await userEvent.click(screen.getByTestId('group-chat-trigger'));
  expect(await screen.findByText('Copper previous reply')).toBeInTheDocument();
  if (filter === 'selected') expect(screen.queryByText('Silver previous reply')).not.toBeInTheDocument();

  await send('@Silver take this task');
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  expect(route.mock.calls[0][1].text).toBe('@Silver take this task');
  expect(violetComposerSentHistory(root).at(-1)?.targetAgentIds).toEqual([terminalId]);
  expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'false');
  expect(screen.getByText('Silver previous reply')).toBeInTheDocument();
  if (filter === 'selected') {
    expect(screen.queryByText('Copper previous reply')).not.toBeInTheDocument();
    expect(screen.getByTestId('group-chat-overlay')).toHaveAttribute('data-chat-filter-agents', terminalId);
  } else expect(screen.getByText('Copper previous reply')).toBeInTheDocument();
  expect(screen.getByTestId('agent-ribbon')).toHaveClass('dance-active');
  expect(screen.getByTestId('ib-target-pill')).toHaveTextContent('AUTO');
  expect(save).toHaveBeenCalledOnce(); // Automatic selection never saves Dance off.

  route.mockImplementationOnce(async (_root, request) => ({ messageId: request.messageId,
    targetAgentId: localId, fallback: true, recorded: true, prepared: { targetAgentId: localId, payload: request.payload } }));
  await waitFor(() => expect(screen.getByTestId('input-field')).toHaveTextContent(''));
  await send('Route this back to Copper');
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'false');
  if (filter === 'selected') {
    expect(screen.getByTestId('group-chat-overlay')).toHaveAttribute('data-chat-filter-agents', localId);
    expect(screen.queryByText('Silver previous reply')).not.toBeInTheDocument();
  }
  expect(screen.getByText('Copper previous reply')).toBeInTheDocument();
  expect(screen.getByTestId('agent-ribbon')).toHaveClass('dance-active');
  expect(route).toHaveBeenCalledTimes(2);
  expect(save).toHaveBeenCalledOnce();
});

it('Dance follows one routed target when enabled from a broadcast selection', async () => {
  const { submit } = await mount([localId, terminalId]);
  await userEvent.click(screen.getByTestId('ib-target-pill'));
  await userEvent.click(screen.getByTestId(`broadcast-option-${terminalId}`));
  await userEvent.click(screen.getByTestId('broadcast-confirm'));
  await userEvent.click(screen.getByTestId('ribbon-filter-clear'));
  expect(screen.getByTestId('group-chat-overlay')).toHaveAttribute('data-chat-filter-agents', `${localId}|${terminalId}`);
  const save = await enableDance();
  vi.spyOn(danceApi, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId,
    targetAgentId: localId, fallback: false, recorded: true, prepared: { targetAgentId: localId, payload: request.payload } }));
  await send('Route once from broadcast');
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId('group-chat-overlay')).toHaveAttribute('data-chat-filter-agents', localId);
  expect(screen.getByTestId('agent-ribbon')).toHaveClass('dance-active');
  expect(save).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance off' }));
  expect(screen.getByTestId('ib-target-pill')).toHaveTextContent('Copper');
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId('group-chat-overlay')).toHaveAttribute('data-chat-filter-agents', localId);
});

it('Dance keeps selection on submit failure and follows its original recipient only after Retry succeeds', async () => {
  const { route, terminalSubmit } = await terminalDance();
  terminalSubmit.mockRejectedValueOnce(new Error('fixture delivery failure'));
  await send('Keep selection until delivered');
  await waitFor(() => expect(screen.getByTestId('input-field')).toHaveTextContent(''));
  expect(terminalSubmit).toHaveBeenCalledOnce();
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'false');
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  await userEvent.click(await screen.findByRole('button', { name: 'Queued message — try sending now' }));
  await waitFor(() => expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'true'));
  expect(terminalSubmit).toHaveBeenCalledTimes(2);
  expect(terminalSubmit.mock.calls[1]).toEqual(terminalSubmit.mock.calls[0]);
  // The same-ID Retry carries the first send's host-only Soul metadata.
  expect(terminalSubmit.mock.calls[0][2]).toEqual({ messageId: route.mock.calls[0][1].messageId, createdAt: route.mock.calls[0][1].timestamp,
    origin: 'puppeteer', originalText: 'Keep selection until delivered', eligibilityText: 'Keep selection until delivered', targetAgentIds: [terminalId] });
  expect(route).toHaveBeenCalledOnce();
  expect(screen.getByTestId('agent-ribbon')).toHaveClass('dance-active');
});

it.each([false, true])('a late Dance route cannot replace a manual selection after exiting Dance (re-enabled=%s)', async restart => {
  const { submit } = await mount([localId, terminalId]); await enableDance();
  let finish!: (result: Awaited<ReturnType<typeof danceApi.routeDance>>) => void;
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  await send('Route before manual exit');
  await waitFor(() => expect(route).toHaveBeenCalledOnce());
  await userEvent.click(screen.getByTestId(`chip-${terminalId}`));
  await waitFor(() => expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'true'));
  await screen.findByRole('button', { name: 'Turn Dance on' });
  if (restart) await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' }));
  const request = route.mock.calls[0][1];
  await act(async () => finish({ messageId: request.messageId, targetAgentId: localId, recorded: true, fallback: false,
    prepared: { targetAgentId: localId, payload: request.payload } }));
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'false');
});

it.each(['route', 'submit'] as const)('a late Dance %s cannot replace selection after switching projects', async pending => {
  const otherRoot = '/tmp/kota-fixture-follow-other';
  const otherId = 'agent-fixture-gold';
  const otherAgent = { ...agent(otherId), projectId: 'fixture-follow-other', projectRoot: otherRoot,
    cwd: `${otherRoot}/.agent-workspaces/${otherId}`, worktreeRoot: `${otherRoot}/.agent-workspaces/${otherId}/project-files`,
    sharedDir: `${otherRoot}/project-memory`, rulesDir: `${otherRoot}/project-rules`, adapterPath: `${otherRoot}/.agent-workspaces/${otherId}/AGENTS.md` };
  const other: pty.WorkspaceProject = { projectId: 'fixture-follow-other', repoFullName: 'mock/fixture-follow-other',
    remoteUrl: 'https://example.invalid/other.git', githubHtmlUrl: 'https://example.invalid/other', defaultBranch: 'main', baseRef: 'HEAD',
    localRoot: otherRoot, localRootBytes: 0, sourceDir: `${otherRoot}/source`, sourceDirBytes: 0,
    sharedDir: `${otherRoot}/project-memory`, rulesDir: `${otherRoot}/project-rules`, agents: [otherAgent] };
  const { submit } = await mount([localId, terminalId], [other]); await enableDance();
  vi.spyOn(pty, 'openWorkspaceProject').mockResolvedValue(other);
  vi.mocked(pty.inspectProjectAgentIdentities).mockResolvedValue({ identities: [{ agentId: otherId,
    displayName: 'Gold', sourceHeroId: 'Hero-FABLE456', status: 'active', provider: 'codex', avatarId: null }], workspaceEntryCount: 1 });
  vi.mocked(pty.loadProjectAgentDetail).mockResolvedValue({ ...detail(otherId), displayName: 'Gold', projectId: other.projectId,
    shellPath: `${otherAgent.cwd}/SHELL.yaml`, agentYamlPath: `${otherAgent.cwd}/agent.yaml`, adapterPath: otherAgent.adapterPath });
  let finishRoute!: (result: Awaited<ReturnType<typeof danceApi.routeDance>>) => void;
  let finishSubmit!: () => void;
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(async (_root, request) => pending === 'route'
    ? new Promise(resolve => { finishRoute = resolve; })
    : { messageId: request.messageId, targetAgentId: localId, recorded: true, fallback: false,
      prepared: { targetAgentId: localId, payload: request.payload } });
  if (pending === 'submit') submit.mockImplementationOnce(() => new Promise(resolve => { finishSubmit = resolve; }));
  await send('Route before room switch');
  await waitFor(() => expect(route).toHaveBeenCalledOnce());
  if (pending === 'submit') await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  await userEvent.click(screen.getByTestId('tab-fixture-follow-other'));
  await screen.findByTestId(`chip-${otherId}`);
  await waitFor(() => expect(document.querySelector('.stage')).not.toHaveAttribute('aria-busy'));
  await userEvent.click(screen.getByTestId(`chip-${otherId}`));
  const request = route.mock.calls[0][1];
  await act(async () => {
    if (pending === 'route') finishRoute({ messageId: request.messageId, targetAgentId: localId, recorded: true, fallback: false,
      prepared: { targetAgentId: localId, payload: request.payload } });
    else finishSubmit();
  });
  expect(screen.getByTestId('tab-fixture-follow-other')).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByTestId(`chip-${otherId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId('ib-target-pill')).toHaveTextContent('Gold');
});

it.each(['Cancel', 'OK'] as const)('Dance ownership preflight %s keeps the draft and reuses the first route on explicit Send', async choice => {
  const { check, route, terminalSubmit, submit, gap, ordinary } = await terminalDance();
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession').mockResolvedValue({ detail: detail(terminalId), request: agent(terminalId) });
  check.mockResolvedValue(false);
  await send('Keep this Dance draft');
  const dialog = await screen.findByRole('dialog', { name: "Session can't resume" });
  // Existing PTY IPC uses null to resolve the currently active workspace.
  expect(check).toHaveBeenCalledWith({ agentId: terminalId, projectRoot: null });
  expect(route).toHaveBeenCalledOnce();
  expect(violetComposerSentHistory(root)).toHaveLength(0);
  expect(terminalSubmit).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled();
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByTestId(`chip-${terminalId}`)).toHaveAttribute('aria-pressed', 'false');
  await userEvent.click(within(dialog).getByRole('button', { name: choice }));
  await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep this Dance draft');
  expect(fresh).toHaveBeenCalledTimes(choice === 'OK' ? 1 : 0);
  expect(terminalSubmit).not.toHaveBeenCalled();
  check.mockResolvedValue(true);
  await userEvent.click(screen.getByTestId('ib-send'));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.getByTestId('input-field')).toHaveTextContent(''));
  expect(terminalSubmit).toHaveBeenCalledWith(terminalId, formatAgentPromptInput(gap + 'Keep this Dance draft'),
    expect.objectContaining({ messageId: route.mock.calls[0][1].messageId, origin: 'puppeteer', eligibilityText: 'Keep this Dance draft' }));
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(violetComposerSentHistory(root)).toHaveLength(1);
  expect(violetComposerSentHistory(root)[0]).toMatchObject({ id: route.mock.calls[0][1].messageId, timestamp: route.mock.calls[0][1].timestamp, targetAgentIds: [terminalId] });
});

it.each(['edit', 'clear'] as const)('Dance ownership recovery invalidates the prepared route after a draft %s', async change => {
  const { check, route, terminalSubmit, gap } = await terminalDance();
  check.mockResolvedValue(false);
  await send('Original Dance draft');
  const dialog = await screen.findByRole('dialog', { name: "Session can't resume" });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  const field = screen.getByTestId('input-field');
  const next = change === 'clear' ? 'Original Dance draft' : 'Original Dance draft edited';
  if (change === 'clear') { field.replaceChildren(); fireEvent.input(field); await userEvent.type(field, next); }
  else await userEvent.type(field, ' edited');
  const abandoned = violetComposerSentHistory(root)[0];
  expect(abandoned).toMatchObject({ id: route.mock.calls[0][1].messageId, text: 'Original Dance draft', messageOrigin: 'puppeteer',
    delivery: { status: 'failed', retryTargetAgentIds: [terminalId] } });
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  expect(await screen.findByRole('button', { name: 'Queued message — try sending now' })).toBeInTheDocument();
  expect(document.querySelector(`[data-violet-message-id="${abandoned.id}"]`)).toBeInTheDocument();
  check.mockResolvedValue(true);
  await userEvent.click(screen.getByTestId('ib-send'));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  expect(route).toHaveBeenCalledTimes(2);
  expect(route.mock.calls[1][1].messageId).not.toBe(route.mock.calls[0][1].messageId);
  expect(terminalSubmit).toHaveBeenCalledWith(terminalId, formatAgentPromptInput(gap + next),
    expect.objectContaining({ messageId: route.mock.calls[1][1].messageId, eligibilityText: next }));
  expect(violetComposerSentHistory(root)).toHaveLength(2);
  expect(violetComposerSentHistory(root)[1].id).toBe(route.mock.calls[1][1].messageId);
});

it('Dance transient ownership read failure keeps the prepared draft without starting Fresh', async () => {
  const { check, route, terminalSubmit, ordinary } = await terminalDance();
  check.mockRejectedValueOnce(new Error('fixture database busy'));
  const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession');
  await send('Keep after a transient read');
  await waitFor(() => expect(alert).toHaveBeenCalledWith(`Open ${terminalId} failed: fixture database busy`));
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep after a transient read');
  expect(screen.queryByRole('dialog', { name: "Session can't resume" })).not.toBeInTheDocument();
  expect(violetComposerSentHistory(root)).toHaveLength(0);
  expect(fresh).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
  await userEvent.click(screen.getByTestId('ib-send'));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(check).toHaveBeenCalledTimes(2);
});
it('retries an abandoned, already recorded Dance draft with its original body/gap while preserving the edited composer draft', async () => {
  const { check, route, terminalSubmit, ordinary, gap } = await terminalDance();
  check.mockResolvedValue(false);
  await send('Recorded original');
  await userEvent.click(within(await screen.findByRole('dialog', { name: "Session can't resume" })).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  await userEvent.type(screen.getByTestId('input-field'), ' edited');
  const original = violetComposerSentHistory(root)[0];
  check.mockResolvedValue(true);
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  await userEvent.click(await screen.findByRole('button', { name: 'Queued message — try sending now' }));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  // A backend prepared gap is part of the original, and Retry must restore it.
  // This draft never reached the host, and the room Retry has no draft: no Soul metadata.
  expect(terminalSubmit).toHaveBeenCalledWith(terminalId, formatAgentPromptInput(gap + 'Recorded original'), undefined);
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(violetComposerSentHistory(root)).toHaveLength(1); expect(violetComposerSentHistory(root)[0].id).toBe(original.id);
  expect(screen.getByTestId('input-field')).toHaveTextContent('Recorded original edited');
});

it.each(['desktop double click', 'Cmd shortcut'])('opens a Local workbench through %s without selecting a target or exiting Dance', async action => {
  readReadyWorkbench(); await mount(); const save = await enableDance();
  if (action === 'Cmd shortcut') await userEvent.keyboard('{Meta>}1{/Meta}');
  else await userEvent.dblClick(document.querySelector('.seat-ring [data-seat-index="0"]')!);
  await screen.findByLabelText('Copper workbench');
  expect(save).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Turn Dance off' })).toBeInTheDocument();
  expect(screen.getByTestId('ib-target-pill')).toHaveTextContent('AUTO');
});

it('Dance ownership denial during submit preserves one original and retries the exact prepared body', async () => {
  const { check, route, terminalSubmit, ordinary } = await terminalDance();
  terminalSubmit.mockRejectedValueOnce(new Error('KOTA_SESSION_UNAVAILABLE: fixture'));
  await send('Keep after the ownership race');
  const dialog = await screen.findByRole('dialog', { name: "Session can't resume" });
  expect(check).toHaveBeenCalledOnce(); expect(terminalSubmit).toHaveBeenCalledOnce();
  expect(violetComposerSentHistory(root)).toHaveLength(1);
  const original = violetComposerSentHistory(root)[0];
  await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.getByTestId('input-field')).toHaveAttribute('contenteditable', 'true'));
  expect(screen.getByTestId('input-field')).toHaveTextContent('');
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  await userEvent.click(await screen.findByRole('button', { name: 'Queued message — try sending now' }));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledTimes(2));
  expect(terminalSubmit.mock.calls[1]).toEqual(terminalSubmit.mock.calls[0]);
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(violetComposerSentHistory(root)).toHaveLength(1);
  expect(violetComposerSentHistory(root)[0].id).toBe(original.id);
});

it('clears the composer as soon as the room bubble appears while Local delivery is still pending', async () => {
  const { submit } = await mount();
  let finishSubmit!: () => void;
  submit.mockImplementationOnce(() => new Promise<void>(resolve => { finishSubmit = resolve; }));
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  await send('Early clear on Local bubble');
  expect(await screen.findByText('Early clear on Local bubble')).toBeInTheDocument();
  expect(screen.getByTestId('input-field')).toHaveTextContent('');
  expect(submit).toHaveBeenCalledOnce();
  await act(async () => finishSubmit());
  expect(screen.getByTestId('input-field')).toHaveTextContent('');
});
it.each(['jev_auth', 'jev_timeout', 'jev_unreachable'])('Dance failure %s keeps the draft, does not deliver or record a speculative recipient', async code => {
  const { submit, terminalSubmit } = await mount(); await enableDance();
  vi.spyOn(danceApi, 'routeDance').mockRejectedValue({ code });
  await send('Keep after classification failed');
  expect(await screen.findByRole('alert')).toHaveTextContent(code === 'jev_auth' ? 'Jev authentication failed. Update the key in Puppeteer.' : code === 'jev_timeout' ? 'Jev did not respond within 3 seconds.' : 'Jev could not be reached.');
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep after classification failed');
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled(); expect(violetComposerSentHistory(root)).toHaveLength(0);
});
it('keeps the composer read only while the real Dance classification is pending and delivers only its original body', async () => {
  const { submit } = await mount(); await enableDance();
  let finish!: (result: Awaited<ReturnType<typeof danceApi.routeDance>>) => void;
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  await send('The first body');
  await waitFor(() => expect(route).toHaveBeenCalledOnce());
  const field = screen.getByTestId('input-field');
  expect(field).toHaveAttribute('contenteditable', 'false'); expect(field).toHaveAttribute('aria-readonly', 'true');
  await userEvent.type(field, 'Do not lose a later edit');
  expect(field).toHaveTextContent('The first body');
  const request = route.mock.calls[0][1];
  await act(async () => finish({ messageId: request.messageId, targetAgentId: localId, recorded: true, fallback: false,
    prepared: { targetAgentId: localId, payload: request.payload } }));
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(submit.mock.calls[0][0].text).toBe('The first body');
  await waitFor(() => expect(field).toHaveAttribute('contenteditable', 'true'));
  expect(field).toHaveTextContent('');
});

it.each(['Wake up', 'Cancel'] as const)('keeps a sleeping Dance target’s prepared route after %s and waits for another explicit Send', async choice => {
  const { spawn, terminalSubmit } = await mount([localId, terminalId]); await enableDance();
  const gap = '<KOTA_TEMPORAL_GAP>\nwake gap\n</KOTA_TEMPORAL_GAP>\n';
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId, targetAgentId: terminalId, recorded: true, fallback: false,
    prepared: { targetAgentId: terminalId, payload: gap + request.payload } }));
  const ordinary = vi.spyOn(pty, 'prepareComposerTemporalContext');
  await send('Wait for the CLI');
  const wake = await screen.findByRole('dialog', { name: 'Wake sleeping agent?' });
  await userEvent.click(within(wake).getByRole('button', { name: choice }));
  await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  expect(terminalSubmit).not.toHaveBeenCalled(); expect(screen.getByTestId('input-field')).toHaveTextContent('Wait for the CLI');
  expect(screen.getByTestId(`chip-${localId}`)).toHaveAttribute('aria-pressed', 'true');
  if (choice === 'Cancel') {
    expect(spawn).not.toHaveBeenCalled();
    await userEvent.click(screen.getByTestId('ib-send'));
    await userEvent.click(within(await screen.findByRole('dialog', { name: 'Wake sleeping agent?' })).getByRole('button', { name: 'Wake up' }));
    await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  }
  expect(spawn).toHaveBeenCalledOnce(); expect(terminalSubmit).not.toHaveBeenCalled(); expect(route).toHaveBeenCalledOnce();
  expect(violetComposerSentHistory(root)).toHaveLength(1); expect(violetComposerSentHistory(root)[0].delivery?.status).toBe('failed');
  await userEvent.click(screen.getByTestId('ib-send'));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  expect(terminalSubmit).toHaveBeenCalledWith(terminalId, formatAgentPromptInput(gap + 'Wait for the CLI'),
    expect.objectContaining({ messageId: route.mock.calls[0][1].messageId, origin: 'puppeteer' }));
  expect(route).toHaveBeenCalledOnce(); expect(ordinary).not.toHaveBeenCalled();
  expect(violetComposerSentHistory(root)).toHaveLength(1);
  expect(violetComposerSentHistory(root)[0]).toMatchObject({ id: route.mock.calls[0][1].messageId, delivery: { status: 'clear' } });
});

it.each([false, true])('uses existing reply context before routing without opening chat (warm projection=%s)', async warm => {
  const message: pty.VioletChatMessage = { id: 'cached-reply', agentId: localId, sessionId: 'cached-session', shell: 'kota-local',
    role: 'assistant', kind: 'message', timestamp: '2026-10-09T00:00:00Z', text: 'Previous work still matters' };
  vi.spyOn(pty, 'peekVioletRoomMessages').mockReturnValue(warm ? [message] : []);
  const cache = vi.spyOn(pty, 'readVioletRoomCache').mockResolvedValue(cachedRoom([message]));
  await mount(); await enableDance(); cache.mockClear();
  const route = vi.spyOn(danceApi, 'routeDance').mockImplementation(async (_root, request) => ({ messageId: request.messageId, targetAgentId: localId, recorded: true, fallback: false,
    prepared: { targetAgentId: localId, payload: request.payload } }));
  await send('Use the recent context'); await waitFor(() => expect(route).toHaveBeenCalledOnce());
  expect(route.mock.calls[0][1].roster[0].lastReply).toEqual({ at: message.timestamp, text: message.text });
  expect(cache.mock.calls.filter(([request]) => request?.limit === 200)).toHaveLength(warm ? 0 : 1);
  expect(screen.queryByTestId('violet-room-panel')).not.toBeInTheDocument();
});
it('keeps the draft and does not classify when a cold readonly cache cannot be read', async () => {
  const { submit } = await mount(); await enableDance();
  vi.mocked(pty.readVioletRoomCache).mockRejectedValue(new Error('fixture cache read failed'));
  const route = vi.spyOn(danceApi, 'routeDance');
  await send('Keep on cache failure');
  expect(await screen.findByRole('alert')).toHaveTextContent('Agent roster unavailable. Try again.');
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep on cache failure');
  expect(route).not.toHaveBeenCalled(); expect(submit).not.toHaveBeenCalled(); expect(violetComposerSentHistory(root)).toHaveLength(0);
});

it.each(['seat', 'PgDn', 'pill'] as const)('waits for a pending settings save before %s exits Dance, preserving that save’s threshold', async action => {
  await mount([localId, terminalId]); const save = await enableDance();
  let finish!: (settings: danceApi.DanceSettingsSnapshot) => void;
  save.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  // Opening settings re-reads the file; it now holds the enabled state just saved.
  vi.mocked(danceApi.readDance).mockResolvedValue({ settings: save.mock.calls[0][1], sourceRevision: 'saved-revision', currentSentinel: save.mock.calls[0][1].sentinel });
  await userEvent.click(screen.getByRole('button', { name: /^Dance settings · / }));
  const editor = await screen.findByRole('dialog', { name: 'Puppeteer Setting' });
  fireEvent.change(within(editor).getByRole('slider'), { target: { value: '0.9' } });
  await userEvent.click(within(editor).getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(2));
  // Closing does not cancel the queued save.
  await userEvent.click(within(editor).getByRole('button', { name: 'Close Dance settings' }));
  if (action === 'PgDn') { await userEvent.click(screen.getByTestId('input-field')); await userEvent.keyboard('{PageDown}'); }
  else if (action === 'pill') await userEvent.click(screen.getByTestId('ib-target-pill'));
  else await userEvent.click(document.querySelector('.seat-ring [data-seat-index="1"]')!);
  // A seat's single click is held for the double-click window before queuing.
  if (action === 'seat') await new Promise(resolve => setTimeout(resolve, 250));
  expect(save).toHaveBeenCalledTimes(2);
  await act(async () => finish({ settings: save.mock.calls[1][1], sourceRevision: 'saved-revision', currentSentinel: save.mock.calls[1][1].sentinel }));
  await waitFor(() => expect(save).toHaveBeenCalledTimes(3));
  expect(save).toHaveBeenLastCalledWith(root, expect.objectContaining({ enabled: false, threshold: .9 }), expect.any(String));
  await screen.findByRole('button', { name: 'Turn Dance on' });
  if (action === 'pill') expect(screen.getByTestId('broadcast-target-popover')).toBeInTheDocument();
  else expect(screen.getByTestId('ib-target-pill')).toHaveTextContent('Silver');
});
it('PgDn and target pill exit Dance directly and preserve the composer draft', async () => {
  await mount([localId, terminalId]); const save = await enableDance();
  await userEvent.type(screen.getByTestId('input-field'), 'Keep while changing target');
  await userEvent.keyboard('{PageDown}');
  await waitFor(() => expect(save).toHaveBeenLastCalledWith(root, expect.objectContaining({ enabled: false }), expect.any(String)));
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep while changing target');
  expect(screen.queryByRole('dialog', { name: /Exit/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Turn Dance on' })); await screen.findByRole('button', { name: 'Turn Dance off' });
  await userEvent.click(screen.getByTestId('ib-target-pill'));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Turn Dance on' })).toBeInTheDocument());
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep while changing target');
  expect(screen.queryByRole('dialog', { name: /Exit/ })).not.toBeInTheDocument();
});
function readReadyWorkbench() {
  vi.spyOn(local, 'controlLocalWorkbench').mockImplementation(async (_scope, request) => {
    const data = request.type === 'get_state' ? { autoCompactionEnabled: true }
      : request.type === 'get_available_models' ? { models: [] }
        : request.type === 'get_available_thinking_levels' ? { levels: [] } : { contextUsage: null };
    return data as never;
  });
  return vi.spyOn(local, 'readLocalWorkbench').mockImplementation(async () =>
    (vi.mocked(startup.startLocalWorkbench).mock.calls.length || vi.mocked(pty.spawnAgentPty).mock.calls.length) ? readyView : null);
}

it('sends once to an offline Local recipient without a terminal Wake dialog or pasted input', async () => {
  const { submit, spawn, terminalSubmit } = await mount();
  await send();
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(submit.mock.calls[0][0]).toMatchObject({ agentId: localId, projectRoot: root, text: 'Keep this exact original text.' });
  expect(screen.queryByRole('dialog', { name: 'Wake sleeping agent?' })).not.toBeInTheDocument();
  expect(spawn).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
});

it('wakes only the terminal on the first mixed Send, then delivers to Local exactly once on the second', async () => {
  const { submit, spawn, terminalSubmit } = await mount([localId, terminalId]);
  await userEvent.click(screen.getByTestId('ib-target-pill'));
  const picker = await screen.findByTestId('broadcast-target-popover');
  await userEvent.click(within(picker).getByText('Silver'));
  await userEvent.click(screen.getByTestId('broadcast-confirm'));
  await send('Mixed original');
  const dialog = await screen.findByRole('dialog', { name: 'Wake sleeping agent?' });
  expect(dialog).toHaveTextContent('Silver is sleeping');
  expect(dialog).not.toHaveTextContent('Copper');
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Wake up' }));
  await waitFor(() => expect(spawn).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  expect(spawn.mock.calls[0][0].agentId).toBe(terminalId);
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
  await userEvent.click(screen.getByTestId('ib-send'));
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(submit.mock.calls[0][0]).toMatchObject({ agentId: localId, text: 'Mixed original' });
  expect(terminalSubmit).toHaveBeenCalledOnce();
});

it.each([false, true])('keeps the existing takeover confirmation on Local send (confirmed=%s)', async (confirmed) => {
  const { submit, spawn } = await mount();
  submit.mockRejectedValueOnce(new pty.AgentSessionLeaseConflictError(conflict));
  spawn.mockRejectedValueOnce(new pty.AgentSessionLeaseConflictError(conflict));
  await send('Takeover original');
  const dialog = await screen.findByRole('dialog', { name: 'Copper is running in another window' });
  expect(submit).toHaveBeenCalledOnce();
  const sent = violetComposerSentHistory(root).at(-1)!;
  expect(submit.mock.calls[0][0].soulInput).toEqual({ messageId: sent.id, createdAt: sent.timestamp, origin: 'user',
    originalText: 'Takeover original', eligibilityText: 'Takeover original', targetAgentIds: [localId] });
  await userEvent.click(within(dialog).getByRole('button', { name: confirmed ? 'Yes, takeover here.' : 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Copper is running in another window' })).not.toBeInTheDocument());
  if (confirmed) {
    await waitFor(() => expect(submit).toHaveBeenCalledTimes(2));
    expect(submit.mock.calls[1][0]).toEqual(submit.mock.calls[0][0]);
    expect(spawn.mock.calls[1][0]).toMatchObject({ agentId: localId, cli: 'kota-local', takeover: true });
  } else {
    expect(submit).toHaveBeenCalledOnce(); expect(spawn).toHaveBeenCalledOnce();
  }
});

it('routes a stopped terminal saved as Local before formatting or the terminal Wake path', async () => {
  const { targets, submit, terminalSubmit } = await mount([localId, terminalId]);
  targets.mockResolvedValue([terminalId]);
  await userEvent.click(screen.getByTestId(`chip-${terminalId}`));
  await send('First input after saving Local');
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(submit.mock.calls[0][0]).toMatchObject({ agentId: terminalId, text: 'First input after saving Local' });
  expect(targets).toHaveBeenCalledWith(root, [terminalId]);
  expect(screen.queryByRole('dialog', { name: 'Wake sleeping agent?' })).not.toBeInTheDocument();
  expect(terminalSubmit).not.toHaveBeenCalled();
});

it('keeps the original two-Send Wake path for a stopped Local saved as a terminal', async () => {
  const { targets, submit, terminalSubmit, spawn } = await mount();
  targets.mockResolvedValue([]);
  vi.mocked(pty.resolveProjectAgentLaunch).mockResolvedValue({ status: 'ready', request: { ...agent(localId), cli: 'codex' } });
  await send('First terminal input');
  const dialog = await screen.findByRole('dialog', { name: 'Wake sleeping agent?' });
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Wake up' }));
  await waitFor(() => expect(spawn).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.getByTestId('ib-send')).not.toBeDisabled());
  expect(terminalSubmit).not.toHaveBeenCalled();
  await userEvent.click(screen.getByTestId('ib-send'));
  await waitFor(() => expect(terminalSubmit).toHaveBeenCalledOnce());
  expect(submit).not.toHaveBeenCalled();
});

it('decodes the existing lease conflict from the Local IPC without obscuring other failures', async () => {
  const invoke = vi.mocked(core.invoke).mockRejectedValue(`KOTA_AGENT_SESSION_LEASE_CONFLICT:${JSON.stringify(conflict)}`);
  const input = { projectRoot: root, agentId: localId, messageId: 'composer-fixture', createdAt: '2026-10-01T00:00:00Z', private: false, text: 'Original' };
  await expect(local.submitLocal(input)).rejects.toBeInstanceOf(pty.AgentSessionLeaseConflictError);
  invoke.mockRejectedValue('fixture disk failure');
  await expect(local.submitLocal(input)).rejects.toBe('fixture disk failure');
});

it('resumes Local on double click and reuses its session when hiding and reopening from the shortcut', async () => {
  const read = readReadyWorkbench();
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession');
  const { workbenchStart: spawn, submit, terminalSubmit } = await mount();
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  const panel = await screen.findByLabelText('Copper workbench');
  expect(read).toHaveBeenCalledWith(root, localId, undefined);
  expect(within(panel).queryByRole('button', { name: 'Start session' })).not.toBeInTheDocument();
  expect(spawn).toHaveBeenCalledOnce();
  expect(spawn.mock.calls[0][0]).toMatchObject({ agentId: localId, cli: 'kota-local' });
  await userEvent.click(within(panel).getByRole('button', { name: 'Hide workbench' }));
  expect(screen.queryByLabelText('Copper workbench')).not.toBeInTheDocument();
  await userEvent.keyboard('{Meta>}1{/Meta}');
  await screen.findByLabelText('Copper workbench');
  await userEvent.keyboard('{Meta>}1{/Meta}');
  expect(screen.queryByLabelText('Copper workbench')).not.toBeInTheDocument();
  expect(spawn).toHaveBeenCalledOnce();
  expect(fresh).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
});

it('Cmd+0 hides the Local workbench and returns to the draft without restarting its session', async () => {
  readReadyWorkbench();
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession');
  const { workbenchStart: spawn, submit, terminalSubmit } = await mount();
  const composer = screen.getByTestId('input-field');
  await userEvent.type(composer, 'Keep this unsent draft');
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  await screen.findByLabelText('Copper workbench');
  await waitFor(() => expect(spawn).toHaveBeenCalledOnce());

  await userEvent.keyboard('{Meta>}0{/Meta}');
  expect(screen.queryByLabelText('Copper workbench')).not.toBeInTheDocument();
  await waitFor(() => expect(composer).toHaveFocus());
  expect(composer).toHaveTextContent('Keep this unsent draft');

  await userEvent.keyboard('{Meta>}1{/Meta}');
  await screen.findByLabelText('Copper workbench');
  expect(spawn).toHaveBeenCalledOnce();
  expect(fresh).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
});

it.each(['Meta', 'Control'])('resumes an offline Local directly from the %s seat shortcut', async (modifier) => {
  readReadyWorkbench();
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession');
  const { workbenchStart: spawn, submit, terminalSubmit } = await mount();
  await userEvent.keyboard(`{${modifier}>}1{/${modifier}}`);
  await screen.findByLabelText('Copper workbench');
  await waitFor(() => expect(spawn).toHaveBeenCalledOnce());
  expect(spawn.mock.calls[0][0]).toMatchObject({ agentId: localId, cli: 'kota-local' });
  expect(fresh).not.toHaveBeenCalled();
  expect(submit).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
});

it('keeps opening the room queue read-only rather than resuming an offline Local', async () => {
  vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const sent = emitVioletComposerSent({ projectRoot: root, text: 'A queued original', targetAgentIds: [localId], privacy: false })!;
  const state: pty.VioletRoomState = {
    messages: [], localPendingDeliveries: [{ messageId: sent.id, targetAgentId: localId }],
    sources: [], workEvents: [], agentBusReceipts: [], syncedAt: new Date().toISOString(),
    rawLogDir: `${root}/project-memory/raw_logs`, chathistoryDir: `${root}/project-memory/chathistory`,
  };
  vi.spyOn(pty, 'readVioletRoomCache').mockResolvedValue(state);
  vi.spyOn(pty, 'syncVioletRoom').mockResolvedValue(state);
  const { workbenchStart: spawn } = await mount();
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  await userEvent.click(await screen.findByRole('button', { name: 'Queued for Copper. Open message queue' }));
  await screen.findByLabelText('Copper workbench');
  expect(spawn).not.toHaveBeenCalled();
});

it('coalesces a double click and shortcut while Local resume is still resolving', async () => {
  vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const { workbenchStart: spawn } = await mount();
  let resolve!: (value: pty.ProjectAgentLaunchResolution) => void;
  const launch = vi.mocked(pty.resolveProjectAgentLaunch).mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  await userEvent.keyboard('{Meta>}1{/Meta}');
  expect(launch).toHaveBeenCalledOnce(); expect(spawn).not.toHaveBeenCalled();
  resolve({ status: 'ready', request: agent(localId) });
  await screen.findByLabelText('Copper workbench');
  expect(spawn).toHaveBeenCalledOnce();
});

it.each([false, true])('keeps takeover confirmation when opening Local (confirmed=%s)', async (confirmed) => {
  vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const { workbenchStart: spawn } = await mount();
  spawn.mockRejectedValueOnce(new pty.AgentSessionLeaseConflictError(conflict));
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  const dialog = await screen.findByRole('dialog', { name: 'Copper is running in another window' });
  // Approved fix: the panel is visible even while a takeover is pending.
  expect(screen.getByLabelText('Copper workbench')).toBeInTheDocument();
  await userEvent.click(within(dialog).getByRole('button', { name: confirmed ? 'Yes, takeover here.' : 'Cancel' }));
  if (confirmed) {
    await screen.findByLabelText('Copper workbench');
    expect(spawn).toHaveBeenCalledTimes(2);
    expect(spawn.mock.calls[1][0]).toMatchObject({ agentId: localId, cli: 'kota-local', takeover: true });
  } else {
    expect(spawn).toHaveBeenCalledOnce();
    expect(screen.getByLabelText('Copper workbench')).toBeInTheDocument();
    await waitFor(() => expect(within(screen.getByLabelText('Copper workbench')).getByRole('button', { name: 'Start session' })).toBeEnabled());
    expect(screen.queryByText('fixture Local launch failure')).not.toBeInTheDocument();
  }
});

it('reports a failed Local open and lets the next explicit double click retry without Fresh', async () => {
  vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession');
  const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
  const { workbenchStart: spawn } = await mount();
  spawn.mockRejectedValueOnce(new Error('fixture Local launch failure'));
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  // Failure used to hide the only repair controls; approved B′/F keeps them open.
  await waitFor(() => expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Session could not start.'));
  expect(alert).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Copper workbench')).toBeInTheDocument();
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  await screen.findByLabelText('Copper workbench');
  expect(spawn).toHaveBeenCalledTimes(2); expect(fresh).not.toHaveBeenCalled();
});

it('opens B′ before Link login, routes Sign in through a closed Tavern, and resumes the original model on return', async () => {
  const read = vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const { workbenchStart: spawn } = await mount();
  vi.mocked(auth.readLocalAuthAvailability).mockResolvedValue({ schema: 1, summary: 'not_ready', providers: [{ providerId: 'fixture', status: 'not_signed_in' }] });
  vi.mocked(auth.readLocalModelCatalog).mockResolvedValue({ schema: 1, providers: [{ providerId: 'fixture', linked: false, models: [{ modelId: 'base', name: 'Base', thinkingLevels: ['off'] }] }] });
  vi.mocked(local.checkLocalLaunch).mockResolvedValue({ status: 'blocked', code: 'provider_not_signed_in', providerId: 'fixture' });
  spawn.mockImplementation(async () => {finishStartup('failed',{code:'provider_not_signed_in',providerId:'fixture',stage:'credential_status'});throw 'provider_not_signed_in (fixture)';});
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  const panel = await screen.findByLabelText('Copper workbench');
  await within(panel).findByText('No provider is signed in.');
  // The approved String picker replaced the old native select. It also offers
  // Jev routing, while unsigned concrete provider models stay out of its list.
  expect(within(panel).getByRole('button', { name: 'Local model' })).toBeEnabled();
  await userEvent.click(within(panel).getByRole('button', { name: 'Local model' }));
  const picker = screen.getByRole('dialog', { name: 'Choose a Local model' });
  expect(within(picker).queryByRole('option', { name: /Base/ })).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByTestId('tavern-page')).not.toBeInTheDocument();
  await userEvent.click(within(panel).getByRole('button', { name: 'Sign in' }));
  const tavern = await screen.findByTestId('tavern-page');
  expect(within(tavern).getByRole('button', { name: 'Link' })).toHaveClass('active');
  expect(within(tavern).getByTestId('tavern-kota-local')).toBeInTheDocument();
  // A Link return repeats the authoritative fresh startup gate, without an extra
  // launch_check helper. Refocusing must preserve the Link return counter.
  await userEvent.click(within(tavern).getByRole('button', { name: 'Back to room' }));
  await waitFor(() => expect(spawn).toHaveBeenCalledTimes(2));
  await within(panel).findByText('No provider is signed in.');
  expect(spawn).toHaveBeenCalledTimes(2);
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  await waitFor(() => expect(spawn).toHaveBeenCalledTimes(3));
  await within(panel).findByText('No provider is signed in.');
  await userEvent.click(within(panel).getByRole('button', { name: 'Sign in' }));
  const returnedTavern = await screen.findByTestId('tavern-page');
  vi.mocked(auth.readLocalAuthAvailability).mockResolvedValue({ schema: 1, summary: 'ready', providers: [{ providerId: 'fixture', status: 'signed_in', authType: 'api_key' }] });
  vi.mocked(local.checkLocalLaunch).mockResolvedValue({ status: 'ok' });
  spawn.mockImplementationOnce(async () => { finishStartup('ready'); read.mockResolvedValue(readyView); return { agentId: localId, outputEvent: 'fixture/output', exitEvent: 'fixture/exit', statusEvent: 'fixture/status', workEvent: 'fixture/work' }; });
  await userEvent.click(within(returnedTavern).getByRole('button', { name: 'Back to room' }));
  await waitFor(() => expect(spawn).toHaveBeenCalledTimes(4));
  expect(screen.getByLabelText('Copper workbench')).toBeInTheDocument();
  expect(spawn.mock.calls[3][0].cli).toBe('kota-local');
});

it('delegates workbench New session to the existing Fresh confirmation and keeps the card on Cancel', async () => {
  vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const fresh = vi.spyOn(pty, 'startFreshProjectAgentSession');
  const { workbenchStart: spawn } = await mount();
  await userEvent.dblClick(screen.getByTestId(`chip-${localId}`));
  const panel = await screen.findByLabelText('Copper workbench');
  await userEvent.click(within(panel).getByRole('button', { name: 'New session' }));
  const dialog = await screen.findByRole('dialog', { name: 'Start fresh session for Copper?' });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(screen.getByLabelText('Copper workbench')).toBeInTheDocument();
  expect(spawn).toHaveBeenCalledOnce(); expect(fresh).not.toHaveBeenCalled();
});

it.each([false, true])('sends workbench Add through the room flow with a fixed recipient (queued=%s)', async queued => {
  const read = readReadyWorkbench();
  if (queued) read.mockResolvedValue({ ...readyView, state: 'busy', queued: 1, queueTotal: 1,
    queue: [{ kotaId: 'older#' + localId, customType: 'kota.user', senderId: null, createdAt: '2026-10-05T00:00:00Z',
      state: 'accepted', summary: 'Existing queued task', truncated: false }] });
  const { submit, terminalSubmit } = await mount([localId, terminalId]);
  await userEvent.type(screen.getByTestId('input-field'), 'Keep the main composer draft');
  await userEvent.dblClick(screen.getByTestId('chip-' + localId));
  const panel = await screen.findByLabelText('Copper workbench');
  await userEvent.click(screen.getByTestId('chip-' + terminalId));
  await userEvent.click(within(panel).getByRole('button', { name: 'Add' }));
  const editor = within(panel).getByRole('region', { name: 'New message' });
  expect(within(editor).getByRole('button', { name: 'Copper' })).toBeDisabled();
  expect(within(editor).queryByRole('button', { name: 'Silver' })).not.toBeInTheDocument();
  const action = within(editor).getByRole('button', { name: queued ? 'Queue message' : 'Send message' });
  expect(action).toBeDisabled();
  await userEvent.type(within(editor).getByTestId('input-field'), 'A new workbench task');
  await userEvent.click(action);
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  await waitFor(() => expect(within(panel).queryByRole('region', { name: 'New message' })).not.toBeInTheDocument());
  const history = violetComposerSentHistory(root);
  expect(history).toHaveLength(1);
  expect(history[0]).toMatchObject({ projectRoot: root, text: 'A new workbench task', targetAgentIds: [localId], privacy: false });
  expect(submit.mock.calls[0][0]).toMatchObject({ projectRoot: root, agentId: localId,
    messageId: history[0].id, text: 'A new workbench task', private: false });
  expect(terminalSubmit).not.toHaveBeenCalled();
  expect(screen.getByTestId('input-field')).toHaveTextContent('Keep the main composer draft');
  expect(vi.mocked(local.controlLocalWorkbench).mock.calls.some(([, request]) => typeof request.type === 'string' && request.type.startsWith('queue_'))).toBe(false);
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  expect(await screen.findByText('A new workbench task')).toBeInTheDocument();
});

it('cancels Add without sending or changing an existing queued item', async () => {
  const read = readReadyWorkbench();
  const older = { kotaId: 'older#' + localId, customType: 'kota.user', senderId: null, createdAt: '2026-10-05T00:00:00Z',
    state: 'accepted' as const, summary: 'Existing queued task', truncated: false };
  read.mockResolvedValue({ ...readyView, queueTotal: 1, queued: 1, queue: [older] });
  const { submit } = await mount();
  await userEvent.dblClick(screen.getByTestId('chip-' + localId));
  const panel = await screen.findByLabelText('Copper workbench');
  await userEvent.click(within(panel).getByRole('button', { name: 'Add' }));
  const editor = within(panel).getByRole('region', { name: 'New message' });
  await userEvent.type(within(editor).getByTestId('input-field'), 'Never send this');
  await userEvent.click(within(editor).getByRole('button', { name: 'Cancel' }));
  expect(within(panel).queryByRole('region', { name: 'New message' })).not.toBeInTheDocument();
  expect(within(panel).getByText('Existing queued task')).toBeInTheDocument();
  expect(submit).not.toHaveBeenCalled(); expect(violetComposerSentHistory(root)).toHaveLength(0);
  expect(vi.mocked(local.controlLocalWorkbench).mock.calls.some(([, request]) => typeof request.type === 'string' && request.type.startsWith('queue_'))).toBe(false);
});

it('uses normal Local sending from an offline workbench without the terminal Wake path', async () => {
  vi.spyOn(local, 'readLocalWorkbench').mockResolvedValue(null);
  const previous = emitVioletComposerSent({ projectRoot: root, text: 'Previously queued', targetAgentIds: [localId], privacy: false })!;
  const state: pty.VioletRoomState = { messages: [], localPendingDeliveries: [{ messageId: previous.id, targetAgentId: localId }],
    sources: [], workEvents: [], agentBusReceipts: [], syncedAt: '2026-10-05T00:00:00Z',
    rawLogDir: root + '/project-memory/raw_logs', chathistoryDir: root + '/project-memory/chathistory' };
  vi.spyOn(pty, 'readVioletRoomCache').mockResolvedValue(state);
  vi.spyOn(pty, 'syncVioletRoom').mockResolvedValue(state);
  const { submit, spawn, terminalSubmit } = await mount();
  await userEvent.click(screen.getByTestId('group-chat-trigger'));
  await userEvent.click(await screen.findByRole('button', { name: 'Queued for Copper. Open message queue' }));
  const panel = await screen.findByLabelText('Copper workbench');
  await userEvent.click(within(panel).getByRole('button', { name: 'Add' }));
  const editor = within(panel).getByRole('region', { name: 'New message' });
  await userEvent.type(within(editor).getByTestId('input-field'), 'Wake through normal Local submit');
  await userEvent.click(within(editor).getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(submit).toHaveBeenCalledOnce());
  expect(submit.mock.calls[0][0]).toMatchObject({ agentId: localId, projectRoot: root, text: 'Wake through normal Local submit' });
  expect(screen.queryByRole('dialog', { name: 'Wake sleeping agent?' })).not.toBeInTheDocument();
  expect(spawn).not.toHaveBeenCalled(); expect(terminalSubmit).not.toHaveBeenCalled();
  expect(violetComposerSentHistory(root)).toHaveLength(2);
});


it('wires composer overrides through Local submit and takeover, keeps them sticky, and omits them again on Auto', async () => {
  const autoRoute: AutoRouteGroup = { threshold: .5, defaultModel: 'model-1', models: [
    { id: 'model-1', providerId: 'fixture', modelId: 'base', thinkingLevel: 'off', intent: 'One' },
    { id: 'model-2', providerId: 'fixture', modelId: 'other', thinkingLevel: 'high', intent: 'Two' },
  ] };
  const { submit, spawn } = await mount([localId, terminalId], [], { model: 'kota/auto', thinkingLevel: 'off', autoRoute });
  await waitFor(() => expect(screen.getByRole('button', { name: 'Auto String routing: Auto' })).toBeEnabled());
  fireEvent.pointerEnter(screen.getByRole('button', { name: 'Auto String routing: Auto' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'fixture / other • high' }));
  submit.mockRejectedValueOnce(new pty.AgentSessionLeaseConflictError(conflict));
  spawn.mockRejectedValueOnce(new pty.AgentSessionLeaseConflictError(conflict));
  await send('Manual takeover');
  const dialog = await screen.findByRole('dialog', { name: 'Copper is running in another window' });
  expect(submit.mock.calls[0][0]).toMatchObject({ agentId: localId, routeModel: 'model-2', text: 'Manual takeover' });
  await userEvent.click(within(dialog).getByRole('button', { name: 'Yes, takeover here.' }));
  await waitFor(() => expect(submit).toHaveBeenCalledTimes(2));
  expect(submit.mock.calls[1][0]).toEqual(submit.mock.calls[0][0]);
  await send('Still manual'); await waitFor(() => expect(submit).toHaveBeenCalledTimes(3));
  expect(submit.mock.calls[2][0].routeModel).toBe('model-2');
  fireEvent.pointerEnter(screen.getByRole('button', { name: 'Manual model: other • high' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'Auto' }));
  await send('Automatic again'); await waitFor(() => expect(submit).toHaveBeenCalledTimes(4));
  expect(submit.mock.calls[3][0]).not.toHaveProperty('routeModel');
  await userEvent.click(screen.getByTestId(`chip-${terminalId}`));
  await waitFor(() => expect(screen.queryByTestId('composer-model-controls')).not.toBeInTheDocument());
});
