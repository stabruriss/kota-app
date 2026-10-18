import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { SoulDismissErrorDialog } from '../src/chrome/SoulDismissErrorDialog';

it('focuses Cancel, traps Tab and maps Escape to cancellation', async () => {
  const onChoose = vi.fn();
  render(<SoulDismissErrorDialog name="Rowan" onChoose={onChoose} />);
  const dialog = screen.getByRole('alertdialog', { name: 'Delete archived agent?' });
  const q = within(dialog);
  expect(q.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  await userEvent.tab({ shift: true });
  expect(q.getByRole('button', { name: 'Retry' })).toHaveFocus();
  await userEvent.tab();
  expect(q.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  await userEvent.tab();
  expect(q.getByRole('button', { name: 'Dismiss anyway' })).toHaveFocus();
  await userEvent.tab();
  expect(q.getByRole('button', { name: 'Retry' })).toHaveFocus();
  await userEvent.tab();
  expect(q.getByRole('button', { name: 'Cancel' })).toHaveFocus();
  await userEvent.keyboard('{Escape}');
  expect(onChoose).toHaveBeenCalledExactlyOnceWith('cancel');
});

it('background cancels, inside clicks do not, and one prompt answers once', async () => {
  const onChoose = vi.fn();
  render(<SoulDismissErrorDialog name="Rowan" onChoose={onChoose} />);
  const dialog = screen.getByRole('alertdialog');
  fireEvent.mouseDown(dialog);
  expect(onChoose).not.toHaveBeenCalled();
  fireEvent.mouseDown(dialog.parentElement!);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Dismiss anyway' }));
  expect(onChoose).toHaveBeenCalledExactlyOnceWith('cancel');
});
