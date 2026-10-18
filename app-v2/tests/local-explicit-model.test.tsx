import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ProjectAgentProfileOverlay } from '../src/chrome/ProjectAgentProfileOverlay';
import * as pty from '../src/pty-client';
import * as auth from '../src/lib/local-auth';
import type { JevSettings } from '../src/lib/jev';

vi.mock('../src/lib/local-rpc', async original => ({
  ...await original<typeof import('../src/lib/local-rpc')>(), KOTA_LOCAL_ENABLED: true,
}));

let detail: pty.ProjectAgentDetail;
const jevIpc = vi.hoisted(() => ({settings:null as JevSettings | null, reads:0}));
vi.mock('@tauri-apps/api/core', async original => {
  const api = await original<typeof import('@tauri-apps/api/core')>();
  return {...api, invoke: (...args:Parameters<typeof api.invoke>) => {
    if (args[0] === 'local_rpc_jev_read') {jevIpc.reads += 1; return Promise.resolve(jevIpc.settings);}
    return api.invoke(...args);
  }};
});
vi.mock('@tauri-apps/api/event', () => ({listen:async () => () => {}}));

const models: auth.LocalModelCatalog = { schema:1, providers:[
  { providerId:'fable', linked:true, models:[
    { modelId:'reason', name:'Reason', thinkingLevels:['low','high'] },
    { modelId:'plain', name:'Plain', thinkingLevels:['off'] },
  ] },
  { providerId:'other', linked:false, models:[{modelId:'hidden',name:'Unlinked',thinkingLevels:['off']}] },
] };
beforeEach(() => {
  vi.restoreAllMocks(); jevIpc.reads = 0; jevIpc.settings = {schema:1,provider:'openrouter',ready:false,keys:[{providerId:'openrouter',status:'none'}]};
  detail = { agentId:'agent-explicit', displayName:'Aster', provider:'codex', cli:'codex', model:'default', effort:null,
    skills:[], ghost:'Fixture', args:[], projectName:'fixture', record:{turns:0,incarnations:1,estimatedTokens:0},
    status:'active', sourceHeroId:'hero-fixture', sourceHeroName:'Fixture', projectId:'fixture', avatarId:null,
    shellPath:'/fixture/SHELL.yaml', adapterPath:'/fixture/AGENTS.md', agentYamlPath:'/fixture/agent.yaml',
    forkable:false, dirty:false, dirtySummary:'',
    inviteEligibility:{eligible:false,reason:'fixture',proposedHeroId:'hero-fixture',proposedDisplayName:'Aster'},
  };
  vi.spyOn(pty,'loadProjectAgentDetail').mockImplementation(async () => structuredClone(detail));
  vi.spyOn(pty,'listAccountSkills').mockResolvedValue([]);
  vi.spyOn(pty,'supportedShellsStatus').mockResolvedValue([{id:'codex',name:'Codex',bin:'codex',installed:true,resolvedBin:'/fixture/codex',installUrl:'https://example.test',summary:'fixture',modelOptions:[{id:'default',label:'CLI default',source:'default'}],effortOptions:[]}]);
  vi.spyOn(pty,'refreshProviderModelOptions').mockResolvedValue([]);
  vi.spyOn(auth,'readLocalModelCatalog').mockResolvedValue(models);
  vi.spyOn(auth,'readLocalAuthAvailability').mockResolvedValue({schema:1,summary:'ready',providers:[{providerId:'fable',status:'signed_in',authType:'api_key'}]});
  vi.spyOn(pty,'saveProjectAgentDetail').mockImplementation(async request => {
    detail = {...detail,provider:request.provider!,model:request.model,effort:request.effort};
    return structuredClone(detail);
  });
});
afterEach(() => {cleanup();vi.restoreAllMocks();});
async function open() {
  render(<ProjectAgentProfileOverlay agentId="agent-explicit" projectRoot="/fixture" existingNames={[]}
    onClose={() => {}} onSaved={() => {}} onRemoveFromProject={() => {}} />);
  const shell = await screen.findByRole('region',{name:'SHELL'});
  await userEvent.click(within(shell).getByRole('button',{name:'Edit SHELL'}));
  return within(shell);
}

async function openStringFromModelPicker(shell: ReturnType<typeof within>) {
  await userEvent.click(shell.getByRole('button', { name: 'Local model' }));
  const automatic = await screen.findByRole('option', { name: /^Auto String/ });
  await waitFor(() => expect(automatic).toBeEnabled());
  await userEvent.click(automatic);
}

