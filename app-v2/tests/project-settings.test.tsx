import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { refreshGithubAccounts } from '../src/lib/github-account-flow';
import { ProjectSettings } from '../src/chrome/ProjectSettings';
import { readProjectSettings, saveProjectCommitEmail, githubAccountsRead, projectConnectionStatus, type ProjectSettingsValue, type WorkspaceProject } from '../src/pty-client';

vi.mock('../src/pty-client', async original => ({
  ...await original<typeof import('../src/pty-client')>(),
  githubAccountsRead: vi.fn(), projectConnectionStatus: vi.fn(),
  readProjectSettings: vi.fn(),
  saveProjectCommitEmail: vi.fn(),
}));

const read = vi.mocked(readProjectSettings);
const save = vi.mocked(saveProjectCommitEmail);
let values: Record<string, string | null>;

beforeEach(async () => {
  vi.resetAllMocks();
  values = {};
  vi.mocked(githubAccountsRead).mockResolvedValue({defaultLogin:null,accounts:[]});
  vi.mocked(projectConnectionStatus).mockResolvedValue({kind:'github',accountLogin:'bound-login',accountState:'ok'});
  await refreshGithubAccounts();
  read.mockImplementation(async id => ({ commitEmail: values[id] ?? null }));
  save.mockImplementation(async (id, value) => {
    values[id] = value;
    return { commitEmail: value };
  });
});

async function open() {
  fireEvent.click(screen.getByRole('button', { name: 'Project settings' }));
  await waitFor(() => expect(screen.getByRole('dialog')).toHaveAttribute('aria-busy', 'false'));
}

