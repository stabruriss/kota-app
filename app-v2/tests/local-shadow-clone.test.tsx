import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { LocalShadowClonePanel } from '../src/chrome/LocalShadowClonePanel';
import * as clones from '../src/lib/local-shadow-clone';
import type { LocalWorkbenchView } from '../src/lib/local-rpc';

const scope = { projectRoot: '/fixture', agentId: 'parent', sessionId: 'parent-session', epoch: 1 };
const activity = vi.fn();
const props = { projectRoot: scope.projectRoot, agentId: scope.agentId, scope, now: Date.parse('2026-10-09T00:00:12Z'), onActivityChange: activity };
const row = (id = 1, state: clones.LocalShadowClone['state'] = 'running'): clones.LocalShadowClone => ({
  id, state, startedAt: '2026-10-09T00:00:00Z', elapsedMs: 9000, lastReply: '', view: null,
});
const view = (rows = [row()]): clones.LocalShadowClones => ({ scope, clones: rows });
const open = () => act(async () => { fireEvent.click(screen.getByRole('button', { name: /Kage Bunshin, \d+ clones?/ })); });

beforeEach(() => {
  vi.restoreAllMocks(); activity.mockClear();
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  vi.spyOn(clones, 'readLocalShadowClones').mockResolvedValue(view());
  vi.spyOn(clones, 'controlLocalShadowClone').mockResolvedValue({});
});

it('shows the generated tab only when a copy exists and cycles the seven display names without model data', async () => {
  vi.mocked(clones.readLocalShadowClones).mockResolvedValueOnce(view([]));
  const ui = render(<LocalShadowClonePanel {...props} />);
  await waitFor(() => expect(clones.readLocalShadowClones).toHaveBeenCalledTimes(1));
  expect(screen.queryByRole('button', { name: /Kage Bunshin/ })).not.toBeInTheDocument();
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([row(8)]));
  ui.rerender(<LocalShadowClonePanel {...props} revision={1} />);
  await screen.findByRole('button', { name: 'Kage Bunshin, 1 clone' }); await open();
  const dialog = screen.getByRole('dialog', { name: 'Kage Bunshin' });
  expect(within(dialog).getByText('Kambei')).toBeInTheDocument();
  expect(within(dialog).getByText('Clone #8')).toBeInTheDocument();
  expect(within(dialog).getByText('00:12')).toBeInTheDocument();
  expect(within(dialog).getByText('1 clone · 1 running')).toBeInTheDocument();
  expect(dialog.querySelector('img.samurai-avatar')?.getAttribute('src')).toContain('kambei.webp');
  expect(activity).toHaveBeenCalledWith(true);
  expect(clones.samuraiNames).toHaveLength(7);
});

it('expands directly into the shared bounded activity DOM and reads only the selected monitor', async () => {
  const monitored = row();
  monitored.view = { runId: 1, revision: 2, dropped: 0, blocks: [
    { id: 'thinking', kind: 'thinking', label: 'Thinking', text: 'Synthetic thought', active: true, error: false, truncated: false, startedAt: '2026-10-09T00:00:00Z' },
    { id: 'answer', kind: 'end', label: 'Answer', text: 'Synthetic final reply', active: false, error: false, truncated: false },
  ] } as LocalWorkbenchView;
  let reads = 0, finishList!: (result: clones.LocalShadowClones) => void;
  vi.mocked(clones.readLocalShadowClones).mockImplementation(async (_root, _agent, monitor) => {
    if (++reads === 2) return new Promise(resolve => { finishList = resolve; });
    return view([monitor === 1 ? monitored : row()]);
  });
  render(<LocalShadowClonePanel {...props} />);
  await screen.findByRole('button', { name: 'Kage Bunshin, 1 clone' }); await open();
  await waitFor(() => expect(reads).toBe(2));
  fireEvent.click(screen.getByRole('button', { name: 'Monitor Kambei, Clone #1' }));
  expect(reads).toBe(2);
  await act(async () => finishList(view()));
  await screen.findByText('Synthetic final reply');
  expect(clones.readLocalShadowClones).toHaveBeenLastCalledWith('/fixture', 'parent', 1);
  const output = screen.getByLabelText('Clone #1 output');
  expect(output.closest('.activity-frame')).not.toBeNull();
  expect(output.querySelectorAll('.stream-block')).toHaveLength(2);
  expect(within(output).getByRole('heading', { name: 'Answer' })).toBeInTheDocument();
  expect(within(output).getByLabelText('Thinking elapsed')).toHaveTextContent('00:12');
  expect(screen.queryByText(/Model|Read only|90 minutes/)).not.toBeInTheDocument();
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Kage Bunshin, 1 clone' })).toHaveFocus();
});

