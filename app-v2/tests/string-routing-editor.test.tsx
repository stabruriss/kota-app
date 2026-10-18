import { useState } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { newStringRoutingDraft, stringDraftComplete, StringRoutingEditor, type StringRoutingDraft } from '../src/chrome/StringRoutingEditor';
import type { LocalModelCatalog } from '../src/lib/local-auth';
const catalog: LocalModelCatalog = { schema: 1, providers: [
  { providerId: 'openai', linked: true, models: [
    { modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'low', 'high', 'max'] },
    { modelId: 'luna', name: 'Luna', thinkingLevels: ['off', 'low'] },
  ] },
  { providerId: 'anthropic', linked: false, models: [{ modelId: 'opus', name: 'Opus', thinkingLevels: ['low', 'high'] }] },
  { providerId: 'magpie', linked: true, models: [{ modelId: 'codex/qwen3:32b', name: 'Qwen', thinkingLevels: ['off', 'high'] }] },
] };
function Harness({ initial = newStringRoutingDraft(), onChange = vi.fn(), onClose = vi.fn(), onCatalogOpen = vi.fn(), surface = 'workbench' }: {
  initial?: StringRoutingDraft; onChange?: (draft: StringRoutingDraft) => void; onClose?: () => void; onCatalogOpen?: () => void; surface?: 'workbench' | 'hero' | 'profile';
}) {
  const [draft, setDraft] = useState(initial);
  return <StringRoutingEditor draft={draft} catalog={catalog} owner="Copper" surface={surface}
    onChange={value => { setDraft(value); onChange(value); }} onClose={onClose} onCatalogOpen={onCatalogOpen} />;
}
async function choose(index: number, name: RegExp) {
  await userEvent.click(screen.getByRole('button', { name: `Model ${index} model` }));
  await userEvent.click(screen.getByRole('option', { name }));
}
describe('String model card editing', () => {
  it('begins with two empty cards, original intents, first default and 0.50, without presets', () => {
    const initial = newStringRoutingDraft();
    render(<Harness initial={initial} />);
    expect(initial.threshold).toBe(.5);
    expect(initial.defaultKey).toBe(initial.cards[0].key);
    expect(initial.cards.every(card => card.model === null && card.thinking === null)).toBe(true);
    expect(screen.getByRole('slider', { name: 'String probability threshold' })).toHaveValue('0.5');
    expect(screen.getByRole('radio', { name: 'Default model 1' })).toBeChecked();
    expect(screen.getByRole('textbox', { name: 'Model 1 intent' })).toHaveValue('Complex work where quality matters, decisions, or work where a mistake is costly.');
    expect(screen.getByRole('textbox', { name: 'Model 2 intent' })).toHaveValue('Light work or simple lookups.');
    expect(screen.getByRole('combobox', { name: 'Model 1 thinking' })).toBeDisabled();
    expect(screen.getByText('The certainty must be higher than this to switch model.')).toBeVisible();
    expect(screen.queryByText(/preset/i)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'OK' })).toBeDisabled();
    expect(screen.getByText('Choose a model for Model 1.')).toBeVisible();
    expect(screen.getAllByRole('button', { name: 'Remove' }).every(button => button.hasAttribute('disabled'))).toBe(true);
  });
  it('uses the widened slider endpoints and fill while keeping complete low-threshold groups enabled', () => {
    const initial = newStringRoutingDraft();
    initial.cards = initial.cards.map((card, index) => ({ ...card, model: { providerId: 'openai', modelId: index ? 'luna' : 'sol' }, thinking: 'off' }));
    const change = vi.fn(); render(<Harness initial={initial} onChange={change} />);
    const slider = screen.getByRole('slider', { name: 'String probability threshold' });
    expect(slider).toHaveAttribute('min', '0.05'); expect(slider).toHaveAttribute('max', '0.95'); expect(slider).toHaveAttribute('step', '0.05');
    expect(slider).toHaveValue('0.5');
    expect(parseFloat(slider.style.getPropertyValue('--string-threshold-fill'))).toBeCloseTo(50);
    for (const [threshold, fill] of [[.05, 0], [.5, 50], [.95, 100]]) {
      fireEvent.change(slider, { target: { value: String(threshold) } });
      expect(slider).toHaveValue(String(threshold));
      expect(parseFloat(slider.style.getPropertyValue('--string-threshold-fill'))).toBeCloseTo(fill);
      expect(change.mock.lastCall![0].threshold).toBe(threshold);
      expect(screen.getByRole('button', { name: 'OK' })).toBeEnabled();
    }
  });
  it('expands searchable catalog in the card, clamps to native levels and keeps one dialog', async () => {
    const change = vi.fn(); render(<Harness onChange={change} />);
    const trigger = screen.getByRole('button', { name: 'Model 1 model' });
    await userEvent.click(trigger);
    const card = screen.getByRole('region', { name: 'Model 1' });
    expect(within(card).getByRole('searchbox')).toHaveFocus();
    expect(trigger).toHaveAttribute('aria-controls', within(card).getByRole('listbox').id);
    expect(screen.getAllByRole('dialog')).toHaveLength(1);
    expect(screen.queryByRole('option', { name: /Opus/ })).not.toBeInTheDocument();
    await userEvent.type(within(card).getByRole('searchbox'), 'sol');
    expect(within(within(card).getByRole('listbox')).getAllByRole('option')).toHaveLength(1);
    await userEvent.keyboard('{ArrowDown}{Enter}');
    expect(trigger).toHaveFocus();
    expect(within(card).queryByRole('listbox')).not.toBeInTheDocument();
    const thinking = screen.getByRole('combobox', { name: 'Model 1 thinking' });
    expect(within(thinking).getAllByRole('option').map(option => option.textContent)).toEqual(['off', 'low', 'high', 'max']);
    expect(thinking).toHaveValue('high'); // native next-higher clamp from medium
    await userEvent.selectOptions(thinking, 'max');
    await choose(1, /Luna/);
    expect(thinking).toHaveValue('low'); // native next-lower clamp
    expect(screen.getByText('Thinking adjusted to low for this model.')).toBeVisible();
    expect(stringDraftComplete(change.mock.lastCall![0])).toBe(false);
    await choose(2, /Magpie \/ codex \/ Qwen/);
    const last: StringRoutingDraft = change.mock.lastCall![0];
    expect(last.cards[1].model).toEqual({ providerId: 'magpie', modelId: 'codex/qwen3:32b' });
    expect(stringDraftComplete(last)).toBe(true);
    expect(screen.getByRole('button', { name: 'OK' })).toBeEnabled();
    expect(screen.getByText('Saved automatically.')).toBeVisible();
  });
  it('keeps Escape within inline menu, then lets OK close the editor', async () => {
    const close = vi.fn(), parentEscape = vi.fn();
    const onParentEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') parentEscape(); };
    document.addEventListener('keydown', onParentEscape, true);
    render(<Harness onClose={close} />);
    await userEvent.click(screen.getByRole('button', { name: 'Model 1 model' }));
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(screen.getByRole('dialog')).toBeVisible();
    expect(close).not.toHaveBeenCalled();
    expect(parentEscape).not.toHaveBeenCalled();
    document.removeEventListener('keydown', onParentEscape, true);
    await choose(1, /Sol OpenAI/);
    await choose(2, /Luna OpenAI/);
    await userEvent.click(screen.getByRole('button', { name: 'OK' }));
    expect(close).toHaveBeenCalledOnce();
  });
  it('limits 2–6 models without expanding added cards, caps Unicode intent and advances removed default', async () => {
    const change = vi.fn(), open = vi.fn();render(<Harness onChange={change} onCatalogOpen={open} />);
    for (let i = 3; i <= 6; i++) {
      await userEvent.click(screen.getByRole('button', { name: '+ Add model' }));
      expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
      expect(within(screen.getByRole('region', { name: `Model ${i}` })).getByRole('button', { name: `Model ${i} model` })).toHaveAttribute('aria-expanded', 'false');
    }
    expect(screen.getByText('6 / 6')).toBeVisible();
    expect(open).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Model 6 model' }));
    expect(open).toHaveBeenCalledOnce();
    expect(within(screen.getByRole('region', { name: 'Model 6' })).getByRole('listbox')).toBeVisible();
    await userEvent.keyboard('{Escape}');
    expect(screen.getByRole('button', { name: '+ Add model' })).toBeDisabled();
    const intent = screen.getByRole('textbox', { name: 'Model 6 intent' });
    await userEvent.click(intent);
    await userEvent.paste('🧵'.repeat(220));
    expect([...(intent as HTMLTextAreaElement).value].length).toBe(200);
    expect([...change.mock.lastCall![0].cards[5].intent].length).toBe(200);
    await userEvent.click(screen.getByRole('radio', { name: 'Default model 3' }));
    await userEvent.click(within(screen.getByRole('region', { name: 'Model 3' })).getByRole('button', { name: 'Remove' }));
    expect(screen.getByRole('radio', { name: 'Default model 1' })).toBeChecked();
    expect(change.mock.lastCall![0].cards).toHaveLength(5);
  });
  it('disables OK again for a blank intent and shows its reason without submitting an incomplete group', async () => {
    const close = vi.fn(); render(<Harness onClose={close} surface="hero" />);
    await choose(1, /Sol OpenAI/);
    expect(screen.getByText('Choose a model for Model 2.')).toBeVisible();
    await choose(2, /Luna OpenAI/);
    const ok = screen.getByRole('button', { name: 'OK' });
    expect(ok).toBeEnabled();
    await userEvent.clear(screen.getByRole('textbox', { name: 'Model 2 intent' }));
    expect(ok).toBeDisabled();
    expect(screen.getByText('Add an intent description for Model 2.')).toBeVisible();
    await userEvent.click(ok);
    expect(close).not.toHaveBeenCalled();
  });
  it('grows an intent input as its content grows', () => {
    const height = vi.spyOn(HTMLTextAreaElement.prototype, 'scrollHeight', 'get').mockImplementation(function (this: HTMLTextAreaElement) {
      return this.value.length > 100 ? 120 : 60;
    });
    try {
      render(<Harness />);
      const input = screen.getByRole('textbox', { name: 'Model 1 intent' });
      expect(input).toHaveStyle({ height: '62px' });
      fireEvent.change(input, { target: { value: 'A long intent. '.repeat(10) } });
      expect(input).toHaveStyle({ height: '122px' });
      fireEvent.change(input, { target: { value: 'Short intent.' } });
      expect(input).toHaveStyle({ height: '62px' });
    } finally { height.mockRestore(); }
  });
  it.each(['hero', 'profile'] as const)('keeps %s editing instructions tied to the existing Save action', surface => {
    render(<Harness surface={surface} />);
    expect(screen.getByText(`Click OK, then ${surface === 'hero' ? 'Save Shell' : 'Save'}.`)).toBeVisible();
    expect(screen.queryByText('Saved automatically.')).not.toBeInTheDocument();
  });
});
