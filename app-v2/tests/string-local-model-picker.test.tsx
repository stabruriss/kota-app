import { useState } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { StringLocalModelPicker } from '../src/chrome/StringLocalModelPicker';
import type { AutoRouteGroup } from '../src/lib/string-routing';
import type { LocalModelCatalog } from '../src/lib/local-auth';
import type { JevSettings } from '../src/lib/jev';

const group = (): AutoRouteGroup => ({ threshold: .85, defaultModel: 'model-1', models: [
  { id: 'model-1', providerId: 'openai', modelId: 'sol', thinkingLevel: 'high', intent: 'Hard tasks.' },
  { id: 'model-2', providerId: 'openai', modelId: 'luna', thinkingLevel: 'off', intent: 'Easy tasks.' },
] });
const catalog: LocalModelCatalog = { schema: 1, providers: [{ providerId: 'openai', linked: true, models: [
  { modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'high', 'max'] },
  { modelId: 'luna', name: 'Luna', thinkingLevels: ['off', 'low'] },
] }] };
const verified: JevSettings = { schema: 1, provider: 'openrouter', keys: [{ providerId: 'openrouter', status: 'verified' }], ready: true };
const jev = { settings: verified, loading: false, failed: false };
function Harness({ surface = 'workbench', initial = group(), save = vi.fn(async (value: AutoRouteGroup) => value), ownerKey = 'copper' }: {
  surface?: 'workbench' | 'hero' | 'profile'; initial?: AutoRouteGroup | null;
  save?: (value: AutoRouteGroup) => Promise<AutoRouteGroup>; ownerKey?: string;
}) {
  const [saved, setSaved] = useState(initial);
  const [model, setModel] = useState('openai/sol');
  const [valid, setValid] = useState(true);
  return <><StringLocalModelPicker ownerKey={ownerKey} owner="Copper" surface={surface} model={model} group={saved}
    catalog={catalog} jev={jev} onValidityChange={setValid} onSelect={choice => setModel(`${choice.providerId}/${choice.modelId}`)}
    onAutoRouteChange={surface === 'workbench' ? async value => { const next = await save(value); setSaved(next); setModel('kota/auto'); return next; }
      : value => { setSaved(value); setModel('kota/auto'); }} onOpen={vi.fn()} onJevSettings={vi.fn()} />
    <output aria-label="Selected model">{model}</output><button disabled={!valid}>Save owner</button></>;
}
async function edit() {
  await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
  await userEvent.click(screen.getByRole('option', { name: /Auto String/ }));
}
afterEach(() => { cleanup(); vi.useRealTimers(); });
describe('String owner save behavior', () => {
  it.each(['profile', 'hero', 'workbench'] as const)('only removes redundant external details on the profile surface (%s)', async surface => {
    render(<StringLocalModelPicker ownerKey="fixture" owner="Copper" surface={surface} model="kota/auto" group={group()}
      catalog={catalog} jev={jev} onSelect={vi.fn()} onAutoRouteChange={vi.fn()} onOpen={vi.fn()} onJevSettings={vi.fn()} />);
    expect(!!screen.queryByRole('button', { name: 'Edit routing' })).toBe(surface !== 'profile');
    expect(!!screen.queryByLabelText('Auto String configuration')).toBe(surface === 'hero');
    await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
    const option = screen.getByRole('option', { name: /Auto String/ });
    expect(option).toHaveTextContent('Sol · high');
    expect(option).toHaveTextContent('Luna · off');
    expect(option).toHaveTextContent('Edit routing');
    await userEvent.click(option);
    expect(screen.getByRole('dialog', { name: 'Auto String' })).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Model 1 intent' })).toHaveValue('Hard tasks.');
  });
  it.each(['Escape', 'close button', 'backdrop', 'OK'])('returns focus to the model trigger after the menu-to-editor transition (%s)', async action => {
    render(<Harness surface="hero" />);
    const trigger = screen.getByRole('button', { name: 'Local model' });
    await edit();
    const editor = screen.getByRole('dialog', { name: 'Auto String' });
    if (action === 'Escape') await userEvent.keyboard('{Escape}');
    else if (action === 'backdrop') fireEvent.pointerDown(editor.parentElement!);
    else await userEvent.click(within(editor).getByRole('button', { name: action === 'OK' ? 'OK' : 'Close model picker' }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
  it.each([false, true])('waits for the pending save before restoring focus without stealing later user focus (%s)', async moveFocus => {
    let finish!: (value: AutoRouteGroup) => void;
    const save = vi.fn(() => new Promise<AutoRouteGroup>(resolve => { finish = resolve; }));
    render(<Harness save={save} />);
    const trigger = screen.getByRole('button', { name: 'Local model' });
    // Native browsers ignore focus() on disabled buttons; happy-dom does not.
    const focus = trigger.focus.bind(trigger);
    vi.spyOn(trigger, 'focus').mockImplementation(() => { if (!(trigger as HTMLButtonElement).disabled) focus(); });
    await edit();
    await userEvent.keyboard('{Escape}');
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(trigger).toBeDisabled();
    const elsewhere = screen.getByRole('button', { name: 'Save owner' });
    if (moveFocus) await userEvent.click(elsewhere);
    await act(async () => finish(group()));
    expect(trigger).toBeEnabled();
    expect(moveFocus ? elsewhere : trigger).toHaveFocus();
  });
  it('does not change the original model while the initial two cards are incomplete', async () => {
    const save = vi.fn(async (value: AutoRouteGroup) => value);
    render(<Harness initial={null} save={save} />); await edit();
    await userEvent.click(screen.getByRole('button', { name: 'Model 1 model' }));
    await userEvent.click(screen.getByRole('option', { name: /Sol/ }));
    expect(screen.getByLabelText('Selected model')).toHaveTextContent('openai/sol');
    expect(save).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'OK' })).toBeDisabled();
    expect(screen.getByText('Choose a model for Model 2.')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Close model picker' }));
    expect(screen.getByRole('button', { name: 'Save owner' })).toBeEnabled();
    expect(save).not.toHaveBeenCalled();
  });
  it.each(['hero', 'profile'] as const)('stages %s changes without a save request and retains the group across concrete selections', async surface => {
    const save = vi.fn(async (value: AutoRouteGroup) => value);
    render(<Harness surface={surface} save={save} />); await edit();
    fireEvent.change(screen.getByRole('slider'), { target: { value: '0.9' } });
    expect(screen.getByLabelText('Selected model')).toHaveTextContent('kota/auto');
    expect(screen.getByText('Unsaved changes')).toBeVisible();
    expect(save).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'OK' }));
    await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
    await userEvent.click(screen.getByRole('option', { name: /Luna OpenAI/ }));
    expect(screen.getByLabelText('Selected model')).toHaveTextContent('openai/luna');
    await edit();
    expect(screen.getByRole('slider')).toHaveValue('0.9');
    expect(screen.getByRole('textbox', { name: 'Model 1 intent' })).toHaveValue('Hard tasks.');
    expect(save).not.toHaveBeenCalled();
  });
  it('coalesces rapid edits and serializes saves; an older native clamp cannot overwrite newer edits', async () => {
    let resolve!: (value: AutoRouteGroup) => void;
    const save = vi.fn().mockImplementationOnce(() => new Promise<AutoRouteGroup>(done => { resolve = done; }))
      .mockImplementation(async (value: AutoRouteGroup) => value);
    render(<Harness save={save} />); await edit();
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
    fireEvent.change(screen.getByRole('slider'), { target: { value: '0.9' } });
    fireEvent.change(screen.getByRole('slider'), { target: { value: '0.95' } });
    await act(async () => { await vi.advanceTimersByTimeAsync(250); });
    expect(save).toHaveBeenCalledTimes(1); expect(save.mock.calls[0][0].threshold).toBe(.95);
    fireEvent.change(screen.getByRole('combobox', { name: 'Model 1 thinking' }), { target: { value: 'max' } });
    fireEvent.change(screen.getByRole('textbox', { name: 'Model 1 intent' }), { target: { value: 'Newer edit.' } });
    await act(async () => { resolve({ ...group(), threshold: .95 }); });
    expect(save).toHaveBeenCalledTimes(2);
    expect(save.mock.calls[1][0].models[0]).toMatchObject({ thinkingLevel: 'max', intent: 'Newer edit.' });
    expect(screen.getByRole('combobox', { name: 'Model 1 thinking' })).toHaveValue('max');
    expect(screen.getByRole('textbox', { name: 'Model 1 intent' })).toHaveValue('Newer edit.');
    expect(screen.getByText('Saved automatically.')).toBeVisible();
  });
  it.each([
    ['jev_key_rejected', 'Jev key rejected. Update it in Puppeteer.'],
    ['auto_route_invalid', 'Auto String settings are incomplete or invalid.'],
  ])('keeps the old model on %s and shows the controlled error without raw exception data', async (code, message) => {
    const save = vi.fn().mockRejectedValue({ code, secret: 'DO_NOT_RENDER' });
    render(<Harness save={save} />); await edit();
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    expect(screen.getByText(message)).toBeVisible();
    expect(screen.queryByText(/DO_NOT_RENDER/)).not.toBeInTheDocument();
    expect(screen.getByLabelText('Selected model')).toHaveTextContent('openai/sol');
    expect(screen.queryByText('Saved automatically.')).not.toBeInTheDocument();
  });
  it('ignores a late failed save from the previous owner', async () => {
    let reject!: (value: unknown) => void;
    const save = vi.fn(() => new Promise<AutoRouteGroup>((_done, fail) => { reject = fail; }));
    const view = render(<Harness save={save} />); await edit();
    await waitFor(() => expect(save).toHaveBeenCalledOnce());
    view.rerender(<Harness ownerKey="silver" save={save} />);
    await act(async () => { reject({ code: 'jev_key_rejected' }); });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByText('Jev key rejected. Update it in Puppeteer.')).not.toBeInTheDocument();
  });
  it('does not put a Jev read error under an ordinary selected model, and keeps ordinary choices usable', async () => {
    render(<StringLocalModelPicker ownerKey="fixture" owner="Copper" surface="hero" model="openai/sol" catalog={catalog}
      jev={{ settings: null, loading: false, failed: true }} onSelect={vi.fn()} onAutoRouteChange={vi.fn()}
      onOpen={vi.fn()} onJevSettings={vi.fn()} />);
    expect(screen.queryByText('Jev settings unavailable. Retry in Puppeteer.')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
    expect(screen.getByRole('option', { name: /Auto String/ })).toBeDisabled();
    expect(within(screen.getByRole('listbox')).getByRole('option', { name: /Sol OpenAI/ })).toBeEnabled();
    expect(screen.getByText('Jev settings unavailable. Retry in Puppeteer.')).toBeVisible();
  });
});