it('adds arbitrary text only after Save, edits, cancels and removes with an activation notice', async () => {
  render(<ProjectSettings projectId="fable-copper" projectName="Fable Copper" />);
  expect(read).not.toHaveBeenCalled();
  await open();
  expect(screen.getByText('Commits will use agent dummy emails.')).toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Add commit email' }));
  const input = screen.getByRole('textbox', { name: 'Email used for new commits' });
  expect(input).toHaveAttribute('type', 'text');
  fireEvent.change(input, { target: { value: '  任意字符串  ' } });
  expect(save).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByRole('button', { name: 'Edit' });
  expect(save).toHaveBeenLastCalledWith('fable-copper', '  任意字符串  ');
  expect(screen.getByRole('status')).toHaveTextContent('Running agents keep their current email until their next start.');
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  expect(screen.getByRole('textbox')).toHaveValue('  任意字符串  ');
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'discarded' } });
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(save).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'next@fable.example' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('next@fable.example');
  fireEvent.click(screen.getByRole('button', { name: 'Close project settings' }));
  await open();
  expect(screen.getByText('next@fable.example')).toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
  await screen.findByText('Commits will use agent dummy emails.');
  expect(save).toHaveBeenLastCalledWith('fable-copper', null);
  expect(screen.getByRole('status')).toHaveTextContent('next start');
  fireEvent.click(screen.getByRole('button', { name: 'Add commit email' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByText('Commits will use agent dummy emails.');
  expect(save).toHaveBeenLastCalledWith('fable-copper', null);
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('keeps failed edits and failed removals visible without claiming success', async () => {
  values['fable-copper'] = 'previous@fable.example';
  render(<ProjectSettings projectId="fable-copper" projectName="Fable Copper" />);
  await open();
  save.mockRejectedValue(new Error('Unable to save project settings'));
  fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
  await screen.findByRole('alert');
  expect(screen.getByText('previous@fable.example')).toBeInTheDocument();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'keep my draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  await screen.findByRole('alert');
  expect(screen.getByRole('textbox')).toHaveValue('keep my draft');
  expect(values['fable-copper']).toBe('previous@fable.example');
});

it('reports a read failure and retries instead of displaying an empty configuration', async () => {
  read.mockRejectedValueOnce(new Error('Unable to read project settings'));
  render(<ProjectSettings projectId="fable-copper" projectName="Fable Copper" />);
  await open();
  expect(screen.getByRole('alert')).toHaveTextContent('Unable to read');
  expect(screen.queryByRole('button', { name: 'Add commit email' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await screen.findByRole('button', { name: 'Add commit email' });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('does not apply a late response to another project and disables repeated saves', async () => {
  let resolveSave!: (value: ProjectSettingsValue) => void;
  save.mockImplementationOnce(() => new Promise(resolve => { resolveSave = resolve; }));
  const view = render(<ProjectSettings key="fable-copper" projectId="fable-copper" projectName="Fable Copper" />);
  await open();
  fireEvent.click(screen.getByRole('button', { name: 'Add commit email' }));
  fireEvent.change(screen.getByRole('textbox'), { target: { value: 'pending' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(screen.getByRole('button', { name: 'Saving…' })).toBeDisabled();
  view.rerender(<ProjectSettings key="fable-indigo" projectId="fable-indigo" projectName="Fable Indigo" />);
  await open();
  await act(async () => resolveSave({ commitEmail: 'pending' }));
  expect(screen.getByText('Commits will use agent dummy emails.')).toBeInTheDocument();
  expect(save).toHaveBeenCalledTimes(1);
  expect(save).toHaveBeenCalledWith('fable-copper', 'pending');
  expect(read).toHaveBeenLastCalledWith('fable-indigo');
  fireEvent.pointerDown(document.body);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
});

it('keeps a normal cursor while saving without losing the save lock or errors', async () => {
  const style = document.createElement('style');
  // Vitest stubs CSS imports; load the real stylesheet for this cursor assertion.
  style.textContent = readFileSync(resolve(__dirname, '../src/styles/project-settings.css'), 'utf8');
  document.head.append(style);
  try {
    let rejectSave!: (reason: Error) => void;
    save.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectSave = reject; }));
    render(<ProjectSettings projectId="fable-copper" projectName="Fable Copper" />);
    await open();
    fireEvent.click(screen.getByRole('button', { name: 'Add commit email' }));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'pending@fable.example' } });
    const form = input.closest('form')!;
    fireEvent.submit(form);
    const pending = screen.getByRole('button', { name: 'Saving…' });
    expect(pending).toBeDisabled();
    expect(input).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
    expect(window.getComputedStyle(pending).cursor).toBe('default');
    fireEvent.submit(form);
    expect(save).toHaveBeenCalledTimes(1);
    await act(async () => rejectSave(new Error('Unable to save project settings')));
    expect(screen.getByRole('alert')).toHaveTextContent('Unable to save project settings');
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
    expect(screen.getByRole('textbox')).toHaveValue('pending@fable.example');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  } finally {
    style.remove();
  }
});

const localWorkspace = { projectId:'local_fixture', displayName:'Local Notes', connection:{kind:'local'},
  repoFullName:'', remoteUrl:'', githubHtmlUrl:'', sourceDir:'/fixture/local', localRoot:'/fixture/state',
  sourceDirBytes:0, localRootBytes:0, sharedDir:'/fixture/shared', rulesDir:'/fixture/rules', agents:[], defaultBranch:'main', baseRef:'main' } satisfies WorkspaceProject;

it('keeps Local disconnected without an available account and preserves its disabled commit email', async () => {
  values.local_fixture = 'retained@example.test';
  render(<ProjectSettings projectId="local_fixture" projectName="Local Notes" workspace={localWorkspace}/>);
  await open();
  const toggle=screen.getByRole('switch',{name:'Link GitHub'});
  expect(toggle).toBeDisabled();expect(toggle).toHaveAttribute('aria-checked','false');
  expect(screen.getByText('retained@example.test')).toBeInTheDocument();
  expect(screen.getByText('Saved commit email')).toBeInTheDocument();
  expect(screen.getByRole('button',{name:'Edit'})).toBeDisabled();
  expect(screen.getByRole('button',{name:'Remove'})).toBeDisabled();
  fireEvent.click(screen.getByRole('button',{name:'Remove'}));
  expect(save).not.toHaveBeenCalled();expect(values.local_fixture).toBe('retained@example.test');
  expect(screen.getByText('Available when this project is connected to GitHub.')).toBeInTheDocument();
  expect(screen.queryByText('Use another GH account for this project')).not.toBeInTheDocument();
});
it('keeps legacy GitHub project email editing and displays a bound project login', async () => {
  render(<ProjectSettings projectId="linked" projectName="Linked" workspace={{...localWorkspace,projectId:'linked',repoFullName:'owner/repo',connection:{kind:'github',accountLogin:'bound-login'}}}/>);
  await open();
  expect(screen.getByText('Connected to GH ID bound-login')).toBeInTheDocument();
  expect(screen.getByRole('switch')).toBeEnabled();expect(screen.getByRole('switch')).toHaveAttribute('aria-checked','true');
  expect(screen.getByRole('button',{name:'Add commit email'})).toBeEnabled();
});
