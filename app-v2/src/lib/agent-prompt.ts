const BRACKETED_PASTE_START = '\x1b[200~';
const BRACKETED_PASTE_END = '\x1b[201~';
const BRACKETED_PASTE_MARKER = /\x1b\[(?:200|201)~/g;

export function normalizeAgentPromptPayload(payload: string): string {
  return payload
    .replace(BRACKETED_PASTE_MARKER, '')
    .replace(/\r\n/g, '\n')
    .replace(/\r/g, '\n')
    .replace(/\n+$/g, '');
}

export function formatAgentPromptInput(rawPayload: string): string {
  const payload = normalizeAgentPromptPayload(rawPayload);
  if (!payload.trim()) return '';
  return `${BRACKETED_PASTE_START}${payload}${BRACKETED_PASTE_END}`;
}
