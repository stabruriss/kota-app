import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { ProjectSetupModal } from '../src/chrome/ProjectSetupModal';
import * as ipc from '../src/pty-client';
import { GithubSignInDialog } from '../src/chrome/GithubSignInDialog';
import { closeGithubSignIn, continueGithubSignIn, refreshGithubAccounts } from '../src/lib/github-account-flow';
import { fakeGithubSignIn } from './github-sign-in-fake';

vi.mock('../src/pty-client', async importOriginal => ({
  ...await importOriginal<typeof ipc>(),
  ghAuthStatus: vi.fn(), githubAccountsRead: vi.fn(), githubListRepos: vi.fn(),
  listWorkspaceProjects: vi.fn(), githubCreateRepo: vi.fn(), prepareGithubProject: vi.fn(),
  createLocalProject: vi.fn(), gitAvailability: vi.fn(), installGitCommandLineTools: vi.fn(),
  githubSignInStart: vi.fn(), githubSignInCancel: vi.fn(), onGithubSignIn: vi.fn(), openExternalUrl: vi.fn(),
}));
const local: ipc.WorkspaceProject = { projectId:'local_fixture', displayName:'Copper / Notes', connection:{kind:'local'},
  repoFullName:'', remoteUrl:'', githubHtmlUrl:'', sourceDir:'/fixture/local', localRoot:'/fixture/state',
  sourceDirBytes:0, localRootBytes:0, sharedDir:'/fixture/shared', rulesDir:'/fixture/rules', agents:[], defaultBranch:'main', baseRef:'main' };
const repo: ipc.GithubRepo = { fullName:'project-account/fixture',name:'fixture',owner:'project-account',private:true,
  defaultBranch:'main',cloneUrl:'https://github.com/project-account/fixture.git',htmlUrl:'https://github.com/project-account/fixture' };
