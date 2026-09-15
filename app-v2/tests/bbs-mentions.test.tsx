import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { bbsCurrentMentionNames, bbsMentionCounts, bbsMentionSelection, bbsMentionTargets } from '../src/bbs-mentions';
import { BbsMentionComposer, type BbsMentionComposerProps } from '../src/chrome/BbsMentionComposer';
import type { BbsMentionSelection, BbsRosterAgent, BbsRosterDevice } from '../src/types/bbs-roster';

const agent = (agentId: string, name = agentId): BbsRosterAgent => ({ agentId, name, targetRef: `local/kota/${agentId}`, avatar: { kind: 'none' } });
const devices: BbsRosterDevice[] = [
  { deviceId: null, name: 'MacBook Pro', local: true, online: true, rosterStatus: 'synced', receivedAt: null, projects: [
    { projectId: 'notes', name: 'Notes', agents: [agent('same', '晴雯')] },
    { projectId: 'kota', name: 'Kota', agents: [agent('same', '晴雯'), agent('pin', '颦儿')] },
  ] },
  { deviceId: 'peer', name: 'MacBook Air', local: false, online: false, rosterStatus: 'synced', receivedAt: '2026-09-13T00:00:00Z', projects: [
    { projectId: 'kota', name: 'Kota', agents: [agent('same', '晴雯')] },
  ] },
  { deviceId: 'unknown', name: 'Travel Mac', local: false, online: false, rosterStatus: 'not_synced', receivedAt: null, projects: null },
];
function selection(d = 0, p = 1, a = 0) { const device = devices[d], project = device.projects![p]; return bbsMentionSelection(device, project, project.agents[a]); }
function Harness(props: Partial<BbsMentionComposerProps> & { initial?: BbsMentionSelection[] }) {
  const [selected, setSelected] = useState(props.initial ?? []);
  const [open, setOpen] = useState(false);
  return <BbsMentionComposer devices={devices} currentProjectId="kota" selected={selected} onChange={setSelected}
    open={open} onOpenChange={setOpen} remoteAllowed submit={<button>Reply</button>} {...props}>
    <textarea aria-label="Reply text" defaultValue="Keep this draft" />
  </BbsMentionComposer>;
}
const trigger = () => screen.getByRole('button', { name: /^@ Agent/ });

describe('BBS mention selection identities', () => {
  it('routes three same-named targets independently and counts project identities, not names', () => {
    const values = [selection(), selection(0, 0), selection(1, 0), selection()];
    expect(bbsMentionCounts(values)).toEqual({ agents: 3, projects: 3, devices: 2 });
    expect(bbsMentionTargets(values)).toEqual([
      { deviceId: 'local', projectId: 'kota', agentId: 'same' },
      { deviceId: 'local', projectId: 'notes', agentId: 'same' },
      { deviceId: 'peer', projectId: 'kota', agentId: 'same' },
    ]);
  });
  it('renames display values without changing IDs and retains missing target selections', () => {
    const selected = [selection(), selection(1, 0)];
    const renamed = structuredClone(devices);
    renamed[0].name = 'New Mac'; renamed[0].projects![1].name = 'Renamed';
    renamed[0].projects![1].agents = [agent('same', 'New Name')];
    const current = bbsCurrentMentionNames(selected, renamed.slice(0, 1));
    expect(current[0]).toEqual({ ...selected[0], deviceName: 'New Mac', projectName: 'Renamed', agentName: 'New Name' });
    expect(current[1]).toEqual(selected[1]);
    expect(bbsMentionTargets(current)).toEqual(bbsMentionTargets(selected));
  });
});

