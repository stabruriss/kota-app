import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createRef, useState, type ComponentProps } from 'react';
import { act, render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../src/App';
import { formatStorageMeasurementAge } from '../src/chrome/TavernModal';
import { FileTree } from '../src/chrome/FileTree';
import { AgentWindowsLayer, terminalDropTargetAtPosition, type AgentWindowsLayerHandle } from '../src/chrome/AgentWindowsLayer';
import {
  TerminalGrid,
  TERMINAL_CELL_WIDTH,
  TERMINAL_LINE_HEIGHT,
} from '../src/chrome/TerminalGrid';
import { ATTR_WIDE, type AgentCli, type GridSnapshot } from '../src/types/agent-pty';
import { __resetMockSmartPtyForTests } from '../src/pty-client';
import * as ptyClient from '../src/pty-client';
import {
  InputBar,
  escapePromptPath,
  type ComposerAttachment,
  type InputBarHandle,
} from '../src/chrome/InputBar';
import {
  MarkdownText,
  VioletRoomPanel,
  mergeSyncedNativeMessages,
} from '../src/chrome/VioletRoomPanel';
import { Stage } from '../src/chrome/Stage';
import { RightColumn } from '../src/chrome/RightColumn';
import {
  createEmberHistoryRecord,
  createEmberSchedule,
  EMBER_NOT_DELIVERED,
  emberStorageKey,
  HUMAN_TELEGRAM_TARGET_ID,
} from '../src/ember-config';
import { emitFileTreeAgentHover } from '../src/lib/file-tree-agent-hover';
import {
  __clearVioletComposerSentHistoryForTests,
  emitVioletComposerDelivery,
  emitVioletComposerSent,
  VIOLET_COMPOSER_AGENT_EXIT_REASON,
  violetComposerSentHistory,
} from '../src/chrome/violet-room-events';
import type { AgentId } from '../src/types/scene';
import { createAgentGridStore } from '../src/lib/agent-grid-store';
import type { WorkspaceTreeListing, WorkspaceTreePathRequest } from '../src/types/tree';
import violetMessageOrderCases from './fixtures/violet-message-order.json';
import {
  parseRoomQuotePrompt,
  serializeRoomQuotePrompt,
  type RoomQuoteReference,
} from '../src/lib/room-quote';

beforeEach(() => {
  vi.restoreAllMocks();
  try {
    window.localStorage.setItem('kota-v2.dev.project-root', '/tmp/kota-test');
    window.localStorage.removeItem('kota-v2.tavern.hero-profiles');
    window.localStorage.removeItem('kota-v2.tavern.custom-heroes');
    window.localStorage.removeItem('kota-v2.tavern.system-heroes');
    window.localStorage.removeItem('kota-v2.bartender.auto-sync.proj');
  } catch {
    /* ignore */
  }
  __clearVioletComposerSentHistoryForTests();
  recruitedHeroAgents['hero-cc'] = [];
  recruitedHeroAgents['hero-dex'] = [];
});

const recruitedHeroAgents: Record<'hero-cc' | 'hero-dex', string[]> = {
  'hero-cc': [],
  'hero-dex': [],
};

function hydrationAgent(agentId: string, projectRoot: string): ptyClient.WorkspaceProject['agents'][number] {
  return {
    agentId,
    cli: 'codex',
    cwd: `${projectRoot}/.agent-workspaces/${agentId}`,
    projectRoot,
    worktreeRoot: `${projectRoot}/.agent-workspaces/${agentId}/project-files`,
    sharedDir: `${projectRoot}/project-memory`,
    rulesDir: `${projectRoot}/project-rules`,
    adapterPath: `${projectRoot}/.agent-workspaces/${agentId}/AGENTS.md`,
    args: ['--model', 'default'],
    projectId: projectRoot.split('/').at(-1),
    projectBaseRef: 'origin/main',
  };
}

function hydrationWorkspace(
  projectId: string,
  agentIds: readonly string[],
): ptyClient.WorkspaceProject {
  const localRoot = `/tmp/kota-hydration/${projectId}`;
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
    agents: agentIds.map((agentId) => hydrationAgent(agentId, localRoot)),
  };
}

function hydrationIdentity(agentId: string, displayName = agentId): ptyClient.ProjectAgentIdentity {
  return {
    agentId,
    displayName,
    sourceHeroId: 'hero-dex',
    status: 'active',
    provider: 'codex',
    avatarId: null,
  };
}

function hydrationDetail(
  agentId: string,
  projectRoot: string,
  displayName = agentId,
): ptyClient.ProjectAgentDetail {
  return {
    agentId,
    displayName,
    sourceHeroId: 'hero-dex',
    sourceHeroName: 'Dex',
    projectId: projectRoot.split('/').at(-1) ?? 'hydration-project',
    projectName: projectRoot.split('/').at(-1) ?? 'hydration-project',
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

async function flushHydrationPromises(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

function withMockLocalStorage(seed: Record<string, string> = {}) {
  const storage = new Map<string, string>(Object.entries(seed));
  const originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: {
      getItem: vi.fn((key: string) => storage.get(key) ?? null),
      setItem: vi.fn((key: string, value: string) => storage.set(key, String(value))),
      removeItem: vi.fn((key: string) => storage.delete(key)),
    },
  });
  return {
    storage,
    restore: () => {
      if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage);
    },
  };
}

/** W5/v2 helper — opens target picker from the always-mounted composer. */
async function enableBroadcastMode() {
  const alice = await recruitHero('hero-cc');
  const bob = await recruitHero('hero-dex');
  await userEvent.click(chip(alice));
  await userEvent.click(screen.getByTestId('ib-target-pill'));
  await screen.findByTestId('broadcast-target-popover');
  expect(broadcastOption(alice)).toHaveClass('selected');
  await userEvent.click(broadcastOption(bob));
  await userEvent.click(screen.getByTestId('broadcast-confirm'));
  await waitFor(() => {
    expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).toHaveClass('broadcast');
  });
}

function composerText(): string {
  return (screen.getByTestId('input-field') as HTMLElement).textContent ?? '';
}

async function recruitHero(id: 'hero-cc' | 'hero-dex') {
  const before = currentAgentChipIds();
  await userEvent.click(screen.getByTestId('ribbon-add'));
  await userEvent.click(await screen.findByTestId(`incarnate-shortcut-${id}`));
  const agentId = await waitForNewAgentChip(before);
  recruitedHeroAgents[id].push(agentId);
  return agentId;
}

function chip(agentId: string) {
  return screen.getByTestId(`chip-${agentId}`);
}

function winFrame(agentId: string) {
  return screen.getByTestId(`win-frame-${agentId}`);
}

function broadcastOption(agentId: string) {
  return screen.getByTestId(`broadcast-option-${agentId}`);
}

function currentAgentChipIds(): Set<string> {
  return new Set(
    screen.queryAllByTestId(/^chip-/)
      .map((element) => element.getAttribute('data-testid')?.replace(/^chip-/, '') ?? '')
      .filter((id) => id && !id.startsWith('empty-') && id !== 'all'),
  );
}

async function waitForNewAgentChip(before: ReadonlySet<string>): Promise<string> {
  let nextId = '';
  await waitFor(() => {
    const next = [...currentAgentChipIds()].filter((id) => !before.has(id));
    expect(next.length).toBeGreaterThan(0);
    [nextId] = next;
    expect(chip(nextId)).toBeInTheDocument();
  });
  return nextId;
}

describe('Composer · bottom input bar', () => {
  function Harness({
    onSend = vi.fn(),
    onPasteImage,
    onMaterializeAttachments,
  }: {
    onSend?: ReturnType<typeof vi.fn>;
    onPasteImage?: (file: File) => Promise<string | null>;
    onMaterializeAttachments?: ComponentProps<typeof InputBar>['onMaterializeAttachments'];
  }) {
    const [value, setValue] = useState('line one\nline two');
    return (
      <InputBar
        value={value}
        onChange={setValue}
        captainId="judy"
        targetAgent="alice"
        onSend={onSend}
        onPasteImage={onPasteImage}
        onMaterializeAttachments={onMaterializeAttachments}
      />
    );
  }

  it('sends a multi-line prompt as one payload', async () => {
    const onSend = vi.fn();
    render(<Harness onSend={onSend} />);
    await userEvent.click(screen.getByTestId('ib-send'));
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith('alice', 'line one\nline two', {
        broadcast: false,
        privacy: false,
      });
    });
  });

  it('ignores repeated sends while delivery is still pending', async () => {
    let resolveSend: (value?: boolean | void) => void = () => {};
    const onSend = vi.fn(() => new Promise<boolean | void>((resolve) => {
      resolveSend = resolve;
    }));
    const user = userEvent.setup();
    render(<Harness onSend={onSend} />);

    const send = screen.getByTestId('ib-send');
    const field = screen.getByTestId('input-field');
    await user.click(send);
    await waitFor(() => expect(send).toBeDisabled());

    await user.click(send);
    fireEvent.keyDown(field, { key: 'Enter', metaKey: true });
    expect(onSend).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveSend(true);
    });
    await waitFor(() => expect(field).toHaveTextContent(''));
  });

  it('ignores private mode while privacy controls are parked', async () => {
    const onSend = vi.fn();
    render(
      <InputBar
        value="secret"
        onChange={() => {}}
        targetAgent="alice"
        privacyMode
        onPrivacyToggle={vi.fn()}
        onSend={onSend}
      />,
    );

    expect(screen.queryByTestId('ib-privacy-tool')).not.toBeInTheDocument();
    expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).not.toHaveClass('private');

    await userEvent.click(screen.getByTestId('ib-send'));
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith('alice', 'secret', {
        broadcast: false,
        privacy: false,
      });
    });
  });

  it('allows sending to a selected sleeping agent so App can wake it first', async () => {
    const onSend = vi.fn();
    render(
      <InputBar
        value="wake me"
        onChange={() => {}}
        targetAgent="alice"
        onSend={onSend}
      />,
    );

    const send = screen.getByTestId('ib-send');
    expect(send).not.toBeDisabled();
    expect(send).toHaveAttribute('title', 'Send (⌘↵)');
    await userEvent.click(send);
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith('alice', 'wake me', {
        broadcast: false,
        privacy: false,
      });
    });
  });

  it('keeps the draft when App handles send without delivery', async () => {
    const onSend = vi.fn(async () => false);
    const user = userEvent.setup();
    render(
      <InputBar
        value="wake me"
        onChange={() => {}}
        targetAgent="alice"
        onSend={onSend}
      />,
    );

    await user.click(screen.getByTestId('ib-send'));

    expect(onSend).toHaveBeenCalled();
    expect(screen.getByTestId('input-field')).toHaveTextContent('wake me');
  });

  it('submits with Cmd+Enter while plain Enter remains composer editing', async () => {
    const onSend = vi.fn();
    render(<Harness onSend={onSend} />);
    const field = screen.getByTestId('input-field') as HTMLElement;
    field.replaceChildren();
    fireEvent.input(field);
    await userEvent.type(field, 'first{enter}second');
    expect(onSend).not.toHaveBeenCalled();

    fireEvent.keyDown(field, { key: 'Enter', metaKey: true });
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith('alice', 'first\nsecond', {
        broadcast: false,
        privacy: false,
      });
    });
  });

  it('opens the @ mention picker inline, not only at line start', async () => {
    render(
      <InputBar
        value=""
        onChange={() => {}}
        targetAgent="alice"
        mentionAgentIds={['alice', 'bob']}
        onSend={vi.fn()}
      />,
    );
    const field = screen.getByTestId('input-field') as HTMLElement;
    await userEvent.click(field);
    await userEvent.type(field, 'ask@');

    const popover = await screen.findByTestId('ib-mention-popover');
    expect(popover).toBeInTheDocument();
    expect(within(popover).getAllByText('CC').length).toBeGreaterThan(0);
  });

  it('turns pasted screenshots into project-memory attachment paths', async () => {
    const shot = new File(['image-bytes'], 'shot.png', { type: 'image/png' });
    const onPasteImage = vi.fn(async () => '/tmp/kota-test/project-memory/attachments/composer/att_123/original.png');
    const onSend = vi.fn();
    render(<Harness onPasteImage={onPasteImage} onSend={onSend} />);
    fireEvent.paste(screen.getByTestId('input-field'), {
      clipboardData: {
        files: [shot],
        getData: () => '',
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('ib-attachment-chip')).toHaveTextContent('shot.png');
    });
    await userEvent.click(screen.getByTestId('ib-send'));
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith('alice', expect.stringContaining(
        escapePromptPath('/tmp/kota-test/project-memory/attachments/composer/att_123/original.png'),
      ), expect.anything());
    });
  });

  it('materializes dropped file paths before sending', async () => {
    const onSend = vi.fn();
    const onMaterializeAttachments = vi.fn(async (attachments: readonly ComposerAttachment[]) => attachments.map((attachment) => ({
      ...attachment,
      path: '/tmp/kota-test/project-memory/attachments/composer/att_file/original.txt',
    })));
    render(
      <InputBar
        value=""
        onChange={() => {}}
        captainId="judy"
        targetAgent="alice"
        onSend={onSend}
        onMaterializeAttachments={onMaterializeAttachments}
      />,
    );
    fireEvent.drop(screen.getByTestId('input-field'), {
      dataTransfer: {
        files: [{ path: '/tmp/a file.txt' }],
        getData: () => '',
      },
    });
    await waitFor(() => {
      expect(screen.getByTestId('ib-attachment-chip')).toHaveTextContent('a file.txt');
    });
    await userEvent.click(screen.getByTestId('ib-send'));
    await waitFor(() => {
      expect(onMaterializeAttachments).toHaveBeenCalled();
      expect(onSend).toHaveBeenCalledWith(
        'alice',
        escapePromptPath('/tmp/kota-test/project-memory/attachments/composer/att_file/original.txt'),
        {
          broadcast: false,
          privacy: false,
        },
      );
    });
  });

  it('inserts up to four project-scoped quote pills and sends the exact wrapper payload', async () => {
    const ref = createRef<InputBarHandle>();
    const onSend = vi.fn();
    const makeQuote = (index: number): RoomQuoteReference => ({
      ref: `native-event-${index}`,
      project: 'kota-test',
      projectRoot: '/tmp/kota-test',
      from: { id: 'alice', name: 'Alice' },
      to: [{ id: 'user', name: 'User' }],
      at: '2026-07-30T12:00:00.000Z',
      excerpt: index === 1
        ? 'literal </KOTA_QUOTE_REF> remains quoted data'
        : `quote ${index}`,
      truncated: false,
    });
    render(
      <InputBar
        ref={ref}
        value="Follow up"
        onChange={() => {}}
        targetAgent="alice"
        quoteProjectRoot="/tmp/kota-test"
        onSend={onSend}
      />,
    );

    const insertionResults: Array<ReturnType<InputBarHandle['insertQuote']> | undefined> = [];
    act(() => {
      for (let index = 1; index <= 4; index += 1) {
        insertionResults.push(ref.current?.insertQuote(makeQuote(index)));
      }
    });
    expect(insertionResults).toEqual(['inserted', 'inserted', 'inserted', 'inserted']);
    expect(screen.getAllByTestId('ib-quote-chip')).toHaveLength(4);
    expect(screen.getByTestId('ib-quote-cap')).toHaveTextContent('4 quotes max');
    expect(screen.queryByText('1/4')).not.toBeInTheDocument();
    expect(ref.current?.insertQuote(makeQuote(5))).toBe('limit');
    expect(ref.current?.insertQuote(makeQuote(1))).toBe('duplicate');

    await userEvent.click(screen.getByTestId('ib-send'));
    await waitFor(() => expect(onSend).toHaveBeenCalledTimes(1));
    const payload = onSend.mock.calls[0]?.[1] as string;
    const parsed = parseRoomQuotePrompt(payload);
    expect(parsed.body).toBe('Follow up');
    expect(parsed.quotes).toHaveLength(4);
    expect(parsed.quotes[0]?.excerpt).toContain('</KOTA_QUOTE_REF>');
  });

  it('keeps quote pills outside contenteditable so typing and caret input remain stable', async () => {
    const ref = createRef<InputBarHandle>();
    const quote: RoomQuoteReference = {
      ref: 'native-event-caret',
      project: 'kota-test',
      projectRoot: '/tmp/kota-test',
      from: { id: 'alice', name: 'Alice' },
      to: [{ id: 'user', name: 'User' }],
      at: '2026-07-30T12:00:00.000Z',
      excerpt: 'Quoted context must not become a contenteditable child.',
      truncated: false,
    };
    function ControlledQuoteComposer() {
      const [value, setValue] = useState('');
      return (
        <InputBar
          ref={ref}
          value={value}
          onChange={setValue}
          targetAgent="alice"
          quoteProjectRoot="/tmp/kota-test"
          onSend={vi.fn()}
        />
      );
    }
    render(<ControlledQuoteComposer />);

    act(() => {
      expect(ref.current?.insertQuote(quote)).toBe('inserted');
    });
    const field = screen.getByTestId('input-field');
    const pill = screen.getByTestId('ib-quote-chip');
    expect(field.contains(pill)).toBe(false);
    expect(pill.querySelector('svg')).not.toBeNull();

    await userEvent.click(field);
    await userEvent.type(field, 'typing after quote');
    expect(field).toHaveTextContent('typing after quote');
    const parsed = parseRoomQuotePrompt(ref.current?.serialize().payload ?? '');
    expect(parsed.body).toBe('typing after quote');
    expect(parsed.quotes).toHaveLength(1);

    await userEvent.click(screen.getByRole('button', { name: 'Remove quote from Alice' }));
    expect(screen.queryByTestId('ib-quote-chip')).not.toBeInTheDocument();
    expect(field).toHaveTextContent('typing after quote');
    expect(parseRoomQuotePrompt(ref.current?.serialize().payload ?? '')).toEqual({
      quotes: [],
      body: 'typing after quote',
    });
  });

  it('rejects quote pills from another project', () => {
    const ref = createRef<InputBarHandle>();
    render(
      <InputBar
        ref={ref}
        value=""
        onChange={() => {}}
        targetAgent="alice"
        quoteProjectRoot="/tmp/current"
        onSend={vi.fn()}
      />,
    );
    expect(ref.current?.insertQuote({
      ref: 'other-event',
      project: 'other',
      projectRoot: '/tmp/other',
      from: { id: 'alice', name: 'Alice' },
      to: [{ id: 'user', name: 'User' }],
      at: '2026-07-30T12:00:00.000Z',
      excerpt: 'cross-project content',
      truncated: false,
    })).toBe('blocked');
    expect(screen.queryByTestId('ib-quote-chip')).not.toBeInTheDocument();
  });
});

