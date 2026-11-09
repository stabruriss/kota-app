import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TavernModal } from '../src/chrome/TavernModal';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import { VioletMessageBubble } from '../src/chrome/VioletRoomPanel';
import { AgentRibbon } from '../src/chrome/AgentRibbon';
import { AgentWindowsLayer } from '../src/chrome/AgentWindowsLayer';
import { createAgentGridStore } from '../src/lib/agent-grid-store';
import iconLocal from '../src/assets/tavern/icons/tavern-entry.svg';
import iconPi from '../src/assets/tavern/icons/providers/pi.svg';
import * as pty from '../src/pty-client';
import * as local from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';

vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(),
  KOTA_LOCAL_ENABLED: true,
  discoverLocal: vi.fn(),
}));

const previousStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
beforeEach(async () => {
  const values = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  const statuses = await pty.supportedShellsStatus();
  vi.spyOn(pty, 'supportedShellsStatus').mockResolvedValue(statuses.map((entry) => ({ ...entry, installed: true })));
  vi.spyOn(pty, 'loadTavernHeroProfiles').mockResolvedValue([]);
  vi.spyOn(pty, 'saveTavernHeroProfiles').mockResolvedValue();
  vi.spyOn(pty, 'listAccountSkills').mockResolvedValue([]);
  vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({
    schema: 1, summary: 'ready', providers: [{ providerId: 'openai', status: 'signed_in', authType: 'api_key' }],
  });
  vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [] });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  if (previousStorage) Object.defineProperty(window, 'localStorage', previousStorage);
  else Reflect.deleteProperty(window, 'localStorage');
});

it('uses the existing Tavern flame in Local status and SHELL choices while keeping the Pi icon', async () => {
  render(<TavernModal open onClose={() => {}} />);
  const status = within(screen.getByRole('region', { name: "Providers' Status" }));
  await waitFor(() => expect(status.getByRole('link', { name: /^Pi\b/ })).toHaveClass('ready'));
  const localStatus = status.getByRole('status', { name: 'Kota Local availability' });
  await waitFor(() => expect(localStatus).toHaveTextContent('Ready'));
  expect(localStatus.querySelector('img')).toHaveAttribute('src', iconLocal);
  expect(status.getByRole('link', { name: /^Pi\b/ }).querySelector('img')).toHaveAttribute('src', iconPi);

  await userEvent.click(screen.getByRole('button', { name: 'New Hero create template' }));
  const profile = within(screen.getByRole('dialog', { name: 'New Hero profile' }));
  await userEvent.click(profile.getByRole('button', { name: 'Edit Shell' }));
  await userEvent.click(profile.getByRole('button', { name: /^Provider / }));
  const localOption = profile.getByRole('option', { name: /^Kota Local\b/ });
  const pi = profile.getByRole('option', { name: /^Pi\b/ });
  expect(localOption).toBeEnabled();
  expect(localOption.querySelector('.project-agent-shell-provider-icon')).toHaveClass('provider-kota-local');
  expect(pi.querySelector('.project-agent-shell-provider-icon')).toHaveClass('provider-pi');
  await userEvent.click(localOption);
  expect(profile.getByRole('button', { name: 'Provider Kota Local' })).toHaveClass('provider-kota-local');
  await userEvent.click(profile.getByRole('button', { name: 'Provider Kota Local' }));
  await userEvent.click(profile.getByRole('option', { name: /^Pi\b/ }));
  expect(profile.getByRole('button', { name: 'Provider Pi' })).toHaveClass('provider-pi');
  expect(profile.getByRole('button', { name: 'Provider Pi' })).not.toHaveClass('provider-kota-local');
  expect(local.discoverLocal).not.toHaveBeenCalled();
});

it('renders the dedicated Kota Local default avatar on Tavern hero cards while keeping Pi on Pi cards', async () => {
  vi.spyOn(pty, 'loadTavernHeroProfiles').mockResolvedValue([
    {
      heroId: 'hero-local-fixture',
      name: 'New Hero II',
      provider: 'kota-local',
      model: '',
      effort: null,
      avatarId: 'pi',
      skills: [],
      ghost: 'Fixture',
      shell: '',
      archived: false,
      dismissed: false,
      kind: 'custom',
    },
    {
      heroId: 'hero-pi-fixture',
      name: 'Pi Fixture',
      provider: 'pi',
      model: 'default',
      effort: null,
      avatarId: 'pi',
      skills: [],
      ghost: 'Fixture',
      shell: '',
      archived: false,
      dismissed: false,
      kind: 'custom',
    },
  ]);
  render(<TavernModal open onClose={() => {}} />);
  const localCard = await screen.findByRole('button', { name: /New Hero II/i });
  const piCard = await screen.findByRole('button', { name: /Pi Fixture/i });
  expect(localCard.querySelector('.tavern-avatar-art')).toHaveClass('provider-kota-local');
  expect(localCard.querySelector('.tavern-avatar-art')).not.toHaveClass('provider-pi');
  expect(piCard.querySelector('.tavern-avatar-art')).toHaveClass('provider-pi');
  expect(piCard.querySelector('.tavern-avatar-art')).not.toHaveClass('provider-kota-local');
});

