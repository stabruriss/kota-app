import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { KotaLocalLinkCard } from '../src/chrome/KotaLocalLinkCard';
import { TavernModal } from '../src/chrome/TavernModal';
import {
  LOCAL_AUTH_MAIN,
  LOCAL_AUTH_MORE,
  LOCAL_AUTH_PROVIDERS,
  catalogModel,
  clampThinkingLevel,
  isLocalResourceUrl,
  linkedCatalogProviders,
  requestKotaLocalLink,
  type LocalModelCatalog,
} from '../src/lib/local-auth';
import * as pty from '../src/pty-client';

type Call = { command: string; args?: any };
const bus = vi.hoisted(() => ({
  tauri: false,
  calls: [] as Array<{ command: string; args?: any }>,
  handlers: new Map<string, (args: any) => unknown>(),
  listeners: new Set<(event: { payload: unknown }) => void>(),
}));
vi.mock('@tauri-apps/api/core', async (original) => ({
  ...await original<typeof import('@tauri-apps/api/core')>(),
  isTauri: () => bus.tauri,
  invoke: async (command: string, args?: unknown) => {
    bus.calls.push({ command, args });
    const handler = bus.handlers.get(command);
    if (!handler) throw new Error(`Unexpected fixture IPC: ${command}`);
    return handler(args);
  },
}));
vi.mock('@tauri-apps/api/event', () => ({
  listen: async (name: string, callback: (event: { payload: unknown }) => void) => {
    if (name !== 'local-auth-event') return () => {};
    bus.listeners.add(callback);
    return () => { bus.listeners.delete(callback); };
  },
}));
vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(),
  KOTA_LOCAL_ENABLED: true,
  discoverLocal: vi.fn(),
}));

const emit = (operationId: string, body: Record<string, unknown>) => {
  for (const listener of [...bus.listeners]) listener({ payload: { schema: 1, channel: 'kota.auth', operationId, ...body } });
};
const emitNow = async (operationId: string, body: Record<string, unknown>) => { await act(async () => { emit(operationId, body); }); };
const callsOf = (command: string) => bus.calls.filter((call: Call) => call.command === command);

type Entry = { status: string; authType?: 'api_key' | 'oauth' };
let states: Record<string, Entry> = {};
const availability = () => ({
  schema: 1,
  providers: LOCAL_AUTH_PROVIDERS.map(({ id }) => ({ providerId: id, ...(states[id] ?? { status: 'not_signed_in' }) })),
  summary: Object.values(states).some((entry) => entry.status === 'signed_in') ? 'ready' : 'not_ready',
});
// The full catalog: every provider, with linked set per test from the sign-in states below.
const catalogModels: Array<{ providerId: string; models: LocalModelCatalog['providers'][number]['models'] }> = [
  { providerId: 'anthropic', models: [{ modelId: 'quill-2', name: 'Quill Two', thinkingLevels: ['off', 'low', 'medium', 'high', 'xhigh', 'max'] }] },
  { providerId: 'openai', models: [
    { modelId: 'sol-1', name: 'Sol One', thinkingLevels: ['low', 'medium', 'high', 'xhigh'] },
    { modelId: 'sol-mini', name: 'Sol Mini', thinkingLevels: ['off'] },
  ] },
  { providerId: 'zai', models: [{ modelId: 'glyph-5', name: 'Glyph Five', thinkingLevels: ['off', 'minimal', 'low', 'medium', 'high'] }] },
];

