import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WhiteboardPanel } from '../src/popups/WhiteboardPanel';
import type { WhiteboardCanvasLoadResult } from '../src/pty-client';

vi.mock('@excalidraw/excalidraw', () => {
  const MainMenu = Object.assign(({ children }: any) => children, {
    DefaultItems: Object.fromEntries(['ChangeCanvasBackground', 'LoadScene', 'SaveToActiveFile', 'Export', 'SaveAsImage', 'SearchMenu', 'Help', 'ToggleTheme', 'ClearCanvas'].map(name => [name, () => null])),
    Separator: () => null,
  });
  return {
    CaptureUpdateAction: { NEVER: 'NEVER' }, MainMenu,
    restore: (raw: unknown) => raw,
    serializeAsJSON: (elements: unknown[], appState: unknown, files: unknown) => JSON.stringify({ elements, appState, files }),
    exportToBlob: vi.fn(),
    Excalidraw: ({ initialData, onChange }: any) => <div>
      <div data-testid="drawing">{JSON.stringify(initialData.elements)}</div>
      <button onClick={() => onChange([{ type: 'text', text: 'unsaved drawing' }], {}, {})}>Edit drawing</button>
    </div>,
  };
});

const root = '/test-project';
const canvasDir = `${root}/project-memory/canvas`;
const path = (index: number) => `${canvasDir}/pages/page-${String(index).padStart(3, '0')}.excalidraw`;
const blank = JSON.stringify({ elements: [], appState: {}, files: {} });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => { resolve = done; });
  return { promise, resolve };
}
function board(count = 2) {
  let pages = Array.from({ length: count }, (_, i) => ({ id: `page-00${i + 1}`, title: `Page ${i + 1}`, path: path(i + 1), modifiedMs: 1, dataJson: blank }));
  let active = pages[pages.length - 1].path;
  const snapshot = (requested = active): WhiteboardCanvasLoadResult => {
    const selected = pages.find(page => page.path === requested) ?? pages[0];
    return { canvasDir, manifestPath: `${canvasDir}/manifest.json`, pagePath: selected.path, pageId: selected.id,
      dataJson: selected.dataJson, modifiedMs: selected.modifiedMs, pages: pages.map(({ dataJson, ...page }) => page) };
  };
  const props = {
    projectRoot: root, onClose: vi.fn(), onInsertDrawing: vi.fn(),
    onLoadCanvas: vi.fn(async (_root?: string | null, requested?: string | null) => snapshot(requested || active)),
    onSaveCanvas: vi.fn(async (request: { pagePath: string; dataJson: string }) => {
      const page = pages.find(page => page.path === request.pagePath)!;
      page.dataJson = request.dataJson; page.modifiedMs++;
      return snapshot(page.path);
    }),
    onRenamePage: vi.fn(async () => snapshot()),
    onSaveSnapshot: vi.fn(),
    onDeletePage: vi.fn(async (request: { pagePath: string; activePagePath?: string | null }) => {
      pages = pages.filter(page => page.path !== request.pagePath);
      if (!pages.length) pages = [{ id: 'page-002', title: 'Page 2', path: path(2), modifiedMs: 2, dataJson: blank }];
      active = pages.find(page => page.path === request.activePagePath)?.path ?? pages[0].path;
      return snapshot();
    }),
  };
  return { props, snapshot };
}
async function openDelete(name = 'Page 2') {
  await act(async () => {
    if (!screen.queryByRole('menu', { name: 'Whiteboard pages' })) fireEvent.click(screen.getByTitle('Open page menu'));
  });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: `Delete ${name}` })); });
}
async function confirm() {
  await act(async () => { fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: 'Delete page' })); });
}
beforeEach(() => { vi.useFakeTimers(); localStorage.clear(); });
afterEach(() => { vi.useRealTimers(); });

