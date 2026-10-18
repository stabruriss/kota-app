import claude from '../assets/tavern/icons/providers/claude.svg';
import codex from '../assets/tavern/icons/providers/openai.svg';
import antigravity from '../assets/tavern/icons/providers/googlegemini.svg';
import opencode from '../assets/tavern/icons/providers/opencode.svg';
import kimi from '../assets/tavern/icons/providers/kimi.svg';
import pi from '../assets/tavern/icons/providers/pi.svg';
import tavernEntry from '../assets/tavern/icons/tavern-entry.svg';

export const PROVIDER_ICONS = {
  'kota-local': { id: 'kota-local', label: 'Kota Local', svg: tavernEntry },
  claude: { id: 'claude', label: 'Claude Code', svg: claude },
  codex: { id: 'codex', label: 'Codex', svg: codex },
  antigravity: { id: 'antigravity', label: 'Antigravity CLI', svg: antigravity },
  opencode: { id: 'opencode', label: 'OpenCode', svg: opencode },
  pi: { id: 'pi', label: 'Pi', svg: pi },
  kimi: { id: 'kimi', label: 'Kimi Code', svg: kimi },
} as const;

export type ProviderId = keyof typeof PROVIDER_ICONS;

export function providerIconForId(id: string | null | undefined) {
  return id && Object.prototype.hasOwnProperty.call(PROVIDER_ICONS, id)
    ? PROVIDER_ICONS[id as ProviderId]
    : null;
}