beforeEach(() => {
  bus.tauri = false;
  bus.calls = [];
  bus.listeners.clear();
  states = { openai: { status: 'signed_in', authType: 'oauth' }, anthropic: { status: 'signed_in', authType: 'api_key' } };
  bus.handlers = new Map<string, (args: any) => unknown>([
    ['github_accounts_read', (): pty.GithubAccounts => ({ defaultLogin: null, accounts: [] })],
    ['local_rpc_auth_availability', () => availability()],
    ['local_rpc_model_catalog', (): LocalModelCatalog => ({
      schema: 1,
      providers: catalogModels.map((provider) => ({ ...provider, linked: states[provider.providerId]?.status === 'signed_in' })),
    })],
    ['local_rpc_auth_respond', () => null],
    ['local_rpc_auth_cancel', () => null],
    ['local_rpc_auth_in_use', () => []],
  ]);
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

const row = (name: string) => screen.getByRole('article', { name });
const summary = () => screen.getByTestId('kota-local-summary');
async function renderCard(props: Parameters<typeof KotaLocalLinkCard>[0] = {}) {
  const view = render(<KotaLocalLinkCard {...props} />);
  await waitFor(() => expect(summary()).not.toHaveTextContent('Checking…'));
  return view;
}

it('lists the main providers, then More with OpenRouter first, and filters More', async () => {
  states.openrouter = { status: 'invalid', authType: 'oauth' };
  await renderCard();
  const order = [...document.querySelectorAll('.local-auth-row')].map((element) => element.getAttribute('data-provider'));
  expect(order).toEqual([...LOCAL_AUTH_MAIN.map(({ id }) => id), 'magpie', ...LOCAL_AUTH_MORE.map(({ id }) => id)]);
  expect(order.filter(id => id !== 'magpie')).toEqual([...LOCAL_AUTH_MAIN, ...LOCAL_AUTH_MORE].map(({ id }) => id));
  expect(LOCAL_AUTH_MAIN).toHaveLength(10);
  expect(LOCAL_AUTH_MORE).toHaveLength(32);
  expect(LOCAL_AUTH_MORE[0].id).toBe('openrouter');
  expect(document.querySelector('.local-auth-count')).toHaveTextContent('32');
  // A More provider with a state worth seeing opens More by itself.
  expect(document.querySelector('details.local-auth-more')).toHaveAttribute('open');

  expect(summary()).toHaveTextContent('2 providers linked · 1 needs attention');
  expect(within(row('OpenAI')).getByText('Linked · OAuth')).toBeInTheDocument();
  expect(within(row('OpenAI')).getByRole('button', { name: 'Sign out' })).toBeEnabled();
  expect(within(row('Anthropic')).getByText('Linked · API key')).toBeInTheDocument();
  expect(within(row('OpenRouter')).getByText('Needs to sign in again')).toBeInTheDocument();
  expect(within(row('OpenRouter')).getByRole('button', { name: 'Sign in again' })).toBeInTheDocument();
  expect(within(row('Moonshot AI')).getByText('Not linked')).toBeInTheDocument();
  expect(within(row('Moonshot AI')).getByText('API key')).toBeInTheDocument();
  expect(within(row('Kimi For Coding')).getByText('OAuth · API key')).toBeInTheDocument();
  expect(row('Azure OpenAI').querySelector('.local-auth-logo.mono')).toHaveTextContent('AO');

  await userEvent.type(screen.getByRole('searchbox', { name: 'Filter providers' }), 'xiao');
  const more = document.querySelector('details.local-auth-more')!;
  expect([...more.querySelectorAll('.local-auth-row')].map((element) => element.getAttribute('data-provider')))
    .toEqual(['xiaomi', 'xiaomi-token-plan-cn', 'xiaomi-token-plan-ams', 'xiaomi-token-plan-sgp']);
});

it('keeps More closed when nothing in it is linked', async () => {
  await renderCard();
  expect(document.querySelector('details.local-auth-more')).not.toHaveAttribute('open');
});

it('shows Status unavailable with Retry when the availability read fails, never Not linked', async () => {
  let fail = true;
  bus.handlers.set('local_rpc_auth_availability', () => {
    if (fail) throw new Error('availability-read-failed');
    return availability();
  });
  await renderCard();
  expect(summary()).toHaveTextContent('Status unavailable');
  expect(screen.queryByText('Not linked')).not.toBeInTheDocument();
  expect(screen.queryAllByRole('button', { name: 'Sign in' })).toHaveLength(0);
  expect(within(row('DeepSeek')).getByText('Status unavailable')).toBeInTheDocument();
  expect(screen.queryByRole('combobox', { name: 'Default model' })).not.toBeInTheDocument();
  expect(screen.queryByRole('combobox', { name: 'Default thinking' })).not.toBeInTheDocument();

  fail = false;
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(summary()).toHaveTextContent('2 providers linked'));
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  expect(within(row('DeepSeek')).getByText('Not linked')).toBeInTheDocument();
});

it('treats an empty error answer as a failed read, not as nothing linked', async () => {
  bus.handlers.set('local_rpc_auth_availability', () => ({ schema: 1, providers: [], summary: 'error' }));
  await renderCard();
  expect(summary()).toHaveTextContent('Status unavailable');
  expect(screen.queryByText('Not linked')).not.toBeInTheDocument();
});

it('gives a provider without a trustworthy status its own Retry', async () => {
  states.deepseek = { status: 'error' };
  await renderCard();
  expect(summary()).toHaveTextContent('2 providers linked · 1 needs attention');
  const reads = callsOf('local_rpc_auth_availability').length;
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(callsOf('local_rpc_auth_availability').length).toBe(reads + 1));
});

it('links to Puppeteer for Jev without reading or writing retired Local defaults', async () => {
  await renderCard();
  expect(screen.queryByText('Local agent defaults')).not.toBeInTheDocument();
  expect(screen.queryByRole('combobox', { name: 'Default model' })).not.toBeInTheDocument();
  expect(screen.queryByRole('combobox', { name: 'Default thinking' })).not.toBeInTheDocument();
  expect(document.getElementById('link-kota-local-defaults')).toBeNull();
  expect(document.getElementById('link-kota-local-jev')).toBeNull();
  expect(screen.queryByRole('combobox', {name:'Jev provider'})).not.toBeInTheDocument();
  expect(screen.getByText('Jev is configured in Puppeteer.')).toBeVisible();
  expect(screen.getByRole('button', {name:'Open Puppeteer'})).toBeEnabled();
  expect(bus.calls.some(({ command }) => command.includes('defaults'))).toBe(false);
  expect(callsOf('local_rpc_model_catalog')).toHaveLength(0);
});

