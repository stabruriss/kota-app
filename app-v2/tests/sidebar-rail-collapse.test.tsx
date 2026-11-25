import { StrictMode } from 'react';
import { act, fireEvent, render, renderHook, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from '../src/App';
import { refreshGithubAccounts } from '../src/lib/github-account-flow';
import {
  LEFT_RAIL_COLLAPSED_STORAGE_KEY,
  RIGHT_RAIL_COLLAPSED_STORAGE_KEY,
  commitSidebarRailInstant,
  leftRailPlateLabel,
  loadSidebarRailCollapsed,
  publishLeftRailUpdateVersion,
  publishRightRailSystemLoadLamp,
  rightRailPlateLabel,
  saveSidebarRailCollapsed,
} from '../src/lib/sidebar-rail';
import { abortAllSidebarRailMotion } from '../src/lib/sidebar-rail-motion';
import {
  BYTES_PER_GIB,
  BYTES_PER_MIB,
  SYSTEM_LOAD_CONSTANTS,
  type SystemLoadSample,
} from '../src/lib/system-load';
import * as ptyClient from '../src/pty-client';
import type { WorkspaceTreeListing, WorkspaceTreePathRequest } from '../src/types/tree';

const renderCounts = vi.hoisted(() => ({
  App: 0,
  Stage: 0,
  FileTree: 0,
  RightColumn: 0,
  StageComposerRailHost: 0,
  LeftSidebarRailSlot: 0,
  RightSidebarRailSlot: 0,
  LeftSidebarRailPlate: 0,
  RightSidebarRailPlate: 0,
}));

function resetRenderCounts(): void {
  renderCounts.App = 0;
  renderCounts.Stage = 0;
  renderCounts.FileTree = 0;
  renderCounts.RightColumn = 0;
  renderCounts.StageComposerRailHost = 0;
  renderCounts.LeftSidebarRailSlot = 0;
  renderCounts.RightSidebarRailSlot = 0;
  renderCounts.LeftSidebarRailPlate = 0;
  renderCounts.RightSidebarRailPlate = 0;
}

vi.mock('../src/chrome/TopBar', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/chrome/TopBar')>();
  return {
    ...actual,
    TopBar: (props: Parameters<typeof actual.TopBar>[0]) => {
      renderCounts.App += 1;
      return actual.TopBar(props);
    },
  };
});

vi.mock('../src/chrome/AgentRibbon', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/chrome/AgentRibbon')>();
  return {
    ...actual,
    AgentRibbon: (props: Parameters<typeof actual.AgentRibbon>[0]) => {
      renderCounts.Stage += 1;
      return actual.AgentRibbon(props);
    },
  };
});

vi.mock('../src/chrome/room-search-elements', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/chrome/room-search-elements')>();
  return {
    ...actual,
    RoomSearchIcon: () => {
      renderCounts.FileTree += 1;
      return actual.RoomSearchIcon();
    },
  };
});

vi.mock('../src/chrome/KotaVersionLabel', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/chrome/KotaVersionLabel')>();
  return {
    ...actual,
    KotaVersionLabel: () => {
      renderCounts.RightColumn += 1;
      return actual.KotaVersionLabel();
    },
  };
});

vi.mock('../src/chrome/SidebarRailCollapse', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/chrome/SidebarRailCollapse')>();
  return {
    ...actual,
    StageComposerRailHost: (props: Parameters<typeof actual.StageComposerRailHost>[0]) => {
      renderCounts.StageComposerRailHost += 1;
      return actual.StageComposerRailHost(props);
    },
  };
});

vi.mock('../src/lib/sidebar-rail', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../src/lib/sidebar-rail')>();
  return {
    ...actual,
    useSidebarRailCollapsed: (side: import('../src/lib/sidebar-rail').SidebarRailSide) => {
      if (side === 'left') renderCounts.LeftSidebarRailSlot += 1;
      else renderCounts.RightSidebarRailSlot += 1;
      return actual.useSidebarRailCollapsed(side);
    },
    useSidebarRailPlate: (side: import('../src/lib/sidebar-rail').SidebarRailSide) => {
      if (side === 'left') renderCounts.LeftSidebarRailPlate += 1;
      else renderCounts.RightSidebarRailPlate += 1;
      return actual.useSidebarRailPlate(side);
    },
  };
});

