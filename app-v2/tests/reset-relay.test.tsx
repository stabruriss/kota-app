import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { StrictMode } from 'react';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../src/App';
import { RightColumn } from '../src/chrome/RightColumn';
import { refreshGithubAccounts } from '../src/lib/github-account-flow';
import * as ptyClient from '../src/pty-client';
import {
  RESET_RELAY_COMPLETE_HOLD_MS,
  RESET_RELAY_ELEMENT_ID,
  RESET_RELAY_HANDOFF_KEY,
  RESET_RELAY_HARD_TIMEOUT_MS,
  RESET_RELAY_STAGE_LABELS,
  RESET_RELAY_TITLE,
  describeResetRelayStage,
  dismissResetRelayImmediately,
  isResetRelayActive,
  resolveResetRelayAction,
} from '../src/lib/reset-relay';
import {
  BYTES_PER_GIB,
  BYTES_PER_MIB,
  recordSystemLoadPageLoadedAt,
  reloadKotaWindow,
  type SystemLoadSample,
} from '../src/lib/system-load';

const INDEX_HTML_PATH = resolve(__dirname, '../index.html');
const INDEX_HTML_RAW = readFileSync(INDEX_HTML_PATH, 'utf8');

function extractIndexRelayArtifacts(html: string): {
  relayMarkup: string;
  bootstrapScript: string;
  systemLoadLinkIndex: number;
  headCloseIndex: number;
  relayIndex: number;
  bootstrapIndex: number;
  moduleIndex: number;
} {
  const relayMatch = html.match(
    /(<div id="kota-reset-relay"[\s\S]*?<\/section>\s*<\/div>)\s*<script id="kota-reset-relay-bootstrap">/,
  );
  const scriptMatch = html.match(/<script id="kota-reset-relay-bootstrap">([\s\S]*?)<\/script>/);
  if (!relayMatch || !scriptMatch) {
    throw new Error('Failed to extract reset relay markup/script from app-v2/index.html');
  }
  return {
    relayMarkup: relayMatch[1]!,
    bootstrapScript: scriptMatch[1]!,
    systemLoadLinkIndex: html.indexOf('<link rel="stylesheet" href="/src/styles/system-load.css" />'),
    headCloseIndex: html.indexOf('</head>'),
    relayIndex: html.indexOf('<div id="kota-reset-relay"'),
    bootstrapIndex: html.indexOf('<script id="kota-reset-relay-bootstrap">'),
    moduleIndex: html.indexOf('<script type="module" src="/src/main.tsx"></script>'),
  };
}

function mountIndexRelayDom(): HTMLElement {
  const { relayMarkup } = extractIndexRelayArtifacts(INDEX_HTML_RAW);
  document.getElementById(RESET_RELAY_ELEMENT_ID)?.remove();
  const holder = document.createElement('div');
  holder.innerHTML = relayMarkup;
  const relayEl = holder.firstElementChild as HTMLElement;
  document.body.appendChild(relayEl);
  return relayEl;
}

function runIndexBootstrapScript(): void {
  const { bootstrapScript } = extractIndexRelayArtifacts(INDEX_HTML_RAW);
  const fn = new Function(bootstrapScript);
  fn();
}

function readRelayDomState(el: HTMLElement | null = document.getElementById(RESET_RELAY_ELEMENT_ID)) {
  if (!el) return null;
  const card = el.querySelector<HTMLElement>('.reset-relay-card');
  const title = el.querySelector<HTMLElement>('.hud-title')?.textContent ?? null;
  const label = el.querySelector<HTMLElement>('[data-relay-label]')?.textContent ?? null;
  const count = el.querySelector<HTMLElement>('[data-relay-count]')?.textContent ?? null;
  const segLit = Array.from(el.querySelectorAll<HTMLElement>('.relay-seg')).map(
    (seg) => seg.getAttribute('data-lit') === 'true',
  );
  const segHead = Array.from(el.querySelectorAll<HTMLElement>('.relay-seg')).map(
    (seg) => seg.getAttribute('data-head') === 'true',
  );
  const tickLit = Array.from(el.querySelectorAll<HTMLElement>('.relay-ticks i')).map(
    (tick) => tick.getAttribute('data-lit') === 'true',
  );
  return {
    hidden: el.hidden,
    ariaLabel: card?.getAttribute('aria-label') ?? null,
    role: card?.getAttribute('role') ?? null,
    title,
    label,
    count,
    segLit,
    segHead,
    tickLit,
  };
}