it.each([2, 6])('keeps %s-model String details inside the picker, not the saved or editing profile', async count => {
  const group: NonNullable<pty.ProjectAgentDetail['autoRoute']> = {
    threshold: .85, defaultModel: 'model-1', models: Array.from({ length: count }, (_, index) => ({
      id: `model-${index + 1}`, providerId: 'fable', modelId: `fixture-${index + 1}`,
      thinkingLevel: 'high', intent: `Intent ${index + 1}.`,
    })),
  };
  Object.assign(detail, { provider: 'kota-local', cli: 'kota-local', model: 'kota/auto', effort: 'off', autoRoute: group });
  jevIpc.settings = { schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] };
  vi.mocked(auth.readLocalModelCatalog).mockResolvedValue({ schema: 1, providers: [{ providerId: 'fable', linked: true,
    models: group.models.map(item => ({ modelId: item.modelId, name: item.modelId, thinkingLevels: ['off', 'high'] })),
  }] });
  render(<ProjectAgentProfileOverlay agentId="agent-explicit" projectRoot="/fixture" existingNames={[]}
    onClose={() => {}} onSaved={() => {}} onRemoveFromProject={() => {}} />);
  const shell = within(await screen.findByRole('region', { name: 'SHELL' }));
  await waitFor(() => expect(shell.getByLabelText('Current SHELL settings')).toHaveTextContent('Auto StringBy Auto String'));
  expect(shell.queryByLabelText('Auto String configuration')).not.toBeInTheDocument();
  expect(shell.queryByText('Probability threshold')).not.toBeInTheDocument();
  await userEvent.click(shell.getByRole('button', { name: 'Edit SHELL' }));
  expect(shell.queryByLabelText('Auto String configuration')).not.toBeInTheDocument();
  expect(shell.queryByRole('button', { name: 'Edit routing' })).not.toBeInTheDocument();
  expect(shell.queryByText(/models from signed-in providers|Fetching model IDs/)).not.toBeInTheDocument();
  expect(shell.getByRole('combobox', { name: 'Thinking' })).toHaveTextContent('By Auto String');
  await openStringFromModelPicker(shell);
  const editor = within(screen.getByRole('dialog', { name: 'Auto String' }));
  for (const [index, item] of group.models.entries()) {
    expect(editor.getByRole('button', { name: `Model ${index + 1} model` })).toHaveTextContent(item.modelId);
    expect(editor.getByRole('combobox', { name: `Model ${index + 1} thinking` })).toHaveValue(item.thinkingLevel);
    expect(editor.getByRole('textbox', { name: `Model ${index + 1} intent` })).toHaveValue(item.intent);
  }
  expect(editor.getByRole('slider')).toHaveValue('0.85');
  await userEvent.click(editor.getByRole('button', { name: 'OK' }));
  await userEvent.click(shell.getByRole('button', { name: 'Save SHELL' }));
  await waitFor(() => expect(shell.getByRole('button', { name: 'Edit SHELL' })).toBeEnabled());
  expect(shell.queryByLabelText('Auto String configuration')).not.toBeInTheDocument();
  expect(pty.saveProjectAgentDetail).not.toHaveBeenCalled(); // Opening/closing does not alter configuration.
});

it.each(['Escape', 'close button', 'backdrop'])('explains disabled profile saving after closing incomplete String edits (%s)', async action => {
  detail.provider = 'kota-local'; detail.cli = 'kota-local'; detail.model = 'kota/auto'; detail.effort = 'off';
  detail.autoRoute = { threshold: .85, defaultModel: 'model-1', models: [
    { id: 'model-1', providerId: 'fable', modelId: 'reason', thinkingLevel: 'high', intent: 'Hard tasks.' },
    { id: 'model-2', providerId: 'fable', modelId: 'plain', thinkingLevel: 'off', intent: 'Easy tasks.' },
  ] };
  jevIpc.settings = { schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] };
  const shell = await open();
  await openStringFromModelPicker(shell);
  const editor = screen.getByRole('dialog', { name: 'Auto String' });
  await userEvent.clear(within(editor).getByRole('textbox', { name: 'Model 2 intent' }));
  if (action === 'Escape') await userEvent.keyboard('{Escape}');
  else if (action === 'backdrop') fireEvent.pointerDown(editor.parentElement!);
  else await userEvent.click(within(editor).getByRole('button', { name: 'Close model picker' }));
  expect(screen.queryByRole('dialog', { name: 'Auto String' })).not.toBeInTheDocument();
  expect(shell.getByRole('button', { name: 'Save SHELL' })).toBeDisabled();
  expect(shell.getByText('Finish the Auto String configuration before saving.')).toHaveClass('project-agent-shell-apply-note');
  expect(shell.queryByText('Apply on Next Launch')).not.toBeInTheDocument();
  expect(pty.saveProjectAgentDetail).not.toHaveBeenCalled();
  // The retained incomplete draft remains editable through the model picker.
  await openStringFromModelPicker(shell);
  expect(screen.getByRole('textbox', { name: 'Model 2 intent' })).toHaveValue('');
  fireEvent.change(screen.getByRole('textbox', { name: 'Model 2 intent' }), { target: { value: 'Easy tasks.' } });
  await userEvent.click(screen.getByRole('button', { name: 'OK' }));
  expect(shell.getByRole('button', { name: 'Save SHELL' })).toBeEnabled();
  expect(shell.getByText('Save applies to a running Local session, or to its next launch.')).toBeVisible();
});

