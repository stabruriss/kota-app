import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { createRef, useCallback, useRef } from 'react';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RightColumn } from '../src/chrome/RightColumn';
import { RoomComposer } from '../src/chrome/RoomComposer';
import type { InputBarHandle } from '../src/chrome/InputBar';
import * as ptyClient from '../src/pty-client';
import * as systemLoad from '../src/lib/system-load';
import {
  BYTES_PER_GIB,
  BYTES_PER_MIB,
  recordSystemLoadPageLoadedAt,
  type SystemLoadSample,
} from '../src/lib/system-load';

function makeWorkspace(projectId = 'synthetic-room'): ptyClient.WorkspaceProject {
  const localRoot = `/tmp/kota-test/${projectId}`;
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
    agents: [
      {
        agentId: 'alice',
        cli: 'codex',
        cwd: `${localRoot}/.agent-workspaces/alice`,
        projectRoot: localRoot,
        worktreeRoot: `${localRoot}/.agent-workspaces/alice/project-files`,
        sharedDir: `${localRoot}/project-memory`,
        rulesDir: `${localRoot}/project-rules`,
        adapterPath: `${localRoot}/.agent-workspaces/alice/AGENTS.md`,
        args: ['--model', 'default'],
        projectId,
        projectBaseRef: 'origin/main',
      },
    ],
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

function toggleRockerSwitch(label: 'Good Night' | 'Back to Kota'): void {
  const rocker = screen.getByRole('switch', { name: label });
  fireEvent.click(rocker);
  fireEvent.click(rocker);
}

function extractCssRules(cssText: string): Array<{ selectors: string[]; body: string }> {
  const withoutComments = cssText.replace(/\/\*[\s\S]*?\*\//g, '');
  const rules: Array<{ selectors: string[]; body: string }> = [];
  let cursor = 0;

  while (cursor < withoutComments.length) {
    const openBrace = withoutComments.indexOf('{', cursor);
    if (openBrace === -1) break;
    const prelude = withoutComments.slice(cursor, openBrace).trim();
    let depth = 1;
    let scan = openBrace + 1;
    while (scan < withoutComments.length && depth > 0) {
      if (withoutComments[scan] === '{') depth += 1;
      else if (withoutComments[scan] === '}') depth -= 1;
      scan += 1;
    }
    const inner = withoutComments.slice(openBrace + 1, scan - 1);
    if (prelude.startsWith('@')) {
      rules.push(...extractCssRules(inner));
    } else if (prelude.length > 0) {
      const selectors = prelude
        .split(',')
        .map((sel) => sel.trim())
        .filter(Boolean);
      rules.push({ selectors, body: inner.trim() });
    }
    cursor = scan;
  }

  return rules;
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllEnvs();
  window.sessionStorage.clear();
  delete window.__KOTA_SYSTEM_LOAD_DEBUG__;
  vi.spyOn(ptyClient, 'githubAccountsRead').mockResolvedValue({
    defaultLogin: 'mock',
    accounts: [{ login: 'mock', default: true, state: 'ok', source: 'gh-cli' }],
  });
  vi.spyOn(ptyClient, 'projectConnectionStatus').mockResolvedValue({
    kind: 'github',
    accountLogin: 'mock',
    accountState: 'ok',
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
  delete window.__KOTA_SYSTEM_LOAD_DEBUG__;
});

describe('System Load instrument & Reset / Dream flows', () => {
  it('avoids filter:blur and :has() in production CSS and exposes custom HUD tooltips without title attributes', async () => {
    const cssText = readFileSync(resolve(__dirname, '../src/styles/system-load.css'), 'utf8');
    expect(cssText).not.toContain('blur(');
    expect(cssText).not.toContain(':has(');

    let emitSample: ((sample: SystemLoadSample) => void) | null = null;
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async (cb) => {
      emitSample = cb;
      return () => {};
    });
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(
      makeSample({
        atMs: 10_000,
        intervalMs: null,
        totalRamBytes: 16 * BYTES_PER_GIB,
        compressedBytes: 6.1 * BYTES_PER_GIB,
        kotaPageBytes: 4.3 * BYTES_PER_GIB,
        swapBytesPerSec: null,
      }),
    );

    await act(async () => {
      render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    });

    const expandBtn = screen.getByRole('button', { name: 'SYSTEM LOAD' });
    expect(expandBtn).not.toHaveAttribute('title');
    expect(expandBtn).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(expandBtn);
    expect(expandBtn).toHaveAttribute('aria-expanded', 'true');

    const panel = screen.getByRole('dialog', { name: 'SYSTEM LOAD' });
    expect(within(panel).getByText('SYSTEM LOAD')).toBeInTheDocument();
    expect(within(panel).getByText('TOTAL RAM 16 GB')).toBeInTheDocument();

    const resetBtn = within(panel).getByRole('button', { name: 'RESET KOTA' });
    expect(resetBtn).not.toHaveAttribute('title');

    const unit = panel.querySelector('.hud-unit')!;
    expect(unit).toHaveClass('compressed-yellow', 'kota-yellow');
    expect(unit).not.toHaveClass('swap-yellow');

    await act(async () => {
      emitSample?.(
        makeSample({
          atMs: 40_000,
          intervalMs: 30_000,
          compressedBytes: 6.1 * BYTES_PER_GIB,
          kotaPageBytes: 4.3 * BYTES_PER_GIB,
          swapBytesPerSec: 6.5 * BYTES_PER_MIB,
        }),
      );
    });
    expect(unit).toHaveClass('swap-yellow');

    for (const label of ['COMPRESSED RAM', 'KOTA RAM', 'SWAP BURDEN'] as const) {
      const row = within(panel).getByRole('img', { name: label });
      expect(row).not.toHaveAttribute('title');
      expect(row.querySelector('.hud-track')).not.toHaveAttribute('title');

      fireEvent.focus(row);
      expect(screen.getByRole('tooltip')).toHaveTextContent(label);
      fireEvent.blur(row);
      expect(screen.queryByRole('tooltip')).not.toBeInTheDocument();
    }

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'SYSTEM LOAD' })).not.toBeInTheDocument();
    expect(document.activeElement).toBe(expandBtn);
  });

  it('does not match worked-clock cards, digits, labels, or key body in system-load.css while unlit, and preserves canvas.css shadows when lit', async () => {
    const cssText = readFileSync(resolve(__dirname, '../src/styles/system-load.css'), 'utf8');
    expect(cssText).not.toContain('.ember-worked-flip-label');

    const rules = extractCssRules(cssText);
    expect(rules.length).toBeGreaterThan(0);

    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async () => () => {});
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(makeSample());

    const view = render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    await act(async () => {
      await Promise.resolve();
    });

    const head = view.container.querySelector('.ember-room-head')!;
    expect(head).not.toHaveClass('ram-lit');

    const guardedGroups: Array<[string, Element[]]> = [
      ['.ember-worked-flip', Array.from(view.container.querySelectorAll('.ember-worked-flip'))],
      ['.ember-worked-flip-card', Array.from(view.container.querySelectorAll('.ember-worked-flip-card'))],
      ['.ember-worked-flip-card > span', Array.from(view.container.querySelectorAll('.ember-worked-flip-card > span'))],
      ['.ember-worked-flip-label', Array.from(view.container.querySelectorAll('.ember-worked-flip-label'))],
      ['.ember-worked-reset.ram-expand', Array.from(view.container.querySelectorAll('.ember-worked-reset.ram-expand'))],
    ];

    for (const [groupName, elements] of guardedGroups) {
      expect(elements.length, `${groupName} must exist`).toBeGreaterThan(0);
      for (const el of elements) {
        for (const rule of rules) {
          for (const rawSelector of rule.selectors) {
            // Strip ::before / ::after so even an un-gated pseudo-element rule on these elements fails.
            const elementSelector = rawSelector.replace(/::(before|after)\b/g, '').trim();
            if (!elementSelector) continue;
            // happy-dom throws on :focus-visible; an unfocused button never matches :focus-visible anyway.
            const safeSelector = elementSelector.includes(':focus-visible')
              ? elementSelector.replace(/:focus-visible/g, '[data-test-never-focus-visible]')
              : elementSelector;
            expect(
              el.matches(safeSelector),
              `unlit ${groupName} must not match "${rawSelector}" in system-load.css`,
            ).toBe(false);
          }
        }
      }
    }

    // Verify lit (.ram-lit) rules keep canvas.css base shadow values intact without altering colors.
    const cardLitRule = rules.find((r) => r.selectors.includes('.ember-room-head.ram-lit .ember-worked-flip-card'));
    expect(cardLitRule).toBeDefined();
    expect(cardLitRule!.body).toContain('0 calc(var(--worked-card-height) * 0.05) calc(var(--worked-card-height) * 0.12) rgba(0, 0, 0, 0.45)');
    expect(cardLitRule!.body).toContain('inset 0 1px 0 rgba(255, 255, 255, 0.05)');
    expect(cardLitRule!.body).toContain('inset 0 -1px 0 rgba(0, 0, 0, 0.40)');
    expect(cardLitRule!.body).not.toMatch(/\bbackground\s*:/);
    expect(cardLitRule!.body).not.toMatch(/\bcolor\s*:/);

    const digitLitRule = rules.find((r) => r.selectors.includes('.ember-room-head.ram-lit .ember-worked-flip-card > span'));
    expect(digitLitRule).toBeDefined();
    expect(digitLitRule!.body).toContain('0 1px 1px rgba(0, 0, 0, 0.45)');
    expect(digitLitRule!.body).not.toMatch(/\bcolor\s*:/);

    const expandLitRule = rules.find((r) => r.selectors.includes('.ember-room-head.ram-lit .ember-worked-reset.ram-expand'));
    expect(expandLitRule).toBeDefined();
    expect(expandLitRule!.body).toContain('0 calc(var(--h) * 0.06) calc(var(--h) * 0.12) rgba(0, 0, 0, 0.5)');
    expect(expandLitRule!.body).toContain('inset 0 1px 0 rgba(255, 255, 255, 0.07)');
    expect(expandLitRule!.body).toContain('inset 0 calc(var(--h) * -0.045) calc(var(--h) * 0.05) rgba(0, 0, 0, 0.4)');
  });

  it('mutates zero DOM nodes when collapsed and the lamp remains off across incoming samples', async () => {
    vi.useFakeTimers();
    const startMs = Date.now() + 5_000;
    vi.setSystemTime(startMs);

    let emitSample: ((sample: SystemLoadSample) => void) | null = null;
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async (cb) => {
      emitSample = cb;
      return () => {};
    });
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(null);

    const view = render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    await act(async () => {
      await Promise.resolve();
    });

    const mutations: MutationRecord[] = [];
    const observer = new MutationObserver((records) => {
      mutations.push(...records);
    });
    observer.observe(view.container, {
      subtree: true,
      attributes: true,
      childList: true,
      characterData: true,
    });

    await act(async () => {
      emitSample?.(
        makeSample({
          atMs: 30_000,
          intervalMs: 30_000,
          totalRamBytes: 16 * BYTES_PER_GIB,
          compressedBytes: 3.5 * BYTES_PER_GIB,
          kotaPageBytes: 2.0 * BYTES_PER_GIB,
          swapBytesPerSec: 1.25 * BYTES_PER_MIB,
        }),
      );
      await Promise.resolve();
    });

    observer.disconnect();
    expect(mutations).toHaveLength(0);

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    const panel = screen.getByRole('dialog', { name: 'SYSTEM LOAD' });
    expect(within(panel).getByText('3.5')).toBeInTheDocument();
    expect(within(panel).getByText('2.0')).toBeInTheDocument();
    expect(within(panel).getByText('1.25 MBps')).toBeInTheDocument();
  });

  it('ignores RESET KOTA within 10s of page load, and disables immediately on direct reload without drafts', async () => {
    const reloadSpy = vi.spyOn(systemLoad, 'reloadKotaWindow').mockImplementation(() => {});
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async () => () => {});
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(makeSample());

    const baseNow = Date.now();
    const nowSpy = vi.spyOn(Date, 'now').mockReturnValue(baseNow);
    recordSystemLoadPageLoadedAt(baseNow);

    await act(async () => {
      render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    });

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    const resetBtn = screen.getByRole('button', { name: 'RESET KOTA' });

    // Within 10s post-load cooldown: ignored.
    nowSpy.mockReturnValue(baseNow + 9_900);
    fireEvent.click(resetBtn);
    expect(reloadSpy).not.toHaveBeenCalled();
    expect(resetBtn).not.toBeDisabled();

    // Past 10s cooldown: immediate disable + single reload even if clicked twice.
    nowSpy.mockReturnValue(baseNow + 10_001);
    fireEvent.click(resetBtn);
    expect(resetBtn).toBeDisabled();
    fireEvent.click(resetBtn);
    expect(reloadSpy).toHaveBeenCalledTimes(1);
  });

  it('syncs 1-bit draft status from RoomComposer (text and attachment) without re-rendering Host, and gates RESET KOTA', async () => {
    const reloadSpy = vi.spyOn(systemLoad, 'reloadKotaWindow').mockImplementation(() => {});
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async () => () => {});
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(makeSample());

    const composerRef = createRef<InputBarHandle>();
    let hostRenders = 0;

    function Host() {
      hostRenders += 1;
      const hasDraftRef = useRef(false);
      const onHasDraftChange = useCallback((hasDraft: boolean) => {
        hasDraftRef.current = hasDraft;
      }, []);
      const readHasDraft = useCallback(() => hasDraftRef.current, []);
      return (
        <div>
          <RoomComposer
            ref={composerRef}
            targetAgent="alice"
            onSend={vi.fn()}
            onHasDraftChange={onHasDraftChange}
          />
          <RightColumn
            sceneKey="conversation"
            onOpenHotMem={vi.fn()}
            hasComposerDraft={readHasDraft}
          />
        </div>
      );
    }

    await act(async () => {
      render(<Host />);
    });
    const initialRenders = hostRenders;

    const field = screen.getByTestId('input-field');
    await userEvent.type(field, 'unsent note');
    expect(hostRenders).toBe(initialRenders);

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    fireEvent.click(screen.getByRole('button', { name: 'RESET KOTA' }));

    const dialog = screen.getByRole('dialog', { name: 'Reset will clear composer drafts.' });
    const cancelBtn = within(dialog).getByRole('button', { name: 'Cancel' });
    expect(document.activeElement).toBe(cancelBtn);

    fireEvent.click(cancelBtn);
    expect(reloadSpy).not.toHaveBeenCalled();

    // Clear text and insert an attachment-only draft: still prompts before Reset.
    act(() => {
      composerRef.current?.clear();
      composerRef.current?.insertAttachment({
        path: '/tmp/diagram.png',
        name: 'diagram.png',
        kind: 'image',
      });
    });

    fireEvent.click(screen.getByRole('button', { name: 'RESET KOTA' }));
    const attachmentDialog = screen.getByRole('dialog', { name: 'Reset will clear composer drafts.' });
    fireEvent.click(within(attachmentDialog).getByRole('button', { name: 'Proceed' }));
    expect(reloadSpy).toHaveBeenCalledTimes(1);
  });

  it('gates Good Night on drafts, preserves worked clock and RESET KOTA on midway return, and reloads after Dream finishes', async () => {
    vi.useFakeTimers();
    const startMs = Date.now() + 60_000;
    vi.setSystemTime(startMs);

    const reloadSpy = vi.spyOn(systemLoad, 'reloadKotaWindow').mockImplementation(() => {});
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
        new Promise((resolve) => {
          resolveConsolidation = () =>
            resolve({
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

    const workspace = makeWorkspace('dream-flow-room');
    let hasDraft = true;
    const view = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        onOpenHotMem={vi.fn()}
        hasComposerDraft={() => hasDraft}
        dreamProjects={[
          {
            projectId: workspace.projectId,
            projectRoot: workspace.localRoot,
            projectName: workspace.projectId,
            agents: [{ id: 'alice', name: 'Alice' }],
          },
        ]}
      />,
    );

    // Advance 5 minutes so the worked clock clearly reads >= 5m.
    act(() => {
      vi.advanceTimersByTime(5 * 60 * 1000);
    });
    const meterBefore = view.container.querySelector('.ember-worked-meter')!;
    const labelBeforeDream = meterBefore.getAttribute('aria-label')!;
    expect(labelBeforeDream).not.toContain('0 hours 0 minutes 0 seconds');

    // Good Night with draft: opens confirm; Cancel stays in room.
    toggleRockerSwitch('Good Night');
    const dreamConfirm = screen.getByRole('dialog', { name: 'Dream will clear composer drafts.' });
    expect(document.activeElement).toBe(within(dreamConfirm).getByRole('button', { name: 'Cancel' }));
    fireEvent.click(within(dreamConfirm).getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByText('Start dreaming in')).not.toBeInTheDocument();

    // Proceed enters Dream countdown.
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Good Night');
    fireEvent.click(
      within(screen.getByRole('dialog', { name: 'Dream will clear composer drafts.' })).getByRole('button', {
        name: 'Proceed',
      }),
    );
    expect(screen.getByText('Start dreaming in')).toBeInTheDocument();

    // Start dreaming and trigger background consolidation (still pending).
    fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    await act(async () => {
      vi.advanceTimersByTime(120_000);
      await Promise.resolve();
    });

    // Midway return while consolidation is pending: NO reload, worked clock NOT zeroed, RESET KOTA NOT disabled.
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Back to Kota');
    expect(reloadSpy).not.toHaveBeenCalled();

    const meterAfterMidway = view.container.querySelector('.ember-worked-meter')!;
    expect(meterAfterMidway.getAttribute('aria-label')).toContain('00 Hrs 08 Min');

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    expect(screen.getByRole('button', { name: 'RESET KOTA' })).not.toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));

    // Finish pending consolidation, then enter Dream without draft and complete Dream -> Back to Kota reloads directly.
    await act(async () => {
      resolveConsolidation?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    hasDraft = false;
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Good Night');
    expect(screen.queryByRole('dialog', { name: 'Dream will clear composer drafts.' })).not.toBeInTheDocument();
    expect(screen.getByText('Start dreaming in')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    await act(async () => {
      vi.advanceTimersByTime(120_000);
      await Promise.resolve();
    });
    await act(async () => {
      resolveConsolidation?.();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Even if drafts reappear or within 10s post-load window, Back to Kota after completed Dream reloads directly.
    hasDraft = true;
    recordSystemLoadPageLoadedAt(Date.now());
    act(() => {
      vi.advanceTimersByTime(2_000);
    });
    toggleRockerSwitch('Back to Kota');
    expect(reloadSpy).toHaveBeenCalledTimes(1);
  });

  it('exposes window.__KOTA_SYSTEM_LOAD_DEBUG__ only when VITE_KOTA_LOCAL_TEST_AGENT is set', async () => {
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async () => () => {});
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(null);

    const prodView = render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    expect(window.__KOTA_SYSTEM_LOAD_DEBUG__).toBeUndefined();
    prodView.unmount();

    vi.stubEnv('VITE_KOTA_LOCAL_TEST_AGENT', 'local-tester');
    const testView = render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    expect(window.__KOTA_SYSTEM_LOAD_DEBUG__).toBeDefined();

    act(() => {
      window.__KOTA_SYSTEM_LOAD_DEBUG__!.set({
        totalRamGb: 32,
        compressedGb: 6.4,
        kotaGb: 4.8,
        swapMbps: 12.5,
        y: 0.4,
        r: 0.8,
      });
    });

    const head = testView.container.querySelector('.ember-room-head')!;
    expect(head).toHaveClass('ram-lit');

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    const panel = screen.getByRole('dialog', { name: 'SYSTEM LOAD' });
    expect(within(panel).getByText('TOTAL RAM 32 GB')).toBeInTheDocument();
    expect(within(panel).getByText('6.4')).toBeInTheDocument();
    expect(within(panel).getByText('4.8')).toBeInTheDocument();
    expect(within(panel).getByText('12.50 MBps')).toBeInTheDocument();

    testView.unmount();
    expect(window.__KOTA_SYSTEM_LOAD_DEBUG__).toBeUndefined();
  });

  it('treats pre-load snapshot kotaPageBytes and intervalMs as null (-- on KOTA and unlit) until the first post-load sample arrives', async () => {
    recordSystemLoadPageLoadedAt(50_000);

    let emitSample: ((sample: SystemLoadSample) => void) | null = null;
    vi.spyOn(ptyClient, 'onSystemLoadSample').mockImplementation(async (cb) => {
      emitSample = cb;
      return () => {};
    });
    vi.spyOn(ptyClient, 'systemLoadSnapshot').mockResolvedValue(
      makeSample({
        atMs: 45_000,
        intervalMs: 30_000,
        totalRamBytes: 16 * BYTES_PER_GIB,
        compressedBytes: 6.2 * BYTES_PER_GIB,
        kotaPageBytes: 5.2 * BYTES_PER_GIB,
        swapBytesPerSec: 1.5 * BYTES_PER_MIB,
      }),
    );

    const view = render(<RightColumn sceneKey="conversation" onOpenHotMem={vi.fn()} />);
    await act(async () => {
      await Promise.resolve();
    });

    // Pre-reload kotaPageBytes (5.2 GB) and intervalMs (30s) must not heat the lamp or show old KOTA footprint.
    const head = view.container.querySelector('.ember-room-head')!;
    expect(head).not.toHaveClass('ram-lit');

    fireEvent.click(screen.getByRole('button', { name: 'SYSTEM LOAD' }));
    const panel = screen.getByRole('dialog', { name: 'SYSTEM LOAD' });
    expect(within(panel).getByText('TOTAL RAM 16 GB')).toBeInTheDocument();
    expect(within(panel).getByText('6.2')).toBeInTheDocument();
    expect(within(panel).getByText('1.50 MBps')).toBeInTheDocument();
    expect(panel.querySelector('.kota-number')).toHaveTextContent('--');
    expect(panel.querySelector('.hud-unit')).toHaveClass('compressed-yellow');
    expect(panel.querySelector('.hud-unit')).not.toHaveClass('kota-yellow');

    // First post-load sample arrives with the fresh WebView process footprint (0.9 GB).
    await act(async () => {
      emitSample?.(
        makeSample({
          atMs: 75_000,
          intervalMs: 30_000,
          totalRamBytes: 16 * BYTES_PER_GIB,
          compressedBytes: 6.0 * BYTES_PER_GIB,
          kotaPageBytes: 0.9 * BYTES_PER_GIB,
          swapBytesPerSec: 0.25 * BYTES_PER_MIB,
        }),
      );
    });

    expect(panel.querySelector('.kota-number')).toHaveTextContent('0.9');
    expect(panel.querySelector('.hud-unit')).not.toHaveClass('kota-yellow');
  });
});
