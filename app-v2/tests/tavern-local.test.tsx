import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi, type MockInstance } from 'vitest';
import { TavernModal, loadTavernHeroIncarnationProfile, loadTavernWorkingHeroes } from '../src/chrome/TavernModal';
import type { LocalAuthAvailability, LocalModelCatalog } from '../src/lib/local-auth';
import * as pty from '../src/pty-client';

const ipc = vi.hoisted(() => ({
  calls: [] as string[],
  handlers: new Map<string, () => unknown>(),
}));
vi.mock('@tauri-apps/api/core', async (original) => ({
  ...await original<typeof import('@tauri-apps/api/core')>(),
  invoke: async (command: string) => {
    ipc.calls.push(command);
    const handler = ipc.handlers.get(command);
    if (!handler) throw new Error(`Unexpected fixture IPC: ${command}`);
    return handler();
  },
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));
vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(), KOTA_LOCAL_ENABLED: true,
}));

const signedIn = (): LocalAuthAvailability => ({ schema: 1, summary: 'ready', providers: [
  { providerId: 'openai-codex', status: 'signed_in', authType: 'oauth' },
  { providerId: 'anthropic', status: 'not_signed_in' },
] });
const catalog: LocalModelCatalog = { schema: 1, providers: [
  { providerId: 'openai-codex', linked: true, models: [
    { modelId: 'sol-1', name: 'Sol One', thinkingLevels: ['low', 'medium', 'high'] },
  ] },
  { providerId: 'anthropic', linked: false, models: [
    { modelId: 'family/quill-2', name: 'Quill Two', thinkingLevels: ['off'] },
  ] },
] };
let storageDescriptor: PropertyDescriptor | undefined;
let disk: pty.TavernHeroProfileDraft[];
let saves: MockInstance<typeof pty.saveTavernHeroProfiles>;
beforeEach(async () => {
  const values = new Map<string, string>();
  storageDescriptor = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  ipc.calls = [];
  ipc.handlers = new Map<string, () => unknown>([
    ['local_rpc_auth_availability', signedIn],
    ['local_rpc_model_catalog', () => catalog],
    ['local_rpc_jev_read', () => ({schema:1, provider:'openrouter', ready:false, keys:[{providerId:'openrouter',status:'none'}]})],
    ['puppeteer_jev_prompts', () => ({stringFirst:{},stringLater:{},danceContext:{},danceRule:{}})],
  ]);
  disk = [{ heroId: 'hero-local-fixture', name: 'Aster', kind: 'custom', provider: 'kota-local',
    model: 'default', effort: null, skills: [], ghost: 'Fixture', shell: 'provider: kota-local\nmodel: default\n' }];
  vi.spyOn(pty, 'loadTavernHeroProfiles').mockImplementation(async () => structuredClone(disk));
  saves = vi.spyOn(pty, 'saveTavernHeroProfiles').mockImplementation(async (profiles) => { disk = structuredClone(profiles); });
  const shells = await pty.supportedShellsStatus();
  vi.spyOn(pty, 'supportedShellsStatus').mockResolvedValue([
    ...shells.map((shell) => ({ ...shell, installed: true })),
    { id: 'kota-local', name: 'Kota Local', bin: 'pi', installed: false, resolvedBin: null,
      installUrl: 'https://pi.dev', summary: 'Missing old CLI fixture', modelOptions: [], effortOptions: [] },
  ]);
});
afterEach(() => {
  cleanup(); vi.restoreAllMocks();
  if (storageDescriptor) Object.defineProperty(window, 'localStorage', storageDescriptor);
});
const chip = () => screen.getByRole('status', { name: 'Kota Local availability' });
const profile = () => screen.getByRole('dialog', { name: 'Aster profile' });
const authNote = () => profile().querySelector('.tavern-local-auth-note') as HTMLElement;
async function openHero() {
  render(<TavernModal open onClose={() => {}} />);
  await userEvent.click(await screen.findByRole('button', { name: /^Aster\b/ }));
  return profile();
}
async function editShell() {
  await userEvent.click(within(profile()).getByRole('button', { name: 'Edit Shell' }));
  const shell = within(profile());
  const localModel = shell.queryByRole('button', {name:'Local model'});
  return localModel ? [localModel, shell.getByRole('combobox', {name:'Thinking'})] : shell.getAllByRole('combobox');
}