it('runs a browser sign-in, holding events that arrive before start returns', async () => {
  bus.handlers.set('local_rpc_auth_start', () => {
    emit('op-1', { event: 'started', operation: 'login', providerId: 'openrouter' });
    emit('op-1', { event: 'auth_event', providerId: 'openrouter', notification: {
      type: 'auth_url', url: 'https://openrouter.ai/auth?callback_url=http://localhost:3000/callback', instructions: 'Complete sign-in in your browser.' } });
    emit('op-1', { event: 'prompt', providerId: 'openrouter', authType: 'oauth', promptId: 'p-code',
      prompt: { type: 'manual_code', message: 'Paste the authorization code here:', placeholder: 'code' } });
    emit('op-other', { event: 'auth_event', providerId: 'openrouter', notification: { type: 'progress', message: 'Not this flow' } });
    return { operationId: 'op-1' };
  });
  await renderCard();
  await userEvent.click(within(row('OpenRouter')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to OpenRouter' });
  expect(within(dialog).getByRole('heading')).toHaveTextContent('Sign in · OpenRouter');
  expect(within(dialog).getByText('How do you want to sign in?')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Continue' })).toBeDisabled();
  // Another provider can't start while this one runs.
  expect(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' })).toBeDisabled();
  await userEvent.click(within(dialog).getByRole('radio', { name: /Sign in with OpenRouter/ }));
  await userEvent.click(within(dialog).getByRole('button', { name: 'Continue' }));

  await within(dialog).findByText('Continue in your browser');
  expect(callsOf('local_rpc_auth_start')[0].args).toEqual({ request: { operation: 'login', providerId: 'openrouter', authType: 'oauth' } });
  expect(within(dialog).getByText('Kota Local · OAuth')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Open browser' })).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Copy link' })).toBeInTheDocument();
  expect(within(dialog).queryByText('Not this flow')).not.toBeInTheDocument();
  const code = within(dialog).getByLabelText('Paste the authorization code here:');
  expect(within(dialog).getByRole('button', { name: 'Submit' })).toBeDisabled();
  expect(code).toHaveFocus();

  await emitNow('op-1', { event: 'prompt_closed', providerId: 'openrouter', promptId: 'p-code', reason: 'step-aborted' });
  expect(within(dialog).getByText('Browser sign-in finished. The paste field closed.')).toBeInTheDocument();
  expect(within(dialog).queryByLabelText('Paste the authorization code here:')).not.toBeInTheDocument();
  await emitNow('op-1', { event: 'auth_event', providerId: 'openrouter', notification: { type: 'progress', message: 'Exchanging authorization code for an API key...' } });
  expect(within(dialog).getByText('Exchanging authorization code for an API key...').closest('.local-auth-progress')).not.toHaveClass('done');

  states.openrouter = { status: 'signed_in', authType: 'oauth' };
  await emitNow('op-1', { event: 'result', operation: 'login', providerId: 'openrouter', outcome: 'succeeded', commit: 'confirmed' });
  expect(within(dialog).getByText('Linked')).toBeInTheDocument();
  expect(within(dialog).getByText('OpenRouter · OAuth.')).toBeInTheDocument();
  expect(bus.listeners.size).toBe(0);
  await userEvent.click(within(dialog).getByRole('button', { name: 'Done' }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await waitFor(() => expect(within(row('OpenRouter')).getByText('Linked · OAuth')).toBeInTheDocument());
  expect(summary()).toHaveTextContent('3 providers linked');
});

it('takes a key without ever putting it in the page and maps an unverified save to Try again', async () => {
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-2' }));
  await renderCard();
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to DeepSeek' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  expect(callsOf('local_rpc_auth_start')[0].args).toEqual({ request: { operation: 'login', providerId: 'deepseek', authType: 'api_key' } });
  await emitNow('op-2', { event: 'prompt', providerId: 'deepseek', authType: 'api_key', promptId: 'p-key', credentialField: 'api_key.key',
    prompt: { type: 'secret', message: 'Enter DeepSeek API key' } });
  const key = within(dialog).getByLabelText('Enter DeepSeek API key');
  expect(key).toHaveAttribute('type', 'password');
  expect(within(dialog).getByText('Saved locally.')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Save key' })).toBeDisabled();
  await userEvent.type(key, 'sk-fixture-secret-123');
  expect(document.body.innerHTML).not.toContain('sk-fixture-secret-123');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Save key' }));
  expect(callsOf('local_rpc_auth_respond').map((call) => call.args)).toEqual([
    { request: { operationId: 'op-2', promptId: 'p-key', value: 'sk-fixture-secret-123' } },
  ]);
  expect(within(dialog).queryByLabelText('Enter DeepSeek API key')).not.toBeInTheDocument();
  expect(within(dialog).getByText('Key received. It is not shown again.')).toBeInTheDocument();
  expect(document.body.innerHTML).not.toContain('sk-fixture-secret-123');

  await emitNow('op-2', { event: 'result', operation: 'login', providerId: 'deepseek', outcome: 'committed-unsynchronized', commit: 'confirmed', code: 'credential-sync-failed' });
  expect(within(dialog).getByText('Could not verify the sign-in. Try again.')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Close' })).toBeInTheDocument();
  await userEvent.click(within(dialog).getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(2));
  expect(callsOf('local_rpc_auth_start')[1].args).toEqual({ request: { operation: 'login', providerId: 'deepseek', authType: 'api_key' } });
});

it.each([
  ['cancelled', 'unknown'],
  ['timeout', 'unknown'],
  ['interrupted', 'unknown'],
  ['failed', 'confirmed'],
  ['cancelled', undefined],
] as const)('never says nothing was saved after %s with commit %s', async (outcome, commit) => {
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-commit' }));
  await renderCard();
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to DeepSeek' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await emitNow('op-commit', { event: 'result', operation: 'login', providerId: 'deepseek', outcome, ...(commit ? { commit } : {}) });
  expect(within(dialog).getByText('Could not verify the sign-in. Try again.')).toBeInTheDocument();
  expect(within(dialog).queryByText(/Nothing was/)).not.toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Close' })).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Try again' })).toBeInTheDocument();
});

it.each([
  ['cancelled', undefined, 'Sign-in cancelled', 'Nothing was saved.', false],
  ['timeout', undefined, 'Sign-in timed out', 'Nothing was saved.', true],
  ['interrupted', 'runtime-exited', 'Sign-in stopped', 'Kota Local’s sign-in helper exited before finishing. Nothing was saved.', true],
  ['failed', 'storage-failed', 'Couldn’t save the sign-in', 'Kota couldn’t write it to its credential store. Nothing was changed.', true],
  ['failed', 'auth-failed', 'Sign-in failed', 'Nothing was saved.', true],
] as const)('says %s (%s) wrote nothing only when Rust reports not-written', async (outcome, code, title, body, retry) => {
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-clean' }));
  await renderCard();
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to DeepSeek' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await emitNow('op-clean', { event: 'result', operation: 'login', providerId: 'deepseek', outcome, commit: 'not-written', ...(code ? { code } : {}) });
  expect(within(dialog).getByText(title)).toBeInTheDocument();
  expect(within(dialog).getByText(body)).toBeInTheDocument();
  expect(within(dialog).queryByText('Could not verify the sign-in. Try again.')).not.toBeInTheDocument();
  expect(!!within(dialog).queryByRole('button', { name: 'Try again' })).toBe(retry);
});

async function openKeyPrompt(operationId: string) {
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId }));
  await renderCard();
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to DeepSeek' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await emitNow(operationId, { event: 'prompt', providerId: 'deepseek', authType: 'api_key', promptId: 'p-key', credentialField: 'api_key.key',
    prompt: { type: 'secret', message: 'Enter DeepSeek API key' } });
  return dialog;
}