// ═════════════════════════════ shell landmarks ═════════════════════════════
describe('M1 · canvas shell landmarks', () => {
  it('renders the top bar with real project controls + Tavern button', () => {
    render(<App />);
    const bar = screen.getByRole('banner', { name: 'Project bar' });
    expect(bar).toBeInTheDocument();
    // Placeholder project tabs are not mounted; only real workspace tabs appear.
    expect(screen.queryByTestId('tab-kota')).not.toBeInTheDocument();
    expect(screen.getByTestId('tab-plus')).toBeInTheDocument();
    expect(screen.getByTestId('tavern-btn')).toBeInTheDocument();
  });

  it('shows Stage tools row and the lowered agent ribbon above the composer', () => {
    const { container } = render(<App />);
    expect(container.querySelector('.stage-tools')).toBeInTheDocument();
    expect(container.querySelector('.stage-bottom-dock .ribbon-wrap')).toBeInTheDocument();
    expect(container.querySelector('.stage-bottom-dock .input-bar-wrap')).toBeInTheDocument();
    expect(screen.getByTestId('picker-trigger')).toBeInTheDocument();
    expect(screen.queryByTestId('chip-all')).not.toBeInTheDocument();
    expect(screen.getByTestId('ribbon-add')).toBeInTheDocument();
    expect(screen.getByTestId('ribbon-filter-clear')).toBeInTheDocument();
    expect(screen.getByTestId('ribbon-filter-clear')).toBeEnabled();
    expect(screen.getByTestId('ribbon-filter-clear')).toHaveAttribute('data-chat-filter-mode', 'all');
    expect(screen.getByTestId('ribbon-filter-clear')).toHaveAccessibleName('Toggle chat filter. Current: All chats.');
    expect(screen.getByText('Click to toggle')).toBeInTheDocument();
    expect(screen.getByText('Every agent · current')).toBeInTheDocument();
    expect(screen.queryByTestId('ribbon-shortcuts')).not.toBeInTheDocument();
  });

  it('shows 8 empty recruit seats', () => {
    const { container } = render(<App />);
    expect(container.querySelectorAll('.seat').length).toBe(8);
    expect(screen.getAllByText('Open seat').length).toBe(8);
  });

  it('right column has system agents and memory summary, with Hot Memory parked', () => {
    render(<App />);
    expect(screen.getByLabelText(/^Worked for \d{2} Hrs \d{2} Min \d{2} Sec$/)).toBeInTheDocument();
    expect(screen.getByText('Bartender')).toBeInTheDocument();
    expect(screen.getByLabelText('Open Violet summary history')).toBeInTheDocument();
    expect(screen.queryByText('Hot Memory')).not.toBeInTheDocument();
    expect(screen.queryByText(/15 records/)).not.toBeInTheDocument();
  });

  it('keeps the human Ember target available without Laughing Man', async () => {
    const workspace = hydrationWorkspace('ember-human-target', []);
    vi.spyOn(ptyClient, 'lmStatus').mockResolvedValue(null);
    vi.spyOn(ptyClient, 'loadAccountUserIdentity').mockResolvedValue({
      name: 'Human Tester',
      avatarId: 'user-default',
    });

    const { container } = render(
      <RightColumn
        sceneKey="conversation"
        onOpenHotMem={vi.fn()}
        workspace={workspace}
        projectRoot={workspace.localRoot}
      />,
    );

    await userEvent.click(screen.getByLabelText('Open Ember schedules and drafts'));
    await userEvent.click(screen.getByRole('button', { name: 'New' }));
    const humanTarget = await screen.findByRole('button', { name: 'Human Tester' });

    expect(humanTarget).toHaveClass('human');
    expect(screen.queryByText('No active agent')).not.toBeInTheDocument();
    expect(container).not.toHaveTextContent(HUMAN_TELEGRAM_TARGET_ID);
  });

  it('shows an Ember delivery alert while a not-delivered schedule remains', async () => {
    const workspace = hydrationWorkspace('ember-delivery-alert', []);
    const schedule = {
      ...createEmberSchedule({
        text: 'Retry this reminder',
        targetAgentId: HUMAN_TELEGRAM_TARGET_ID,
        targetAgentName: 'Human Tester',
        mode: 'delay',
        delayAmount: 10,
        delayUnit: 'minutes',
      }),
      error: 'app not running',
    };
    const storageKey = emberStorageKey(workspace.projectId);
    expect(storageKey).not.toBeNull();
    const mockStorage = withMockLocalStorage({
      [storageKey!]: JSON.stringify({
        drafts: [],
        schedules: [schedule],
        history: [
          createEmberHistoryRecord(
            schedule,
            'failed',
            new Date().toISOString(),
            'app not running',
          ),
        ],
      }),
    });
    const view = render(
      <RightColumn
        sceneKey="conversation"
        onOpenHotMem={vi.fn()}
        workspace={workspace}
        projectRoot={workspace.localRoot}
      />,
    );

    try {
      const emberCard = screen.getByLabelText('Open Ember schedules and drafts, not delivered');
      expect(emberCard.querySelector('.ember-delivery-alert')).toBeInTheDocument();

      fireEvent.click(emberCard);
      expect(screen.getAllByText(EMBER_NOT_DELIVERED).length).toBeGreaterThan(0);
      fireEvent.click(screen.getByRole('button', { name: 'Delete' }));

      await waitFor(() => {
        expect(screen.getByLabelText('Open Ember schedules and drafts')).toBeInTheDocument();
        expect(view.container.querySelector('.ember-delivery-alert')).not.toBeInTheDocument();
      });
    } finally {
      view.unmount();
      mockStorage.restore();
    }
  });

  it('surfaces automatic Violet summary failures', async () => {
    const summaryState: ptyClient.VioletSummaryState = {
      latest: null,
      history: [],
      outstanding: {
        sinceTs: '2026-06-04T05:00:00Z',
        messageCount: 30,
      },
      logPath: 'project-memory/chathistory/summaries/recent.json',
      promptPath: '$KOTA_HOME/heroes/system-violet/violet-summary.md',
      updatedAt: '2026-06-04T05:01:00Z',
    };
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota-test',
      localRootBytes: 0,
      sourceDir: '/tmp/kota-test/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota-test/project-memory',
      rulesDir: '/tmp/kota-test/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    vi.spyOn(ptyClient, 'readVioletSummary').mockResolvedValue(summaryState);
    vi.spyOn(ptyClient, 'summarizeVioletAuto').mockResolvedValue({
      ...summaryState,
      error: 'spawn codex summary CLI: No such file or directory',
    });

    render(
      <RightColumn
        sceneKey="conversation"
        onOpenHotMem={vi.fn()}
        workspace={workspace}
        projectRoot="/tmp/kota-test"
      />,
    );

    expect(await screen.findByText('spawn codex summary CLI: No such file or directory'))
      .toBeInTheDocument();
  });

  it('fans out Dream prompts and consolidates delivered projects in one account batch', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-06-10T12:00:00Z'));
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    const send = vi.spyOn(ptyClient, 'agentBusSend').mockImplementation(async (request) => ({
      eventId: request.eventId || `event-${request.target}`,
      targetAgentId: request.target,
      submitted: true,
      duplicate: false,
      skippedReason: null,
    }));
    vi.spyOn(ptyClient, 'emberPrepareDreams').mockImplementation(async (request) => ({
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      archiveDir: '/tmp/Kota/dreams/archive',
      projectDreamsPath: `${request.projectRoot}/project-memory/dreams.md`,
      projected: false,
    }));
    const consolidate = vi.spyOn(ptyClient, 'emberConsolidateDreams').mockImplementation(async () => ({
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      oldDreamsPath: '/tmp/Kota/dreams/old_dreams.md',
      promptPath: '$KOTA_HOME/heroes/system-ember/ember-dream-consolidate.md',
      processedEntryCount: 1,
      activeEntryCount: 1,
      archivedEntryCount: 0,
      updatedAt: new Date().toISOString(),
      error: null,
    }));

    try {
      render(
        <RightColumn
          sceneKey="conversation"
          workspace={workspace}
          workingAgents={new Set()}
          onOpenHotMem={vi.fn()}
          dreamProjects={[
            {
              projectId: 'proj',
              projectRoot: '/tmp/kota/proj',
              projectName: 'proj',
              agents: [{ id: 'alice', name: 'Alice' }],
            },
            {
              projectId: 'other',
              projectRoot: '/tmp/kota/other',
              projectName: 'other',
              agents: [
                { id: 'bob', name: 'Bob' },
                { id: 'cara', name: 'Cara' },
              ],
            },
          ]}
        />,
      );

      const switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(send).toHaveBeenCalledTimes(3);
      expect(send.mock.calls.map(([request]) => [request.projectRoot, request.target])).toEqual([
        ['/tmp/kota/proj', 'alice'],
        ['/tmp/kota/other', 'bob'],
        ['/tmp/kota/other', 'cara'],
      ]);

      await act(async () => {
        vi.advanceTimersByTime(120_000);
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(consolidate).toHaveBeenCalledTimes(1);
      expect(consolidate).toHaveBeenCalledWith({
        projectRoots: ['/tmp/kota/proj', '/tmp/kota/other'],
        provider: 'codex',
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it('pauses the Dream deadline and resumes from the same rounded-up remainder', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-06-10T12:00:00Z'));
    const workspace = hydrationWorkspace('dream-pause-resume', []);
    const send = vi.spyOn(ptyClient, 'agentBusSend').mockImplementation(async (request) => ({
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
      projectDreamsPath: `${workspace.localRoot}/project-memory/dreams.md`,
      projected: false,
    });
    const view = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        onOpenHotMem={vi.fn()}
        dreamProjects={[{
          projectId: workspace.projectId,
          projectRoot: workspace.localRoot,
          projectName: workspace.projectId,
          agents: [{ id: 'alice', name: 'Alice' }],
        }]}
      />,
    );

    try {
      const switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      const countdown = screen.getByText('Start dreaming in').closest('.ember-dream-countdown') as HTMLElement;

      expect(within(countdown).getByText('10:00')).toBeInTheDocument();
      act(() => vi.advanceTimersByTime(90_000));
      expect(within(countdown).getByText('8:30')).toBeInTheDocument();

      fireEvent.click(within(countdown).getByRole('button', { name: 'Pause' }));
      act(() => vi.advanceTimersByTime(600_000));
      expect(within(countdown).getByText('8:30')).toBeInTheDocument();
      expect(send).not.toHaveBeenCalled();

      fireEvent.click(within(countdown).getByRole('button', { name: 'Resume' }));
      act(() => vi.advanceTimersByTime(509_000));
      expect(within(countdown).getByText('0:01')).toBeInTheDocument();
      expect(send).not.toHaveBeenCalled();

      await act(async () => {
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(send).toHaveBeenCalledTimes(1);
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('waits at zero for an in-flight Dream digest, then starts exactly once when it clears', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-06-10T12:00:00Z'));
    const workspace = hydrationWorkspace('dream-digest-wait', []);
    const send = vi.spyOn(ptyClient, 'agentBusSend').mockImplementation(async (request) => ({
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
      projectDreamsPath: `${workspace.localRoot}/project-memory/dreams.md`,
      projected: false,
    });
    const consolidationResolvers: Array<() => void> = [];
    const consolidate = vi.spyOn(ptyClient, 'emberConsolidateDreams').mockImplementation(() => (
      new Promise((resolve) => {
        consolidationResolvers.push(() => resolve({
          accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
          entriesDir: '/tmp/Kota/dreams/entries',
          oldDreamsPath: '/tmp/Kota/dreams/old_dreams.md',
          promptPath: '$KOTA_HOME/heroes/system-ember/ember-dream-consolidate.md',
          processedEntryCount: 1,
          activeEntryCount: 1,
          archivedEntryCount: 0,
          updatedAt: new Date().toISOString(),
          error: null,
        }));
      })
    ));
    const view = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        onOpenHotMem={vi.fn()}
        dreamProjects={[{
          projectId: workspace.projectId,
          projectRoot: workspace.localRoot,
          projectName: workspace.projectId,
          agents: [{ id: 'alice', name: 'Alice' }],
        }]}
      />,
    );

    try {
      let switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(send).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(120_000);
        await Promise.resolve();
      });
      expect(consolidate).toHaveBeenCalledTimes(1);

      switchButton = screen.getByRole('switch', { name: 'Back to Kota' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      act(() => vi.advanceTimersByTime(1700));
      switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);

      await act(async () => {
        vi.advanceTimersByTime(600_000);
        await Promise.resolve();
      });
      const countdown = screen.getByText('Start dreaming in').closest('.ember-dream-countdown') as HTMLElement;
      expect(within(countdown).getByText('0:00')).toBeInTheDocument();
      expect(within(countdown).getByText('Last dreams is still being digested, waiting...')).toBeInTheDocument();
      expect(send).toHaveBeenCalledTimes(1);

      await act(async () => {
        consolidationResolvers[0]?.();
        await Promise.resolve();
        await Promise.resolve();
      });
      await act(async () => {
        vi.advanceTimersByTime(0);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(send).toHaveBeenCalledTimes(2);
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('reconciles a Dream overlay when the working update arrives before delivery completes', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-06-10T12:00:00Z'));
    const workspace = hydrationWorkspace('dream-working-race', []);
    let finishDelivery: (() => void) | null = null;
    const delivery = new Promise<void>((resolve) => {
      finishDelivery = resolve;
    });
    const send = vi.spyOn(ptyClient, 'agentBusSend').mockImplementation(async (request) => {
      await delivery;
      return {
        eventId: request.eventId || `event-${request.target}`,
        targetAgentId: request.target,
        submitted: true,
        duplicate: false,
        skippedReason: null,
      };
    });
    vi.spyOn(ptyClient, 'emberPrepareDreams').mockResolvedValue({
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      archiveDir: '/tmp/Kota/dreams/archive',
      projectDreamsPath: `${workspace.localRoot}/project-memory/dreams.md`,
      projected: false,
    });
    const onDreamingStatusAgentsChange = vi.fn();
    const dreamProjects = [{
      projectId: workspace.projectId,
      projectRoot: workspace.localRoot,
      projectName: workspace.projectId,
      agents: [{ id: 'alice', name: 'Alice' }],
    }];
    const renderColumn = (
      workingAgents: ReadonlySet<AgentId>,
      workingStartedAt: ReadonlyMap<AgentId, string>,
    ) => (
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={workingAgents}
        workingStartedAt={workingStartedAt}
        onOpenHotMem={vi.fn()}
        dreamProjects={dreamProjects}
        onDreamingStatusAgentsChange={onDreamingStatusAgentsChange}
      />
    );
    const view = render(renderColumn(new Set(), new Map()));

    try {
      const switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(send).toHaveBeenCalledTimes(1);

      view.rerender(renderColumn(
        new Set<AgentId>(['alice']),
        new Map<AgentId, string>([['alice', new Date().toISOString()]]),
      ));
      await act(async () => {
        await Promise.resolve();
      });

      await act(async () => {
        finishDelivery?.();
        await delivery;
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(onDreamingStatusAgentsChange).toHaveBeenLastCalledWith(['alice']);
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('does not consolidate Dream roots when every fanout send fails', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-06-10T12:00:00Z'));
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    vi.spyOn(ptyClient, 'agentBusSend').mockRejectedValue(new Error('offline'));
    vi.spyOn(ptyClient, 'emberPrepareDreams').mockResolvedValue({
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      archiveDir: '/tmp/Kota/dreams/archive',
      projectDreamsPath: '/tmp/kota/proj/project-memory/dreams.md',
      projected: false,
    });
    const consolidate = vi.spyOn(ptyClient, 'emberConsolidateDreams').mockResolvedValue({
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

    try {
      render(
        <RightColumn
          sceneKey="conversation"
          workspace={workspace}
          workingAgents={new Set()}
          onOpenHotMem={vi.fn()}
          dreamProjects={[
            {
              projectId: 'proj',
              projectRoot: '/tmp/kota/proj',
              projectName: 'proj',
              agents: [{ id: 'alice', name: 'Alice' }],
            },
            {
              projectId: 'other',
              projectRoot: '/tmp/kota/other',
              projectName: 'other',
              agents: [{ id: 'bob', name: 'Bob' }],
            },
          ]}
        />,
      );

      const switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      await act(async () => {
        vi.advanceTimersByTime(120_000);
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(consolidate).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps one account-wide Dream consolidation in flight', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-06-10T12:00:00Z'));
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    vi.spyOn(ptyClient, 'agentBusSend').mockImplementation(async (request) => ({
      eventId: request.eventId || `event-${request.target}`,
      targetAgentId: request.target,
      submitted: true,
      duplicate: false,
      skippedReason: null,
    }));
    vi.spyOn(ptyClient, 'emberPrepareDreams').mockImplementation(async (request) => ({
      accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
      entriesDir: '/tmp/Kota/dreams/entries',
      archiveDir: '/tmp/Kota/dreams/archive',
      projectDreamsPath: `${request.projectRoot}/project-memory/dreams.md`,
      projected: false,
    }));
    const startedRootBatches: string[][] = [];
    const resolvers: Array<() => void> = [];
    const consolidate = vi.spyOn(ptyClient, 'emberConsolidateDreams').mockImplementation((request) => {
      startedRootBatches.push(request.projectRoots ?? []);
      return new Promise((resolve) => {
        resolvers.push(() => resolve({
          accountDreamsPath: '/tmp/Kota/dreams/dreams.md',
          entriesDir: '/tmp/Kota/dreams/entries',
          oldDreamsPath: '/tmp/Kota/dreams/old_dreams.md',
          promptPath: '$KOTA_HOME/heroes/system-ember/ember-dream-consolidate.md',
          processedEntryCount: 1,
          activeEntryCount: 1,
          archivedEntryCount: 0,
          updatedAt: new Date().toISOString(),
          error: null,
        }));
      });
    });

    try {
      render(
        <RightColumn
          sceneKey="conversation"
          workspace={workspace}
          workingAgents={new Set()}
          onOpenHotMem={vi.fn()}
          dreamProjects={[
            {
              projectId: 'proj',
              projectRoot: '/tmp/kota/proj',
              projectName: 'proj',
              agents: [{ id: 'alice', name: 'Alice' }],
            },
            {
              projectId: 'other',
              projectRoot: '/tmp/kota/other',
              projectName: 'other',
              agents: [{ id: 'bob', name: 'Bob' }],
            },
          ]}
        />,
      );

      const switchButton = screen.getByRole('switch', { name: 'Good Night' });
      fireEvent.click(switchButton);
      fireEvent.click(switchButton);
      fireEvent.click(screen.getByRole('button', { name: 'Skip' }));
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });

      await act(async () => {
        vi.advanceTimersByTime(120_000);
        await Promise.resolve();
      });
      expect(startedRootBatches).toEqual([['/tmp/kota/proj', '/tmp/kota/other']]);
      expect(consolidate).toHaveBeenCalledTimes(1);

      await act(async () => {
        resolvers[0]?.();
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(startedRootBatches).toEqual([['/tmp/kota/proj', '/tmp/kota/other']]);
      expect(consolidate).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows Bartender action buttons only when sync or push changes exist', async () => {
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    vi.spyOn(ptyClient, 'bartenderStatus').mockResolvedValue({
      projectId: 'proj',
      sourceDir: workspace.sourceDir,
      defaultBranch: 'main',
      roomChangeCount: 3,
      sourceChangeCount: 0,
      githubChangeCount: 2,
      githubBehindCount: 0,
      githubNeedsInitialPush: false,
      githubPushBranch: null,
      githubInitialPushCommitCount: 0,
      dirtyAgents: [],
      checkedAt: new Date().toISOString(),
      state: 'roomDiff',
      message: '3 changed files waiting in agent worktrees.',
    });

    const localStorage = withMockLocalStorage();
    try {
      render(
        <RightColumn
          sceneKey="conversation"
          workspace={workspace}
          workingAgents={new Set()}
          onOpenHotMem={vi.fn()}
        />,
      );

      const autoSync = await screen.findByRole('switch', { name: /auto sync/i });
      expect(autoSync).toHaveAttribute('aria-checked', 'false');
      await userEvent.click(autoSync);
      expect(autoSync).toHaveAttribute('aria-checked', 'true');
      expect(localStorage.storage.get('kota-v2.bartender.auto-sync.proj')).toBe('true');
      expect(await screen.findByText('Sync 3 changed files in room')).toBeInTheDocument();
      expect(screen.getByText('Push 2 changes to GitHub')).toBeInTheDocument();
    } finally {
      localStorage.restore();
    }
  });

  it('keeps one Bartender listener, scopes external sync by project, and reconciles missed completion', async () => {
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    const otherWorkspace = {
      ...workspace,
      projectId: 'other',
      repoFullName: 'mock/other',
      localRoot: '/tmp/kota/other',
      sourceDir: '/tmp/kota/other/source',
      sharedDir: '/tmp/kota/other/project-memory',
      rulesDir: '/tmp/kota/other/rules',
    } satisfies ptyClient.WorkspaceProject;
    const status = {
      projectId: 'proj',
      sourceDir: workspace.sourceDir,
      defaultBranch: 'main',
      roomChangeCount: 1,
      sourceChangeCount: 0,
      githubChangeCount: 0,
      githubBehindCount: 0,
      githubNeedsInitialPush: false,
      githubPushBranch: null,
      githubInitialPushCommitCount: 0,
      dirtyAgents: [],
      checkedAt: new Date().toISOString(),
      state: 'roomDiff',
      message: '1 changed file waiting in agent worktrees.',
    } satisfies ptyClient.BartenderStatus;
    const result = {
      ok: true,
      message: 'Synced 1 change.',
      snapshotCount: 0,
      publishedCommitCount: 1,
      publishedAgents: [{ agentId: 'alice', commitCount: 1 }],
      conflicts: [],
      status: { ...status, roomChangeCount: 0, state: 'githubDiff' },
    } satisfies ptyClient.BartenderSyncResult;
    let syncEventHandler: (payload: ptyClient.BartenderSyncEvent) => void = () => {};
    let progressHandler: (payload: ptyClient.BartenderSyncProgressEvent) => void = () => {};
    const onSyncEvent = vi.spyOn(ptyClient, 'onBartenderSyncEvent').mockImplementation(async (callback) => {
      syncEventHandler = callback;
      return async () => {};
    });
    const onProgress = vi.spyOn(ptyClient, 'onBartenderSyncProgressEvent').mockImplementation(async (callback) => {
      progressHandler = callback;
      return async () => {};
    });
    vi.spyOn(ptyClient, 'bartenderStatus').mockResolvedValue(status);
    let resolveReceipt: (receipt: ptyClient.BartenderSyncReceipt) => void = () => {};
    vi.spyOn(ptyClient, 'bartenderSyncReceipt').mockImplementation(() => new Promise((resolve) => {
      resolveReceipt = resolve;
    }));

    const { rerender } = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set(['alice' as AgentId])}
        onOpenHotMem={vi.fn()}
      />,
    );
    await waitFor(() => {
      expect(onSyncEvent).toHaveBeenCalledTimes(1);
      expect(onProgress).toHaveBeenCalledTimes(1);
    });

    act(() => {
      syncEventHandler({
        projectRoot: workspace.localRoot,
        requestId: 'sync-123',
        phase: 'started',
      });
      progressHandler({
        projectRoot: workspace.localRoot,
        phase: 'finished',
        message: 'Synced 1 change.',
        elapsedMs: 1343,
      });
    });
    expect(await screen.findByText('Syncing · Done')).toBeInTheDocument();

    rerender(
      <RightColumn
        sceneKey="conversation"
        workspace={otherWorkspace}
        workingAgents={new Set()}
        onOpenHotMem={vi.fn()}
      />,
    );
    await waitFor(() => expect(screen.queryByText('Syncing · Done')).not.toBeInTheDocument());
    expect(onSyncEvent).toHaveBeenCalledTimes(1);
    expect(onProgress).toHaveBeenCalledTimes(1);

    rerender(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set(['alice' as AgentId])}
        onOpenHotMem={vi.fn()}
      />,
    );
    expect(await screen.findByText('Syncing · Done')).toBeInTheDocument();

    await act(async () => {
      resolveReceipt({
        projectRoot: workspace.localRoot,
        requestId: 'sync-123',
        phase: 'finished',
        result,
      });
      await Promise.resolve();
    });
    await waitFor(() => expect(screen.queryByText('Syncing · Done')).not.toBeInTheDocument());
    expect(onSyncEvent).toHaveBeenCalledTimes(1);
    expect(onProgress).toHaveBeenCalledTimes(1);
  });

  it('releases an external Bartender sync whose durable receipt never arrives', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-07-09T20:00:00Z'));
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    const status = {
      projectId: 'proj',
      sourceDir: workspace.sourceDir,
      defaultBranch: 'main',
      roomChangeCount: 1,
      sourceChangeCount: 0,
      githubChangeCount: 0,
      githubBehindCount: 0,
      githubNeedsInitialPush: false,
      githubPushBranch: null,
      githubInitialPushCommitCount: 0,
      dirtyAgents: [],
      checkedAt: new Date().toISOString(),
      state: 'roomDiff',
      message: '1 changed file waiting in agent worktrees.',
    } satisfies ptyClient.BartenderStatus;
    let syncEventHandler: (payload: ptyClient.BartenderSyncEvent) => void = () => {};
    vi.spyOn(ptyClient, 'onBartenderSyncEvent').mockImplementation(async (callback) => {
      syncEventHandler = callback;
      return async () => {};
    });
    vi.spyOn(ptyClient, 'onBartenderSyncProgressEvent').mockResolvedValue(async () => {});
    vi.spyOn(ptyClient, 'bartenderStatus').mockResolvedValue(status);
    vi.spyOn(ptyClient, 'bartenderSyncReceipt').mockResolvedValue({
      projectRoot: workspace.localRoot,
      requestId: 'sync-ghost',
      phase: 'pending',
    });

    const view = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        onOpenHotMem={vi.fn()}
      />,
    );
    try {
      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      act(() => {
        syncEventHandler({
          projectRoot: workspace.localRoot,
          requestId: 'sync-ghost',
          phase: 'started',
        });
      });
      expect(screen.getByText('Syncing · Starting')).toBeInTheDocument();

      await act(async () => {
        await Promise.resolve();
        await Promise.resolve();
      });
      vi.setSystemTime(new Date('2026-07-09T20:10:01Z'));
      await act(async () => {
        vi.advanceTimersByTime(1000);
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(screen.queryByText(/Syncing/)).not.toBeInTheDocument();
      expect(screen.getByText('Bartender sync status is unknown. Stopped waiting.')).toBeInTheDocument();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('blocks sync and push while a worktree conflict is assigned', async () => {
    const workspace = {
      projectId: 'proj',
      repoFullName: 'mock/proj',
      remoteUrl: 'https://github.com/mock/proj.git',
      githubHtmlUrl: 'https://github.com/mock/proj',
      defaultBranch: 'main',
      baseRef: 'origin/main',
      localRoot: '/tmp/kota/proj',
      localRootBytes: 0,
      sourceDir: '/tmp/kota/proj/source',
      sourceDirBytes: 0,
      sharedDir: '/tmp/kota/proj/project-memory',
      rulesDir: '/tmp/kota/proj/rules',
      agents: [],
    } satisfies ptyClient.WorkspaceProject;
    const status = {
      projectId: 'proj',
      sourceDir: workspace.sourceDir,
      defaultBranch: 'main',
      roomChangeCount: 3,
      sourceChangeCount: 0,
      githubChangeCount: 2,
      githubBehindCount: 0,
      githubNeedsInitialPush: false,
      githubPushBranch: null,
      githubInitialPushCommitCount: 0,
      dirtyAgents: [],
      checkedAt: new Date().toISOString(),
      state: 'roomDiff',
      message: '3 changed files waiting in agent worktrees.',
    } satisfies ptyClient.BartenderStatus;
    vi.spyOn(ptyClient, 'bartenderStatus').mockResolvedValue(status);
    vi.spyOn(ptyClient, 'bartenderSyncLocal').mockResolvedValue({
      ok: false,
      message: 'Asked Alice to resolve a conflict.',
      snapshotCount: 0,
      publishedCommitCount: 0,
      publishedAgents: [],
      conflicts: [{ agentId: 'alice', commit: 'abc123', message: 'conflict' }],
      status,
    });
    const onOpenAgentFilteredChat = vi.fn();
    const agentMeta = {
      alice: {
        name: 'Alice',
        emoji: '◇',
        role: 'Project agent',
        hue: 'var(--brass-bright)',
      },
    };

    const { rerender } = render(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        agentMeta={agentMeta}
        onOpenHotMem={vi.fn()}
        onOpenAgentFilteredChat={onOpenAgentFilteredChat}
      />,
    );

    await userEvent.click(await screen.findByText('Sync 3 changed files in room'));
    const blocker = await screen.findByRole('button', { name: 'Alice resolving' });
    expect(blocker).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Unlock' })).toBeInTheDocument();
    await waitFor(() => {
      expect(screen.queryByText('Sync 3 changed files in room')).not.toBeInTheDocument();
      expect(screen.queryByText('Push 2 changes to GitHub')).not.toBeInTheDocument();
    });

    await userEvent.click(blocker);
    expect(onOpenAgentFilteredChat).toHaveBeenCalledWith('alice');

    rerender(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set(['alice'])}
        agentMeta={agentMeta}
        onOpenHotMem={vi.fn()}
        onOpenAgentFilteredChat={onOpenAgentFilteredChat}
      />,
    );
    expect(screen.getByRole('button', { name: 'Alice resolving' })).toBeInTheDocument();

    rerender(
      <RightColumn
        sceneKey="conversation"
        workspace={workspace}
        workingAgents={new Set()}
        agentMeta={agentMeta}
        onOpenHotMem={vi.fn()}
        onOpenAgentFilteredChat={onOpenAgentFilteredChat}
      />,
    );
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: 'Alice resolving' })).not.toBeInTheDocument();
    });
    expect(await screen.findByText('Sync 3 changed files in room')).toBeInTheDocument();
    expect(screen.getByText('Push 2 changes to GitHub')).toBeInTheDocument();
  });
});

// ═══════════════════════════ W3+W5 · composer target + broadcast ═══════
//  Live PTYs still render as floating windows, while the bottom composer
//  stays mounted for multi-line prompt editing and one-shot injection.
describe('W3+W5 · composer target picker', () => {
  it.each(violetMessageOrderCases)('$name', ({ messages, expectedIds }) => {
    const merged = mergeSyncedNativeMessages(
      [],
      messages.map((message) => ({
        ...message,
        sessionId: 'shared-order-fixture',
        agentId: 'alice',
        shell: 'codex',
        role: 'assistant',
        kind: 'message',
        text: `text for ${message.id}`,
        sourcePath: null,
        nativeEventId: message.id,
      })),
      { preserveLoadedHistory: true },
    );

    expect(merged.map((message) => message.id)).toEqual(expectedIds);
  });

  it('default state: composer is mounted, broadcast is off, no windows', () => {
    render(<App />);
    expect(screen.getByTestId('ib-target-pill')).toBeInTheDocument();
    expect(screen.getByTestId('input-field')).toBeInTheDocument();
    expect(screen.queryByTestId(/^agent-window-/)).not.toBeInTheDocument();
    expect(screen.queryByTestId('broadcast-overlay')).not.toBeInTheDocument();
    expect(within(screen.getByTestId('ib-target-pill')).getByText('None')).toBeInTheDocument();
  });

  it('selecting broadcast targets switches the composer into broadcast mode', async () => {
    render(<App />);
    await enableBroadcastMode();
    expect(screen.queryByTestId('broadcast-overlay')).not.toBeInTheDocument();
    expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).toHaveClass('broadcast');
    expect(within(screen.getByTestId('ib-target-pill')).getByText('2 Agents')).toBeInTheDocument();
  });

  it('broadcast target pill reopens the target picker', async () => {
    render(<App />);
    await enableBroadcastMode();
    await userEvent.click(screen.getByTestId('ib-target-pill'));
    expect(await screen.findByTestId('broadcast-target-popover')).toBeInTheDocument();
  });

  it('typing a multi-line prompt keeps browser-native composer editing', async () => {
    render(<App />);
    await userEvent.type(screen.getByTestId('input-field'), 'hello{enter}everyone');
    expect(composerText()).toContain('hello');
    expect(composerText()).toContain('everyone');
    expect(within(screen.getByTestId('ib-target-pill')).getByText('None')).toBeInTheDocument();
  });

  it('the group chat trigger opens and closes the group chat overlay', async () => {
    render(<App />);
    expect(screen.queryByTestId('group-chat-overlay')).not.toBeInTheDocument();
    expect(screen.getByTestId('input-field')).toBeInTheDocument();
    await userEvent.click(screen.getByTestId('group-chat-trigger'));
    expect(screen.getByTestId('group-chat-overlay')).toBeInTheDocument();
    expect(screen.getByTestId('input-field')).toBeInTheDocument();
    await userEvent.click(screen.getByTestId('group-chat-trigger'));
    expect(screen.queryByTestId('group-chat-overlay')).not.toBeInTheDocument();
  });

  it('opens the project rules medal from the whistle button', async () => {
    render(<App />);
    expect(screen.getByTestId('project-rules-trigger')).toBeInTheDocument();
    expect(screen.getByTestId('group-chat-trigger')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('project-rules-trigger'));
    const medal = await screen.findByTestId('project-rules-medal');
    expect(within(medal).getByText(/Rules for all agents in/i)).toBeInTheDocument();
    expect(within(medal).getByText('project-context.md')).toBeInTheDocument();

    await userEvent.click(within(medal).getByText('New'));
    expect(await within(medal).findByDisplayValue('New Project Rule')).toBeInTheDocument();
    const trigger = within(medal).getByLabelText('On-demand trigger');
    expect(trigger.tagName).toBe('TEXTAREA');
    expect(trigger).toHaveAttribute('rows', '2');
    expect(trigger).toHaveAttribute('wrap', 'soft');
    expect(within(medal).getByTestId('project-rule-editor-scroll')).toBeInTheDocument();
    expect(within(medal).getByTestId('project-rule-editor-footer')).toBeInTheDocument();
  });

  it('Ctrl+9 toggles the group chat overlay', async () => {
    render(<App />);
    expect(screen.queryByTestId('group-chat-overlay')).not.toBeInTheDocument();
    fireEvent.keyDown(window, { key: '9', ctrlKey: true });
    expect(screen.getByTestId('group-chat-overlay')).toBeInTheDocument();
    fireEvent.keyDown(window, { key: '9', ctrlKey: true });
    expect(screen.queryByTestId('group-chat-overlay')).not.toBeInTheDocument();
  });

  it('Ctrl+0 minimizes terminals without closing the group chat overlay', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    await userEvent.click(screen.getByTestId('group-chat-trigger'));
    expect(screen.getByTestId('group-chat-overlay')).toBeInTheDocument();
    fireEvent.keyDown(window, { key: '0', ctrlKey: true });
    await waitFor(() => expect(screen.queryByTestId(`win-frame-${alice}`)).not.toBeInTheDocument());
    expect(screen.getByTestId('group-chat-overlay')).toBeInTheDocument();
  });

  it('replays composer sends when the group chat mounts after the prompt was sent', async () => {
    emitVioletComposerSent({
      projectRoot: '/tmp/kota-test',
      text: 'hello minimized terminal',
      targetAgentIds: ['alice'],
      privacy: false,
    });

    render(
      <VioletRoomPanel
        projectRoot="/tmp/kota-test"
        agentIds={['alice']}
      />,
    );

    expect(await screen.findByText('hello minimized terminal')).toBeInTheDocument();
    expect(screen.getByText('@alice')).toBeInTheDocument();
    expect(document.querySelector('.violet-room-footer')).not.toBeInTheDocument();
  });

  it.each([
    {
      label: 'a local image link',
      markdown: '[local mock](/tmp/kota-image-preview.png)',
      path: '/tmp/kota-image-preview.png',
    },
    {
      label: 'an angle-wrapped Markdown image',
      markdown: '![local mock](</tmp/kota image preview.png>)',
      path: '/tmp/kota image preview.png',
    },
  ])('previews and opens $label', async ({ markdown, path }) => {
    const resolveSpy = vi.spyOn(ptyClient, 'violetResolveFileRef').mockResolvedValue({
      path,
      isDir: false,
    });
    vi.spyOn(ptyClient, 'fileImageDataUrl').mockResolvedValue('data:image/png;base64,aW1hZ2U=');
    const openSpy = vi.spyOn(ptyClient, 'violetOpenFileRef').mockResolvedValue();

    const { container } = render(
      <MarkdownText
        text={markdown}
        projectRoot="/tmp/kota-test"
        enableLocalFileRefs
      />,
    );

    const preview = await screen.findByRole('button', { name: 'local mock' });
    expect(preview).toHaveAttribute('src', 'data:image/png;base64,aW1hZ2U=');
    expect(container).not.toHaveTextContent(markdown);
    expect(container).not.toHaveTextContent('!');
    expect(resolveSpy).toHaveBeenCalledWith({
      projectRoot: '/tmp/kota-test',
      path,
    });

    await userEvent.click(preview);
    expect(openSpy).toHaveBeenCalledWith({
      projectRoot: '/tmp/kota-test',
      path,
    });
  });

  it('leaves standalone and escaped exclamation marks as text', () => {
    const resolveSpy = vi.spyOn(ptyClient, 'violetResolveFileRef');
    const { container } = render(
      <MarkdownText
        text={'Standalone! \\![literal](</tmp/not-an-image.png>)'}
        projectRoot="/tmp/kota-test"
        enableLocalFileRefs
      />,
    );

    expect(container).toHaveTextContent('Standalone! ![literal](</tmp/not-an-image.png>)');
    expect(container.querySelector('img')).not.toBeInTheDocument();
    expect(resolveSpy).not.toHaveBeenCalled();
  });

  it('keeps remote Markdown images as links without fetching them', () => {
    const resolveSpy = vi.spyOn(ptyClient, 'violetResolveFileRef');
    const imageSpy = vi.spyOn(ptyClient, 'fileImageDataUrl');
    render(
      <MarkdownText
        text="![remote mock](https://example.com/mock.png)"
        projectRoot="/tmp/kota-test"
        enableLocalFileRefs
      />,
    );

    expect(screen.getByRole('link', { name: 'remote mock' })).toHaveAttribute(
      'href',
      'https://example.com/mock.png',
    );
    expect(resolveSpy).not.toHaveBeenCalled();
    expect(imageSpy).not.toHaveBeenCalled();
  });

  it('shows Ember human reminders with the account name and no internal target id', async () => {
    const projectRoot = '/tmp/kota-ember-human-room';
    vi.spyOn(ptyClient, 'loadAccountUserIdentity').mockResolvedValue({
      name: 'Human Tester',
      avatarId: 'user-default',
    });
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [{
        id: 'ember-human-reminder',
        sessionId: 'actor-ember',
        agentId: 'ember',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-22T10:00:00.000Z',
        text: 'Take a break.',
        sourcePath: null,
        nativeEventId: 'ember-reminder-human-one',
        targetAgentIds: [HUMAN_TELEGRAM_TARGET_ID],
        agentDisplayName: 'Ember',
        agentAvatarId: 'ember',
        agentProvider: 'system',
        agentStatus: null,
      }],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-07-22T10:00:01.000Z',
    });

    const { container } = render(
      <VioletRoomPanel projectRoot={projectRoot} agentIds={['agent-one']} />,
    );

    const bubble = (await screen.findByText('Take a break.')).closest('.violet-msg');
    expect(bubble).not.toBeNull();
    expect(within(bubble as HTMLElement).getByText('@Human Tester')).toHaveClass('human');
    expect(bubble).not.toHaveClass('delivery-issue');
    expect(bubble).not.toHaveTextContent('Ember scheduled prompt');
    expect(container).not.toHaveTextContent(HUMAN_TELEGRAM_TARGET_ID);
  });

  it('shows Quote only on the room message allowlist and reports the fifth-quote limit', async () => {
    const projectRoot = '/tmp/kota-room-quote-allowlist';
    const messages: ptyClient.VioletChatMessage[] = [
      {
        id: 'agent-final',
        sessionId: 'agent-alice',
        agentId: 'alice',
        shell: 'codex',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:00.000Z',
        text: 'Allowed final answer',
      },
      {
        id: 'agent-progress',
        sessionId: 'agent-alice',
        agentId: 'alice',
        shell: 'codex',
        role: 'assistant',
        kind: 'commentary',
        timestamp: '2026-07-30T10:00:01.000Z',
        text: 'Hidden progress update',
      },
      {
        id: 'agent-bus',
        sessionId: 'actor-alice',
        agentId: 'alice',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:02.000Z',
        text: 'Allowed handoff',
        nativeEventId: 'agentbus-quote-one',
        actorIntent: 'handoff',
        targetAgentIds: ['bob'],
      },
      {
        id: 'laughing-man-message',
        sessionId: 'actor-laughing-man',
        agentId: 'laughing-man',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:03.000Z',
        text: 'Allowed remote user message',
        nativeEventId: 'lm-update-quote-one',
        actorIntent: 'telegram',
        targetAgentIds: ['alice'],
      },
      {
        id: 'ember-dream',
        sessionId: 'actor-ember',
        agentId: 'ember',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:04.000Z',
        text: "It's time to dream.\n\nInternal dream prompt",
        nativeEventId: 'ember-dream-one',
        actorIntent: 'dream',
      },
      {
        id: 'ember-reminder',
        sessionId: 'actor-ember',
        agentId: 'ember',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:04.500Z',
        text: 'Allowed scheduled reminder',
        nativeEventId: 'ember-reminder-quote-one',
        actorIntent: 'reminder',
        targetAgentIds: ['alice'],
      },
      {
        id: 'bartender-conflict',
        sessionId: 'actor-bartender',
        agentId: 'bartender',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:05.000Z',
        text: 'Git said: resolve this conflict',
        nativeEventId: 'bartender-conflict:alice:one:two',
        actorIntent: 'conflict',
        targetAgentIds: ['alice'],
      },
      {
        id: 'bbs-prompt',
        sessionId: 'actor-bbs',
        agentId: 'bbs',
        shell: 'system',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:00:05.250Z',
        text: 'Check this BBS reminder',
        nativeEventId: 'bbs-thread-quote-one',
        actorIntent: 'bbs-thread',
        targetAgentIds: ['alice'],
      },
      {
        id: 'native-system-envelope',
        sessionId: 'agent-alice',
        agentId: 'alice',
        shell: 'codex',
        role: 'user',
        kind: 'message',
        timestamp: '2026-07-30T10:00:05.500Z',
        text: '<KOTA_MESSAGE id="ember-dream-one" from="ember" to="alice" intent="dream">\ninternal dream echo\n</KOTA_MESSAGE>',
      },
      {
        id: 'artifact-message',
        sessionId: 'agent-alice',
        agentId: 'alice',
        shell: 'codex',
        role: 'assistant',
        kind: 'artifact',
        timestamp: '2026-07-30T10:00:06.000Z',
        text: 'Generated artifact project-memory/artifacts/example/output.png',
      },
    ];
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages,
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-07-30T10:00:07.000Z',
    });
    const onQuoteMessage = vi.fn(() => 'limit' as const);

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice', 'bob']}
        onQuoteMessage={onQuoteMessage}
      />,
    );

    const finalBubble = (await screen.findByText('Allowed final answer')).closest('.violet-msg') as HTMLElement;
    const handoffBubble = screen.getByText('Allowed handoff').closest('.violet-msg') as HTMLElement;
    const remoteBubble = screen.getByText('Allowed remote user message').closest('.violet-msg') as HTMLElement;
    const reminderBubble = screen.getByText('Allowed scheduled reminder').closest('.violet-msg') as HTMLElement;
    const artifactBubble = screen.getByText(/Generated artifact/).closest('.violet-msg') as HTMLElement;
    expect(within(finalBubble).getByTitle('Quote message')).toBeInTheDocument();
    expect(within(handoffBubble).getByTitle('Quote message')).toBeInTheDocument();
    expect(within(remoteBubble).getByTitle('Quote message')).toBeInTheDocument();
    expect(within(reminderBubble).getByTitle('Quote message')).toBeInTheDocument();
    expect(within(artifactBubble).getByTitle('Quote message')).toBeInTheDocument();

    const progressBubble = screen.getAllByText('Hidden progress update')[0]!
      .closest('.violet-msg') as HTMLElement;
    const dreamBubble = screen.getByText("It's time to dream.").closest('.violet-msg') as HTMLElement;
    const conflictBubble = screen.getByText(/resolving worktree conflict/).closest('.violet-msg') as HTMLElement;
    const bbsBubble = screen.getByText('Check this BBS reminder').closest('.violet-msg') as HTMLElement;
    const nativeEnvelopeBubble = screen.getByText(/internal dream echo/).closest('.violet-msg') as HTMLElement;
    expect(within(progressBubble).queryByTitle('Quote message')).not.toBeInTheDocument();
    expect(within(dreamBubble).queryByTitle('Quote message')).not.toBeInTheDocument();
    expect(within(conflictBubble).queryByTitle('Quote message')).not.toBeInTheDocument();
    expect(within(bbsBubble).queryByTitle('Quote message')).not.toBeInTheDocument();
    expect(within(nativeEnvelopeBubble).queryByTitle('Quote message')).not.toBeInTheDocument();

    await userEvent.click(within(finalBubble).getByTitle('Quote message'));
    expect(onQuoteMessage).toHaveBeenCalledWith(expect.objectContaining({
      ref: 'agent-final',
      project: 'kota-room-quote-allowlist',
      excerpt: 'Allowed final answer',
    }));
    expect(screen.getByRole('status')).toHaveTextContent('4 quotes max');
  });

  it('quotes a materialized local user bubble by its native event id, but not a pending bubble', async () => {
    const projectRoot = '/tmp/kota-materialized-user-quote';
    const timestamp = new Date().toISOString();
    const materialized = emitVioletComposerSent({
      projectRoot,
      text: 'Materialized user prompt',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    emitVioletComposerSent({
      projectRoot,
      text: 'Still pending user prompt',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [{
        id: 'native-user-event',
        sessionId: 'agent-alice',
        agentId: 'alice',
        shell: 'codex',
        role: 'user',
        kind: 'message',
        timestamp,
        text: 'Materialized user prompt',
      }],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: timestamp,
    });
    const onQuoteMessage = vi.fn(() => 'inserted' as const);

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        onQuoteMessage={onQuoteMessage}
      />,
    );

    expect(materialized).not.toBeNull();
    const materializedBubble = (await screen.findByText('Materialized user prompt')).closest('.violet-msg') as HTMLElement;
    const pendingBubble = screen.getByText('Still pending user prompt').closest('.violet-msg') as HTMLElement;
    expect(screen.getAllByText('Materialized user prompt')).toHaveLength(1);
    expect(within(materializedBubble).getByTitle('Quote message')).toBeInTheDocument();
    expect(within(pendingBubble).queryByTitle('Quote message')).not.toBeInTheDocument();

    await userEvent.click(within(materializedBubble).getByTitle('Quote message'));
    expect(onQuoteMessage).toHaveBeenCalledWith(expect.objectContaining({
      ref: 'native-user-event',
      from: { id: 'user', name: 'You' },
    }));
  });

  it('renders quote wrappers as compact cards while keeping the new message body separate', async () => {
    const projectRoot = '/tmp/kota-room-quote-render';
    const text = serializeRoomQuotePrompt([{
      ref: 'quoted-native-event',
      project: 'kota-room-quote-render',
      from: { id: 'alice', name: 'Alice' },
      to: [{ id: 'user', name: 'User' }],
      at: '2026-07-30T10:00:00.000Z',
      excerpt: 'This quoted excerpt is intentionally long enough to demonstrate the compact card.',
      truncated: false,
    }], 'This is the new message body.');
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [{
        id: 'message-with-quote',
        sessionId: 'agent-alice',
        agentId: 'alice',
        shell: 'codex',
        role: 'assistant',
        kind: 'message',
        timestamp: '2026-07-30T10:01:00.000Z',
        text,
      }],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-07-30T10:01:01.000Z',
    });

    const { container } = render(
      <VioletRoomPanel projectRoot={projectRoot} agentIds={['alice']} />,
    );
    expect(await screen.findByTestId('violet-room-quote-cards')).toHaveTextContent('Alice → User');
    expect(screen.getByText('This is the new message body.')).toBeInTheDocument();
    expect(container).not.toHaveTextContent('KOTA_QUOTE_META');
    expect(container).not.toHaveTextContent('KOTA_QUOTE_REF');
  });

  it('shows delivery retry only for user messages, not Agent Bus messages', async () => {
    const projectRoot = '/tmp/kota-retry-visibility-test';
    const queuedAgentBusMessage: ptyClient.VioletChatMessage & {
      deliveryStatus: 'unconfirmed';
      deliveryReason: string;
    } = {
      id: 'queued-agent-bus-message',
      sessionId: 'agent-bus',
      agentId: 'alice',
      shell: 'system',
      role: 'assistant',
      kind: 'message',
      timestamp: '2026-05-20T10:00:00.000Z',
      text: 'queued Agent Bus handoff',
      sourcePath: null,
      nativeEventId: 'agentbus-retry-visibility-test',
      targetAgentIds: ['bob'],
      deliveryStatus: 'unconfirmed',
      deliveryReason: 'A2A message was not confirmed in the provider log.',
    };
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [queuedAgentBusMessage],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:01:31.000Z',
    });
    const onRetryComposerMessage = vi.fn();

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice', 'bob']}
        onRetryComposerMessage={onRetryComposerMessage}
      />,
    );

    const agentBusBubble = (await screen.findByText('queued Agent Bus handoff')).closest('.violet-msg');
    expect(agentBusBubble).not.toBeNull();
    expect(agentBusBubble).not.toHaveClass('delivery-issue');
    expect(within(agentBusBubble as HTMLElement).queryByRole('button', {
      name: 'Queued message — try sending now',
    })).not.toBeInTheDocument();

    let sentMessage: ReturnType<typeof emitVioletComposerSent> = null;
    act(() => {
      sentMessage = emitVioletComposerSent({
        projectRoot,
        text: 'retry this composer prompt',
        targetAgentIds: ['alice'],
        privacy: false,
      });
    });
    expect(sentMessage).not.toBeNull();
    expect(await screen.findByText('retry this composer prompt')).toBeInTheDocument();

    act(() => {
      emitVioletComposerDelivery({
        id: sentMessage!.id,
        status: 'failed',
        reason: 'Prompt was not delivered.',
        retryTargetAgentIds: ['alice'],
      });
    });

    const userBubble = screen.getByText('retry this composer prompt').closest('.violet-msg');
    expect(userBubble).not.toBeNull();
    expect(userBubble).toHaveClass('delivery-issue');
    const retryButton = within(userBubble as HTMLElement).getByRole('button', {
      name: 'Queued message — try sending now',
    });
    await userEvent.click(retryButton);
    expect(onRetryComposerMessage).toHaveBeenCalledWith({
      text: 'retry this composer prompt',
      targetAgentIds: ['alice'],
      privacy: false,
      mentions: undefined,
    });
    await waitFor(() => expect(within(userBubble as HTMLElement).queryByRole('button', {
      name: 'Queued message — try sending now',
    })).not.toBeInTheDocument());
    expect(violetComposerSentHistory(projectRoot).find((item) => item.id === sentMessage!.id)?.delivery?.status)
      .toBe('clear');
  });

  it.each([
    { outcome: 'clear' as const, reject: false },
    { outcome: 'failed' as const, reject: true },
  ])('settles a $outcome retry after switching away from its project', async ({ outcome, reject }) => {
    const projectRoot = `/tmp/kota-retry-project-switch-${outcome}`;
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-07-18T12:00:00.000Z',
    });
    let resolveRetry!: (value: boolean) => void;
    let rejectRetry!: (reason: Error) => void;
    const retryResult = new Promise<boolean>((resolve, rejectPromise) => {
      resolveRetry = resolve;
      rejectRetry = rejectPromise;
    });
    const onRetryComposerMessage = vi.fn(() => retryResult);
    const sentMessage = emitVioletComposerSent({
      projectRoot,
      text: `retry while switching projects (${outcome})`,
      targetAgentIds: ['alice'],
      privacy: false,
    });
    expect(sentMessage).not.toBeNull();
    emitVioletComposerDelivery({
      id: sentMessage!.id,
      status: 'failed',
      reason: 'Prompt was not delivered.',
      retryTargetAgentIds: ['alice'],
    });

    const view = render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        onRetryComposerMessage={onRetryComposerMessage}
      />,
    );
    const bubble = (await screen.findByText(`retry while switching projects (${outcome})`))
      .closest('.violet-msg');
    await userEvent.click(within(bubble as HTMLElement).getByRole('button', {
      name: 'Queued message — try sending now',
    }));
    await waitFor(() => expect(violetComposerSentHistory(projectRoot)[0]?.delivery?.status)
      .toBe('retrying'));

    view.rerender(
      <VioletRoomPanel
        projectRoot="/tmp/kota-other-project"
        agentIds={['bob']}
        onRetryComposerMessage={onRetryComposerMessage}
      />,
    );
    await act(async () => {
      if (reject) rejectRetry(new Error('retry rejected'));
      else resolveRetry(true);
      await Promise.resolve();
    });
    await waitFor(() => expect(violetComposerSentHistory(projectRoot)[0]?.delivery?.status)
      .toBe(outcome));

    view.rerender(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        onRetryComposerMessage={onRetryComposerMessage}
      />,
    );
    const restoredBubble = (await screen.findByText(`retry while switching projects (${outcome})`))
      .closest('.violet-msg');
    const retryButton = within(restoredBubble as HTMLElement).queryByRole('button', {
      name: 'Queued message — try sending now',
    });
    if (outcome === 'failed') expect(retryButton).toBeInTheDocument();
    else expect(retryButton).not.toBeInTheDocument();
  });

  it('uses one deadline, not polling, before offering to send an unconfirmed Composer message now', async () => {
    vi.useFakeTimers();
    try {
      const projectRoot = '/tmp/kota-queued-message-timeout-test';
      vi.setSystemTime(new Date('2026-07-18T13:00:00.000Z'));
      vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
        messages: [],
        sources: [],
        workEvents: [],
        agentBusReceipts: [],
        rawLogDir: `${projectRoot}/project-memory/raw_logs`,
        chathistoryDir: `${projectRoot}/project-memory/chathistory`,
        syncedAt: '2026-07-18T13:00:00.000Z',
      });
      emitVioletComposerSent({
        projectRoot,
        text: 'wait for the provider queue',
        targetAgentIds: ['alice'],
        privacy: false,
      });

      render(
        <VioletRoomPanel
          projectRoot={projectRoot}
          agentIds={['alice']}
          onRetryComposerMessage={vi.fn()}
        />,
      );
      await act(async () => {});

      const messageBubble = screen.getByText('wait for the provider queue').closest('.violet-msg');
      expect(messageBubble).not.toBeNull();
      expect(within(messageBubble as HTMLElement).queryByRole('button', {
        name: 'Queued message — try sending now',
      })).not.toBeInTheDocument();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(179_000);
      });
      expect(within(messageBubble as HTMLElement).queryByRole('button', {
        name: 'Queued message — try sending now',
      })).not.toBeInTheDocument();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1_000);
      });
      expect(within(messageBubble as HTMLElement).getByRole('button', {
        name: 'Queued message — try sending now',
      })).toHaveAttribute('title', 'Queued — try sending now');
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps an explicitly queued Composer prompt until native user evidence arrives', async () => {
    const projectRoot = '/tmp/kota-exit-queued-native-evidence-test';
    const sentMessage = emitVioletComposerSent({
      projectRoot,
      text: 'agent exited before accepting this',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    expect(sentMessage).not.toBeNull();
    emitVioletComposerDelivery({
      id: sentMessage!.id,
      status: 'unconfirmed',
      reason: VIOLET_COMPOSER_AGENT_EXIT_REASON,
      retryTargetAgentIds: ['alice'],
    });
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [{
        id: 'assistant-activity-without-user-evidence',
        sessionId: 'session-a',
        agentId: 'alice',
        shell: 'codex',
        role: 'assistant',
        kind: 'message',
        timestamp: new Date(Date.parse(sentMessage!.timestamp) + 1_000).toISOString(),
        text: 'Unrelated prior activity.',
        sourcePath: null,
        nativeEventId: null,
      }],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: new Date(Date.parse(sentMessage!.timestamp) + 2_000).toISOString(),
    });

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        onRetryComposerMessage={vi.fn()}
      />,
    );

    const messageBubble = (await screen.findByText('agent exited before accepting this'))
      .closest('.violet-msg');
    expect(within(messageBubble as HTMLElement).getByRole('button', {
      name: 'Queued message — try sending now',
    })).toBeInTheDocument();

    act(() => {
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot, agentIds: ['alice'] },
          state: {
            messages: [{
              id: 'native-user-evidence-after-exit',
              sessionId: 'session-a',
              agentId: 'alice',
              shell: 'codex',
              role: 'user',
              kind: 'message',
              timestamp: new Date(Date.parse(sentMessage!.timestamp) + 3_000).toISOString(),
              text: 'agent exited before accepting this',
              sourcePath: null,
              nativeEventId: null,
            }],
            sources: [],
            workEvents: [],
            agentBusReceipts: [],
            rawLogDir: `${projectRoot}/project-memory/raw_logs`,
            chathistoryDir: `${projectRoot}/project-memory/chathistory`,
            syncedAt: new Date(Date.parse(sentMessage!.timestamp) + 4_000).toISOString(),
          },
        },
      }));
    });
    await waitFor(() => expect(within(messageBubble as HTMLElement).queryByRole('button', {
      name: 'Queued message — try sending now',
    })).not.toBeInTheDocument());
  });

  it('retires a structurally confirmed Composer message across room remounts', async () => {
    const projectRoot = '/tmp/kota-composer-delivery-remount-test';
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [],
      sources: [],
      workEvents: [],
      agentBusReceipts: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-07-18T14:00:00.000Z',
    });
    const sentMessage = emitVioletComposerSent({
      projectRoot,
      text: 'structurally confirmed prompt',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    expect(sentMessage).not.toBeNull();
    emitVioletComposerDelivery({ id: sentMessage!.id, status: 'clear' });

    const first = render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        onRetryComposerMessage={vi.fn()}
      />,
    );
    expect(await screen.findByText('structurally confirmed prompt')).toBeInTheDocument();
    first.unmount();

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        onRetryComposerMessage={vi.fn()}
      />,
    );
    const remountedBubble = (await screen.findByText('structurally confirmed prompt'))
      .closest('.violet-msg');
    expect(remountedBubble).not.toBeNull();
    expect(within(remountedBubble as HTMLElement).queryByRole('button', {
      name: 'Queued message — try sending now',
    })).not.toBeInTheDocument();
  });

  it('cancels the Composer deadline when native confirmation arrives at 179 seconds', async () => {
    vi.useFakeTimers();
    try {
      const projectRoot = '/tmp/kota-composer-native-confirm-test';
      vi.setSystemTime(new Date('2026-07-18T15:00:00.000Z'));
      vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
        messages: [],
        sources: [],
        workEvents: [],
        agentBusReceipts: [],
        rawLogDir: `${projectRoot}/project-memory/raw_logs`,
        chathistoryDir: `${projectRoot}/project-memory/chathistory`,
        syncedAt: '2026-07-18T15:00:00.000Z',
      });
      const sentMessage = emitVioletComposerSent({
        projectRoot,
        text: 'confirm just before the deadline',
        targetAgentIds: ['alice'],
        privacy: false,
      });
      expect(sentMessage).not.toBeNull();

      render(
        <VioletRoomPanel
          projectRoot={projectRoot}
          agentIds={['alice']}
          onRetryComposerMessage={vi.fn()}
        />,
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(179_000);
      });
      act(() => {
        window.dispatchEvent(new CustomEvent('violet://room/synced', {
          detail: {
            request: { projectRoot, agentIds: ['alice'] },
            state: {
              messages: [{
                id: 'native-confirm-at-179',
                sessionId: 'session-a',
                agentId: 'alice',
                shell: 'claude',
                role: 'user',
                kind: 'message',
                timestamp: '2026-07-18T15:02:59.000Z',
                text: 'confirm just before the deadline',
                sourcePath: null,
                nativeEventId: null,
              }],
              sources: [],
              workEvents: [],
              agentBusReceipts: [],
              rawLogDir: `${projectRoot}/project-memory/raw_logs`,
              chathistoryDir: `${projectRoot}/project-memory/chathistory`,
              syncedAt: '2026-07-18T15:02:59.000Z',
            },
          },
        }));
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2_000);
      });
      const bubble = document.querySelector(
        `[data-violet-message-id="${sentMessage!.id}"]`,
      ) as HTMLElement | null;
      expect(bubble).not.toBeNull();
      expect(within(bubble!).queryByRole('button', {
        name: 'Queued message — try sending now',
      })).not.toBeInTheDocument();
      expect(violetComposerSentHistory(projectRoot)[0]?.delivery?.status).toBe('clear');
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps the Violet room pinned to the bottom when synced messages append', async () => {
    render(
      <VioletRoomPanel
        projectRoot="/tmp/kota-scroll-test"
        agentIds={['alice']}
      />,
    );

    await act(async () => {});
    const scroller = document.querySelector('.violet-room-scroll') as HTMLDivElement;
    let scrollHeight = 220;
    Object.defineProperty(scroller, 'clientHeight', {
      configurable: true,
      get: () => 100,
    });
    Object.defineProperty(scroller, 'scrollHeight', {
      configurable: true,
      get: () => scrollHeight,
    });

    act(() => {
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot: '/tmp/kota-scroll-test', agentIds: ['alice'] },
          state: {
            messages: [
              {
                id: 'alice-scroll-first',
                sessionId: 's',
                agentId: 'alice',
                shell: 'claude',
                role: 'assistant',
                kind: 'message',
                timestamp: '2026-05-20T10:00:00.000Z',
                text: 'first scroll message',
                sourcePath: null,
                nativeEventId: null,
              },
            ],
            sources: [],
            workEvents: [],
            rawLogDir: '/tmp/kota-scroll-test/project-memory/raw_logs',
            chathistoryDir: '/tmp/kota-scroll-test/project-memory/chathistory',
            syncedAt: '2026-05-20T10:00:00.000Z',
          },
        },
      }));
    });

    expect(await screen.findByText('first scroll message')).toBeInTheDocument();
    scroller.scrollTop = scrollHeight;
    scrollHeight = 420;

    act(() => {
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot: '/tmp/kota-scroll-test', agentIds: ['alice'] },
          state: {
            messages: [
              {
                id: 'alice-scroll-first',
                sessionId: 's',
                agentId: 'alice',
                shell: 'claude',
                role: 'assistant',
                kind: 'message',
                timestamp: '2026-05-20T10:00:00.000Z',
                text: 'first scroll message',
                sourcePath: null,
                nativeEventId: null,
              },
              {
                id: 'alice-scroll-second',
                sessionId: 's',
                agentId: 'alice',
                shell: 'claude',
                role: 'assistant',
                kind: 'message',
                timestamp: '2026-05-20T10:00:01.000Z',
                text: 'second scroll message',
                sourcePath: null,
                nativeEventId: null,
              },
            ],
            sources: [],
            workEvents: [],
            rawLogDir: '/tmp/kota-scroll-test/project-memory/raw_logs',
            chathistoryDir: '/tmp/kota-scroll-test/project-memory/chathistory',
            syncedAt: '2026-05-20T10:00:01.000Z',
          },
        },
      }));
    });

    expect(await screen.findByText('second scroll message')).toBeInTheDocument();
    await waitFor(() => {
      expect(scroller.scrollTop).toBe(scrollHeight);
    });
  });

  it('keeps composer replay history scoped to the active project root', () => {
    emitVioletComposerSent({
      projectRoot: '/tmp/kota-test',
      text: 'kota prompt',
      targetAgentIds: ['alice'],
      privacy: false,
    });
    emitVioletComposerSent({
      projectRoot: '/tmp/kotatest1',
      text: '再来一个',
      targetAgentIds: ['agent-764ad85d1e'],
      privacy: false,
    });

    expect(violetComposerSentHistory('/tmp/kota-test').map((item) => item.text)).toEqual(['kota prompt']);
    expect(violetComposerSentHistory('/tmp/kotatest1').map((item) => item.text)).toEqual(['再来一个']);
    expect(violetComposerSentHistory(null)).toEqual([]);
  });

  it('clears local composer echoes when switching project roots', async () => {
    const { rerender } = render(
      <VioletRoomPanel
        projectRoot="/tmp/kotatest1"
        agentIds={['agent-764ad85d1e']}
      />,
    );

    act(() => {
      emitVioletComposerSent({
        projectRoot: '/tmp/kotatest1',
        text: '再来一个',
        targetAgentIds: ['agent-764ad85d1e'],
        privacy: false,
      });
    });

    expect(await screen.findByText('再来一个')).toBeInTheDocument();

    rerender(
      <VioletRoomPanel
        projectRoot="/tmp/kota-test"
        agentIds={['agent-99d1b25ab6']}
      />,
    );

    await waitFor(() => {
      expect(screen.queryByText('再来一个')).not.toBeInTheDocument();
    });
  });

  it('merges external Violet sync results into an open group chat panel', async () => {
    render(
      <VioletRoomPanel
        projectRoot="/tmp/kota-test"
        agentIds={['alice', 'bob']}
      />,
    );

    await act(async () => {});
    act(() => {
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot: '/tmp/kota-test', agentIds: ['alice'] },
          state: {
            messages: [
              {
                id: 'alice-reply',
                sessionId: 's',
                agentId: 'alice',
                shell: 'claude',
                role: 'assistant',
                kind: 'message',
                timestamp: '2026-05-20T10:00:00.000Z',
                text: 'external sync reply',
                sourcePath: null,
                nativeEventId: null,
              },
            ],
            sources: [],
            workEvents: [],
            rawLogDir: '/tmp/kota-test/project-memory/raw_logs',
            condensedDir: '/tmp/kota-test/project-memory/raw_logs_condensed',
            syncedAt: '2026-05-20T10:00:00.000Z',
          },
        },
      }));
    });

    expect(await screen.findByText('external sync reply')).toBeInTheDocument();
  });

  it('renders Codex commentary as a collapsed progress message', async () => {
    const projectRoot = '/tmp/kota-commentary-test';
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [],
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T09:59:59.000Z',
    });

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['dex']}
      />,
    );

    await act(async () => {});
    act(() => {
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot, agentIds: ['dex'] },
          state: {
            messages: [
              {
                id: 'dex-commentary-1',
                sessionId: 's',
                agentId: 'dex',
                shell: 'codex',
                role: 'assistant',
                kind: 'commentary',
                timestamp: '2026-05-20T10:00:00.000Z',
                text: 'I am checking the layout first.',
                sourcePath: null,
                nativeEventId: null,
              },
              {
                id: 'dex-commentary-2',
                sessionId: 's',
                agentId: 'dex',
                shell: 'codex',
                role: 'assistant',
                kind: 'commentary',
                timestamp: '2026-05-20T10:00:01.000Z',
                text: 'Then I will run the focused smoke test.',
                messageOrigin: 'subagent',
                sourcePath: null,
                nativeEventId: null,
              },
            ],
            sources: [],
            workEvents: [],
            rawLogDir: `${projectRoot}/project-memory/raw_logs`,
            chathistoryDir: `${projectRoot}/project-memory/chathistory`,
            syncedAt: '2026-05-20T10:00:00.000Z',
          },
        },
      }));
    });

    const details = await screen.findByText('Progress');
    expect(screen.getAllByText('Progress')).toHaveLength(1);
    const wrapper = details.closest('details') as HTMLDetailsElement;
    const article = details.closest('article') as HTMLElement;
    const summary = wrapper.querySelector('summary') as HTMLElement;
    expect(wrapper).toHaveClass('violet-commentary-details');
    expect(wrapper.open).toBe(false);
    expect(article.querySelector('.violet-msg-avatar')).not.toBeNull();
    expect(within(article).queryByText('commentary')).not.toBeInTheDocument();
    expect(within(article).getByText(/2 updates/)).toBeInTheDocument();
    const summaryOrigin = summary.querySelector('.violet-subagent-origin-summary') as HTMLElement;
    expect(summaryOrigin).toHaveAttribute(
      'aria-label',
      'Subagent update',
    );
    expect(summaryOrigin).not.toHaveAttribute('title');
    expect(within(summaryOrigin).getByText('Subagent')).toBeInTheDocument();
    fireEvent.click(summary);
    expect(wrapper.open).toBe(true);
    expect(screen.getByText('I am checking the layout first.')).toBeVisible();
    expect(screen.getByText('Then I will run the focused smoke test.')).toBeVisible();
    const entries = article.querySelectorAll('.violet-commentary-entry');
    expect(entries[0]?.querySelector('.violet-subagent-origin-entry')).toBeNull();
    const childOrigin = entries[1]?.querySelector('.violet-subagent-origin-entry') as HTMLElement;
    expect(childOrigin).not.toBeNull();
    expect(within(childOrigin).getByText('Subagent')).toBeInTheDocument();
  });

  it('keeps a hidden subagent update off the collapsed progress summary', async () => {
    const projectRoot = '/tmp/kota-subagent-progress-test';
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [
        {
          id: 'dex-child-progress',
          sessionId: 's',
          agentId: 'dex',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:00.000Z',
          text: 'A child agent found the relevant fixture.',
          messageOrigin: 'subagent',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'dex-root-progress',
          sessionId: 's',
          agentId: 'dex',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:01.000Z',
          text: 'The root agent is checking the result.',
          sourcePath: null,
          nativeEventId: null,
        },
      ],
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:00:02.000Z',
    });

    render(<VioletRoomPanel projectRoot={projectRoot} agentIds={['dex']} />);

    const progressLabel = await screen.findByText('Progress');
    const article = progressLabel.closest('article') as HTMLElement;
    const summary = article.querySelector('summary') as HTMLElement;
    expect(summary.querySelector('.violet-subagent-origin-summary')).toBeNull();

    fireEvent.click(summary);
    const entries = article.querySelectorAll('.violet-commentary-entry');
    const childOrigin = entries[0]?.querySelector('.violet-subagent-origin-entry') as HTMLElement;
    expect(childOrigin).not.toBeNull();
    expect(within(childOrigin).getByText('Subagent')).toBeInTheDocument();
    expect(entries[1]?.querySelector('.violet-subagent-origin-entry')).toBeNull();
  });

  it('groups adjacent all-chat progress runs across agents', async () => {
    const projectRoot = '/tmp/kota-multi-progress-test';
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [
        {
          id: 'alpha-progress-1',
          sessionId: 's-alpha',
          agentId: 'alpha',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:00.000Z',
          text: 'Alpha checks the room projection.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'beta-progress-1',
          sessionId: 's-beta',
          agentId: 'beta',
          shell: 'claude',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:01.000Z',
          text: 'Beta reviews the folding invariant.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'alpha-progress-2',
          sessionId: 's-alpha',
          agentId: 'alpha',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:02.000Z',
          text: 'Alpha keeps the first message id stable.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'beta-progress-2',
          sessionId: 's-beta',
          agentId: 'beta',
          shell: 'claude',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:03.000Z',
          text: 'Beta confirms the scroll key still changes.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'beta-final',
          sessionId: 's-beta',
          agentId: 'beta',
          shell: 'claude',
          role: 'assistant',
          kind: 'message',
          timestamp: '2026-05-20T10:00:04.000Z',
          text: 'Final reply stays outside progress.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'alpha-progress-3',
          sessionId: 's-alpha',
          agentId: 'alpha',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:05.000Z',
          text: 'Alpha starts a new progress run.',
          sourcePath: null,
          nativeEventId: null,
        },
      ],
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:00:06.000Z',
    });

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alpha', 'beta']}
        agentMeta={{
          alpha: { name: 'Alpha', emoji: 'A', role: 'Agent', hue: '#76a9d8', avatarClass: 'provider-codex' },
          beta: { name: 'Beta', emoji: 'B', role: 'Agent', hue: '#9a7bc4', avatarClass: 'provider-claude' },
        }}
      />,
    );

    const progressLabels = await screen.findAllByText('Progress');
    expect(progressLabels).toHaveLength(2);
    const firstArticle = progressLabels[0]!.closest('article') as HTMLElement;
    expect(firstArticle.dataset.violetMessageId).toBe('alpha-progress-1');
    expect(document.querySelector('[data-violet-message-id="beta-progress-1"]')).toBeNull();
    expect(within(firstArticle).getByText(/4 updates · 2 agents/)).toBeInTheDocument();
    expect(firstArticle.querySelectorAll('.violet-progress-avatar-mini')).toHaveLength(2);
    expect(screen.getByText('Final reply stays outside progress.')).toBeVisible();

    fireEvent.click(firstArticle.querySelector('summary') as HTMLElement);
    expect(firstArticle.querySelectorAll('.violet-commentary-entry')).toHaveLength(4);
    expect(firstArticle.querySelectorAll('.violet-commentary-entry-speaker')).toHaveLength(4);
    expect(within(firstArticle).getByText('Alpha checks the room projection.')).toBeVisible();
    expect(within(firstArticle).getByText('Beta reviews the folding invariant.')).toBeVisible();

    const secondArticle = progressLabels[1]!.closest('article') as HTMLElement;
    expect(secondArticle.dataset.violetMessageId).toBe('alpha-progress-3');
    expect(within(secondArticle).queryByText(/1 agents/)).not.toBeInTheDocument();
    expect(secondArticle.querySelector('.violet-progress-avatar-stack')).toBeNull();
  });

  it('keeps filtered progress grouping scoped to the filtered agent', async () => {
    const projectRoot = '/tmp/kota-filtered-progress-test';
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [
        {
          id: 'alpha-filter-progress-1',
          sessionId: 's-alpha',
          agentId: 'alpha',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:00.000Z',
          text: 'Alpha first filtered update.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'beta-filter-progress-1',
          sessionId: 's-beta',
          agentId: 'beta',
          shell: 'claude',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:01.000Z',
          text: 'Beta should be hidden by the filter.',
          sourcePath: null,
          nativeEventId: null,
        },
        {
          id: 'alpha-filter-progress-2',
          sessionId: 's-alpha',
          agentId: 'alpha',
          shell: 'codex',
          role: 'assistant',
          kind: 'commentary',
          timestamp: '2026-05-20T10:00:02.000Z',
          text: 'Alpha second filtered update.',
          sourcePath: null,
          nativeEventId: null,
        },
      ],
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:00:03.000Z',
    });

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alpha', 'beta']}
        chatFilterActive
        chatFilterAgentIds={['alpha']}
        agentMeta={{
          alpha: { name: 'Alpha', emoji: 'A', role: 'Agent', hue: '#76a9d8', avatarClass: 'provider-codex' },
          beta: { name: 'Beta', emoji: 'B', role: 'Agent', hue: '#9a7bc4', avatarClass: 'provider-claude' },
        }}
      />,
    );

    const progressLabel = await screen.findByText('Progress');
    expect(screen.getAllByText('Progress')).toHaveLength(1);
    const article = progressLabel.closest('article') as HTMLElement;
    expect(within(article).getByText(/2 updates/)).toBeInTheDocument();
    expect(within(article).queryByText(/2 agents/)).not.toBeInTheDocument();
    expect(article.querySelector('.violet-progress-avatar-stack')).toBeNull();
    expect(article.querySelector('.violet-commentary-entry-speaker')).toBeNull();
    expect(screen.queryByText('Beta should be hidden by the filter.')).not.toBeInTheDocument();
  });

  it('renders Bartender conflict prompts as collapsed agent messages with a target badge', async () => {
    const projectRoot = '/tmp/kota-bartender-conflict-test';
    const prompt = [
      'Inspect the conflict state.',
      '',
      'Your commit `abc123` conflicts while I sync `mock/proj` into room HEAD `def456`.',
      '',
      'Git said:',
      'CONFLICT (content): Merge conflict in app.ts',
    ].join('\n');
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockResolvedValue({
      messages: [
        {
          id: 'bartender-conflict',
          sessionId: 'actor-bartender',
          agentId: 'bartender',
          shell: 'system',
          role: 'assistant',
          kind: 'message',
          timestamp: '2026-05-20T10:00:00.000Z',
          text: prompt,
          sourcePath: null,
          nativeEventId: 'bartender-conflict:alice:abc123:def456',
          targetAgentIds: ['alice'],
          agentDisplayName: 'Bartender',
          agentAvatarId: 'bartender',
          agentProvider: 'system',
        },
      ],
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:00:00.000Z',
    });

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        agentMeta={{
          alice: {
            name: 'Alice',
            emoji: '◇',
            role: 'Project agent',
            hue: 'var(--brass-bright)',
          },
        }}
      />,
    );

    const summaryText = await screen.findByText('@Alice resolving worktree conflict');
    const details = summaryText.closest('details') as HTMLDetailsElement;
    const article = summaryText.closest('article') as HTMLElement;
    expect(article).toHaveClass('agent');
    expect(article).toHaveClass('bartender-conflict');
    expect(within(article).getByText('Bartender')).toBeInTheDocument();
    expect(within(article).getByText('@Alice')).toBeInTheDocument();
    expect(details.open).toBe(false);
    expect(within(details).getByText(/CONFLICT \(content\)/)).toBeInTheDocument();

    fireEvent.click(details.querySelector('summary') as HTMLElement);
    expect(details.open).toBe(true);
  });

  it('auto-loads older pages when a target filter is empty in the latest page', async () => {
    const projectRoot = '/tmp/kota-filter-backfill';
    const latestPageFirstTimestamp = '2026-05-20T10:00:00.000Z';
    const readSpy = vi.spyOn(ptyClient, 'readVioletRoomCache').mockImplementation(async (request = {}) => ({
      messages: request.before
        ? [
            {
              id: 'alice-older-reply',
              sessionId: 's-a',
              agentId: 'alice',
              shell: 'claude',
              role: 'assistant',
              kind: 'message',
              timestamp: '2026-05-20T09:00:00.000Z',
              text: 'older alice reply',
              sourcePath: null,
              nativeEventId: null,
            },
          ]
        : Array.from({ length: 30 }, (_, index) => ({
            id: `bob-latest-reply-${index}`,
            sessionId: 's-b',
            agentId: 'bob',
            shell: 'codex',
            role: 'assistant',
            kind: 'message',
            timestamp: `2026-05-20T10:${String(index).padStart(2, '0')}:00.000Z`,
            text: index === 0 ? 'latest bob reply' : `latest bob reply ${index}`,
            sourcePath: null,
            nativeEventId: null,
          })),
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:00:01.000Z',
    }));

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice', 'bob']}
        chatFilterActive
        chatFilterAgentIds={['alice']}
      />,
    );

    expect(await screen.findByText('older alice reply')).toBeInTheDocument();
    expect(screen.queryByText('latest bob reply')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(readSpy).toHaveBeenCalledWith(expect.objectContaining({
        before: latestPageFirstTimestamp,
        agentIds: ['alice'],
      }));
    });
  });

  it('reads the latest filtered page with the target agent ids', async () => {
    const projectRoot = '/tmp/kota-filter-latest-agent-page';
    const readSpy = vi.spyOn(ptyClient, 'readVioletRoomCache').mockImplementation(async (request = {}) => ({
      messages: request.agentIds?.includes('alice')
        ? [
            {
              id: 'alice-latest-reply',
              sessionId: 's-a',
              agentId: 'alice',
              shell: 'claude',
              role: 'assistant',
              kind: 'message',
              timestamp: '2026-05-20T10:00:00.000Z',
              text: 'latest alice reply from filtered read',
              sourcePath: null,
              nativeEventId: null,
            },
          ]
        : [
            {
              id: 'bob-latest-reply',
              sessionId: 's-b',
              agentId: 'bob',
              shell: 'codex',
              role: 'assistant',
              kind: 'message',
              timestamp: '2026-05-20T10:00:00.000Z',
              text: 'latest bob reply from global read',
              sourcePath: null,
              nativeEventId: null,
            },
          ],
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:00:01.000Z',
    }));

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice', 'bob']}
        chatFilterActive
        chatFilterAgentIds={['alice']}
      />,
    );

    expect(await screen.findByText('latest alice reply from filtered read')).toBeInTheDocument();
    expect(screen.queryByText('latest bob reply from global read')).not.toBeInTheDocument();
    await waitFor(() => {
      expect(readSpy).toHaveBeenCalledWith(expect.objectContaining({
        agentIds: ['alice'],
      }));
    });
  });

  it('loads previous filtered messages using the earliest visible filtered message as the page anchor', async () => {
    const projectRoot = '/tmp/kota-filter-manual-older';
    const latestMessages = Array.from({ length: 30 }, (_, index) => ({
      id: index === 10 ? 'alice-latest-reply' : `bob-latest-reply-${index}`,
      sessionId: index === 10 ? 's-a' : 's-b',
      agentId: index === 10 ? 'alice' : 'bob',
      shell: index === 10 ? 'claude' : 'codex',
      role: 'assistant' as const,
      kind: 'message',
      timestamp: `2026-05-20T10:${String(index).padStart(2, '0')}:00.000Z`,
      text: index === 10 ? 'latest visible alice reply' : `latest bob reply ${index}`,
      sourcePath: null,
      nativeEventId: null,
    }));
    const readSpy = vi.spyOn(ptyClient, 'readVioletRoomCache').mockImplementation(async (request = {}) => ({
      messages: request.before
        ? [
            {
              id: 'alice-older-reply',
              sessionId: 's-a',
              agentId: 'alice',
              shell: 'claude',
              role: 'assistant',
              kind: 'message',
              timestamp: '2026-05-20T09:00:00.000Z',
              text: 'manual older alice reply',
              sourcePath: null,
              nativeEventId: null,
            },
          ]
        : latestMessages,
      sources: [],
      workEvents: [],
      rawLogDir: `${projectRoot}/project-memory/raw_logs`,
      chathistoryDir: `${projectRoot}/project-memory/chathistory`,
      syncedAt: '2026-05-20T10:30:01.000Z',
    }));

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice', 'bob']}
        chatFilterActive
        chatFilterAgentIds={['alice']}
      />,
    );

    expect(await screen.findByText('latest visible alice reply')).toBeInTheDocument();
    expect(screen.queryByText('latest bob reply 0')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: /load previous 30 messages/i }));

    expect(await screen.findByText('manual older alice reply')).toBeInTheDocument();
    await waitFor(() => {
      expect(readSpy).toHaveBeenCalledWith(expect.objectContaining({
        before: '2026-05-20T10:10:00.000Z',
        agentIds: ['alice'],
      }));
    });
  });

  it('keeps the filtered room viewport stable while prepending older messages', async () => {
    const projectRoot = '/tmp/kota-filter-prepend-scroll';
    const latestMessages = Array.from({ length: 30 }, (_, index) => ({
      id: `alice-latest-${index}`,
      sessionId: 's-a',
      agentId: 'alice',
      shell: 'claude',
      role: 'assistant' as const,
      kind: 'message',
      timestamp: `2026-05-20T10:${String(index).padStart(2, '0')}:00.000Z`,
      text: index === 0 ? 'top visible alice reply' : `latest alice reply ${index}`,
      sourcePath: null,
      nativeEventId: null,
    }));
    let resolveOlder: ((state: ptyClient.VioletRoomState) => void) | null = null;
    const olderPromise = new Promise<ptyClient.VioletRoomState>((resolve) => {
      resolveOlder = resolve;
    });
    vi.spyOn(ptyClient, 'readVioletRoomCache').mockImplementation(async (request = {}) => {
      if (request.before) return olderPromise;
      return {
        messages: latestMessages,
        sources: [],
        workEvents: [],
        rawLogDir: `${projectRoot}/project-memory/raw_logs`,
        chathistoryDir: `${projectRoot}/project-memory/chathistory`,
        syncedAt: '2026-05-20T10:30:01.000Z',
      };
    });

    render(
      <VioletRoomPanel
        projectRoot={projectRoot}
        agentIds={['alice']}
        chatFilterActive
        chatFilterAgentIds={['alice']}
      />,
    );

    expect(await screen.findByText('top visible alice reply')).toBeInTheDocument();
    const scroller = document.querySelector('.violet-room-scroll') as HTMLDivElement;
    let scrollHeight = 900;
    Object.defineProperty(scroller, 'clientHeight', {
      configurable: true,
      get: () => 300,
    });
    Object.defineProperty(scroller, 'scrollHeight', {
      configurable: true,
      get: () => scrollHeight,
    });
    await act(async () => {
      for (let i = 0; i < 12; i += 1) {
        await new Promise<void>((resolve) => {
          window.requestAnimationFrame(() => resolve());
        });
      }
    });
    scroller.scrollTop = 0;
    fireEvent.scroll(scroller);
    await act(async () => {
      await new Promise<void>((resolve) => {
        window.requestAnimationFrame(() => resolve());
      });
    });
    expect(scroller.scrollTop).toBe(0);
    expect(scroller.scrollHeight).toBe(900);

    await userEvent.click(screen.getByRole('button', { name: /load previous 30 messages/i }));
    expect(await screen.findByText('Loading previous messages')).toBeInTheDocument();

    scrollHeight = 1260;
    await act(async () => {
      resolveOlder?.({
        messages: [
          {
            id: 'alice-older-prepend',
            sessionId: 's-a',
            agentId: 'alice',
            shell: 'claude',
            role: 'assistant',
            kind: 'message',
            timestamp: '2026-05-20T09:00:00.000Z',
            text: 'older alice message above viewport',
            sourcePath: null,
            nativeEventId: null,
          },
        ],
        sources: [],
        workEvents: [],
        rawLogDir: `${projectRoot}/project-memory/raw_logs`,
        chathistoryDir: `${projectRoot}/project-memory/chathistory`,
        syncedAt: '2026-05-20T10:30:02.000Z',
      });
      await olderPromise;
    });

    expect(await screen.findByText('older alice message above viewport')).toBeInTheDocument();
    await waitFor(() => {
      expect(scroller.scrollTop).toBe(360);
    });
  });

  it('toggles target-following group chat filter from the ribbon button', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    const bob = await recruitHero('hero-dex');

    await userEvent.click(chip(alice));
    await userEvent.click(screen.getByTestId('ribbon-filter-clear'));
    expect(screen.getByTestId('group-chat-overlay')).toHaveClass('chat-filter-active');
    expect(chip(alice)).toHaveClass('chat-filter-target');
    expect(screen.getByTestId('ribbon-filter-clear')).toHaveAttribute('data-chat-filter-mode', 'filter');
    expect(screen.getByTestId('ribbon-filter-clear')).toHaveAccessibleName('Toggle chat filter. Current: Filtered.');
    expect(screen.getByText('Selected agents · current')).toBeInTheDocument();

    await act(async () => {});
    act(() => {
      emitVioletComposerSent({
        projectRoot: '/tmp/kota-dev',
        text: 'direct to filtered alice',
        targetAgentIds: [alice],
        privacy: false,
      });
      emitVioletComposerSent({
        projectRoot: '/tmp/kota-dev',
        text: 'broadcast to both filtered agents',
        targetAgentIds: [alice, bob],
        privacy: false,
      });
      emitVioletComposerSent({
        projectRoot: '/tmp/kota-dev',
        text: 'direct to filtered bob',
        targetAgentIds: [bob],
        privacy: false,
      });
      window.dispatchEvent(new CustomEvent('violet://room/synced', {
        detail: {
          request: { projectRoot: null, agentIds: [alice, bob] },
          state: {
            messages: [
              {
                id: 'alice-filter-reply',
                sessionId: 's-a',
                agentId: alice,
                shell: 'claude',
                role: 'assistant',
                kind: 'message',
                timestamp: '2026-05-20T10:00:00.000Z',
                text: 'alice filtered reply',
                sourcePath: null,
                nativeEventId: null,
              },
              {
                id: 'bob-filter-reply',
                sessionId: 's-b',
                agentId: bob,
                shell: 'codex',
                role: 'assistant',
                kind: 'message',
                timestamp: '2026-05-20T10:00:01.000Z',
                text: 'bob filtered reply',
                sourcePath: null,
                nativeEventId: null,
              },
            ],
            sources: [],
            workEvents: [],
            rawLogDir: '/tmp/kota-test/project-memory/raw_logs',
            chathistoryDir: '/tmp/kota-test/project-memory/chathistory',
            syncedAt: '2026-05-20T10:00:02.000Z',
          },
        },
      }));
    });

    expect(await screen.findByText('direct to filtered alice')).toBeInTheDocument();
    expect(screen.getByText('broadcast to both filtered agents')).toBeInTheDocument();
    expect(screen.getByText('alice filtered reply')).toBeInTheDocument();
    expect(screen.queryByText('direct to filtered bob')).not.toBeInTheDocument();
    expect(screen.queryByText('bob filtered reply')).not.toBeInTheDocument();

    await userEvent.click(chip(bob));
    expect(chip(bob)).toHaveClass('chat-filter-target');
    expect(chip(alice)).not.toHaveClass('chat-filter-target');
    expect(screen.queryByText('direct to filtered alice')).not.toBeInTheDocument();
    expect(screen.getByText('broadcast to both filtered agents')).toBeInTheDocument();
    expect(screen.getByText('direct to filtered bob')).toBeInTheDocument();
    expect(screen.getByText('bob filtered reply')).toBeInTheDocument();
    expect(screen.queryByText('alice filtered reply')).not.toBeInTheDocument();

    await userEvent.click(screen.getByTestId('ribbon-filter-clear'));
    expect(screen.getByTestId('group-chat-overlay')).not.toHaveClass('chat-filter-active');
    expect(screen.getByTestId('ribbon-filter-clear')).toHaveAttribute('data-chat-filter-mode', 'all');
    expect(chip(bob)).not.toHaveClass('chat-filter-target');
    expect(screen.getByText('direct to filtered alice')).toBeInTheDocument();
    expect(screen.getByText('alice filtered reply')).toBeInTheDocument();
  });

  it('hides parked private chat and kage bunshin entry points', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    expect(screen.queryByTestId('ib-mode-indicator')).not.toBeInTheDocument();
    expect(screen.queryByTestId('ib-privacy-tool')).not.toBeInTheDocument();
    expect(screen.queryByTestId('privacy-trigger')).not.toBeInTheDocument();

    fireEvent.contextMenu(chip(alice));
    const menu = screen.getByRole('menu');
    expect(menu).toHaveTextContent('Detail');
    expect(menu).toHaveTextContent('Terminal');
    expect(within(menu).queryByText('Private Chat')).not.toBeInTheDocument();
    expect(within(menu).queryByText('End Private Chat')).not.toBeInTheDocument();
    expect(within(menu).queryByText('Kage Bunshin')).not.toBeInTheDocument();
    expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).not.toHaveClass('private');
  });

  it('broadcast mode stays public while private chat is parked', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    const bob = await recruitHero('hero-dex');
    await userEvent.click(screen.getByTestId('ib-target-pill'));
    expect(broadcastOption(bob)).toHaveClass('selected');
    await userEvent.click(broadcastOption(alice));
    await userEvent.click(screen.getByTestId('broadcast-confirm'));
    expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).toHaveClass('broadcast');
    expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).not.toHaveClass('broadcast-mixed');
    expect(screen.getByTestId('ib-mode-indicator')).toHaveTextContent('Broadcast');
    expect(screen.getByTestId('ib-mode-indicator')).not.toHaveTextContent('private');
    expect(screen.getByTestId('ib-mode-indicator')).not.toHaveTextContent('public');
  });

  it('clicking the target pill opens broadcast selection; single select becomes direct target', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    await userEvent.click(screen.getByTestId('ib-target-pill'));
    expect(screen.getByTestId('broadcast-target-popover')).toBeInTheDocument();
    expect(broadcastOption(alice)).toHaveClass('selected');
    await userEvent.click(screen.getByTestId('broadcast-confirm'));
    await waitFor(() => expect(screen.queryByTestId('broadcast-target-popover')).not.toBeInTheDocument());
    expect(chip(alice)).toHaveClass('target');
  });

  it('Ctrl+1 focuses the first seat terminal and routes the composer target there', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    const bob = await recruitHero('hero-dex');
    expect(chip(bob)).toHaveClass('target');
    fireEvent.keyDown(window, { key: '1', ctrlKey: true });
    await waitFor(() => expect(winFrame(alice)).toHaveClass('focused'));
    expect(chip(alice)).toHaveClass('target');
  });

  it('double-clicking an active agent chip leaves terminal input focused', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    const field = screen.getByTestId('input-field');
    field.focus();
    expect(document.activeElement).toBe(field);

    await userEvent.dblClick(chip(alice));

    const terminalInput = screen.getByLabelText(/CC.*terminal input/) as HTMLTextAreaElement;
    await waitFor(() => expect(winFrame(alice)).toHaveClass('focused'));
    await waitFor(() => expect(document.activeElement).toBe(terminalInput));
  });

  it('Ctrl+1 minimizes the first seat terminal when it is already focused', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    await recruitHero('hero-dex');
    const field = screen.getByTestId('input-field');
    fireEvent.keyDown(window, { key: '1', ctrlKey: true });
    await waitFor(() => expect(winFrame(alice)).toHaveClass('focused'));
    fireEvent.keyDown(window, { key: '1', ctrlKey: true });
    await waitFor(() => expect(screen.queryByTestId(`win-frame-${alice}`)).not.toBeInTheDocument());
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(chip(alice)).toHaveClass('target');
  });

  it('Ctrl+1 on an empty seat opens the shortcut add-agent dialog', async () => {
    render(<App />);
    fireEvent.keyDown(window, { key: '1', ctrlKey: true });
    expect(await screen.findByTestId('shortcut-recruit-modal')).toBeInTheDocument();
    expect(screen.getByTestId('incarnate-shortcut-picker')).toBeInTheDocument();
    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(screen.queryByTestId('shortcut-recruit-modal')).not.toBeInTheDocument());
    fireEvent.keyDown(window, { key: '1', ctrlKey: true });
    const before = currentAgentChipIds();
    await userEvent.click(await screen.findByTestId('incarnate-shortcut-hero-cc'));
    const alice = await waitForNewAgentChip(before);
    await waitFor(() => expect(winFrame(alice)).toHaveClass('focused'));
  });

  it('Ctrl+8 addresses the eighth agent slot instead of opening broadcast selection', async () => {
    render(<App />);
    fireEvent.keyDown(window, { key: '8', ctrlKey: true });
    expect(await screen.findByTestId('shortcut-recruit-modal')).toBeInTheDocument();
    expect(screen.queryByTestId('broadcast-target-popover')).not.toBeInTheDocument();
  });

  it('PageUp and PageDown cycle the composer target across table agents', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    const bob = await recruitHero('hero-dex');
    expect(chip(bob)).toHaveClass('target');
    const field = screen.getByTestId('input-field');
    field.focus();
    expect(document.activeElement).toBe(field);
    fireEvent.keyDown(field, { key: 'PageUp' });
    await waitFor(() => expect(chip(alice)).toHaveClass('target'));
    fireEvent.keyDown(field, { key: 'PageDown' });
    expect(chip(bob)).toHaveClass('target');
  });

  it('clicking a seat updates composer target without rewriting the prompt', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    await userEvent.type(screen.getByTestId('input-field'), 'please wire');
    await userEvent.click(screen.getByRole('button', { name: /Switch focus to CC/ }));
    expect(chip(alice)).toHaveClass('target');
    expect(composerText()).toBe('please wire');
  });

  it('clicking another recruited agent switches target and keeps prompt text intact', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    const bob = await recruitHero('hero-dex');
    await userEvent.type(screen.getByTestId('input-field'), 'plan the thing');
    await userEvent.click(screen.getByRole('button', { name: /Switch focus to CC/ }));
    expect(chip(alice)).toHaveClass('target');
    await userEvent.click(screen.getByRole('button', { name: /Switch focus to Dex/ }));
    expect(chip(bob)).toHaveClass('target');
    expect(composerText()).toBe('plan the thing');
  });

  it('broadcast mode toggles back without hiding the composer', async () => {
    render(<App />);
    await enableBroadcastMode();
    await userEvent.click(screen.getByTestId('ib-target-pill'));
    await screen.findByTestId('broadcast-target-popover');
    await userEvent.click(screen.getByLabelText('Close broadcast target selection'));
    await waitFor(() => expect(screen.getByTestId('input-field').closest('.input-bar-wrap')).not.toHaveClass('broadcast'));
    expect(chip(recruitedHeroAgents['hero-cc'][0])).toHaveClass('target');
    expect(screen.getByTestId('input-field')).toBeInTheDocument();
    expect(screen.queryByTestId('chip-all')).not.toBeInTheDocument();
  });

  it('AgentWindowsLayer is mounted as a workspace overlay', () => {
    render(<App />);
    expect(screen.getByTestId('agent-windows-layer')).toBeInTheDocument();
  });
});