it.each(['Escape', 'close button', 'backdrop'])('explains disabled Hero saving after closing incomplete String edits (%s)', async action => {
  disk[0].model = 'kota/auto'; disk[0].effort = 'off';
  disk[0].autoRoute = { threshold: .85, defaultModel: 'model-1', models: [
    { id: 'model-1', providerId: 'openai-codex', modelId: 'sol-1', thinkingLevel: 'high', intent: 'Hard tasks.' },
    { id: 'model-2', providerId: 'openai-codex', modelId: 'sol-1', thinkingLevel: 'low', intent: 'Easy tasks.' },
  ] };
  ipc.handlers.set('local_rpc_jev_read', () => ({ schema: 1, provider: 'openrouter', ready: true, keys: [{ providerId: 'openrouter', status: 'verified' }] }));
  await openHero(); await editShell();
  await waitFor(() => expect(within(profile()).getByRole('button', { name: 'Edit routing' })).toBeEnabled());
  await userEvent.click(within(profile()).getByRole('button', { name: 'Edit routing' }));
  const editor = screen.getByRole('dialog', { name: 'Auto String' });
  await userEvent.clear(within(editor).getByRole('textbox', { name: 'Model 2 intent' }));
  if (action === 'Escape') await userEvent.keyboard('{Escape}');
  else if (action === 'backdrop') fireEvent.pointerDown(editor.parentElement!);
  else await userEvent.click(within(editor).getByRole('button', { name: 'Close model picker' }));
  expect(screen.queryByRole('dialog', { name: 'Auto String' })).not.toBeInTheDocument();
  const shell = within(profile());
  expect(shell.getByRole('button', { name: 'Save Shell' })).toBeDisabled();
  expect(shell.getByText('Finish the Auto String configuration before saving.')).toHaveClass('project-agent-shell-apply-note');
  expect(shell.queryByText('Apply on Next Incarnation')).not.toBeInTheDocument();
  expect(saves).not.toHaveBeenCalled();
  await userEvent.click(shell.getByRole('button', { name: 'Edit routing' }));
  expect(screen.getByRole('textbox', { name: 'Model 2 intent' })).toHaveValue('');
  fireEvent.change(screen.getByRole('textbox', { name: 'Model 2 intent' }), { target: { value: 'Easy tasks.' } });
  await userEvent.click(screen.getByRole('button', { name: 'OK' }));
  expect(shell.getByRole('button', { name: 'Save Shell' })).toBeEnabled();
  expect(shell.getByText('Apply on Next Incarnation')).toBeVisible();
});

it('shows the Anthropic credit marker in Hero candidates and saved labels, using provider identity instead of model names', async () => {
  ipc.handlers.set('local_rpc_model_catalog', () => ({ schema:1, providers:[
    { providerId:'anthropic', linked:true, models:[{modelId:'claude-fixture',name:'Claude fixture',thinkingLevels:['off']}] },
    { providerId:'openrouter', linked:true, models:[{modelId:'anthropic/claude-fixture',name:'Claude fixture',thinkingLevels:['off']}] },
  ] }));
  await openHero();
  let [model] = await editShell();
  await within(profile()).findByText('2 models from signed-in providers');
  await userEvent.click(model);
  const choice = screen.getByRole('option',{name:/Claude fixture \(Use Credit\) Anthropic/});
  expect(within(choice).getByText('(Use Credit)')).toHaveClass('local-model-credit');
  expect(within(screen.getByRole('option',{name:/Claude fixture OpenRouter/})).queryByText('(Use Credit)')).not.toBeInTheDocument();
  await userEvent.click(choice);
  expect(within(profile()).getByText('(Use Credit)')).toHaveClass('local-model-credit');
  await userEvent.click(within(profile()).getByRole('button',{name:'Save Shell'}));
  await waitFor(() => expect(disk.find(entry => entry.heroId === 'hero-local-fixture')?.model).toBe('anthropic/claude-fixture'));
  expect(within(profile()).getByLabelText('Current SHELL settings')).toHaveTextContent('anthropic/claude-fixture(Use Credit)');
  [model] = await editShell();
  await userEvent.click(model);
  await userEvent.click(screen.getByRole('option',{name:/Claude fixture OpenRouter/}));
  expect(within(profile()).queryByText('(Use Credit)')).not.toBeInTheDocument();
});