it('requires a supported linked model on a Local provider switch and saves medium-clamped thinking', async () => {
  const shell = await open();
  await userEvent.selectOptions(shell.getByRole('combobox',{name:'Provider'}),'kota-local');
  const model = shell.getByRole('button', {name:'Local model'});
  const thinking = shell.getByRole('combobox', {name:'Thinking'});
  await shell.findByText('2 models from signed-in providers');
  expect(shell.queryByText('Ready')).not.toBeInTheDocument();
  expect(shell.queryByRole('button', {name:/Sign in|Retry/})).not.toBeInTheDocument();
  expect(model).toHaveTextContent('Choose a model');
  expect(thinking).toBeDisabled(); expect(thinking).toHaveTextContent('Choose a model first');
  expect(shell.getByRole('button',{name:'Save SHELL'})).toBeDisabled();
  expect(shell.getByText('Choose a model before saving.')).toBeInTheDocument();
  await userEvent.click(model);
  expect(shell.queryByRole('option',{name:/default/i})).not.toBeInTheDocument();
  expect(screen.queryByRole('option',{name:/Unlinked/})).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('option',{name:/Reason fable/}));
  expect(thinking).toHaveValue('high');
  expect(within(thinking).getAllByRole('option').map(option => option.textContent)).toEqual(['low','high']);
  expect(shell.getByText('Save applies to a running Local session, or to its next launch.')).toBeInTheDocument();
  await userEvent.click(shell.getByRole('button',{name:'Save SHELL'}));
  await waitFor(() => expect(pty.saveProjectAgentDetail).toHaveBeenCalledOnce());
  expect(pty.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({provider:'kota-local',model:'fable/reason',effort:'high'}));
  expect(pty.refreshProviderModelOptions).not.toHaveBeenCalled();
});

it('keeps the loaded Local SHELL revision on Save and requires Reload after an external edit', async () => {
  Object.assign(detail, { provider: 'kota-local', cli: 'kota-local', model: 'fable/reason', effort: 'high', shellRevision: 'before-edit' });
  const shell = await open();
  await waitFor(() => expect(shell.getByRole('combobox', { name: 'Thinking' })).toBeEnabled());
  await userEvent.selectOptions(shell.getByRole('combobox', { name: 'Thinking' }), 'low');
  detail = { ...detail, effort: 'high', shellRevision: 'external-edit' };
  vi.mocked(pty.saveProjectAgentDetail).mockRejectedValueOnce('local_config_changed');
  await userEvent.click(shell.getByRole('button', { name: 'Save SHELL' }));
  await screen.findByText('Settings changed outside this editor. Reload before saving.');
  expect(pty.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({ shellRevision: 'before-edit', effort: 'low' }));
  expect(shell.getByRole('combobox', { name: 'Thinking' })).toHaveValue('low');
  await userEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await waitFor(() => expect(shell.queryByRole('button', { name: 'Save SHELL' })).not.toBeInTheDocument());
  expect(pty.loadProjectAgentDetail).toHaveBeenCalledTimes(2);
});

