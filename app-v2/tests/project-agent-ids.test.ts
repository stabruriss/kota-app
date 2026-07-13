import { describe, expect, it } from 'vitest';

import { mintProjectAgentId } from '../src/lib/project-agent-ids';

describe('project agent workspace ids', () => {
  it('mints opaque ids independent from template ids and display names', () => {
    expect(mintProjectAgentId(new Set(), () => 'anything-readable')).toBe('agent-anythingre');
  });

  it('normalizes candidate ids into the agent namespace', () => {
    expect(mintProjectAgentId(new Set(), () => 'custom-1778909084279')).toBe('agent-custom1778');
  });

  it('skips collisions without deriving from mutable display names', () => {
    const candidates = ['agent-first', 'agent-second'];
    expect(mintProjectAgentId(new Set(['agent-first']), () => candidates.shift() ?? 'agent-fallback'))
      .toBe('agent-second');
  });
});
