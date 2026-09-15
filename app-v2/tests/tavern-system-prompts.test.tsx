import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TavernModal } from '../src/chrome/TavernModal';
import {
  EMBER_DREAM_AGENT_PROMPT_PATH,
  EMBER_DREAM_CONSOLIDATE_PROMPT_PATH,
} from '../src/ember-config';
import * as ptyClient from '../src/pty-client';
import { VIOLET_SUMMARY_PROMPT_PATH } from '../src/violet-summary-config';

const VIOLET_A = 'Violet prompt A';
const VIOLET_B = 'Violet prompt B';
const EMBER_AGENT_B = 'Ember agent prompt B';
const EMBER_CONSOLIDATE_B = 'Ember consolidation prompt B';
let originalStorage: PropertyDescriptor | undefined;

beforeEach(() => {
  const storage = new Map<string, string>();
  originalStorage = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => storage.get(key) ?? null,
      setItem: (key: string, value: string) => storage.set(key, String(value)),
      removeItem: (key: string) => storage.delete(key),
    },
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  if (originalStorage) Object.defineProperty(window, 'localStorage', originalStorage);
});

describe('Tavern system prompt refresh', () => {
  it('rereads Violet on reopen and both Ember prompt files on hero selection', async () => {
    const contents = new Map<string, string>([
      [VIOLET_SUMMARY_PROMPT_PATH, VIOLET_A],
      [EMBER_DREAM_AGENT_PROMPT_PATH, 'Ember agent prompt A'],
      [EMBER_DREAM_CONSOLIDATE_PROMPT_PATH, 'Ember consolidation prompt A'],
    ]);
    mockPromptFiles(contents);
    const onClose = vi.fn();
    const view = render(<TavernModal open onClose={onClose} />);

    await openSystemHero('Violet');
    let dialog = screen.getByRole('dialog', { name: /Violet profile/i });
    expect(await within(dialog).findByLabelText(/^Prompt/)).toHaveValue(VIOLET_A);

    contents.set(VIOLET_SUMMARY_PROMPT_PATH, VIOLET_B);
    view.rerender(<TavernModal open={false} onClose={onClose} />);
    view.rerender(<TavernModal open onClose={onClose} />);

    dialog = await screen.findByRole('dialog', { name: /Violet profile/i });
    await waitFor(() => expect(within(dialog).getByLabelText(/^Prompt/)).toHaveValue(VIOLET_B));

    contents.set(EMBER_DREAM_AGENT_PROMPT_PATH, EMBER_AGENT_B);
    contents.set(EMBER_DREAM_CONSOLIDATE_PROMPT_PATH, EMBER_CONSOLIDATE_B);
    await userEvent.click(within(dialog).getByRole('button', { name: 'Back' }));
    await userEvent.click(screen.getByRole('button', { name: /Ember/i }));

    dialog = screen.getByRole('dialog', { name: /Ember profile/i });
    await waitFor(() => {
      expect(within(dialog).getByLabelText(/^Dream Agent Prompt/)).toHaveValue(EMBER_AGENT_B);
      expect(within(dialog).getByLabelText(/^Dream Consolidation Prompt/)).toHaveValue(EMBER_CONSOLIDATE_B);
    });
  });

  it('keeps the previous prompt visible without a loading placeholder while rereading', async () => {
    const contents = new Map<string, string>([[VIOLET_SUMMARY_PROMPT_PATH, VIOLET_A]]);
    let refreshGate: ReturnType<typeof deferred> | null = null;
    const promptReader = mockPromptFiles(contents, () => refreshGate?.promise);
    const onClose = vi.fn();
    const view = render(<TavernModal open onClose={onClose} />);

    await openSystemHero('Violet');
    let dialog = screen.getByRole('dialog', { name: /Violet profile/i });
    expect(await within(dialog).findByLabelText(/^Prompt/)).toHaveValue(VIOLET_A);
    const callsBeforeReopen = promptReader.mock.calls.length;

    refreshGate = deferred();
    contents.set(VIOLET_SUMMARY_PROMPT_PATH, VIOLET_B);
    view.rerender(<TavernModal open={false} onClose={onClose} />);
    view.rerender(<TavernModal open onClose={onClose} />);
    await waitFor(() => expect(promptReader.mock.calls.length).toBeGreaterThan(callsBeforeReopen));

    dialog = screen.getByRole('dialog', { name: /Violet profile/i });
    expect(within(dialog).getByLabelText(/^Prompt/)).toHaveValue(VIOLET_A);
    expect(within(dialog).queryByText('Loading Prompts')).not.toBeInTheDocument();

    refreshGate.resolve();
    await waitFor(() => expect(within(dialog).getByLabelText(/^Prompt/)).toHaveValue(VIOLET_B));
  });

  it('retains the previous prompt and reports an error when rereading fails', async () => {
    const contents = new Map<string, string>([[VIOLET_SUMMARY_PROMPT_PATH, VIOLET_A]]);
    let refreshError: Error | null = null;
    mockPromptFiles(contents, () => refreshError ? Promise.reject(refreshError) : undefined);
    const onClose = vi.fn();
    const view = render(<TavernModal open onClose={onClose} />);

    await openSystemHero('Violet');
    let dialog = screen.getByRole('dialog', { name: /Violet profile/i });
    expect(await within(dialog).findByLabelText(/^Prompt/)).toHaveValue(VIOLET_A);

    refreshError = new Error('prompt refresh failed');
    view.rerender(<TavernModal open={false} onClose={onClose} />);
    view.rerender(<TavernModal open onClose={onClose} />);

    expect(await screen.findByText('Error: prompt refresh failed')).toBeInTheDocument();
    dialog = screen.getByRole('dialog', { name: /Violet profile/i });
    expect(within(dialog).getByLabelText(/^Prompt/)).toHaveValue(VIOLET_A);
    expect(within(dialog).queryByText('Loading Prompts')).not.toBeInTheDocument();
  });
});

async function openSystemHero(name: string): Promise<void> {
  await userEvent.click(screen.getByRole('button', { name: /System Heros/i }));
  await userEvent.click(screen.getByRole('button', { name: new RegExp(name, 'i') }));
}

function mockPromptFiles(
  contents: ReadonlyMap<string, string>,
  beforeRead?: () => Promise<void> | undefined,
) {
  return vi.spyOn(ptyClient, 'readSystemPrompt').mockImplementation(async (request, fallback) => {
    await beforeRead?.();
    return {
      path: request.path,
      content: contents.get(request.path) ?? fallback,
    };
  });
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
