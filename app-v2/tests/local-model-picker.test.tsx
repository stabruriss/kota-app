import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { LocalModelPicker, localCatalogChoices, localModelKey } from '../src/chrome/LocalModelPicker';
import type { LocalModelCatalog } from '../src/lib/local-auth';

const catalog: LocalModelCatalog = { schema: 1, providers: [
  { providerId: 'openai', linked: true, models: [{ modelId: 'sol', name: 'Sol', thinkingLevels: ['off', 'high'] }] },
  { providerId: 'anthropic', linked: false, models: [{ modelId: 'opus', name: 'Opus', thinkingLevels: ['low', 'high'] }] },
  { providerId: 'magpie', linked: true, models: [
    { modelId: 'copilot/gpt:beta', name: 'GPT', thinkingLevels: ['low', 'high'] },
    { modelId: 'codex/gpt:beta', name: 'GPT', thinkingLevels: ['low', 'high'] },
  ] },
] };
function setup(ready = true, rejected = false) {
  const onSelect = vi.fn(), onOpen = vi.fn(), onEdit = vi.fn(), onSettings = vi.fn();
  render(<LocalModelPicker value="openai/sol" choices={localCatalogChoices(catalog)} onSelect={onSelect} onOpen={onOpen}
    autoRoute={{ ready, rejected, onEdit, onSettings }} />);
  return { onSelect, onOpen, onEdit, onSettings };
}
describe('Local model picker shared catalog', () => {
  it('only lists linked concrete models, preserving Magpie upstream IDs and names', async () => {
    expect(localCatalogChoices(catalog).map(localModelKey)).toEqual(['openai/sol', 'magpie/copilot/gpt:beta', 'magpie/codex/gpt:beta']);
    const callbacks = setup();
    await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
    const dialog = screen.getByRole('dialog');
    expect(within(dialog).queryByRole('option', { name: /Opus/ })).not.toBeInTheDocument();
    expect(within(dialog).getByRole('option', { name: /Magpie \/ copilot \/ GPT/ })).toBeVisible();
    expect(within(dialog).getByRole('option', { name: /Magpie \/ codex \/ GPT/ })).toBeVisible();
    await userEvent.type(screen.getByRole('searchbox'), 'codex');
    expect(screen.getAllByRole('option')).toHaveLength(1);
    await userEvent.keyboard('{ArrowDown}{Enter}');
    expect(callbacks.onSelect).toHaveBeenCalledWith({ providerId: 'magpie', modelId: 'codex/gpt:beta', name: 'Magpie / codex / GPT' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(callbacks.onOpen).toHaveBeenCalledTimes(1);
  });
  it.each([[false, false, 'Set up Jev to use Auto String'], [false, true, 'Jev key rejected. Update it in Puppeteer.']] as const)
  ('blocks Auto String for missing/rejected key while normal models remain usable (%s %s)', async (ready, rejected, note) => {
    const callbacks = setup(ready, rejected);
    await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
    expect(screen.getByRole('option', { name: /Auto String/ })).toBeDisabled();
    expect(screen.getByText(note)).toBeVisible();
    expect(screen.getByRole('option', { name: /Sol OpenAI/ })).toBeEnabled();
    await userEvent.click(screen.getByRole('button', { name: 'Jev settings' }));
    expect(callbacks.onEdit).not.toHaveBeenCalled();
    expect(callbacks.onSettings).toHaveBeenCalledOnce();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it('opens per-owner Auto String editing and restores trigger focus on Escape', async () => {
    const callbacks = setup();
    const trigger = screen.getByRole('button', { name: 'Local model' });
    await userEvent.click(trigger);
    expect(screen.getByRole('searchbox')).toHaveFocus();
    await userEvent.keyboard('{Escape}');
    expect(trigger).toHaveFocus();
    await userEvent.click(trigger);
    await userEvent.click(screen.getByRole('option', { name: /Auto String/ }));
    expect(callbacks.onEdit).toHaveBeenCalledOnce();
    expect(callbacks.onSelect).not.toHaveBeenCalled();
  });
  it('keeps wheel and touch events inside the popup instead of forwarding them to its parent', async () => {
    const wheel = vi.fn(), touch = vi.fn();
    render(<div onWheel={wheel} onTouchMove={touch}><LocalModelPicker value="openai/sol" choices={localCatalogChoices(catalog)} onSelect={vi.fn()} /></div>);
    await userEvent.click(screen.getByRole('button', { name: 'Local model' }));
    const list = screen.getByRole('listbox');
    fireEvent.wheel(list, { deltaY: 500 });
    fireEvent.touchMove(list);
    expect(wheel).not.toHaveBeenCalled();
    expect(touch).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog')).toBeVisible();
  });
});
