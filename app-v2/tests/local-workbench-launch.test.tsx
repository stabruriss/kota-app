import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { StrictMode } from 'react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { LocalWorkbenchHost } from '../src/chrome/LocalWorkbenchHost';
import * as pty from '../src/pty-client';
import * as rpc from '../src/lib/local-rpc';
import * as auth from '../src/lib/local-auth';
import * as startup from '../src/lib/local-startup';
import * as magpie from '../src/lib/local-magpie';
import type { ExistingAgentLaunchResult } from '../src/lib/existing-agent-launch';

const root = '/fixture/local-open', agentId = 'agent-open-fixture';
const owner = { name:'Copper', emoji:'', role:'', hue:'', avatarId:null, provider:'kota-local' };
const original = { provider:'openai-codex', id:'gpt-6.1-sol', thinking:'max' };
let snapshot: rpc.LocalWorkbenchView | null, saved: typeof original | null;
let availability: auth.LocalAuthAvailability, catalog: auth.LocalModelCatalog;
let attempt: startup.LocalStartView | null, sequence: number;
let listeners: Set<(view: startup.LocalStartView) => void>;
const ready = (): rpc.LocalWorkbenchView => ({
  scope:{ projectRoot:root, agentId, sessionId:'fixture-session', epoch:1 }, state:'ready', childPid:42,
  model:saved && {provider:saved.provider,id:saved.id}, thinkingLevel:saved?.thinking ?? null, error:null,
  compacting:false, queued:0, revision:1, runId:0, startedAt:null, phase:'Online', statsRevision:0,
  dropped:0, queueTotal:0, queueRevision:0, blocks:[], queue:[],
});
function publish(state: startup.LocalStartView['state'], error: startup.LocalStartError | null = null, stage = 'session_ready') {
  if (!attempt || attempt.state !== 'running') return;
  attempt = { ...attempt, state, error, revision:attempt.revision + 1,
    steps:[...attempt.steps, { stage, startedAt:new Date().toISOString(), finishedAt:state === 'running' ? null : new Date().toISOString(), elapsedMs:10, error }] };
  for (const listener of listeners) listener(structuredClone(attempt));
}
const resume = async (): Promise<ExistingAgentLaunchResult> => {
  if (attempt?.selection) {
    saved = {provider:attempt.selection.providerId,id:attempt.selection.modelId,thinking:attempt.selection.thinkingLevel};
    attempt.saved = true;
  }
  const row = availability.providers.find(p => p.providerId === saved?.provider);
  if (row?.status !== 'signed_in') {
    publish('failed', {code:row?.status === 'invalid' ? 'credential_invalid' : 'provider_not_signed_in',providerId:saved?.provider,stage:'credential_status'}, 'credential_status');
    return {status:'failed',error:'controlled failure'};
  }
  snapshot = ready(); publish('ready'); return {status:'launched'};
};
beforeEach(() => {
  vi.restoreAllMocks(); snapshot = null; saved = {...original}; attempt = null; sequence = 0; listeners = new Set();
  availability = {schema:1,summary:'not_ready',providers:[{providerId:'openai-codex',status:'not_signed_in'}]};
  catalog = {schema:1,providers:[
    {providerId:'openai-codex',linked:false,models:[{modelId:original.id,name:'GPT-6.1 Sol',thinkingLevels:['off','max']}]},
    {providerId:'zai',linked:false,models:[{modelId:'glm',name:'GLM-5-Turbo',thinkingLevels:['off','high']}]},
  ]};
  vi.spyOn(magpie,'probeMagpie').mockImplementation(async () => {
    const state: magpie.MagpieState = {settings:{mode:'local',ip:'',port:3425}, linked:true,keySaved:true,modelCount:0,defaultKey:'fixture-default'};
    window.dispatchEvent(new CustomEvent(magpie.MAGPIE_CHANGED,{detail:state}));
    return state;
  });
  vi.spyOn(pty,'loadProjectAgentDetail').mockRejectedValue(new Error('startup must use the narrow configuration read'));
  vi.spyOn(auth,'readLocalAuthAvailability').mockImplementation(async () => structuredClone(availability));
  vi.spyOn(auth,'readLocalModelCatalog').mockImplementation(async () => structuredClone(catalog));
  vi.spyOn(startup,'onLocalStartup').mockImplementation(async (_id, handler) => {listeners.add(handler); return () => {listeners.delete(handler);};});
  vi.spyOn(startup,'localLaunchAction').mockImplementation(async (_root,_id,action,extra: any = {}) => {
    if (action === 'configuration') return {model:saved ? `${saved.provider}/${saved.id}` : null,thinkingLevel:saved?.thinking ?? 'medium'} as never;
    if (action === 'read') return structuredClone(attempt) as never;
    if (action === 'begin') {
      const reused = attempt?.state === 'running';
      if (!reused) attempt = {projectRoot:root,agentId,attemptId:`attempt-${++sequence}`,revision:1,state:'running',selection:extra.selection ?? null,saved:false,error:null,
        steps:[{stage:'configuration_read',startedAt:new Date(Date.now() + sequence).toISOString(),finishedAt:null,elapsedMs:0,error:null}]};
      return {view:structuredClone(attempt),reused} as never;
    }
    if (action === 'cancel') publish('cancelled',{code:'startup_cancelled'});
    if (action === 'failed') publish('failed',{code:'local_start_failed'});
    return structuredClone(attempt) as never;
  });
  vi.spyOn(rpc,'readLocalWorkbench').mockImplementation(async () => snapshot && structuredClone(snapshot));
  vi.spyOn(rpc,'readLocalAuth').mockResolvedValue({provider:'zai',authType:'api_key',status:'ready'});
  vi.spyOn(rpc,'controlLocalWorkbench').mockImplementation(async (_scope,request) => (
    request.type === 'get_state' ? {model:snapshot?.model,thinkingLevel:snapshot?.thinkingLevel}
    : request.type === 'get_available_models' ? {models:saved ? [{provider:saved.provider,id:saved.id}] : []}
    : request.type === 'get_available_thinking_levels' ? {levels:['off','high','max']} : {contextUsage:null}) as never);
  vi.spyOn(rpc,'setOfflineLocalModel').mockRejectedValue(new Error('selection is only saved by Start session'));
});
afterEach(() => {vi.useRealTimers(); vi.restoreAllMocks();});
function mount(onResume: (id: string, options?: startup.LocalStartupOptions) => Promise<unknown> = vi.fn(resume), resumeOnOpen = true) {
  const props = {projectRoot:root,agents:{[agentId]:owner},records:{},targets:[agentId],onResume,onFresh:vi.fn(),onDetails:vi.fn(),onClose:vi.fn(),onSend:vi.fn().mockResolvedValue(true),onGuardsChange:vi.fn()};
  const open = {projectRoot:root,agentId,focusRequest:1,resumeOnOpen};
  return {...render(<LocalWorkbenchHost {...props} open={open} />),props,open,onResume};
}
function anotherLinked() {
  availability.summary = 'ready'; availability.providers.push({providerId:'zai',status:'signed_in',authType:'api_key'}); catalog.providers[1].linked = true;
}
async function choose() {await userEvent.click(screen.getByRole('button', {name:'Local model'})); await userEvent.click(await screen.findByRole('option', {name:/GLM-5-Turbo Z.AI/}));}

