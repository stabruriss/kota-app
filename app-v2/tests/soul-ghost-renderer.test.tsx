import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { VioletRoomPanel } from '../src/chrome/VioletRoomPanel';
import * as client from '../src/pty-client';
import fixture from './fixtures/soul-ghost-renderer.json';

afterEach(() => vi.restoreAllMocks());

// Render-only fixture supplied by the backend author. This deliberately does
// not test native receipt association, which belongs to the projection tests.
it('folds both memory sections and the repeated-push hint into the existing Ghost Sasayaki bubble', async () => {
  const message = {
    id: 'soul-render-fixture', sessionId: 'fixture-session', agentId: 'agent-fixture',
    shell: 'kota-local', role: 'user' as const, kind: 'message' as const,
    timestamp: '2026-10-15T12:00:00Z', ...fixture,
  };
  vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue({
    messages: [message], sources: [], workEvents: [], agentBusReceipts: [],
    rawLogDir: '/tmp/soul-render-fixture/raw_logs',
    chathistoryDir: '/tmp/soul-render-fixture/chathistory', syncedAt: message.timestamp,
  });
  const view = render(<VioletRoomPanel projectRoot="/tmp/soul-render-fixture" agentIds={['agent-fixture']} />);
  try {
    const summary = (await screen.findByText('Ghost Sasayaki')).closest('summary')!;
    const details = summary.closest('details')!;
    expect(details).not.toHaveAttribute('open');
    const marks = screen.getByText(/Your own marks that may apply/);
    const dreams = screen.getByText(/What Kota has learned about the user/);
    expect(marks).not.toBeVisible(); expect(dreams).not.toBeVisible();
    await userEvent.click(summary);
    expect(details).toHaveAttribute('open');
    expect(marks).toBeVisible(); expect(dreams).toBeVisible();
    expect(details).toHaveTextContent('Keep synthetic receipts.');
    expect(details).toHaveTextContent('The fixture user reads the result first.');
    expect(details).toHaveTextContent('↑ Pushed to you 3 times. If this is a lasting lesson for you, add-mark it; then run clear-heat d-new either way.');
    expect(details).not.toHaveTextContent(/KOTA_MARKS|revision=|probabilit|Certainty/);
    expect(view.container.querySelectorAll('.ghost-sasayaki')).toHaveLength(1);
    await userEvent.click(summary);
    expect(details).not.toHaveAttribute('open'); expect(marks).not.toBeVisible();
  } finally { view.unmount(); }
});

it('shows the host context row for marks delivered with a message in the same bubble', async () => {
  const message = { id: 'violet-soul-fixture', sessionId: 'soul', agentId: 'agent-fixture', shell: 'system',
    role: 'system', kind: 'context', timestamp: '2026-10-15T12:00:00Z', text: fixture.text, targetAgentIds: ['agent-fixture'] };
  // Its own room: the panel caches each project's rows across mounts.
  vi.spyOn(client, 'readVioletRoomCache').mockResolvedValue({
    messages: [message], sources: [], workEvents: [], agentBusReceipts: [],
    rawLogDir: '/tmp/soul-context-row-fixture/raw_logs',
    chathistoryDir: '/tmp/soul-context-row-fixture/chathistory', syncedAt: message.timestamp,
  });
  const view = render(<VioletRoomPanel projectRoot="/tmp/soul-context-row-fixture" agentIds={['agent-fixture']} />);
  try {
    const details = (await screen.findByText('Ghost Sasayaki')).closest('details')!;
    expect(details).toHaveTextContent('Keep synthetic receipts.');
    expect(view.container.querySelectorAll('.ghost-sasayaki')).toHaveLength(1);
    expect(view.container.querySelector('[data-violet-message-id="violet-soul-fixture"]')).toHaveClass('ghost-sasayaki');
  } finally { view.unmount(); }
});