describe('AWL · agent terminal input', () => {
  function snapWithCursor(
    rows: number,
    cols: number,
    cursorRow: number,
    cursorCol: number,
    cursorVisible: boolean,
  ): GridSnapshot {
    return {
      rows,
      cols,
      cells: Array.from({ length: rows * cols }, () => ({ ch: ' ' })),
      cursorRow,
      cursorCol,
      cursorVisible,
    };
  }

  async function publishGrid(
    gridStore: ReturnType<typeof createAgentGridStore>,
    agentId: AgentId,
    grid: GridSnapshot,
  ) {
    await act(async () => {
      gridStore.setSnapshot(agentId, grid);
      await new Promise<void>((resolve) => window.requestAnimationFrame(() => resolve()));
    });
  }

  function renderAgentInput({
    onKey = vi.fn(),
    grid,
    cli = 'claude',
    focusedAgent = 'alice',
    onFocusAgent = vi.fn(),
    focusInput = true,
  }: {
    onKey?: ReturnType<typeof vi.fn>;
    grid?: GridSnapshot;
    cli?: AgentCli;
    focusedAgent?: AgentId | null;
    onFocusAgent?: ReturnType<typeof vi.fn>;
    focusInput?: boolean;
  } = {}) {
    const gridStore = createAgentGridStore();
    if (grid) gridStore.setSnapshot('alice', grid);
    const result = render(
      <>
        <button type="button">focus sink</button>
        <AgentWindowsLayer
          liveAgents={['alice']}
          gridStore={gridStore}
          status={new Map([
            ['alice', { agentId: 'alice', running: true, cli, cwd: '/tmp/alice' }],
          ])}
          focusedAgent={focusedAgent}
          minimized={new Set()}
          onFocusAgent={onFocusAgent}
          onMinimizeAgent={() => {}}
          onAgentKey={(_id, bytes) => onKey(bytes)}
          projectId="test"
        />
      </>,
    );
    const input = screen.getByLabelText('CC terminal input') as HTMLTextAreaElement;
    if (focusInput) input.focus();
    return { input, onKey, container: result.container, onFocusAgent, gridStore };
  }

  it('keeps a minimized terminal unsubscribed and restores on the latest hidden frame', async () => {
    const gridStore = createAgentGridStore();
    const first = snapWithCursor(1, 5, 0, 0, false);
    first.cells = ['f', 'i', 'r', 's', 't'].map((ch) => ({ ch }));
    const latest = snapWithCursor(1, 6, 0, 0, false);
    latest.cells = ['l', 'a', 't', 'e', 's', 't'].map((ch) => ({ ch }));
    gridStore.setSnapshot('alice', first);
    const subscribe = vi.spyOn(gridStore, 'subscribe');
    const props = (minimized: ReadonlySet<AgentId>) => (
      <AgentWindowsLayer
        liveAgents={['alice']}
        gridStore={gridStore}
        status={new Map([
          ['alice', { agentId: 'alice', running: true, cli: 'claude' as const, cwd: '/tmp/alice' }],
        ])}
        focusedAgent={null}
        minimized={minimized}
        onFocusAgent={() => {}}
        onMinimizeAgent={() => {}}
        onAgentKey={() => {}}
        projectId="hidden-terminal"
      />
    );
    const view = render(props(new Set<AgentId>(['alice'])));

    expect(subscribe).not.toHaveBeenCalled();
    expect(screen.queryByTestId('term-grid')).not.toBeInTheDocument();
    gridStore.setSnapshot('alice', latest);
    expect(subscribe).not.toHaveBeenCalled();

    view.rerender(props(new Set()));
    await waitFor(() => expect(screen.getByTestId('term-grid')).toHaveTextContent('latest'));
    expect(subscribe).toHaveBeenCalledTimes(1);

    view.rerender(props(new Set<AgentId>(['alice'])));
    await waitFor(() => expect(screen.queryByTestId('term-grid')).not.toBeInTheDocument());
  });

  it('reattaches resize observation to the new terminal body after restore', () => {
    vi.useFakeTimers();
    const resize = vi.spyOn(ptyClient, 'resizeAgentPty').mockResolvedValue();
    const observers: Array<{
      callback: ResizeObserverCallback;
      observe: ReturnType<typeof vi.fn>;
      disconnect: ReturnType<typeof vi.fn>;
    }> = [];
    class TestResizeObserver {
      callback: ResizeObserverCallback;
      observe = vi.fn();
      unobserve = vi.fn();
      disconnect = vi.fn();

      constructor(callback: ResizeObserverCallback) {
        this.callback = callback;
        observers.push(this);
      }
    }
    vi.stubGlobal('ResizeObserver', TestResizeObserver);
    const gridStore = createAgentGridStore();
    gridStore.setSnapshot('alice', snapWithCursor(20, 80, 0, 0, false));
    const props = (minimized: ReadonlySet<AgentId>) => (
      <AgentWindowsLayer
        liveAgents={['alice']}
        gridStore={gridStore}
        status={new Map([
          ['alice', { agentId: 'alice', running: true, cli: 'claude' as const, cwd: '/tmp/alice' }],
        ])}
        focusedAgent={null}
        minimized={minimized}
        onFocusAgent={() => {}}
        onMinimizeAgent={() => {}}
        onAgentKey={() => {}}
        projectId="resize-after-restore"
      />
    );

    try {
      const view = render(props(new Set()));
      expect(observers).toHaveLength(1);
      const firstBody = observers[0]!.observe.mock.calls[0]?.[0];
      act(() => {
        observers[0]!.callback([{
          contentRect: { width: 770, height: 300 },
        } as ResizeObserverEntry], observers[0] as unknown as ResizeObserver);
        vi.advanceTimersByTime(50);
      });
      expect(resize).toHaveBeenCalledTimes(1);

      view.rerender(props(new Set<AgentId>(['alice'])));
      expect(observers[0]!.disconnect).toHaveBeenCalledTimes(1);
      view.rerender(props(new Set()));
      expect(observers).toHaveLength(2);
      const restoredBody = observers[1]!.observe.mock.calls[0]?.[0];
      expect(restoredBody).not.toBe(firstBody);
      act(() => {
        observers[1]!.callback([{
          contentRect: { width: 616, height: 240 },
        } as ResizeObserverEntry], observers[1] as unknown as ResizeObserver);
        vi.advanceTimersByTime(50);
      });
      expect(resize).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it('keeps project and agent grid subscriptions isolated while sessions stay live', async () => {
    const gridStore = createAgentGridStore();
    const frame = (text: string) => {
      const snap = snapWithCursor(1, text.length, 0, 0, false);
      snap.cells = Array.from(text, (ch) => ({ ch }));
      return snap;
    };
    gridStore.setSnapshot('alice', frame('alice-1'));
    gridStore.setSnapshot('bob', frame('bob-1'));
    const props = (agentId: AgentId, projectId: string) => (
      <AgentWindowsLayer
        liveAgents={[agentId]}
        gridStore={gridStore}
        status={new Map([
          [agentId, { agentId, running: true, cli: 'claude' as const, cwd: `/tmp/${agentId}` }],
        ])}
        focusedAgent={null}
        minimized={new Set()}
        onFocusAgent={() => {}}
        onMinimizeAgent={() => {}}
        onAgentKey={() => {}}
        projectId={projectId}
      />
    );
    const view = render(props('alice', 'project-a'));
    expect(screen.getByTestId('term-grid')).toHaveTextContent('alice-1');

    view.rerender(props('bob', 'project-b'));
    await waitFor(() => expect(screen.getByTestId('term-grid')).toHaveTextContent('bob-1'));
    gridStore.setSnapshot('alice', frame('alice-2'));
    await publishGrid(gridStore, 'bob', frame('bob-2'));
    expect(screen.getByTestId('term-grid')).toHaveTextContent('bob-2');
    expect(screen.getByTestId('term-grid')).not.toHaveTextContent('alice-2');

    view.rerender(props('alice', 'project-a'));
    await waitFor(() => expect(screen.getByTestId('term-grid')).toHaveTextContent('alice-2'));
  });

  it('sends inserted symbol text via the browser text-input path', () => {
    const { input, onKey } = renderAgentInput();
    fireEvent.input(input, { target: { value: '/?@[]{}#$' } });
    expect(onKey).toHaveBeenCalledWith('/?@[]{}#$');
    expect(input.value).toBe('');
  });

  it('focuses terminal input when raised through the imperative handle', async () => {
    const ref = createRef<AgentWindowsLayerHandle>();
    const gridStore = createAgentGridStore();
    render(
      <>
        <button type="button">focus sink</button>
        <AgentWindowsLayer
          ref={ref}
          liveAgents={['alice']}
          gridStore={gridStore}
          status={new Map([
            ['alice', { agentId: 'alice', running: true, cli: 'claude', cwd: '/tmp/alice' }],
          ])}
          focusedAgent="alice"
          minimized={new Set()}
          onFocusAgent={() => {}}
          onMinimizeAgent={() => {}}
          onAgentKey={() => {}}
          projectId="test"
        />
      </>,
    );

    const sink = screen.getByRole('button', { name: 'focus sink' });
    const input = screen.getByLabelText('CC terminal input') as HTMLTextAreaElement;
    sink.focus();
    expect(document.activeElement).toBe(sink);

    act(() => {
      ref.current?.bringToFront('alice');
    });

    await waitFor(() => expect(document.activeElement).toBe(input));
  });

  it('does not steal focus on terminal body pointerdown so text can be selected', async () => {
    const { container, input, onFocusAgent } = renderAgentInput({
      focusedAgent: null,
      focusInput: false,
    });
    const sink = screen.getByRole('button', { name: 'focus sink' });
    const body = container.querySelector('.win-terminal-body');
    expect(body).not.toBeNull();
    sink.focus();
    expect(document.activeElement).toBe(sink);

    fireEvent.pointerDown(body as Element, { clientX: 20, clientY: 20 });

    expect(onFocusAgent).toHaveBeenCalledWith('alice');
    expect(document.activeElement).toBe(sink);

    fireEvent.pointerUp(body as Element, { clientX: 20, clientY: 20 });

    await waitFor(() => expect(document.activeElement).toBe(input));
  });

  it('does not drop Option/Alt-produced symbols before input fires', () => {
    const { input, onKey } = renderAgentInput();
    fireEvent.keyDown(input, { key: '@', altKey: true });
    expect(onKey).not.toHaveBeenCalled();
    fireEvent.input(input, { target: { value: '@' } });
    expect(onKey).toHaveBeenCalledWith('@');
  });

  it('keeps control keys on the keydown path', () => {
    const { input, onKey } = renderAgentInput();
    fireEvent.keyDown(input, { key: 'Enter' });
    fireEvent.keyDown(input, { key: 'Backspace' });
    expect(onKey).toHaveBeenNthCalledWith(1, '\r');
    expect(onKey).toHaveBeenNthCalledWith(2, '\x7f');
  });

  it('keeps ordinary wheel scrolling on the backend display-offset path', () => {
    const scroll = vi.spyOn(ptyClient, 'scrollAgentPty').mockResolvedValue();
    const { container, onKey } = renderAgentInput({
      grid: snapWithCursor(20, 80, 0, 0, false),
      focusedAgent: null,
      focusInput: false,
    });
    const body = container.querySelector('.win-terminal-body') as HTMLElement;

    fireEvent.wheel(body, { deltaY: -30, deltaMode: 0 });
    fireEvent.wheel(body, { deltaY: 15, deltaMode: 0 });

    expect(scroll).toHaveBeenNthCalledWith(1, 'alice', 3);
    expect(scroll).toHaveBeenNthCalledWith(2, 'alice', -2);
    expect(onKey).not.toHaveBeenCalled();
  });

  it('uses the latest mouse mode snapshot for SGR wheel bytes instead of scroll IPC', async () => {
    const scroll = vi.spyOn(ptyClient, 'scrollAgentPty').mockResolvedValue();
    const initial = snapWithCursor(20, 80, 0, 0, false);
    const { container, onKey, gridStore } = renderAgentInput({
      grid: initial,
      focusedAgent: null,
      focusInput: false,
    });
    const mouseGrid = { ...initial, mouseMode: true, sgrMouse: true };
    await publishGrid(gridStore, 'alice', mouseGrid);
    const body = container.querySelector('.win-terminal-body') as HTMLElement;

    const wheel = new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: -80 });
    Object.defineProperties(wheel, {
      clientX: { value: 14 },
      clientY: { value: 30 },
    });
    fireEvent(body, wheel);

    expect(onKey).toHaveBeenCalledWith('\x1b[<64;2;3M');
    expect(scroll).not.toHaveBeenCalled();
  });

  it('preserves legacy X10 wheel encoding when SGR mouse is disabled', () => {
    const scroll = vi.spyOn(ptyClient, 'scrollAgentPty').mockResolvedValue();
    const grid = {
      ...snapWithCursor(20, 80, 0, 0, false),
      mouseMode: true,
      sgrMouse: false,
    };
    const { container, onKey } = renderAgentInput({
      grid,
      focusedAgent: null,
      focusInput: false,
    });
    const body = container.querySelector('.win-terminal-body') as HTMLElement;

    const wheel = new WheelEvent('wheel', { bubbles: true, cancelable: true, deltaY: -80 });
    Object.defineProperties(wheel, {
      clientX: { value: 14 },
      clientY: { value: 30 },
    });
    fireEvent(body, wheel);

    expect(onKey).toHaveBeenCalledWith(
      `\x1b[M${String.fromCharCode(32 + 64)}${String.fromCharCode(32 + 2)}${String.fromCharCode(32 + 3)}`,
    );
    expect(scroll).not.toHaveBeenCalled();
  });

  it('drops file paths into the terminal input using terminal-style escaping', () => {
    const { container, onKey } = renderAgentInput();
    const body = container.querySelector('.win-terminal-body');
    expect(body).not.toBeNull();
    fireEvent.drop(body as Element, {
      dataTransfer: {
        files: [{ path: '/tmp/a file.txt' }],
        getData: () => '',
      },
    });
    expect(onKey).toHaveBeenCalledWith('/tmp/a\\ file.txt');
  });

  it('marks terminal bodies for native Tauri file-drop targeting', () => {
    const { container } = renderAgentInput();
    const body = container.querySelector('.win-terminal-body');
    expect(body?.getAttribute('data-agent-terminal-body')).toBe('true');
    expect(body?.getAttribute('data-agent-id')).toBe('alice');
  });

  it('prefers raw Tauri drop coordinates when scaled coordinates hit another terminal', () => {
    const originalElementFromPoint = document.elementFromPoint;
    const originalDevicePixelRatio = window.devicePixelRatio;
    Object.defineProperty(window, 'devicePixelRatio', { value: 2, configurable: true });

    const bob = document.createElement('div');
    bob.dataset.agentTerminalBody = 'true';
    bob.dataset.agentId = 'bob';
    bob.getBoundingClientRect = () => ({
      left: 90,
      top: 90,
      right: 160,
      bottom: 160,
      width: 70,
      height: 70,
      x: 90,
      y: 90,
      toJSON: () => ({}),
    });
    const david = document.createElement('div');
    david.dataset.agentTerminalBody = 'true';
    david.dataset.agentId = 'david';
    david.getBoundingClientRect = () => ({
      left: 200,
      top: 200,
      right: 300,
      bottom: 300,
      width: 100,
      height: 100,
      x: 200,
      y: 200,
      toJSON: () => ({}),
    });

    document.elementFromPoint = vi.fn((x: number, y: number) => {
      if (x >= 200 && x <= 300 && y >= 200 && y <= 300) return david;
      if (x >= 90 && x <= 160 && y >= 90 && y <= 160) return bob;
      return null;
    });

    try {
      const hit = terminalDropTargetAtPosition(
        { type: 'drop', paths: ['/tmp/demo.txt'], position: { x: 240, y: 240 } as any },
        new Map([
          ['bob', { el: bob, focusInput: vi.fn() }],
          ['david', { el: david, focusInput: vi.fn() }],
        ]),
        ['bob', 'david'],
      );

      expect(hit?.agentId).toBe('david');
    } finally {
      document.elementFromPoint = originalElementFromPoint;
      Object.defineProperty(window, 'devicePixelRatio', {
        value: originalDevicePixelRatio,
        configurable: true,
      });
    }
  });

  it('anchors IME capture at a useful terminal cursor with room for composition', () => {
    const { input } = renderAgentInput({
      cli: 'codex',
      grid: snapWithCursor(20, 80, 4, 3, true),
    });
    expect(input.style.top).toBe(`${4 * TERMINAL_LINE_HEIGHT}px`);
    expect(input.style.left).toBe(`${3 * TERMINAL_CELL_WIDTH}px`);
    expect(input.style.right).toBe('0px');
    expect(input.style.bottom).toBe('0px');
    expect(input.style.minWidth).toBe(`${32 * TERMINAL_CELL_WIDTH}px`);
    expect(input.getAttribute('wrap')).toBe('off');
    expect(input.style.pointerEvents).toBe('none');
  });

  it('bottom-anchors ratatui CLIs when their terminal cursor is frozen at origin', () => {
    const { input } = renderAgentInput({
      cli: 'codex',
      grid: snapWithCursor(20, 80, 0, 0, true),
    });
    expect(input.style.top).toBe(`${19 * TERMINAL_LINE_HEIGHT}px`);
    expect(input.style.left).toBe('0px');
  });

  it('trusts moved Claude cursor coordinates even if the cursor is hidden', () => {
    const { input } = renderAgentInput({
      cli: 'claude',
      grid: snapWithCursor(20, 80, 6, 2, false),
    });
    expect(input.style.top).toBe(`${6 * TERMINAL_LINE_HEIGHT}px`);
    expect(input.style.left).toBe(`${2 * TERMINAL_CELL_WIDTH}px`);
  });

  it('moves the IME anchor with the latest leaf snapshot', async () => {
    const initial = snapWithCursor(20, 80, 1, 1, true);
    const { input, gridStore } = renderAgentInput({ cli: 'claude', grid: initial });

    await publishGrid(gridStore, 'alice', snapWithCursor(20, 80, 7, 4, true));

    expect(input.style.top).toBe(`${7 * TERMINAL_LINE_HEIGHT}px`);
    expect(input.style.left).toBe(`${4 * TERMINAL_CELL_WIDTH}px`);
  });
});

describe('Tavern · storage measurement', () => {
  it('formats storage measurement age with one largest unit and no seconds', () => {
    const now = 200 * 24 * 60 * 60 * 1000;
    expect(formatStorageMeasurementAge(now / 1000 - 59, now)).toBe('just now');
    expect(formatStorageMeasurementAge(now / 1000 - 133, now)).toBe('2m ago');
    expect(formatStorageMeasurementAge(now / 1000 - (20 * 60 * 60 + 48 * 60), now)).toBe('20h ago');
    expect(formatStorageMeasurementAge(now / 1000 - (100 * 24 * 60 * 60 + 20 * 60 * 60), now)).toBe('100 days ago');
  });

  it('shows an account-only inline empty state without Available or project size', async () => {
    vi.spyOn(ptyClient, 'storageMeasureStatus').mockResolvedValue({
      updating: false,
      onDiskBytes: null,
      availableBytes: null,
      measuredAt: null,
      error: null,
    });

    render(<App />);
    await userEvent.click(screen.getByTestId('tavern-btn'));
    await screen.findByTestId('tavern-page');
    await userEvent.click(screen.getByRole('button', { name: 'Link' }));
    const storage = await screen.findByTestId('tavern-local-storage');

    await waitFor(() => expect(storage).toHaveTextContent('Size not measured'));
    expect(storage).not.toHaveTextContent('available');
    expect(storage).not.toHaveTextContent('Project files');
    expect(within(storage).getByRole('button', { name: 'Refresh storage usage' })).toBeEnabled();
  });

  it('keeps the previous storage result visible while a manual refresh runs', async () => {
    const measuredAt = Math.floor(Date.now() / 1000) - 7 * 60;
    const ready = {
      updating: false,
      onDiskBytes: 46 * 1024 ** 3,
      availableBytes: 161 * 1024 ** 3,
      measuredAt,
      error: null,
    };
    vi.spyOn(ptyClient, 'storageMeasureStatus').mockResolvedValue(ready);
    const start = vi.spyOn(ptyClient, 'storageMeasureStart').mockResolvedValue({
      ...ready,
      updating: true,
    });

    render(<App />);
    await userEvent.click(screen.getByTestId('tavern-btn'));
    await screen.findByTestId('tavern-page');
    await userEvent.click(screen.getByRole('button', { name: 'Link' }));
    const storage = await screen.findByTestId('tavern-local-storage');

    await waitFor(() => expect(storage).toHaveTextContent('Size ≈ 46.0 GB'));
    expect(storage).toHaveTextContent('161.0 GB available');
    expect(storage).toHaveTextContent('Updated 7m ago');

    const refresh = within(storage).getByRole('button', { name: 'Refresh storage usage' });
    await userEvent.click(refresh);
    await waitFor(() => expect(start).toHaveBeenCalledTimes(1));
    expect(storage).toHaveTextContent('Size ≈ 46.0 GB');
    expect(storage).toHaveTextContent('Updating…');
    expect(storage).toHaveTextContent('will take 2–5 min');
    expect(refresh).toBeDisabled();
  });
});

describe('M6.A · quick incarnate picker', () => {
  it('uses the topbar Tavern control as a project back button while Tavern is open', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('tavern-btn'));
    await screen.findByTestId('tavern-page');

    const tavernButton = screen.getByTestId('tavern-btn');
    expect(tavernButton).toHaveAttribute('aria-label', 'Back to project');
    await userEvent.click(tavernButton);

    await waitFor(() => {
      expect(screen.queryByTestId('tavern-page')).not.toBeInTheDocument();
    });
  });

  it('opens active working heroes from the agent bar without entering Tavern', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('ribbon-add'));
    expect(screen.getByTestId('shortcut-recruit-modal')).toBeInTheDocument();
    expect(screen.queryByTestId('tavern-page')).not.toBeInTheDocument();
    expect(screen.getByTestId('incarnate-shortcut-hero-cc')).toHaveTextContent('CC');
    expect(screen.getByTestId('incarnate-shortcut-hero-dex')).toHaveTextContent('Dex');
    expect(screen.getByTestId('incarnate-shortcut-hero-gem')).toHaveTextContent('Gem');
    expect(screen.getByTestId('incarnate-shortcut-hero-op')).toHaveTextContent('Op');
    await waitFor(() => expect(screen.getByTestId('incarnate-shortcut-hero-gem')).toBeDisabled());
    expect(screen.getByTestId('incarnate-shortcut-hero-op')).toBeDisabled();
    expect(screen.queryByTestId('incarnate-shortcut-david')).not.toBeInTheDocument();
    expect(screen.queryByTestId('incarnate-shortcut-charlie')).not.toBeInTheDocument();
  });

  it('does not offer archived Tavern heroes in project recruit pickers', async () => {
    const localStorage = withMockLocalStorage({
      'kota-v2.dev.project-root': '/tmp/kota-test',
      'kota-v2.tavern.hero-profiles': JSON.stringify({
        'hero-cc': { name: 'CC', archived: true },
        'hero-dex': { name: 'Dex' },
        'custom-gone': { name: 'Gone', provider: 'codex', archived: true },
        'custom-live': { name: 'Live', provider: 'codex' },
      }),
      'kota-v2.tavern.custom-heroes': JSON.stringify([
        { id: 'custom-gone', kind: 'custom', provider: 'codex', name: 'Gone' },
        { id: 'custom-live', kind: 'custom', provider: 'codex', name: 'Live' },
      ]),
    });
    try {
      render(<App />);
      await userEvent.click(screen.getByTestId('ribbon-add'));
      expect(screen.getByTestId('shortcut-recruit-modal')).toBeInTheDocument();
      expect(screen.queryByTestId('incarnate-shortcut-hero-cc')).not.toBeInTheDocument();
      expect(screen.getByTestId('incarnate-shortcut-hero-dex')).toHaveTextContent('Dex');
      expect(screen.queryByTestId('incarnate-shortcut-custom-gone')).not.toBeInTheDocument();
      expect(screen.getByTestId('incarnate-shortcut-custom-live')).toHaveTextContent('Live');
    } finally {
      localStorage.restore();
    }
  });

  it('can recruit the same hero template multiple times with project-local names', async () => {
    render(<App />);
    const firstAlice = await recruitHero('hero-cc');
    await userEvent.click(screen.getByTestId('ribbon-add'));
    expect(screen.getByTestId('incarnate-shortcut-hero-cc')).toBeEnabled();
    expect(screen.getByTestId('incarnate-shortcut-hero-dex')).toBeEnabled();
    const before = currentAgentChipIds();
    await userEvent.click(screen.getByTestId('incarnate-shortcut-hero-cc'));
    const secondAlice = await waitForNewAgentChip(before);
    expect(chip(firstAlice)).toHaveTextContent('CC');
    expect(chip(secondAlice)).toHaveTextContent('CC II');
  });

  it('keeps Tavern accessible from the top bar with six default provider heroes', async () => {
    const storage = new Map<string, string>([
      ['kota-v2.dev.project-root', '/tmp/kota-test'],
      ['kota-v2.tavern.hero-profiles', JSON.stringify({
        alice: { name: 'Alice' },
        bob: { name: 'Bob' },
        david: { name: 'David' },
        charlie: { name: 'Charlie' },
      })],
      ['kota-v2.tavern.custom-heroes', JSON.stringify([
        { id: 'custom-old', kind: 'custom', provider: 'codex', name: 'Glass Scribe' },
      ])],
    ]);
    const originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: vi.fn((key: string) => storage.get(key) ?? null),
        setItem: vi.fn((key: string, value: string) => storage.set(key, String(value))),
        removeItem: vi.fn((key: string) => storage.delete(key)),
      },
    });
    try {
      const { container } = render(<App />);
      await userEvent.click(screen.getByTestId('tavern-btn'));
      await screen.findByTestId('tavern-page');
      expect(container.querySelectorAll('.tavern-hero-card:not(.add)')).toHaveLength(6);
      expect(screen.getByTestId('tavern-hero-hero-cc')).toHaveTextContent('CC');
      expect(screen.getByTestId('tavern-hero-hero-dex')).toHaveTextContent('Dex');
      expect(screen.getByTestId('tavern-hero-hero-gem')).toHaveTextContent('Gem');
      expect(screen.getByTestId('tavern-hero-hero-op')).toHaveTextContent('Op');
      expect(screen.getByTestId('tavern-hero-hero-pi')).toHaveTextContent('Pi');
      expect(screen.getByTestId('tavern-hero-hero-kimi')).toHaveTextContent('Kimi');
      expect(screen.queryByTestId('tavern-hero-david')).not.toBeInTheDocument();
      expect(screen.queryByTestId('tavern-hero-charlie')).not.toBeInTheDocument();
      expect(screen.getByText('New Hero')).toBeInTheDocument();
      expect(screen.queryByText('Glass Scribe')).not.toBeInTheDocument();
      const providerStatus = screen.getByRole('region', { name: "Providers' Status" });
      expect(container.querySelector('.tavern-hero-stage')).toContainElement(providerStatus);
      expect(container.querySelector('.tavern-page-head')).not.toContainElement(providerStatus);
      expect(within(providerStatus).getAllByRole('link')).toHaveLength(6);
      expect(within(providerStatus).queryByText('GitHub CLI')).not.toBeInTheDocument();
      expect(providerStatus.querySelectorAll('.tavern-provider-icon img')).toHaveLength(6);
      expect(container.querySelector('.tavern-loading-chip')).not.toBeInTheDocument();
      expect(container.querySelector('.tavern-bottom-loading-log')).not.toBeInTheDocument();
    } finally {
      if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage);
    }
  });

  it('uses the wrapping, independently scrolling editor shell for account rules', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('tavern-btn'));
    await screen.findByTestId('tavern-page');
    await userEvent.click(screen.getByRole('button', { name: 'Rules' }));
    await screen.findByText('Account Rules');
    await userEvent.click(screen.getByRole('button', { name: /Rules For Coding/i }));

    const trigger = await screen.findByLabelText('On-demand trigger');
    expect(trigger.tagName).toBe('TEXTAREA');
    expect(trigger).toHaveAttribute('rows', '2');
    expect(trigger).toHaveAttribute('wrap', 'soft');
    expect(screen.getByTestId('account-rule-editor-scroll')).toBeInTheDocument();
    expect(screen.getByTestId('account-rule-editor-footer')).toBeInTheDocument();
  });

  it('can create and remove a Tavern hero template', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('tavern-btn'));
    await screen.findByTestId('tavern-page');
    await userEvent.click(screen.getByText('New Hero'));
    const dialog = screen.getByRole('dialog', { name: /New Hero profile/ });
    expect(dialog).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Edit Shell' }));
    await userEvent.click(within(dialog).getByRole('button', { name: /Provider/ }));
    expect(within(dialog).getAllByText('BETA')).toHaveLength(4);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Change avatar' }));
    await userEvent.click(within(dialog).getByRole('radio', { name: 'Glass' }));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Change avatar' }));
    expect(within(dialog).getByRole('radio', { name: 'Glass' })).toHaveAttribute('aria-checked', 'true');
    expect(screen.queryByText('Recruit Hero')).not.toBeInTheDocument();
    await userEvent.click(screen.getByText('Remove Hero'));
    expect(screen.getByText('Drifters')).toBeInTheDocument();
    expect(screen.getByText('Call Back')).toBeInTheDocument();
    expect(screen.getByText('Dismiss')).toBeInTheDocument();
  });

  it('lets Magi choose Claude or Codex while showing read-only prompt and commands', async () => {
    const localStorage = withMockLocalStorage({ 'kota-v2.dev.project-root': '/tmp/kota-test' });
    try {
      render(<App />);
      await userEvent.click(screen.getByTestId('tavern-btn'));
      await screen.findByTestId('tavern-page');
      await userEvent.click(screen.getByRole('button', { name: /System Heros/i }));
      await userEvent.click(screen.getByRole('button', { name: /Magi/i }));
      const dialog = screen.getByRole('dialog', { name: /Magi profile/i });

      expect(within(dialog).getByText('Translation command')).toBeInTheDocument();
      expect(within(dialog).getByText(/\$KOTA_HOME\/heroes\/system-magi\/magi-nl-translate\.md/)).toBeInTheDocument();
      const prompt = within(dialog).getByDisplayValue(/You translate natural language/);
      expect(prompt).toHaveAttribute('readonly');

      await userEvent.click(within(dialog).getByRole('radio', { name: 'Codex' }));
      expect(within(dialog).getByDisplayValue(/1\. codex exec/)).toBeInTheDocument();
      expect(within(dialog).getByDisplayValue('codex --dangerously-bypass-approvals-and-sandbox')).toBeInTheDocument();
      expect(JSON.parse(localStorage.storage.get('kota-v2.tavern.system-heroes') ?? '{}').magi.provider).toBe('codex');
    } finally {
      localStorage.restore();
    }
  });

  it('incarnates into the empty round-table seat that opened the picker', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('seat-violet'));
    const before = currentAgentChipIds();
    await userEvent.click(await screen.findByTestId('incarnate-seat-violet-hero-cc'));
    const alice = await waitForNewAgentChip(before);
    expect(within(screen.getByTestId('seat-violet')).getByText('CC')).toBeInTheDocument();
    expect(screen.getByTestId('seat-bob')).toHaveTextContent('Open seat');
    await waitFor(() => expect(winFrame(alice)).toHaveClass('focused'));
  });
});

