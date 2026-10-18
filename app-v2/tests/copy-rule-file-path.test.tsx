import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { CopyRuleFilePath } from '../src/chrome/CopyRuleFilePath';
import { StringRoutingEditor, newStringRoutingDraft } from '../src/chrome/StringRoutingEditor';
import { StringLocalModelPicker } from '../src/chrome/StringLocalModelPicker';
import { DanceSettingsEditor } from '../src/chrome/PuppeteerDanceCard';
import * as client from '../src/pty-client';
import * as jev from '../src/lib/jev';
vi.mock('@tauri-apps/api/core', async original => ({ ...await original<typeof import('@tauri-apps/api/core')>(), invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
beforeEach(() => {
  vi.mocked(invoke).mockReset().mockImplementation(async command => command === 'local_rpc_jev_read'
    ? { schema: 1, provider: 'openrouter', ready: false, keys: [] }
    : { path: '/fixture/project-memory/puppeteer-dance.json' });
  vi.spyOn(client, 'violetCopyFilePath').mockResolvedValue(undefined);
  vi.spyOn(jev, 'readJevSettings').mockResolvedValue({ schema: 1, provider: 'openrouter', ready: false, keys: [] });
});
afterEach(() => vi.restoreAllMocks());
it('prepares once before copying the returned absolute path, leaves dirty values alone and refreshes afterwards', async () => {
  let finish!: (value: { path: string }) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const refresh = vi.fn();
  render(<CopyRuleFilePath target={{ projectRoot: '/other-project', kind: 'string', agentId: 'agent-a' }} dirty onPrepared={refresh} />);
  const button = screen.getByRole('button', { name: 'Copy rule file path' });
  expect(button).toHaveTextContent(/^$/);
  expect(button).toHaveAttribute('aria-describedby', screen.getByRole('tooltip', { hidden: true }).id);
  fireEvent.click(button); fireEvent.click(button);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('puppeteer_prepare_setup', { projectRoot: '/other-project', kind: 'string', agentId: 'agent-a' });
  expect(client.violetCopyFilePath).not.toHaveBeenCalled(); expect(button).toBeDisabled();
  expect(screen.getByText('This file does not include your unsaved changes.')).toBeVisible();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await act(async () => { finish({ path: '/other-project/.agent-workspaces/agent-a/SHELL.yaml' }); });
  expect(client.violetCopyFilePath).toHaveBeenCalledExactlyOnceWith('/other-project/.agent-workspaces/agent-a/SHELL.yaml');
  expect(await screen.findByText('Path copied.')).toBeVisible(); expect(button).toBeEnabled(); expect(refresh).toHaveBeenCalledOnce();
});
it.each(['prepare', 'clipboard'])('never reports copied when %s fails, unlocks and allows Retry', async stage => {
  if (stage === 'prepare') vi.mocked(invoke).mockRejectedValueOnce({ code: 'rule_file_prepare_failed', private: 'never show' });
  else vi.mocked(client.violetCopyFilePath).mockRejectedValueOnce(new Error('clipboard unavailable'));
  render(<CopyRuleFilePath target={{ projectRoot: '/fixture', kind: 'dance' }} />);
  const button = screen.getByRole('button', { name: 'Copy rule file path' });
  fireEvent.click(button);
  await screen.findByText(stage === 'prepare' ? 'Could not prepare the configuration file. Try again.' : 'Could not copy the file path. Try again.');
  expect(button).toBeEnabled(); expect(screen.queryByText('Path copied.')).not.toBeInTheDocument();
  if (stage === 'prepare') expect(client.violetCopyFilePath).not.toHaveBeenCalled();
  fireEvent.click(button); await screen.findByText('Path copied.');
});
it.each(['hero', 'profile', 'workbench', 'tavern'] as const)('shows a real-file copy action on %s only when it is not a Hero template, even without Jev', surface => {
  render(<StringRoutingEditor draft={newStringRoutingDraft()} catalog={null} owner="Fixture" surface={surface} enabled={false}
    ruleFile={{ projectRoot: '/other', kind: 'string', agentId: 'agent-a' }} onChange={vi.fn()} onClose={vi.fn()} />);
  if (surface === 'hero') expect(screen.queryByRole('button', { name: 'Copy rule file path' })).not.toBeInTheDocument();
  else {
    const button = screen.getByRole('button', { name: 'Copy rule file path' });
    expect(button).toBeEnabled(); expect(button).toHaveTextContent(/^$/);
    expect(button.closest('.local-model-dialog-head')).not.toBeNull();
    expect(screen.getByRole('heading', { name: 'Auto String' }).parentElement).toContainElement(button);
  }
});
it('offers Dance copy only in Rule mode, retaining a dirty rule and never saving it', async () => {
  const save = vi.fn();
  render(<DanceSettingsEditor settings={{ schema: 1, enabled: false, sentinel: 'agent-a', strategy: 'context', threshold: .75, conditions: {} }}
    agents={[{ id: 'agent-a', name: 'Ada' }]} owner="Fixture" projectRoot="/fixture" busy={false} error={null} onSave={save} onClose={vi.fn()} />);
  expect(screen.queryByRole('button', { name: 'Copy rule file path' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Rule based' }));
  const copy = screen.getByRole('button', { name: 'Copy rule file path' });
  expect(copy).toHaveTextContent(/^$/);
  expect(screen.getByRole('heading', { name: 'Puppeteer Setting' }).parentElement).toContainElement(copy);
  const rule = screen.getByRole('textbox', { name: 'Routing condition for Ada' });
  fireEvent.change(rule, { target: { value: 'Keep this unsaved rule' } });
  fireEvent.click(screen.getByRole('button', { name: 'Copy rule file path' }));
  await waitFor(() => expect(client.violetCopyFilePath).toHaveBeenCalled());
  expect(rule).toHaveValue('Keep this unsaved rule'); expect(save).not.toHaveBeenCalled();
});
it('uses readonly detail reads when opening String rules and after prepare, leaving an incomplete draft unsubmitted', async () => {
  const detail = vi.spyOn(client, 'loadProjectAgentDetail').mockResolvedValue({ provider: 'kota-local', autoRoute: null } as unknown as client.ProjectAgentDetail);
  const changed = vi.fn();
  render(<StringLocalModelPicker ownerKey="fixture" owner="Fixture" surface="profile" model="openai/sol" group={null} catalog={null}
    jev={{ settings: { schema: 1, provider: 'openrouter', ready: true, keys: [] }, loading: false, failed: false }}
    ruleFile={{ projectRoot: '/fixture', kind: 'string', agentId: 'agent-a' }} onSelect={vi.fn()} onAutoRouteChange={changed} onOpen={vi.fn()} onJevSettings={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Local model' }));
  fireEvent.click(screen.getByRole('option', { name: /Auto String/ }));
  await waitFor(() => expect(detail).toHaveBeenCalledTimes(1));
  fireEvent.click(screen.getByRole('button', { name: 'Copy rule file path' }));
  await screen.findByText('Path copied.');
  await waitFor(() => expect(detail).toHaveBeenCalledTimes(2));
  expect(detail.mock.calls).toEqual([
    [{ projectRoot: '/fixture', agentId: 'agent-a' }, { readOnly: true }],
    [{ projectRoot: '/fixture', agentId: 'agent-a' }, { readOnly: true }],
  ]);
  expect(changed).not.toHaveBeenCalled();
});