it('the Local thinking menu lists every supported level and saves the one picked', async () => {
  const shell = await open();
  await userEvent.selectOptions(shell.getByRole('combobox', {name:'Provider'}), 'kota-local');
  await shell.findByText('2 models from signed-in providers');
  await userEvent.click(shell.getByRole('button', {name:'Local model'}));
  await userEvent.click(screen.getByRole('option', {name:/Reason fable/}));
  const thinking = shell.getByRole('combobox', {name:'Thinking'});
  expect(thinking).toHaveValue('high');
  // Native fixed choices preserve the Thinking fix: no fuzzy filtering or text reset.
  expect(within(thinking).getAllByRole('option').map(option => option.textContent)).toEqual(['low', 'high']);
  await userEvent.type(thinking, '{Backspace}{Backspace}');
  expect(thinking).toHaveValue('high');
  await userEvent.selectOptions(thinking, 'low');
  expect(thinking).toHaveValue('low');
  await userEvent.click(shell.getByRole('button', {name:'Save SHELL'}));
  await waitFor(() => expect(pty.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({provider:'kota-local',model:'fable/reason',effort:'low'})));
});

it('keeps the saved Local thinking when Edit SHELL opens before the model catalog arrives', async () => {
  detail.provider = detail.cli = 'kota-local'; detail.model = 'fable/reason'; detail.effort = 'low';
  let resolveCatalog!: (catalog: auth.LocalModelCatalog) => void;
  vi.mocked(auth.readLocalModelCatalog).mockImplementation(() => new Promise(resolve => { resolveCatalog = resolve; }));
  const shell = await open();
  await waitFor(() => expect(auth.readLocalModelCatalog).toHaveBeenCalled());
  resolveCatalog(models);
  await shell.findByText('2 models from signed-in providers');
  expect(shell.getByRole('combobox', {name:'Thinking'})).toHaveValue('low');
});

it('keeps a terminal Effort field free text', async () => {
  const shell = await open();
  const [,,effort] = shell.getAllByRole('combobox');
  expect(effort).not.toHaveAttribute('readonly');
});

it('repairs a legacy Local default without effort and chooses off for a non-reasoning model', async () => {
  detail.provider = 'kota-local'; detail.cli = 'kota-local';
  const shell = await open();
  await waitFor(() => expect(auth.readLocalModelCatalog).toHaveBeenCalledOnce());
  const model = shell.getByRole('button', {name:'Local model'});
  const thinking = shell.getByRole('combobox', {name:'Thinking'});
  expect(model).toHaveTextContent('Choose a model'); expect(thinking).toBeDisabled();
  await userEvent.click(model); await userEvent.click(screen.getByRole('option',{name:/Plain fable/}));
  expect(thinking).toHaveValue('off');
  await userEvent.click(shell.getByRole('button',{name:'Save SHELL'}));
  await waitFor(() => expect(pty.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({model:'fable/plain',effort:'off'})));
});

it('does not query Local while editing a Terminal provider and keeps its CLI Default option', async () => {
  const shell = await open();
  const [,model] = shell.getAllByRole('combobox');
  await userEvent.clear(model);
  expect(shell.getByRole('option',{name:/default/})).toBeInTheDocument();
  expect(auth.readLocalAuthAvailability).not.toHaveBeenCalled();
  expect(auth.readLocalModelCatalog).not.toHaveBeenCalled();
});

it('marks exact Anthropic candidates before selection and the selected Local model, without marking Magpie Claude', async () => {
  vi.mocked(auth.readLocalModelCatalog).mockResolvedValue({ schema:1, providers:[
    { providerId:'anthropic', linked:true, models:[{modelId:'claude-fixture',name:'Claude fixture',thinkingLevels:['off']}] },
    { providerId:'magpie', linked:true, models:[{modelId:'claude-fixture',name:'Claude fixture',thinkingLevels:['off']}] },
  ] });
  const shell = await open();
  await userEvent.selectOptions(shell.getByRole('combobox',{name:'Provider'}),'kota-local');
  await shell.findByText('2 models from signed-in providers');
  const model = shell.getByRole('button', {name:'Local model'});
  await userEvent.click(model);
  const anthropic = screen.getByRole('option',{name:/Claude fixture \(Use Credit\) Anthropic/});
  const magpie = screen.getByRole('option',{name:/Magpie \/ Claude fixture/});
  expect(within(anthropic).getByText('(Use Credit)')).toHaveClass('local-model-credit');
  expect(within(magpie).queryByText('(Use Credit)')).not.toBeInTheDocument();
  await userEvent.click(anthropic);
  expect(shell.getByText('(Use Credit)')).toHaveClass('local-model-credit');
  await userEvent.click(shell.getByRole('button',{name:'Save SHELL'}));
  await waitFor(() => expect(pty.saveProjectAgentDetail).toHaveBeenCalledWith(expect.objectContaining({model:'anthropic/claude-fixture',effort:'off'})));
  expect(shell.getByLabelText('Current SHELL settings')).toHaveTextContent('anthropic/claude-fixture(Use Credit)');
  await userEvent.click(shell.getByRole('button',{name:'Edit SHELL'}));
  const nextModel = shell.getByRole('button', {name:'Local model'});
  await userEvent.click(nextModel);
  await userEvent.click(screen.getByRole('option',{name:/Magpie \/ Claude fixture/}));
  expect(shell.queryByText('(Use Credit)')).not.toBeInTheDocument();
});

