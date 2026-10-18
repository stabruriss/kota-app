import { useState } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { StringRoutingEditor } from '../src/chrome/StringRoutingEditor';
import { StringLocalModelPicker } from '../src/chrome/StringLocalModelPicker';
import { nextStringTemplateName, stringGroupToDraft, type AutoRouteGroup, type StringTemplateLibrary } from '../src/lib/string-routing';
import type { LocalModelCatalog } from '../src/lib/local-auth';

vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn() }));
const group: AutoRouteGroup = { threshold: .5, defaultModel: 'model-1', models: [
  { id: 'model-1', providerId: 'openai', modelId: 'sol', thinkingLevel: 'high', intent: 'Complex work.' },
  { id: 'model-2', providerId: 'openai', modelId: 'luna', thinkingLevel: 'low', intent: 'Quick work.' },
] };
const imported: AutoRouteGroup = { threshold: .8, defaultModel: 'model-2', models: [
  { ...group.models[0], providerId: 'magpie', modelId: 'anthropic/long-model:beta' },
  { ...group.models[1], providerId: 'anthropic', modelId: 'sonnet', thinkingLevel: 'off' },
] };
const catalog: LocalModelCatalog = { schema: 1, providers: [
  { providerId: 'openai', linked: true, models: [{ modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'high'] }, { modelId: 'luna', name: 'Luna', thinkingLevels: ['off', 'low'] }] },
  { providerId: 'anthropic', linked: false, models: [{ modelId: 'sonnet', name: 'Sonnet', thinkingLevels: ['off', 'high'] }] },
  { providerId: 'magpie', linked: false, models: [{ modelId: 'anthropic/long-model:beta', name: 'Long model', thinkingLevels: ['off', 'high'] }] },
] };
let library: StringTemplateLibrary;
beforeEach(() => {
  library = { schema: 1, revision: 'before', templates: [
    { id: 'existing-1', name: 'String 1', savedAt: '2026-01-01T00:00:00Z', group: structuredClone(imported) },
    { id: 'existing-3', name: 'String 3', savedAt: '2026-01-02T00:00:00Z', group: structuredClone(group) },
  ] };
  vi.mocked(invoke).mockReset().mockImplementation(async (command, args) => {
    if (command === 'puppeteer_string_templates_read') return structuredClone(library);
    if (command === 'puppeteer_string_templates_save') {
      library = { ...(args as { library: StringTemplateLibrary }).library, revision: 'after' }; return structuredClone(library);
    }
    throw new Error(`Unexpected IPC ${command}`);
  });
});
const writes = () => vi.mocked(invoke).mock.calls.filter(([name]) => name === 'puppeteer_string_templates_save');
function Editor({ onChange = vi.fn(), initial = group }: { onChange?: ReturnType<typeof vi.fn>; initial?: AutoRouteGroup }) {
  const [draft, setDraft] = useState(stringGroupToDraft(initial));
  return <StringRoutingEditor draft={draft} catalog={catalog} owner="Copper" surface="hero"
    onChange={value => { setDraft(value); onChange(value); }} onClose={vi.fn()} />;
}
async function saveCard() {
  await userEvent.click(screen.getByRole('button', { name: 'Save as template' }));
  const input = await screen.findByRole('textbox', { name: 'Template name' });
  await waitFor(() => expect(input).toBeEnabled()); return input;
}
async function importCard() {
  await userEvent.click(screen.getByRole('button', { name: 'Import' }));
  await screen.findByRole('article', { name: 'String 1' });
  return screen.getByRole('dialog', { name: 'Import template' });
}