it('keeps the field when Rust refuses a key, clears the key and says why', async () => {
  let refuse: unknown = { code: 'credential_invalid', providerId: 'deepseek' };
  bus.handlers.set('local_rpc_auth_respond', () => { if (refuse) throw refuse; return null; });
  const dialog = await openKeyPrompt('op-refused');
  await userEvent.type(within(dialog).getByLabelText('Enter DeepSeek API key'), '$OPENAI_SECRET_FIXTURE');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Save key' }));
  const key = await within(dialog).findByLabelText('Enter DeepSeek API key');
  expect(within(dialog).getByRole('alert')).toHaveTextContent('Paste the key itself. Commands and $VARIABLES aren’t accepted.');
  expect(key).toHaveValue('');
  expect(key).toBeEnabled();
  expect(key).toHaveFocus();
  expect(document.body.innerHTML).not.toContain('OPENAI_SECRET_FIXTURE');
  expect(within(dialog).queryByText('Key received. It is not shown again.')).not.toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Save key' })).toBeDisabled();

  refuse = { code: 'auth_prompt_closed' };
  await userEvent.type(key, 'sk-fixture-second');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Save key' }));
  expect(await within(dialog).findByText('Couldn’t submit this answer. Try again.')).toBeInTheDocument();
  expect(within(dialog).getByLabelText('Enter DeepSeek API key')).toHaveValue('');

  refuse = null;
  await userEvent.type(within(dialog).getByLabelText('Enter DeepSeek API key'), 'sk-fixture-third');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Save key' }));
  await waitFor(() => expect(within(dialog).queryByLabelText('Enter DeepSeek API key')).not.toBeInTheDocument());
  expect(within(dialog).queryByRole('alert')).not.toBeInTheDocument();
  expect(within(dialog).getByText('Key received. It is not shown again.')).toBeInTheDocument();
  expect(callsOf('local_rpc_auth_respond').map((call) => call.args.request.value))
    .toEqual(['$OPENAI_SECRET_FIXTURE', 'sk-fixture-second', 'sk-fixture-third']);
  expect(document.body.innerHTML).not.toMatch(/sk-fixture-(second|third)/);
});

