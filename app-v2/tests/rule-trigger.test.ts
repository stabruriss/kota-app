import { describe, expect, it } from 'vitest';

import { normalizedRuleTrigger } from '../src/lib/rule-trigger';

describe('rule trigger normalization', () => {
  it('collapses pasted newlines, repeated spaces, and surrounding whitespace', () => {
    expect(normalizedRuleTrigger({
      loadPolicy: 'on-demand',
      taskTrigger: '  coding,   debugging,\n\n  design\t review  ',
    })).toBe('coding, debugging, design review');
  });

  it('clears dormant trigger text for always-loaded rules', () => {
    expect(normalizedRuleTrigger({
      loadPolicy: 'always',
      taskTrigger: 'coding, debugging',
    })).toBe('');
  });
});
