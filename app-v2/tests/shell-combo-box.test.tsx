import { useMemo, useState } from 'react';
import { render, screen, within } from '@testing-library/react';
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
  it('without a footer preserves option rendering, keyboard highlight, selection and ARIA references', async () => {
    const onChange = vi.fn();
    render(<Harness onChange={onChange} />);
    const input = screen.getByRole('combobox');
    expect(input).toHaveAttribute('aria-expanded', 'false');
    expect(input).not.toHaveAttribute('aria-controls');
    await userEvent.click(input);
    const listbox = screen.getByRole('listbox');
    const options = within(listbox).getAllByRole('option');
    expect(options.map((option) => option.title)).toEqual(PROVIDER_OPTIONS.map((option) => option.id));
    for (const label of ['Provider', 'Models.dev', 'Kota']) expect(within(listbox).getByText(label)).toBeVisible();
    expect(input).toHaveAttribute('aria-controls', listbox.id);
    expect(input).toHaveAttribute('aria-expanded', 'true');
    expect(screen.queryByRole('button', { name: 'About auto route' })).not.toBeInTheDocument();
    const highlighted = (selected: number) => {
      options.forEach((option, index) => {
        expect(option).toHaveAttribute('aria-selected', String(index === selected));
        if (index === selected) expect(option).toHaveClass('active');
        else expect(option).not.toHaveClass('active');
      });
      expect(input).toHaveAttribute('aria-activedescendant', options[selected].id);
      expect(input).toHaveAttribute('aria-controls', listbox.id);
    };
    highlighted(0);
    await userEvent.keyboard('{ArrowDown}');
    highlighted(1);
    await userEvent.keyboard('{ArrowDown}');
    highlighted(2);
    await userEvent.keyboard('{ArrowUp}');
    highlighted(1);
    await userEvent.keyboard('{Enter}');
    expect(onChange).toHaveBeenLastCalledWith('models-dev');
    expect(input).toHaveValue('models-dev');
    expect(input).toHaveAttribute('aria-expanded', 'false');
    expect(input).not.toHaveAttribute('aria-controls');
    expect(input).not.toHaveAttribute('aria-activedescendant');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    await userEvent.keyboard('{ArrowDown}');
    expect(input).toHaveAttribute('aria-controls', screen.getByRole('listbox').id);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(input).toHaveValue('models-dev');
    expect(onChange).toHaveBeenCalledTimes(1);
  });

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

describe('ShellComboBox fixed choices', () => {
  it('selectOnly always lists every option, opens on the current one and cannot be typed over', async () => {
    const levels: ShellComboOption[] = ['low', 'medium', 'high', 'xhigh'].map((id) => ({ id, label: id, source: 'provider' }));
    function Choice({ onChange }: { onChange: (value: string) => void }) {
      const [value, setValue] = useState('medium');
      return <ShellComboBox value={value} options={levels} placeholder="" selectOnly onChange={(next) => { setValue(next); onChange(next); }} />;
    }
    const onChange = vi.fn();
    render(<Choice onChange={onChange} />);
    const input = screen.getByRole('combobox');
    expect(input).toHaveAttribute('readonly');
    expect(input).toHaveAttribute('aria-autocomplete', 'none');
    await userEvent.click(input);
    const options = within(screen.getByRole('listbox')).getAllByRole('option');
    // The current value does not filter the menu down to itself.
    expect(options.map((option) => option.title)).toEqual(['low', 'medium', 'high', 'xhigh']);
    expect(options[1]).toHaveAttribute('aria-selected', 'true');
    await userEvent.keyboard('{Backspace}{Backspace}x');
    expect(input).toHaveValue('medium');
    expect(onChange).not.toHaveBeenCalled();
    await userEvent.keyboard('{ArrowDown}{ArrowDown}{Enter}');
    expect(onChange).toHaveBeenLastCalledWith('xhigh');
    expect(input).toHaveValue('xhigh');
    await userEvent.click(input);
    expect(within(screen.getByRole('listbox')).getAllByRole('option')).toHaveLength(4);
  });
});
