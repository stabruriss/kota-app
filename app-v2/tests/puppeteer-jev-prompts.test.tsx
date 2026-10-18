import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { PuppeteerJevPrompts } from '../src/chrome/PuppeteerJevPrompts';
import * as jev from '../src/lib/jev';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.mocked(invoke).mockReset(); });
const prompts = () => ({
  stringFirst: { questions: { model: { type: 'choice', instructions: 'First question: state.models', criteria: { '<model_id>': '<intent>' } } } },
  stringLater: { questions: { model: { type: 'choice', instructions: 'Later question: state.current_model', criteria: { '<model_id>': '<intent>' } } } },
  danceContext: { questions: { target: { type: 'choice', instructions: 'Context question: state.roster', criteria: { '<agent_id>': 'Route to <agent_id> in state.roster.' } } } },
  danceRule: { questions: { target: { type: 'choice', instructions: 'Rule question: state.roster', criteria: { '<agent_id>': '<condition>' } } } },
});
it('shows the four backend questions unchanged through the narrow read-only IPC', async () => {
  const value = prompts(); vi.mocked(invoke).mockResolvedValue(value);
  const { container } = render(<PuppeteerJevPrompts />);
  await waitFor(() => expect(container.querySelector('pre')).not.toBeNull());
  expect(JSON.parse(container.querySelector('pre')!.textContent!)).toEqual(value.stringFirst);
  for (const [id, question] of Object.entries(value)) {
    await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Jev prompt template' }), id);
    expect(JSON.parse(container.querySelector('pre')!.textContent!)).toEqual(question);
  }
  expect(invoke).toHaveBeenCalledExactlyOnceWith('puppeteer_jev_prompts');
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: /Save|Reset/ })).not.toBeInTheDocument();
});
it('keeps failure neutral and retries without rendering backend exception contents', async () => {
  vi.spyOn(jev, 'readPuppeteerJevPrompts').mockRejectedValueOnce({ secret: 'DO_NOT_RENDER' }).mockResolvedValue(prompts());
  const { container } = render(<PuppeteerJevPrompts />);
  expect(await screen.findByRole('status')).toHaveTextContent('Prompts unavailable.');
  expect(screen.queryByText(/DO_NOT_RENDER/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(container.querySelector('pre')).not.toBeNull());
  expect(jev.readPuppeteerJevPrompts).toHaveBeenCalledTimes(2);
});
it('does not revive a closed prompt panel when a read finishes late', async () => {
  let finish!: (value: jev.PuppeteerJevPrompts) => void;
  vi.spyOn(jev, 'readPuppeteerJevPrompts').mockImplementation(() => new Promise(done => { finish = done; }));
  const view = render(<PuppeteerJevPrompts />); view.unmount();
  await act(async () => { finish(prompts()); });
  expect(screen.queryByRole('region', { name: 'Jev prompt' })).not.toBeInTheDocument();
});
