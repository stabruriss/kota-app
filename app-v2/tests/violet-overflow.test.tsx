import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { VioletMessageBubble } from '../src/chrome/VioletRoomPanel';
import * as pty from '../src/pty-client';
import type { VioletChatMessage } from '../src/pty-client';

const label = 'too long, see full response here.';
const projectRoot = '/tmp/fixture-overflow-project';
const path = 'project-memory/chathistory/overflow/violet-fixture.md';
const fullPath = `${projectRoot}/${path}`;
const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
const message: VioletChatMessage = {
  id: 'fixture-overflow', sessionId: 'fixture-session', agentId: 'agent-fixture',
  shell: 'codex', role: 'assistant', kind: 'message', timestamp: '2026-10-09T05:00:00Z',
  text: 'A long reply preview.', model: 'fixture-model', overflowPath: path,
};
function bubble(overrides: Partial<VioletChatMessage> = {}) {
  return <VioletMessageBubble message={{ ...message, ...overrides }} projectRoot={projectRoot} humanTargetName="Human Fixture" />;
}

beforeEach(() => {
  window.getSelection()?.removeAllRanges();
  vi.spyOn(pty, 'violetResolveFileRef').mockImplementation(async request => ({
    path: request.path.startsWith('/') ? request.path : `${projectRoot}/${request.path}`, isDir: false,
  }));
  vi.spyOn(pty, 'violetOpenFileRef').mockResolvedValue();
  vi.spyOn(pty, 'violetRevealFileRef').mockResolvedValue();
});
afterEach(() => {
  vi.restoreAllMocks();
  if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
  else Reflect.deleteProperty(navigator, 'clipboard');
});

describe('long response file footer', () => {
  it.each([undefined, null])('does not invent a file entry from body length when path is %s', overflowPath => {
    render(bubble({ text: 'x'.repeat(20_000), overflowPath }));
    expect(screen.queryByText(label)).not.toBeInTheDocument();
  });

  it('keeps the file outside an unclosed code fence and above the model caption', async () => {
    const { container } = render(bubble({ text: '```typescript\nconst unfinished = {' }));
    const link = await screen.findByRole('link', { name: label });
    expect(link.closest('pre')).toBeNull();
    const footer = link.closest('.violet-overflow-file')!;
    expect(footer.parentElement).toHaveClass('violet-msg-body');
    expect(footer.querySelector('svg')).toBeInTheDocument();
    const model = container.querySelector('.violet-msg-model')!;
    expect(footer.compareDocumentPosition(model) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    fireEvent.click(link);
    await waitFor(() => expect(pty.violetOpenFileRef).toHaveBeenCalledWith({ projectRoot, path }));
  });

  it('lets the text-file glyph open the same existing file action', async () => {
    const { container } = render(bubble());
    await screen.findByRole('link', { name: label });
    fireEvent.click(container.querySelector('.violet-file-icon')!);
    await waitFor(() => expect(pty.violetOpenFileRef).toHaveBeenCalledWith({ projectRoot, path }));
  });

  it.each([
    { role: 'user', kind: 'message' },
    { role: 'assistant', kind: 'commentary' },
  ])('does not add an endturn footer to $role/$kind', async override => {
    render(bubble(override));
    expect(screen.queryByText(label)).not.toBeInTheDocument();
  });

  it('reuses the three file actions and copies the resolved absolute path, including pointerdown', async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    render(bubble());
    const link = await screen.findByRole('link', { name: label });
    for (const action of ['Copy Full Path', 'Reveal in Finder', 'Open Default App']) {
      fireEvent.mouseDown(link, { button: 2 });
      fireEvent.contextMenu(link, { clientX: 200, clientY: 120 });
      const menu = screen.getByRole('menu');
      expect(within(menu).getAllByRole('menuitem').map(item => item.textContent)).toEqual([
        'Open Default App', 'Reveal in Finder', 'Copy Full Path',
      ]);
      const item = within(menu).getByRole('menuitem', { name: action });
      fireEvent.pointerDown(item);
      fireEvent.mouseDown(item);
      fireEvent.click(item);
      await waitFor(() => expect(screen.queryByRole('menu')).not.toBeInTheDocument());
    }
    expect(writeText).toHaveBeenCalledExactlyOnceWith(fullPath);
    expect(pty.violetRevealFileRef).toHaveBeenCalledExactlyOnceWith({ projectRoot: null, path: fullPath });
    expect(pty.violetOpenFileRef).toHaveBeenCalledExactlyOnceWith({ projectRoot: null, path: fullPath });
  });

  it('reports clipboard rejection rather than presenting it as successful copying', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('Clipboard denied'));
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    render(bubble());
    fireEvent.contextMenu(await screen.findByRole('link', { name: label }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'Copy Full Path' }));
    await waitFor(() => expect(alert).toHaveBeenCalledWith('copy failed: Error: Clipboard denied'));
  });
});
