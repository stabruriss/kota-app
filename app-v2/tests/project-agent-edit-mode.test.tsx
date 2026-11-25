import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import { sameAutoRoute } from '../src/chrome/ProfileSheet';
import * as client from '../src/pty-client';

const projectRoot = '/fixture/project';
function setup(existingNames: Array<{ id: string; name: string }> = []) {
  let detail: client.ProjectAgentDetail = {
    agentId: 'agent-aster', displayName: 'Aster', sourceHeroId: 'source-hero', sourceHeroName: 'Hero',
    projectId: 'project-kota', projectName: 'Kota', cli: 'codex', provider: 'codex', model: 'default',
    skills: [], args: [], ghost: 'Original ghost', status: 'active', avatarId: null,
    adapterPath: '/fixture/AGENTS.md', shellPath: '/fixture/SHELL.yaml', agentYamlPath: '/fixture/agent.yaml',
    inviteEligibility: { eligible: false, reason: 'fixture', proposedHeroId: 'h', proposedDisplayName: 'Aster' },
    record: { turns: 0, incarnations: 1, estimatedTokens: 0 }, forkable: false, dirty: false, dirtySummary: '',
  };
  vi.spyOn(client, 'loadProjectAgentDetail').mockImplementation(async () => structuredClone(detail));
  vi.spyOn(client, 'listAccountSkills').mockResolvedValue([{ id: 'fixture-skill', name: 'Fixture skill', description: 'Does a fixture thing.',
    path: '/fixture/skill', kind: 'manual', bundledDefault: false, valid: true, createdAt: '' }]);
  vi.spyOn(client, 'supportedShellsStatus').mockResolvedValue([]);
  const save = vi.spyOn(client, 'saveProjectAgentDetail').mockImplementation(async (request) => {
    detail = { ...detail, displayName: request.displayName, nameFields: request.nameFields, ghost: request.ghost,
      skills: request.skills, avatarId: request.avatarId, model: request.model, effort: request.effort };
    return structuredClone(detail);
  });
  const close = vi.fn();
  const view = render(<ProjectAgentProfileOverlay agentId="agent-aster" projectRoot={projectRoot}
    existingNames={existingNames} onClose={close} onSaved={vi.fn()} onRemoveFromProject={vi.fn()} />);
  return { ...view, save, close };
}
async function edit() {
  const button = await screen.findByRole('button', { name: 'Edit' });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
}
const ghostInput = () => screen.getByRole('textbox', { name: 'GHOST text' });
const typeGhost = (value: string) => fireEvent.change(ghostInput(), { target: { value } });
afterEach(() => vi.restoreAllMocks());

it('reads without inputs until Edit, then saves name, GHOST and skills in one request', async () => {
  const { save, close } = setup();
  const editButton = await screen.findByRole('button', { name: 'Edit' });
  await waitFor(() => expect(editButton).toBeEnabled());
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Change avatar' })).toBeDisabled();
  expect(screen.getByLabelText('GHOST text')).toHaveTextContent('Original ghost');
  expect(screen.getByText('0 active · 1 available')).toBeInTheDocument();
  expect(screen.getByText('No active skills.')).toBeInTheDocument();

  await userEvent.click(editButton);
  expect(screen.getByText('Editing')).toBeInTheDocument();
  expect(screen.getByText('No changes yet')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Remove from project' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Tavern/ })).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Given name'), { target: { value: 'Rowan' } });
  typeGhost('New ghost');
  await userEvent.click(screen.getByRole('checkbox', { name: 'Fixture skill skill' }));
  expect(screen.getByText('Unsaved changes')).toBeInTheDocument();
  expect(screen.getByText('1 active · 1 available')).toBeInTheDocument();
  expect(save).not.toHaveBeenCalled();

  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Edit' })).toBeEnabled());
  expect(save).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({
    agentId: 'agent-aster', projectRoot, ghost: 'New ghost', skills: ['fixture-skill'],
    nameFields: expect.objectContaining({ given: 'Rowan' }), provider: 'codex', model: 'default',
  }));
  expect(screen.getByText('Saved')).toBeInTheDocument();
  expect(screen.queryByText('Editing')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.getByLabelText('GHOST text')).toHaveTextContent('New ghost');
  expect(within(screen.getByRole('region', { name: 'SKILLS' })).getByText('Fixture skill')).toBeInTheDocument();
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(close).not.toHaveBeenCalled();
});

