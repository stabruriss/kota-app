import { describe, expect, it } from 'vitest';

import {
  formatAgentPromptInput,
  normalizeAgentPromptPayload,
} from '../src/lib/agent-prompt';

describe('agent prompt formatting', () => {
  it('normalizes terminal newlines without trimming prompt text spaces', () => {
    expect(normalizeAgentPromptPayload('first\r\nsecond\rthird\n\n')).toBe('first\nsecond\nthird');
    expect(normalizeAgentPromptPayload('keep trailing spaces  \n')).toBe('keep trailing spaces  ');
  });

  it('wraps prompt text in terminal paste semantics before submit', () => {
    expect(formatAgentPromptInput('line one\nline two')).toBe(
      '\x1b[200~line one\nline two\x1b[201~',
    );
  });

  it('strips pasted bracketed-paste markers from user content', () => {
    expect(formatAgentPromptInput('\x1b[200~hello\x1b[201~')).toBe(
      '\x1b[200~hello\x1b[201~',
    );
  });

  it('does not emit input bytes for blank payloads', () => {
    expect(formatAgentPromptInput(' \n\t\n')).toBe('');
  });
});