function makeWorkspace(
  projectId = 'cedar-room',
  agentIds: readonly string[] = ['agent-rowan'],
): ptyClient.WorkspaceProject {
  const localRoot = `/tmp/kota-rail/${projectId}`;
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

function makeIdentity(agentId = 'agent-rowan', displayName = 'Rowan'): ptyClient.ProjectAgentIdentity {
  return {
    agentId,
    displayName,
    sourceHeroId: 'hero-dex',
    status: 'active',
    provider: 'codex',
    avatarId: null,
  };
}

function makeDetail(
  agentId: string,
  projectRoot: string,
  displayName = 'Rowan',
): ptyClient.ProjectAgentDetail {
  const projectId = projectRoot.split('/').at(-1) ?? 'cedar-room';
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

function makeNormalSample(): SystemLoadSample {
  return {
    atMs: 100_000,
    intervalMs: 30_000,
    totalRamBytes: 16 * BYTES_PER_GIB,
    compressedBytes: 1.5 * BYTES_PER_GIB,
    kotaPageBytes: 1.1 * BYTES_PER_GIB,
    swapBytesPerSec: 0,
  };
}

function makeYellowSample(): SystemLoadSample {
  return {
    atMs: 200_000,
    intervalMs: SYSTEM_LOAD_CONSTANTS.yellowRiseMs,
    totalRamBytes: 16 * BYTES_PER_GIB,
    compressedBytes: 2.0 * BYTES_PER_GIB,
    kotaPageBytes: 4.5 * BYTES_PER_GIB,
    swapBytesPerSec: 0,
  };
}

function makeRedSample(): SystemLoadSample {
  return {
    atMs: 300_000,
    intervalMs: SYSTEM_LOAD_CONSTANTS.redRiseMs,
    totalRamBytes: 16 * BYTES_PER_GIB,
    compressedBytes: 6.2 * BYTES_PER_GIB,
    kotaPageBytes: 4.8 * BYTES_PER_GIB,
    swapBytesPerSec: 18.0 * BYTES_PER_MIB,
  };
}

function mockTreeListing(request: WorkspaceTreePathRequest): WorkspaceTreeListing {
  const rootPath = request.rootKind === 'projectFiles' ? '/tmp/kota-rail/cedar-room/source' : '/tmp/kota-rail/cedar-room';
  if (request.relativePath === 'src') {
    return {
      root: {
        kind: request.rootKind,
        label: request.rootKind,
        absolutePath: rootPath,
        changeOverview: null,
      },
      entries: [
        {
          name: 'panel.tsx',
          path: 'src/panel.tsx',
          absolutePath: `${rootPath}/src/panel.tsx`,
          kind: 'file',
          isHidden: false,
        },
      ],
    };
  }
  return {
    root: {
      kind: request.rootKind,
      label: request.rootKind,
      absolutePath: rootPath,
      changeOverview: null,
    },
    entries: [
      {
        name: 'src',
        path: 'src',
        absolutePath: `${rootPath}/src`,
        kind: 'folder',
        isHidden: false,
      },
      {
        name: 'README.md',
        path: 'README.md',
        absolutePath: `${rootPath}/README.md`,
        kind: 'file',
        isHidden: false,
      },
    ],
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

function installBaseAppMocks(workspace = makeWorkspace()) {
  let emitSystemLoadSample: ((sample: SystemLoadSample) => void) | null = null;

  vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
  vi.spyOn(ptyClient, 'workspaceStatus').mockResolvedValue({ active: workspace });
  vi.spyOn(ptyClient, 'listWorkspaceProjects').mockResolvedValue([workspace]);
  vi.spyOn(ptyClient, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
  vi.spyOn(ptyClient, 'saveProjectAgentLayoutFile').mockResolvedValue();
  vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
    identities: [makeIdentity()],
    workspaceEntryCount: 1,
  });
  vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation(async (req) =>
    makeDetail(req.agentId, req.projectRoot ?? workspace.localRoot),
  );
  vi.spyOn(ptyClient, 'workspaceListTreePath').mockImplementation(async (req) => mockTreeListing(req));
  vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(makeNormalSample());
  vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async (listener) => {
    emitSystemLoadSample = listener;
    return () => {
      emitSystemLoadSample = null;
    };
  });
  vi.spyOn(ptyClient, 'checkAppUpdate').mockResolvedValue({
    hasUpdate: false,
    latestVersion: '0.1.27',
    homeUrl: 'https://kota.place',
  });

  return {
    workspace,
    pushSystemLoadSample: (sample: SystemLoadSample) => {
      act(() => {
        emitSystemLoadSample?.(sample);
      });
    },
  };
}

beforeEach(async () => {
  // Other suites in singleFork (e.g. rail-cards-room.test.tsx) assign Element.prototype.animate directly.
  delete (Element.prototype as Partial<Element>).animate;
  vi.restoreAllMocks();
  window.localStorage.clear();
  window.sessionStorage.clear();
  commitSidebarRailInstant('left', false);
  commitSidebarRailInstant('right', false);
  publishLeftRailUpdateVersion(null);
  publishRightRailSystemLoadLamp(null);
  resetRenderCounts();
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
  abortAllSidebarRailMotion();
  delete (Element.prototype as Partial<Element>).animate;
  window.localStorage.clear();
  window.sessionStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('Sidebar rail collapse · persistence, render isolation, and storage resilience', () => {
  it('toggles both rails, writes localStorage on each click, preserves mounted DOM/scrollTop, and keeps collapsed state across remount', async () => {
    installBaseAppMocks();

    const firstMount = render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
    await flushMicrotasks();
    await flushMicrotasks();

    const leftBtn = screen.getByTestId('sidebar-rail-toggle-left');
    const rightBtn = screen.getByTestId('sidebar-rail-toggle-right');
    const leftSlot = screen.getByTestId('sidebar-rail-slot-left');
    const rightSlot = screen.getByTestId('sidebar-rail-slot-right');
    const fileTree = screen.getByTestId('file-tree');
    const rightAside = document.getElementById('right-column') as HTMLElement;

    expect(leftSlot).toHaveClass('sidebar-rail-slot', 'sidebar-rail-slot--left');
    expect(rightSlot).toHaveClass('sidebar-rail-slot', 'sidebar-rail-slot--right');
    expect(leftSlot).not.toHaveClass('rail-slot');
    expect(rightSlot).not.toHaveClass('rail-slot');
    expect(rightAside.closest('.rail-slot')).toBeNull();

    expect(leftSlot).toHaveAttribute('data-collapsed', 'false');
    expect(rightSlot).toHaveAttribute('data-collapsed', 'false');
    expect(leftBtn).toHaveAttribute('aria-expanded', 'true');
    expect(leftBtn).toHaveAttribute('aria-controls', 'file-tree');
    expect(rightBtn).toHaveAttribute('aria-expanded', 'true');
    expect(rightBtn).toHaveAttribute('aria-controls', 'right-column');
    expect(fileTree.hasAttribute('inert')).toBe(false);
    expect(rightAside.hasAttribute('inert')).toBe(false);

    const srcRow = await screen.findByTestId('tree-row-projectFiles-src');
    fireEvent.click(within(srcRow).getByRole('button', { name: 'Expand folder' }));
    expect(await screen.findByTestId('tree-row-projectFiles-src/panel.tsx')).toBeInTheDocument();

    const treeBody = fileTree.querySelector('.tree-body') as HTMLElement;
    const srBody = rightAside.querySelector('.sr-body') as HTMLElement;
    treeBody.scrollTop = 148;
    srBody.scrollTop = 212;

    let resizeCount = 0;
    const onResize = () => { resizeCount += 1; };
    window.addEventListener('resize', onResize);

    fireEvent.click(leftBtn);
    expect(window.localStorage.getItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('true');
    expect(leftSlot).toHaveAttribute('data-collapsed', 'true');
    expect(leftBtn).toHaveAttribute('data-collapsed', 'true');
    expect(leftBtn).toHaveAttribute('aria-expanded', 'false');
    expect(leftBtn).toHaveAttribute('aria-label', 'Expand file tree');
    expect(leftBtn).toHaveAttribute('title', 'Expand file tree');
    expect(fileTree.hasAttribute('inert')).toBe(true);
    expect(fileTree).toHaveAttribute('aria-hidden', 'true');
    expect(resizeCount).toBe(1);

    fireEvent.click(rightBtn);
    expect(window.localStorage.getItem(RIGHT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('true');
    expect(rightSlot).toHaveAttribute('data-collapsed', 'true');
    expect(rightBtn).toHaveAttribute('data-collapsed', 'true');
    expect(rightBtn).toHaveAttribute('aria-expanded', 'false');
    expect(rightBtn).toHaveAttribute('aria-label', 'Expand right sidebar');
    expect(rightBtn).toHaveAttribute('title', 'Expand right sidebar');
    expect(rightAside.hasAttribute('inert')).toBe(true);
    expect(rightAside).toHaveAttribute('aria-hidden', 'true');
    expect(resizeCount).toBe(2);

    expect(screen.getByTestId('file-tree')).toBe(fileTree);
    expect(document.getElementById('right-column')).toBe(rightAside);

    fireEvent.click(leftBtn);
    expect(window.localStorage.getItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('false');
    expect(leftSlot).toHaveAttribute('data-collapsed', 'false');
    expect(fileTree.hasAttribute('inert')).toBe(false);
    expect(screen.getByTestId('tree-row-projectFiles-src/panel.tsx')).toBeInTheDocument();
    expect(treeBody.scrollTop).toBe(148);
    expect(resizeCount).toBe(3);

    fireEvent.click(rightBtn);
    expect(window.localStorage.getItem(RIGHT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('false');
    expect(rightSlot).toHaveAttribute('data-collapsed', 'false');
    expect(rightAside.hasAttribute('inert')).toBe(false);
    expect(srBody.scrollTop).toBe(212);
    expect(resizeCount).toBe(4);

    fireEvent.click(leftBtn);
    fireEvent.click(rightBtn);
    window.removeEventListener('resize', onResize);
    firstMount.unmount();

    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(false);
    let remountResizeCount = 0;
    const onRemountResize = () => { remountResizeCount += 1; };
    window.addEventListener('resize', onRemountResize);

    const secondMount = render(
      <StrictMode>
        <App />
      </StrictMode>,
    );

    expect(screen.getByTestId('sidebar-rail-slot-left')).toHaveAttribute('data-collapsed', 'true');
    expect(screen.getByTestId('sidebar-rail-slot-right')).toHaveAttribute('data-collapsed', 'true');
    expect(screen.getByTestId('sidebar-rail-toggle-left')).toHaveAttribute('aria-expanded', 'false');
    expect(screen.getByTestId('sidebar-rail-toggle-right')).toHaveAttribute('aria-expanded', 'false');
    expect(screen.getByTestId('file-tree').hasAttribute('inert')).toBe(true);
    expect(document.getElementById('right-column')?.hasAttribute('inert')).toBe(true);
    expect(remountResizeCount).toBe(0);

    window.removeEventListener('resize', onRemountResize);
    secondMount.unmount();
  });

  it('clicking a nameplate renders only that side slot and plate while App, Stage, FileTree, RightColumn, and host render 0 times', async () => {
    installBaseAppMocks();

    const view = render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();
    await screen.findByTestId('tree-row-projectFiles-src');

    const leftBtn = screen.getByTestId('sidebar-rail-toggle-left');
    const rightBtn = screen.getByTestId('sidebar-rail-toggle-right');

    resetRenderCounts();
    fireEvent.click(leftBtn);
    expect(renderCounts).toEqual({
      App: 0,
      Stage: 0,
      FileTree: 0,
      RightColumn: 0,
      StageComposerRailHost: 0,
      LeftSidebarRailSlot: 1,
      RightSidebarRailSlot: 0,
      LeftSidebarRailPlate: 1,
      RightSidebarRailPlate: 0,
    });

    resetRenderCounts();
    fireEvent.click(rightBtn);
    expect(renderCounts).toEqual({
      App: 0,
      Stage: 0,
      FileTree: 0,
      RightColumn: 0,
      StageComposerRailHost: 0,
      LeftSidebarRailSlot: 0,
      RightSidebarRailSlot: 1,
      LeftSidebarRailPlate: 0,
      RightSidebarRailPlate: 1,
    });

    resetRenderCounts();
    fireEvent.click(leftBtn);
    expect(renderCounts).toEqual({
      App: 0,
      Stage: 0,
      FileTree: 0,
      RightColumn: 0,
      StageComposerRailHost: 0,
      LeftSidebarRailSlot: 1,
      RightSidebarRailSlot: 0,
      LeftSidebarRailPlate: 1,
      RightSidebarRailPlate: 0,
    });

    resetRenderCounts();
    fireEvent.click(rightBtn);
    expect(renderCounts).toEqual({
      App: 0,
      Stage: 0,
      FileTree: 0,
      RightColumn: 0,
      StageComposerRailHost: 0,
      LeftSidebarRailSlot: 0,
      RightSidebarRailSlot: 1,
      LeftSidebarRailPlate: 0,
      RightSidebarRailPlate: 1,
    });

    view.unmount();
  });
});

describe('Sidebar rail collapse · status lamps, hover copy, focus, Tab skip, and composerless screens', () => {
  it('keeps status lamps always-on across expanded/collapsed states and updates hover/aria labels', async () => {
    vi.useFakeTimers();
    const { pushSystemLoadSample } = installBaseAppMocks();
    vi.spyOn(ptyClient, 'checkAppUpdate').mockResolvedValue({
      hasUpdate: true,
      latestVersion: '0.2.0',
      homeUrl: 'https://kota.place',
    });

    render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();

    const leftBtn = screen.getByTestId('sidebar-rail-toggle-left');
    const rightBtn = screen.getByTestId('sidebar-rail-toggle-right');

    expect(leftBtn).toHaveAttribute('data-glow', 'none');
    expect(rightBtn).toHaveAttribute('data-glow', 'none');

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_100);
    });
    await flushMicrotasks();

    expect(leftBtn).toHaveAttribute('data-glow', 'gold');
    expect(leftBtn).toHaveAttribute('title', 'Collapse file tree');

    fireEvent.click(leftBtn);
    expect(leftBtn).toHaveAttribute('data-glow', 'gold');
    expect(leftBtn).toHaveAttribute('title', 'Expand file tree · Kota 0.2.0 is available');
    expect(leftBtn).toHaveAttribute('aria-label', 'Expand file tree · Kota 0.2.0 is available');

    pushSystemLoadSample(makeYellowSample());
    expect(rightBtn).toHaveAttribute('data-glow', 'yellow');
    expect(rightBtn).toHaveAttribute('title', 'Collapse right sidebar');

    fireEvent.click(rightBtn);
    expect(rightBtn).toHaveAttribute('data-glow', 'yellow');
    expect(rightBtn).toHaveAttribute('title', 'Expand right sidebar · System Load yellow reset');
    expect(rightBtn).toHaveAttribute('aria-label', 'Expand right sidebar · System Load yellow reset');

    pushSystemLoadSample(makeRedSample());
    expect(rightBtn).toHaveAttribute('data-glow', 'red');
    expect(rightBtn).toHaveAttribute('title', 'Expand right sidebar · System Load red reset');

    fireEvent.click(rightBtn);
    expect(rightBtn).toHaveAttribute('data-glow', 'red');
    expect(rightBtn).toHaveAttribute('title', 'Collapse right sidebar');

    expect(leftRailPlateLabel(false, null)).toBe('Collapse file tree');
    expect(leftRailPlateLabel(true, null)).toBe('Expand file tree');
    expect(leftRailPlateLabel(true, '0.2.0')).toBe('Expand file tree · Kota 0.2.0 is available');
    expect(rightRailPlateLabel(false, null)).toBe('Collapse right sidebar');
    expect(rightRailPlateLabel(true, null)).toBe('Expand right sidebar');
  });

  it('preserves composer focus on nameplate mousedown, blurs focused rail controls on collapse, and skips collapsed rails on Tab', async () => {
    installBaseAppMocks();
    const user = userEvent.setup();

    render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();

    const leftBtn = screen.getByTestId('sidebar-rail-toggle-left');
    const rightBtn = screen.getByTestId('sidebar-rail-toggle-right');
    const fileTree = screen.getByTestId('file-tree');
    const rightAside = document.getElementById('right-column') as HTMLElement;

    const emberBtn = rightAside.querySelector('.ember-card button, .system-agent-stack button') as HTMLElement | null;
    expect(emberBtn).not.toBeNull();
    expect(screen.getByTestId('sidebar-rail-slot-right').children).toHaveLength(1);
    expect(screen.getByTestId('sidebar-rail-slot-right').firstElementChild).toBe(rightAside);

    const composerInput = document.querySelector('.stage-bottom-dock .ib-rich-field') as HTMLElement;
    expect(composerInput).not.toBeNull();
    composerInput.focus();
    expect(document.activeElement).toBe(composerInput);

    expect(fireEvent.mouseDown(leftBtn)).toBe(false);
    fireEvent.click(leftBtn);
    expect(document.activeElement).toBe(composerInput);

    expect(fireEvent.mouseDown(rightBtn)).toBe(false);
    fireEvent.click(rightBtn);
    expect(document.activeElement).toBe(composerInput);

    fireEvent.click(leftBtn);
    const treeSearchInput = fileTree.querySelector('.tree-search input') as HTMLElement;
    expect(treeSearchInput).not.toBeNull();
    treeSearchInput.focus();
    expect(document.activeElement).toBe(treeSearchInput);

    fireEvent.click(leftBtn);
    expect(fileTree.contains(document.activeElement)).toBe(false);

    leftBtn.focus();
    expect(document.activeElement).toBe(leftBtn);
    for (let step = 0; step < 25; step += 1) {
      await user.tab();
      expect(fileTree.contains(document.activeElement)).toBe(false);
      expect(rightAside.contains(document.activeElement)).toBe(false);
    }
  });

  it('keeps RightColumn expanded on loading/empty screens, never unmounts RoomComposer when loading finishes, and restores saved collapsed state', async () => {
    commitSidebarRailInstant('left', true);
    commitSidebarRailInstant('right', true);

    const workspace = makeWorkspace();
    let resolveStatus!: (val: { active: ptyClient.WorkspaceProject | null }) => void;
    let resolveProjects!: (val: ptyClient.WorkspaceProject[]) => void;

    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockImplementation(
      () => new Promise((res) => { resolveStatus = res; }),
    );
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockImplementation(
      () => new Promise((res) => { resolveProjects = res; }),
    );
    vi.spyOn(ptyClient, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
    vi.spyOn(ptyClient, 'saveProjectAgentLayoutFile').mockResolvedValue();
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
      identities: [makeIdentity()],
      workspaceEntryCount: 1,
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation(async (req) =>
      makeDetail(req.agentId, req.projectRoot ?? workspace.localRoot),
    );
    vi.spyOn(ptyClient, 'workspaceListTreePath').mockImplementation(async (req) => mockTreeListing(req));
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(makeNormalSample());
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async () => () => {});

    const removedInputBars: Node[] = [];
    const observer = new MutationObserver((mutations) => {
      for (const m of mutations) {
        m.removedNodes.forEach((node) => {
          if (node instanceof HTMLElement && (node.classList.contains('input-bar-wrap') || node.querySelector('.input-bar-wrap'))) {
            removedInputBars.push(node);
          }
        });
      }
    });
    observer.observe(document.body, { childList: true, subtree: true });

    const view = render(<App />);
    await flushMicrotasks();

    // 1. Loading screen: zero nameplates, RightColumn remains expanded and not inert.
    expect(screen.queryByTestId('sidebar-rail-toggle-left')).not.toBeInTheDocument();
    expect(screen.queryByTestId('sidebar-rail-toggle-right')).not.toBeInTheDocument();
    expect(screen.getByTestId('sidebar-rail-slot-right')).toHaveAttribute('data-collapsed', 'false');
    expect(document.getElementById('right-column')?.hasAttribute('inert')).toBe(false);

    // 2. Loading finishes with an active workspace: RoomComposer mounts once (0 unmounts!) and both rails restore collapsed!
    await act(async () => {
      resolveStatus({ active: workspace });
      resolveProjects([workspace]);
    });
    const mountedInputField = document.querySelector('.stage-bottom-dock .ib-rich-field');
    expect(mountedInputField).not.toBeNull();
    await flushMicrotasks();

    expect(removedInputBars).toHaveLength(0);
    expect(document.querySelector('.stage-bottom-dock .ib-rich-field')).toBe(mountedInputField);
    expect(screen.getByTestId('sidebar-rail-slot-left')).toHaveAttribute('data-collapsed', 'true');
    expect(screen.getByTestId('sidebar-rail-slot-right')).toHaveAttribute('data-collapsed', 'true');
    expect(window.localStorage.getItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('true');
    expect(window.localStorage.getItem(RIGHT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('true');

    observer.disconnect();
    view.unmount();
  });
});

describe('Sidebar rail collapse · Option 甲 210ms WAAPI slide, mid-slide 7-click reversal, abort sync, and Option 乙 reduced-motion', () => {
  interface MockAnimRecord {
    target: Element;
    keyframes: Keyframe[];
    options: KeyframeAnimationOptions;
    cancelled: boolean;
    resolveFinished: () => void;
    rejectFinished: () => void;
  }

  function installMockWaapi(readMidTransform?: (el: Element) => string) {
    const records: MockAnimRecord[] = [];
    Element.prototype.animate = vi.fn(function (
      this: Element,
      keyframes: Keyframe[] | PropertyIndexedKeyframes | null,
      options?: number | KeyframeAnimationOptions,
    ) {
      let resolveFinished!: () => void;
      let rejectFinished!: () => void;
      const finished = new Promise<Animation>((res, rej) => {
        resolveFinished = () => res(undefined as unknown as Animation);
        rejectFinished = () => rej(new DOMException('Aborted', 'AbortError'));
      });
      finished.catch(() => {});
      const rec: MockAnimRecord = {
        target: this,
        keyframes: (keyframes as Keyframe[]) ?? [],
        options: typeof options === 'number' ? { duration: options } : (options ?? {}),
        cancelled: false,
        resolveFinished,
        rejectFinished,
      };
      records.push(rec);
      return {
        finished,
        cancel() {
          if (!rec.cancelled) {
            rec.cancelled = true;
            rec.rejectFinished();
          }
        },
      } as unknown as Animation;
    });

    if (readMidTransform) {
      const origGetComputedStyle = window.getComputedStyle.bind(window);
      vi.spyOn(window, 'getComputedStyle').mockImplementation((el: Element, pseudo?: string | null) => {
        const base = origGetComputedStyle(el, pseudo);
        const customTx = readMidTransform(el);
        if (!customTx) return base;
        return new Proxy(base, {
          get(target, prop) {
            if (prop === 'transform') return customTx;
            const value = Reflect.get(target, prop, target);
            return typeof value === 'function' ? value.bind(target) : value;
          },
        });
      });
    }

    return records;
  }

  it('slides aside and plate for 210ms before changing slot width, lifts z-index only while animating, and fires 1 resize on settle', async () => {
    const records = installMockWaapi();
    installBaseAppMocks();

    const view = render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();

    const leftBtn = screen.getByTestId('sidebar-rail-toggle-left');
    const leftSlot = screen.getByTestId('sidebar-rail-slot-left');
    const fileTree = screen.getByTestId('file-tree');

    let resizeCount = 0;
    const onResize = () => { resizeCount += 1; };
    window.addEventListener('resize', onResize);

    resetRenderCounts();
    fireEvent.click(leftBtn);

    expect(records).toHaveLength(2);
    expect(records[0]!.target).toBe(fileTree);
    expect(records[1]!.target).toBe(leftBtn);
    expect(records[0]!.options).toMatchObject({
      duration: 210,
      easing: 'cubic-bezier(0.28, 0.08, 0.22, 1)',
      fill: 'forwards',
    });
    expect(records[0]!.keyframes).toEqual([
      { transform: 'translateX(0.00px)' },
      { transform: 'translateX(-7.00px)', offset: 0.14 },
      { transform: 'translateX(-7.00px)', offset: 0.21 },
      { transform: 'translateX(-240.00px)' },
    ]);
    expect(records[1]!.keyframes).toEqual(records[0]!.keyframes);

    expect(leftSlot).toHaveAttribute('data-collapsed', 'false');
    expect(leftSlot).toHaveAttribute('data-animating', 'true');
    expect(fileTree.style.visibility).toBe('visible');
    expect(leftBtn).toHaveAttribute('data-collapsed', 'true');
    expect(resizeCount).toBe(0);

    expect(renderCounts).toEqual({
      App: 0,
      Stage: 0,
      FileTree: 0,
      RightColumn: 0,
      StageComposerRailHost: 0,
      LeftSidebarRailSlot: 0,
      RightSidebarRailSlot: 0,
      LeftSidebarRailPlate: 1,
      RightSidebarRailPlate: 0,
    });

    await act(async () => {
      records[0]!.resolveFinished();
      records[1]!.resolveFinished();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(records[0]!.cancelled).toBe(true);
    expect(records[1]!.cancelled).toBe(true);
    expect(leftSlot).toHaveAttribute('data-collapsed', 'true');
    expect(leftSlot).not.toHaveAttribute('data-animating');
    expect(fileTree.style.visibility).toBe('hidden');
    expect(fileTree.hasAttribute('inert')).toBe(true);
    expect(resizeCount).toBe(1);
    expect(renderCounts.LeftSidebarRailSlot).toBe(1);
    expect(renderCounts.App).toBe(0);
    expect(renderCounts.Stage).toBe(0);
    expect(renderCounts.FileTree).toBe(0);
    expect(renderCounts.RightColumn).toBe(0);

    window.removeEventListener('resize', onResize);
    view.unmount();
  });

  it('reverses smoothly from current mid-slide DOMMatrixReadOnly translateX across 7 rapid clicks and syncs aside back to slot on mid-slide abort', async () => {
    let midTx = 'none';
    const records = installMockWaapi((el) => (el.id === 'file-tree' && midTx !== 'none' ? midTx : ''));
    installBaseAppMocks();

    const view = render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();

    const leftBtn = screen.getByTestId('sidebar-rail-toggle-left');
    const leftSlot = screen.getByTestId('sidebar-rail-slot-left');
    const rightBtn = screen.getByTestId('sidebar-rail-toggle-right');
    const rightAside = document.getElementById('right-column') as HTMLElement;

    let resizeCount = 0;
    const onResize = () => { resizeCount += 1; };
    window.addEventListener('resize', onResize);

    const midPositions = [
      'matrix(1, 0, 0, 1, -18, 0)',
      'matrix(1, 0, 0, 1, -92, 0)',
      'matrix(1, 0, 0, 1, -64, 0)',
      'matrix(1, 0, 0, 1, -140, 0)',
      'matrix(1, 0, 0, 1, -105, 0)',
      'matrix(1, 0, 0, 1, -172, 0)',
      'matrix(1, 0, 0, 1, -130, 0)',
    ];

    for (let click = 0; click < 7; click += 1) {
      fireEvent.click(leftBtn);
      midTx = midPositions[click]!;
    }

    expect(records).toHaveLength(14);
    for (let i = 0; i < 12; i += 1) {
      expect(records[i]!.cancelled).toBe(true);
    }
    expect(records[12]!.cancelled).toBe(false);
    expect(records[13]!.cancelled).toBe(false);
    expect(records[12]!.keyframes[0]).toEqual({ transform: 'translateX(-172.00px)' });
    expect(records[12]!.keyframes.at(-1)).toEqual({ transform: 'translateX(-240.00px)' });
    expect(resizeCount).toBe(0);

    await act(async () => {
      records[12]!.resolveFinished();
      records[13]!.resolveFinished();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(leftSlot).toHaveAttribute('data-collapsed', 'true');
    expect(leftSlot).not.toHaveAttribute('data-animating');
    expect(window.localStorage.getItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY)).toBe('true');
    expect(resizeCount).toBe(1);

    // Mid-slide abort test (Mandatory Fix 3): start collapsing right rail, then abort mid-slide -> aside restored to non-inert!
    fireEvent.click(rightBtn);
    expect(screen.getByTestId('sidebar-rail-slot-right')).toHaveAttribute('data-collapsed', 'false');
    expect(screen.getByTestId('sidebar-rail-slot-right')).toHaveAttribute('data-animating', 'true');
    expect(rightAside.hasAttribute('inert')).toBe(true);

    act(() => {
      abortAllSidebarRailMotion();
    });
    expect(screen.getByTestId('sidebar-rail-slot-right')).not.toHaveAttribute('data-animating');
    expect(rightAside.hasAttribute('inert')).toBe(false);
    expect(rightAside).not.toHaveAttribute('aria-hidden');
    expect(rightAside.style.visibility).toBe('');

    window.removeEventListener('resize', onResize);
    view.unmount();
  });

  it('uses the 0ms instant Option 乙 path when prefers-reduced-motion: reduce is active even with WAAPI present', async () => {
    const records = installMockWaapi();
    vi.spyOn(window, 'matchMedia').mockImplementation((query: string) => ({
      matches: query === '(prefers-reduced-motion: reduce)',
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }));
    installBaseAppMocks();

    const view = render(<App />);
    await flushMicrotasks();
    await flushMicrotasks();

    let resizeCount = 0;
    const onResize = () => { resizeCount += 1; };
    window.addEventListener('resize', onResize);

    fireEvent.click(screen.getByTestId('sidebar-rail-toggle-right'));

    expect(records).toHaveLength(0);
    expect(screen.getByTestId('sidebar-rail-slot-right')).toHaveAttribute('data-collapsed', 'true');
    expect(screen.getByTestId('sidebar-rail-slot-right')).not.toHaveAttribute('data-animating');
    expect(resizeCount).toBe(1);

    window.removeEventListener('resize', onResize);
    view.unmount();
  });

  it('hydrates initial store state synchronously at module load and treats unreadable localStorage as never saved', async () => {
    // 1. Persisted 'true' values in localStorage are read once at module evaluation time.
    window.localStorage.setItem(LEFT_RAIL_COLLAPSED_STORAGE_KEY, 'true');
    window.localStorage.setItem(RIGHT_RAIL_COLLAPSED_STORAGE_KEY, 'true');
    vi.doUnmock('../src/lib/sidebar-rail');
    vi.resetModules();

    const reloadedStore = await import('../src/lib/sidebar-rail');
    const leftHook = renderHook(() => reloadedStore.useSidebarRailCollapsed('left'));
    const rightHook = renderHook(() => reloadedStore.useSidebarRailCollapsed('right'));
    expect(leftHook.result.current).toBe(true);
    expect(rightHook.result.current).toBe(true);
    leftHook.unmount();
    rightHook.unmount();

    // 2. Unreadable localStorage at module evaluation time falls back to false without throwing.
    const getItemSpy = vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('Storage disabled', 'SecurityError');
    });
    const setItemSpy = vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('Storage disabled', 'SecurityError');
    });
    vi.resetModules();

    const fallbackStore = await import('../src/lib/sidebar-rail');
    expect(loadSidebarRailCollapsed('left')).toBe(false);
    expect(() => saveSidebarRailCollapsed('left', true)).not.toThrow();

    const fallbackLeft = renderHook(() => fallbackStore.useSidebarRailCollapsed('left'));
    const fallbackRight = renderHook(() => fallbackStore.useSidebarRailCollapsed('right'));
    expect(fallbackLeft.result.current).toBe(false);
    expect(fallbackRight.result.current).toBe(false);

    act(() => {
      fallbackStore.toggleSidebarRail('left');
    });
    expect(fallbackLeft.result.current).toBe(true);
    fallbackLeft.unmount();
    fallbackRight.unmount();

    getItemSpy.mockRestore();
    setItemSpy.mockRestore();
  });
});
