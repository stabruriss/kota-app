import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { LocalModelCredit, localModelProvider } from '../src/chrome/LocalModelCredit';

afterEach(cleanup);

it('uses the exact leading provider id, keeping malformed ids and Claude from other providers unmarked', () => {
  for (const model of ['Anthropic/claude', 'anthropic-extra/claude', 'anthropics', 'default', 'magpie/claude', 'openrouter/anthropic/claude']) {
    const {unmount} = render(<LocalModelCredit providerId={localModelProvider(model)} />);
    expect(screen.queryByText('(Use Credit)')).not.toBeInTheDocument();
    unmount();
  }
  render(<LocalModelCredit providerId={localModelProvider('anthropic/any-model')} />);
  expect(screen.getByText('(Use Credit)')).toHaveClass('local-model-credit');
});
