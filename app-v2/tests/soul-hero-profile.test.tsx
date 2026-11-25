import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TavernModal, loadTavernHeroIncarnationProfile } from '../src/chrome/TavernModal';
import * as pty from '../src/pty-client';

let disk: pty.TavernHeroProfileDraft[];
let storageDescriptor: PropertyDescriptor | undefined;

beforeEach(() => {
  const values = new Map<string, string>();
  storageDescriptor = Object.getOwnPropertyDescriptor(window, 'localStorage');
  Object.defineProperty(window, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
    removeItem: (key: string) => values.delete(key),
  } });
  disk = [];
  vi.spyOn(pty, 'loadTavernHeroProfiles').mockImplementation(async () => structuredClone(disk));
  vi.spyOn(pty, 'saveTavernHeroProfiles').mockImplementation(async next => { disk = structuredClone(next); });
  vi.spyOn(pty, 'hasTauriRuntime').mockReturnValue(true);
  vi.spyOn(pty, 'revealTavernHeroFile').mockResolvedValue({ path: '/fixture/hero/GHOST.md' });
  vi.spyOn(pty, 'deleteTavernHero').mockResolvedValue(undefined);
});
afterEach(() => {
  cleanup(); vi.restoreAllMocks();
  if (storageDescriptor) Object.defineProperty(window, 'localStorage', storageDescriptor);
});

