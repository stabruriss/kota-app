import { useEffect, useMemo, useState } from 'react';
import { fileImageDataUrl, type BbsAttachment } from '../pty-client';

const imageCache = new Map<string, string>();
// Existing BBS body recognizer: do not broaden this into attachment collection.
const BBS_IMAGE_PATH_RE = /(?:^|[\s("'`])((?:~\/|\/)?[^\s)"'`]*(?:attachments|images?|screenshots?)[^\s)"'`]*\.(?:png|jpe?g|gif|webp)|(?:~\/|\/)[^\s)"'`]+\.(?:png|jpe?g|gif|webp))/gi;
const isImage = (path: string) => /\.(png|jpe?g|gif|webp)$/i.test(path);

function bodyImages(body: string, baseRoot: string | null): { path: string; available: boolean }[] {
  const paths: string[] = [];
  for (const match of body.matchAll(BBS_IMAGE_PATH_RE)) {
    const raw = (match[1] ?? '').trim();
    if (!raw || paths.includes(raw)) continue;
    paths.push(raw);
    if (paths.length === 6) break;
  }
  return paths.map((path) => path.startsWith('/') ? { path, available: true }
    : path.startsWith('~/') || !baseRoot ? { path, available: false }
      : { path: `${baseRoot.replace(/\/$/, '')}/${path}`, available: true });
}

export function isBbsAttachment(attachment: BbsAttachment, postId: string): boolean {
  if (typeof attachment.id !== 'string' || !/^[A-Za-z0-9_-]+$/.test(attachment.id)
    || !/^[A-Za-z0-9_.-]+$/.test(postId) || postId === '.' || postId === '..') return false;
  const prefix = `attachments/${postId}/${attachment.id}`;
  return typeof attachment.path === 'string'
    && (attachment.path === prefix || (attachment.path.startsWith(`${prefix}.`)
      && /^[A-Za-z0-9]{1,16}$/.test(attachment.path.slice(prefix.length + 1))))
    && typeof attachment.localPath === 'string' && attachment.localPath.startsWith('/')
    && typeof attachment.available === 'boolean';
}

function BbsInlineImage({ path, name, available, cacheKey }: { path: string; name: string; available: boolean; cacheKey: string }) {
  const [state, setState] = useState<{ status: 'loading' | 'ready' | 'error'; src?: string }>(() => (
    !available ? { status: 'error' } : imageCache.has(cacheKey)
      ? { status: 'ready', src: imageCache.get(cacheKey) } : { status: 'loading' }
  ));
  useEffect(() => {
    if (!available) { setState({ status: 'error' }); return; }
    const cached = imageCache.get(cacheKey);
    if (cached) { setState({ status: 'ready', src: cached }); return; }
    let cancelled = false;
    setState({ status: 'loading' });
    void fileImageDataUrl(path).then((src) => {
      if (!cancelled) {
        imageCache.set(cacheKey, src);
        setState({ status: 'ready', src });
      }
    }).catch(() => { if (!cancelled) setState({ status: 'error' }); });
    return () => { cancelled = true; };
  }, [available, cacheKey, path]);
  if (state.status === 'loading') return null;
  if (state.status === 'error') return <span className="bbs-image-unavailable" title={path}>Image unavailable</span>;
  return <img className="bbs-post-image" src={state.src} alt={name} onError={() => setState({ status: 'error' })} />;
}

export function BbsAttachments({ body, baseRoot, postId, attachments = [] }: {
  body: string; baseRoot: string | null; postId: string; attachments?: readonly BbsAttachment[];
}) {
  const entries = useMemo(() => attachments.filter((item) => isBbsAttachment(item, postId)).slice(0, 9), [attachments, postId]);
  const images = useMemo(() => {
    const declared = entries.filter((item) => isImage(item.path)).map((item) => ({
      path: item.localPath, name: item.name, available: item.available,
      // A promoted Fork can reuse the resident path with different bytes.
      cacheKey: JSON.stringify([item.localPath, item.sha256]),
    }));
    const seen = new Set(declared.map((item) => item.path));
    for (const { path, available } of bodyImages(body, baseRoot)) {
      if (seen.has(path)) continue;
      seen.add(path);
      declared.push({ path, name: path.split('/').pop() ?? 'attachment', available, cacheKey: path });
    }
    return declared.slice(0, 6);
  }, [baseRoot, body, entries]);
  // Keep the existing six-image preview budget, but do not hide the remaining registered files.
  const previewed = new Set(images.map((item) => item.path));
  const files = entries.filter((item) => !isImage(item.path) || !previewed.has(item.localPath));
  return <>
    {images.length > 0 && <div className="bbs-post-images">
      {images.map((item) => <BbsInlineImage key={item.cacheKey} {...item} />)}
    </div>}
    {files.length > 0 && <ul className="bbs-post-files" aria-label="Attachments">
      {files.map((item) => <li key={item.id}>
        <span>{item.name}{!item.available && <small> · File unavailable</small>}</span>
        <code>{item.localPath}</code>
      </li>)}
    </ul>}
  </>;
}