it('shows neutral Checking while auth is pending, even with a populated catalog and missing CLI fixture', async () => {
  let reply!: (value: LocalAuthAvailability) => void;
  ipc.handlers.set('local_rpc_auth_availability', () => new Promise<LocalAuthAvailability>((resolve) => { reply = resolve; }));
  render(<TavernModal open onClose={() => {}} />);
  expect(chip()).toHaveTextContent('Checking sign-in');
  expect(within(chip()).queryByRole('button')).not.toBeInTheDocument();
  await act(async () => { reply(signedIn()); });
  await waitFor(() => expect(chip()).toHaveTextContent('Ready'));
  expect(ipc.calls).not.toContain('local_rpc_discover');
});

it.each(['rejection', 'error DTO'] as const)('shows Status unavailable and Retry only after %s; retry reads auth again', async (failure) => {
  disk[0].model = 'openai-codex/sol-1';
  disk[0].shell = 'provider: kota-local\nmodel: openai-codex/sol-1\n';
  let failed = true;
  ipc.handlers.set('local_rpc_auth_availability', () => {
    if (!failed) return signedIn();
    if (failure === 'rejection') throw new Error('auth_store_read_failed');
    return { schema: 1, summary: 'error', providers: [] };
  });
  await openHero();
  await waitFor(() => expect(authNote()).toHaveTextContent('Status unavailable'));
  expect(within(authNote()).queryByRole('button', { name: /Sign in/ })).not.toBeInTheDocument();
  expect(authNote()).not.toHaveTextContent('Not signed in');
  expect(ipc.calls.filter((command) => command === 'local_rpc_auth_availability')).toHaveLength(1);
  failed = false;
  await userEvent.click(within(authNote()).getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(authNote()).toHaveTextContent('Ready'));
});

it.each([
  ['not_signed_in', 'Not signed in'], ['invalid', 'Sign-in needs attention'],
  ['checking', 'Checking sign-in'], ['error', 'Status unavailable'],
] as const)('a specific model uses its own provider %s while the provider chip remains Ready', async (status, text) => {
  disk[0].model = 'anthropic/family/quill-2';
  disk[0].shell = 'provider: kota-local\nmodel: anthropic/family/quill-2\n';
  ipc.handlers.set('local_rpc_auth_availability', () => ({ ...signedIn(), providers: [
    signedIn().providers[0], { providerId: 'anthropic', status },
  ] }));
  await openHero();
  await waitFor(() => expect(chip()).toHaveTextContent('Ready'));
  expect(authNote()).toHaveTextContent(text);
  const action = status === 'not_signed_in' || status === 'invalid' ? /Sign in/ : status === 'error' ? 'Retry' : null;
  if (action) expect(within(authNote()).getByRole('button', { name: action })).toBeInTheDocument();
  else expect(within(authNote()).queryByRole('button')).not.toBeInTheDocument();
});

it('offers only linked models and saves concrete native-supported thinking, preserving off', async () => {
  disk[0].model = 'anthropic/family/quill-2';
  disk[0].shell = 'provider: kota-local\nmodel: anthropic/family/quill-2\n';
  await openHero();
  let [model, thinking] = await editShell();
  expect(within(profile()).getByText('1 model from signed-in providers')).toBeInTheDocument();
  expect(model).toHaveTextContent('Quill Two');
  expect(thinking).toHaveValue('off');
  expect(within(profile()).getByRole('button', { name: 'Save Shell' })).toBeDisabled();
  await userEvent.click(model);
  expect(screen.getByRole('option', { name: /Sol One OpenAI Codex/ })).toBeInTheDocument();
  expect(screen.queryByRole('option', { name: /Quill Two/ })).not.toBeInTheDocument();
  expect(within(profile()).queryByRole('option', { name: /default/i })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('option', { name: /Sol One OpenAI Codex/ }));
  expect(thinking).toHaveValue('low'); // Existing explicit off clamps upward for this model.
  await userEvent.click(within(profile()).getByRole('button', { name: 'Save Shell' }));
  await waitFor(() => expect(disk.find(entry => entry.heroId === 'hero-local-fixture')).toMatchObject({ model: 'openai-codex/sol-1', effort: 'low' }));
  const recruit = loadTavernHeroIncarnationProfile('hero-local-fixture')!;
  expect(recruit.args).toEqual(['--model', 'openai-codex/sol-1', '--thinking', 'low']);
  expect(recruit.shell).toContain('effort: low');
  expect(ipc.calls).not.toContain('local_rpc_discover');
});

