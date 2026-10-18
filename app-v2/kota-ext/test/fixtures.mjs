import { fileURLToPath } from 'node:url';

export const extensionPath = fileURLToPath(new URL('../index.ts', import.meta.url));
export const tinyPng = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII=';

export function envelope(messageId = 'msg-fixture', customType = 'kota.user', content = '  unchanged\ninput\u2028\u2029  ') {
  return {
    kotaId: `${messageId}#agent-fixture`, customType, content,
    details: { schema: 1, messageId, targetAgentId: 'agent-fixture', createdAt: '2026-10-02T00:00:00Z', private: false },
  };
}

export const injectCommand = input => ({ message: `/kota-inject ${JSON.stringify(input)}` });
export const isConsumed = kotaId => record => record.type === 'message_end' && record.message.role === 'custom' && record.message.details?.kotaId === kotaId;
