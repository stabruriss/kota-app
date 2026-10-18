/** Local-only composition of Pi's native codemode and MCP extensions. */
import {
  createCodemodeExtension, createMcpExtension, createToolSearchExtension, getAgentDir,
  type ExtensionAPI, type ExtensionContext,
} from '@earendil-works/pi-coding-agent';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { mcpSnapshot, parseMcpRequest } from './mcp-status.ts';

export async function registerLocalCapabilities(pi: ExtensionAPI) {
  // A query or login helper must never connect servers or expose model tools.
  if (process.env.KOTA_LOCAL_AUTH_HELPER === '1') return;
  await createCodemodeExtension({ mode: 'on' })(pi);
  await createToolSearchExtension()(pi);
  let completions: ((prefix: string) => unknown) | undefined;
  let configured: Array<{ name: string; enabled: boolean }> = [];
  let unavailable = false;

  const context = (ctx: ExtensionContext) => new Proxy(ctx, {
    get(target, key) {
      if (key !== 'ui') return Reflect.get(target, key);
      return new Proxy(target.ui, {
        get(ui, field) {
          if (field !== 'notify') return Reflect.get(ui, field);
          return (message: string, kind: 'info' | 'warning' | 'error') => {
            // Native configuration/runtime-load errors have no public getter.
            // Retain only a boolean, never their possibly sensitive contents.
            if (message.startsWith('MCP failed to load:') ||
                (message.startsWith('MCP servers need attention:') && /\n\s+config:/.test(message))) unavailable = true;
            ui.notify(message, kind);
          };
        },
      });
    },
  });
  const native = new Proxy(pi, {
    get(target, key) {
      if (key === 'registerCommand') return (name: string, command: Parameters<ExtensionAPI['registerCommand']>[1]) => {
        if (name === 'mcp') completions = command.getArgumentCompletions;
        target.registerCommand(name, command);
      };
      if (key === 'on') return (event: string, handler: (event: never, ctx: ExtensionContext) => unknown) => {
        // The native handlers and command remain unchanged. This observes only
        // the public notification surface, without accessing their closures.
        (target.on as (event: string, handler: unknown) => void)(event, (event: never, ctx: ExtensionContext) => handler(event, context(ctx)));
      };
      return Reflect.get(target, key);
    },
  });

  // Capture the configuration seen at this session's start, not later edits.
  pi.on('session_start', () => {
    unavailable = false;
    configured = [];
    try {
      const value = JSON.parse(readFileSync(join(getAgentDir(), 'mcp.json'), 'utf8'));
      if (!value || typeof value !== 'object' || Array.isArray(value) ||
          (value.mcpServers !== undefined && (!value.mcpServers || typeof value.mcpServers !== 'object' || Array.isArray(value.mcpServers)))) {
        unavailable = true; return;
      }
      configured = Object.entries(value.mcpServers ?? {}).map(([name, config]) => ({
        name, enabled: (config as { enabled?: boolean })?.enabled !== false,
      }));
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') unavailable = true; }
  });
  await createMcpExtension()(native);
  pi.on('session_start', () => {
    const active = pi.getActiveTools();
    if (!active.includes('codemode')) pi.setActiveTools([...active, 'codemode']);
  });
  pi.registerCommand('kota-mcp', {
    description: 'Read Local MCP connection state',
    handler: async (args, ctx) => {
      const request = parseMcpRequest(args);
      if (!request) throw new Error('invalid MCP status request');
      // getArgumentCompletions is synchronous. Calling /mcp's handler would
      // await all connections and could hang this read-only UI query.
      const snapshot = mcpSnapshot(completions, configured, unavailable);
      ctx.ui.notify(`kota.mcp.v1:${JSON.stringify({ schema: 1, requestId: request.requestId, ...snapshot })}`, 'info');
    },
  });
}
