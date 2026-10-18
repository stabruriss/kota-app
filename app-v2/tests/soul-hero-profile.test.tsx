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
 expect(q.getByRole('button',{name:'Edit Shell'})).toBeDisabled();
 expect(q.getByRole('button',{name:'Read only'})).toBeDisabled();
 expect(q.getByRole('button',{name:'Open raw file'})).toBeDisabled();
 expect(q.getByRole('button',{name:'Rowan'})).toBeDisabled();
 expect(q.getByRole('button',{name:'Change avatar'})).toBeDisabled();
 for(const checkbox of q.queryAllByRole('checkbox'))expect(checkbox).toBeDisabled();
 if(state==='linked'){
   expect(q.getByText('from project Atlas · read only')).toHaveAttribute('title','from project Atlas · read only');
   expect(q.getByText('SHELL, GHOST and skills are read from the source incarnation.')).toBeInTheDocument();
 }else{
   expect(q.getByText(/Source incarnation is no longer reachable/)).toBeInTheDocument();
   expect(q.queryByText('from project Atlas · read only')).not.toBeInTheDocument();
 }
 expect(screen.queryByText('Turns',{exact:true})).not.toBeInTheDocument();expect(screen.queryByText('Commends',{exact:true})).not.toBeInTheDocument();
 await userEvent.click(q.getByRole('button',{name:'Source Ghost, unchanged.'}));
 expect(q.getByRole('textbox')).toHaveAttribute('readonly');
 await userEvent.click(q.getByRole('button',{name:'Open raw file'}));
 await userEvent.click(q.getByRole('button',{name:'Back'}));
 await act(async()=>{await new Promise(resolve=>setTimeout(resolve,300));});
 expect(pty.saveTavernHeroProfiles).not.toHaveBeenCalled();expect(pty.revealTavernHeroFile).not.toHaveBeenCalled();
});
it.each(['native','relic'] as const)('%s has normal editable controls and no source badge',async state=>{
 const dialog=await open(state);const q=within(dialog);
 expect(q.queryByText(/from project/)).not.toBeInTheDocument();
 expect(q.getByRole('button',{name:'Edit Shell'})).toBeEnabled();
 await userEvent.click(q.getByRole('button',{name:'Edit GHOST'}));
 const textarea=q.getByRole('textbox');expect(textarea).not.toHaveAttribute('readonly');
 fireEvent.change(textarea,{target:{value:'Edited Ghost.'}});
 await userEvent.click(q.getByRole('button',{name:'Save GHOST'}));
 await waitFor(()=>expect(pty.saveTavernHeroProfiles).toHaveBeenCalled());
 const saved=disk.find(p=>p.heroId==='invited-fixture')!;
 expect(saved.ghost).toBe('Edited Ghost.');
 if(state==='relic')expect(saved.source).toMatchObject({dismissedAt:'2026-10-15T12:00:00Z'});
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
