import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { DanceRouteLog, danceEventText } from '../src/chrome/DanceRouteLog';
import type { DanceEvent } from '../src/lib/dance-events';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));
const selected = (id: number): DanceEvent => ({ id, occurredAt: '2026-10-10T08:00:00.129+00:00', kind: 'selection', outcome: 'selected',
  target: { agentId: 'agent-a', name: '座山雕' }, candidate: { agentId: 'agent-a', name: '座山雕', probability: .82 }, threshold: .75, strategy: 'context' });
type Push = (event: { payload: { projectRoot: string; event: DanceEvent } }) => void;
let push: Push; const stop = vi.fn();
beforeEach(() => { vi.clearAllMocks(); vi.mocked(invoke).mockResolvedValue([]); vi.mocked(listen).mockImplementation(async (_name, fn) => { push = fn as Push; return stop; }); });
afterEach(() => { vi.clearAllMocks(); });
const footer = <span>Jev · Verified</span>;

it('formats selected, sentinel and direct without inventing probability for bypasses', () => {
  const event = selected(1); if (event.kind !== 'selection') throw Error('fixture');
  expect(danceEventText(event)).toBe('座山雕 selected • Certainty 0.82');
  expect(danceEventText({ ...event, outcome: 'sentinel', reason: 'below_threshold', candidate: { agentId: 'agent-b', name: '颦儿', probability: .61 } })).toBe('座山雕 sentinel • Candidate 颦儿 0.61 ≤ 0.75');
  for (const [reason, text] of [['empty_text', 'Empty message'], ['no_candidates', 'No candidates'], ['target_left', 'Target left'], ['only_seat', 'Only seat'], ['only_candidate', 'Only candidate']] as const) {
    const outcome = reason.startsWith('only_') ? 'direct' : 'sentinel';
    expect(danceEventText({ ...event, outcome, reason, candidate: undefined })).toBe(`座山雕 ${outcome} • ${text}`);
  }
});
it.each([['jev_auth','Jev authentication failed. Update the key in Puppeteer.'],['jev_timeout','Jev did not respond within 3 seconds.'],['jev_unreachable','Jev could not be reached.'],['dance_record_failed','Could not record this message. It was not sent.']])('uses existing error copy for %s', (code, text) => {
  expect(danceEventText({ id: 1, occurredAt: '', kind: code.startsWith('jev_') ? 'jev_error' : 'dance_error', code })).toBe(text);
});
it('subscribes before reading, merges concurrent pushes by ID and retains only fifteen newest across turns', async () => {
  let resolve!: (events: DanceEvent[]) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise(done => { resolve = done as typeof resolve; }));
  render(<DanceRouteLog projectRoot="/a" footer={footer} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledWith('puppeteer_dance_events', { projectRoot: '/a' }));
  expect(vi.mocked(listen).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(invoke).mock.invocationCallOrder[0]);
  act(() => { push({ payload: { projectRoot: '/a', event: selected(16) } }); push({ payload: { projectRoot: '/other', event: selected(200) } }); });
  await act(async () => resolve(Array.from({ length: 15 }, (_, i) => selected(15 - i))));
  expect(screen.getAllByRole('listitem').map(row => row.dataset.eventId)).toEqual(Array.from({ length: 15 }, (_, i) => String(16 - i)));
  act(() => push({ payload: { projectRoot: '/a', event: selected(16) } }));
  expect(screen.getAllByRole('listitem')).toHaveLength(15);
  const time = screen.getAllByRole('listitem')[0].querySelector('time')!;
  expect(time.dateTime).toBe('2026-10-10T08:00:00.129+00:00'); expect(time.textContent).toMatch(/\.12$/);
});
it('unsubscribes on close, rereads on reopen and does not leak late snapshots across projects', async () => {
  let resolveA!: (events: DanceEvent[]) => void;
  vi.mocked(invoke).mockImplementation(async (_name, args) => (args as { projectRoot: string }).projectRoot === '/a' ? new Promise<DanceEvent[]>(done => { resolveA = done; }) : []);
  const view = render(<DanceRouteLog projectRoot="/a" footer={footer} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
  const oldPush = push;
  view.rerender(<DanceRouteLog projectRoot="/b" footer={footer} />);
  await screen.findByText('Waiting for a Dance decision…');
  await act(async () => { resolveA([selected(99)]); oldPush({ payload: { projectRoot: '/a', event: selected(100) } }); });
  expect(screen.queryAllByRole('listitem')).toHaveLength(0); expect(stop).toHaveBeenCalledTimes(1);
  view.unmount(); expect(stop).toHaveBeenCalledTimes(2);
  render(<DanceRouteLog projectRoot="/b" footer={footer} />);
  await screen.findByText('Waiting for a Dance decision…'); expect(invoke).toHaveBeenCalledTimes(3);
});
it.each(['dance_failed', 'dance_busy'])('shows %s separately from an empty log and retries explicitly without polling', async code => {
  vi.mocked(invoke).mockRejectedValueOnce({ code });
  render(<DanceRouteLog projectRoot="/a" footer={footer} />);
  await screen.findByText('Dance log unavailable.');
  expect(screen.queryByText('Waiting for a Dance decision…')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Retry log' }));
  await screen.findByText('Waiting for a Dance decision…'); expect(invoke).toHaveBeenCalledTimes(2);
});
it('keeps pushed rows when the snapshot is busy and merges the complete log on explicit retry', async () => {
  let rejectSnapshot!: (reason: unknown) => void;
  vi.mocked(invoke).mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectSnapshot = reject; }))
    .mockResolvedValueOnce([selected(2), selected(1)]);
  render(<DanceRouteLog projectRoot="/a" footer={footer} />);
  await waitFor(() => expect(invoke).toHaveBeenCalledTimes(1));
  act(() => push({ payload: { projectRoot: '/a', event: selected(2) } }));
  await act(async () => rejectSnapshot({ code: 'dance_busy' }));
  expect(screen.getAllByRole('listitem').map(row => row.dataset.eventId)).toEqual(['2']);
  fireEvent.click(screen.getByRole('button', { name: 'Retry log' }));
  await waitFor(() => expect(screen.getAllByRole('listitem')).toHaveLength(2));
  expect(screen.queryByRole('button', { name: 'Retry log' })).not.toBeInTheDocument();
  expect(invoke).toHaveBeenCalledTimes(2); expect(stop).toHaveBeenCalledTimes(1);
  act(() => push({ payload: { projectRoot: '/a', event: selected(3) } }));
  expect(screen.getAllByRole('listitem').map(row => row.dataset.eventId)).toEqual(['3', '2', '1']);
});
it('keeps old-row and horizontal scroll positions while new events arrive', async () => {
  vi.mocked(invoke).mockResolvedValue(Array.from({ length: 15 }, (_, i) => selected(15 - i)));
  render(<DanceRouteLog projectRoot="/a" footer={footer} />);
  await waitFor(() => expect(screen.getAllByRole('listitem')).toHaveLength(15));
  const viewport = screen.getByRole('region', { name: 'Dance events' });
  Object.defineProperties(viewport, { clientHeight: { value: 180 }, offsetHeight: { value: 180 }, scrollHeight: { value: 540 } });
  viewport.scrollTop = 72; viewport.scrollLeft = 23; fireEvent.scroll(viewport);
  act(() => push({ payload: { projectRoot: '/a', event: selected(16) } }));
  expect(viewport.scrollTop).toBe(108); expect(viewport.scrollLeft).toBe(23); expect(viewport.style.height).toBe('180px');
  fireEvent.click(screen.getByRole('button', { name: '1 new ↑' })); expect(viewport.scrollTop).toBe(0);
});