it('probes Magpie only when the model menu actually opens, not on focus, render or selection', async () => {
  anotherLinked(); const m = mount(vi.fn(resume), false);
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  const picker = screen.getByRole('button', {name:'Local model'});
  fireEvent.focus(picker); expect(magpie.probeMagpie).not.toHaveBeenCalled();
  await userEvent.click(picker);
  await waitFor(() => expect(magpie.probeMagpie).toHaveBeenCalledTimes(1));
  const search = screen.getByRole('searchbox', {name:'Search models'});
  fireEvent.keyDown(search,{key:'ArrowDown'});
  await userEvent.click(await screen.findByRole('option',{name:/GLM-5-Turbo Z.AI/}));
  expect(magpie.probeMagpie).toHaveBeenCalledTimes(1); expect(m.onResume).not.toHaveBeenCalled();
  m.rerender(<LocalWorkbenchHost {...m.props} open={m.open} />);
  expect(magpie.probeMagpie).toHaveBeenCalledTimes(1);
  await userEvent.click(picker);
  await waitFor(() => expect(magpie.probeMagpie).toHaveBeenCalledTimes(2));
});

it('an empty model picker discovers Magpie without starting, changing the saved model, or losing inner slashes', async () => {
  const m = mount(vi.fn(resume), false); await screen.findByText('No provider is signed in.');
  expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled();
  availability.providers.push({providerId:'magpie',status:'signed_in',authType:'api_key'});
  catalog.providers.push({providerId:'magpie',linked:true,models:[{modelId:'vendor/family/model',name:'Pretty',thinkingLevels:['off','high']}]});
  await userEvent.click(screen.getByRole('button', {name:'Local model'}));
  const option = await screen.findByRole('option',{name:'Magpie / vendor / Pretty Magpie'});
  expect(saved).toEqual(original); expect(m.onResume).not.toHaveBeenCalled();
  await userEvent.click(option);
  expect(screen.getByText('Magpie · Linked')).toBeInTheDocument();
  expect(screen.getByLabelText('Thinking')).toHaveValue('high');
  expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('Magpie / vendor / Pretty');
  expect(magpie.probeMagpie).toHaveBeenCalledTimes(1); expect(saved).toEqual(original);
  fireEvent.click(screen.getByRole('button', {name:'Start session'}));
  await waitFor(() => expect(startup.localLaunchAction).toHaveBeenCalledWith(root, agentId, 'begin', {selection:{providerId:'magpie',modelId:'vendor/family/model',thinkingLevel:'high'}}));
});

