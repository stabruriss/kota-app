import { invoke, isTauri } from '@tauri-apps/api/core';
import type { AgentCli } from '../types/agent-pty';

export type HeroProviderId = 'claude' | 'codex' | 'antigravity' | 'opencode' | 'pi';
export type GeneratedHeroAvatarId =
  | 'claude'
  | 'claude-lantern'
  | 'claude-quill'
  | 'codex'
  | 'codex-slate'
  | 'codex-prism'
  | 'antigravity'
  | 'opencode'
  | 'pi'
  | 'magi'
  | 'violet'
  | 'ember'
  | 'laughing-man'
  | 'puppeteer'
  | 'bartender'
  | 'user-default';

export type HeroAvatarId = string;

export interface HeroAvatarSpec {
  id: GeneratedHeroAvatarId;
  provider?: HeroProviderId;
  label: string;
  className: string;
}

export interface UserHeroAvatar {
  id: string;
  label: string;
  dataUrl: string;
  createdAt: string;
  path?: string | null;
  mime?: string | null;
  sizeBytes?: number;
}

export const HERO_AVATARS: readonly HeroAvatarSpec[] = [
  { id: 'claude', provider: 'claude', label: 'Compass', className: 'provider-claude' },
  { id: 'claude-lantern', provider: 'claude', label: 'Lantern', className: 'provider-claude-lantern' },
  { id: 'claude-quill', provider: 'claude', label: 'Quill', className: 'provider-claude-quill' },
  { id: 'codex', provider: 'codex', label: 'Slate', className: 'provider-codex' },
  { id: 'codex-slate', provider: 'codex', label: 'Glass', className: 'provider-codex-slate' },
  { id: 'codex-prism', provider: 'codex', label: 'Prism', className: 'provider-codex-prism' },
  { id: 'antigravity', provider: 'antigravity', label: 'Antigravity', className: 'provider-antigravity' },
  { id: 'opencode', provider: 'opencode', label: 'OpenCode', className: 'provider-opencode' },
  { id: 'pi', provider: 'pi', label: 'Pi', className: 'provider-pi' },
  { id: 'magi', label: 'Magi', className: 'system-magi' },
  { id: 'violet', label: 'Violet', className: 'system-violet' },
  { id: 'ember', label: 'Ember', className: 'system-ember' },
  { id: 'laughing-man', label: 'Laughing Man', className: 'system-laughing-man' },
  { id: 'puppeteer', label: 'Puppeteer', className: 'system-puppeteer' },
  { id: 'bartender', label: 'Bartender', className: 'system-bartender' },
  { id: 'user-default', label: 'Human', className: 'system-human' },
];

const HERO_AVATAR_BY_ID = Object.fromEntries(
  HERO_AVATARS.map((avatar) => [avatar.id, avatar]),
) as Record<GeneratedHeroAvatarId, HeroAvatarSpec>;

const DEFAULT_AVATAR_BY_PROVIDER: Record<HeroProviderId, GeneratedHeroAvatarId> = {
  claude: 'claude',
  codex: 'codex',
  antigravity: 'antigravity',
  opencode: 'opencode',
  pi: 'pi',
};
const USER_AVATAR_STORAGE_KEY = 'kota-v2.user-hero-avatars';
const TAVERN_PROFILE_STORAGE_KEY = 'kota-v2.tavern.hero-profiles';
const USER_AVATAR_CHANGED_EVENT = 'kota-v2:user-hero-avatars-changed';
let userAvatarCache: UserHeroAvatar[] = [];

export function isHeroProviderId(value: unknown): value is HeroProviderId {
  return value === 'claude' || value === 'codex' || value === 'antigravity' || value === 'opencode' || value === 'pi';
}

export function providerFromCli(cli: AgentCli | string | null | undefined): HeroProviderId {
  if (cli === 'claude' || cli === 'codex' || cli === 'antigravity' || cli === 'opencode' || cli === 'pi') return cli;
  if (cli === 'agy') return 'antigravity';
  return 'codex';
}

export function defaultAvatarIdForProvider(provider: string | null | undefined): GeneratedHeroAvatarId {
  return DEFAULT_AVATAR_BY_PROVIDER[isHeroProviderId(provider) ? provider : 'codex'];
}

export function isUserHeroAvatarId(value: string | null | undefined): value is string {
  return !!value && value.startsWith('user:');
}