function makeWorkspace(projectId: string, agentIds: readonly string[] = ['agent-alpha']): ptyClient.WorkspaceProject {
  const localRoot = `/tmp/kota-relay/${projectId}`;
  return {
    projectId,
    repoFullName: `mock/${projectId}`,
    remoteUrl: `https://github.com/mock/${projectId}.git`,
    githubHtmlUrl: `https://github.com/mock/${projectId}`,
    defaultBranch: 'main',
    baseRef: 'origin/main',
    localRoot,
    localRootBytes: 0,
    sourceDir: `${localRoot}/source`,
    sourceDirBytes: 0,
    sharedDir: `${localRoot}/project-memory`,
    rulesDir: `${localRoot}/project-rules`,
    agents: agentIds.map((agentId) => ({
      agentId,
      cli: 'codex',
      cwd: `${localRoot}/.agent-workspaces/${agentId}`,
      projectRoot: localRoot,
      worktreeRoot: `${localRoot}/.agent-workspaces/${agentId}/project-files`,
      sharedDir: `${localRoot}/project-memory`,
      rulesDir: `${localRoot}/project-rules`,
      adapterPath: `${localRoot}/.agent-workspaces/${agentId}/AGENTS.md`,
      args: ['--model', 'default'],
      projectId,
      projectBaseRef: 'origin/main',
    })),
  };
}

function makeIdentity(agentId: string, displayName = agentId): ptyClient.ProjectAgentIdentity {
  return {
    agentId,
    displayName,
    sourceHeroId: 'hero-dex',
    status: 'active',
    provider: 'codex',
    avatarId: null,
  };
}

function makeDetail(agentId: string, projectRoot: string, displayName = agentId): ptyClient.ProjectAgentDetail {
  const projectId = projectRoot.split('/').at(-1) ?? 'relay-room';
  return {
    agentId,
    displayName,
    sourceHeroId: 'hero-dex',
    sourceHeroName: 'Dex',
    projectId,
    projectName: projectId,
    cli: 'codex',
    provider: 'codex',
    model: 'default',
    effort: 'xhigh',
    avatarId: null,
    skills: [],
    args: ['--model', 'default'],
    ghost: '',
    adapterPath: `${projectRoot}/.agent-workspaces/${agentId}/AGENTS.md`,
    shellPath: `${projectRoot}/.agent-workspaces/${agentId}/SHELL.yaml`,
    agentYamlPath: `${projectRoot}/.agent-workspaces/${agentId}/agent.yaml`,
    status: 'active',
    inviteEligibility: {
      eligible: true,
      proposedHeroId: 'hero-dex',
      proposedDisplayName: displayName,
    },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 },
    forkable: true,
    dirty: false,
    dirtySummary: '',
  };
}

function makeSample(overrides: Partial<SystemLoadSample> = {}): SystemLoadSample {
  return {
    atMs: 100_000,
    intervalMs: 30_000,
    totalRamBytes: 16 * BYTES_PER_GIB,
    compressedBytes: 2.4 * BYTES_PER_GIB,
    kotaPageBytes: 1.2 * BYTES_PER_GIB,
    swapBytesPerSec: 0.5 * BYTES_PER_MIB,
    ...overrides,
  };
}

async function flushMicrotasks(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function toggleRockerSwitch(label: 'Good Night' | 'Back to Kota'): void {
  const rocker = screen.getByRole('switch', { name: label });
  fireEvent.click(rocker);
  fireEvent.click(rocker);
}

beforeEach(async () => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  dismissResetRelayImmediately();
  delete window.__KOTA_RESET_RELAY__;
  document.getElementById(RESET_RELAY_ELEMENT_ID)?.remove();
  window.sessionStorage.clear();
  vi.spyOn(ptyClient, 'githubAccountsRead').mockResolvedValue({
    defaultLogin: 'mock',
    accounts: [{ login: 'mock', default: true, state: 'ok', source: 'gh-cli' }],
  });
  vi.spyOn(ptyClient, 'projectConnectionStatus').mockResolvedValue({
    kind: 'github',
    accountLogin: 'mock',
    accountState: 'ok',
  });
  await refreshGithubAccounts();
});