describe('TerminalGrid · wide-char layout', () => {
  function snapWithRow(cells: { ch: string; attrs?: number }[]): GridSnapshot {
    return {
      cols: cells.length,
      rows: 1,
      cells,
      cursorRow: 0,
      cursorCol: cells.length,
      cursorVisible: true,
    };
  }

  it('pins wide cells to exactly 2*cellWidth and drops their spacer cells', () => {
    // "新版" — two CJK chars, each marked WIDE_CHAR by the backend; the
    // right-hand cell of each pair arrives as `ch: ""` (the alacritty
    // WIDE_CHAR_SPACER). Render path must (a) emit one inline-block span
    // per wide cell with explicit 2*cellWidth, (b) skip the spacer
    // entirely. Otherwise the browser's CJK font fallback drifts the
    // cursor right of the typed text.
    const snap = snapWithRow([
      { ch: '新', attrs: ATTR_WIDE },
      { ch: '' },
      { ch: '版', attrs: ATTR_WIDE },
      { ch: '' },
    ]);
    const { container } = render(
      <TerminalGrid snapshot={snap} cellWidth={10} fontSize={12} />,
    );
    const spans = container.querySelectorAll('.term-grid > div > span');
    // Exactly one span per wide cell — spacers contribute nothing.
    expect(spans).toHaveLength(2);
    expect(spans[0]?.textContent).toBe('新');
    expect(spans[1]?.textContent).toBe('版');
    expect((spans[0] as HTMLElement).style.display).toBe('inline-block');
    expect((spans[0] as HTMLElement).style.width).toBe('20px');
    expect((spans[1] as HTMLElement).style.width).toBe('20px');
  });

  it('coalesces ASCII runs into a single span', () => {
    const snap = snapWithRow([
      { ch: 'h' },
      { ch: 'e' },
      { ch: 'l' },
      { ch: 'l' },
      { ch: 'o' },
    ]);
    const { container } = render(
      <TerminalGrid snapshot={snap} cellWidth={10} fontSize={12} />,
    );
    const spans = container.querySelectorAll('.term-grid > div > span');
    expect(spans).toHaveLength(1);
    expect(spans[0]?.textContent).toBe('hello');
  });
});

