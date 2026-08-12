import { createRef } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { RoomComposer } from '../src/chrome/RoomComposer';
import { escapePromptPath, type InputBarHandle } from '../src/chrome/InputBar';
import type { RoomQuoteReference } from '../src/lib/room-quote';

describe('RoomComposer', () => {
  it('keeps draft updates inside the leaf and sends exactly what it displays', async () => {
    const ref = createRef<InputBarHandle>();
    const onSend = vi.fn();
    let hostRenderCount = 0;

    function Host() {
      hostRenderCount += 1;
      return <RoomComposer ref={ref} targetAgent="alice" onSend={onSend} />;
    }

    render(<Host />);
    const field = screen.getByTestId('input-field');
    await userEvent.type(field, 'first{enter}second');

    expect(hostRenderCount).toBe(1);
    expect(ref.current?.serialize().payload).toBe('first\nsecond');

    await userEvent.click(screen.getByTestId('ib-send'));
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith('alice', 'first\nsecond', {
        broadcast: false,
        privacy: false,
      });
      expect(field).toHaveTextContent('');
    });
  });

  it('forwards attachment and clear operations through the existing composer ref', () => {
    const ref = createRef<InputBarHandle>();
    render(<RoomComposer ref={ref} targetAgent="alice" />);

    act(() => {
      ref.current?.insertAttachment({
        path: '/tmp/kota-test/file name.txt',
        name: 'file name.txt',
        kind: 'file',
      });
    });

    expect(screen.getByTestId('ib-attachment-chip')).toHaveTextContent('file name.txt');
    expect(ref.current?.serialize().payload).toBe(escapePromptPath('/tmp/kota-test/file name.txt'));

    act(() => ref.current?.clear());
    expect(screen.queryByTestId('ib-attachment-chip')).not.toBeInTheDocument();
    expect(ref.current?.serialize().payload).toBe('');
  });

  it('keeps the existing body draft while project-scoped quotes are discarded on root change', async () => {
    const ref = createRef<InputBarHandle>();
    const quote: RoomQuoteReference = {
      ref: 'quote-one',
      project: 'one',
      projectRoot: '/tmp/project-one',
      from: { id: 'alice', name: 'Alice' },
      to: [{ id: 'user', name: 'User' }],
      at: '2026-08-02T12:00:00Z',
      excerpt: 'quoted text',
      truncated: false,
    };
    const { rerender } = render(
      <RoomComposer ref={ref} targetAgent="alice" quoteProjectRoot="/tmp/project-one" />,
    );

    await userEvent.type(screen.getByTestId('input-field'), 'keep this draft');
    let insertResult;
    act(() => {
      insertResult = ref.current?.insertQuote(quote);
    });
    expect(insertResult).toBe('inserted');
    expect(screen.getByTestId('ib-quote-chip')).toBeInTheDocument();

    rerender(
      <RoomComposer ref={ref} targetAgent="alice" quoteProjectRoot="/tmp/project-two" />,
    );

    await waitFor(() => expect(screen.queryByTestId('ib-quote-chip')).not.toBeInTheDocument());
    expect(screen.getByTestId('input-field')).toHaveTextContent('keep this draft');
    expect(ref.current?.serialize().payload).toBe('keep this draft');
  });
});