afterEach(() => {
  dismissResetRelayImmediately();
  delete window.__KOTA_RESET_RELAY__;
  document.getElementById(RESET_RELAY_ELEMENT_ID)?.remove();
  window.sessionStorage.clear();
  vi.useRealTimers();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe('Reset Kota two-phase relay · pure state & index.html bootstrap', () => {
  it('maps stages 1..4 to approved copy and resolves every convergence branch without extra guards', () => {
    expect(RESET_RELAY_TITLE).toBe('RESET KOTA');
    expect(RESET_RELAY_STAGE_LABELS).toEqual([
      'CLOSING PAGE',
      'LOADING UI',
      'OPENING ROOM',
      'ROOM READY',
    ]);

    expect(describeResetRelayStage(1)).toEqual({
      stage: 1,
      title: 'RESET KOTA',
      label: 'CLOSING PAGE',
      fraction: '1/4',
      litSegments: [true, false, false, false],
      headIndex: 0,
    });
    expect(describeResetRelayStage(2)).toEqual({
      stage: 2,
      title: 'RESET KOTA',
      label: 'LOADING UI',
      fraction: '2/4',
      litSegments: [true, true, false, false],
      headIndex: 1,
    });
    expect(describeResetRelayStage(3)).toEqual({
      stage: 3,
      title: 'RESET KOTA',
      label: 'OPENING ROOM',
      fraction: '3/4',
      litSegments: [true, true, true, false],
      headIndex: 2,
    });
    expect(describeResetRelayStage(4)).toEqual({
      stage: 4,
      title: 'RESET KOTA',
      label: 'ROOM READY',
      fraction: '4/4',
      litSegments: [true, true, true, true],
      headIndex: 3,
    });

    expect(
      resolveResetRelayAction({
        hasTauri: false,
        workspaceInitialLoadComplete: true,
        hasActiveWorkspace: false,
        agentsHydrating: false,
        hasHydrationFailure: false,
      }),
    ).toEqual({ kind: 'dismiss_immediately' });

    expect(
      resolveResetRelayAction({
        hasTauri: true,
        workspaceInitialLoadComplete: false,
        hasActiveWorkspace: false,
        agentsHydrating: false,
        hasHydrationFailure: false,
      }),
    ).toEqual({ kind: 'hold_stage_2' });

    // Protection 1: empty project -> dismiss immediately.
    expect(
      resolveResetRelayAction({
        hasTauri: true,
        workspaceInitialLoadComplete: true,
        hasActiveWorkspace: false,
        agentsHydrating: false,
        hasHydrationFailure: false,
      }),
    ).toEqual({ kind: 'dismiss_immediately' });

    // Stage 3: active project while roster hydration is in flight.
    expect(
      resolveResetRelayAction({
        hasTauri: true,
        workspaceInitialLoadComplete: true,
        hasActiveWorkspace: true,
        agentsHydrating: true,
        hasHydrationFailure: false,
      }),
    ).toEqual({ kind: 'advance_stage_3' });

    // Protection 2: hydration failure converges !agentsHydrating -> dismiss immediately.
    expect(
      resolveResetRelayAction({
        hasTauri: true,
        workspaceInitialLoadComplete: true,
        hasActiveWorkspace: true,
        agentsHydrating: false,
        hasHydrationFailure: true,
      }),
    ).toEqual({ kind: 'dismiss_immediately' });

    // Stage 4: hydration confirmed -> flash 4/4 ROOM READY and dismiss.
    expect(
      resolveResetRelayAction({
        hasTauri: true,
        workspaceInitialLoadComplete: true,
        hasActiveWorkspace: true,
        agentsHydrating: false,
        hasHydrationFailure: false,
      }),
    ).toEqual({ kind: 'complete_stage_4' });
  });

  it('links system-load.css once in head with zero inline style and places static hidden relay markup before main.tsx', () => {
    const {
      systemLoadLinkIndex,
      headCloseIndex,
      relayIndex,
      bootstrapIndex,
      moduleIndex,
    } = extractIndexRelayArtifacts(INDEX_HTML_RAW);
    expect(systemLoadLinkIndex).toBeGreaterThan(0);
    expect(headCloseIndex).toBeGreaterThan(systemLoadLinkIndex);
    expect(relayIndex).toBeGreaterThan(headCloseIndex);
    expect(bootstrapIndex).toBeGreaterThan(relayIndex);
    expect(moduleIndex).toBeGreaterThan(bootstrapIndex);
    expect(INDEX_HTML_RAW).not.toContain('<style');

    const rightColumnSource = readFileSync(resolve(__dirname, '../src/chrome/RightColumn.tsx'), 'utf8');
    expect(rightColumnSource).not.toContain('system-load.css');

    const systemLoadCss = readFileSync(resolve(__dirname, '../src/styles/system-load.css'), 'utf8');
    expect(systemLoadCss).toContain('.reset-relay-layer');
    expect(systemLoadCss).toContain('.reset-relay-layer[hidden]');
    expect(systemLoadCss).toContain('.reset-relay-card');
    expect(systemLoadCss).not.toContain('backdrop-filter');
    expect(systemLoadCss).not.toContain('blur(');
    expect(systemLoadCss).not.toContain(':has(');

    const staticEl = mountIndexRelayDom();
    expect(readRelayDomState(staticEl)).toEqual({
      hidden: true,
      ariaLabel: 'RESET KOTA',
      role: 'status',
      title: 'RESET KOTA',
      label: 'LOADING UI',
      count: '2/4',
      segLit: [true, true, false, false],
      segHead: [false, true, false, false],
      tickLit: [true, true, false, false],
    });
  });

  it('has strictly zero cold-start side effects when sessionStorage marker is absent', () => {
    const relayEl = mountIndexRelayDom();
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBeNull();

    const setTimeoutSpy = vi.spyOn(window, 'setTimeout');
    const setIntervalSpy = vi.spyOn(window, 'setInterval');
    const rafSpy = vi.spyOn(window, 'requestAnimationFrame');
    const winListenerSpy = vi.spyOn(window, 'addEventListener');
    const docListenerSpy = vi.spyOn(document, 'addEventListener');

    const mutations: MutationRecord[] = [];
    const observer = new MutationObserver((records) => {
      mutations.push(...records);
    });
    observer.observe(document.documentElement, {
      subtree: true,
      attributes: true,
      childList: true,
      characterData: true,
    });

    runIndexBootstrapScript();
    observer.disconnect();

    expect(mutations).toHaveLength(0);
    expect(setTimeoutSpy).not.toHaveBeenCalled();
    expect(setIntervalSpy).not.toHaveBeenCalled();
    expect(rafSpy).not.toHaveBeenCalled();
    expect(winListenerSpy).not.toHaveBeenCalled();
    expect(docListenerSpy).not.toHaveBeenCalled();
    expect(relayEl.hidden).toBe(true);
    expect(window.__KOTA_RESET_RELAY__).toBeUndefined();
    expect(isResetRelayActive()).toBe(false);
  });

  it('deletes the handoff marker immediately on read and shows 2/4 LOADING UI once', () => {
    vi.useFakeTimers();
    const relayEl = mountIndexRelayDom();
    window.sessionStorage.setItem(RESET_RELAY_HANDOFF_KEY, '1');

    const setTimeoutSpy = vi.spyOn(window, 'setTimeout');
    runIndexBootstrapScript();

    // Marker is consumed immediately on read (no 30s TTL residue).
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBeNull();
    expect(isResetRelayActive()).toBe(true);
    expect(setTimeoutSpy).toHaveBeenCalledTimes(1);
    expect(setTimeoutSpy.mock.calls[0]?.[1]).toBe(RESET_RELAY_HARD_TIMEOUT_MS);

    expect(readRelayDomState(relayEl)).toEqual({
      hidden: false,
      ariaLabel: 'RESET KOTA',
      role: 'status',
      title: 'RESET KOTA',
      label: 'LOADING UI',
      count: '2/4',
      segLit: [true, true, false, false],
      segHead: [false, true, false, false],
      tickLit: [true, true, false, false],
    });

    // Simulate a subsequent normal reload without clicking Reset: marker is already gone, so cold-start stays hidden.
    dismissResetRelayImmediately();
    delete window.__KOTA_RESET_RELAY__;
    const freshEl = mountIndexRelayDom();
    runIndexBootstrapScript();
    expect(freshEl.hidden).toBe(true);
    expect(isResetRelayActive()).toBe(false);
  });

  it('writes marker, shows 1/4 CLOSING PAGE, and reloads only after double rAF with no pre-reload timeout', () => {
    mountIndexRelayDom();
    const rafQueue: FrameRequestCallback[] = [];
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafQueue.push(cb);
      return rafQueue.length;
    });
    const setTimeoutSpy = vi.spyOn(window, 'setTimeout');
    const reloadSpy = vi.fn();

    reloadKotaWindow(reloadSpy);

    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBe('1');
    expect(setTimeoutSpy).not.toHaveBeenCalled();
    expect(reloadSpy).not.toHaveBeenCalled();
    expect(readRelayDomState()).toEqual({
      hidden: false,
      ariaLabel: 'RESET KOTA',
      role: 'status',
      title: 'RESET KOTA',
      label: 'CLOSING PAGE',
      count: '1/4',
      segLit: [true, false, false, false],
      segHead: [true, false, false, false],
      tickLit: [true, false, false, false],
    });

    // 1st rAF: still has not reloaded yet (guarantees Stage 1 frame commits).
    expect(rafQueue).toHaveLength(1);
    rafQueue.shift()!(16);
    expect(reloadSpy).not.toHaveBeenCalled();
    expect(rafQueue).toHaveLength(1);

    // 2nd rAF: triggers reload.
    rafQueue.shift()!(32);
    expect(reloadSpy).toHaveBeenCalledTimes(1);

    // If #kota-reset-relay is absent, product code does not synthesize DOM and still reloads after double rAF.
    document.getElementById(RESET_RELAY_ELEMENT_ID)?.remove();
    window.sessionStorage.clear();
    const fallbackReloadSpy = vi.fn();
    reloadKotaWindow(fallbackReloadSpy);

    expect(document.getElementById(RESET_RELAY_ELEMENT_ID)).toBeNull();
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBe('1');
    expect(fallbackReloadSpy).not.toHaveBeenCalled();
    expect(rafQueue).toHaveLength(1);
    rafQueue.shift()!(48);
    expect(fallbackReloadSpy).not.toHaveBeenCalled();
    rafQueue.shift()!(64);
    expect(fallbackReloadSpy).toHaveBeenCalledTimes(1);
  });
});