it('Stop retains the copy and only Stopped exposes Dismiss, which removes its row after ACK', async () => {
  render(<LocalShadowClonePanel {...props} />);
  await screen.findByRole('button', { name: 'Kage Bunshin, 1 clone' }); await open();
  expect(screen.queryByRole('button', { name: 'Dismiss Clone #1' })).not.toBeInTheDocument();
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([row(1, 'stopped')]));
  fireEvent.click(screen.getByRole('button', { name: 'Stop Clone #1' }));
  const dismiss = await screen.findByRole('button', { name: 'Dismiss Clone #1' });
  expect(clones.controlLocalShadowClone).toHaveBeenCalledWith(scope, 1, 'stop');
  expect(dismiss).toHaveAttribute('title', 'Delete this clone and its records');
  expect(screen.getByText('00:09')).toBeInTheDocument();
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([]));
  fireEvent.click(dismiss);
  await waitFor(() => expect(screen.queryByRole('button', { name: /Kage Bunshin, / })).not.toBeInTheDocument());
  expect(clones.controlLocalShadowClone).toHaveBeenLastCalledWith(scope, 1, 'destroy');
  expect(activity).toHaveBeenLastCalledWith(false);
});

it('keeps records on an action failure, unlocks controls and never renders a native error or path', async () => {
  vi.mocked(clones.controlLocalShadowClone).mockRejectedValue(new Error('/private/synthetic-secret'));
  render(<LocalShadowClonePanel {...props} />);
  await screen.findByRole('button', { name: 'Kage Bunshin, 1 clone' }); await open();
  fireEvent.click(screen.getByRole('button', { name: 'Stop Clone #1' }));
  await screen.findByText("Couldn't update this clone. Try again.");
  expect(screen.getByRole('button', { name: 'Stop Clone #1' })).toBeEnabled();
  expect(document.body).not.toHaveTextContent('synthetic-secret');
});

it('uses the existing clock, merges concurrent reads, ignores a replaced owner and suspends hidden reads', async () => {
  let finish!: (value: clones.LocalShadowClones) => void;
  vi.mocked(clones.readLocalShadowClones).mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const ui = render(<LocalShadowClonePanel {...props} />);
  ui.rerender(<LocalShadowClonePanel {...props} now={props.now + 1000} />);
  expect(clones.readLocalShadowClones).toHaveBeenCalledTimes(1);
  const next = { ...scope, sessionId: 'next', epoch: 2 };
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue({ scope: next, clones: [] });
  ui.rerender(<LocalShadowClonePanel {...props} scope={next} now={props.now + 2000} />);
  await waitFor(() => expect(clones.readLocalShadowClones).toHaveBeenCalledTimes(2));
  await act(async () => finish(view()));
  expect(screen.queryByRole('button', { name: /Kage Bunshin, / })).not.toBeInTheDocument();
  Object.defineProperty(document, 'hidden', { configurable: true, value: true });
  ui.rerender(<LocalShadowClonePanel {...props} scope={next} now={props.now + 3000} />);
  expect(clones.readLocalShadowClones).toHaveBeenCalledTimes(2);
  ui.unmount(); expect(activity).toHaveBeenLastCalledWith(false);
});