it('keeps a refused text answer for editing', async () => {
  bus.handlers.set('local_rpc_auth_respond', () => { throw { code: 'auth_timeout' }; });
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-text' }));
  await renderCard();
  await userEvent.click(within(row('GitHub Copilot')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to GitHub Copilot' });
  await userEvent.click(within(dialog).getByRole('radio', { name: /Continue in your browser/ }));
  await userEvent.click(within(dialog).getByRole('button', { name: 'Continue' }));
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await emitNow('op-text', { event: 'prompt', providerId: 'github-copilot', authType: 'oauth', promptId: 'p-domain',
    prompt: { type: 'text', message: 'GitHub Enterprise URL/domain (blank for github.com)' } });
  await userEvent.type(within(dialog).getByLabelText('GitHub Enterprise URL/domain (blank for github.com)'), 'fixture.ghe.com');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Continue' }));
  expect(await within(dialog).findByText('Couldn’t submit this answer. Try again.')).toBeInTheDocument();
  expect(within(dialog).getByLabelText('GitHub Enterprise URL/domain (blank for github.com)')).toHaveValue('fixture.ghe.com');
});

it('removes the field only after Rust acknowledges the answer', async () => {
  let ack: () => void = () => {};
  bus.handlers.set('local_rpc_auth_respond', () => new Promise<null>((resolve) => { ack = () => resolve(null); }));
  const dialog = await openKeyPrompt('op-ack');
  await userEvent.type(within(dialog).getByLabelText('Enter DeepSeek API key'), 'sk-fixture-ack');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Save key' }));
  await waitFor(() => expect(callsOf('local_rpc_auth_respond')).toHaveLength(1));
  const key = within(dialog).getByLabelText('Enter DeepSeek API key');
  expect(key).toHaveValue('');
  expect(key).toBeDisabled();
  expect(within(dialog).getByRole('button', { name: 'Save key' })).toBeDisabled();
  expect(within(dialog).queryByText('Key received. It is not shown again.')).not.toBeInTheDocument();
  expect(document.body.innerHTML).not.toContain('sk-fixture-ack');
  await act(async () => { ack(); });
  expect(within(dialog).queryByLabelText('Enter DeepSeek API key')).not.toBeInTheDocument();
  expect(within(dialog).getByText('Key received. It is not shown again.')).toBeInTheDocument();
});

it('keeps a newer prompt that arrives before the old acknowledgement', async () => {
  let ack: () => void = () => {};
  bus.handlers.set('local_rpc_auth_respond', () => new Promise<null>((resolve) => { ack = () => resolve(null); }));
  const dialog = await openKeyPrompt('op-order');
  await userEvent.type(within(dialog).getByLabelText('Enter DeepSeek API key'), 'sk-fixture-order');
  await userEvent.click(within(dialog).getByRole('button', { name: 'Save key' }));
  await waitFor(() => expect(callsOf('local_rpc_auth_respond')).toHaveLength(1));
  await emitNow('op-order', { event: 'prompt', providerId: 'deepseek', authType: 'api_key', promptId: 'p-next',
    prompt: { type: 'text', message: 'Account region (blank for default)' } });
  expect(within(dialog).getByLabelText('Account region (blank for default)')).toBeEnabled();
  await act(async () => { ack(); });
  expect(within(dialog).getByLabelText('Account region (blank for default)')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Continue' })).toBeEnabled();
});

it('cancels with Escape without closing Tavern, and cancels when the dialog goes away', async () => {
  const tavernEscape = vi.fn();
  const onDocumentKey = (event: KeyboardEvent) => { if (event.key === 'Escape') tavernEscape(); };
  document.addEventListener('keydown', onDocumentKey, true);
  try {
    let next = 3;
    bus.handlers.set('local_rpc_auth_start', () => ({ operationId: `op-${next++}` }));
    const view = await renderCard();
    await userEvent.click(within(row('OpenAI Codex (legacy)')).getByRole('button', { name: 'Sign in' }));
    const dialog = await screen.findByRole('dialog', { name: 'Sign in to OpenAI Codex (legacy)' });
    await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
    await userEvent.keyboard('{Escape}');
    expect(callsOf('local_rpc_auth_cancel').map((call) => call.args)).toEqual([{ operationId: 'op-3' }]);
    expect(tavernEscape).not.toHaveBeenCalled();
    expect(within(dialog).getByRole('button', { name: 'Cancelling…' })).toBeDisabled();
    await emitNow('op-3', { event: 'result', operation: 'login', providerId: 'openai-codex', outcome: 'cancelled', commit: 'not-written' });
    expect(within(dialog).getByText('Sign-in cancelled')).toBeInTheDocument();
    expect(within(dialog).getByText('Nothing was saved.')).toBeInTheDocument();
    expect(within(dialog).queryByRole('button', { name: 'Try again' })).not.toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();

    await userEvent.click(within(row('OpenAI Codex (legacy)')).getByRole('button', { name: 'Sign in' }));
    await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(2));
    view.unmount();
    expect(callsOf('local_rpc_auth_cancel').map((call) => call.args)).toEqual([{ operationId: 'op-3' }, { operationId: 'op-4' }]);
    expect(bus.listeners.size).toBe(0);
  } finally {
    document.removeEventListener('keydown', onDocumentKey, true);
  }
});

it('cancels a sign-in that was still starting when Cancel was pressed', async () => {
  let release: (value: { operationId: string }) => void = () => {};
  bus.handlers.set('local_rpc_auth_start', () => new Promise((resolve) => { release = resolve; }));
  await renderCard();
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to DeepSeek' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
  expect(within(dialog).getByRole('button', { name: 'Cancelling…' })).toBeDisabled();
  expect(callsOf('local_rpc_auth_cancel')).toHaveLength(0);
  await act(async () => { release({ operationId: 'op-late' }); });
  expect(callsOf('local_rpc_auth_cancel').map((call) => call.args)).toEqual([{ operationId: 'op-late' }]);
  await emitNow('op-late', { event: 'result', operation: 'login', providerId: 'deepseek', outcome: 'cancelled', commit: 'not-written' });
  expect(within(dialog).getByText('Sign-in cancelled')).toBeInTheDocument();
});

it('shows a device code with its countdown while waiting on the browser', async () => {
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-device' }));
  await renderCard();
  await userEvent.click(within(row('OpenAI Codex (legacy)')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to OpenAI Codex (legacy)' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await emitNow('op-device', { event: 'auth_event', providerId: 'openai-codex', notification: {
    type: 'device_code', userCode: 'K7QF-2MRD', verificationUri: 'https://auth.openai.com/codex/device', intervalSeconds: 5, expiresInSeconds: 900 } });
  expect(within(dialog).getByText('Enter this code')).toBeInTheDocument();
  expect(within(dialog).getByText('K7QF-2MRD')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Copy code' })).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Open auth.openai.com' })).toBeInTheDocument();
  expect(within(dialog).getByText('Expires in 15:00')).toBeInTheDocument();
  expect(within(dialog).getByText('Waiting for you to confirm in the browser…')).toBeInTheDocument();
  await emitNow('op-device', { event: 'result', operation: 'login', providerId: 'openai-codex', outcome: 'timeout', commit: 'not-written' });
  expect(within(dialog).queryByText('Waiting for you to confirm in the browser…')).not.toBeInTheDocument();
  expect(within(dialog).getByText('Sign-in timed out')).toBeInTheDocument();
  expect(within(dialog).getByRole('button', { name: 'Try again' })).toBeInTheDocument();
});