it('a removed Magpie model stays selected and unavailable, never becomes a default or a new session', async () => {
  saved = {provider:'magpie',id:'vendor/removed/model',thinking:'high'};
  availability = {schema:1,summary:'ready',providers:[{providerId:'magpie',status:'signed_in',authType:'api_key'}]};
  catalog = {schema:1,providers:[{providerId:'magpie',linked:true,models:[{modelId:saved.id,name:'Old name',thinkingLevels:['off','high']}]}]};
  const m = mount(vi.fn(resume),false);
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('Magpie / vendor / Old name'));
  catalog.providers[0].models=[];
  await userEvent.click(screen.getByRole('button', {name:'Local model'}));
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('magpie/vendor/removed/model'));
  expect(screen.queryByRole('option', {name:/Old name/})).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(saved.id).toBe('vendor/removed/model'); expect(m.onResume).not.toHaveBeenCalled();
  expect(rpc.setOfflineLocalModel).not.toHaveBeenCalled();
});

it('opens an existing agent before Link login and shows its blocked reason inside the panel', async () => {
  const m = mount(); await screen.findByText('No provider is signed in.');
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  // Empty catalogs can now be opened to discover the explicitly configured Magpie target.
  expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled();
  expect(magpie.probeMagpie).not.toHaveBeenCalled();
  expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('GPT-6.1 Sol');
  await userEvent.click(screen.getByRole('button', {name:'Local model'}));
  expect(screen.queryByRole('option',{name:/GPT-6.1 Sol/})).not.toBeInTheDocument();
  await userEvent.keyboard('{Escape}');
  expect(screen.getByRole('button',{name:'New session'})).toBeDisabled(); expect(screen.getByRole('button',{name:'Add'})).toBeDisabled();
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('This provider is not signed in.');
  expect(pty.loadProjectAgentDetail).not.toHaveBeenCalled();
  const event = vi.fn(); window.addEventListener(auth.KOTA_LOCAL_LINK_EVENT,event);
  fireEvent.click(screen.getByRole('button',{name:'Sign in'})); expect(event).toHaveBeenCalledOnce(); window.removeEventListener(auth.KOTA_LOCAL_LINK_EVENT,event);
});

it('offers linked models but leaves model/thinking as draft until Start session, then saves and starts', async () => {
  anotherLinked(); const m = mount(); await screen.findByText('OpenAI Codex (legacy) isn’t signed in.');
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  await choose(); fireEvent.change(screen.getByLabelText('Thinking'),{target:{value:'off'}});
  expect(saved).toEqual(original); expect(m.onResume).toHaveBeenCalledOnce(); expect(rpc.setOfflineLocalModel).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button',{name:'Start session'}));
  await screen.findByText('Online'); expect(m.onResume).toHaveBeenCalledTimes(2);
  expect(saved).toEqual({provider:'zai',id:'glm',thinking:'off'});
  expect(screen.getByLabelText('Thinking')).toHaveValue('off');
});