it('keeps the stopped tab available after the parent exits and uses its last confirmed scope for Dismiss', async () => {
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([row(1, 'stopped')]));
  render(<LocalShadowClonePanel {...props} scope={undefined} />);
  await screen.findByRole('button', { name: 'Kage Bunshin, 1 clone' }); await open();
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss Clone #1' }));
  await waitFor(() => expect(clones.controlLocalShadowClone).toHaveBeenCalledWith(scope, 1, 'destroy'));
});

it('uses only the B WebP artwork for the seven names and keeps Stop distinct from Dismiss', async () => {
  const rows = Array.from({ length: 7 }, (_, index) => row(index + 1, index === 6 ? 'stopped' : index === 5 ? 'finished' : 'running'));
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view(rows));
  render(<LocalShadowClonePanel {...props} />);
  await screen.findByRole('button', { name: 'Kage Bunshin, 7 clones' }); await open();
  const dialog = screen.getByRole('dialog', { name: 'Kage Bunshin' });
  expect(Array.from(dialog.querySelectorAll('.clone-identity strong'), node => node.textContent)).toEqual(clones.samuraiNames);
  const portraits = Array.from(dialog.querySelectorAll<HTMLImageElement>('.samurai-avatar'), node => node.getAttribute('src'));
  expect(portraits.map(src => src?.split('/').pop())).toEqual(clones.samuraiNames.map(name => `${name.toLowerCase()}.webp`));
  expect(dialog.querySelector('img.kage-icon')?.getAttribute('src')).toContain('kage-bunshin.webp');
  expect(dialog.querySelector('img[src$=".png"], img[src$=".svg"]')).toBeNull();
  expect(within(dialog).getAllByRole('button', { name: /^Stop Clone/ })).toHaveLength(6);
  expect(within(dialog).getByRole('button', { name: 'Dismiss Clone #7' })).toHaveAttribute('title', 'Delete this clone and its records');
  expect(within(dialog).getByText('Idle')).toBeInTheDocument();
});

it('Dismiss waits for the real ACK, removes the selected monitor and restores focus before hiding the last tab', async () => {
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([row(1, 'stopped'), row(2, 'stopped')]));
  render(<section className="local-workbench" tabIndex={-1} aria-label="Fixture workbench"><LocalShadowClonePanel {...props} /></section>);
  await screen.findByRole('button', { name: 'Kage Bunshin, 2 clones' }); await open();
  const monitor = screen.getByRole('button', { name: 'Monitor Kambei, Clone #1' });
  fireEvent.click(monitor);
  const region = screen.getByRole('region', { name: 'Clone #1 monitor' });
  expect(monitor).toHaveAttribute('aria-controls', region.id);
  expect(within(region).queryByRole('heading', { name: 'Current turn' })).not.toBeInTheDocument();
  let acknowledge!: () => void;
  vi.mocked(clones.controlLocalShadowClone).mockImplementationOnce(() => new Promise(resolve => { acknowledge = () => resolve({}); }));
  const dismiss = screen.getByRole('button', { name: 'Dismiss Clone #1' });
  dismiss.focus(); fireEvent.click(dismiss);
  expect(dismiss).toBeDisabled(); expect(region).toBeInTheDocument();
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([row(2, 'stopped')]));
  await act(async () => acknowledge());
  await waitFor(() => expect(screen.getByRole('button', { name: 'Monitor Gorobei, Clone #2' })).toHaveFocus());
  expect(screen.queryByRole('region', { name: 'Clone #1 monitor' })).not.toBeInTheDocument();
  expect(clones.readLocalShadowClones).toHaveBeenLastCalledWith('/fixture', 'parent', undefined);
  vi.mocked(clones.readLocalShadowClones).mockResolvedValue(view([]));
  const last = screen.getByRole('button', { name: 'Dismiss Clone #2' });
  last.focus(); fireEvent.click(last);
  await waitFor(() => expect(screen.getByRole('region', { name: 'Fixture workbench' })).toHaveFocus());
  expect(screen.queryByRole('button', { name: /Kage Bunshin, / })).not.toBeInTheDocument();
});