describe('whiteboard page deletion', () => {
  it('places Delete after Rename; Cancel, Escape and backdrop do not delete or close the board', async () => {
    const { props } = board();
    await act(async () => { render(<WhiteboardPanel {...props} />); });
    await openDelete();
    expect(screen.getByRole('button', { name: 'Cancel' })).toHaveFocus();
    expect(screen.getByText(/“Page 2” and everything/)).toHaveTextContent('Images already inserted in chat will be kept.');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Cancel' })); });
    const row = screen.getByRole('button', { name: 'Rename Page 2' }).parentElement!;
    expect(within(row).getAllByRole('button').map(button => button.textContent)).toEqual(['Page 2', 'Rename', 'Delete']);
    await openDelete();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    await openDelete();
    fireEvent.mouseDown(screen.getByRole('alertdialog').parentElement!);
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
    expect(props.onDeletePage).not.toHaveBeenCalled();
    expect(props.onClose).not.toHaveBeenCalled();
  });

  it('deletes the current page only after confirmation and discards its queued autosave', async () => {
    const { props } = board();
    await act(async () => { render(<WhiteboardPanel {...props} />); });
    fireEvent.click(screen.getByRole('button', { name: 'Edit drawing' }));
    await openDelete();
    expect(props.onDeletePage).not.toHaveBeenCalled();
    await confirm();
    expect(props.onDeletePage).toHaveBeenCalledExactlyOnceWith({ projectRoot: root, pagePath: path(2), activePagePath: path(2) });
    expect(screen.getByTitle('Page 1')).toHaveTextContent('Page 1');
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(props.onSaveCanvas).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTitle('Open page menu'));
    expect(screen.queryByRole('button', { name: 'Delete Page 2' })).not.toBeInTheDocument();
    expect(props.onSaveSnapshot).not.toHaveBeenCalled();
  });

  it('preserves unsaved edits and selection when deleting a different page', async () => {
    const { props } = board();
    await act(async () => { render(<WhiteboardPanel {...props} />); });
    fireEvent.click(screen.getByRole('button', { name: 'Edit drawing' }));
    await openDelete('Page 1'); await confirm();
    expect(props.onSaveCanvas).toHaveBeenCalledWith(expect.objectContaining({ pagePath: path(2), dataJson: expect.stringContaining('unsaved drawing') }));
    expect(props.onSaveCanvas.mock.invocationCallOrder[0]).toBeLessThan(props.onDeletePage.mock.invocationCallOrder[0]);
    expect(screen.getByTitle('Page 2')).toBeVisible();
  });

  it('waits for an in-flight save and poll; their late results cannot resurrect the deleted page', async () => {
    const { props, snapshot } = board();
    const old = snapshot(), save = deferred<WhiteboardCanvasLoadResult>(), poll = deferred<WhiteboardCanvasLoadResult>();
    props.onSaveCanvas.mockImplementationOnce(() => save.promise);
    await act(async () => { render(<WhiteboardPanel {...props} />); });
    props.onLoadCanvas.mockImplementationOnce(() => poll.promise);
    fireEvent.click(screen.getByRole('button', { name: 'Edit drawing' }));
    await act(async () => { await vi.advanceTimersByTimeAsync(1600); });
    await openDelete(); await confirm();
    expect(props.onDeletePage).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Deleting…' })).toBeDisabled();
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.getByRole('alertdialog')).toBeVisible();
    await act(async () => { save.resolve(old); });
    expect(props.onDeletePage).not.toHaveBeenCalled();
    await act(async () => { poll.resolve({ ...old, modifiedMs: 9 }); });
    expect(props.onDeletePage).toHaveBeenCalledTimes(1);
    expect(screen.getByTitle('Page 1')).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(1600); });
    expect(screen.getByTitle('Page 1')).toBeVisible();
    expect(props.onSaveCanvas).toHaveBeenCalledTimes(1);
  });

  it('opens the backend-created blank page after deleting the last page', async () => {
    const { props } = board(1);
    await act(async () => { render(<WhiteboardPanel {...props} />); });
    fireEvent.click(screen.getByRole('button', { name: 'Edit drawing' }));
    await openDelete('Page 1'); await confirm();
    expect(screen.getByTitle('Page 2')).toBeVisible();
    expect(screen.getByTestId('drawing')).toHaveTextContent('[]');
    expect(localStorage.getItem(`kota:whiteboard:last-page:${root}`)).toBe(path(2));
  });

  it('leaves a failed deletion visible, permits retry, and does not remove the current page', async () => {
    const { props } = board();
    props.onDeletePage.mockRejectedValueOnce(new Error('Permission denied'));
    await act(async () => { render(<WhiteboardPanel {...props} />); });
    await openDelete(); await confirm();
    expect(screen.getByRole('alert')).toHaveTextContent('Permission denied');
    expect(screen.getByTitle('Page 2')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Delete page' })).toBeEnabled();
    await confirm();
    expect(screen.getByTitle('Page 1')).toBeVisible();
    expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument();
  });
});
