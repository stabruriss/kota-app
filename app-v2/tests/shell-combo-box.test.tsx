import { useMemo, useState } from 'react';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import {
  ShellComboBox,
  uniqueShellComboOptions,
  type ShellComboOption,
} from '../src/chrome/ShellComboBox';

const PROVIDER_OPTIONS: ShellComboOption[] = [
  { id: 'provider-model', label: 'Provider model', source: 'provider' },
  { id: 'models-dev', label: 'Models.dev model', source: 'models.dev' },
  { id: 'kota-seed', label: 'Kota seed', source: 'seed' },
];

function Harness({ onChange = vi.fn() }: { onChange?: (value: string) => void }) {
  const [value, setValue] = useState('');
  const options = useMemo(() => uniqueShellComboOptions([
    ...PROVIDER_OPTIONS,
    value.trim() ? { id: value.trim(), label: value.trim(), source: 'current' } : null,
  ]), [value]);
  return (
    <ShellComboBox
      value={value}
      options={options}
      placeholder="Exact model ID"
      onChange={(next) => {
        setValue(next);
        onChange(next);
      }}
    />
  );
}

describe('ShellComboBox custom values', () => {
  it('shows catalog sources and labels an unmatched current value as Custom', async () => {
    render(<Harness />);
    const input = screen.getByRole('combobox');

    await userEvent.click(input);
    expect(screen.getByText('Provider')).toBeVisible();
    expect(screen.getByText('Models.dev')).toBeVisible();
    expect(screen.getByText('Kota')).toBeVisible();

    await userEvent.type(input, 'private-model');
    expect(screen.getByRole('option', { name: /private-model Custom/i })).toBeVisible();
  });

  it('keeps an exact custom value when its synthetic option is chosen', async () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = screen.getByRole('combobox');

    await userEvent.type(input, 'private-model');
    await userEvent.keyboard('{Enter}');

    expect(input).toHaveValue('private-model');
    expect(onChange).toHaveBeenLastCalledWith('private-model');
    expect(screen.queryByRole('listbox')).toBeNull();
  });

  it('prefers a catalog row over a duplicate current-value row', () => {
    expect(uniqueShellComboOptions([
      { id: 'provider-model', label: 'Provider Model', source: 'provider' },
      { id: 'provider-model', label: 'provider-model', source: 'current' },
    ])).toEqual([
      { id: 'provider-model', label: 'Provider Model', source: 'provider' },
    ]);
  });
});
