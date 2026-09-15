import { useEffect, useRef, useState } from 'react';
import type { BbsPost } from '../pty-client';
import { avatarClassForAgentFallback, avatarClassForId, avatarImageStyleForId } from '../lib/hero-avatars';
import { bbsBuiltinAvatarClass, bbsSyncAvatarImages, parseBbsSyncAvatar, type BbsAvatarRef } from '../bbs-sync-avatars';

function RemoteImageAvatar({ sha256, ext, small, fallback }: BbsAvatarRef & { small: boolean; fallback: string }) {
  const frame = useRef<HTMLSpanElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    let shown = false;
    let stop: (() => void) | undefined;
    const visible = (isVisible: boolean) => {
      shown = isVisible;
      if (isVisible && !stop) {
        stop = bbsSyncAvatarImages.subscribe({ sha256, ext }, (image) => { if (alive && shown) setSrc(image); });
      } else if (!isVisible) {
        stop?.(); stop = undefined; setSrc(null);
      }
    };
    const element = frame.current;
    const observer = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver((entries) => {
      if (alive) for (const entry of entries) if (entry.target === element) visible(entry.isIntersecting);
    });
    if (observer && element) observer.observe(element);
    else visible(true); // Older test/browser hosts still use the same bounded queue.
    return () => { alive = false; observer?.disconnect(); stop?.(); };
  }, [sha256, ext]);
  return <span ref={frame} className={`bbs-msg-avatar ${small ? 'sm' : ''} tavern-avatar-art ${src ? 'bbs-sync-avatar-image' : fallback}`} aria-hidden>
    {src ? <img src={src} alt="" decoding="async" onError={() => {
      bbsSyncAvatarImages.forget({ sha256, ext }); setSrc(null);
    }} /> : <><span /><i /><b /></>}
  </span>;
}

export function BbsPostAvatar({ post, meta, small = false }: {
  post: Pick<BbsPost, 'agentId' | 'agentAvatar' | 'syncAvatar'>;
  meta?: { avatarId?: string | null; avatarClass?: string };
  small?: boolean;
}) {
  const origin = parseBbsSyncAvatar(post.syncAvatar);
  if (origin !== undefined) {
    const fallback = post.agentId === 'human' ? 'system-human' : 'provider-codex';
    if (origin.kind === 'image' && origin.available) {
      return <RemoteImageAvatar key={`${origin.sha256}.${origin.ext}`} sha256={origin.sha256} ext={origin.ext} small={small} fallback={fallback} />;
    }
    const className = origin.kind === 'builtin' ? bbsBuiltinAvatarClass(origin.id) ?? fallback : fallback;
    return <span className={`bbs-msg-avatar ${small ? 'sm' : ''} tavern-avatar-art ${className}`} aria-hidden><span /><i /><b /></span>;
  }
  // Keep the existing local-post path byte-for-byte in meaning. No shared hero
  // cache is consulted for any received sidecar (including none/invalid).
  const avatarId = post.agentAvatar ?? meta?.avatarId ?? (post.agentId === 'human' ? 'user-default' : null);
  const className = avatarId ? avatarClassForId(avatarId, null) : meta?.avatarClass ?? avatarClassForAgentFallback(null, post.agentId);
  return <span className={`bbs-msg-avatar ${small ? 'sm' : ''} tavern-avatar-art ${className}`} style={avatarImageStyleForId(avatarId)} aria-hidden>
    <span /><i /><b />
  </span>;
}
