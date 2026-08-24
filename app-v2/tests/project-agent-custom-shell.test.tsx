import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import * as ptyClient from '../src/pty-client';

const LEGACY_MODEL_CACHE_KEY = 'kota-v2.project-agent.model-catalog-cache';

function projectDetail(model = 'provider-model'): ptyClient.ProjectAgentDetail {
  return {
    agentId: 'agent-custom',
    displayName: 'Custom Agent',
    sourceHeroId: 'hero-dex',
    sourceHeroName: 'Dex',
    projectId: 'custom-project',
    projectName: 'custom-project',
    cli: 'codex',
    provider: 'codex',
    model,
    effort: 'high',
    avatarId: null,
    skills: [],
    args: ['--model', model],
    ghost: '',
    adapterPath: '/tmp/custom/AGENTS.md',
    shellPath: '/tmp/custom/SHELL.yaml',
    agentYamlPath: '/tmp/custom/agent.yaml',
    status: 'active',
    inviteEligibility: {
      eligible: false,
      reason: 'fixture',
      proposedHeroId: 'hero-dex',
      proposedDisplayName: 'Custom Agent',
    },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 },
    forkable: true,
    dirty: false,
    dirtySummary: '',
  };
}

function codexStatus(): ptyClient.SupportedShellStatus {
  return {
    id: 'codex',
    name: 'Codex',
    bin: 'codex',
    installed: true,
    resolvedBin: '/usr/local/bin/codex',
    installUrl: 'https://example.test/codex',
    summary: 'fixture',
    modelOptions: [{ id: 'provider-model', label: 'Provider model', source: 'provider' }],
    effortOptions: [{ value: 'high', label: 'High' }],
  };
}

function renderOverlay() {
  return render(
    <ProjectAgentProfileOverlay
      agentId="agent-custom"
      projectRoot="/tmp/custom-project"
      existingNames={[]}
      onClose={vi.fn()}
      onSaved={vi.fn()}
      onRemoveFromProject={vi.fn()}
    />,
  );
}

function mockDetail(detail = projectDetail()) {
  vi.spyOn(ptyClient, 'loadProjectAgentDetail').mockResolvedValue(detail);
  vi.spyOn(ptyClient, 'listAccountSkills').mockResolvedValue([]);
  vi.spyOn(ptyClient, 'supportedShellsStatus').mockResolvedValue([codexStatus()]);
  return vi.spyOn(ptyClient, 'saveProjectAgentDetail')
    .mockImplementation(async (request) => ({
      ...detail,
      model: request.model,
      effort: request.effort,
      avatarId: request.avatarId,
    }));
}

describe('Project agent custom SHELL values', () => {
  it('saves exact unmatched model and effort values only on Save SHELL', async () => {
    const saveShell = mockDetail();

    try {
      renderOverlay();
      const shell = await screen.findByRole('region', { name: 'SHELL' });
      await userEvent.click(within(shell).getByRole('button', { name: 'Edit SHELL' }));
      const [modelInput, effortInput] = within(shell).getAllByRole('combobox');

      await userEvent.clear(modelInput);
      await userEvent.type(modelInput, 'private-model');
      await userEvent.keyboard('{Enter}');
      await userEvent.clear(effortInput);
      await userEvent.type(effortInput, 'extreme');
      await userEvent.keyboard('{Enter}');

      expect(saveShell).not.toHaveBeenCalled();
      await userEvent.click(within(shell).getByRole('button', { name: 'Save SHELL' }));

      await waitFor(() => expect(saveShell).toHaveBeenCalledTimes(1));
      expect(saveShell.mock.calls[0][0]).toMatchObject({
        model: 'private-model',
        effort: 'extreme',
      });
    } finally {
      vi.restoreAllMocks();
    }
  });

  it('keeps staged custom text out of unrelated profile saves', async () => {
    const saveShell = mockDetail();

    try {
      renderOverlay();
      const shell = await screen.findByRole('region', { name: 'SHELL' });
      await userEvent.click(within(shell).getByRole('button', { name: 'Edit SHELL' }));
      const modelInput = within(shell).getAllByRole('combobox')[0];
      await userEvent.clear(modelInput);
      await userEvent.type(modelInput, 'half-written-model');

      await userEvent.click(screen.getByRole('button', { name: 'Change avatar' }));
      const avatars = screen.getAllByRole('radio');
      await userEvent.click(avatars.find((option) => option.getAttribute('aria-checked') !== 'true')!);

      await waitFor(() => expect(saveShell).toHaveBeenCalledTimes(1));
      expect(saveShell.mock.calls[0][0].model).toBe('provider-model');
      expect(modelInput).toHaveValue('half-written-model');

      await userEvent.click(within(shell).getByRole('button', { name: 'Save SHELL' }));
      await waitFor(() => expect(saveShell).toHaveBeenCalledTimes(2));
      expect(saveShell.mock.calls[1][0].model).toBe('half-written-model');
    } finally {
      vi.restoreAllMocks();
    }
  });

  it('ignores the pre-rollback project model cache', async () => {
    const storage = new Map<string, string>([[LEGACY_MODEL_CACHE_KEY, JSON.stringify({
      codex: {
        updatedAt: 1,
        models: [{ id: 'stale-probe-model', label: 'stale-probe-model', source: 'acp' }],
      },
    })]]);
    const originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
    Object.defineProperty(window, 'localStorage', {
      configurable: true,
      value: {
        getItem: vi.fn((key: string) => storage.get(key) ?? null),
        setItem: vi.fn((key: string, value: string) => storage.set(key, String(value))),
        removeItem: vi.fn((key: string) => storage.delete(key)),
      },
    });
    mockDetail();

    try {
      renderOverlay();
      const shell = await screen.findByRole('region', { name: 'SHELL' });
      await userEvent.click(within(shell).getByRole('button', { name: 'Edit SHELL' }));
      const modelInput = within(shell).getAllByRole('combobox')[0];
      await userEvent.clear(modelInput);

      expect(within(shell).queryByRole('option', { name: /stale-probe-model/i })).toBeNull();
    } finally {
      vi.restoreAllMocks();
      if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage);
    }
  });
});