// ═══════════════════════════════ popups unchanged ═══════════════════════════
describe('M1 · popups', () => {
  it('keeps the parked Hot Memory popup hidden from the default UI', () => {
    render(<App />);
    expect(screen.queryByText(/heat gradient · click a tile/)).not.toBeInTheDocument();
  });

  it('opens the group chat overlay from the room shortcut button', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('group-chat-trigger'));
    expect(screen.getByTestId('group-chat-overlay')).toBeInTheDocument();
  });
});

// ═══════════════════════════════ M2 · salvage ═══════════════════════════════
//  Fire / Magic / Flora hearth + Room / Desk / Center palette picker with
//  translucent-vignette overlays (mix-blend-mode: soft-light), NOT flat bg.
describe('M2 · hearth centerpiece', () => {
  beforeEach(() => {
    // happy-dom's localStorage doesn't implement clear(); wipe the M2 keys explicitly.
    for (const key of [
      'kota-v2.layout-mode',
      'kota-v2.centerpiece',
      'kota-v2.room-color',
      'kota-v2.desk-color',
    ]) {
      try { window.localStorage.removeItem(key); } catch { /* ignore */ }
    }
  });

  it('mounts Fire by default into the hearth slot', () => {
    const { container } = render(<App />);
    const hearth = screen.getByTestId('hearth');
    expect(hearth).toHaveAttribute('data-centerpiece', 'fire');
    // Hearth lives inside the slot that Stage.tsx exposes for mounting.
    const slot = container.querySelector('#hearth-anim-slot');
    expect(slot).toContainElement(hearth);
  });

  it('clicking Magic switches the centerpiece to magic', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('picker-trigger'));
    await userEvent.click(screen.getByTestId('center-magic'));
    // M3.4: hearth crossfades — wait for the old sprite to exit.
    await waitFor(() => {
      const hearths = screen.getAllByTestId('hearth');
      expect(hearths).toHaveLength(1);
      expect(hearths[0]).toHaveAttribute('data-centerpiece', 'magic');
    });
  });

  it('clicking Flora switches the centerpiece to flora', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('picker-trigger'));
    await userEvent.click(screen.getByTestId('center-flora'));
    await waitFor(() => {
      const hearths = screen.getAllByTestId('hearth');
      expect(hearths).toHaveLength(1);
      expect(hearths[0]).toHaveAttribute('data-centerpiece', 'flora');
    });
  });

  it('all three hearth sprites are loaded with distinct background images', async () => {
    render(<App />);
    const fireBg = screen.getByTestId('hearth').style.backgroundImage;
    await userEvent.click(screen.getByTestId('picker-trigger'));
    await userEvent.click(screen.getByTestId('center-magic'));
    await waitFor(() => expect(screen.getAllByTestId('hearth')).toHaveLength(1));
    const magicBg = screen.getByTestId('hearth').style.backgroundImage;
    await userEvent.click(screen.getByTestId('center-flora'));
    await waitFor(() => expect(screen.getAllByTestId('hearth')).toHaveLength(1));
    const floraBg = screen.getByTestId('hearth').style.backgroundImage;
    // All three resolve to different URLs (Vite asset imports).
    expect(fireBg).not.toBe('');
    expect(fireBg).not.toBe(magicBg);
    expect(magicBg).not.toBe(floraBg);
    expect(fireBg).not.toBe(floraBg);
  });

  // Persistence across remount is exercised manually — happy-dom 15's
  // Storage proxy doesn't consistently expose getItem/setItem in the test
  // execution context, so we skip round-trip assertions and trust the
  // thin try/catch localStorage wrapper in App.tsx.
});