it('loads only on demand and saves the complete screen draft under the smallest unused default name', async () => {
  render(<Editor />); expect(invoke).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('slider'), { target: { value: '.75' } });
  const input = await saveCard(); expect(input).toHaveValue('String 2');
  expect(nextStringTemplateName([{ ...library.templates[0], name: ' string 1 ' }])).toBe('String 2');
  await userEvent.clear(input); expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  fireEvent.change(input, { target: { value: '🧵'.repeat(41) } }); expect([...(input as HTMLInputElement).value]).toHaveLength(40);
  fireEvent.change(input, { target: { value: 'String 2' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Saved “String 2”.');
  expect(writes()).toHaveLength(1);
  const sent = (writes()[0][1] as { library: StringTemplateLibrary }).library;
  expect(sent.revision).toBe('before'); expect(sent.templates.at(-1)?.group).toEqual({ ...group, threshold: .75 });
  expect(screen.queryByRole('dialog', { name: 'Save template' })).not.toBeInTheDocument();
  expect(screen.getByRole('dialog', { name: 'Auto String' })).toBeVisible();
  expect(vi.mocked(invoke).mock.calls.every(([cmd]) => cmd.startsWith('puppeteer_string_templates_'))).toBe(true);
});

it('uses the backend duplicate identity for Rename and Overwrite, retaining the original id', async () => {
  const realSave = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation(async (cmd, args) => {
    if (cmd === 'puppeteer_string_templates_save' && (args as { library: StringTemplateLibrary }).library.templates.length > 2) throw { code: 'string_template_name_exists', duplicateId: 'existing-1' };
    return realSave(cmd, args);
  });
  render(<Editor />); const input = await saveCard();
  fireEvent.change(input, { target: { value: ' STRING 1 ' } });
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(await screen.findByRole('dialog', { name: 'Template already exists' })).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Rename' }));
  expect(screen.getByRole('textbox', { name: 'Template name' })).toHaveValue(' STRING 1 ');
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await userEvent.click(await screen.findByRole('button', { name: 'Overwrite' }));
  await screen.findByText('Saved “String 1”.');
  expect(library.templates).toHaveLength(2); expect(library.templates[0]).toMatchObject({ id: 'existing-1', name: 'String 1', group });
});

it('imports unlinked models with full IDs, replaces the draft and applies Credit only to direct Anthropic', async () => {
  const change = vi.fn(); render(<Editor onChange={change} />); const panel = await importCard();
  const card = within(panel).getByRole('article', { name: 'String 1' });
  expect(within(card).getAllByText('Not linked')).toHaveLength(2);
  expect(within(card).getAllByText('(Use Credit)')).toHaveLength(1);
  expect(within(card).getByText('Magpie / anthropic / Long model', { exact: false })).toBeVisible();
  await userEvent.click(within(card).getByRole('button', { name: 'Import' }));
  expect(change).toHaveBeenCalledOnce(); expect(change.mock.calls[0][0]).toEqual(stringGroupToDraft(imported));
  expect(screen.getByRole('slider')).toHaveValue('0.8'); expect(screen.getByRole('radio', { name: 'Default model 2' })).toBeChecked();
  expect(writes()).toHaveLength(0); expect(vi.mocked(invoke).mock.calls).toHaveLength(1);
});

it.each(['workbench', 'hero', 'profile'] as const)('Import respects the existing %s owner saving seam', async surface => {
  const save = vi.fn(); const stage = vi.fn();
  render(<StringLocalModelPicker ownerKey="fixture" owner="Copper" surface={surface} model="kota/auto" group={group} catalog={catalog}
    jev={{ settings: { schema: 1, provider: 'openrouter', keys: [], ready: true }, failed: false, loading: false }}
    onSelect={vi.fn()} onOpen={vi.fn()} onJevSettings={vi.fn()} onAutoRouteChange={surface === 'workbench' ? save : stage} />);
  await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
  await userEvent.click(screen.getByRole('option', { name: /Auto String/ }));
  const panel = await importCard(); await userEvent.click(within(within(panel).getByRole('article', { name: 'String 1' })).getByRole('button', { name: 'Import' }));
  if (surface === 'workbench') await waitFor(() => expect(save).toHaveBeenCalledWith(imported));
  else { expect(stage).toHaveBeenCalledWith(imported); expect(save).not.toHaveBeenCalled(); }
  expect(writes()).toHaveLength(0);
});

it('requires Reload after CAS conflict; no automatic retry or silently rebased write', async () => {
  const realSave = vi.mocked(invoke).getMockImplementation()!;
  let conflicted = false;
  vi.mocked(invoke).mockImplementation(async (cmd, args) => {
    if (cmd === 'puppeteer_string_templates_save' && !conflicted) { conflicted = true; library.revision = 'external'; throw { code: 'string_templates_conflict', secret: 'DO_NOT_RENDER' }; }
    return realSave(cmd, args);
  });
  render(<Editor />); await saveCard(); await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Settings changed outside this editor. Reload before saving.');
  expect(screen.queryByText('DO_NOT_RENDER')).not.toBeInTheDocument(); expect(writes()).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled());
  expect(writes()).toHaveLength(1);
  await userEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Saved “String 2”.');
  expect((writes()[1][1] as { library: StringTemplateLibrary }).library.revision).toBe('external');
});

it('deletes only after confirmation, confines Escape/focus, leaves source and earlier imports unchanged', async () => {
  const change = vi.fn(); render(<Editor onChange={change} />); const panel = await importCard();
  const card = within(panel).getByRole('article', { name: 'String 1' });
  await userEvent.click(within(card).getByRole('button', { name: 'Delete' }));
  const confirm = screen.getByRole('alertdialog'); expect(within(confirm).getByRole('button', { name: 'Cancel' })).toHaveFocus();
  expect(writes()).toHaveLength(0);
  await userEvent.keyboard('{Escape}'); expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument(); expect(panel).toBeVisible();
  await userEvent.click(within(card).getByRole('button', { name: 'Delete' }));
  await userEvent.click(screen.getByRole('button', { name: 'Delete template' }));
  await waitFor(() => expect(screen.queryByRole('article', { name: 'String 1' })).not.toBeInTheDocument());
  expect(library.templates.map(t => t.id)).toEqual(['existing-3']); expect(change).not.toHaveBeenCalled();
});

it('does not close or submit twice while a write is in flight', async () => {
  let finish!: (value: unknown) => void; const realSave = vi.mocked(invoke).getMockImplementation()!;
  vi.mocked(invoke).mockImplementation((cmd, args) => cmd === 'puppeteer_string_templates_save' ? new Promise(resolve => { finish = resolve; }) : realSave(cmd, args));
  render(<Editor />); await saveCard(); await userEvent.dblClick(screen.getByRole('button', { name: 'Save' }));
  expect(writes()).toHaveLength(1); await userEvent.keyboard('{Escape}');
  expect(screen.getByRole('dialog', { name: 'Save template' })).toBeVisible();
  await act(async () => finish({ ...library, revision: 'after' }));
  expect(screen.queryByRole('dialog', { name: 'Save template' })).not.toBeInTheDocument();
});

it('read errors never become an empty writable library; explicit Retry recovers', async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ code: 'string_templates_invalid' });
  render(<Editor />); await userEvent.click(screen.getByRole('button', { name: 'Save as template' }));
  await screen.findByText('String templates unavailable.');
  expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled(); expect(writes()).toHaveLength(0);
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(screen.getByRole('textbox', { name: 'Template name' })).toHaveValue('String 2'));
});


it.each(['Import', 'Save as template'])('returns focus to %s after WebKit-style pointer activation', async label => {
  render(<Editor />); screen.getByRole('dialog', { name: 'Auto String' }).focus();
  const trigger = screen.getByRole('button', { name: label }); fireEvent.click(trigger);
  await screen.findByRole('dialog', { name: label === 'Import' ? 'Import template' : 'Save template' });
  await userEvent.keyboard('{Escape}'); expect(trigger).toHaveFocus();
});