export function loadUserHeroAvatars(): UserHeroAvatar[] {
  if (userAvatarCache.length > 0) return userAvatarCache;
  if (typeof window === 'undefined') return [];
  try {
    const parsed = JSON.parse(window.localStorage.getItem(USER_AVATAR_STORAGE_KEY) || '[]');
    if (!Array.isArray(parsed)) return [];
    userAvatarCache = parsed.filter((item): item is UserHeroAvatar => (
      item &&
      typeof item.id === 'string' &&
      item.id.startsWith('user:') &&
      typeof item.label === 'string' &&
      typeof item.dataUrl === 'string' &&
      item.dataUrl.startsWith('data:image/') &&
      typeof item.createdAt === 'string'
    ));
    return userAvatarCache;
  } catch {
    return [];
  }
}

export function saveUserHeroAvatars(avatars: readonly UserHeroAvatar[]): void {
  userAvatarCache = [...avatars];
  if (typeof window === 'undefined') return;
  window.localStorage.setItem(USER_AVATAR_STORAGE_KEY, JSON.stringify(avatars));
  window.dispatchEvent(new Event(USER_AVATAR_CHANGED_EVENT));
}

export async function refreshUserHeroAvatars(): Promise<UserHeroAvatar[]> {
  if (typeof window === 'undefined') return [];
  if (isTauri()) {
    const fromDisk = await invoke<UserHeroAvatar[]>('hero_avatar_list');
    const local = readLocalUserHeroAvatars();
    if (fromDisk.length === 0 && local.length > 0) {
      const migrated: UserHeroAvatar[] = [];
      for (const avatar of local) {
        migrated.push(await invoke<UserHeroAvatar>('hero_avatar_save', {
          request: {
            id: avatar.id,
            label: avatar.label,
            dataUrl: avatar.dataUrl,
          },
        }));
      }
      saveUserHeroAvatars(migrated);
      return migrated;
    }
    saveUserHeroAvatars(fromDisk);
    return fromDisk;
  }
  const local = readLocalUserHeroAvatars();
  saveUserHeroAvatars(local);
  return local;
}

export function userAvatarUrlForId(id: string | null | undefined): string | null {
  if (!isUserHeroAvatarId(id)) return null;
  return loadUserHeroAvatars().find((avatar) => avatar.id === id)?.dataUrl ?? null;
}

export async function addUserHeroAvatar(fileName: string, dataUrl: string): Promise<UserHeroAvatar> {
  const base = fileName.replace(/\.[^.]+$/, '').trim() || 'Avatar';
  if (typeof window !== 'undefined' && isTauri()) {
    const avatar = await invoke<UserHeroAvatar>('hero_avatar_save', {
      request: { label: base.slice(0, 40), dataUrl },
    });
    saveUserHeroAvatars([...loadUserHeroAvatars().filter((item) => item.id !== avatar.id), avatar]);
    return avatar;
  }
  const avatar = makeLocalUserHeroAvatar(base, dataUrl);
  saveUserHeroAvatars([...loadUserHeroAvatars(), avatar]);
  return avatar;
}

export async function deleteUserHeroAvatar(id: string): Promise<void> {
  const references = avatarReferenceNames(id);
  if (references.length > 0) {
    throw new Error(`Avatar is still used by ${references.join(', ')}`);
  }
  if (typeof window !== 'undefined' && isTauri()) {
    await invoke('hero_avatar_delete', { request: { avatarId: id } });
  }
  saveUserHeroAvatars(loadUserHeroAvatars().filter((avatar) => avatar.id !== id));
}

export function avatarReferenceNames(id: string, extraNames: readonly string[] = []): string[] {
  if (!isUserHeroAvatarId(id)) return [];
  const names = new Set(extraNames.filter(Boolean));
  if (typeof window !== 'undefined') {
    try {
      const parsed = JSON.parse(window.localStorage.getItem(TAVERN_PROFILE_STORAGE_KEY) || '{}');
      if (parsed && typeof parsed === 'object') {
        for (const value of Object.values(parsed) as Array<Record<string, unknown>>) {
          if (value?.avatarId === id) {
            names.add(typeof value.name === 'string' && value.name.trim() ? value.name : 'Tavern hero');
          }
        }
      }
    } catch {
      // Reference checks are best-effort in browser-only mode.
    }
  }
  return [...names];
}

