import { render, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { VioletRoomPanel } from '../src/chrome/VioletRoomPanel';
import * as client from '../src/pty-client';

function row(id: number, text: string, messageOrigin?: string): client.VioletChatMessage {
  return { id: `progress-${id}`, sessionId: 'soul', agentId: 'agent-piner', shell: 'codex',
    role: 'assistant', kind: 'commentary', timestamp: `2026-10-27T10:00:0${id}Z`, text, messageOrigin };
}
let fixtureId = 0;
function mount(messages: client.VioletChatMessage[]) {
  const projectRoot = `/tmp/kota-note-progress-fixture-${++fixtureId}`;
  vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue({ messages, sources: [], workEvents: [],
    rawLogDir: `${projectRoot}/project-memory/raw_logs`, chathistoryDir: `${projectRoot}/project-memory/chathistory`,
    syncedAt: '2026-10-27T10:01:00Z' });
  return render(<VioletRoomPanel projectRoot={projectRoot} agentIds={['agent-piner']} />);
}
afterEach(() => vi.restoreAllMocks());

it('preserves note and subagent marks through folded mixed progress and uses Markdown for the saved text', async () => {
  const view = mount([row(1, 'When editing `file.rs`, check its tests.', 'note'),
    row(2, 'Ordinary progress.'), row(3, 'Child report.', 'subagent'), row(4, 'Saved **lesson**.', 'note')]);
  await waitFor(() => expect(view.container.querySelectorAll('.violet-commentary-entry')).toHaveLength(4));
  const article = view.container.querySelector('article')!;
  const summary = article.querySelector('summary')!;
  expect(summary.querySelector('.violet-note-origin-summary')).toHaveAttribute('aria-label', 'Note added');
  expect(summary.querySelector('.violet-subagent-origin')).toBeNull();
  const entries = article.querySelectorAll('.violet-commentary-entry');
  expect(entries[0].querySelector('.violet-note-origin-entry')).not.toBeNull();
  expect(within(entries[0] as HTMLElement).getByText('file.rs').tagName).toBe('CODE');
  expect(entries[1].querySelector('.violet-note-origin, .violet-subagent-origin')).toBeNull();
  expect(entries[2].querySelector('.violet-subagent-origin-entry')).not.toBeNull();
  expect(entries[2].querySelector('.violet-note-origin')).toBeNull();
  expect(entries[3].querySelector('.violet-note-origin-entry')).not.toBeNull();
  expect(entries[3].querySelector('strong')).toHaveTextContent('lesson');
});

it('uses only the latest entry origin in the collapsed summary while retaining an earlier note', async () => {
  const view = mount([row(1, 'Saved lesson.', 'note'), row(2, 'Continuing work.')]);
  await waitFor(() => expect(view.container.querySelectorAll('.violet-commentary-entry')).toHaveLength(2));
  expect(view.container.querySelector('summary .violet-note-origin')).toBeNull();
  expect(view.container.querySelectorAll('.violet-note-origin-entry')).toHaveLength(1);
});

it('does not infer note origin from an ordinary progress text', async () => {
  const view = mount([row(1, 'Note: Added a note.')]);
  await waitFor(() => expect(view.container.querySelector('summary')).not.toBeNull());
  expect(view.container.querySelector('.violet-note-origin')).toBeNull();
});