it('missing-model configuration appears immediately even while credentials and catalog are pending, with no auto-start', async () => {
  saved = null;
  vi.mocked(auth.readLocalAuthAvailability).mockReturnValue(new Promise(() => {}));
  vi.mocked(auth.readLocalModelCatalog).mockReturnValue(new Promise(() => {}));
  const m = mount(); await screen.findByText('Choose a model to start this agent.');
  expect(screen.queryByText('Starting',{exact:true})).not.toBeInTheDocument(); expect(m.onResume).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Read configuration');
});

it('a legacy default chooses model and thinking before its first explicit start', async () => {
  saved = null; anotherLinked(); const m = mount(); await screen.findByText('Choose a model to start this agent.');
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  expect(screen.getByLabelText('Thinking')).toHaveDisplayValue('—'); expect(m.onResume).not.toHaveBeenCalled();
  await choose(); expect(screen.getByLabelText('Thinking')).toHaveValue('high');
  fireEvent.click(screen.getByRole('button',{name:'Start session'}));
  await screen.findByText('Online'); expect(m.onResume).toHaveBeenCalledOnce();
});

it('shows a get_commands timeout in Current turn and unlocks before the old IPC promise settles', async () => {
  anotherLinked(); const m = mount(vi.fn(() => new Promise<ExistingAgentLaunchResult>(() => {})));
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  expect(screen.getByRole('button', {name:'Local model'})).toBeDisabled(); expect(screen.getByLabelText('Thinking')).toBeDisabled();
  await act(async () => publish('failed',{code:'auth_timeout',stage:'helper_ready'},'helper_ready'));
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  expect(screen.getByLabelText('Current turn')).toHaveTextContent('Wait for Pi commands timed out. Try again.');
  expect(screen.queryByRole('button',{name:'Cancel startup'})).not.toBeInTheDocument();
  await choose(); expect(screen.getByLabelText('Thinking')).toBeEnabled();
});

it('credential-store waiting is visible and its deadline result immediately unlocks the selectors', async () => {
  anotherLinked(); const m = mount(vi.fn(() => new Promise<ExistingAgentLaunchResult>(() => {})));
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  await act(async () => publish('running',null,'credential_store'));
  expect(screen.getByLabelText('Current turn')).toHaveTextContent('Waiting for credential store');
  expect(screen.getByRole('button', {name:'Local model'})).toBeDisabled();
  await act(async () => publish('failed',{code:'auth_store_busy',stage:'credential_store'},'credential_store'));
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  expect(screen.getByLabelText('Current turn')).toHaveTextContent('Credential storage is busy. Try again.');
});