export function normalizeHeroAvatarId(
  value: string | null | undefined,
  provider: string | null | undefined,
): HeroAvatarId {
  const fallback = defaultAvatarIdForProvider(provider);
  if (!value) return fallback;
  if (isUserHeroAvatarId(value) && userAvatarUrlForId(value)) return value;
  if (value in HERO_AVATAR_BY_ID) return value;
  return fallback;
}

export function avatarOptionsForProvider(provider: string | null | undefined): HeroAvatarSpec[] {
  void provider;
  return [...HERO_AVATARS];
}

export function avatarClassForId(
  value: string | null | undefined,
  provider: string | null | undefined,
): string {
  const normalized = normalizeHeroAvatarId(value, provider);
  if (isUserHeroAvatarId(normalized)) return 'user-avatar';
  return HERO_AVATAR_BY_ID[normalized as GeneratedHeroAvatarId].className;
}

export function avatarImageStyleForId(
  value: string | null | undefined,
): {
  backgroundImage: string;
  backgroundSize: string;
  backgroundPosition: string;
  backgroundRepeat: string;
  borderRadius: string;
  overflow: 'hidden';
} | undefined {
  const url = userAvatarUrlForId(value);
  if (!url) return undefined;
  // User-uploaded avatars are baked to a full-bleed square. Carry the sizing
  // inline (not via the `.user-avatar` class) because some render sites resolve
  // the class to `provider-*` while still injecting the user image here.
  // Use 114% (NOT `cover`): in WKWebView, background-size:cover (exact-fill) on a
  // CSS background-image bypasses border-radius clipping and renders SQUARE edges.
  // Any size >100% avoids the bug (generated avatars use 114% and stay round);
  // 114% crops ~7%/edge, fine for the centered square bake. `transform:
  // translateZ(0)` was tried as a zero-crop fix but WKWebView optimizes the
  // identity transform away (computes to matrix(1,0,0,1,0,0), no compositing).
  return {
    backgroundImage: `url("${url.replace(/"/g, '\\"')}")`,
    backgroundSize: '114%',
    backgroundPosition: 'center',
    backgroundRepeat: 'no-repeat',
    borderRadius: '50%',
    overflow: 'hidden',
  };
}

export function avatarClassForCli(cli: AgentCli | string | null | undefined): string {
  return avatarClassForId(null, providerFromCli(cli));
}

export function avatarClassForAgentFallback(
  cli: AgentCli | string | null | undefined,
  id: string,
): string {
  const baseId = id.replace(/-\d+$/, '');
  if (cli) return avatarClassForCli(cli);
  if (baseId === 'hero-cc') return avatarClassForId(null, 'claude');
  if (baseId === 'hero-dex') return avatarClassForId(null, 'codex');
  if (baseId === 'hero-gem') return avatarClassForId(null, 'antigravity');
  if (baseId === 'hero-op') return avatarClassForId(null, 'opencode');
  if (baseId === 'hero-pi') return avatarClassForId(null, 'pi');
  if (baseId === 'claude') return avatarClassForId(null, 'claude');
  if (baseId === 'codex') return avatarClassForId(null, 'codex');
  if (baseId === 'alice') return avatarClassForId(null, 'claude');
  if (baseId === 'bob') return avatarClassForId(null, 'codex');
  if (baseId === 'david') return avatarClassForId(null, 'antigravity');
  if (baseId === 'charlie') return avatarClassForId(null, 'opencode');
  return avatarClassForId(null, 'codex');
}

function readLocalUserHeroAvatars(): UserHeroAvatar[] {
  if (typeof window === 'undefined') return [];
  try {
    const parsed = JSON.parse(window.localStorage.getItem(USER_AVATAR_STORAGE_KEY) || '[]');
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is UserHeroAvatar => (
      item &&
      typeof item.id === 'string' &&
      item.id.startsWith('user:') &&
      typeof item.label === 'string' &&
      typeof item.dataUrl === 'string' &&
      item.dataUrl.startsWith('data:image/') &&
      typeof item.createdAt === 'string'
    ));
  } catch {
    return [];
  }
}

function makeLocalUserHeroAvatar(label: string, dataUrl: string): UserHeroAvatar {
  const id = `user:${typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random().toString(16).slice(2)}`}`;
  return {
    id,
    label: label.slice(0, 40),
    dataUrl,
    createdAt: new Date().toISOString(),
    mime: dataUrl.slice(5, dataUrl.indexOf(';')) || null,
    sizeBytes: dataUrl.length,
  };
}
