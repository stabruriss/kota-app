import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { LocalQueueEditor } from '../src/chrome/LocalQueueEditor';
import type { LocalQueueController, LocalQueueDraft } from '../src/hooks/useLocalQueue';
import * as client from '../src/pty-client';

const root = '/fixture/queue-paste';
const path = `${root}/project-memory/attachments/composer/fixture/screenshot.png`;
const scope = { projectRoot: root, agentId: 'agent-copper', sessionId: 'fixture-session', epoch: 1 };
const original = (): LocalQueueDraft => ({ instance: 'fixture-draft', scope,
  item: { kotaId: 'fixture#agent-copper', customType: 'kota.user', senderId: null,
    createdAt: '2026-10-04T12:00:00Z', state: 'accepted', summary: 'Compare this screenshot.', truncated: false },
  original: { text: 'Compare this screenshot.', privacy: false }, text: 'Compare this screenshot.',
  target: scope.agentId, editToken: 'fixture-token', valid: true, changed: false, busy: false });
const finish = vi.fn();
function Editor({ initial = original() }: { initial?: LocalQueueDraft }) {
  const [draft, setDraft] = useState(initial);
  const queue: LocalQueueController = {
    drafts: new Map(), addDrafts: new Map(), pending: new Map(), errors: new Map(), ownerKey: () => '', itemKey: () => '',
    add: () => {}, finishAdd: async () => {}, updateAddDraft: () => {}, removeAddDraft: () => {},
    invalidate: () => {}, cancel: async () => {}, removeDraft: () => {}, finish,
    updateDraft: (_root, _agent, change) => setDraft(old => ({ ...old, ...change })),
  };
  return <LocalQueueEditor draft={draft} queue={queue}
    agents={{ [scope.agentId]: { name: 'Copper', emoji: '', role: '', hue: '' } }} targets={[scope.agentId]} />;
}
function pasteImage() {
  const file = new File(['synthetic pixels'], 'screenshot.png', { type: 'image/png' });
  return { file, prevented: !fireEvent.paste(screen.getByTestId('input-field'), {
    clipboardData: { files: [file], getData: (type: string) => type === 'text/html' ? '<img src="data:image/png;base64,cGl4ZWxz">' : '' },
  }) };
}
beforeEach(() => {
  vi.restoreAllMocks(); finish.mockReset();
  vi.spyOn(client, 'saveComposerClipboardImage').mockResolvedValue(path);
});

it('saves a Local screenshot in the draft project, inserts the normal pill and sends only its path', async () => {
  render(<Editor />);
  const { file, prevented } = pasteImage();
  expect(prevented).toBe(true);
  await waitFor(() => expect(client.saveComposerClipboardImage).toHaveBeenCalledWith(file, { projectRoot: root }));
  expect(await screen.findByTestId('ib-attachment-chip')).toHaveAttribute('data-path', path);
  expect(screen.getByTestId('input-field').innerHTML).not.toContain('base64');
  fireEvent.click(screen.getByRole('button', { name: 'Queue message' }));
  expect(finish).toHaveBeenCalledWith(root, scope.agentId, false, `Compare this screenshot. ${path}`, [], 'Compare this screenshot.');
});

it.each(['expired', 'busy'])('does not save or insert pasted pixels in a %s draft', (state) => {
  const draft = original();
  if (state === 'expired') draft.valid = false; else draft.busy = true;
  render(<Editor initial={draft} />);
  const before = screen.getByTestId('input-field').innerHTML;
  pasteImage();
  expect(client.saveComposerClipboardImage).not.toHaveBeenCalled();
  expect(screen.getByTestId('input-field').innerHTML).toBe(before);
  expect(screen.queryByTestId('ib-attachment-chip')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Queue/ })).toBeDisabled();
  expect(finish).not.toHaveBeenCalled();
});