const ready = { defaultLogin:'project-account',accounts:[{login:'project-account',default:true,state:'ok' as const,source:'gh-cli' as const}] };
const mount = () => { const prepared=vi.fn(); const view=render(<ProjectSetupModal open onClose={vi.fn()} onWorkspacePrepared={prepared}/>);return{...view,prepared}; };
let gh: ReturnType<typeof fakeGithubSignIn>;
beforeEach(async () => {
  vi.clearAllMocks();
  vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:false,username:null,scopes:[],error:null,cliMissing:true});
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue({defaultLogin:null,accounts:[]});
  vi.mocked(ipc.gitAvailability).mockResolvedValue({available:true,version:'git fixture'});
  vi.mocked(ipc.listWorkspaceProjects).mockResolvedValue([]);
  vi.mocked(ipc.githubListRepos).mockResolvedValue([]);
  vi.mocked(ipc.createLocalProject).mockResolvedValue(local);
  vi.mocked(ipc.githubCreateRepo).mockResolvedValue(repo);
  vi.mocked(ipc.prepareGithubProject).mockResolvedValue({...local,repoFullName:repo.fullName,connection:{kind:'github',accountLogin:'project-account'}});
  vi.mocked(ipc.installGitCommandLineTools).mockResolvedValue();
  closeGithubSignIn(); await refreshGithubAccounts(); vi.clearAllMocks();
  gh = fakeGithubSignIn(ipc);
});
it('creates a local project without GitHub, locks the switch off and never creates a remote', async () => {
  const {prepared}=mount();
  const name=screen.getByRole('textbox',{name:'Project name'});
  fireEvent.change(name,{target:{value:'Copper / Notes'}});
  const create=screen.getByRole('button',{name:'Create local project'});
  await waitFor(()=>expect(create).toBeEnabled());
  expect(screen.getByRole('switch',{name:'Link GitHub'})).toBeDisabled();
  expect(screen.getByRole('switch')).toHaveAttribute('aria-checked','false');
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  fireEvent.click(create);
  await waitFor(()=>expect(prepared).toHaveBeenCalledWith(local));
  expect(ipc.createLocalProject).toHaveBeenCalledWith('Copper / Notes');
  expect(ipc.githubCreateRepo).not.toHaveBeenCalled();
  expect(ipc.githubListRepos).not.toHaveBeenCalled();
});
it('pins Connected creation and repository reads to the Kota default, not the gh active login', async () => {
  vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:true,username:'other-active',scopes:[],error:null,cliMissing:false});
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue(ready);
  const {prepared}=mount();
  await screen.findByRole('combobox', { name: 'GitHub account' });
  expect(screen.getByRole('combobox', { name: 'GitHub account' })).toHaveValue('project-account');
  expect(screen.getByRole('checkbox',{name:'Private repository'})).toBeChecked();
  expect(ipc.githubListRepos).toHaveBeenCalledWith('project-account');
  fireEvent.click(screen.getByRole('switch'));
  expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('switch'));
  fireEvent.change(screen.getByRole('textbox'),{target:{value:'fixture'}});
  fireEvent.click(screen.getByRole('button',{name:'Create on GitHub + Open'}));
  await waitFor(()=>expect(prepared).toHaveBeenCalled());
  expect(ipc.githubCreateRepo).toHaveBeenCalledWith({name:'fixture',private:true,autoInit:true,accountLogin:'project-account'});
  expect(ipc.prepareGithubProject).toHaveBeenCalledWith(repo.fullName,'project-account');
  expect(ipc.createLocalProject).not.toHaveBeenCalled();
});
it('keeps local creation independent of a stalled GitHub probe', async () => {
  vi.mocked(ipc.ghAuthStatus).mockImplementation(()=>new Promise(()=>{}));
  const {prepared}=mount();
  fireEvent.change(screen.getByRole('textbox'),{target:{value:'Independent'}});
  const create=screen.getByRole('button',{name:'Create local project'});
  await waitFor(()=>expect(create).toBeEnabled());fireEvent.click(create);
  await waitFor(()=>expect(prepared).toHaveBeenCalledWith(local));
});
it('detects missing git, opens the installer only on request, and rechecks explicitly', async () => {
  vi.mocked(ipc.gitAvailability).mockResolvedValueOnce({available:false});
  mount();fireEvent.change(screen.getByRole('textbox'),{target:{value:'fixture'}});
  await screen.findByText('Git is required to create a project.');
  expect(screen.getByRole('button',{name:'Create local project'})).toBeDisabled();
  expect(ipc.installGitCommandLineTools).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button',{name:'Install Command Line Tools'}));
  await waitFor(()=>expect(ipc.installGitCommandLineTools).toHaveBeenCalledTimes(1));
  await waitFor(()=>expect(screen.getByRole('button',{name:'Check again'})).toBeEnabled());
  fireEvent.click(screen.getByRole('button',{name:'Check again'}));
  await waitFor(()=>expect(screen.getByRole('button',{name:'Create local project'})).toBeEnabled());
  expect(ipc.gitAvailability).toHaveBeenCalledTimes(2);
});
it('keeps a failed name and creation choice, prevents duplicate submits and permits retry', async () => {
  let fail!:(reason:Error)=>void;
  vi.mocked(ipc.createLocalProject).mockImplementationOnce(()=>new Promise((_,reject)=>{fail=reject;}));
  mount();fireEvent.change(screen.getByRole('textbox'),{target:{value:'keep me'}});
  const create=screen.getByRole('button',{name:'Create local project'});
  await waitFor(()=>expect(create).toBeEnabled());fireEvent.click(create);fireEvent.click(create);
  expect(ipc.createLocalProject).toHaveBeenCalledTimes(1);
  await act(async()=>fail(new Error('local_project_name_invalid')));
  expect(screen.getByRole('alert')).toHaveTextContent('Use 1–100 characters without control characters for the project name.');
  expect(screen.getByRole('textbox')).toHaveValue('keep me');expect(create).toBeEnabled();
  fireEvent.click(create);await waitFor(()=>expect(ipc.createLocalProject).toHaveBeenCalledTimes(2));
});
it('does not let an old shared read replace a newer sign-in result after reopening', async () => {
  vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:false,username:null,scopes:[],error:null,cliMissing:false});
  let resolve!:(v:ipc.GithubAccounts)=>void;
  vi.mocked(ipc.githubAccountsRead).mockImplementationOnce(()=>new Promise(r=>{resolve=r;}));
  const {rerender}=mount();
  rerender(<ProjectSetupModal open={false} onClose={vi.fn()} onWorkspacePrepared={vi.fn()}/>);
  vi.mocked(ipc.githubAccountsRead).mockResolvedValue({ ...ready, accounts:ready.accounts.map(a=>({...a,state:'needs_login'})) });
  rerender(<ProjectSetupModal open onClose={vi.fn()} onWorkspacePrepared={vi.fn()}/>);
  await act(async()=>{void continueGithubSignIn();});
  await gh.signIn({login:'project-account',view:{...ready,accounts:ready.accounts.map(a=>({...a,state:'needs_login' as const}))}});
  act(closeGithubSignIn);
  await act(async()=>resolve(ready));
  await screen.findByText('Sign in required');
  expect(screen.getByRole('switch')).toBeDisabled();expect(screen.getByRole('switch')).toHaveAttribute('aria-checked','false');
});