it('the Hero Local thinking menu lists every supported level and saves the one picked', async () => {
  disk[0].model = 'openai-codex/sol-1';
  disk[0].effort = 'medium';
  disk[0].shell = 'provider: kota-local\nmodel: openai-codex/sol-1\neffort: medium\n';
  await openHero();
  const [, thinking] = await editShell();
  await waitFor(() => expect(thinking).toHaveValue('medium'));
  await userEvent.click(thinking);
  expect(within(thinking).getAllByRole('option').map(option => option.textContent)).toEqual(['low', 'medium', 'high']);
  await userEvent.type(thinking, '{Backspace}');
  expect(thinking).toHaveValue('medium');
  await userEvent.selectOptions(thinking, 'high');
  await userEvent.click(within(profile()).getByRole('button', { name: 'Save Shell' }));
  await waitFor(() => expect(disk.find(entry => entry.heroId === 'hero-local-fixture')).toMatchObject({ model: 'openai-codex/sol-1', effort: 'high' }));
});

it('a catalog read failure keeps Ready, with a separate model refresh action', async () => {
  ipc.handlers.set('local_rpc_model_catalog', () => { throw new Error('catalog_unavailable'); });
  await openHero();
  await editShell();
  await waitFor(() => expect(authNote()).toHaveTextContent('Ready'));
  expect(within(profile()).getByText('Model catalog unavailable')).toBeInTheDocument();
  expect(within(profile()).getByRole('button', { name: 'Update model list' })).toBeEnabled();
  expect(within(authNote()).queryByRole('button', { name: /Sign in/ })).not.toBeInTheDocument();
});

