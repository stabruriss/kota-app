import { invoke, isTauri } from '@tauri-apps/api/core';
import { BBS_AVATAR_MAX_BYTES, bbsAvatarDataUrl, bbsAvatarExt, bbsAvatarHash, createBbsImageQueue } from './bbs-avatar-images';
import { bbsRosterSafeId } from './bbs-roster-client';

export interface BbsRosterAvatarRef { deviceId: string; sha256: string; ext: string; sizeBytes: number }
const valid = (ref: BbsRosterAvatarRef) => bbsRosterSafeId(ref.deviceId) && bbsAvatarHash(ref.sha256) && bbsAvatarExt(ref.ext)
  && Number.isSafeInteger(ref.sizeBytes) && ref.sizeBytes > 0 && ref.sizeBytes <= BBS_AVATAR_MAX_BYTES;
export const bbsRosterAvatarKey = ({ deviceId, sha256, ext, sizeBytes }: BbsRosterAvatarRef) => JSON.stringify([deviceId, sha256, ext, sizeBytes]);

/** Authorization belongs to exactly this device's current roster, not a post,
 * another device's same hash, or any UI-supplied path/extension. */
export async function bbsRosterAvatarRead(ref: BbsRosterAvatarRef): Promise<string> {
  const unavailable = () => new Error('Roster avatar is unavailable.');
  if (!valid(ref) || !isTauri()) throw unavailable();
  let value: unknown;
  try { value = await invoke('bbs_roster_avatar_read', { request: { deviceId: ref.deviceId, sha256: ref.sha256 } }); }
  catch { throw unavailable(); }
  const src = bbsAvatarDataUrl(value, ref.ext, ref.sizeBytes);
  if (src === null) throw unavailable();
  return src;
}
export function createBbsRosterAvatarImages(read: (ref: BbsRosterAvatarRef) => Promise<string> = bbsRosterAvatarRead) {
  return createBbsImageQueue({ read, valid, key: bbsRosterAvatarKey,
    snapshot: ({ deviceId, sha256, ext, sizeBytes }) => ({ deviceId, sha256, ext, sizeBytes }) });
}
export const bbsRosterAvatarImages = createBbsRosterAvatarImages();