it('keeps a Terminal provider model unmarked even if its id starts with anthropic', async () => {
  detail.model = 'anthropic/claude-fixture';
  const shell = await open();
  expect(shell.queryByText('(Use Credit)')).not.toBeInTheDocument();
  expect(auth.readLocalModelCatalog).not.toHaveBeenCalled();
});

it('saves the staged agent String group only with Save SHELL and restores it after saving a concrete model', async () => {
  jevIpc.settings = {schema:1,provider:'openrouter',ready:true,keys:[{providerId:'openrouter',status:'verified'}]};
  detail.provider = detail.cli = 'kota-local'; detail.model = 'fable/reason'; detail.effort = 'high';
  vi.mocked(pty.saveProjectAgentDetail).mockImplementation(async request => {
    detail = {...detail,provider:request.provider!,model:request.model,effort:request.effort,
      autoRoute:request.model === 'kota/auto' ? request.autoRoute ?? undefined : detail.autoRoute};
    return structuredClone(detail);
  });
  const shell = await open();
  await userEvent.click(shell.getByRole('button',{name:'Local model'}));
  await userEvent.click(screen.getByRole('option',{name:/^Auto String/}));
  const editor = screen.getByRole('dialog',{name:'Auto String'});
  for (const [index,name] of [[1,/Reason fable/],[2,/Plain fable/]] as const) {
    await userEvent.click(within(editor).getByRole('button',{name:`Model ${index} model`}));
    await userEvent.click(within(editor).getByRole('option',{name}));
  }
  await userEvent.selectOptions(within(editor).getByRole('combobox',{name:'Model 1 thinking'}),'high');
  await userEvent.click(within(editor).getByRole('radio',{name:'Default model 2'}));
  await userEvent.click(within(editor).getByRole('button',{name:'OK'}));
  expect(pty.saveProjectAgentDetail).not.toHaveBeenCalled();
  expect(shell.getByRole('combobox',{name:'Thinking'})).toBeDisabled();
  expect(shell.getByRole('button',{name:'Save SHELL'})).toBeEnabled();
  await userEvent.click(shell.getByRole('button',{name:'Save SHELL'}));
  await waitFor(() => expect(pty.saveProjectAgentDetail).toHaveBeenCalledOnce());
  expect(pty.saveProjectAgentDetail).toHaveBeenLastCalledWith(expect.objectContaining({model:'kota/auto',effort:'off',autoRoute:{threshold:.5,defaultModel:'model-2',models:[
    expect.objectContaining({id:'model-1',providerId:'fable',modelId:'reason',thinkingLevel:'high'}),
    expect.objectContaining({id:'model-2',providerId:'fable',modelId:'plain',thinkingLevel:'off'}),
  ]}}));
  await userEvent.click(shell.getByRole('button',{name:'Edit SHELL'}));
  await userEvent.click(shell.getByRole('button',{name:'Local model'}));
  await userEvent.click(screen.getByRole('option',{name:/Plain fable/}));
  await userEvent.click(shell.getByRole('button',{name:'Save SHELL'}));
  await waitFor(() => expect(pty.saveProjectAgentDetail).toHaveBeenCalledTimes(2));
  expect(pty.saveProjectAgentDetail).toHaveBeenLastCalledWith(expect.not.objectContaining({autoRoute:expect.anything()}));
  await userEvent.click(shell.getByRole('button',{name:'Edit SHELL'}));
  await userEvent.click(shell.getByRole('button',{name:'Local model'}));
  await userEvent.click(screen.getByRole('option',{name:/^Auto String/}));
  const reopened = screen.getByRole('dialog',{name:'Auto String'});
  expect(within(reopened).getByRole('button',{name:'Model 1 model'})).toHaveTextContent('Reason');
  expect(within(reopened).getByRole('combobox',{name:'Model 1 thinking'})).toHaveValue('high');
  expect(within(reopened).getByRole('radio',{name:'Default model 2'})).toBeChecked();
  expect(pty.saveProjectAgentDetail).toHaveBeenCalledTimes(2);
});
