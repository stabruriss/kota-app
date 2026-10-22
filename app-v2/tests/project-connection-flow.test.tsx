import { useEffect, useState } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import * as ipc from '../src/pty-client';
import { ProjectConnectDialog } from '../src/chrome/ProjectConnectDialog';
import { ProjectSettings } from '../src/chrome/ProjectSettings';
import { finishGithubSignIn, refreshGithubAccounts } from '../src/lib/github-account-flow';
import { PROJECT_CONNECTION_CHANGED, RESTART_NOTICE, dismissConnectionNotice, mergeWorkspaceConnection } from '../src/lib/project-connection-ui';

vi.mock('../src/pty-client', async original => ({ ...await original<typeof ipc>(),
  githubAccountsRead:vi.fn(), githubAccountAddFinish:vi.fn(), projectConnectionCandidates:vi.fn(), projectConnectionCheck:vi.fn(),
  projectConnectionApply:vi.fn(), projectConnectionSetLocal:vi.fn(), projectConnectionStatus:vi.fn(), readProjectSettings:vi.fn(),
}));
const workspace:ipc.WorkspaceProject = {projectId:'fixture',displayName:'Fixture',connection:{kind:'local'},repoFullName:'',remoteUrl:'',githubHtmlUrl:'',sourceDir:'/fixture/project',localRoot:'/fixture/state',sourceDirBytes:0,localRootBytes:0,sharedDir:'/fixture/shared',rulesDir:'/fixture/rules',agents:[],defaultBranch:'main',baseRef:'main'};
const accounts:ipc.GithubAccounts = {defaultLogin:'alpha',accounts:['alpha','beta'].map(login=>({login,default:login==='alpha',state:'ok',source:'gh-cli'}))};
const candidate = (login='alpha'):ipc.ConnectionCandidates => ({accountLogin:login,selected:`${login}/fixture`,largeFiles:[],candidates:[{fullName:`${login}/fixture`,private:true,defaultBranch:'main',previous:true},{fullName:`${login}/other`,private:false,defaultBranch:'main',previous:false}]});
const connected:ipc.WorkspaceProject = {...workspace,connection:{kind:'github',accountLogin:'alpha'},repoFullName:'alpha/fixture',remoteUrl:'https://github.com/alpha/fixture.git',githubHtmlUrl:'https://github.com/alpha/fixture'};
function deferred<T>() {let resolve!:(v:T)=>void;const promise=new Promise<T>(r=>{resolve=r;});return{promise,resolve};}
const mount = (base=workspace, switchOrigin=true) => {const onSaved=vi.fn(),onCancel=vi.fn();render(<ProjectConnectDialog workspace={base} login="alpha" loginReady switchOrigin={switchOrigin} onCancel={onCancel} onSaved={onSaved}/>);return{onSaved,onCancel};};
const continueButton=()=>within(screen.getByRole('dialog',{name:'Connect to GitHub'})).getByRole('button',{name:/^(Connect|Create & connect)$/});
async function ready() {await waitFor(()=>expect(continueButton()).toBeEnabled());}
async function confirm() {await ready();fireEvent.click(continueButton());return screen.findByRole('alertdialog');}
beforeEach(async()=>{
 vi.resetAllMocks();
 vi.mocked(ipc.githubAccountsRead).mockResolvedValue(accounts);
 vi.mocked(ipc.githubAccountAddFinish).mockResolvedValue({added:[],active:'alpha',restoreFailed:false,view:accounts});
 vi.mocked(ipc.projectConnectionCandidates).mockImplementation(async(_id,login)=>candidate(login));
 vi.mocked(ipc.projectConnectionCheck).mockImplementation(async(_id,accountLogin,repoFullName)=>({accountLogin,repoFullName,state:'in_sync'}));
 vi.mocked(ipc.projectConnectionApply).mockResolvedValue({workspace:connected,pushed:false});
 vi.mocked(ipc.projectConnectionSetLocal).mockResolvedValue(workspace);
 vi.mocked(ipc.projectConnectionStatus).mockResolvedValue({kind:'github',accountLogin:'alpha',accountState:'ok'});
 vi.mocked(ipc.readProjectSettings).mockResolvedValue({commitEmail:null});
 await finishGithubSignIn();await refreshGithubAccounts();dismissConnectionNotice('fixture');vi.clearAllMocks();
});
it('ignores late repository checks and account candidate lists, never enabling Connect with their results',async()=>{
 const oldCheck=deferred<ipc.ConnectionCheck>(),oldList=deferred<ipc.ConnectionCandidates>();
 vi.mocked(ipc.projectConnectionCheck).mockReturnValueOnce(oldCheck.promise);
 mount();await waitFor(()=>expect(ipc.projectConnectionCheck).toHaveBeenCalledTimes(1));
 fireEvent.change(screen.getByRole('combobox',{name:'Repository'}),{target:{value:'alpha/other'}});
 await ready();await act(async()=>oldCheck.resolve({accountLogin:'alpha',repoFullName:'alpha/fixture',state:'no_access'}));
 expect(screen.getByText('Already in sync')).toBeInTheDocument();
 vi.mocked(ipc.projectConnectionCandidates).mockReturnValueOnce(oldList.promise);
 fireEvent.change(screen.getByRole('combobox',{name:'Connect to GH ID'}),{target:{value:'beta'}});
 await waitFor(()=>expect(ipc.projectConnectionCandidates).toHaveBeenLastCalledWith('fixture','beta'));
 fireEvent.change(screen.getByRole('combobox',{name:'Connect to GH ID'}),{target:{value:'alpha'}});
 await ready();await act(async()=>oldList.resolve(candidate('beta')));
 expect(screen.getByRole('combobox',{name:'Repository'})).toHaveValue('alpha/fixture');
 expect(ipc.projectConnectionApply).not.toHaveBeenCalled();
});
it('requires a separate confirmation, defaults to Cancel, and cancels the switch-origin flow without a write',async()=>{
 const {onCancel}=mount();const card=await confirm();
 expect(within(card).getByRole('button',{name:'Cancel'})).toHaveFocus();
 expect(ipc.projectConnectionApply).not.toHaveBeenCalled();
 fireEvent.keyDown(window,{key:'Escape'});
 expect(onCancel).toHaveBeenCalledTimes(1);expect(ipc.projectConnectionApply).not.toHaveBeenCalled();
});
it('cancels an account-change confirmation back to its editable draft, preserving the current connection',async()=>{
 const {onCancel}=mount(connected,false);const card=await confirm();
 fireEvent.click(within(card).getByRole('button',{name:'Cancel'}));
 expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
 expect(screen.getByRole('combobox',{name:'Repository'})).toHaveValue('alpha/fixture');
 expect(onCancel).not.toHaveBeenCalled();expect(ipc.projectConnectionApply).not.toHaveBeenCalled();
});
it('prevents creating a remote when the local history contains oversized files',async()=>{
 vi.mocked(ipc.projectConnectionCandidates).mockResolvedValue({...candidate(),selected:null,largeFiles:[{path:'data/archive.bin',bytes:123_000_000}]});
 mount();await screen.findByText('data/archive.bin');
 expect(screen.getByRole('checkbox',{name:'Private repository'})).toBeChecked();
 expect(continueButton()).toBeDisabled();expect(ipc.projectConnectionApply).not.toHaveBeenCalled();
});
it('uses a freshly blocked apply result and preserves the old connection instead of showing success',async()=>{
 vi.mocked(ipc.projectConnectionApply).mockRejectedValue({code:'not_connectable',message:'History changed.',state:'unrelated'});
 const {onSaved}=mount(connected,false);const card=await confirm();fireEvent.click(within(card).getByRole('button',{name:'Change account'}));
 await screen.findByText('Different history');expect(continueButton()).toBeDisabled();
 expect(screen.getByRole('alert')).toHaveTextContent('This project is still connected to alpha/fixture with alpha.');
 expect(onSaved).not.toHaveBeenCalled();
});
it('explains a retained created repository and retries it as existing, never creating it twice',async()=>{
 vi.mocked(ipc.projectConnectionCandidates).mockResolvedValue({...candidate(),selected:null});
 vi.mocked(ipc.projectConnectionApply).mockRejectedValueOnce({code:'push_failed',message:'Push failed.',createdRepo:'alpha/Fixture'});
 const {onSaved}=mount();let card=await confirm();fireEvent.click(within(card).getByRole('button',{name:'Create & connect'}));
 await screen.findByRole('alert');expect(screen.getByRole('alert')).toHaveTextContent('alpha/Fixture was created. The repository remains on GitHub.');
 expect(screen.getByRole('alert')).toHaveTextContent('Your project is still Local only.');
 await ready();expect(screen.getByRole('combobox',{name:'Repository'})).toHaveValue('alpha/Fixture');
 card=await confirm();fireEvent.click(within(card).getByRole('button',{name:'Connect'}));
 await waitFor(()=>expect(onSaved).toHaveBeenCalledTimes(1));
 expect(ipc.projectConnectionApply).toHaveBeenNthCalledWith(1,{projectId:'fixture',accountLogin:'alpha',target:{kind:'new',name:'Fixture',private:true}});
 expect(ipc.projectConnectionApply).toHaveBeenNthCalledWith(2,{projectId:'fixture',accountLogin:'alpha',target:{kind:'existing',repoFullName:'alpha/Fixture'}});
});
it('serializes apply and publishes the returned workspace without reopening or replacing the agent roster',async()=>{
 const pending=deferred<ipc.ConnectionApplied>();vi.mocked(ipc.projectConnectionApply).mockReturnValueOnce(pending.promise);
 const changed=vi.fn();window.addEventListener(PROJECT_CONNECTION_CHANGED,changed);
 try {
  const {onSaved}=mount();const card=await confirm();const button=within(card).getByRole('button',{name:'Connect'});
  fireEvent.click(button);fireEvent.click(button);expect(button).toBeDisabled();
  expect(ipc.projectConnectionApply).toHaveBeenCalledTimes(1);
  await act(async()=>pending.resolve({workspace:connected,pushed:false}));
  expect(onSaved).toHaveBeenCalledTimes(1);expect(changed.mock.calls[0][0].detail).toEqual(connected);
  const current={...workspace,agents:[{agentId:'live-agent'}] as ipc.WorkspaceProject['agents']};
  expect(mergeWorkspaceConnection(current,connected).agents).toBe(current.agents);
  expect(mergeWorkspaceConnection({...current,projectId:'other'},connected).connection).toEqual({kind:'local'});
 }finally{window.removeEventListener(PROJECT_CONNECTION_CHANGED,changed);}
});
function SettingsHarness({initial=connected}:{initial?:ipc.WorkspaceProject}) {
 const [value,setValue]=useState(initial);
 useEffect(()=>{const update=(e:Event)=>setValue(v=>mergeWorkspaceConnection(v,(e as CustomEvent<ipc.WorkspaceProject>).detail));window.addEventListener(PROJECT_CONNECTION_CHANGED,update);return()=>window.removeEventListener(PROJECT_CONNECTION_CHANGED,update);},[]);
 return <ProjectSettings projectId={value.projectId} projectName="Fixture" workspace={value}/>;
}
function finishMotion() {const event=new Event('transitionend',{bubbles:true});Object.defineProperty(event,'propertyName',{value:'transform'});fireEvent(document.querySelector('.project-github-thumb')!,event);}
it('disconnects only after the switch animation and confirmation, and retains the dismissible reminder across Settings closes',async()=>{
 render(<SettingsHarness/>);fireEvent.click(screen.getByRole('button',{name:'Project settings'}));
 await screen.findByRole('button',{name:'Use another GH account for this project'});
 fireEvent.click(screen.getByRole('switch'));expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
 finishMotion();const card=await screen.findByRole('alertdialog');
 expect(ipc.projectConnectionSetLocal).not.toHaveBeenCalled();
 fireEvent.click(within(card).getByRole('button',{name:'Switch to Local only'}));
 await screen.findByText(RESTART_NOTICE.local);expect(ipc.projectConnectionSetLocal).toHaveBeenCalledWith('fixture');
 fireEvent.click(screen.getByRole('button',{name:'Close project settings'}));fireEvent.click(screen.getByRole('button',{name:'Project settings'}));
 await screen.findByText(RESTART_NOTICE.local);fireEvent.click(screen.getByRole('button',{name:'Dismiss restart reminder'}));
 expect(screen.queryByText(RESTART_NOTICE.local)).not.toBeInTheDocument();
});
it('shows missing credentials without falling back to a usable default account',async()=>{
 vi.mocked(ipc.projectConnectionStatus).mockResolvedValue({kind:'github',accountLogin:'removed',accountState:'missing'});
 render(<SettingsHarness initial={{...connected,connection:{kind:'github',accountLogin:'removed'}}}/>);
 fireEvent.click(screen.getByRole('button',{name:'Project settings'}));
 await screen.findByText('No account assigned');expect(screen.getByRole('button',{name:'Choose account'})).toBeEnabled();
 expect(screen.getByText('Connected to GH ID removed')).toBeInTheDocument();
 expect(ipc.projectConnectionApply).not.toHaveBeenCalled();
});
