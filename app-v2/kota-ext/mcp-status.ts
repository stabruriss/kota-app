/** Human-only projection of Pi 1.3.0's native /mcp completion descriptions. */
export type McpHealth = 'connected' | 'connecting' | 'needs_auth' | 'not_connected' | 'failed';
export type McpReason = 'connection_failed' | 'connection_refused' | 'command_not_found' | 'connection_timeout' | 'authentication_failed';
export type McpRow = { name: string; status: McpHealth; reason?: McpReason };
export type McpCompletion = { value: string; label: string; description?: string };
const validName = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);

export function parseMcpRequest(text: string): { schema: 1; requestId: string } | undefined {
  if (text.length > 1024) return;
  try {
    const value = JSON.parse(text);
    if (value?.schema === 1 && validName(value.requestId) &&
        Object.keys(value).length === 2) return value;
  } catch { /* No partial IDs or text are recovered. */ }
}

function reason(error: string): McpReason {
  if (/\b(401|403)\b|unauthori[sz]ed|forbidden/i.test(error)) return 'authentication_failed';
  if (/ENOENT|not found/i.test(error)) return 'command_not_found';
  if (/ECONNREFUSED|connection refused/i.test(error)) return 'connection_refused';
  if (/timed? ?out|timeout/i.test(error)) return 'connection_timeout';
  return 'connection_failed';
}
export function completionRow(item: McpCompletion): McpRow | undefined {
  if (!validName(item.label) || item.value !== `reconnect ${item.label}` || typeof item.description !== 'string') return;
  const text = item.description;
  if (/^connected · \d+ tools?( · (\d+ resources?|resources))?$/.test(text)) return { name: item.label, status: 'connected' };
  if (text === 'connecting…' || text === 'starting') return { name: item.label, status: 'connecting' };
  if (text === 'needs sign-in') return { name: item.label, status: 'needs_auth' };
  if (text === 'disconnected' || text === 'closed' || text === 'disabled') return { name: item.label, status: 'not_connected' };
  if (text.startsWith('failed: ')) return { name: item.label, status: 'failed', reason: reason(text.slice(8)) };
}

export function mcpSnapshot(completions: ((prefix: string) => unknown) | undefined,
  configured: ReadonlyArray<{ name: string; enabled: boolean }>, unavailable: boolean): { available: boolean; servers: McpRow[] } {
  const unknown = { available: false, servers: [] };
  if (unavailable || !completions || configured.length > 256 || configured.some(row => !validName(row.name))) return unknown;
  try {
    const items = completions('reconnect ');
    if (items instanceof Promise) { void items.catch(() => {}); return unknown; }
    if (items !== null && !Array.isArray(items)) return unknown;
    const rows = new Map<string, McpRow>();
    for (const item of items ?? []) {
      const row = completionRow(item);
      if (!row || rows.has(row.name) || rows.size >= 256) return unknown;
      rows.set(row.name, row);
    }
    for (const row of configured) if (!rows.has(row.name)) rows.set(row.name, {
      name: row.name, status: row.enabled ? 'connecting' : 'not_connected',
    });
    return { available: true, servers: [...rows.values()] };
  } catch { return unknown; }
}