describe('BBS layout B mention composer', () => {
  it('starts unselected, mounts the picker only when opened and defaults to current project', async () => {
    const avatar = vi.fn((item: BbsRosterAgent) => <span>{item.agentId}</span>);
    render(<Harness renderAvatar={avatar} />);
    expect(avatar).not.toHaveBeenCalled();
    expect(screen.queryByRole('region', { name: 'Mention agents' })).not.toBeInTheDocument();
    expect(screen.queryByRole('searchbox')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    await userEvent.click(trigger());
    const panel = screen.getByRole('region', { name: 'Mention agents' });
    const projects = within(panel).getAllByRole('region');
    expect(projects.map(item => item.getAttribute('aria-label'))).toEqual(['MacBook Pro / Kota', 'MacBook Pro / Notes']);
    expect(within(panel).getByText('0 agents · 0 projects · 0 devices')).toBeInTheDocument();
    expect(within(panel).getByRole('button', { name: 'Travel Mac' })).toBeDisabled();
    expect(within(panel).queryByText(/尚未同步|not synced/i)).not.toBeInTheDocument();
  });
  it('selects across scopes, keeps full summary on close, and removes just one homonym', async () => {
    render(<Harness />);
    const editor = screen.getByRole('textbox');
    await userEvent.click(trigger());
    await userEvent.click(screen.getByRole('checkbox', { name: '晴雯, MacBook Pro, Kota' }));
    await userEvent.click(screen.getByRole('checkbox', { name: '晴雯, MacBook Pro, Notes' }));
    await userEvent.click(within(screen.getByRole('navigation', { name: 'Devices' })).getByRole('button', { name: /MacBook Air/ }));
    const remote = screen.getByRole('checkbox', { name: '晴雯, MacBook Air, Kota' });
    expect(remote).toBeEnabled();
    remote.focus(); await userEvent.keyboard(' ');
    expect(remote).toBeChecked(); expect(remote).toHaveFocus();
    expect(screen.getByText('3 agents · 3 projects · 2 devices')).toBeInTheDocument();
    expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Done' }));
    expect(trigger()).toHaveFocus();
    const summary = screen.getByLabelText('Selected recipients');
    expect(within(summary).getAllByRole('button')).toHaveLength(3);
    await userEvent.click(within(summary).getByRole('button', { name: 'Remove 晴雯, MacBook Air, Kota' }));
    expect(within(summary).getAllByRole('button')).toHaveLength(2);
    expect(trigger()).toHaveTextContent('@ Agent · 2');
    await userEvent.click(trigger());
    expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    expect(screen.getByRole('checkbox', { name: '晴雯, MacBook Air, Kota' })).not.toBeChecked();
    expect(screen.getByRole('textbox')).toBe(editor); expect(editor).toHaveValue('Keep this draft');
  });
  it('Escape closes only the picker, ignores composing Escape, and outside clicks preserve selection', async () => {
    render(<Harness initial={[selection()]} />);
    const parentEscape = vi.fn();
    document.addEventListener('keydown', parentEscape);
    try {
      await userEvent.click(trigger());
      expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
      fireEvent.keyDown(document, { key: 'Escape', isComposing: true });
      expect(trigger()).toHaveAttribute('aria-expanded', 'true');
      parentEscape.mockClear(); await userEvent.keyboard('{Escape}');
      expect(parentEscape).not.toHaveBeenCalled(); expect(trigger()).toHaveFocus();
      expect(trigger()).toHaveAttribute('aria-expanded', 'false');
      expect(screen.getByLabelText('Selected recipients')).toBeInTheDocument();
      await userEvent.click(trigger());
      expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
      await userEvent.click(screen.getByRole('textbox'));
      expect(trigger()).toHaveAttribute('aria-expanded', 'false');
      expect(trigger()).toHaveTextContent('@ Agent · 1');
      expect(screen.getByRole('button', { name: 'Remove 晴雯, MacBook Pro, Kota' })).toBeInTheDocument();
    } finally { document.removeEventListener('keydown', parentEscape); }
  });
  it('private replies allow other local projects, prohibit remote selection, and keep stale selected names', async () => {
    render(<Harness remoteAllowed={false} initial={[selection(1, 0)]} />);
    await userEvent.click(trigger());
    expect(screen.getByRole('checkbox', { name: '晴雯, MacBook Pro, Notes' })).toBeEnabled();
    await userEvent.click(within(screen.getByRole('navigation', { name: 'Devices' })).getByRole('button', { name: /MacBook Air/ }));
    expect(screen.getByRole('checkbox', { name: '晴雯, MacBook Air, Kota' })).toBeDisabled();
    expect(screen.getByText('This thread is not shared. Start a new thread to @ this agent.')).toBeInTheDocument();
    // No silent target removal: final publication remains the backend's all-or-none check.
    expect(trigger()).toHaveTextContent('@ Agent · 1');
  });
  it('disables all selection mutations in flight while preserving draft and allowing Done', async () => {
    const change = vi.fn();
    const { rerender } = render(<Harness initial={[selection()]} disabled open onChange={change} />);
    expect(trigger()).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: '晴雯, MacBook Pro, Kota' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Clear' })).toBeDisabled();
    expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Done' })).toBeEnabled();
    rerender(<Harness initial={[selection()]} disabled open={false} onChange={change} />);
    expect(screen.getByRole('button', { name: 'Remove 晴雯, MacBook Pro, Kota' })).toBeDisabled();
    expect(change).not.toHaveBeenCalled(); expect(screen.getByRole('textbox')).toHaveValue('Keep this draft');
  });
  it('has no total roster cutoff: later projects and the 65th agent remain selectable', async () => {
    const many = structuredClone(devices.slice(0, 1));
    many[0].projects = Array.from({ length: 10 }, (_, p) => ({ projectId: `project-${p}`, name: `Project ${p}`,
      agents: Array.from({ length: 65 }, (_, a) => agent(`agent-${a}`, `Agent ${a}`)) }));
    render(<Harness devices={many} currentProjectId="project-0" />);
    await userEvent.click(trigger());
    expect(screen.queryByRole('region', { name: 'MacBook Pro / Project 8' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Show more projects (2)' }));
    const project = screen.getByRole('region', { name: 'MacBook Pro / Project 9' });
    expect(within(project).queryByRole('checkbox', { name: 'Agent 64, MacBook Pro, Project 9' })).not.toBeInTheDocument();
    await userEvent.click(within(project).getByRole('button', { name: 'Show more agents (1)' }));
    await userEvent.click(within(project).getByRole('checkbox', { name: 'Agent 64, MacBook Pro, Project 9' }));
    expect(trigger()).toHaveTextContent('@ Agent · 1');
    expect(screen.queryByLabelText('Selected recipients')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Done' }));
    expect(screen.getByRole('button', { name: 'Remove Agent 64, MacBook Pro, Project 9' })).toBeInTheDocument();
  });
  it('shows loading/retry without inventing an empty roster and supports clearing selection', async () => {
    const retry = vi.fn();
    const { rerender } = render(<Harness devices={[]} loading error="Could not load agent list." onRetry={retry} />);
    await userEvent.click(trigger());
    expect(screen.getByText('Loading agents…')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled();
    expect(screen.queryByText('No active agents.')).not.toBeInTheDocument();
    rerender(<Harness devices={[]} error="Could not load agent list." onRetry={retry} />);
    await userEvent.click(screen.getByRole('button', { name: 'Retry' })); expect(retry).toHaveBeenCalledOnce();
    rerender(<Harness initial={[selection()]} devices={devices} />);
    await userEvent.click(screen.getByRole('checkbox', { name: '晴雯, MacBook Pro, Kota' }));
    await userEvent.click(screen.getByRole('button', { name: 'Clear' }));
    expect(trigger()).toHaveTextContent('@ Agent'); expect(trigger()).not.toHaveTextContent('·');
    expect(screen.getByRole('button', { name: 'Clear' })).toBeDisabled();
  });
});
