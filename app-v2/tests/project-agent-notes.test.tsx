import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import * as client from '../src/pty-client';

const projectRoot = '/fixture/project';
function setup(exists = true) {
  const detail: client.ProjectAgentDetail = {
    agentId: 'agent-piner', displayName: '颦儿 v. Kota', sourceHeroId: 'source-hero', sourceHeroName: 'Hero',
    projectId: 'project-kota', projectName: 'Kota', cli: 'codex', provider: 'codex', model: 'default',
    skills: [], args: [], ghost: 'Original GHOST', status: 'archived',
    adapterPath: '/fixture/AGENTS.md', shellPath: '/fixture/SHELL.yaml', agentYamlPath: '/fixture/agent.yaml',
    notes: { exists, path: `${projectRoot}/.agent-workspaces/agent-piner/under-table/soul/marks.json` },
    inviteEligibility: { eligible: false, proposedHeroId: 'h', proposedDisplayName: '颦儿' },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, forkable: false, dirty: false, dirtySummary: '',
  };
  vi.spyOn(client, 'loadProjectAgentDetail').mockResolvedValue(detail);
  vi.spyOn(client, 'listAccountSkills').mockResolvedValue([]);
  vi.spyOn(client, 'supportedShellsStatus').mockResolvedValue([]);
  const save = vi.spyOn(client, 'saveProjectAgentDetail');
  const open = vi.spyOn(client, 'openProjectAgentNotes').mockResolvedValue();
  const close = vi.fn();
  const view = render(<ProjectAgentProfileOverlay agentId="agent-piner" projectRoot={projectRoot}
    existingNames={[]} onClose={close} onSaved={vi.fn()} onRemoveFromProject={vi.fn()} />);
  return { ...view, open, save, close };
}
afterEach(() => vi.restoreAllMocks());

it('opens this offline incarnation from inside GHOST without saving or discarding its draft', async () => {
  const { open, save, close, container } = setup();
  const button = await screen.findByRole('button', { name: 'View notes' });
  await waitFor(() => expect(button).toBeEnabled());
  expect(container.querySelector('.tavern-ghost-scroll')).toContainElement(button);
  fireEvent.click(screen.getByRole('button', { name: 'Edit GHOST' }));
  const draft = screen.getByRole('textbox');
  fireEvent.change(draft, { target: { value: 'Unsaved GHOST draft' } });
  fireEvent.click(button);
  await waitFor(() => expect(open).toHaveBeenCalledExactlyOnceWith({ agentId: 'agent-piner', projectRoot }));
  expect(draft).toHaveValue('Unsaved GHOST draft');
  expect(save).not.toHaveBeenCalled();
  expect(close).not.toHaveBeenCalled();
});

it('disables the missing-file entry and never invokes open', async () => {
  const { open } = setup(false);
  await screen.findByText('No notes yet');
  const button = screen.getByRole('button', { name: 'View notes' });
  expect(button).toBeDisabled();
  fireEvent.click(button);
  expect(open).not.toHaveBeenCalled();
});

it('shows a system-open error without replacing it with the empty-state copy', async () => {
  const { open, close } = setup();
  open.mockRejectedValue('Notes file not found.');
  const button = await screen.findByRole('button', { name: 'View notes' });
  await waitFor(() => expect(button).toBeEnabled());
  fireEvent.click(button);
  await screen.findByText('Notes file not found.');
  expect(screen.queryByText('No notes yet')).not.toBeInTheDocument();
  expect(close).not.toHaveBeenCalled();
  await waitFor(() => expect(button).toBeEnabled());
});
