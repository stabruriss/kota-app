import { useEffect, useRef, useState } from 'react';
import { bbsBuiltinAvatarClass } from '../bbs-sync-avatars';
import { bbsRosterAvatarImages, bbsRosterAvatarKey, type BbsRosterAvatarRef } from '../bbs-roster-avatars';
import type { BbsRosterAvatar as Avatar } from '../types/bbs-roster';

function ImageAvatar({ resource, letter }: { resource: BbsRosterAvatarRef; letter: string }) {
  const frame = useRef<HTMLSpanElement>(null);
  const [src, setSrc] = useState<string | null>(null);
  const { deviceId, sha256, ext, sizeBytes } = resource;
  useEffect(() => {
    let alive = true, shown = false;
    let stop: (() => void) | undefined;
    const visible = (isVisible: boolean) => {
      shown = isVisible;
      if (isVisible && !stop) {
        stop = bbsRosterAvatarImages.subscribe({ deviceId, sha256, ext, sizeBytes }, image => { if (alive && shown) setSrc(image); });
      } else if (!isVisible) { stop?.(); stop = undefined; setSrc(null); }
    };
    const element = frame.current;
    const observer = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver(entries => {
      if (alive) for (const entry of entries) if (entry.target === element) visible(entry.isIntersecting);
    });
    if (observer && element) observer.observe(element); else visible(true);
    return () => { alive = false; observer?.disconnect(); stop?.(); };
  }, [deviceId, sha256, ext, sizeBytes]);
  return <span ref={frame} className="bbs-mentions-avatar bbs-mentions-letter" aria-hidden="true">
    {src ? <img src={src} alt="" decoding="async" onError={() => { bbsRosterAvatarImages.forget(resource); setSrc(null); }} /> : letter}
  </span>;
}

/** No local hero/name lookup. Missing images, none and unknown builtins use the
 * displayed name's first character until the roster reports verified bytes. */
export function BbsRosterAvatar({ deviceId, name, avatar }: { deviceId: string; name: string; avatar: Avatar }) {
  const letter = Array.from(name)[0] || '?';
  if (avatar.kind === 'image' && avatar.available) {
    const resource = { deviceId, sha256: avatar.sha256, ext: avatar.ext, sizeBytes: avatar.sizeBytes };
    return <ImageAvatar key={bbsRosterAvatarKey(resource)} resource={resource} letter={letter} />;
  }
  const builtin = avatar.kind === 'builtin' && bbsBuiltinAvatarClass(avatar.id);
  return builtin ? <span className={`bbs-mentions-avatar tavern-avatar-art ${builtin}`} aria-hidden="true"><span /><i /><b /></span>
    : <span className="bbs-mentions-avatar bbs-mentions-letter" aria-hidden="true">{letter}</span>;
}