describe('Reset Kota two-phase relay · RightColumn triggers & App startup flow', () => {
  it('writes the handoff marker only on the three approved triggerResetReload() exits', async () => {
    vi.useFakeTimers();
    const startMs = Date.now() + 60_000;
    vi.setSystemTime(startMs);
    recordSystemLoadPageLoadedAt(startMs);

    const rafQueue: FrameRequestCallback[] = [];
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafQueue.push(cb);
      return rafQueue.length;
    });

    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async () => () => {});
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(makeSample());
    vi.spyOn(ptyClient, 'emberExportEpisodes').mockResolvedValue(undefined);
    vi.spyOn(ptyClient, 'agentBusSend').mockImplementation(async (request) => ({
      eventId: request.eventId || `event-${request.target}`,
      targetAgentId: request.target,
      submitted: true,
      duplicate: false,
      skippedReason: null,
    }));
    vi.spyOn(ptyClient, 'emberPrepareDreams').mockResolvedValue({
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      archiveDir: '/tmp/Kota/dreams/archive',
      projectDreamsPath: '/tmp/Kota/dreams/project-dreams.md',
      projected: false,
    });

    let resolveConsolidation: (() => void) | null = null;
    vi.spyOn(ptyClient, 'emberConsolidateDreams').mockImplementation(
      () =>
        new Promise((res) => {
          resolveConsolidation = () =>
            res({
              accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
              entriesDir: '/tmp/Kota/dreams/entries',
              oldDreamsPath: '/tmp/Kota/dreams/old_dreams.md',
              promptPath: '$KOTA_HOME/heroes/system-ember/ember-dream-consolidate.md',
              processedEntryCount: 1,
              activeEntryCount: 1,
              archivedEntryCount: 0,
              updatedAt: new Date().toISOString(),
              error: null,
            });
        }),
    );

    // 1. Direct RESET KOTA (no drafts): ignored during 10s cooldown, writes marker + 1/4 past 10s.
    mountIndexRelayDom();
    const directView = render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    await flushMicrotasks();

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    const resetBtn = screen.getByRole('button', { name: 'RESET KOTA' });

    act(() => {
      vi.setSystemTime(startMs + 5_000);
    });
    fireEvent.click(resetBtn);
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBeNull();
    expect(readRelayDomState()?.hidden).toBe(true);

    act(() => {
      vi.setSystemTime(startMs + 10_001);
    });
    fireEvent.click(resetBtn);
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBe('1');
    expect(readRelayDomState()).toMatchObject({ hidden: false, label: 'CLOSING PAGE', count: '1/4' });
    directView.unmount();

    // 2. Draft confirm: Cancel writes nothing; Proceed writes marker + 1/4.
    window.sessionStorage.removeItem(RESET_RELAY_HANDOFF_KEY);
    rafQueue.length = 0;
    mountIndexRelayDom();
    const draftView = render(
      <RightColumn
        sceneKey="conversation"
        onOpenHotMem={vi.fn()}
        hasComposerDraft={() => true}
      />,
    );
    await flushMicrotasks();

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    fireEvent.click(screen.getByRole('button', { name: 'RESET KOTA' }));
    const confirmCancel = screen.getByRole('dialog', { name: 'Reset will clear composer drafts.' });
    fireEvent.click(within(confirmCancel).getByRole('button', { name: 'Cancel' }));
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBeNull();
    expect(readRelayDomState()?.hidden).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: 'RESET KOTA' }));
    const confirmProceed = screen.getByRole('dialog', { name: 'Reset will clear composer drafts.' });
    fireEvent.click(within(confirmProceed).getByRole('button', { name: 'Proceed' }));
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBe('1');
    expect(readRelayDomState()).toMatchObject({ hidden: false, label: 'CLOSING PAGE', count: '1/4' });
    draftView.unmount();

    // 3. Dream: midway Back to Kota does NOT write marker; finished Dream Back to Kota DOES write marker + 1/4.
    window.sessionStorage.removeItem(RESET_RELAY_HANDOFF_KEY);
    rafQueue.length = 0;
    mountIndexRelayDom();
    const workspace = makeWorkspace('dream-relay-room');
    const dreamView = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        onOpenHotMem={vi.fn()}
        dreamProjects={[
          {
            projectId: workspace.projectId,
            projectRoot: workspace.localRoot,
            projectName: workspace.projectId,
            agents: [{ id: 'agent-alpha', name: 'Alpha' }],
          },
        ]}
      />,
    );
    await flushMicrotasks();

    toggleRockerSwitch('Good Night');
    fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
    await flushMicrotasks();
    await act(async () => {
      vi.advanceTimersByTime(120_000);
      await Promise.resolve();
    });

    // Midway return while consolidation is pending: no marker, badge remains hidden.
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Back to Kota');
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBeNull();
    expect(readRelayDomState()?.hidden).toBe(true);

    await act(async () => {
      resolveConsolidation?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Complete a full Dream cycle -> Back to Kota writes marker + 1/4.
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Good Night');
    fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
    await flushMicrotasks();
    await act(async () => {
      vi.advanceTimersByTime(120_000);
      await Promise.resolve();
    });
    await act(async () => {
      resolveConsolidation?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Back to Kota');
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBe('1');
    expect(readRelayDomState()).toMatchObject({ hidden: false, label: 'CLOSING PAGE', count: '1/4' });
    dreamView.unmount();
  });

  it('progresses 2/4 -> 3/4 -> 4/4 -> dismiss, suppresses room-restore-overlay during relay only, and adds zero history reads', async () => {
    vi.useFakeTimers();
    const workspaceA = makeWorkspace('relay-room-a', ['agent-alpha']);
    const workspaceB = makeWorkspace('relay-room-b', ['agent-beta']);

    let resolveStatus!: (val: { active: ptyClient.WorkspaceProject | null }) => void;
    let resolveProjects!: (val: ptyClient.WorkspaceProject[]) => void;
    let resolveIdentitiesA!: (val: ptyClient.ProjectAgentIdentityListing) => void;
    let resolveIdentitiesB!: (val: ptyClient.ProjectAgentIdentityListing) => void;

    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockImplementation(
      () => new Promise((res) => { resolveStatus = res; }),
    );
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockImplementation(
      () => new Promise((res) => { resolveProjects = res; }),
    );
    vi.spyOn(ptyClient, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
    vi.spyOn(ptyClient, 'saveProjectAgentLayoutFile').mockResolvedValue();
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockImplementation((projectRoot) => {
      if (projectRoot === workspaceA.localRoot) {
        return new Promise((res) => { resolveIdentitiesA = res; });
      }
      return new Promise((res) => { resolveIdentitiesB = res; });
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation(async (req) =>
      makeDetail(req.agentId, req.projectRoot ?? workspaceA.localRoot, req.agentId === 'agent-beta' ? 'Beta' : 'Alpha'),
    );
    vi.spyOn(ptyClient, 'openWorkspaceProject').mockResolvedValue(workspaceB);
    const violetCacheSpy = vi.spyOn(ptyClient, 'readVioletRoomCache');

    // Simulate reload arrival with marker '1': index.html consumes marker and shows Stage 2.
    window.sessionStorage.setItem(RESET_RELAY_HANDOFF_KEY, '1');
    mountIndexRelayDom();
    runIndexBootstrapScript();
    expect(window.sessionStorage.getItem(RESET_RELAY_HANDOFF_KEY)).toBeNull();
    expect(readRelayDomState()).toMatchObject({ hidden: false, label: 'LOADING UI', count: '2/4' });

    // Mount <App /> inside <StrictMode> (matching src/main.tsx).
    const view = render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
    await flushMicrotasks();

    // Still waiting on workspaceStatus / listWorkspaceProjects -> holds Stage 2.
    expect(readRelayDomState()).toMatchObject({ hidden: false, label: 'LOADING UI', count: '2/4' });

    // Workspace opens while roster hydration is in flight -> advances to Stage 3 (OPENING ROOM).
    await act(async () => {
      resolveStatus({ active: workspaceA });
      resolveProjects([workspaceA, workspaceB]);
    });
    await flushMicrotasks();

    expect(readRelayDomState()).toEqual({
      hidden: false,
      ariaLabel: 'RESET KOTA',
      role: 'status',
      title: 'RESET KOTA',
      label: 'OPENING ROOM',
      count: '3/4',
      segLit: [true, true, true, false],
      segHead: [false, false, true, false],
      tickLit: [true, true, true, false],
    });

    // Advance past Stage.tsx's 500ms ROOM_RESTORE_OVERLAY_DELAY_MS: restore overlay MUST be suppressed during relay!
    await act(async () => {
      await vi.advanceTimersByTimeAsync(600);
    });
    expect(screen.queryByTestId('room-restore-overlay')).not.toBeInTheDocument();
    expect(violetCacheSpy).not.toHaveBeenCalled();

    // Settle roster hydration -> immediately shows 4/4 ROOM READY, then dismisses after hold.
    await act(async () => {
      resolveIdentitiesA({
        identities: [makeIdentity('agent-alpha', 'Alpha')],
        workspaceEntryCount: 1,
      });
    });
    await flushMicrotasks();
    await flushMicrotasks();

    expect(readRelayDomState()).toEqual({
      hidden: false,
      ariaLabel: 'RESET KOTA',
      role: 'status',
      title: 'RESET KOTA',
      label: 'ROOM READY',
      count: '4/4',
      segLit: [true, true, true, true],
      segHead: [false, false, false, true],
      tickLit: [true, true, true, true],
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(RESET_RELAY_COMPLETE_HOLD_MS);
    });
    expect(readRelayDomState()?.hidden).toBe(true);
    expect(isResetRelayActive()).toBe(false);
    expect(violetCacheSpy).not.toHaveBeenCalled();

    // Subsequent project switch in the same window must show Stage.tsx's normal "Restoring Room..." overlay!
    fireEvent.click(screen.getByTestId('tab-relay-room-b'));
    await flushMicrotasks();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(600);
    });
    expect(readRelayDomState()?.hidden).toBe(true);
    expect(screen.getByTestId('room-restore-overlay')).toHaveTextContent('Restoring Room...');

    await act(async () => {
      resolveIdentitiesB({
        identities: [makeIdentity('agent-beta', 'Beta')],
        workspaceEntryCount: 1,
      });
    });
    await flushMicrotasks();
    view.unmount();
  });

  it('preserves Stage.tsx Restoring Room... overlay on cold start and keeps relay hidden', async () => {
    vi.useFakeTimers();
    const workspace = makeWorkspace('cold-start-room', ['agent-alpha']);
    let resolveIdentities!: (val: ptyClient.ProjectAgentIdentityListing) => void;

    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockResolvedValue({ active: workspace });
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockResolvedValue([workspace]);
    vi.spyOn(ptyClient, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
    vi.spyOn(ptyClient, 'saveProjectAgentLayoutFile').mockResolvedValue();
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockImplementation(
      () => new Promise((res) => { resolveIdentities = res; }),
    );
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation(async (req) =>
      makeDetail(req.agentId, workspace.localRoot, 'Alpha'),
    );

    // Cold start: no sessionStorage marker.
    mountIndexRelayDom();
    runIndexBootstrapScript();
    expect(readRelayDomState()?.hidden).toBe(true);

    const view = render(<App />);
    await flushMicrotasks();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(600);
    });

    expect(readRelayDomState()?.hidden).toBe(true);
    expect(screen.getByTestId('room-restore-overlay')).toHaveTextContent('Restoring Room...');

    await act(async () => {
      resolveIdentities({
        identities: [makeIdentity('agent-alpha', 'Alpha')],
        workspaceEntryCount: 1,
      });
    });
    await flushMicrotasks();
    view.unmount();
  });

  it('dismisses immediately when workspace initial load settles with no open project', async () => {
    vi.useFakeTimers();
    let resolveStatus!: (val: { active: ptyClient.WorkspaceProject | null }) => void;
    let resolveProjects!: (val: ptyClient.WorkspaceProject[]) => void;

    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockImplementation(
      () => new Promise((res) => { resolveStatus = res; }),
    );
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockImplementation(
      () => new Promise((res) => { resolveProjects = res; }),
    );

    window.sessionStorage.setItem(RESET_RELAY_HANDOFF_KEY, '1');
    mountIndexRelayDom();
    runIndexBootstrapScript();

    const view = render(<App />);
    await flushMicrotasks();
    expect(readRelayDomState()).toMatchObject({ hidden: false, label: 'LOADING UI', count: '2/4' });

    await act(async () => {
      resolveStatus({ active: null });
      resolveProjects([]);
    });
    await flushMicrotasks();

    // Immediate dismiss without waiting 180ms or 12s.
    expect(readRelayDomState()?.hidden).toBe(true);
    expect(isResetRelayActive()).toBe(false);
    expect(screen.getByTestId('empty-project-state')).toBeInTheDocument();
    view.unmount();
  });

  it('dismisses immediately when agent hydration settles with failure so the retry banner is exposed', async () => {
    vi.useFakeTimers();
    const workspace = makeWorkspace('degraded-relay-room', ['agent-alpha']);

    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockResolvedValue({ active: workspace });
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockResolvedValue([workspace]);
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
      identities: [makeIdentity('agent-alpha', 'Alpha')],
      workspaceEntryCount: 1,
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockResolvedValue(
      makeDetail('agent-alpha', workspace.localRoot, 'Alpha'),
    );
    vi.spyOn(ptyClient, 'loadProjectAgentLayoutFile').mockRejectedValue(
      new Error('simulated layout read failure'),
    );
    vi.spyOn(console, 'warn').mockImplementation(() => {});

    window.sessionStorage.setItem(RESET_RELAY_HANDOFF_KEY, '1');
    mountIndexRelayDom();
    runIndexBootstrapScript();

    const view = render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();

    expect(readRelayDomState()?.hidden).toBe(true);
    expect(isResetRelayActive()).toBe(false);
    expect(screen.getByTestId('room-restore-degraded')).toHaveTextContent('Agent roster unavailable');
    view.unmount();
  });

  it('forces removal at the 12s hard ceiling when workspace startup hangs', async () => {
    vi.useFakeTimers();
    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockImplementation(() => new Promise(() => {}));
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockImplementation(() => new Promise(() => {}));

    window.sessionStorage.setItem(RESET_RELAY_HANDOFF_KEY, '1');
    mountIndexRelayDom();
    runIndexBootstrapScript();

    const view = render(<App />);
    await flushMicrotasks();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(RESET_RELAY_HARD_TIMEOUT_MS - 1);
    });
    expect(readRelayDomState()?.hidden).toBe(false);
    expect(isResetRelayActive()).toBe(true);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(readRelayDomState()?.hidden).toBe(true);
    expect(isResetRelayActive()).toBe(false);
    view.unmount();
  });
});