it.each([
  ['kota-local', 'Kota Local', 'provider-kota-local', 'provider-pi'],
  ['pi', 'Pi', 'provider-pi', 'provider-kota-local'],
] as const)('keeps the correct SHELL icon for %s in project agent detail', async (provider, label, className, otherClass) => {
  const detail: pty.ProjectAgentDetail = {
    agentId: 'agent-fixture', displayName: 'Fixture', sourceHeroId: 'hero-fixture', sourceHeroName: 'Fixture',
    projectId: 'project-fixture', projectName: 'Fixture', cli: provider, provider,
    model: 'default', skills: [], args: [], ghost: '', status: 'active',
    adapterPath: '/fixture/AGENTS.md', shellPath: '/fixture/SHELL.yaml', agentYamlPath: '/fixture/agent.yaml',
    inviteEligibility: { eligible: false, reason: 'Fixture', proposedHeroId: 'hero-fixture', proposedDisplayName: 'Fixture' },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, forkable: false, dirty: false, dirtySummary: '',
  };
  vi.spyOn(pty, 'loadProjectAgentDetail').mockResolvedValue(detail);
  render(<ProjectAgentProfileOverlay agentId="agent-fixture" projectRoot="/fixture/project" existingNames={[]}
    onClose={vi.fn()} onSaved={vi.fn()} onRemoveFromProject={vi.fn()} />);
  const badge = await screen.findByLabelText(`Provider ${label}`);
  expect(badge).toHaveClass(className);
  expect(badge).not.toHaveClass(otherClass);
});

it('preserves the Tavern SVG colors instead of masking it, without changing the Pi mask', () => {
  const css = readFileSync(resolve(__dirname, '../src/styles/canvas.css'), 'utf8');
  const local = css.match(/\.project-agent-shell-provider\.provider-kota-local [^{]+\{([^}]+)\}/)?.[1];
  expect(local).toContain('background: url("../assets/tavern/icons/tavern-entry.svg") center / contain no-repeat;');
  expect(local).toContain('mask: none;');
  expect(local).toContain('-webkit-mask: none;');
  const pi = css.match(/\.project-agent-shell-provider\.provider-pi,\s*\.project-agent-shell-provider-icon\.provider-pi\s*\{([^}]+)\}/)?.[1];
  expect(pi).toContain('--project-agent-shell-provider-icon: url("../assets/tavern/icons/providers/pi.svg");');
  const localAvatar = css.match(/\.tavern-avatar-art\.provider-kota-local\s*\{([^}]+)\}/)?.[1];
  expect(localAvatar).toContain("background-image: url('../assets/tavern/optimized/avatars/kota-local.webp');");
  expect(css).toContain(".tavern-avatar-art.provider-pi {\n  background-image: url('../assets/tavern/optimized/avatars/pi.webp');\n}");
});

it.each([
  ['kota-local', 'Kota Local', iconLocal, 'pi'],
  ['pi', 'Pi', iconPi, 'kota-local'],
] as const)('uses the actual %s turn icon and entry caption, not the current agent provider', (shell, label, icon, agentProvider) => {
  const message: pty.VioletChatMessage = {
    id: `fixture-${shell}-reply`, sessionId: 'fixture-session', agentId: 'agent-fixture',
    shell, agentProvider, role: 'assistant', kind: 'message', timestamp: '2026-10-02T00:00:00.000Z',
    text: 'Finished the fixture check.', model: 'k3', effort: 'max',
    sourcePath: null, nativeEventId: 'fixture-session/fixture-entry:1',
  };
  render(<VioletMessageBubble message={message} humanTargetName="Human Fixture" />);
  const badge = screen.getByLabelText(label);
  expect(badge).toHaveAttribute('data-provider', shell);
  expect(badge.querySelector('img')).toHaveAttribute('src', icon);
  expect(badge.querySelector('img')).toHaveAttribute('width', '10');
  expect(badge.querySelector('img')).toHaveAttribute('height', '10');
  expect(badge.parentElement).toHaveClass('violet-msg-model');
  expect(badge.parentElement).toHaveTextContent('k3 • max');
  const tooltip = within(badge).getByRole('tooltip', { hidden: true });
  expect(tooltip).toHaveTextContent(label);
  expect(badge).toHaveAttribute('aria-describedby', tooltip.id);
});

