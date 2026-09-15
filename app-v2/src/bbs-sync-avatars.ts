import { invoke, isTauri } from '@tauri-apps/api/core';
import { HERO_AVATARS } from './lib/hero-avatars';
import type { BbsSyncAvatar } from './pty-client';
import { bbsAvatarDataUrl, bbsAvatarExt, bbsAvatarHash, createBbsImageQueue } from './bbs-avatar-images';

export interface BbsAvatarRef { sha256: string; ext: string }
const builtins = new Map(HERO_AVATARS.map(({ id, className }) => [id as string, className]));
const validRef = (value: BbsAvatarRef) => bbsAvatarHash(value.sha256) && bbsAvatarExt(value.ext);
const keyFor = ({ sha256, ext }: BbsAvatarRef) => `${sha256}.${ext}`;

/** A present but unsupported origin snapshot must NEVER resolve a coincident
 * local agent/user-avatar ID. Unknown data gets the ordinary default instead. */
export function parseBbsSyncAvatar(value: unknown): BbsSyncAvatar | undefined {
  if (value === undefined) return undefined;
  if (!value || typeof value !== 'object' || Array.isArray(value)) return { kind: 'none' };
  const item = value as Record<string, unknown>;
  if (item.kind === 'builtin' && typeof item.id === 'string' && builtins.has(item.id)) return { kind: 'builtin', id: item.id };
  if (item.kind === 'image' && typeof item.sha256 === 'string' && typeof item.ext === 'string'
    && validRef({ sha256: item.sha256, ext: item.ext }) && typeof item.available === 'boolean'
    && (item.localPath === null || typeof item.localPath === 'string')) {
    return { kind: 'image', sha256: item.sha256, ext: item.ext, localPath: item.localPath, available: item.available };
  }
  return { kind: 'none' };
}

export const bbsBuiltinAvatarClass = (id: string) => builtins.get(id);

/** BBS-only, async verified-resource reader. Paths (even the projected localPath)
 * never cross this IPC boundary. The backend verifies SHA and the byte limit. */
export async function bbsSyncAvatarRead({ sha256, ext }: BbsAvatarRef): Promise<string> {
  const unavailable = () => new Error('BBS avatar is unavailable.');
  if (!validRef({ sha256, ext }) || !isTauri()) throw unavailable();
  let value: unknown;
  try { value = await invoke('bbs_sync_avatar_read', { request: { sha256, ext } }); }
  catch { throw unavailable(); }
  const src = bbsAvatarDataUrl(value, ext);
  if (src === null) throw unavailable();
  return src;
}

/** Tiny BBS image queue: two reads, at most 32 distinct outstanding resources,
 * 16 successful data URLs in LRU memory. No disk cache, timers or retry loop.
 * Offscreen/unmounted views unsubscribe; queued work with no readers is removed.
 * Already-started bounded IPC reads may finish, without updating gone views. */
export function createBbsAvatarImages(read: (ref: BbsAvatarRef) => Promise<string> = bbsSyncAvatarRead) {
  return createBbsImageQueue({ read, valid: validRef, key: keyFor, snapshot: ({ sha256, ext }) => ({ sha256, ext }) });
}

// Importing/constructing this does not invoke, register listeners or write storage.
export const bbsSyncAvatarImages = createBbsAvatarImages();