it.each(['none', 'rejected'] as const)('keeps Auto String disabled with a %s key and opens Puppeteer without saving staged SHELL', async status => {
  ipc.handlers.set('local_rpc_jev_read', () => ({schema:1, provider:'openrouter', ready:false, keys:[{providerId:'openrouter',status}]}));
  await openHero();
  const [model] = await editShell();
  await userEvent.click(model);
  expect(screen.getByRole('option', { name: /^Auto String/ })).toBeDisabled();
  expect(screen.getByRole('option', { name: /Sol One OpenAI Codex/ })).toBeEnabled();
  expect(screen.queryByText('Coming later')).not.toBeInTheDocument();
  expect(screen.getByText(status === 'rejected' ? 'Jev key rejected. Update it in Puppeteer.' : 'Set up Jev to use Auto String')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Jev settings' }));
  await waitFor(() => expect(document.activeElement?.id).toBe('puppeteer-jev'));
  expect(screen.getByRole('button', { name: 'Hero' })).toHaveClass('active');
  expect(screen.queryByRole('dialog', { name: 'Aster profile' })).not.toBeInTheDocument();
  expect(loadTavernHeroIncarnationProfile('hero-local-fixture')!.model).toBe('default');
  for (const [saved] of saves.mock.calls) expect(saved.find((entry: pty.TavernHeroProfileDraft) => entry.heroId === 'hero-local-fixture')?.model).toBe('default');
  expect(ipc.calls.some((command) => /auth_start|write_defaults|local_rpc_start|local_rpc_control|spawn/.test(command))).toBe(false);
});

it('normal Sign in lands on the whole Local card; returning to Hero refreshes login and ignores a stale missing CLI entry', async () => {
  let linked = false;
  ipc.handlers.set('local_rpc_auth_availability', () => linked ? signedIn() : { schema: 1, summary: 'not_ready', providers: [] });
  await openHero();
  await waitFor(() => expect(authNote()).toHaveTextContent('Not signed in'));
  await userEvent.click(within(authNote()).getByRole('button', { name: /Sign in/ }));
  await waitFor(() => expect(document.activeElement?.id).toBe('link-kota-local'));
  linked = true;
  await userEvent.click(screen.getByRole('button', { name: 'Hero' }));
  await waitFor(() => expect(chip()).toHaveTextContent('Ready'));
  const missing = await pty.supportedShellsStatus();
  expect(loadTavernWorkingHeroes(missing).find((hero) => hero.id === 'hero-local-fixture')).toMatchObject({ available: false, unavailableReason: 'Choose a model for this Hero in Tavern.' });
  expect(ipc.calls).not.toContain('local_rpc_discover');
});

it('a late pre-Link response cannot restore Ready after returning to a signed-out Hero', async () => {
  let oldReply!: (value: LocalAuthAvailability) => void;
  let reads = 0;
  ipc.handlers.set('local_rpc_auth_availability', () => {
    reads += 1;
    if (reads === 1) return new Promise<LocalAuthAvailability>((resolve) => { oldReply = resolve; });
    return { schema: 1, summary: 'not_ready', providers: [] };
  });
  render(<TavernModal open onClose={() => {}} />);
  expect(chip()).toHaveTextContent('Checking sign-in');
  await userEvent.click(screen.getByRole('button', { name: 'Link' }));
  await screen.findByTestId('tavern-kota-local');
  await userEvent.click(screen.getByRole('button', { name: 'Hero' }));
  await waitFor(() => expect(chip()).toHaveTextContent('Not signed in'));
  await act(async () => { oldReply(signedIn()); });
  expect(chip()).toHaveTextContent('Not signed in');
  expect(chip()).not.toHaveTextContent('Ready');
});

it('choosing Local from a CLI template requires a supported model before saving, without discovery', async () => {
  disk[0].provider = 'codex'; disk[0].model = 'existing-model'; disk[0].effort = 'high';
  disk[0].shell = 'provider: codex\nmodel: existing-model\neffort: high\n';
  await openHero(); await editShell();
  await userEvent.click(within(profile()).getByRole('button', { name: 'Provider Codex' }));
  await userEvent.click(within(profile()).getByRole('option', { name: /Kota Local Ready/ }));
  await waitFor(() => expect(authNote()).toHaveTextContent('Ready'));
  const model = within(profile()).getByRole('button', {name:'Local model'});
  const thinking = within(profile()).getByRole('combobox', {name:'Thinking'});
  expect(model).toHaveTextContent('Choose a model');
  expect(thinking).toBeDisabled(); expect(thinking).toHaveTextContent('Choose a model first');
  expect(within(profile()).getByRole('button', { name: 'Save Shell' })).toBeDisabled();
  expect(within(profile()).getByText('Choose a model before saving.')).toBeInTheDocument();
  await userEvent.click(model);
  expect(within(profile()).queryByRole('option', { name: /default/i })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('option', { name: /Sol One OpenAI Codex/ }));
  expect(thinking).toHaveValue('medium');
  expect(within(profile()).getByText('Apply on Next Incarnation')).toBeInTheDocument();
  await userEvent.click(within(profile()).getByRole('button', { name: 'Save Shell' }));
  await waitFor(() => expect(disk.find(entry => entry.heroId === 'hero-local-fixture')).toMatchObject({ provider: 'kota-local', model: 'openai-codex/sol-1', effort: 'medium' }));
  expect(loadTavernHeroIncarnationProfile('hero-local-fixture')!.args).toEqual(['--model', 'openai-codex/sol-1', '--thinking', 'medium']);
  expect(ipc.calls).not.toContain('local_rpc_discover');
});

it('keeps an old Local default template in batch saves while another Hero changes normally', async () => {
  disk.push({ heroId: 'hero-terminal-fixture', name: 'Quill', kind: 'custom', provider: 'codex', model: 'default', effort: null, skills: [], ghost: 'Terminal fixture', shell: 'provider: codex\nmodel: default\n' });
  render(<TavernModal open onClose={() => {}} />);
  await userEvent.click(await screen.findByRole('button', { name: /^Quill\b/ }));
  const quill = screen.getByRole('dialog', { name: 'Quill profile' });
  await userEvent.click(within(quill).getByRole('button', { name: 'Edit Shell' }));
  const [model] = within(quill).getAllByRole('combobox');
  await userEvent.clear(model); await userEvent.type(model, 'explicit-terminal-model');
  await userEvent.click(within(quill).getByRole('button', { name: 'Save Shell' }));
  await waitFor(() => expect(disk.find(entry => entry.heroId === 'hero-terminal-fixture')?.model).toBe('explicit-terminal-model'));
  expect(disk.find(entry => entry.heroId === 'hero-local-fixture')?.model).toBe('default');
  expect(saves.mock.calls.some(([profiles]) => profiles.some(entry => entry.heroId === 'hero-local-fixture' && entry.model === 'default') && profiles.some(entry => entry.heroId === 'hero-terminal-fixture' && entry.model === 'explicit-terminal-model'))).toBe(true);
  expect(loadTavernWorkingHeroes().find(hero => hero.id === 'hero-local-fixture')).toMatchObject({ available: false, unavailableReason: 'Choose a model for this Hero in Tavern.' });
});

it('stages a complete Hero String group until Save Shell, then copies its quoted configuration to new incarnations', async () => {
  const savedHero = () => disk.find(hero => hero.heroId === 'hero-local-fixture')!;
  disk[0].model = 'openai-codex/sol-1'; disk[0].effort = 'high';
  disk[0].shell = 'provider: kota-local\nmodel: openai-codex/sol-1\neffort: high\n';
  ipc.handlers.set('local_rpc_jev_read', () => ({schema:1,provider:'openrouter',ready:true,keys:[{providerId:'openrouter',status:'verified'}]}));
  ipc.handlers.set('local_rpc_model_catalog', () => ({...catalog, providers:[catalog.providers[0],
    {providerId:'magpie',linked:true,models:[{modelId:'copilot/qwen3:32b',name:'Qwen 3',thinkingLevels:['off','low']}]}]}));
  await openHero(); const [model] = await editShell();
  await userEvent.click(model); await userEvent.click(screen.getByRole('option',{name:/^Auto String/}));
  const editor = screen.getByRole('dialog',{name:'Auto String'});
  expect(within(editor).getByRole('radio',{name:'Default model 1'})).toBeChecked();
  expect(within(editor).getByRole('slider')).toHaveValue('0.5');
  expect(within(editor).getByRole('textbox',{name:'Model 1 intent'})).not.toHaveValue('');
  await userEvent.click(within(editor).getByRole('button',{name:'Model 1 model'}));
  await userEvent.click(within(editor).getByRole('option',{name:/Sol One OpenAI Codex/}));
  expect(savedHero().model).toBe('openai-codex/sol-1');
  await userEvent.click(within(editor).getByRole('button',{name:'Model 2 model'}));
  await userEvent.type(within(editor).getByRole('searchbox'), 'qwen');
  await userEvent.click(within(editor).getByRole('option',{name:'Magpie / copilot / Qwen 3 Magpie'}));
  await userEvent.selectOptions(within(editor).getByRole('combobox',{name:'Model 2 thinking'}),'off');
  expect(within(editor).getByRole('combobox',{name:'Model 2 thinking'})).toHaveValue('off');
  expect(screen.getAllByRole('dialog',{name:'Auto String'})).toHaveLength(1);
  await userEvent.click(within(editor).getByRole('button',{name:'OK'}));
  expect(savedHero().model).toBe('openai-codex/sol-1');
  expect(within(profile()).getByRole('combobox',{name:'Thinking'})).toBeDisabled();
  expect(within(profile()).getByRole('combobox',{name:'Thinking'})).toHaveDisplayValue('By Auto String');
  expect(within(profile()).getByRole('button',{name:'Save Shell'})).toBeEnabled();
  await userEvent.click(within(profile()).getByRole('button',{name:'Save Shell'}));
  await waitFor(() => expect(savedHero().model).toBe('kota/auto'));
  expect(savedHero().autoRoute).toMatchObject({threshold:.5,defaultModel:'model-1',models:[
    {id:'model-1',providerId:'openai-codex',modelId:'sol-1',thinkingLevel:'medium'},
    {id:'model-2',providerId:'magpie',modelId:'copilot/qwen3:32b',thinkingLevel:'off'},
  ]});
  const recruit = loadTavernHeroIncarnationProfile('hero-local-fixture')!;
  expect(recruit.args).toEqual(['--model','kota/auto','--thinking','off']);
  expect(recruit.shell).toContain('model: "magpie/copilot/qwen3:32b"');
  expect(recruit.shell).toContain('effort: "off"');
  expect(recruit.shell).toContain('default-model: "model-1"');
  expect(ipc.calls).not.toContain('local_rpc_control');
});
