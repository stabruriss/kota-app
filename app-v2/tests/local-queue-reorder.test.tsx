import { fireEvent, render, screen, waitFor, act, cleanup } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useLocalQueueReorder } from '../src/hooks/useLocalQueueReorder';
import type { LocalQueueItem } from '../src/lib/local-rpc';
const item = (id:string, handed=false):LocalQueueItem => ({ kotaId:id, handed, customType:'kota.user', senderId:null,
  createdAt:'2026-10-06T00:00:00Z', state:'accepted', summary:id, truncated:false });
const items = [item('X',true), item('A'), item('B'), item('C')];
const refresh = vi.fn();
function Page({ values=items, scope='s', revision=1, submit=vi.fn().mockResolvedValue({reordered:true}) }:{values?:LocalQueueItem[];scope?:string;revision?:number;submit?:(ids:string[])=>Promise<unknown>}) {
 const r=useLocalQueueReorder({items:values,scopeKey:scope,identity:`${scope}:${revision}`,disabled:false,submit,refresh});
 return <><button onClick={()=>r.cancel()}>Stop</button><div ref={r.list} data-testid="list" aria-busy={r.pending}>
   {values.map(i=><article key={i.kotaId} {...r.rowProps(i)}>{r.grip(i)}<span>{i.summary}</span></article>)}</div>{r.error&&<p role="status">{r.error}</p>}{r.ghost}</>;
}
const keys=(button:HTMLElement,...keys:string[])=>keys.forEach(key=>fireEvent.keyDown(button,{key}));
afterEach(()=>{cleanup();vi.restoreAllMocks();refresh.mockClear();});
it('keeps X fixed and list unchanged until the ACK/read; submits the complete movable set once',async()=>{
 let resolve!:(v:unknown)=>void;const submit=vi.fn(()=>new Promise(r=>resolve=r));render(<Page submit={submit}/>);
 const grips=screen.getAllByRole('button',{name:'Drag to reorder'});expect(grips).toHaveLength(3);
 keys(grips[2],' ','ArrowUp','ArrowUp','ArrowUp','Enter');
 expect(submit).toHaveBeenCalledExactlyOnceWith(['C','A','B']);expect(screen.getByTestId('list').textContent).toBe('XABC');
 expect(grips.every(x=>(x as HTMLButtonElement).disabled)).toBe(true);expect(refresh).not.toHaveBeenCalled();
 await act(async()=>resolve({reordered:true}));expect(refresh).toHaveBeenCalledOnce();expect(grips[2]).toHaveFocus();
});
it('Escape, Stop and queue changes abandon the preview without sending',()=>{
 const submit=vi.fn();const p=render(<Page submit={submit}/>);let c=screen.getAllByRole('button',{name:'Drag to reorder'})[2];
 keys(c,' ','ArrowUp','Escape');expect(c).toHaveAttribute('aria-pressed','false');
 keys(c,' ','ArrowUp');fireEvent.click(screen.getByText('Stop'));expect(c).toHaveAttribute('aria-pressed','false');
 keys(c,' ','ArrowUp');p.rerender(<Page submit={submit} values={[...items,item('D')]} revision={2}/>);
 expect(screen.getByRole('status')).toHaveTextContent('Queue changed. Try again.');expect(submit).not.toHaveBeenCalled();
});
it.each([['queue_changed','Queue changed. Try again.'],['offline',"Couldn't reorder messages. Try again."]])('reports %s and refreshes without optimistic order',async(error,copy)=>{
 render(<Page submit={vi.fn().mockRejectedValue(error)}/>);keys(screen.getAllByRole('button',{name:'Drag to reorder'})[2],' ','ArrowUp','Enter');
 await waitFor(()=>expect(screen.getByRole('status')).toHaveTextContent(copy));expect(screen.getByTestId('list').textContent).toBe('XABC');expect(refresh).toHaveBeenCalledOnce();
});
it('scope replacement ignores an old ACK and unlocks the new queue',async()=>{
 let reject!:(e:unknown)=>void;const submit=vi.fn(()=>new Promise((_,r)=>reject=r));const p=render(<Page submit={submit}/>);
 keys(screen.getAllByRole('button',{name:'Drag to reorder'})[2],' ','ArrowUp','Enter');p.rerender(<Page scope="new" submit={submit}/>);
 await act(async()=>reject('queue_changed'));expect(screen.queryByRole('status')).toBeNull();expect(refresh).not.toHaveBeenCalled();
 expect(screen.getAllByRole('button',{name:'Drag to reorder'}).every(x=>!(x as HTMLButtonElement).disabled)).toBe(true);
});
it('cannot cross an Already sent item in the middle',async()=>{
 const submit=vi.fn().mockResolvedValue({});render(<Page values={[item('A'),item('B'),item('X',true),item('C'),item('D')]} submit={submit}/>);
 keys(screen.getAllByRole('button',{name:'Drag to reorder'})[3],' ','ArrowUp','ArrowUp','ArrowUp','Enter');
 expect(submit).toHaveBeenCalledExactlyOnceWith(['A','B','D','C']);await waitFor(()=>expect(screen.getByTestId('list')).toHaveAttribute('aria-busy','false'));
});
function rectangles() {
 const list=screen.getByTestId('list');vi.spyOn(list,'getBoundingClientRect').mockReturnValue({left:0,right:300,top:0,bottom:204,width:300,height:204} as DOMRect);
 [...list.children].forEach((row,i)=>vi.spyOn(row,'getBoundingClientRect').mockReturnValue({left:0,right:300,top:i*60,bottom:(i+1)*60,width:300,height:60} as DOMRect));
}
it('native fast release computes its own target even without a move and lost capture preceding up',async()=>{
 const submit=vi.fn().mockResolvedValue({});render(<Page submit={submit}/>);rectangles();const c=screen.getAllByRole('button',{name:'Drag to reorder'})[2];
 fireEvent.pointerDown(c,{pointerId:1,button:0,buttons:1,clientX:10,clientY:190});
 fireEvent.pointerMove(document,{pointerId:1,buttons:0,clientX:10,clientY:0});
 fireEvent.lostPointerCapture(c,{pointerId:1});fireEvent.pointerUp(document,{pointerId:1,clientX:10,clientY:0});
 expect(submit).toHaveBeenCalledExactlyOnceWith(['C','A','B']);await waitFor(()=>expect(screen.queryByText('C',{selector:'.local-queue-drag-ghost span'})).toBeNull());
});
it('outside release and lost pointer cancel remove ghost and unlock',()=>{
 const submit=vi.fn();render(<Page submit={submit}/>);rectangles();const c=screen.getAllByRole('button',{name:'Drag to reorder'})[2];
 fireEvent.pointerDown(c,{pointerId:1,button:0,buttons:1,clientX:10,clientY:190});fireEvent.pointerUp(document,{pointerId:1,clientX:400,clientY:0});
 expect(c).toHaveAttribute('aria-pressed','false');expect(document.querySelector('.local-queue-drag-ghost')).toBeNull();expect(submit).not.toHaveBeenCalled();
 fireEvent.pointerDown(c,{pointerId:2,button:0,buttons:1,clientX:10,clientY:190});fireEvent.pointerCancel(document,{pointerId:2});expect(c).toHaveAttribute('aria-pressed','false');
});

it('moving focus from the previous grip does not cancel a new pointer drag',async()=>{
 const submit=vi.fn().mockResolvedValue({});render(<Page submit={submit}/>);rectangles();
 const [a,,c]=screen.getAllByRole('button',{name:'Drag to reorder'});c.focus();
 fireEvent.pointerDown(a,{pointerId:4,button:0,buttons:1,clientX:10,clientY:80});
 expect(a).toHaveAttribute('aria-pressed','true');
 fireEvent.pointerMove(document,{pointerId:4,buttons:1,clientX:10,clientY:203});
 expect(screen.getByTestId('list').querySelector('.drop-before')?.getAttribute('data-queue-id')).toBe('C');
 fireEvent.pointerUp(document,{pointerId:4,clientX:10,clientY:203});
 expect(submit).toHaveBeenCalledExactlyOnceWith(['B','A','C']);await waitFor(()=>expect(screen.getByTestId('list')).toHaveAttribute('aria-busy','false'));
});