describe('M2 · theme picker (Room Light / Desk Light) — translucent vignette', () => {
  beforeEach(() => {
    // happy-dom's localStorage doesn't implement clear(); wipe the M2 keys explicitly.
    for (const key of [
      'kota-v2.layout-mode',
      'kota-v2.centerpiece',
      'kota-v2.room-color',
      'kota-v2.desk-color',
    ]) {
      try { window.localStorage.removeItem(key); } catch { /* ignore */ }
    }
  });

  it('picker is collapsed by default — only the trigger is visible', () => {
    render(<App />);
    expect(screen.getByTestId('picker-trigger')).toBeInTheDocument();
    expect(screen.queryByTestId('picker-popover')).not.toBeInTheDocument();
  });

  it('clicking the trigger opens the popover with Room Light / Desk Light / Center rows', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('picker-trigger'));
    const popover = await screen.findByTestId('picker-popover');
    expect(within(popover).getByText(/Room Light/i)).toBeInTheDocument();
    expect(within(popover).getByText(/Desk Light/i)).toBeInTheDocument();
    expect(within(popover).getByText(/Center/i)).toBeInTheDocument();
    // 5 Room Light swatches + 5 Desk Light swatches = 10 color-dots.
    expect(popover.querySelectorAll('.color-dot').length).toBe(10);
  });

  it('Esc closes the popover', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('picker-trigger'));
    await screen.findByTestId('picker-popover');
    await userEvent.keyboard('{Escape}');
    await waitFor(() => {
      expect(screen.queryByTestId('picker-popover')).not.toBeInTheDocument();
    });
  });

  it('mounts translucent-vignette overlays (not flat bg) for Room and Desk', () => {
    render(<App />);
    // Per UI spec: picker apply = translucent overlay divs, NOT flat bg swap.
    const roomTint = screen.getByTestId('room-tint');
    const deskTint = screen.getByTestId('desk-tint');
    expect(roomTint).toBeInTheDocument();
    expect(deskTint).toBeInTheDocument();
    // Rely on JSDOM returning class names rather than computed styles; the
    // actual `mix-blend-mode: soft-light` lives in canvas.css (jsdom doesn't
    // parse that) but the overlay DOM nodes are the testable contract.
    expect(roomTint).toHaveClass('room-tint');
    expect(deskTint).toHaveClass('desk-tint');
  });

  it('clicking a Room swatch writes the color into --room-tint-color on .stage', async () => {
    const { container } = render(<App />);
    // After UI-fix-room-bg the room tint moved to .stage so the painted
    // area covers the whole room (not just the 1120x660 scene rect).
    const stage = container.querySelector('.stage') as HTMLElement;
    expect(stage.style.getPropertyValue('--room-tint-color')).toBe('#2C2720');
    await userEvent.click(screen.getByTestId('picker-trigger'));
    await userEvent.click(screen.getByTestId('room-swatch-sage'));
    expect(stage.style.getPropertyValue('--room-tint-color')).toBe('#8AAE8E');
    expect(screen.getByTestId('room-swatch-sage')).toHaveClass('active');
  });

  it('clicking a Desk swatch writes the color into --desk-tint-color on .rt-desk', async () => {
    const { container } = render(<App />);
    const desk = container.querySelector('.rt-desk') as HTMLElement;
    // default = Walnut serious light.
    expect(desk.style.getPropertyValue('--desk-tint-color')).toBe('#2A2119');
    await userEvent.click(screen.getByTestId('picker-trigger'));
    await userEvent.click(screen.getByTestId('desk-swatch-copper'));
    expect(desk.style.getPropertyValue('--desk-tint-color')).toBe('#BA8A73');
    expect(screen.getByTestId('desk-swatch-copper')).toHaveClass('active');
  });

  // (See note in 'M2 · hearth centerpiece' about happy-dom Storage —
  // localStorage round-trip assertions live outside this file if added.)
});