it('opens sign-in links in the system browser through Kota, never with window.open', async () => {
  bus.tauri = true;
  let refuse = false;
  bus.handlers.set('open_external_url', () => { if (refuse) throw new Error('open-failed'); return null; });
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-links' }));
  const windowOpen = vi.spyOn(window, 'open').mockImplementation(() => null);
  await renderCard();
  await userEvent.click(within(row('OpenAI Codex (legacy)')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to OpenAI Codex (legacy)' });
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  await emitNow('op-links', { event: 'auth_event', providerId: 'openai-codex', notification: {
    type: 'info', message: 'Read the plan terms first.', links: [{ url: 'https://example.com/terms', label: 'Plan terms' }] } });
  await emitNow('op-links', { event: 'auth_event', providerId: 'openai-codex', notification: { type: 'auth_url', url: 'https://auth.example.com/authorize?state=fixture' } });
  await emitNow('op-links', { event: 'auth_event', providerId: 'openai-codex', notification: {
    type: 'device_code', userCode: 'K7QF-2MRD', verificationUri: 'https://auth.example.com/device' } });

  await userEvent.click(within(dialog).getByRole('button', { name: 'Plan terms' }));
  await userEvent.click(within(dialog).getByRole('button', { name: 'Open browser' }));
  await userEvent.click(within(dialog).getByRole('button', { name: 'Open auth.example.com' }));
  await waitFor(() => expect(callsOf('open_external_url')).toHaveLength(3));
  expect(callsOf('open_external_url').map((call) => call.args)).toEqual([
    { url: 'https://example.com/terms' },
    { url: 'https://auth.example.com/authorize?state=fixture' },
    { url: 'https://auth.example.com/device' },
  ]);
  expect(windowOpen).not.toHaveBeenCalled();

  // A failed open adds no message; Copy link stays the way out.
  refuse = true;
  const before = dialog.textContent;
  await userEvent.click(within(dialog).getByRole('button', { name: 'Open browser' }));
  await waitFor(() => expect(callsOf('open_external_url')).toHaveLength(4));
  expect(dialog.textContent).toBe(before);
  expect(within(dialog).getByRole('button', { name: 'Copy link' })).toBeEnabled();
  expect(windowOpen).not.toHaveBeenCalled();
});

it('says another sign-in is running when start is refused as busy', async () => {
  bus.handlers.set('local_rpc_auth_start', () => { throw 'busy-auth'; });
  await renderCard();
  await userEvent.click(within(row('DeepSeek')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to DeepSeek' });
  expect(await within(dialog).findByText('Another sign-in is in progress')).toBeInTheDocument();
  expect(within(dialog).getByText('Finish or cancel it first.')).toBeInTheDocument();
  expect(bus.listeners.size).toBe(0);
});

it('does not link new Azure from a legacy provider entry', async () => {
  states = { 'azure-openai-responses': { status: 'signed_in', authType: 'api_key' } };
  await renderCard();
  expect(LOCAL_AUTH_PROVIDERS.some(provider => provider.id === 'azure-openai-responses')).toBe(false);
  expect(within(row('Azure OpenAI')).getByText('Not linked')).toBeInTheDocument();
  expect(within(row('Azure OpenAI')).getByRole('button', { name: 'Sign in' })).toBeEnabled();
  expect(within(row('Azure OpenAI')).queryByText('Linked · API key')).not.toBeInTheDocument();
});

it('asks new Azure for the Resource URL first and sends it with the login', async () => {
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-azure' }));
  await renderCard();
  await userEvent.click(within(row('Azure OpenAI')).getByRole('button', { name: 'Sign in' }));
  const dialog = await screen.findByRole('dialog', { name: 'Sign in to Azure OpenAI' });
  const url = within(dialog).getByLabelText('Resource URL');
  expect(url).toHaveAttribute('placeholder', 'https://<resource>.openai.azure.com');
  const next = within(dialog).getByRole('button', { name: 'Continue' });
  for (const bad of ['fixture.openai.azure.com', 'ftp://fixture.openai.azure.com', 'https://user:pw@fixture.openai.azure.com']) {
    await userEvent.clear(url);
    await userEvent.type(url, bad);
    expect(next).toBeDisabled();
  }
  expect(callsOf('local_rpc_auth_start')).toHaveLength(0);
  await userEvent.clear(url);
  await userEvent.type(url, 'https://fixture.openai.azure.com');
  await userEvent.click(next);
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  expect(callsOf('local_rpc_auth_start')[0].args).toEqual({ request: {
    operation: 'login', providerId: 'azure', authType: 'api_key', resourceUrl: 'https://fixture.openai.azure.com' } });
  await emitNow('op-azure', { event: 'prompt', providerId: 'azure', authType: 'api_key', promptId: 'p-azure', credentialField: 'api_key.key',
    prompt: { type: 'secret', message: 'Enter Azure OpenAI API key' } });
  expect(within(dialog).getByLabelText('Enter Azure OpenAI API key')).toHaveAttribute('type', 'password');
});

it('lists the agents using a provider before signing out, then signs out', async () => {
  bus.handlers.set('local_rpc_auth_in_use', () => [
    { agentId: 'agent-copper', projectRoot: '/fixture/a' },
    { agentId: 'agent-gone', projectRoot: '/fixture/b' },
  ]);
  const detail = vi.spyOn(pty, 'loadProjectAgentDetail').mockImplementation(async ({ agentId }) => {
    if (agentId === 'agent-gone') throw new Error('missing');
    return { agentId, displayName: 'Copper' } as Awaited<ReturnType<typeof pty.loadProjectAgentDetail>>;
  });
  bus.handlers.set('local_rpc_auth_start', () => ({ operationId: 'op-out' }));
  await renderCard();
  await userEvent.click(within(row('OpenAI')).getByRole('button', { name: 'Sign out' }));
  const confirm = await screen.findByRole('alertdialog', { name: 'Sign out of OpenAI' });
  expect(callsOf('local_rpc_auth_in_use')[0].args).toEqual({ providerId: 'openai' });
  expect(detail).toHaveBeenCalledWith({ agentId: 'agent-copper', projectRoot: '/fixture/a' });
  expect(within(confirm).getByRole('heading')).toHaveTextContent('Sign out of OpenAI?');
  expect(within(confirm).getByText('Local agents using OpenAI:')).toBeInTheDocument();
  expect(within(confirm).getByText('Copper')).toBeInTheDocument();
  expect(within(confirm).getByText('agent-gone')).toBeInTheDocument();
  expect(within(confirm).getByText('After you sign out, remember to change their model.')).toBeInTheDocument();
  expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus();

  await userEvent.click(within(confirm).getByRole('button', { name: 'Sign out' }));
  await waitFor(() => expect(callsOf('local_rpc_auth_start')).toHaveLength(1));
  expect(callsOf('local_rpc_auth_start')[0].args).toEqual({ request: { operation: 'logout', providerId: 'openai' } });
  expect(row('OpenAI').querySelector('.local-auth-status-text')).toHaveTextContent('Signing out…');
  expect(within(row('OpenAI')).getByRole('button', { name: 'Signing out…' })).toBeDisabled();
  states.openai = { status: 'not_signed_in' };
  await emitNow('op-out', { event: 'result', operation: 'logout', providerId: 'openai', outcome: 'succeeded', commit: 'confirmed' });
  await waitFor(() => expect(within(row('OpenAI')).getByText('Not linked')).toBeInTheDocument());
  expect(bus.listeners.size).toBe(0);
});

it('signs out with the short note when no agent uses the provider, and Escape keeps it signed in', async () => {
  await renderCard();
  await userEvent.click(within(row('Anthropic')).getByRole('button', { name: 'Sign out' }));
  const confirm = await screen.findByRole('alertdialog', { name: 'Sign out of Anthropic' });
  expect(within(confirm).getByText('Kota Local agents can’t use Anthropic until you sign in again.')).toBeInTheDocument();
  expect(within(confirm).queryByText(/Local agents using/)).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  expect(callsOf('local_rpc_auth_start')).toHaveLength(0);
  expect(within(row('Anthropic')).getByText('Linked · API key')).toBeInTheDocument();
});

it('lands on the Local login card', async () => {
  const handled = vi.fn();
  await renderCard({ focusTarget: 'kota-local', onFocusHandled: handled });
  const card = document.getElementById('link-kota-local')!;
  expect(card).toHaveFocus();
  expect(card).toHaveClass('is-landing');
  expect(handled).toHaveBeenCalled();
  expect(document.getElementById('link-kota-local-jev')).toBeNull();
});

it('opens Puppeteer for Jev and keeps ordinary sign-in on Tavern Link', async () => {
  const previous = Object.getOwnPropertyDescriptor(window, 'localStorage');
  const values = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  try {
    bus.handlers.set('local_rpc_jev_read', () => ({schema:1, provider:'openrouter', ready:false, keys:[{providerId:'openrouter',status:'none'}]}));
    bus.handlers.set('puppeteer_jev_prompts', () => ({stringFirst:{},stringLater:{},danceContext:{},danceRule:{}}));
    vi.spyOn(pty, 'loadTavernHeroProfiles').mockResolvedValue([]);
    vi.spyOn(pty, 'saveTavernHeroProfiles').mockResolvedValue();
    render(<TavernModal open onClose={() => {}} />);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Back to room' })).toHaveFocus());
    expect(screen.queryByTestId('tavern-kota-local')).not.toBeInTheDocument();
    act(() => requestKotaLocalLink('kota-local-jev'));
    await waitFor(() => expect(document.getElementById('puppeteer-jev')).toHaveFocus());
    expect(screen.getByRole('button', { name: 'Hero' })).toHaveClass('active');
    expect(screen.queryByTestId('tavern-kota-local')).not.toBeInTheDocument();
    act(() => requestKotaLocalLink('kota-local'));
    await screen.findByTestId('tavern-kota-local');
    expect(screen.getByRole('button', { name: 'Link' })).toHaveClass('active');
    await waitFor(() => expect(document.getElementById('link-kota-local')).toHaveFocus());
    const github = screen.getByRole('region', { name: 'GitHub accounts' });
    await within(github).findByText('No GitHub accounts yet.');
    const cards = [...document.querySelectorAll('.tavern-pane.connections .tavern-section-title')].map((title) => title.textContent);
    expect(cards.slice(0, 2)).toEqual(['GitHub', 'Kota Local']);
  } finally {
    if (previous) Object.defineProperty(window, 'localStorage', previous);
    else Reflect.deleteProperty(window, 'localStorage');
  }
});

it('offers only linked providers but keeps every name in the catalog', () => {
  const full: LocalModelCatalog = { schema: 1, providers: [
    { providerId: 'openai', linked: true, models: [{ modelId: 'sol-1', name: 'Sol One', thinkingLevels: ['low'] }] },
    { providerId: 'anthropic', linked: false, models: [{ modelId: 'quill-2', name: 'Quill Two', thinkingLevels: ['high'] }] },
  ] };
  expect(linkedCatalogProviders(full).map((provider) => provider.providerId)).toEqual(['openai']);
  expect(linkedCatalogProviders(null)).toEqual([]);
  expect(catalogModel(full, 'anthropic', 'quill-2')?.name).toBe('Quill Two');
  expect(catalogModel(full, 'anthropic', 'missing')).toBeUndefined();
});

it('ports Pi thinking clamps and the Resource URL check', () => {
  expect(clampThinkingLevel(['low', 'medium', 'high'], 'medium')).toBe('medium');
  expect(clampThinkingLevel(['medium', 'high'], 'minimal')).toBe('medium');
  expect(clampThinkingLevel(['low', 'medium', 'high', 'xhigh'], 'max')).toBe('xhigh');
  expect(clampThinkingLevel(['off'], 'max')).toBe('off');
  expect(isLocalResourceUrl('https://fixture.openai.azure.com/openai/v1')).toBe(true);
  expect(isLocalResourceUrl('http://127.0.0.1:8080')).toBe(true);
  expect(isLocalResourceUrl('https://user@fixture.openai.azure.com')).toBe(false);
  expect(isLocalResourceUrl('fixture.openai.azure.com')).toBe(false);
});
