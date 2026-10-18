import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import { MarkdownText, VioletMessageBubble } from '../src/chrome/VioletRoomPanel';
import * as pty from '../src/pty-client';

const fullPath = '/fixture/project/中文 space/"quoted" $file.md';
const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
const writeText = vi.fn();

beforeEach(() => {
  window.getSelection()?.removeAllRanges();
  vi.mocked(isTauri).mockReturnValue(true);
  vi.mocked(invoke).mockReset().mockResolvedValue(true);
  writeText.mockReset().mockRejectedValue(new DOMException('Clipboard denied', 'NotAllowedError'));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  vi.spyOn(console, 'warn').mockImplementation(() => {});
});
afterEach(() => {
  vi.restoreAllMocks();
  if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
  else Reflect.deleteProperty(navigator, 'clipboard');
});

describe('bubble file path copying', () => {
  it.each(['inline', 'overflow'])('uses the native writer when WebKit rejects a %s file menu click', async kind => {
    vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path: fullPath, isDir: false });
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    render(kind === 'inline'
      ? <MarkdownText text="Review `src/source.md:42`." projectRoot="/fixture/project" enableLocalFileRefs />
      : <VioletMessageBubble projectRoot="/fixture/project" humanTargetName="Fixture human" message={{
        id: 'copy-fixture', agentId: 'agent-fixture', sessionId: 'fixture-session', shell: 'codex',
        role: 'assistant', kind: 'message', timestamp: '2026-10-09T20:00:00Z', text: 'Preview.', overflowPath: fullPath,
      }} />);
    const link = await screen.findByRole('link', { name: kind === 'inline' ? 'src/source.md:42' : 'too long, see full response here.' });
    fireEvent.mouseDown(link, { button: 2 });
    fireEvent.contextMenu(link, { clientX: 80, clientY: 80 });
    const item = screen.getByRole('menuitem', { name: 'Copy Full Path' });
    fireEvent.pointerDown(item);
    fireEvent.mouseDown(item);
    fireEvent.click(item);
    await waitFor(() => expect(invoke).toHaveBeenCalledExactlyOnceWith('violet_copy_file_path', { path: fullPath }));
    expect(writeText).toHaveBeenCalledExactlyOnceWith(fullPath);
    expect(alert).not.toHaveBeenCalled();
    expect(console.warn).toHaveBeenCalledWith(expect.stringContaining('native path copy'), {
      error: expect.objectContaining({ name: 'NotAllowedError' }), userActivation: navigator.userActivation?.isActive,
    });
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  });

  it('keeps the successful browser clipboard path without a second write', async () => {
    writeText.mockResolvedValue(undefined);
    await pty.violetCopyFilePath(fullPath);
    expect(writeText).toHaveBeenCalledExactlyOnceWith(fullPath);
    expect(invoke).not.toHaveBeenCalled();
  });

  it('uses native copying if the web clipboard API is absent', async () => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
    await pty.violetCopyFilePath(fullPath);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('violet_copy_file_path', { path: fullPath });
  });

  it('preserves rejection in a browser without invoking native commands', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(pty.violetCopyFilePath(fullPath)).rejects.toThrow('Clipboard denied');
    expect(invoke).not.toHaveBeenCalled();
  });

  it('does not report unsupported native platforms as successful copies', async () => {
    vi.mocked(invoke).mockResolvedValue(false);
    await expect(pty.violetCopyFilePath(fullPath)).rejects.toThrow('Clipboard denied');
  });

  it('propagates a failed native write', async () => {
    vi.mocked(invoke).mockRejectedValue(new Error('Clipboard write failed'));
    await expect(pty.violetCopyFilePath(fullPath)).rejects.toThrow('Clipboard write failed');
  });

  it('does not hide unrelated web clipboard failures behind a native retry', async () => {
    writeText.mockRejectedValue(new Error('Unexpected clipboard failure'));
    await expect(pty.violetCopyFilePath(fullPath)).rejects.toThrow('Unexpected clipboard failure');
    expect(invoke).not.toHaveBeenCalled();
  });
});
