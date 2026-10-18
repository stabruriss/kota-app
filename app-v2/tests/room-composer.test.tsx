import { createRef } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { RoomComposer } from '../src/chrome/RoomComposer';
import { escapePromptPath, type InputBarHandle } from '../src/chrome/InputBar';
import type { RoomQuoteReference } from '../src/lib/room-quote';
afterEach(() => vi.restoreAllMocks());

describe('RoomComposer', () => {
  it.each([true, false, 'reject'] as const)('locks Dance draft mutations until the pending send ends (%s), even after exiting Dance', async outcome => {
    const ref = createRef<InputBarHandle>();
    let finish!: (value: boolean) => void; let fail!: (error: Error) => void;
    const onSend = vi.fn(() => new Promise<boolean>((resolve, reject) => { finish = resolve; fail = reject; }));
    const changed = vi.fn(); vi.spyOn(console, 'error').mockImplementation(() => {});
    const view = render(<RoomComposer ref={ref} danceMode targetAgent="alice" onSend={onSend} onDraftChange={changed} />);
    const field = screen.getByTestId('input-field');
    await userEvent.type(field, 'Original draft');
    act(() => ref.current?.insertAttachment({ path: '/tmp/original.txt', name: 'original.txt', kind: 'file' }));
    const original = ref.current!.serialize().payload; const revisions = changed.mock.calls.length;
    await userEvent.click(screen.getByTestId('ib-send'));
    expect(field).toHaveAttribute('contenteditable', 'false'); expect(field).toHaveAttribute('aria-readonly', 'true');
    await userEvent.type(field, 'Must not enter');
    act(() => { ref.current?.insertAttachment({ path: '/tmp/late.txt', name: 'late.txt', kind: 'file' }); ref.current?.clear(); });
    expect(ref.current!.serialize().payload).toBe(original); expect(changed).toHaveBeenCalledTimes(revisions);
    view.rerender(<RoomComposer ref={ref} danceMode={false} targetAgent="alice" onSend={onSend} onDraftChange={changed} />);
    expect(field).toHaveAttribute('contenteditable', 'false');
    await act(async () => { if (outcome === 'reject') fail(new Error('fixture send failed')); else finish(outcome); });
    await waitFor(() => expect(field).toHaveAttribute('contenteditable', 'true'));
    expect(field).toHaveAttribute('aria-readonly', 'false');
    expect(ref.current!.serialize().payload).toBe(outcome === true ? '' : original);
    await userEvent.type(field, ' unlocked');
    expect(ref.current!.serialize().payload).toContain('unlocked');
  });
  it('gives App only the typed prose as Soul eligibility, never mention or attachment nodes', async () => {
    const ref = createRef<InputBarHandle>(); const onSend = vi.fn();
    render(<RoomComposer ref={ref} targetAgent="alice" onSend={onSend} />);
    const field = screen.getByTestId('input-field');
    await userEvent.type(field, 'Review this plan ');
    const mention = document.createElement('span');
    Object.assign(mention.dataset, { ibMention: 'true', agentId: 'bob', aka: 'Bob' });
    mention.textContent = '@Bob';
    act(() => { field.appendChild(mention); ref.current?.insertAttachment({ path: '/tmp/plan.txt', name: 'plan.txt', kind: 'file' }); });
    await userEvent.click(screen.getByTestId('ib-send'));
    expect(onSend).toHaveBeenCalledWith('alice', expect.stringContaining('@Bob'), expect.objectContaining({ eligibilityText: 'Review this plan' }));
  });
  it('keeps the ordinary composer editable during its pending send', async () => {
    let finish!: (value: boolean) => void;
    render(<RoomComposer targetAgent="alice" onSend={() => new Promise<boolean>(resolve => { finish = resolve; })} />);
    const field = screen.getByTestId('input-field'); await userEvent.type(field, 'Ordinary');
    await userEvent.click(screen.getByTestId('ib-send'));
    expect(field).toHaveAttribute('contenteditable', 'true'); expect(field).toHaveAttribute('aria-readonly', 'false');
    await userEvent.type(field, ' retained');
    await act(async () => finish(false));
    expect(field).toHaveTextContent('Ordinary retained');
  });
  it('sends Dance clean text and attachment metadata separately, including an attachment-only message', async () => {
    const ref = createRef<InputBarHandle>(); const onSend = vi.fn(async () => false);
    render(<RoomComposer ref={ref} danceMode targetAgent={null} onSend={onSend} />);
    act(() => ref.current?.insertAttachment({ path: '/tmp/attachment name.png', name: 'attachment name.png', kind: 'image' }));
    expect(screen.getByTestId('ib-target-pill')).toHaveTextContent('AUTO');
    await userEvent.click(screen.getByTestId('ib-send'));
    expect(onSend).toHaveBeenCalledWith(null, escapePromptPath('/tmp/attachment name.png'), {
      broadcast: false, privacy: false, eligibilityText: '', dance: { text: '', attachments: [{ name: 'attachment name.png', path: '/tmp/attachment name.png' }] },
      onAccepted: expect.any(Function),
    });
    expect(screen.getByTestId('ib-attachment-chip')).toBeInTheDocument();
  });
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
        eligibilityText: 'first\nsecond',
        onAccepted: expect.any(Function),
      });
      expect(field).toHaveTextContent('');
    });
  });

  it.each([true, false] as const)('clears the draft on onAccepted while delivery is still pending, without clearing again or refilling on %s', async outcome => {
    let finish!: (value: boolean) => void;
    const changed = vi.fn();
    const onSend = vi.fn((_target, _payload, options?: { onAccepted?: () => void }) => {
      options?.onAccepted?.();
      return new Promise<boolean>((resolve) => {
        finish = resolve;
      });
    });
    render(<RoomComposer targetAgent="alice" onSend={onSend} onDraftChange={changed} />);
    const field = screen.getByTestId('input-field');
    await userEvent.type(field, 'Clear immediately');
    changed.mockClear();

    await userEvent.click(screen.getByTestId('ib-send'));
    expect(field).toHaveTextContent('');
    expect(screen.getByTestId('ib-send')).toBeDisabled();
    expect(changed).toHaveBeenCalledTimes(1);

    if (outcome) {
      await userEvent.type(field, 'Next draft');
      expect(field).toHaveTextContent('Next draft');
    }
    const changesBeforeFinish = changed.mock.calls.length;

    await act(async () => finish(outcome));
    expect(field).toHaveTextContent(outcome ? 'Next draft' : '');
    expect(changed).toHaveBeenCalledTimes(changesBeforeFinish);
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
