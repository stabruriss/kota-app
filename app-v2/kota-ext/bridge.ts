import type { ExtensionAPI } from '@earendil-works/pi-coding-agent';
import { parseInjection, parseCancelId, parseQueueControl, parseQueueReorder, QUEUE_CONTROL_CODEC } from './inject.ts';

export function registerInjection(pi: ExtensionAPI) {
  const held: ReturnType<typeof parseInjection>[] = [];
  let heldBytes = 0;
  let paused = false;
  const bytes = (input: ReturnType<typeof parseInjection>) => Buffer.byteLength(JSON.stringify(input));
  const send = (input: ReturnType<typeof parseInjection>) => pi.sendMessage({
    customType: input.customType, content: input.content, display: true,
    details: { ...input.details, kotaId: input.kotaId },
  }, { triggerTurn: true, deliverAs: 'steer' });
  // Synchronous boundary: no await between taking the batch and steering it.
  const flush = () => { if (paused) return; const batch = held.splice(0); heldBytes = 0; for (const input of batch) send(input); };
  pi.on('turn_end', flush);
  pi.on('agent_before_settle', flush);
  pi.registerCommand('kota-inject', {
    description: 'Inject an existing Kota input.',
    handler: async (raw, ctx) => {
      const input = parseInjection(raw);
      if (held.some(item => item.kotaId === input.kotaId)) throw new Error(`kota-inject-failed kotaId=${encodeURIComponent(input.kotaId)} code=duplicate`);
      if (!paused && ctx.isIdle()) { flush(); send(input); }
      else {
        const size = bytes(input);
        if (held.length >= 64 || heldBytes + size > 32 * 1024 * 1024) {
          throw new Error(`kota-inject-failed kotaId=${encodeURIComponent(input.kotaId)} code=queue-limit`);
        }
        held.push(input); heldBytes += size;
      }
    },
  });
  pi.registerCommand('kota-cancel', {
    description: 'Remove one waiting Kota input.',
    handler: async raw => {
      const id = parseCancelId(raw);
      const index = held.findIndex(item => item.kotaId === id);
      if (index < 0) throw new Error(`kota-cancel-too-late kotaId=${encodeURIComponent(id)}`);
      heldBytes -= bytes(held[index]); held.splice(index, 1);
    },
  });
  pi.registerCommand('kota-reorder', {
    description: 'Reorder waiting Kota inputs.',
    handler: async raw => {
      const { requestId, ids } = parseQueueReorder(raw);
      const byId = new Map(held.map(item => [item.kotaId, item]));
      if (paused || ids.length !== held.length || ids.some(id => !byId.has(id))) {
        throw new Error(`kota-reorder-changed requestId=${requestId}`);
      }
      // Validation and replacement share one synchronous boundary with flush.
      held.splice(0, held.length, ...ids.map(id => byId.get(id)!));
    },
  });
  pi.registerCommand('kota-hold-pause', {
    description: 'Pause waiting Kota inputs.',
    handler: async (raw, ctx) => {
      const requestId = parseQueueControl(raw);
      paused = true;
      ctx.ui.notify(QUEUE_CONTROL_CODEC + JSON.stringify({ schema: 1, requestId, action: 'pause', kotaIds: held.map(item => item.kotaId) }), 'info');
    },
  });
  pi.registerCommand('kota-hold-resume', {
    description: 'Resume waiting Kota inputs.',
    handler: async (raw, ctx) => {
      const requestId = parseQueueControl(raw);
      if (!paused || !ctx.isIdle()) throw new Error('Kota queue is not paused and idle.');
      const kotaIds = held.map(item => item.kotaId);
      paused = false;
      flush();
      ctx.ui.notify(QUEUE_CONTROL_CODEC + JSON.stringify({ schema: 1, requestId, action: 'resume', kotaIds }), 'info');
    },
  });
  pi.registerCommand('kota-clear-held', { description: 'Discard waiting Kota inputs.', handler: async () => { held.splice(0); heldBytes = 0; } });
}