const signIn = async (result: Parameters<typeof gh.signIn>[0]) => {
  fireEvent.click(await screen.findByRole('button', { name: /^(Add GitHub account|Sign in)$/ }));
  const next = await screen.findByRole('button', { name: 'Signed in, next' });
  await act(async () => { fireEvent.click(next); });
  await gh.signIn(result);
};
it('signs in to the first account in place and reads its repositories',async()=>{
 vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:false,username:null,scopes:[],error:null,cliMissing:false});
 mount();render(<GithubSignInDialog/>);
 const add=await screen.findByRole('button',{name:'Add GitHub account'});await waitFor(()=>expect(add).toBeEnabled());
 await signIn({login:'project-account',view:ready});
 await waitFor(()=>expect(ipc.githubListRepos).toHaveBeenCalledWith('project-account'));
 expect(screen.getByText('project-account added')).toBeInTheDocument();
});
it('chooses the account just added, not the default',async()=>{
 vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:true,username:'x',scopes:[],error:null,cliMissing:false});
 vi.mocked(ipc.githubAccountsRead).mockResolvedValue(ready);
 mount();render(<GithubSignInDialog/>);
 await waitFor(()=>expect(ipc.githubListRepos).toHaveBeenCalledWith('project-account'));
 await waitFor(()=>expect(screen.getByRole('button',{name:'Add GitHub account'})).toBeEnabled());
 await signIn({login:'second',view:{...ready,accounts:[...ready.accounts,{login:'second',default:false,state:'ok',source:'gh-cli'}]}});
 await waitFor(()=>expect(ipc.githubListRepos).toHaveBeenLastCalledWith('second'));
 expect(screen.getByRole('combobox',{name:'Repository account'})).toHaveValue('second');
});
it('signs in again to the account it shows',async()=>{
 vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:false,username:null,scopes:[],error:null,cliMissing:false});
 vi.mocked(ipc.githubAccountsRead).mockResolvedValue({...ready,accounts:ready.accounts.map(a=>({...a,state:'needs_login' as const}))});
 mount();render(<GithubSignInDialog/>);
 await screen.findByText('Sign in required');
 await waitFor(()=>expect(screen.getByRole('button',{name:'Sign in'})).toBeEnabled());
 await act(async()=>{fireEvent.click(screen.getByRole('button',{name:'Sign in'}));});
 expect(screen.getByRole('dialog',{name:'Sign in to GitHub'})).toHaveTextContent('sign in to GitHub as project-account');
});
it('looks for GitHub CLI only while it is missing',async()=>{
 vi.useFakeTimers({toFake:['setInterval','clearInterval']});
 try{
  mount();
  fireEvent.click(await screen.findByRole('button',{name:'Install GitHub CLI'}));
  expect(ipc.openExternalUrl).toHaveBeenCalledWith(ipc.GITHUB_CLI_INSTALL_URL);
  vi.mocked(ipc.ghAuthStatus).mockClear();
  vi.mocked(ipc.ghAuthStatus).mockResolvedValue({authenticated:false,username:null,scopes:[],error:null,cliMissing:false});
  await act(async()=>{vi.advanceTimersByTime(3000);});
  await screen.findByRole('button',{name:'Add GitHub account'});
  await act(async()=>{vi.advanceTimersByTime(60_000);});
  expect(ipc.ghAuthStatus).toHaveBeenCalledTimes(1);
 }finally{vi.useRealTimers();}
});