it.each([
  ['user', 'message'],
  ['assistant', 'commentary'],
] as const)('does not add a Local provider caption to a %s %s bubble', (role, kind) => {
  render(<VioletMessageBubble humanTargetName="Human Fixture" message={{
    id: 'fixture-non-end-turn', sessionId: 'fixture-session', agentId: 'agent-fixture',
    shell: 'kota-local', role, kind, timestamp: '2026-10-02T00:00:00.000Z',
    text: 'Fixture progress.', model: 'k3', effort: 'max', sourcePath: null, nativeEventId: null,
  }} />);
  expect(screen.queryByLabelText('Kota Local')).not.toBeInTheDocument();
});

it.each([
  ['kota-local', 'provider-kota-local', 'provider-pi'],
  ['pi', 'provider-pi', 'provider-kota-local'],
] as const)('uses %s default avatar class in room bubbles without colliding with the other provider', (provider, expectedClass, otherClass) => {
  const { container } = render(<VioletMessageBubble humanTargetName="Human Fixture" message={{
    id: `fixture-avatar-${provider}`, sessionId: 'fixture-session', agentId: `agent-${provider}`,
    shell: provider, agentProvider: provider, role: 'assistant', kind: 'message',
    timestamp: '2026-11-06T00:00:00.000Z', text: 'Avatar check.', model: 'k3', effort: 'max',
    sourcePath: null, nativeEventId: 'fixture-session/fixture-entry:2',
  }} />);
  const avatar = container.querySelector('.violet-msg-avatar.tavern-avatar-art');
  expect(avatar).toHaveClass(expectedClass);
  expect(avatar).not.toHaveClass(otherClass);
});

it('uses provider-kota-local in AgentRibbon and AgentWindowsLayer fallbacks without changing provider-pi', () => {
  const ribbon = render(
    <AgentRibbon
      onTable={[
        { id: 'agent-local', state: 'idle' },
        { id: 'agent-pi', state: 'idle' },
      ]}
      offTable={[]}
      targetAgent={null}
      onOpenAgent={vi.fn()}
      workingHeroes={[
        { id: 'agent-local', name: 'Local Hero', record: '1 turn', cli: 'kota-local' },
        { id: 'agent-pi', name: 'Pi Hero', record: '1 turn', cli: 'pi' },
      ]}
      shortcutOrder={['agent-local', 'agent-pi']}
    />,
  );
  const chipAvatars = ribbon.container.querySelectorAll('.chip-avatar.tavern-avatar-art');
  expect(chipAvatars[0]).toHaveClass('provider-kota-local');
  expect(chipAvatars[0]).not.toHaveClass('provider-pi');
  expect(chipAvatars[1]).toHaveClass('provider-pi');
  expect(chipAvatars[1]).not.toHaveClass('provider-kota-local');
  cleanup();

  const status = new Map([
    ['agent-local', { agentId: 'agent-local', cli: 'kota-local' as const, cwd: '/tmp', running: true }],
    ['agent-pi', { agentId: 'agent-pi', cli: 'pi' as const, cwd: '/tmp', running: true }],
  ]);
  const windows = render(
    <AgentWindowsLayer
      liveAgents={['agent-local', 'agent-pi']}
      gridStore={createAgentGridStore()}
      status={status}
      focusedAgent="agent-local"
      minimized={new Set()}
      onFocusAgent={vi.fn()}
      onMinimizeAgent={vi.fn()}
      onAgentKey={vi.fn()}
      projectId="project-fixture"
    />,
  );
  const winAvatars = windows.container.querySelectorAll('.win-title-agent-avatar.tavern-avatar-art');
  expect(winAvatars[0]).toHaveClass('provider-kota-local');
  expect(winAvatars[0]).not.toHaveClass('provider-pi');
  expect(winAvatars[1]).toHaveClass('provider-pi');
  expect(winAvatars[1]).not.toHaveClass('provider-kota-local');
});
