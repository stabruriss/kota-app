export const MAX_AGENT_SLOTS = 8;

export const AGENT_SLOT_KEY_RANGE_LABEL = `1-${MAX_AGENT_SLOTS}`;

export function agentSlotIndexFromKey(key: string): number | null {
  if (!/^[1-9]$/.test(key)) return null;
  const index = Number(key) - 1;
  return index >= 0 && index < MAX_AGENT_SLOTS ? index : null;
}
