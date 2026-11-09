import { Buffer } from 'node:buffer';
import type { ExtensionAPI, SessionEntry } from '@earendil-works/pi-coding-agent';

export const HISTORY_CODEC = 'kota.history.v1:';
// Reserve an envelope allowance even if every inner byte needs escaping in
// the outer RPC notification. The process's 64 MiB record limit stays intact.
export const HISTORY_BYTES = 32 * 1024 * 1024 - 1024;

export function parseHistoryRequest(text: string): { schema: 1; requestId: string } | undefined {
  if (text.length > 1024) return;
  try {
    const value = JSON.parse(text);
    if (value?.schema === 1 && typeof value.requestId === 'string' &&
        /^[A-Za-z0-9_-]{1,128}$/.test(value.requestId) && Object.keys(value).length === 2) return value;
  } catch { /* Never recover partial control text. */ }
}

/** A display/reconciliation copy, never a mutation of Pi's history or context. */
export function historyReply(requestId: string, entries: SessionEntry[], leafId: string | null): string {
  const view = entries.map(entry => {
    if (entry.type !== 'message' || entry.message.role !== 'toolResult') return entry;
    const content = entry.message.content.filter(block => block.type !== 'image');
    if (content.length === entry.message.content.length) return entry;
    return { ...entry, message: { ...entry.message, content } };
  });
  const text = JSON.stringify({ schema: 1, requestId, result: { status: 'ready', entries: view, leafId } });
  // Measure exactly the string we send, in UTF-8 bytes (not UTF-16 length).
  // A text-only oversized history still has this one temporary allocation.
  if (Buffer.byteLength(text, 'utf8') <= HISTORY_BYTES) return HISTORY_CODEC + text;
  // requestId has already passed the strict command parser; no large second
  // serialization or copy is needed to report the fallback.
  return `${HISTORY_CODEC}{"schema":1,"requestId":"${requestId}","result":{"status":"too_large"}}`;
}

export function registerHistory(pi: ExtensionAPI) {
  pi.registerCommand('kota-history', {
    description: 'Read Local history without tool result images',
    handler: async (args, ctx) => {
      const request = parseHistoryRequest(args);
      if (!request) throw new Error('invalid history request');
      ctx.ui.notify(historyReply(request.requestId, ctx.sessionManager.getEntries(), ctx.sessionManager.getLeafId()), 'info');
    },
  });
}