it('a failed begin IPC is also logged in Current turn and never leaves the selectors locked', async () => {
  anotherLinked(); const m = mount(vi.fn(resume), false);
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled();
  await choose();
  await waitFor(() => expect(screen.getByRole('button', {name:'Start session'})).toBeEnabled());
  vi.mocked(startup.localLaunchAction).mockRejectedValueOnce({code:'local_config_busy',stage:'configuration_lock'});
  fireEvent.click(screen.getByRole('button', {name:'Start session'}));
  await waitFor(() => expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Kota configuration is busy. Try again.'));
  expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled();
  expect(screen.getByLabelText('Thinking')).toBeEnabled();
  expect(m.onResume).not.toHaveBeenCalled();
});

it('cancel is terminal, unlocks selectors, and a late launch failure does not resurrect the attempt', async () => {
  anotherLinked(); let complete!: (v: ExistingAgentLaunchResult) => void;
  const m = mount(vi.fn(() => new Promise<ExistingAgentLaunchResult>(done => {complete = done;})));
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  fireEvent.click(screen.getByRole('button',{name:'Cancel startup'}));
  await waitFor(() => expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled());
  await act(async () => complete({status:'failed',error:'late failure'}));
  expect(attempt?.state).toBe('cancelled'); expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Startup cancelled.');
  expect(screen.queryByText('late failure')).not.toBeInTheDocument();
});

it('declining takeover cancels the attempt and unlocks model and thinking without another click', async () => {
  availability = {schema:1,summary:'ready',providers:[{providerId:original.provider,status:'signed_in',authType:'oauth'}]};
  catalog.providers[0].linked = true;
  let decline!: (result: boolean) => void;
  const m = mount(vi.fn(() => new Promise<boolean>(resolve => {decline = resolve;})));
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  await act(async () => publish('running', null, 'session_takeover'));
  expect(screen.getByRole('button', {name:'Local model'})).toBeDisabled();
  expect(screen.getByLabelText('Thinking')).toBeDisabled();
  await act(async () => decline(false));
  await waitFor(() => expect(attempt?.state).toBe('cancelled'));
  expect(startup.localLaunchAction).toHaveBeenCalledWith(root, agentId, 'cancel', {attemptId:'attempt-1'});
  expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled();
  expect(screen.getByLabelText('Thinking')).toBeEnabled();
  expect(screen.queryByRole('button',{name:'Cancel startup'})).not.toBeInTheDocument();
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Startup cancelled.');
});

it('reopening an ongoing attempt joins it and does not issue a second start', async () => {
  const m = mount(vi.fn(() => new Promise<ExistingAgentLaunchResult>(() => {})));
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  m.rerender(<LocalWorkbenchHost {...m.props} open={null} />);
  m.rerender(<LocalWorkbenchHost {...m.props} open={{...m.open,focusRequest:2}} />);
  await screen.findByRole('button',{name:'Cancel startup'});
  expect(m.onResume).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', {name:'Local model'})).toBeDisabled();
});

it('StrictMode still starts only once and the independent configuration read completes', async () => {
  const m = mount(vi.fn(() => new Promise<ExistingAgentLaunchResult>(() => {})),false); m.unmount();
  const resumeOnce = vi.fn(() => new Promise<ExistingAgentLaunchResult>(() => {}));
  render(<StrictMode><LocalWorkbenchHost {...m.props} onResume={resumeOnce} open={{...m.open,resumeOnOpen:true}} /></StrictMode>);
  await screen.findByRole('button',{name:'Cancel startup'});
  expect(resumeOnce).toHaveBeenCalledOnce();
});

it('Link return rereads configuration and resumes the original model after relinking', async () => {
  const m = mount(); await screen.findByText('No provider is signed in.');
  await waitFor(() => expect(attempt?.state).toBe('failed'));
  availability = {schema:1,summary:'ready',providers:[{providerId:original.provider,status:'signed_in',authType:'oauth'}]}; catalog.providers[0].linked = true;
  m.rerender(<LocalWorkbenchHost {...m.props} open={{...m.open,linkReturnRequest:1}} />);
  await screen.findByText('Online'); expect(m.onResume).toHaveBeenCalledTimes(2); expect(saved).toEqual(original);
});

it('saving failure retains the draft, reports the stage, and can be retried', async () => {
  saved = null; anotherLinked();
  const m = mount(vi.fn(async () => {publish('failed',{code:'local_config_changed',stage:'configuration_save'},'configuration_save'); return {status:'failed',error:'fixture'} as ExistingAgentLaunchResult;}));
  await screen.findByText('Choose a model to start this agent.'); await choose();
  fireEvent.click(screen.getByRole('button',{name:'Start session'}));
  await waitFor(() => expect(attempt?.state).toBe('failed'));
  expect(saved).toBeNull(); expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('GLM-5-Turbo');
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Save configuration');
  expect(screen.getByRole('button',{name:'Start session'})).toBeEnabled(); expect(m.onResume).toHaveBeenCalledOnce();
});

it('saved selection stays selected when later session startup fails', async () => {
  saved = null; anotherLinked();
  mount(vi.fn(async () => {saved = {provider:'zai',id:'glm',thinking:'high'}; attempt!.saved = true; publish('failed',{code:'local_start_failed',stage:'session_ready'}); return {status:'failed',error:'fixture'} as ExistingAgentLaunchResult;}));
  await screen.findByText('Choose a model to start this agent.'); await choose();
  fireEvent.click(screen.getByRole('button',{name:'Start session'}));
  await waitFor(() => expect(attempt?.state).toBe('failed'));
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Configuration saved.');
  expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('GLM-5-Turbo');
});

it('missing model with no linked providers keeps the placeholder and sign-in route', async () => {
  saved = null; const m = mount(); await screen.findByText('No provider is signed in.');
  expect(screen.getByRole('button', {name:'Local model'})).toHaveTextContent('Choose a model');
  expect(screen.getByText('Sign in, then choose a model to continue.')).toBeInTheDocument();
  expect(m.onResume).not.toHaveBeenCalled();
});

it('metadata failure displays its stage without inventing signed-out status or retrying on a timer', async () => {
  saved = null; availability = {schema:1,summary:'error',providers:[],error:{code:'local_config_busy',stage:'configuration_lock'}};
  const m = mount(); await screen.findByText('Kota configuration is busy. Try again.');
  expect(screen.queryByText('No provider is signed in.')).not.toBeInTheDocument(); expect(m.onResume).not.toHaveBeenCalled();
  expect(auth.readLocalAuthAvailability).toHaveBeenCalledOnce();
});

it('invalid credential remains sign in again rather than being treated as not linked', async () => {
  anotherLinked(); availability.providers[0].status = 'invalid'; mount();
  await screen.findByText('OpenAI Codex (legacy) needs to sign in again.');
  expect(screen.getByRole('button',{name:'Add'})).toBeDisabled();
});

async function pendingStart() {
  availability = {schema:1,summary:'ready',providers:[{providerId:original.provider,status:'signed_in',authType:'oauth'}]};
  catalog.providers[0].linked = true;
  let finish!: (value: ExistingAgentLaunchResult) => void;
  const m = mount(vi.fn(() => new Promise<ExistingAgentLaunchResult>(resolve => {finish = resolve;})));
  await waitFor(() => expect(m.onResume).toHaveBeenCalledOnce());
  const complete = async () => { await act(async () => {snapshot = ready(); publish('ready'); finish({status:'launched'});}); };
  return {...m, complete};
}

it('pins startup steps in the activity frame, distinguishes the configuration checks, and waits for whole-attempt ready', async () => {
  await pendingStart();
  const log = screen.getByLabelText('Session startup log'), scroll = log.closest('.activity-scroll')!;
  expect(scroll).toHaveClass('pinned'); expect(scroll).not.toHaveAttribute('tabindex');
  expect(log.closest('.activity-frame')).not.toBeNull();
  expect(within(log).getAllByText('Read configuration')).toHaveLength(1);
  expect(within(log).getAllByText('Check launch configuration')).toHaveLength(1);
  const wheel = new WheelEvent('wheel',{deltaY:-500,bubbles:true,cancelable:true});
  scroll.dispatchEvent(wheel); expect(wheel.defaultPrevented).toBe(true);
  await act(async () => {attempt!.saved = true; publish('running',null,'configuration_save');});
  expect(screen.queryByText('Session ready.')).not.toBeInTheDocument();
  expect(scroll.closest('.completing')).toBeNull();
  fireEvent.animationEnd(scroll,{animationName:'local-startup-log-out'});
  expect(screen.getByLabelText('Session startup log')).toBeInTheDocument();
  expect(screen.getByRole('button', {name:'Local model'})).toBeDisabled();
});

it('distinguishes the fresh launch credential check from the panel metadata check', async () => {
  await pendingStart();
  await act(async () => publish('running',null,'credential_status'));
  const log = screen.getByLabelText('Session startup log');
  expect(within(log).getAllByText('Check credentials')).toHaveLength(1);
  expect(within(log).getAllByText('Check launch credentials')).toHaveLength(1);
});

it('finishes the ready transition only on its CSS animation end, then keeps the same frame for idle', async () => {
  const m = await pendingStart(); await m.complete();
  const log = screen.getByLabelText('Session startup log'), scroll = log.closest('.activity-scroll')!;
  const frame = scroll.closest('.activity-frame');
  expect(frame).toHaveClass('completing'); expect(log).toHaveTextContent('Session ready.');
  expect(screen.getByRole('button', {name:'Local model'})).toBeEnabled();
  const actions = vi.mocked(startup.localLaunchAction).mock.calls.length;
  fireEvent.animationEnd(log,{animationName:'local-startup-log-out'});
  fireEvent.animationEnd(scroll,{animationName:'local-workbench-flicker'});
  expect(screen.getByLabelText('Session startup log')).toBe(log);
  fireEvent.animationEnd(scroll,{animationName:'local-startup-log-out'});
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  expect(screen.getByText('Ready when you are.').closest('.activity-frame')).toBe(frame);
  expect(frame).not.toHaveClass('completing');
  expect(startup.localLaunchAction).toHaveBeenCalledTimes(actions);
});

it('releases scrolling after failure or cancellation and retains their steps', async () => {
  await pendingStart();
  await act(async () => publish('failed',{code:'auth_timeout',stage:'helper_ready'},'helper_ready'));
  let scroll = screen.getByLabelText('Session startup log').closest('.activity-scroll')!;
  expect(scroll).not.toHaveClass('pinned'); expect(scroll).toHaveAttribute('tabindex','0');
  const wheel = new WheelEvent('wheel',{deltaY:-500,bubbles:true,cancelable:true});
  scroll.dispatchEvent(wheel); expect(wheel.defaultPrevented).toBe(false);
  fireEvent.animationEnd(scroll,{animationName:'local-startup-log-out'});
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Wait for Pi commands timed out. Try again.');
  fireEvent.click(screen.getByRole('button',{name:'Start session'}));
  await screen.findByRole('button',{name:'Cancel startup'});
  fireEvent.click(screen.getByRole('button',{name:'Cancel startup'}));
  await waitFor(() => expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Startup cancelled.'));
  scroll = screen.getByLabelText('Session startup log').closest('.activity-scroll')!;
  expect(scroll).not.toHaveClass('pinned'); expect(scroll).toHaveAttribute('tabindex','0');
  fireEvent.animationEnd(scroll,{animationName:'local-startup-log-out'});
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Startup cancelled.');
  snapshot = ready();
  await screen.findByText('Online', {}, {timeout:2000});
  expect(screen.getByLabelText('Session startup log')).toHaveTextContent('Startup cancelled.');
});

it('switches directly to idle with reduced motion, including a preference change during the ready hold', async () => {
  const query = new EventTarget() as MediaQueryList;
  Object.defineProperty(query,'matches',{configurable:true,value:false});
  vi.spyOn(window,'matchMedia').mockReturnValue(query);
  const m = await pendingStart(); await m.complete();
  expect(screen.getByLabelText('Session startup log').closest('.activity-frame')).toHaveClass('completing');
  await act(async () => {Object.defineProperty(query,'matches',{configurable:true,value:true}); query.dispatchEvent(new Event('change'));});
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  expect(screen.getByText('Ready when you are.')).toBeInTheDocument();
  await act(async () => {Object.defineProperty(query,'matches',{configurable:true,value:false}); query.dispatchEvent(new Event('change'));});
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  m.unmount(); snapshot = null; attempt = null;
  Object.defineProperty(query,'matches',{configurable:true,value:true});
  const reduced = await pendingStart(); await reduced.complete();
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  expect(screen.getByText('Ready when you are.').closest('.completing')).toBeNull();
});

it('lets real output immediately take over from the success transition and never replays it on idle reopen', async () => {
  const m = await pendingStart(); await m.complete();
  const oldScroll = screen.getByLabelText('Session startup log').closest('.activity-scroll')!;
  snapshot = {...snapshot!,state:'busy',revision:2,runId:1,startedAt:new Date().toISOString(),blocks:[
    {id:'real',kind:'progress',label:'Output',text:'Actual turn output',active:true,error:false,truncated:false},
  ]};
  // An idle view polls once per second; allow that existing poll to deliver it.
  await screen.findByText('Actual turn output', {}, {timeout:2000});
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  fireEvent.animationEnd(oldScroll,{animationName:'local-startup-log-out'});
  expect(screen.getByLabelText('Current turn output')).toHaveTextContent('Actual turn output');
  snapshot = {...snapshot,revision:3,state:'ready',startedAt:null,blocks:[]};
  await screen.findByText('Ready when you are.');
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  m.rerender(<LocalWorkbenchHost {...m.props} open={null} />);
  m.rerender(<LocalWorkbenchHost {...m.props} open={{...m.open,focusRequest:2}} />);
  await screen.findByText('Online');
  expect(screen.queryByLabelText('Session startup log')).not.toBeInTheDocument();
  expect(screen.getByText('Ready when you are.').closest('.completing')).toBeNull();
});
