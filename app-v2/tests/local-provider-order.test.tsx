import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { TavernModal } from '../src/chrome/TavernModal';
import * as pty from '../src/pty-client';
import * as local from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';

vi.mock('../src/lib/local-rpc', async (original) => ({
  ...await original<typeof import('../src/lib/local-rpc')>(),
  KOTA_LOCAL_ENABLED: true,
  discoverLocal: vi.fn(),
}));
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it.each([
  ['codex', 'Codex', ['codex', 'pi']],
  ['pi', 'Pi', ['pi', 'kimi']],
] as const)('keeps %s as the first installed terminal default after selecting Local without discovery', async (providerId, providerName, installed) => {
  const previous = Object.getOwnPropertyDescriptor(window, 'localStorage');
  const values = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  try {
    const statuses = await pty.supportedShellsStatus();
    vi.spyOn(pty, 'supportedShellsStatus').mockResolvedValue(statuses.map((entry) => ({
      ...entry, installed: (installed as readonly string[]).includes(entry.id),
    })));
    vi.spyOn(pty, 'loadTavernHeroProfiles').mockResolvedValue([]);
    vi.spyOn(pty, 'saveTavernHeroProfiles').mockResolvedValue();
    const availability = vi.spyOn(auth, 'readLocalAuthAvailability').mockResolvedValue({
      schema: 1, summary: 'ready', providers: [{ providerId: 'openai', status: 'signed_in', authType: 'api_key' }],
    });
    vi.spyOn(auth, 'readLocalModelCatalog').mockResolvedValue({ schema: 1, providers: [{providerId:'openai',linked:true,models:[{modelId:'fixture-model',name:'Fixture model',thinkingLevels:['off']}]}] });
    render(<TavernModal open onClose={() => {}} />);
    const status = within(screen.getByRole('region', { name: "Providers' Status" }));
    await waitFor(() => expect(status.getByRole('link', { name: providerName === 'Pi' ? /^Pi\b/ : providerName })).toHaveClass('ready'));
    const localStatus = status.getByRole('status', { name: 'Kota Local availability' });
    await waitFor(() => expect(localStatus).toHaveTextContent('Ready'));
    expect(availability).toHaveBeenCalledTimes(1);
    expect(local.discoverLocal).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole('button', { name: 'New Hero create template' }));
    const first = within(screen.getByRole('dialog', { name: 'New Hero profile' }));
    expect(first.getByRole('button', { name: `Provider ${providerName}` })).toHaveClass(`provider-${providerId}`);
    await userEvent.click(first.getByRole('button', { name: 'Edit Shell' }));
    await userEvent.click(first.getByRole('button', { name: `Provider ${providerName}` }));
    const localOption = first.getByRole('option', { name: /^Kota Local\b/ });
    expect(localOption).toBeEnabled();
    await userEvent.click(localOption);
    await waitFor(() => expect(availability).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(localStatus).toHaveTextContent('Ready'));
    expect(first.getByRole('button', { name: 'Provider Kota Local' })).toBeInTheDocument();
    expect(first.getByRole('button', { name: 'Save Shell' })).toBeDisabled();
    const localModel = first.getByRole('button', {name:'Local model'});
    await userEvent.click(localModel);
    await userEvent.click(screen.getByRole('option', { name: /Fixture model OpenAI/ }));
    await userEvent.click(first.getByRole('button', { name: 'Save Shell' }));
    await waitFor(() => expect(first.getByRole('button', { name: 'Provider Kota Local' })).toBeDisabled());
    expect(local.discoverLocal).not.toHaveBeenCalled();
    await userEvent.click(first.getByRole('button', { name: 'Back' }));

    await userEvent.click(screen.getByRole('button', { name: 'New Hero create template' }));
    const next = within(screen.getByRole('dialog', { name: /^New Hero.* profile$/ }));
    expect(next.getByRole('button', { name: `Provider ${providerName}` })).toHaveClass(`provider-${providerId}`);
    await userEvent.click(next.getByRole('button', { name: 'Edit Shell' }));
    await userEvent.click(next.getByRole('button', { name: `Provider ${providerName}` }));
    expect(next.getAllByRole('option').at(-1)).toHaveTextContent('Kota Local');
    expect(next.getByRole('option', { name: /^Kota Local\b/ })).toBeEnabled();
    expect(local.discoverLocal).not.toHaveBeenCalled();
    expect(pty.supportedShellsStatus).toHaveBeenCalledTimes(1);
  } finally {
    if (previous) Object.defineProperty(window, 'localStorage', previous);
    else Reflect.deleteProperty(window, 'localStorage');
  }
});