// ═════════════════════════════════ P3v2 · agent ribbon ═════════════════════════
describe('P3v2 · agent ribbon', () => {
  it('mounts the ribbon without placeholder working chips', () => {
    render(<App />);
    const ribbon = screen.getByTestId('agent-ribbon');
    expect(ribbon).toBeInTheDocument();
    expect(currentAgentChipIds().size).toBe(0);
    expect(screen.getByTestId('ribbon-add')).toBeInTheDocument();
  });

  it('ribbon starts collapsed — no off-table row visible', () => {
    render(<App />);
    expect(screen.queryByTestId('ribbon-row-off')).not.toBeInTheDocument();
  });

  it('chevron is hidden without off-table placeholder agents', () => {
    render(<App />);
    expect(screen.queryByTestId('ribbon-chevron')).not.toBeInTheDocument();
  });

  it('quick incarnating from the ribbon adds a real on-table chip', async () => {
    render(<App />);
    const alice = await recruitHero('hero-cc');
    expect(chip(alice)).toBeInTheDocument();
  });

  it('clicking a chip invokes onOpenAgent (routes target to that agent)', async () => {
    render(<App />);
    const bob = await recruitHero('hero-dex');
    await userEvent.type(screen.getByTestId('input-field'), 'keep this draft');
    await userEvent.click(chip(bob));
    expect(chip(bob)).toHaveClass('target');
    expect(composerText()).toBe('keep this draft');
  });
});

// ═════════════════════════════════ P3 · app chrome ═════════════════════════════
describe('P3 · workspace file tree', () => {
  async function expandRow(testId: string) {
    const row = await screen.findByTestId(testId);
    await userEvent.click(within(row).getByRole('button', { name: /expand folder/i }));
    await waitFor(() => {
      expect(row).toHaveAttribute('aria-expanded', 'true');
    });
    return row;
  }

  it('renders Project Files and Project Workspace sections with root paths', async () => {
    render(<App />);
    const tree = screen.getByTestId('file-tree');
    expect(tree).toBeInTheDocument();
    expect(within(tree).queryByText(/raw fs/i)).not.toBeInTheDocument();
    expect(screen.getByTestId('tree-tab-projectFiles')).toHaveClass('active');
    expect(within(tree).getAllByText('Project Files').length).toBeGreaterThan(0);
    expect(await screen.findByTestId('tree-row-projectFiles-app-v2')).toBeInTheDocument();
    expect(await screen.findByTestId('tree-row-projectFiles-agent-only.txt')).toHaveTextContent('added');
    expect(screen.getByTestId('tree-root-path')).toHaveAttribute('data-full-path', '/tmp/kota-dev');
    expect(screen.getByTestId('tree-root-path')).toHaveTextContent('/tmp/kota-dev');

    await userEvent.click(screen.getByTestId('tree-tab-projectWorkspace'));
    expect(screen.getByTestId('tree-tab-projectWorkspace')).toHaveClass('active');
    expect(await screen.findByTestId('tree-row-projectWorkspace-workspace.json')).toBeInTheDocument();
    expect(within(tree).getAllByText('Project Workspace').length).toBeGreaterThan(0);
    expect(screen.getByTestId('tree-root-path')).toHaveAttribute('data-full-path', '/tmp/kota-dev/Kota/Workspaces/mock');
    expect(screen.getByTestId('tree-root-path')).toHaveTextContent('/tmp/kota-dev/Kota/Workspaces/mock');
  });

  it('refreshes visible listings when the file tree refresh token changes', async () => {
    let version = 0;
    const listingFor = (request: WorkspaceTreePathRequest): WorkspaceTreeListing => {
      const rootPath = request.rootKind === 'projectFiles' ? '/tmp/repo' : '/tmp/workspace';
      const name = version === 0 ? 'before.txt' : 'after.txt';
      return {
        root: {
          kind: request.rootKind,
          label: request.rootKind,
          absolutePath: rootPath,
          changeOverview: null,
        },
        entries: request.relativePath
          ? []
          : [{
              name,
              path: name,
              absolutePath: `${rootPath}/${name}`,
              kind: 'file',
              isHidden: false,
            }],
      };
    };
    const spy = vi
      .spyOn(ptyClient, 'workspaceListTreePath')
      .mockImplementation(async (request) => listingFor(request));

    const { rerender } = render(
      <FileTree
        projectId="project-1"
        repoName="repo"
        sourceDir="/tmp/repo"
        workspaceDir="/tmp/workspace"
        refreshToken={0}
      />,
    );
    expect(await screen.findByTestId('tree-row-projectFiles-before.txt')).toBeInTheDocument();

    version = 1;
    rerender(
      <FileTree
        projectId="project-1"
        repoName="repo"
        sourceDir="/tmp/repo"
        workspaceDir="/tmp/workspace"
        refreshToken={1}
      />,
    );

    expect(await screen.findByTestId('tree-row-projectFiles-after.txt')).toBeInTheDocument();
    expect(screen.queryByTestId('tree-row-projectFiles-before.txt')).not.toBeInTheDocument();
    expect(spy).toHaveBeenCalledTimes(2);
    spy.mockRestore();
  });

  it('shows a connecting state while the initial file tree listing is pending', () => {
    const spy = vi
      .spyOn(ptyClient, 'workspaceListTreePath')
      .mockImplementation(() => new Promise<WorkspaceTreeListing>(() => {}));
    const view = render(
      <FileTree
        projectId="project-1"
        repoName="repo"
        sourceDir="/tmp/repo"
        workspaceDir="/tmp/workspace"
      />,
    );

    expect(screen.getByTestId('tree-loading')).toHaveTextContent('Connecting...');
    expect(screen.queryByText('Loading files...')).not.toBeInTheDocument();

    view.unmount();
    spy.mockRestore();
  });

  it('shows hidden files by default without a hidden toggle or bottom inspector', async () => {
    render(<App />);
    expect(await screen.findByTestId('tree-row-projectFiles-.git')).toBeInTheDocument();
    expect(screen.getByTestId('tree-row-projectFiles-root')).toBeInTheDocument();
    expect(screen.queryByTestId('tree-hidden-toggle')).not.toBeInTheDocument();
    expect(within(screen.getByTestId('file-tree')).queryByText('Selected path')).not.toBeInTheDocument();

    await userEvent.click(screen.getByTestId('tree-tab-projectWorkspace'));
    expect(await screen.findByTestId('tree-row-projectWorkspace-.agent-workspaces')).toBeInTheDocument();
    expect(screen.getByTestId('tree-row-projectWorkspace-root')).toBeInTheDocument();
  });

  it('expands folders with the chevron while row click only selects', async () => {
    render(<App />);
    const app = await screen.findByTestId('tree-row-projectFiles-app-v2');
    await userEvent.click(app);
    expect(app).toHaveClass('active');
    expect(app).toHaveAttribute('aria-expanded', 'false');

    await userEvent.click(within(app).getByRole('button', { name: /expand folder/i }));
    expect(await screen.findByTestId('tree-row-projectFiles-app-v2/src')).toBeInTheDocument();
    const packageRow = await screen.findByTestId('tree-row-projectFiles-app-v2/package.json');
    expect(packageRow).toHaveTextContent('-7');
    expect(packageRow).toHaveTextContent('+18');
  });

  it('shows symlink/worktree labels and the context menu actions', async () => {
    render(<App />);
    expect(await screen.findByTestId('tree-row-projectFiles-.git')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('tree-tab-projectWorkspace'));
    await expandRow('tree-row-projectWorkspace-.agent-workspaces');
    const agentRow = await expandRow('tree-row-projectWorkspace-.agent-workspaces/alice');
    expect(agentRow).not.toHaveTextContent('Dr. Alice Liddell');
    expect(screen.getByTestId('tree-agent-avatar-.agent-workspaces/alice')).toHaveAttribute('data-agent-name', 'Dr. Alice Liddell');
    expect(screen.queryByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/shared')).not.toBeInTheDocument();
    expect(screen.queryByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/.kota')).not.toBeInTheDocument();
    expect(await screen.findByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/project-memory')).toHaveTextContent('symlink');
    expect(await screen.findByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/project-rules')).toHaveTextContent('symlink');
    await expandRow('tree-row-projectWorkspace-.agent-workspaces/alice/.claude');
    await expandRow('tree-row-projectWorkspace-.agent-workspaces/alice/.claude/skills');
    expect(await screen.findByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/.claude/skills/reviewer-kit'))
      .toHaveAttribute('data-symlink-source-target', '/Users/mock/Library/Application Support/Kota/skills');
    expect(await screen.findByTestId('tree-source-marker')).toHaveTextContent('account skills');
    expect(await screen.findByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/project-files')).toHaveTextContent('worktree');
    await waitFor(() => {
      expect(document.querySelector('.tree-symlink-overlay path')).toBeInTheDocument();
    });

    fireEvent.contextMenu(screen.getByTestId('tree-row-projectWorkspace-.agent-workspaces/alice/project-files'));
    const menu = await screen.findByTestId('tree-context-menu');
    expect(menu).toHaveTextContent('Open Default App');
    expect(menu).toHaveTextContent('Reveal in Finder');
    expect(menu).toHaveTextContent('Copy Full Path');
  });

  it('flashes the matching agent bar chip and table card from workspace avatar hover', async () => {
    const tableSlots = [null, null, null, null, null, null, null, 'alice' as AgentId];
    render(
      <Stage
        sceneKey="conversation"
        liveAgents={new Set<AgentId>()}
        tableSlots={tableSlots}
        shortcutAgentsOrdered={tableSlots}
        targetAgent={null}
        onOpenAgent={vi.fn()}
        centerpiece="fire"
        roomColor="#2b2c2f"
        deskColor="#6d5241"
        roomTheme="classic"
        deskTheme="warm"
        onChangeCenter={vi.fn()}
        onChangeRoom={vi.fn()}
        onChangeDesk={vi.fn()}
        onChangeRoomTheme={vi.fn()}
        onChangeDeskTheme={vi.fn()}
      />,
    );

    const chip = screen.getByTestId('chip-alice');
    const seat = screen.getByTestId('seat-alice');
    expect(chip).not.toHaveClass('file-tree-hover');
    expect(seat).not.toHaveClass('file-tree-hover');

    act(() => emitFileTreeAgentHover('alice' as AgentId, true));
    await waitFor(() => {
      expect(chip).toHaveClass('file-tree-hover');
      expect(seat).toHaveClass('file-tree-hover');
    });

    act(() => emitFileTreeAgentHover('alice' as AgentId, false));
    await waitFor(() => {
      expect(chip).not.toHaveClass('file-tree-hover');
      expect(seat).not.toHaveClass('file-tree-hover');
    });
  });

  it('delays the room restore overlay and reveals real agent progress for a long load', () => {
    vi.useFakeTimers();
    const stageProps = {
      sceneKey: 'conversation',
      liveAgents: new Set<AgentId>(),
      targetAgent: null,
      onOpenAgent: vi.fn(),
      centerpiece: 'fire',
      roomColor: '#2b2c2f',
      deskColor: '#6d5241',
      roomTheme: 'classic',
      deskTheme: 'warm',
      onChangeCenter: vi.fn(),
      onChangeRoom: vi.fn(),
      onChangeDesk: vi.fn(),
      onChangeRoomTheme: vi.fn(),
      onChangeDeskTheme: vi.fn(),
      projectRoot: '/tmp/project-1',
    } satisfies ComponentProps<typeof Stage>;
    const view = render(
      <Stage
        {...stageProps}
        agentsHydrating
        agentHydrationProgress={{ completed: 2, total: 8 }}
      />,
    );

    try {
      expect(screen.queryByTestId('room-restore-overlay')).not.toBeInTheDocument();
      act(() => vi.advanceTimersByTime(499));
      expect(screen.queryByTestId('room-restore-overlay')).not.toBeInTheDocument();

      act(() => vi.advanceTimersByTime(1));
      expect(screen.getByTestId('room-restore-overlay')).toHaveTextContent('Restoring Room...');
      expect(screen.queryByTestId('room-restore-progress')).not.toBeInTheDocument();

      act(() => vi.advanceTimersByTime(2499));
      expect(screen.queryByTestId('room-restore-progress')).not.toBeInTheDocument();
      act(() => vi.advanceTimersByTime(1));
      expect(screen.getByTestId('room-restore-progress')).toHaveTextContent('Restoring agents · 2/8');

      view.rerender(
        <Stage
          {...stageProps}
          agentsHydrating
          agentHydrationProgress={{ completed: 3, total: 8 }}
        />,
      );
      expect(screen.getByTestId('room-restore-progress')).toHaveTextContent('Restoring agents · 3/8');

      view.rerender(
        <Stage
          {...stageProps}
          agentsHydrating={false}
          agentHydrationProgress={{ completed: 8, total: 8 }}
        />,
      );
      expect(screen.queryByTestId('room-restore-overlay')).not.toBeInTheDocument();
    } finally {
      view.unmount();
      vi.useRealTimers();
    }
  });

  it('turns a failed room restore into a non-modal retry bar', async () => {
    const onRetryAgentHydration = vi.fn();
    const { container } = render(
      <Stage
        sceneKey="conversation"
        liveAgents={new Set<AgentId>()}
        targetAgent={null}
        onOpenAgent={vi.fn()}
        centerpiece="fire"
        roomColor="#2b2c2f"
        deskColor="#6d5241"
        roomTheme="classic"
        deskTheme="warm"
        onChangeCenter={vi.fn()}
        onChangeRoom={vi.fn()}
        onChangeDesk={vi.fn()}
        onChangeRoomTheme={vi.fn()}
        onChangeDeskTheme={vi.fn()}
        agentHydrationFailed
        onRetryAgentHydration={onRetryAgentHydration}
      />,
    );

    expect(screen.queryByTestId('room-restore-overlay')).not.toBeInTheDocument();
    expect(screen.getByTestId('room-restore-degraded')).toHaveTextContent('Agent roster unavailable');
    expect(container.querySelector('.stage')).not.toHaveAttribute('aria-busy');
    await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRetryAgentHydration).toHaveBeenCalledTimes(1);
  });

  it('opens the group chat filter from an external agent request', async () => {
    const tableSlots = [null, null, null, null, null, null, null, 'alice' as AgentId];
    render(
      <Stage
        sceneKey="conversation"
        liveAgents={new Set<AgentId>()}
        tableSlots={tableSlots}
        shortcutAgentsOrdered={tableSlots}
        targetAgent="alice"
        chatFilterTargetAgents={['alice']}
        chatFilterOpenRequest={{ agentId: 'alice', nonce: 1 }}
        groupChatOpen
        onOpenAgent={vi.fn()}
        onToggleGroupChat={vi.fn()}
        centerpiece="fire"
        roomColor="#2b2c2f"
        deskColor="#6d5241"
        roomTheme="classic"
        deskTheme="warm"
        onChangeCenter={vi.fn()}
        onChangeRoom={vi.fn()}
        onChangeDesk={vi.fn()}
        onChangeRoomTheme={vi.fn()}
        onChangeDeskTheme={vi.fn()}
      />,
    );

    await waitFor(() => {
      expect(screen.getByTestId('group-chat-overlay')).toHaveClass('chat-filter-active');
    });
    expect(chip('alice')).toHaveClass('chat-filter-target');
  });

  it('shows active work as a heartbeat on the matching agent bar chip', () => {
    const tableSlots = [null, null, null, null, null, null, null, 'alice' as AgentId];
    const { container } = render(
      <Stage
        sceneKey="conversation"
        liveAgents={new Set<AgentId>(['alice'])}
        workingAgents={new Set<AgentId>(['alice'])}
        tableSlots={tableSlots}
        shortcutAgentsOrdered={tableSlots}
        targetAgent={null}
        onOpenAgent={vi.fn()}
        centerpiece="fire"
        roomColor="#2b2c2f"
        deskColor="#6d5241"
        roomTheme="classic"
        deskTheme="warm"
        onChangeCenter={vi.fn()}
        onChangeRoom={vi.fn()}
        onChangeDesk={vi.fn()}
        onChangeRoomTheme={vi.fn()}
        onChangeDeskTheme={vi.fn()}
      />,
    );

    const chip = screen.getByTestId('chip-alice');
    const seat = screen.getByTestId('seat-alice');
    expect(chip).toHaveClass('working');
    expect(chip).toHaveAttribute('data-working', 'true');
    expect(seat).toHaveClass('working');
    expect(seat).toHaveClass('thinking');
    expect(seat).toHaveAttribute('data-working', 'true');
    expect(chip.querySelector('.chip-work-heartbeat')).not.toBeInTheDocument();
    expect(container.querySelector('.ribbon-work-nameplate')).toBeInTheDocument();
  });

  it('opens the agent terminal from the agent bar on double click', () => {
    const tableSlots = [null, null, null, null, null, null, null, 'alice' as AgentId];
    const onOpenAgent = vi.fn();
    const onDblClickAgent = vi.fn();
    render(
      <Stage
        sceneKey="conversation"
        liveAgents={new Set<AgentId>()}
        tableSlots={tableSlots}
        shortcutAgentsOrdered={tableSlots}
        targetAgent={null}
        onOpenAgent={onOpenAgent}
        onDblClickAgent={onDblClickAgent}
        centerpiece="fire"
        roomColor="#2b2c2f"
        deskColor="#6d5241"
        roomTheme="classic"
        deskTheme="warm"
        onChangeCenter={vi.fn()}
        onChangeRoom={vi.fn()}
        onChangeDesk={vi.fn()}
        onChangeRoomTheme={vi.fn()}
        onChangeDeskTheme={vi.fn()}
      />,
    );

    const chip = screen.getByTestId('chip-alice');
    fireEvent.click(chip);
    fireEvent.doubleClick(chip);

    expect(onOpenAgent).toHaveBeenCalledTimes(1);
    expect(onOpenAgent).toHaveBeenCalledWith('alice');
    expect(onDblClickAgent).toHaveBeenCalledTimes(1);
    expect(onDblClickAgent).toHaveBeenCalledWith('alice');
  });

  it('shows an unsupported provider as disabled table and ribbon cards', () => {
    const tableSlots = [null, null, null, null, null, null, null, 'alice' as AgentId];
    const unsupportedAgentProviders = new Map<AgentId, string>([['alice', 'future-cli']]);
    const onOpenAgent = vi.fn();
    const onDblClickAgent = vi.fn();
    const onAgentContextMenu = vi.fn();
    const onCommendAgent = vi.fn();
    const { container } = render(
      <Stage
        sceneKey="conversation"
        liveAgents={new Set<AgentId>()}
        tableSlots={tableSlots}
        shortcutAgentsOrdered={tableSlots}
        targetAgent={null}
        agentMeta={{
          alice: {
            name: 'Alice',
            emoji: '◇',
            role: 'Unsupported provider: future-cli',
            hue: 'var(--brass-bright)',
            unsupportedProvider: 'future-cli',
          },
        }}
        unsupportedAgentProviders={unsupportedAgentProviders}
        onOpenAgent={onOpenAgent}
        onDblClickAgent={onDblClickAgent}
        onAgentContextMenu={onAgentContextMenu}
        onCommendAgent={onCommendAgent}
        centerpiece="fire"
        roomColor="#2b2c2f"
        deskColor="#6d5241"
        roomTheme="classic"
        deskTheme="warm"
        onChangeCenter={vi.fn()}
        onChangeRoom={vi.fn()}
        onChangeDesk={vi.fn()}
        onChangeRoomTheme={vi.fn()}
        onChangeDeskTheme={vi.fn()}
      />,
    );

    const chip = screen.getByTestId('chip-alice');
    const seat = screen.getByTestId('seat-alice');
    expect(chip).toBeDisabled();
    expect(chip).toHaveClass('unsupported');
    expect(chip).toHaveAttribute('data-unsupported-provider', 'future-cli');
    expect(chip).toHaveTextContent('Unsupported');
    expect(seat).toHaveClass('unsupported');
    expect(seat).toHaveAttribute('aria-disabled', 'true');
    expect(seat).toHaveTextContent('Unsupported · future-cli');

    fireEvent.click(chip);
    fireEvent.doubleClick(chip);
    fireEvent.contextMenu(chip);
    fireEvent.click(seat);
    fireEvent.doubleClick(seat);
    fireEvent.contextMenu(seat);

    expect(onOpenAgent).not.toHaveBeenCalled();
    expect(onDblClickAgent).not.toHaveBeenCalled();
    expect(onAgentContextMenu).not.toHaveBeenCalled();
    expect(onCommendAgent).not.toHaveBeenCalled();
    expect(container.querySelector('.agent-commend-button')).not.toBeInTheDocument();
  });

  it('drags a tree path into the composer as an attachment chip', async () => {
    render(<App />);
    await expandRow('tree-row-projectFiles-app-v2');
    await expandRow('tree-row-projectFiles-app-v2/src');
    const row = await screen.findByTestId('tree-row-projectFiles-app-v2/src/App.tsx');
    const dataTransfer = {
      files: [],
      effectAllowed: '',
      dropEffect: '',
      setData: vi.fn(),
      getData: vi.fn(() => ''),
    };
    fireEvent.dragStart(row, { dataTransfer });
    fireEvent.drop(screen.getByTestId('input-field'), { dataTransfer });
    await waitFor(() => {
      expect(screen.getByTestId('ib-attachment-chip')).toHaveTextContent('App.tsx');
    });
  });

  it('drags a tree path onto the composer shell as an attachment chip', async () => {
    const { container } = render(<App />);
    await expandRow('tree-row-projectFiles-app-v2');
    const row = await screen.findByTestId('tree-row-projectFiles-app-v2/package.json');
    const dataTransfer = {
      files: [],
      effectAllowed: '',
      dropEffect: '',
      setData: vi.fn(),
      getData: vi.fn(() => ''),
    };
    fireEvent.dragStart(row, { dataTransfer });
    fireEvent.drop(container.querySelector('.input-bar-wrap') as HTMLElement, { dataTransfer });
    await waitFor(() => {
      expect(screen.getByTestId('ib-attachment-chip')).toHaveTextContent('package.json');
    });
  });
});

// ═════════════════════════════════ ST · Smart Terminal ═════════════════════════
describe('ST · Smart Terminal (mock PTY)', () => {
  beforeEach(() => {
    __resetMockSmartPtyForTests();
    for (const k of [
      'kota-v2.st.expanded',
      'kota-v2.st.height',
    ]) {
      try { window.localStorage.removeItem(k); } catch { /* ignore */ }
    }
  });

  it('mounts collapsed by default with quiet status', () => {
    render(<App />);
    const st = screen.getByTestId('smart-terminal');
    expect(st).toHaveAttribute('data-state', 'collapsed');
    expect(within(st).getByText('quiet')).toBeInTheDocument();
    expect(within(st).getByTestId('st-cwd-chip').textContent).toContain('~');
  });

  it('expands on click (bar → panel)', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    const st = screen.getByTestId('smart-terminal');
    expect(st).toHaveAttribute('data-state', 'expanded');
    expect(screen.getByTestId('st-input')).toBeInTheDocument();
  });

  it('clicking the Smart command row activates NL mode', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    const input = screen.getByTestId('st-input') as HTMLInputElement;
    await userEvent.click(input);
    const row = input.closest('.st-prompt-row');
    expect(row).toHaveAttribute('data-nl-mode', 'typing');
    expect(row).toHaveAttribute('data-focus-mode', 'smart');
    expect(row).toHaveClass('nl');
  });

  it('Enter on Smart command inserts a translated command into the shell line', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    const input = screen.getByTestId('st-input') as HTMLInputElement;
    await userEvent.click(input);
    await userEvent.type(input, 'todo last week');
    await userEvent.keyboard('{Enter}');
    // Thinking state appears briefly.
    await waitFor(() => {
      const row = input.closest('.st-prompt-row');
      expect(row).toHaveAttribute('data-nl-mode', 'thinking');
    });
    // Then the translator resolves and inserts the command into the
    // shell capture; the Smart row clears and focus returns to shell.
    await waitFor(() => {
      const row = input.closest('.st-prompt-row');
      expect(row).toHaveAttribute('data-nl-mode', 'shell');
      expect(row).toHaveAttribute('data-focus-mode', 'shell');
      expect(input.value).toBe('');
    }, { timeout: 1500 });
    await userEvent.keyboard('{Enter}');
    await screen.findByText(/grep -rn "TODO"/);
  });

  it('long multi-step NL auto-hands off to claude --dangerously-skip-permissions', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    const input = screen.getByTestId('st-input') as HTMLInputElement;
    await userEvent.click(input);
    await userEvent.type(input, 'refactor all auth-related files to use the new error type');
    await userEvent.keyboard('{Enter}');
    // Auto-handoff: TUI mode kicks in — the visible smart input swaps
    // for the invisible TUI capture input, and the prompt row carries
    // a `data-tui="claude"` marker. No pre-fill / escape-hint either.
    await waitFor(() => {
      const currentInput = screen.getByTestId('st-input') as HTMLInputElement;
      const row = currentInput.closest('.st-prompt-row');
      expect(row).toHaveAttribute('data-tui', 'claude');
      expect(currentInput.value).toBe('');
      expect(screen.queryByTestId('st-escape-hint')).not.toBeInTheDocument();
    }, { timeout: 1500 });
    await screen.findByText(/~ › claude --dangerously-skip-permissions/);
    expect(screen.getAllByText(/Claude Code/).length).toBeGreaterThan(0);
  });

  it('Magi Codex NLP setting hands off long NL requests to codex', async () => {
    const localStorage = withMockLocalStorage({
      'kota-v2.dev.project-root': '/tmp/kota-test',
      'kota-v2.tavern.system-heroes': JSON.stringify({ magi: { provider: 'codex' } }),
    });
    try {
      render(<App />);
      await userEvent.click(screen.getByTestId('smart-terminal'));
      const input = screen.getByTestId('st-input') as HTMLInputElement;
      await userEvent.click(input);
      await userEvent.type(input, 'refactor all auth-related files to use the new error type');
      await userEvent.keyboard('{Enter}');
      await waitFor(() => {
        const currentInput = screen.getByTestId('st-input') as HTMLInputElement;
        const row = currentInput.closest('.st-prompt-row');
        expect(row).toHaveAttribute('data-tui', 'codex');
        expect(currentInput.value).toBe('');
      }, { timeout: 1500 });
      await screen.findByText(/~ › codex --dangerously-bypass-approvals-and-sandbox/);
      expect(screen.getByText(/Codex CLI/)).toBeInTheDocument();
    } finally {
      localStorage.restore();
    }
  });

  it('Esc on Smart command clears input and returns to shell focus', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    const input = screen.getByTestId('st-input') as HTMLInputElement;
    await userEvent.click(input);
    await userEvent.type(input, 'do a thing');
    await userEvent.keyboard('{Escape}');
    const row = input.closest('.st-prompt-row');
    expect(row).toHaveAttribute('data-nl-mode', 'shell');
    expect(row).toHaveAttribute('data-focus-mode', 'shell');
    expect(input.value).toBe('');
  });

  it('plain shell Enter appends the command to scrollback', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await userEvent.keyboard('git status');
    await userEvent.keyboard('{Enter}');
    // Mock script produces `On branch main` among other lines.
    await screen.findByText(/On branch main/);
    expect((screen.getByTestId('st-input') as HTMLInputElement).value).toBe('');
  });

  it('shell input preserves symbols produced through the text-input path', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    const input = screen.getByTestId('st-shell-capture') as HTMLInputElement;
    fireEvent.keyDown(input, { key: '@', altKey: true });
    fireEvent.input(input, { target: { value: '@' }, data: '@', inputType: 'insertText' });
    fireEvent.keyDown(input, { key: 'Enter' });
    await screen.findByText(/› @/);
  });
});

