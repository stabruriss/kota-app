import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { StringRouteLog, stringEventText } from '../src/chrome/StringRouteLog';
import type { StringRouteEvent } from '../src/lib/string-routing';

const selection = (id = 1): StringRouteEvent => ({ id, occurredAt: '2026-10-10T01:50:54.129Z', kind: 'selection', outcome: 'selected',
  selected:{provider:'openai',model:'gpt-6-astra',thinkingLevel:'high'},candidate:{routeModel:'model-1',provider:'openai',model:'gpt-6-astra',probability:.96},threshold:.85 });
const failure = (id: number, message = 'Connection error.'): StringRouteEvent => ({id,occurredAt:'2026-10-10T01:50:54Z',kind:'model_error',provider:'openai',model:'gpt-6-astra',message});
afterEach(() => vi.restoreAllMocks());

it('shows actual IDs and bracketed thinking, never a config number or independent Thinking segment', () => {
  render(<StringRouteLog events={[selection()]} jevStatus="Configured" />);
  expect(screen.getByText('openai/gpt-6-astra[high] selected • Certainty 0.96')).toBeVisible();
  expect(screen.queryByText(/Thinking|Model 1|Selected model|Actual model|This turn|Attempts|Newest first/)).not.toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('Jev · Configured');
});
it.each(['retained','default'] as const)('keeps the %s model distinct from the unapplied candidate probability', outcome => {
  const event = selection(); if (event.kind !== 'selection') throw Error('fixture');
  event.outcome=outcome; event.candidate={routeModel:'model-2',provider:'google',model:'gemini-pro',probability:.72};
  expect(stringEventText(event)).toBe(`openai/gpt-6-astra[high] ${outcome} • Candidate google/gemini-pro 0.72 ≤ 0.85`);
});
it('preserves server newest-first order, duplicate real failures, complete error text and original timestamps', () => {
  const events = [failure(3,'Connection error.\nProvider details'),failure(2),{id:1,kind:'jev_error',code:'jev_auth',message:'Jev authentication failed.'} as StringRouteEvent];
  const original = structuredClone(events);
  const {rerender} = render(<StringRouteLog events={events} jevStatus="Rejected" />);
  const rows=screen.getAllByRole('listitem');
  expect(rows.map(row=>row.dataset.eventId)).toEqual(['3','2','1']);
  expect(rows[0]).toHaveTextContent('Connection error. ↵ Provider details');
  expect(rows[0].querySelector('.string-log-message')?.getAttribute('title')).toBe('openai/gpt-6-astra • Connection error.\nProvider details');
  const time=rows[0].querySelector('time')!;
  expect(time.dateTime).toBe(events[0].occurredAt); expect(time.textContent).not.toContain('.00');
  expect(time.title).toContain('2026'); expect(rows[2].querySelector('.string-log-time')).toHaveTextContent('—');
  rerender(<StringRouteLog events={[...events]} jevStatus="Rejected" />);
  expect(events).toEqual(original); expect(rows[0].querySelector('time')?.dateTime).toBe(time.dateTime);
});
it('shows hundredths only if the intake timestamp contains that precision', () => {
  const {container} = render(<StringRouteLog events={[selection(),failure(2),{...failure(3),occurredAt:'2026-10-10T01:50:54.1Z'}]} jevStatus="Configured" />);
  const times=container.querySelectorAll('time');
  expect(times[0].textContent).toMatch(/\.12$/); expect(times[1].textContent).not.toContain('.'); expect(times[2].textContent).not.toContain('.');
});
it('uses an empty state without manufacturing an event', () => {
  render(<StringRouteLog events={[]} jevStatus="Not set" />);
  expect(screen.getByText('Waiting for an Auto String event…')).toBeVisible();
  expect(screen.queryAllByRole('listitem')).toHaveLength(0);
});
it('caps the viewport at five rows and retains an older-row anchor with new-event count', () => {
  const events=Array.from({length:15},(_,i)=>failure(15-i));
  const {rerender}=render(<StringRouteLog events={events} jevStatus="Configured" />);
  const viewport=screen.getByRole('region',{name:'Auto String events'});
  Object.defineProperties(viewport,{clientHeight:{configurable:true,get:()=>180},offsetHeight:{configurable:true,get:()=>180},scrollHeight:{configurable:true,get:()=>540}});
  viewport.scrollTop=72; fireEvent.scroll(viewport);
  rerender(<StringRouteLog events={[failure(16),...events.slice(0,14)]} jevStatus="Configured" />);
  expect(viewport.scrollTop).toBe(108);
  expect(screen.getAllByRole('listitem')[3].dataset.eventId).toBe('13');
  expect(viewport.style.height).toBe('180px'); expect(viewport.style.overflowY).toBe('auto');
  fireEvent.click(screen.getByRole('button',{name:'1 new ↑'}));
  expect(viewport.scrollTop).toBe(0); expect(screen.queryByRole('button',{name:/new ↑/})).not.toBeInTheDocument();
});
it('only widens for visible long rows and avoids vertical overflow for five or fewer', () => {
  const style=document.createElement('style'); style.textContent=readFileSync(resolve(__dirname,'../src/styles/string-route-log.css'),'utf8'); document.head.append(style);
  vi.spyOn(Element.prototype,'getBoundingClientRect').mockImplementation(function(this:Element) {
    return {width:this.classList.contains('string-log-message') ? (this.textContent?.includes('LONG') ? 1000:100):80} as DOMRect;
  });
  try {
    const events=Array.from({length:6},(_,i)=>failure(6-i,i===5?'LONG':'Short'));
    const {rerender}=render(<StringRouteLog events={events} jevStatus="Configured" />);
    const viewport=screen.getByRole('region',{name:'Auto String events'});
    Object.defineProperties(viewport,{clientWidth:{configurable:true,value:784},offsetHeight:{configurable:true,value:180},clientHeight:{configurable:true,value:180}});
    fireEvent.scroll(viewport);
    expect(screen.getByRole('list').style.width).toBe('784px');
    viewport.scrollTop=36; fireEvent.scroll(viewport);
    expect(parseFloat(screen.getByRole('list').style.width)).toBeGreaterThan(1000);
    viewport.scrollTop=0; rerender(<StringRouteLog events={events.slice(0,5)} jevStatus="Configured" />);
    expect(viewport.style.overflowY).toBe('hidden'); expect(screen.getByRole('list').style.width).toBe('784px');
  } finally {style.remove();}
});