it('asks before Cancel drops changes, and keeps or discards them as chosen', async () => {
  const { save, close } = setup();
  await edit();
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument(); // Nothing staged: straight back to reading.
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();

  await edit();
  typeGhost('Draft ghost');
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  const prompt = within(screen.getByRole('alertdialog', { name: 'Discard changes?' }));
  expect(prompt.getByText('Your changes to GHOST will be lost.')).toBeInTheDocument();
  expect(prompt.queryByRole('button', { name: 'Save and Leave' })).not.toBeInTheDocument();
  await userEvent.click(prompt.getByRole('button', { name: 'Keep Editing' }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(ghostInput()).toHaveValue('Draft ghost');

  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Discard Changes' }));
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(screen.getByLabelText('GHOST text')).toHaveTextContent('Original ghost');
  expect(screen.getByRole('button', { name: 'Edit' })).toBeEnabled();
  expect(save).not.toHaveBeenCalled();
  expect(close).not.toHaveBeenCalled();
});

it.each(['Save and Leave', 'Discard Changes'])('asks before Back leaves with changes (%s)', async (choice) => {
  const { save, close } = setup();
  await userEvent.click(await screen.findByRole('button', { name: 'Back' }));
  expect(close).toHaveBeenCalledOnce(); // Nothing staged in the reading state.
  close.mockClear();

  await edit();
  fireEvent.change(screen.getByLabelText('Given name'), { target: { value: 'Rowan' } });
  typeGhost('Draft ghost');
  await userEvent.click(screen.getByRole('button', { name: 'Back' }));
  const prompt = within(screen.getByRole('alertdialog', { name: 'Save changes before leaving?' }));
  expect(prompt.getByText('You have unsaved changes to name and GHOST.')).toBeInTheDocument();
  expect(close).not.toHaveBeenCalled();
  await userEvent.click(prompt.getByRole('button', { name: choice }));
  await waitFor(() => expect(close).toHaveBeenCalledOnce());
  if (choice === 'Save and Leave') {
    expect(save).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ ghost: 'Draft ghost', nameFields: expect.objectContaining({ given: 'Rowan' }) }));
  } else {
    expect(save).not.toHaveBeenCalled();
  }
});

it('stays in the edit state when Save and Leave fails', async () => {
  const { save, close } = setup();
  save.mockRejectedValueOnce(new Error('write failed'));
  await edit();
  typeGhost('Draft ghost');
  await userEvent.click(screen.getByRole('button', { name: 'Back' }));
  await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Save and Leave' }));
  await screen.findByText('Error: write failed');
  expect(close).not.toHaveBeenCalled();
  expect(ghostInput()).toHaveValue('Draft ghost');
  expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
});

it('steps back one level per Escape: prompt, edit state, then the page', async () => {
  const { save, close } = setup();
  await edit();
  typeGhost('Draft ghost');
  fireEvent.keyDown(ghostInput(), { key: 'Escape', isComposing: true }); // An IME closing its candidates.
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.getByRole('alertdialog', { name: 'Discard changes?' })).toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(ghostInput()).toHaveValue('Draft ghost');
  typeGhost('Original ghost');
  expect(screen.getByText('No changes yet')).toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(close).not.toHaveBeenCalled();
  await userEvent.keyboard('{Escape}');
  expect(close).toHaveBeenCalledOnce();
  expect(save).not.toHaveBeenCalled();
});

it('holds Save and says why while the name cannot be saved', async () => {
  const { save } = setup([{ id: 'agent-other', name: 'Taken' }]);
  await edit();
  const given = screen.getByLabelText('Given name');
  fireEvent.change(given, { target: { value: '  ' } });
  expect(screen.getByText('Enter a given name to save')).toBeInTheDocument();
  expect(given).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.change(given, { target: { value: 'Taken' } });
  expect(screen.getByText('Name already exists in this project.')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.change(given, { target: { value: 'Rowan' } });
  expect(screen.getByText('Unsaved changes')).toBeInTheDocument();
  expect(given).toHaveAttribute('aria-invalid', 'false');
  expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
  expect(save).not.toHaveBeenCalled();
});

it('does not count a skill switched off and on again as a change', async () => {
  setup();
  await edit();
  const skill = screen.getByRole('checkbox', { name: 'Fixture skill skill' });
  await userEvent.click(skill);
  expect(screen.getByText('Unsaved changes')).toBeInTheDocument();
  await userEvent.click(skill);
  expect(screen.getByText('No changes yet')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
});

it('compares Auto String groups by what they route, not by key order or model ids', () => {
  const saved = { models: [
    { intent: 'Hard tasks.', thinkingLevel: 'high', modelId: 'reason', providerId: 'fable', id: 'sol' },
    { intent: 'Easy tasks.', thinkingLevel: 'off', modelId: 'plain', providerId: 'fable', id: 'luna' },
  ], defaultModel: 'luna', threshold: 0.85 };
  const edited = { threshold: 0.85, defaultModel: 'model-2', models: [
    { id: 'model-1', providerId: 'fable', modelId: 'reason', thinkingLevel: 'high', intent: 'Hard tasks.' },
    { id: 'model-2', providerId: 'fable', modelId: 'plain', thinkingLevel: 'off', intent: 'Easy tasks.' },
  ] };
  expect(sameAutoRoute(saved, edited)).toBe(true);
  expect(sameAutoRoute(null, undefined)).toBe(true);
  expect(sameAutoRoute(saved, null)).toBe(false);
  expect(sameAutoRoute(saved, { ...edited, defaultModel: 'model-1' })).toBe(false);
  expect(sameAutoRoute(saved, { ...edited, threshold: 0.8 })).toBe(false);
  expect(sameAutoRoute(saved, { ...edited, models: [edited.models[0], { ...edited.models[1], intent: 'Simple tasks.' }] })).toBe(false);
  expect(sameAutoRoute(saved, { ...edited, models: [edited.models[1], edited.models[0]], defaultModel: 'model-1' })).toBe(false);
});
