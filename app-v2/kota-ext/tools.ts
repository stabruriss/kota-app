import { Type } from '@earendil-works/pi-ai';
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { executeCommand, wrapperFailure, timeoutForTool, FAILURE_COPY, type CommandResult } from './cli.ts';
import { buildInvocation, InvalidToolArguments, type ToolName } from './tool-command.ts';

const nonempty = Type.String({ minLength: 1 });
const targets = Type.Array(nonempty, { minItems: 1 });
const attachments = Type.Optional(Type.Array(nonempty, { maxItems: 9 }));
// Roots stay single objects: Pi's non-strict tool conversion keeps only root type/properties/required
// (pi-ai api/anthropic-messages.js convertTools), so a union root reaches the model as an empty object.
// Per-action required/forbidden fields stay strictly enforced by tool-command.ts fields()/timing().
const timing = Type.Object({ kind: Type.Union(['in', 'every', 'at', 'cron', 'idle'].map(value => Type.Literal(value))), value: Type.Optional(nonempty) }, { additionalProperties: false });
const end = { endAfter: Type.Optional(Type.Integer({ minimum: 1 })), endAt: Type.Optional(nonempty) };
const post = { at: Type.Optional(nonempty), attachments };

const schemas = {
  kota_agent_bus: Type.Object({ to: nonempty, body: nonempty, intent: Type.Optional(nonempty) }, { additionalProperties: false }),
  kota_bbs: Type.Object({
    action: Type.Union(['agents', 'show', 'new', 'reply'].map(value => Type.Literal(value))),
    threadId: Type.Optional(nonempty),
    body: Type.Optional(Type.String()),
    projects: Type.Optional(targets),
    broadcast: Type.Optional(Type.Boolean()),
    ...post,
  }, { additionalProperties: false }),
  kota_ember: Type.Object({
    action: Type.Union(['list', 'show', 'delete', 'add', 'update'].map(value => Type.Literal(value))),
    id: Type.Optional(nonempty),
    draft: Type.Optional(Type.Boolean()),
    body: Type.Optional(nonempty),
    to: Type.Optional(targets),
    timing: Type.Optional(timing),
    ...end,
  }, { additionalProperties: false }),
  kota_bartender: Type.Object({ action: Type.Literal('sync') }, { additionalProperties: false }),
  kota_my_soul: Type.Object({
    action: Type.Union(['add-mark', 'edit-mark', 'delete-mark', 'read-mark', 'search-episode', 'clear-heat'].map(value => Type.Literal(value))),
    text: Type.Optional(nonempty),
    id: Type.Optional(nonempty),
    query: Type.Optional(nonempty),
    project: Type.Optional(nonempty),
  }, { additionalProperties: false }),
};

// Approved TOOLS-FINAL.md, SHA-256 7e422261e0029a9ff15a613c58505ad1a97e3a1dde2c2844b50246bbaa40b7b5.
const copy: Record<ToolName, { label: string; description: string }> = {
  kota_agent_bus: {
    label: 'Agent Bus',
    description: "Send a message to a teammate in the current project. Use the teammate's AKA or agent ID in to. The body is sent unchanged. A successful result means the local dispatch was recorded, not that the teammate has replied.",
  },
  kota_bbs: {
    label: 'Bulletin Board',
    description: 'Look up projects and agents, read a BBS thread, or publish a new post or reply. Use action agents to find project IDs and the targetRef for at. Set explicit projects or broadcast for a new thread. Use at for an explicit mention and attachments for files to copy into the post. Successful publication does not confirm delivery to another device.',
  },
  kota_ember: {
    label: 'Ember',
    description: 'Create, list, read, update, or delete scheduled prompts and draft notes for the current project. Schedule targets use agent names, agent IDs, or the configured human name. A draft is a note without delivery timing. Timing: in and every take a duration such as 10m, 2h, or 1d (every at least 1m); at takes a local datetime such as "2026-06-18 14:00"; cron takes a cron expression; idle delivers when the target is idle.',
  },
  kota_bartender: {
    label: 'Bartender',
    description: "Request synchronization of this project's agent worktrees through the running Kota app. Use only when the user has asked to sync. Concurrent sync requests are rejected by the existing service.",
  },
  // kota-my-soul TEXTS-FOR-REVIEW.md §E (SOUL-PLAN v4 §13/§17), used verbatim.
  kota_my_soul: {
    label: 'My Soul',
    description: 'kota_my_soul — Your marks (lessons you keep across projects) and episodes (past room messages). Actions: add-mark, edit-mark, delete-mark, read-mark, search-episode, clear-heat. Same arguments and output as the kota-my-soul CLI.',
  },
};

export function registerTools(pi: ExtensionAPI) {
  for (const name of Object.keys(schemas) as ToolName[]) {
    pi.registerTool({
      name,
      label: copy[name].label,
      description: copy[name].description,
      parameters: schemas[name],
      async execute(_toolCallId, args, signal, _onUpdate, ctx) {
        let result: CommandResult;
        try {
          result = await executeCommand(buildInvocation(name, args), { cwd: ctx.cwd, signal, copy: FAILURE_COPY, timeoutMs: timeoutForTool(name) });
        } catch (error) {
          if (!(error instanceof InvalidToolArguments)) throw error;
          result = wrapperFailure('invalid_arguments', FAILURE_COPY);
        }
        return {
          content: [{ type: 'text', text: JSON.stringify(result) }],
          details: result.error === 'output_limit' ? { ...result, truncated: true } : result,
          isError: result.exitCode !== 0 || result.error !== undefined,
        };
      },
    });
  }
}
