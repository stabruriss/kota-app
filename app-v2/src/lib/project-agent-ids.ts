const PROJECT_AGENT_ID_PREFIX = 'agent-';

export function mintProjectAgentId(
  occupiedIds: ReadonlySet<string>,
  candidateFactory = randomProjectAgentId,
): string {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    const candidate = normalizeProjectAgentId(candidateFactory());
    if (!occupiedIds.has(candidate)) return candidate;
  }
  throw new Error('Could not mint an available agent workspace id.');
}

function randomProjectAgentId(): string {
  const cryptoApi = globalThis.crypto;
  if (cryptoApi?.randomUUID) {
    return `${PROJECT_AGENT_ID_PREFIX}${cryptoApi.randomUUID().replace(/-/g, '').slice(0, 10)}`;
  }
  if (cryptoApi?.getRandomValues) {
    const bytes = new Uint8Array(6);
    cryptoApi.getRandomValues(bytes);
    return `${PROJECT_AGENT_ID_PREFIX}${Array.from(bytes, (byte) => byte.toString(36).padStart(2, '0')).join('').slice(0, 10)}`;
  }
  return `${PROJECT_AGENT_ID_PREFIX}${Math.random().toString(36).slice(2, 12).padEnd(10, '0')}`;
}

function normalizeProjectAgentId(value: string): string {
  const suffix = String(value)
    .toLowerCase()
    .replace(/^agent-/, '')
    .replace(/[^a-z0-9]+/g, '')
    .slice(0, 10);
  return `${PROJECT_AGENT_ID_PREFIX}${suffix || Math.random().toString(36).slice(2, 12).padEnd(10, '0')}`;
}
