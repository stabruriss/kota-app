import { Type } from '@earendil-works/pi-ai';
import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';

export const CLONE_CODEC = 'kota.clone.v1:';
// PROMPTS-v3.md, authored by the user. Preserve spelling and line breaks.
export const CLONE_DESCRIPTION = `Start copies of yourself. Each clone is a
separate agent in this workspace. Clones
do not have context, so give each one a complete task. create
returns with the clone ids; each clone's final
reply arrives as a message when it finishes. ask sends a follow-up to a
clone that has finished; it continues with its own history. status lists
your clones and what each one last said. stop ends a clone and keeps its
records in under-table/clone-<id>/; destroy removes them. Unless read only, clones could edit your files. At most 7 clones run at once, and a run is limited to max 90
minutes unless you set another limit.`;

const parameters = Type.Object({
  action: Type.Union(['create', 'ask', 'status', 'stop', 'destroy'].map(action => Type.Literal(action)), { description: 'create, ask, status, stop, or destroy.' }),
  tasks: Type.Optional(Type.Array(Type.Object({
    task: Type.String({ minLength: 1, description: 'What the clone should do, with the context it needs.' }),
    read_only: Type.Optional(Type.Boolean({ description: 'true to give the clone read-only file access.' })),
    timeout_minutes: Type.Optional(Type.Number({ minimum: 0, description: 'Run time limit. Default 90. Use 0 for no limit.' })),
  }, { additionalProperties: false }), { minItems: 1, description: 'For create: one entry per clone.' })),
  clone_id: Type.Optional(Type.Integer({ minimum: 1, description: 'For ask, stop, and destroy: the clone to act on.' })),
  message: Type.Optional(Type.String({ minLength: 1, description: 'For ask: the follow-up for the clone.' })),
  timeout_minutes: Type.Optional(Type.Number({ minimum: 0, description: 'For ask: a new run time limit for this clone.' })),
}, { additionalProperties: false });

/** A thin RPC client; Rust owns every copy and its lifetime. */
export function registerShadowClones(pi: ExtensionAPI) {
  if (process.env.KOTA_LOCAL_AUTH_HELPER === '1') return;
  pi.registerTool({
    name: 'kota_shadow_clone', label: 'Kage Bunshin', description: CLONE_DESCRIPTION, parameters,
    async execute(_id, args, signal, _update, ctx) {
      let result: { success: boolean; data?: unknown; error?: string };
      if (ctx.mode !== 'rpc') result = { success: false, error: 'clone_rpc_required' };
      else {
        const response = await ctx.ui.input(CLONE_CODEC + JSON.stringify(args), '', { signal });
        try {
          const parsed = response === undefined ? undefined : JSON.parse(response);
          result = parsed && typeof parsed === 'object' && typeof parsed.success === 'boolean' ? parsed
            : { success: false, error: response === undefined ? 'clone_cancelled' : 'clone_response_invalid' };
        }
        catch { result = { success: false, error: 'clone_response_invalid' }; }
      }
      const data = result.success ? result.data : { error: result.error ?? 'clone_response_invalid' };
      return { content: [{ type: 'text', text: JSON.stringify(data) }], details: data, isError: !result.success };
    },
  });
}