function hero(state: pty.TavernHeroSourceState): pty.TavernHeroProfileDraft {
  const source = state === 'native' ? undefined : {
    projectId: 'fixture-project', agentId: 'source-agent', agentCwd: '/fixture/source-agent',
    ...(state === 'relic' ? { dismissedAt: '2026-10-15T12:00:00Z' } : {}),
  };
  const p = loadTavernHeroIncarnationProfile('hero-dex')!;
  return {
    heroId: 'invited-fixture', name: 'Rowan', nameFields: { given: 'Rowan' }, kind: 'invited',
    provider: 'codex', model: 'test-model', effort: 'high', avatarId: p.avatarId,
    skills: ['fixture-skill'], ghost: 'Source Ghost, unchanged.',
    shell: 'provider: codex\nmodel: test-model\n', archived: false, dismissed: false,
    source, sourceState: state, sourceProjectName: source ? 'Atlas' : undefined,
    sourceReason: state === 'dangling'
      ? 'Source incarnation is no longer reachable. Recruiting uses the invitation snapshot without its under-table.'
      : undefined,
  };
}
async function open(state: pty.TavernHeroSourceState) {
  disk = [hero(state)];
  render(<TavernModal open onClose={vi.fn()} />);
  await userEvent.click(await screen.findByTestId('tavern-hero-invited-fixture'));
  return screen.findByRole('dialog', { name: 'Rowan profile' });
}
it.each(['linked','dangling'] as const)('%s content stays read-only while Ghost can be viewed without writing raw files',async state=>{
 const dialog=await open(state);const q=within(dialog);
 expect(q.getByRole('button',{name: 'Edit'})).toBeDisabled();
 expect(q.getByText('Read only')).toBeInTheDocument();
 expect(q.getByRole('button',{name:'Open raw file'})).toBeDisabled();
 expect(q.getByRole('button',{name:'Change avatar'})).toBeDisabled();
 expect(q.queryByLabelText('Given name')).not.toBeInTheDocument();
 expect(q.queryAllByRole('checkbox')).toHaveLength(0);
 if(state==='linked'){
   expect(q.getByText('from project Atlas · read only')).toHaveAttribute('title','from project Atlas · read only');
   expect(q.getByText('SHELL, GHOST and skills are read from the source incarnation.')).toBeInTheDocument();
 }else{
   expect(q.getByText(/Source incarnation is no longer reachable/)).toBeInTheDocument();
   expect(q.queryByText('from project Atlas · read only')).not.toBeInTheDocument();
 }
 expect(screen.queryByText('Turns',{exact:true})).not.toBeInTheDocument();expect(screen.queryByText('Commends',{exact:true})).not.toBeInTheDocument();
 // The whole Ghost reads in place; no editor opens for it.
 expect(q.getByLabelText('GHOST text')).toHaveTextContent('Source Ghost, unchanged.');
 expect(q.queryByRole('textbox')).not.toBeInTheDocument();
 await userEvent.click(q.getByRole('button',{name:'Open raw file'}));
 await userEvent.click(q.getByRole('button',{name:'Back'}));
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,300));});
 expect(pty.saveTavernHeroProfiles).not.toHaveBeenCalled();expect(pty.revealTavernHeroFile).not.toHaveBeenCalled();
});
it.each(['native','relic'] as const)('%s has normal editable controls and no source badge',async state=>{
 const dialog=await open(state);const q=within(dialog);
 expect(q.queryByText(/from project/)).not.toBeInTheDocument();
 expect(q.queryByText('Read only')).not.toBeInTheDocument();
 await userEvent.click(q.getByRole('button',{name: 'Edit'}));
 const textarea=q.getByRole('textbox',{name:'GHOST text'});
 fireEvent.change(textarea,{target:{value:'Edited Ghost.'}});
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,300));});
 expect(pty.saveTavernHeroProfiles).not.toHaveBeenCalled(); // Typing is staged; only Save writes.
 await userEvent.click(q.getByRole('button',{name:'Save'}));
 await waitFor(()=>expect(pty.saveTavernHeroProfiles).toHaveBeenCalled());
 const saved=disk.find(p=>p.heroId==='invited-fixture')!;
 expect(saved.ghost).toBe('Edited Ghost.');
 if(state==='relic')expect(saved.source).toMatchObject({dismissedAt:'2026-10-15T12:00:00Z'});
});
it('stages name, avatar, Ghost and skills together and writes them with one Save',async()=>{
 const dialog=await open('native');const q=within(dialog);
 expect(q.queryByRole('textbox')).not.toBeInTheDocument();expect(q.queryByRole('checkbox')).not.toBeInTheDocument();
 expect(q.queryByRole('button',{name:'Save'})).not.toBeInTheDocument();
 await userEvent.click(q.getByRole('button',{name:'Edit'}));
 expect(q.getByText('Editing')).toBeInTheDocument();expect(q.getByText('No changes yet')).toBeInTheDocument();
 expect(q.getByRole('button',{name:'Save'})).toBeDisabled();
 expect(q.queryByRole('button',{name:'Remove Hero'})).not.toBeInTheDocument();
 expect(q.queryByRole('button',{name:'Open raw file'})).not.toBeInTheDocument();
 fireEvent.change(q.getByLabelText('Given name'),{target:{value:'Juniper'}});
 fireEvent.change(q.getByRole('textbox',{name:'GHOST text'}),{target:{value:'Edited Ghost.'}});
 const skill=q.getAllByRole('checkbox').find(box=>(box as HTMLInputElement).checked)!;
 await userEvent.click(skill);
 await userEvent.click(q.getByRole('button',{name:'Change avatar'}));
 await userEvent.click(q.getAllByRole('radio').find(option=>option.getAttribute('aria-checked')!=='true')!);
 expect(q.getByText('Unsaved changes')).toBeInTheDocument();
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,300));});
 expect(pty.saveTavernHeroProfiles).not.toHaveBeenCalled();
 const before=hero('native');
 await userEvent.click(q.getByRole('button',{name:'Save'}));
 await waitFor(()=>expect(disk.find(p=>p.heroId==='invited-fixture')).toMatchObject({name:'Juniper',ghost:'Edited Ghost.',skills:[]}));
 expect(disk.find(p=>p.heroId==='invited-fixture')!.avatarId).not.toBe(before.avatarId);
 expect(disk.find(p=>p.heroId==='invited-fixture')).toMatchObject({provider:before.provider,model:before.model,effort:before.effort});
 const saved=within(screen.getByRole('dialog',{name:'Juniper profile'}));
 expect(saved.getByText('Saved')).toBeInTheDocument();expect(saved.queryByText('Editing')).not.toBeInTheDocument();
 expect(saved.queryByRole('textbox')).not.toBeInTheDocument();
 expect(saved.getByLabelText('GHOST text')).toHaveTextContent('Edited Ghost.');
 expect(saved.getByText('No active skills.')).toBeInTheDocument();
});
it('asks before Cancel or Back drops staged Hero edits, and writes only on Save and Leave',async()=>{
 const dialog=await open('native');const q=within(dialog);
 await userEvent.click(q.getByRole('button',{name:'Edit'}));
 fireEvent.change(q.getByRole('textbox',{name:'GHOST text'}),{target:{value:'Draft Ghost.'}});
 await userEvent.click(q.getByRole('button',{name:'Cancel'}));
 let prompt=within(screen.getByRole('alertdialog',{name:'Discard changes?'}));
 expect(prompt.getByText('Your changes to GHOST will be lost.')).toBeInTheDocument();
 await userEvent.keyboard('{Escape}');
 expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
 expect(q.getByRole('textbox',{name:'GHOST text'})).toHaveValue('Draft Ghost.');
 await userEvent.keyboard('{Escape}');
 await userEvent.click(within(screen.getByRole('alertdialog')).getByRole('button',{name:'Discard Changes'}));
 expect(q.getByLabelText('GHOST text')).toHaveTextContent('Source Ghost, unchanged.');
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,300));});
 expect(pty.saveTavernHeroProfiles).not.toHaveBeenCalled();
 await userEvent.click(q.getByRole('button',{name:'Edit'}));
 fireEvent.change(q.getByRole('textbox',{name:'GHOST text'}),{target:{value:'Kept Ghost.'}});
 await userEvent.click(q.getByRole('button',{name:'Back'}));
 prompt=within(screen.getByRole('alertdialog',{name:'Save changes before leaving?'}));
 expect(prompt.getByText('You have unsaved changes to GHOST.')).toBeInTheDocument();
 await userEvent.click(prompt.getByRole('button',{name:'Save and Leave'}));
 await waitFor(()=>expect(disk.find(p=>p.heroId==='invited-fixture')?.ghost).toBe('Kept Ghost.'));
 expect(screen.queryByRole('dialog',{name:'Rowan profile'})).not.toBeInTheDocument();
});
it('holds Save and says why while a Hero name is empty or taken',async()=>{
 disk=[hero('native'),{...hero('native'),heroId:'other-fixture',name:'Taken',nameFields:{given:'Taken'}}];
 render(<TavernModal open onClose={vi.fn()} />);
 await userEvent.click(await screen.findByTestId('tavern-hero-invited-fixture'));
 const q=within(await screen.findByRole('dialog',{name:'Rowan profile'}));
 await userEvent.click(q.getByRole('button',{name:'Edit'}));
 const given=q.getByLabelText('Given name');
 fireEvent.change(given,{target:{value:' '}});
 expect(q.getByText('Enter a given name to save')).toBeInTheDocument();expect(q.getByRole('button',{name:'Save'})).toBeDisabled();
 fireEvent.change(given,{target:{value:'Taken'}});
 expect(q.getByText('Name already exists in Tavern.')).toBeInTheDocument();expect(q.getByRole('button',{name:'Save'})).toBeDisabled();
 fireEvent.change(given,{target:{value:'Juniper'}});
 expect(q.getByText('Unsaved changes')).toBeInTheDocument();expect(q.getByRole('button',{name:'Save'})).toBeEnabled();
});
it('returns to the saved model, effort and avatar when the staged provider goes back to the saved one',async()=>{
 const dialog=await open('native');const q=within(dialog);
 await userEvent.click(q.getByRole('button',{name:'Edit'}));
 await userEvent.click(q.getByRole('button',{name:/^Provider /}));
 await userEvent.click(q.getAllByRole('option').find(option=>!/Codex/.test(option.textContent??'')&&!(option as HTMLButtonElement).disabled)!);
 expect(q.getByText('Unsaved changes')).toBeInTheDocument();
 await userEvent.click(q.getByRole('button',{name:/^Provider /}));
 await userEvent.click(q.getByRole('option',{name:/^Codex\b/}));
 expect(q.getByText('No changes yet')).toBeInTheDocument();
 expect(q.getByPlaceholderText('Exact model ID')).toHaveValue('test-model');
 await userEvent.click(q.getByRole('button',{name:'Cancel'}));
 expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,300));});
 expect(pty.saveTavernHeroProfiles).not.toHaveBeenCalled();
});
it('Remove Hero archives a live invitation through the Tavern saver without deleting its source',async()=>{
 const dialog=await open('linked');
 await userEvent.click(within(dialog).getByRole('button',{name:'Remove Hero'}));
 await waitFor(()=>expect(disk.find(p=>p.heroId==='invited-fixture')?.archived).toBe(true));
 expect(disk.find(p=>p.heroId==='invited-fixture')?.source).toMatchObject({agentId:'source-agent'});
 expect(pty.deleteTavernHero).not.toHaveBeenCalled();
});
it('Reset All Heroes archives live/dangling sources without renaming them on a factory-name collision',async()=>{
 disk=[{...hero('linked'),name:'Dex'},{...hero('dangling'),heroId:'dangling-fixture',name:'Cc'}];
 render(<TavernModal open onClose={vi.fn()}/>);
 await waitFor(()=>expect(screen.getByRole('button',{name:'Reset All Heroes'})).toBeEnabled());
 await userEvent.click(screen.getByRole('button',{name:'Reset All Heroes'}));
 await userEvent.click(within(screen.getByRole('dialog',{name:'Reset All Heroes?'})).getByRole('button',{name:'Reset All Heroes'}));
 await waitFor(()=>expect(pty.saveTavernHeroProfiles).toHaveBeenCalled());
 expect(disk.find(p=>p.heroId==='invited-fixture')).toMatchObject({name:'Dex',archived:true});
 expect(disk.find(p=>p.heroId==='dangling-fixture')).toMatchObject({name:'Cc',archived:true});
});
