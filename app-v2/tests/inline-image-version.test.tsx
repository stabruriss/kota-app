import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import { MarkdownText } from '../src/chrome/VioletRoomPanel';
import * as pty from '../src/pty-client';

const oldImage = 'data:image/png;base64,b2xk';
const newImage = 'data:image/png;base64,bmV3';
const projectRoot = '/fixture/image-version-project';
const preview = (path: string, message: string) => (
  <MarkdownText key={message} text={path} projectRoot={projectRoot} enableLocalFileRefs />
);

beforeEach(() => {
  vi.stubGlobal('IntersectionObserver', undefined);
});
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('local image preview versions', () => {
  it('reuses an unchanged file across new messages without reading the image again', async () => {
    const path = `${projectRoot}/unchanged.png`;
    const resolve = vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path, isDir: false, version: 'v1' });
    const read = vi.spyOn(pty, 'fileImageDataUrl').mockResolvedValue(oldImage);
    const view = render(preview(path, 'first'));
    expect(await screen.findByRole('button', { name: 'unchanged.png' })).toHaveAttribute('src', oldImage);
    view.rerender(preview(path, 'second'));
    expect(await screen.findByRole('button', { name: 'unchanged.png' })).toHaveAttribute('src', oldImage);
    expect(resolve).toHaveBeenCalledTimes(2);
    expect(read).toHaveBeenCalledExactlyOnceWith(path);
  });

  it('loads changed bytes for a new message using the same absolute path', async () => {
    const path = `${projectRoot}/changed.png`;
    const resolve = vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path, isDir: false, version: 'v1' });
    const read = vi.spyOn(pty, 'fileImageDataUrl').mockResolvedValueOnce(oldImage).mockResolvedValue(newImage);
    const view = render(preview(path, 'first'));
    expect(await screen.findByRole('button', { name: 'changed.png' })).toHaveAttribute('src', oldImage);
    resolve.mockResolvedValue({ path, isDir: false, version: 'v2' });
    view.rerender(preview(path, 'second'));
    await waitFor(() => expect(screen.getByRole('button', { name: 'changed.png' })).toHaveAttribute('src', newImage));
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('keeps paths distinct even when their filename and metadata version match', async () => {
    const firstPath = `${projectRoot}/a/same.png`;
    const secondPath = `${projectRoot}/b/same.png`;
    vi.spyOn(pty, 'violetResolveFileRef').mockImplementation(async ({ path }) => ({ path, isDir: false, version: 'v1' }));
    const read = vi.spyOn(pty, 'fileImageDataUrl').mockResolvedValueOnce(oldImage).mockResolvedValue(newImage);
    const view = render(preview(firstPath, 'first'));
    expect(await screen.findByRole('button', { name: 'same.png' })).toHaveAttribute('src', oldImage);
    view.rerender(preview(secondPath, 'second'));
    expect(await screen.findByRole('button', { name: 'same.png' })).toHaveAttribute('src', newImage);
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('discards an old unmounted request after a newer version has finished', async () => {
    const path = `${projectRoot}/late.png`;
    const resolve = vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path, isDir: false, version: 'v1' });
    let finishOld!: (data: string) => void;
    const read = vi.spyOn(pty, 'fileImageDataUrl')
      .mockReturnValueOnce(new Promise<string>(done => { finishOld = done; }))
      .mockResolvedValue(newImage);
    const view = render(preview(path, 'first'));
    await waitFor(() => expect(read).toHaveBeenCalledOnce());
    resolve.mockResolvedValue({ path, isDir: false, version: 'v2' });
    view.rerender(preview(path, 'second'));
    expect(await screen.findByRole('button', { name: 'late.png' })).toHaveAttribute('src', newImage);
    await act(async () => finishOld(oldImage));
    expect(screen.getByRole('button', { name: 'late.png' })).toHaveAttribute('src', newImage);
    view.rerender(preview(path, 'third'));
    expect(await screen.findByRole('button', { name: 'late.png' })).toHaveAttribute('src', newImage);
    expect(read).toHaveBeenCalledTimes(2);
    // Even the abandoned version must not have been populated by its late reply.
    resolve.mockResolvedValue({ path, isDir: false, version: 'v1' });
    view.rerender(preview(path, 'abandoned-version'));
    expect(await screen.findByRole('button', { name: 'late.png' })).toHaveAttribute('src', newImage);
    expect(read).toHaveBeenCalledTimes(3);
  });

  it('isolates versions when old and new messages remain mounted together', async () => {
    const path = `${projectRoot}/simultaneous.png`;
    const resolve = vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path, isDir: false, version: 'v1' });
    let finishOld!: (data: string) => void;
    const read = vi.spyOn(pty, 'fileImageDataUrl')
      .mockReturnValueOnce(new Promise<string>(done => { finishOld = done; }))
      .mockResolvedValue(newImage);
    const view = render(<>{preview(path, 'first')}</>);
    await waitFor(() => expect(read).toHaveBeenCalledOnce());
    resolve.mockResolvedValue({ path, isDir: false, version: 'v2' });
    view.rerender(<>{preview(path, 'first')}{preview(path, 'second')}</>);
    expect(await screen.findByRole('button', { name: 'simultaneous.png' })).toHaveAttribute('src', newImage);
    await act(async () => finishOld(oldImage));
    view.rerender(<>{preview(path, 'first')}{preview(path, 'second')}{preview(path, 'third')}</>);
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'simultaneous.png' })).toHaveLength(3));
    expect(screen.getAllByRole('button', { name: 'simultaneous.png' }).map(img => img.getAttribute('src')))
      .toEqual([oldImage, newImage, newImage]);
    expect(read).toHaveBeenCalledTimes(2);
  });

  it.each([undefined, null])('does not share a path-only cache when the version is %s', async version => {
    const path = `${projectRoot}/unknown-${String(version)}.png`;
    vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path, isDir: false, version });
    const read = vi.spyOn(pty, 'fileImageDataUrl').mockResolvedValueOnce(oldImage).mockResolvedValue(newImage);
    const view = render(preview(path, 'first'));
    expect(await screen.findByRole('button', { name: `unknown-${String(version)}.png` })).toHaveAttribute('src', oldImage);
    view.rerender(preview(path, 'second'));
    expect(await screen.findByRole('button', { name: `unknown-${String(version)}.png` })).toHaveAttribute('src', newImage);
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('still waits for visibility when an older version is already cached', async () => {
    const path = `${projectRoot}/lazy-version.png`;
    const resolve = vi.spyOn(pty, 'violetResolveFileRef').mockResolvedValue({ path, isDir: false, version: 'v1' });
    const read = vi.spyOn(pty, 'fileImageDataUrl').mockResolvedValueOnce(oldImage).mockResolvedValue(newImage);
    const view = render(preview(path, 'first'));
    await screen.findByRole('button', { name: 'lazy-version.png' });
    let intersect!: () => void;
    const observe = vi.fn();
    vi.stubGlobal('IntersectionObserver', class {
      constructor(callback: IntersectionObserverCallback, options: IntersectionObserverInit) {
        expect(options.rootMargin).toBe('480px 0px');
        intersect = () => callback([{ isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
      }
      observe = observe;
      disconnect() {}
    });
    resolve.mockResolvedValue({ path, isDir: false, version: 'v2' });
    view.rerender(preview(path, 'second'));
    await waitFor(() => expect(observe).toHaveBeenCalledOnce());
    expect(screen.queryByRole('button', { name: 'lazy-version.png' })).toBeNull();
    expect(read).toHaveBeenCalledOnce();
    act(() => intersect());
    expect(await screen.findByRole('button', { name: 'lazy-version.png' })).toHaveAttribute('src', newImage);
    expect(read).toHaveBeenCalledTimes(2);
  });

  it('retains the 50-entry cache bound across file versions', async () => {
    const path = `${projectRoot}/bounded.png`;
    const resolve = vi.spyOn(pty, 'violetResolveFileRef');
    const read = vi.spyOn(pty, 'fileImageDataUrl').mockResolvedValue(newImage);
    const view = render(<div />);
    for (let index = 0; index < 51; index += 1) {
      resolve.mockResolvedValue({ path, isDir: false, version: `version-${index}` });
      view.rerender(preview(path, `message-${index}`));
      await screen.findByRole('button', { name: 'bounded.png' });
    }
    expect(read).toHaveBeenCalledTimes(51);
    resolve.mockResolvedValue({ path, isDir: false, version: 'version-0' });
    view.rerender(preview(path, 'oldest'));
    await screen.findByRole('button', { name: 'bounded.png' });
    expect(read).toHaveBeenCalledTimes(52);
  });
});