// ═════════════════════════════════ MT · Smart Terminal multi-tab ═════════════
describe('MT · Smart Terminal multi-tab', () => {
  beforeEach(() => {
    __resetMockSmartPtyForTests();
    for (const k of [
      'kota-v2.st.expanded',
      'kota-v2.st.height',
      'kota-v2.st.tabs',
    ]) {
      try { window.localStorage.removeItem(k); } catch { /* ignore */ }
    }
  });

  it('shows shells chip in collapsed bar after default tab spawns', async () => {
    render(<App />);
    // Default tab spawns asynchronously on mount.
    await waitFor(() => {
      expect(screen.getByTestId('st-shells-chip')).toBeInTheDocument();
    });
    expect(screen.getByTestId('st-shells-chip').textContent).toContain('1');
  });

  it('renders the tab strip with one default tab when expanded', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await screen.findByTestId('st-tabstrip');
    // Plus button always visible.
    expect(screen.getByTestId('st-plus')).toBeInTheDocument();
    expect(screen.getByTestId('st-tabstrip').querySelector('.st-tab-track > .st-plus')).toBe(
      screen.getByTestId('st-plus'),
    );
    expect(screen.getByLabelText('Command K clears the Smart Shell screen')).toHaveTextContent('⌘K');
    expect(screen.getByLabelText('Command K clears the Smart Shell screen')).toHaveTextContent('Clear screen');
    expect(screen.getByText('⌘`')).toBeInTheDocument();
    // Exactly one tab chip after default spawn.
    await waitFor(() => {
      const chips = within(screen.getByTestId('st-tabstrip')).getAllByRole('tab');
      expect(chips.length).toBe(1);
    });
  });

  it('clicking + spawns a second tab and switches focus to it', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await waitFor(() => expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(1));
    await userEvent.click(screen.getByTestId('st-plus'));
    await waitFor(() => expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(2));
    // Second tab should be the active one.
    const chips = within(screen.getByTestId('st-tabstrip')).getAllByRole('tab');
    expect(chips[1]).toHaveAttribute('aria-selected', 'true');
    expect(chips[0]).toHaveAttribute('aria-selected', 'false');
  });

  it('clicking an inactive tab activates it', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await waitFor(() => expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(1));
    await userEvent.click(screen.getByTestId('st-plus'));
    await waitFor(() => expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(2));
    // Click first tab → it becomes active.
    const chips = within(screen.getByTestId('st-tabstrip')).getAllByRole('tab');
    await userEvent.click(chips[0]!);
    expect(chips[0]).toHaveAttribute('aria-selected', 'true');
  });

  it('closing the last tab auto-collapses the panel', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await waitFor(() => expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(1));
    const tab = within(screen.getByTestId('st-tabstrip')).getByRole('tab');
    const closeBtn = within(tab).getByRole('button', { name: /^Close / });
    await userEvent.click(closeBtn);
    await waitFor(() => {
      expect(screen.getByTestId('smart-terminal')).toHaveAttribute('data-state', 'collapsed');
    });
  });

  it('closing a running tab shows the kill-and-close confirm tooltip', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await waitFor(() => expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(1));
    // Run `claude` to flip the tab into a TUI / running state.
    await userEvent.keyboard('claude');
    await userEvent.keyboard('{Enter}');
    // ▶ pill should appear in the head (active tab is now TUI-running).
    await screen.findByTestId('st-tui-pill');
    // Click × on the running tab (scope to the tabstrip so we don't pick
    // up TopBar tab close buttons).
    const tab = within(screen.getByTestId('st-tabstrip')).getByRole('tab');
    const closeBtn = within(tab).getByRole('button', { name: /^Close / });
    await userEvent.click(closeBtn);
    // Confirm tooltip appears instead of immediately closing.
    expect(await screen.findByTestId('st-confirm-tip')).toBeInTheDocument();
    expect(within(screen.getByTestId('st-tabstrip')).getAllByRole('tab')).toHaveLength(1);
  });

  it('TUI capture preserves symbols produced through the text-input path', async () => {
    render(<App />);
    await userEvent.click(screen.getByTestId('smart-terminal'));
    await userEvent.keyboard('claude');
    await userEvent.keyboard('{Enter}');
    await screen.findByTestId('st-tui-pill');

    const input = screen.getByTestId('st-input') as HTMLInputElement;
    fireEvent.keyDown(input, { key: '@', altKey: true });
    fireEvent.input(input, { target: { value: '@' }, data: '@', inputType: 'insertText' });
    fireEvent.keyDown(input, { key: 'Enter' });
    await screen.findByText(/› @/);
  });
});

describe('Project agent hydration recovery', () => {
  function mockWorkspaceBootstrap(
    active: ptyClient.WorkspaceProject,
    projects: readonly ptyClient.WorkspaceProject[] = [active],
  ) {
    const storage = withMockLocalStorage({
      'kota-v2.dev.project-root': '/tmp/kota-test',
    });
    vi.spyOn(ptyClient, 'hasTauriRuntime').mockReturnValue(true);
    vi.spyOn(ptyClient, 'workspaceStatus').mockResolvedValue({ active });
    vi.spyOn(ptyClient, 'listWorkspaceProjects').mockResolvedValue([...projects]);
    vi.spyOn(ptyClient, 'loadProjectAgentLayoutFile').mockResolvedValue(null);
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    return {
      saveLayout: vi.spyOn(ptyClient, 'saveProjectAgentLayoutFile').mockResolvedValue(),
      restoreStorage: storage.restore,
    };
  }

  it('keeps provisional seats, retries suspicious empty results, and never persists them', async () => {
    vi.useFakeTimers();
    const workspace = hydrationWorkspace('suspect-empty', ['agent-alpha']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspace);
    const inspectIdentities = vi.spyOn(ptyClient, 'inspectProjectAgentIdentities')
      .mockResolvedValue({ identities: [], workspaceEntryCount: 0 });
    const loadDetail = vi.spyOn(ptyClient, 'loadProjectAgentDetail')
      .mockRejectedValue(new Error('identity files temporarily unavailable'));
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      await flushHydrationPromises();
      expect(screen.getByTestId('chip-agent-alpha')).toBeInTheDocument();

      await act(async () => { await vi.advanceTimersByTimeAsync(500); });
      expect(screen.getByTestId('room-restore-overlay')).toBeInTheDocument();

      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
      await flushHydrationPromises();

      expect(inspectIdentities).toHaveBeenCalledTimes(4);
      expect(loadDetail).toHaveBeenCalledTimes(4);
      expect(screen.getByTestId('chip-agent-alpha')).toBeInTheDocument();
      expect(screen.queryByTestId('room-restore-overlay')).not.toBeInTheDocument();
      expect(screen.getByTestId('room-restore-degraded')).toHaveTextContent('Agent roster unavailable');
      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout).not.toHaveBeenCalled();

      fireEvent.click(within(screen.getByTestId('room-restore-degraded')).getByRole('button', {
        name: 'Retry',
      }));
      await flushHydrationPromises();
      expect(inspectIdentities).toHaveBeenCalledTimes(5);
      await act(async () => { await vi.advanceTimersByTimeAsync(500); });
      expect(screen.getByTestId('room-restore-overlay')).toBeInTheDocument();
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });

  it('settles a genuinely empty project after one immediate confirmation read', async () => {
    const workspace = hydrationWorkspace('true-empty', []);
    const { restoreStorage } = mockWorkspaceBootstrap(workspace);
    const inspectIdentities = vi.spyOn(ptyClient, 'inspectProjectAgentIdentities')
      .mockResolvedValue({ identities: [], workspaceEntryCount: 0 });
    const loadDetail = vi.spyOn(ptyClient, 'loadProjectAgentDetail');
    const view = render(<App />);

    try {
      await waitFor(() => expect(inspectIdentities).toHaveBeenCalledTimes(2));
      await waitFor(() => {
        expect(view.container.querySelector('.stage')).not.toHaveAttribute('aria-busy');
      });
      expect(loadDetail).not.toHaveBeenCalled();
      expect(screen.queryByTestId('room-restore-degraded')).not.toBeInTheDocument();
    } finally {
      view.unmount();
      restoreStorage();
    }
  });

  it('settles an unknown provider as a visible unsupported card without loading or launching it', async () => {
    const workspace = hydrationWorkspace('future-provider', ['agent-alpha']);
    workspace.agents[0]!.cli = 'future-cli';
    const { restoreStorage } = mockWorkspaceBootstrap(workspace);
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
      identities: [{
        ...hydrationIdentity('agent-alpha', 'Alpha'),
        provider: 'future-cli',
      }],
      workspaceEntryCount: 1,
    });
    const loadDetail = vi.spyOn(ptyClient, 'loadProjectAgentDetail')
      .mockRejectedValue(new Error('unsupported provider should not be loaded'));
    const resolveLaunch = vi.spyOn(ptyClient, 'resolveProjectAgentLaunch')
      .mockRejectedValue(new Error('unsupported provider should not be launched'));
    const view = render(<App />);

    try {
      const chip = await screen.findByTestId('chip-agent-alpha');
      await waitFor(() => {
        expect(view.container.querySelector('.stage')).not.toHaveAttribute('aria-busy');
      });
      expect(chip).toBeDisabled();
      expect(chip).toHaveTextContent('Unsupported');
      expect(chip).toHaveAttribute('data-unsupported-provider', 'future-cli');
      expect(screen.queryByTestId('room-restore-degraded')).not.toBeInTheDocument();
      expect(loadDetail).not.toHaveBeenCalled();

      fireEvent.keyDown(window, { key: '1', metaKey: true });
      expect(resolveLaunch).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      restoreStorage();
    }
  });

  it('offers the existing fresh-session path when a saved session is unavailable', async () => {
    const workspace = hydrationWorkspace('expired-session', ['agent-alpha']);
    workspace.agents[0]!.cli = 'claude';
    const { restoreStorage } = mockWorkspaceBootstrap(workspace);
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
      identities: [{
        ...hydrationIdentity('agent-alpha', 'Alpha'),
        provider: 'claude',
      }],
      workspaceEntryCount: 1,
    });
    const detail = {
      ...hydrationDetail('agent-alpha', workspace.localRoot, 'Alpha'),
      cli: 'claude' as const,
      provider: 'claude',
      sessionId: null,
    };
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockResolvedValue(detail);
    const resolveLaunch = vi.spyOn(ptyClient, 'resolveProjectAgentLaunch')
      .mockResolvedValue({ status: 'sessionUnavailable' });
    const startFresh = vi.spyOn(ptyClient, 'startFreshProjectAgentSession')
      .mockResolvedValue({
        detail,
        request: {
          ...workspace.agents[0]!,
          cli: 'claude',
          sessionId: null,
        },
      });
    const clearSession = vi.spyOn(ptyClient, 'clearProjectAgentSessionMetadata')
      .mockResolvedValue(detail);
    const spawnAgent = vi.spyOn(ptyClient, 'spawnAgentPty');
    const view = render(<App />);

    try {
      await screen.findByTestId('chip-agent-alpha');
      await waitFor(() => {
        expect(view.container.querySelector('.stage')).not.toHaveAttribute('aria-busy');
      });

      fireEvent.keyDown(window, { key: '1', metaKey: true });
      let dialog = await screen.findByRole('dialog', { name: "Session can't resume" });
      expect(within(dialog).getByText(
        'The saved session has expired. Start a new session?',
      )).toBeInTheDocument();
      await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
      expect(startFresh).not.toHaveBeenCalled();
      expect(clearSession).not.toHaveBeenCalled();
      expect(spawnAgent).not.toHaveBeenCalled();

      fireEvent.keyDown(window, { key: '1', metaKey: true });
      dialog = await screen.findByRole('dialog', { name: "Session can't resume" });
      await userEvent.click(within(dialog).getByRole('button', { name: 'OK' }));

      await waitFor(() => {
        expect(startFresh).toHaveBeenCalledWith({
          agentId: 'agent-alpha',
          projectRoot: null,
        });
        expect(spawnAgent).toHaveBeenCalledOnce();
      });
      expect(clearSession).toHaveBeenCalledWith({
        agentId: 'agent-alpha',
        projectRoot: null,
      });
      expect(resolveLaunch).toHaveBeenCalledTimes(2);
    } finally {
      view.unmount();
      restoreStorage();
    }
  });

  it('persists seats only after a non-empty roster and every detail are confirmed', async () => {
    vi.useFakeTimers();
    const workspace = hydrationWorkspace('confirmed-roster', ['agent-alpha']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspace);
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
      identities: [hydrationIdentity('agent-alpha', 'Alpha')],
      workspaceEntryCount: 1,
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockResolvedValue(
      hydrationDetail('agent-alpha', workspace.localRoot, 'Alpha'),
    );
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      await flushHydrationPromises();
      expect(screen.getByTestId('chip-agent-alpha')).toBeInTheDocument();
      expect(saveLayout).not.toHaveBeenCalled();

      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout).toHaveBeenCalledWith(
        workspace.localRoot,
        expect.arrayContaining(['agent-alpha']),
      );
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });

  it('treats unaccounted workspace directories as an incomplete roster', async () => {
    vi.useFakeTimers();
    const workspace = hydrationWorkspace('partial-directory', ['agent-alpha']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspace);
    const inspectIdentities = vi.spyOn(ptyClient, 'inspectProjectAgentIdentities')
      .mockResolvedValue({
        identities: [hydrationIdentity('agent-alpha', 'Alpha')],
        workspaceEntryCount: 2,
      });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockResolvedValue(
      hydrationDetail('agent-alpha', workspace.localRoot, 'Alpha'),
    );
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
      await flushHydrationPromises();

      expect(inspectIdentities).toHaveBeenCalledTimes(4);
      expect(screen.getByTestId('chip-agent-alpha')).toBeInTheDocument();
      expect(screen.getByTestId('room-restore-degraded')).toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });

  it('degrades instead of spinning forever when identity and detail IPCs hang', async () => {
    vi.useFakeTimers();
    const workspace = hydrationWorkspace('hung-hydration', ['agent-alpha']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspace);
    const inspectIdentities = vi.spyOn(ptyClient, 'inspectProjectAgentIdentities')
      .mockImplementation(() => new Promise<ptyClient.ProjectAgentIdentityListing>(() => {}));
    const loadDetail = vi.spyOn(ptyClient, 'loadProjectAgentDetail')
      .mockImplementation(() => new Promise<ptyClient.ProjectAgentDetail>(() => {}));
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      for (const retryDelay of [0, 1000, 2000, 4000]) {
        if (retryDelay > 0) {
          await act(async () => { await vi.advanceTimersByTimeAsync(retryDelay); });
        }
        await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
        await flushHydrationPromises();
        await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
        await flushHydrationPromises();
      }

      expect(inspectIdentities).toHaveBeenCalledTimes(4);
      expect(loadDetail).toHaveBeenCalledTimes(4);
      expect(screen.getByTestId('room-restore-degraded')).toBeInTheDocument();
      expect(saveLayout).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });

  it('does not settle archived identities as empty when a partial directory remains', async () => {
    vi.useFakeTimers();
    const workspace = hydrationWorkspace('archived-plus-partial', []);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspace);
    const archivedIdentity = {
      ...hydrationIdentity('agent-archived', 'Archived'),
      status: 'archived',
    };
    const inspectIdentities = vi.spyOn(ptyClient, 'inspectProjectAgentIdentities')
      .mockResolvedValue({ identities: [archivedIdentity], workspaceEntryCount: 2 });
    const loadDetail = vi.spyOn(ptyClient, 'loadProjectAgentDetail');
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
      await flushHydrationPromises();

      expect(inspectIdentities).toHaveBeenCalledTimes(4);
      expect(loadDetail).not.toHaveBeenCalled();
      expect(screen.getByTestId('room-restore-degraded')).toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });

  it('retains identity cards and blocks persistence when one detail never loads', async () => {
    vi.useFakeTimers();
    const workspace = hydrationWorkspace('partial-detail', ['agent-alpha', 'agent-beta']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspace);
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockResolvedValue({
      identities: [
        hydrationIdentity('agent-alpha', 'Alpha'),
        hydrationIdentity('agent-beta', 'Beta'),
      ],
      workspaceEntryCount: 2,
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation((request) => (
      request.agentId === 'agent-beta'
        ? Promise.reject(new Error('beta detail unavailable'))
        : Promise.resolve(hydrationDetail(
          request.agentId,
          request.projectRoot ?? workspace.localRoot,
          'Alpha',
        ))
    ));
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      await flushHydrationPromises();
      await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
      await flushHydrationPromises();

      expect(screen.getByTestId('chip-agent-alpha')).toBeInTheDocument();
      expect(screen.getByTestId('chip-agent-beta')).toHaveTextContent('Beta');
      expect(screen.getByTestId('seat-bob')).toHaveTextContent('Beta');
      expect(screen.getByTestId('room-restore-degraded')).toBeInTheDocument();
      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout).not.toHaveBeenCalled();
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });

  it('keeps the active workspace reference when a cached refresh has identical agent data', async () => {
    const workspaceA = hydrationWorkspace('project-a', ['agent-alpha']);
    const workspaceB = hydrationWorkspace('project-b', ['agent-beta']);
    const { restoreStorage } = mockWorkspaceBootstrap(workspaceA, [workspaceA, workspaceB]);
    const byRoot = new Map([
      [workspaceA.localRoot, [hydrationIdentity('agent-alpha', 'Alpha')]],
      [workspaceB.localRoot, [hydrationIdentity('agent-beta', 'Beta')]],
    ]);
    const inspectIdentities = vi.spyOn(ptyClient, 'inspectProjectAgentIdentities')
      .mockImplementation(async (projectRoot) => ({
        identities: byRoot.get(projectRoot ?? '') ?? [],
        workspaceEntryCount: byRoot.get(projectRoot ?? '')?.length ?? 0,
      }));
    const freshWorkspaceB: ptyClient.WorkspaceProject = {
      ...workspaceB,
      agents: workspaceB.agents.map((agent) => ({
        ...agent,
        args: [...(agent.args ?? [])],
      })),
    };
    vi.spyOn(ptyClient, 'openWorkspaceProject').mockResolvedValue(freshWorkspaceB);
    const view = render(<App />);

    try {
      await screen.findByTestId('chip-agent-alpha');
      await userEvent.click(await screen.findByTestId('tab-project-b'));
      await screen.findByTestId('chip-agent-beta');
      await flushHydrationPromises();

      const projectBIdentityReads = inspectIdentities.mock.calls.filter(
        ([projectRoot]) => projectRoot === workspaceB.localRoot,
      );
      expect(projectBIdentityReads).toHaveLength(1);
    } finally {
      view.unmount();
      restoreStorage();
    }
  });

  it('cancels a stale generation before its late detail can restore or persist the old room', async () => {
    const workspaceA = hydrationWorkspace('cancel-project-a', ['agent-alpha']);
    const workspaceB = hydrationWorkspace('cancel-project-b', ['agent-beta']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(workspaceA, [workspaceA, workspaceB]);
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockImplementation(async (projectRoot) => ({
      identities: projectRoot === workspaceA.localRoot
        ? [hydrationIdentity('agent-alpha', 'Alpha')]
        : [hydrationIdentity('agent-beta', 'Beta')],
      workspaceEntryCount: 1,
    }));
    let resolveAlphaDetail!: (detail: ptyClient.ProjectAgentDetail) => void;
    const alphaDetail = new Promise<ptyClient.ProjectAgentDetail>((resolve) => {
      resolveAlphaDetail = resolve;
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation((request) => (
      request.agentId === 'agent-alpha'
        ? alphaDetail
        : Promise.resolve(hydrationDetail('agent-beta', workspaceB.localRoot, 'Beta'))
    ));
    vi.spyOn(ptyClient, 'openWorkspaceProject').mockResolvedValue(workspaceB);
    const view = render(<App />);

    try {
      await waitFor(() => {
        expect(ptyClient.loadProjectAgentDetail).toHaveBeenCalledWith({
          agentId: 'agent-alpha',
          projectRoot: workspaceA.localRoot,
        });
      });
      await userEvent.click(await screen.findByTestId('tab-cancel-project-b'));
      await screen.findByTestId('chip-agent-beta');

      await act(async () => {
        resolveAlphaDetail(hydrationDetail('agent-alpha', workspaceA.localRoot, 'Alpha'));
        await Promise.resolve();
      });
      await waitFor(() => {
        expect(saveLayout.mock.calls.some(([root]) => root === workspaceB.localRoot)).toBe(true);
      });
      expect(screen.queryByTestId('chip-agent-alpha')).not.toBeInTheDocument();
      expect(saveLayout.mock.calls.some(([root]) => root === workspaceA.localRoot)).toBe(false);
    } finally {
      view.unmount();
      restoreStorage();
    }
  });

  it('waits for a late cached refresh before an empty roster can be persisted', async () => {
    vi.useFakeTimers();
    const workspaceA = hydrationWorkspace('fresh-project-a', ['agent-alpha']);
    const cachedWorkspaceB = hydrationWorkspace('fresh-project-b', []);
    const freshWorkspaceB = hydrationWorkspace('fresh-project-b', ['agent-beta']);
    const { saveLayout, restoreStorage } = mockWorkspaceBootstrap(
      workspaceA,
      [workspaceA, cachedWorkspaceB],
    );
    vi.spyOn(ptyClient, 'inspectProjectAgentIdentities').mockImplementation(async (projectRoot) => {
      if (projectRoot === workspaceA.localRoot) {
        return { identities: [hydrationIdentity('agent-alpha', 'Alpha')], workspaceEntryCount: 1 };
      }
      return { identities: [hydrationIdentity('agent-beta', 'Beta')], workspaceEntryCount: 1 };
    });
    vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockImplementation(async (request) => (
      hydrationDetail(
        request.agentId,
        request.projectRoot ?? workspaceA.localRoot,
        request.agentId === 'agent-beta' ? 'Beta' : 'Alpha',
      )
    ));
    let resolveFreshWorkspace!: (workspace: ptyClient.WorkspaceProject) => void;
    const freshWorkspace = new Promise<ptyClient.WorkspaceProject>((resolve) => {
      resolveFreshWorkspace = resolve;
    });
    vi.spyOn(ptyClient, 'openWorkspaceProject').mockReturnValue(freshWorkspace);
    const view = render(<App />);

    try {
      await flushHydrationPromises();
      fireEvent.click(screen.getByTestId('tab-fresh-project-b'));
      await flushHydrationPromises();

      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout.mock.calls.some(([root]) => root === cachedWorkspaceB.localRoot)).toBe(false);

      await act(async () => {
        resolveFreshWorkspace(freshWorkspaceB);
        await Promise.resolve();
        await Promise.resolve();
      });
      await flushHydrationPromises();
      expect(screen.getByTestId('chip-agent-beta')).toBeInTheDocument();

      await act(async () => { await vi.advanceTimersByTimeAsync(800); });
      expect(saveLayout).toHaveBeenCalledWith(
        freshWorkspaceB.localRoot,
        expect.arrayContaining(['agent-beta']),
      );
    } finally {
      view.unmount();
      restoreStorage();
      vi.useRealTimers();
    }
  });
});
