import { describe, expect, it } from 'vitest';
import { mergeLocalToolImages, violetMessageMatchesAgentFilter } from '../src/chrome/VioletRoomPanel';
import { filterAgentBusMessages } from '../src/lib/violet-message-filter';
import type { VioletChatMessage } from '../src/pty-client';

type Row = VioletChatMessage & { toolImagePaths?: readonly string[] };

const LOCAL = 'agent-local';
const OTHER = 'agent-other';
const REVIEWER = 'agent-reviewer';
const DIR = 'project-memory/attachments/violet/kota-local';

const row = (fields: Partial<Row> & Pick<Row, 'id' | 'agentId'>): Row => ({
  sessionId: 'session', shell: 'kota-local', role: 'assistant', kind: 'message',
  timestamp: '2026-11-14T09:39:00Z', text: 'Reply', ...fields,
});
const images = (id: string, ...names: string[]): Row => row({
  id, agentId: LOCAL, kind: 'artifact',
  text: `${names.length === 1 ? '1 image' : `${names.length} images`}\n\n${names.map((name) => `${DIR}/${name}`).join('\n\n')}`,
});
const reply = (id: string, agentId = LOCAL): Row => row({ id, agentId });
const other = (id: string): Row => row({ id, agentId: OTHER, shell: 'claude', text: 'Another agent' });
const bus = (id: string, from: string, to: string): Row => row({
  id, agentId: from, shell: 'system', nativeEventId: `agentbus-${from}-${to}-${id}`, targetAgentIds: [to], text: 'Bus note',
});
const paths = (...names: string[]) => names.map((name) => `${DIR}/${name}`);
const summary = (messages: readonly Row[]) => messages.map((message) => [message.id, message.toolImagePaths ?? []]);

describe('mergeLocalToolImages', () => {
  it('folds an agent\'s image rows into its next reply past other messages, in order', () => {
    const rows = [images('i1', 'a.jpg'), other('o1'), images('i2', 'b.jpg', 'c.png'), bus('b1', REVIEWER, LOCAL),
      images('i3', 'd.png'), reply('r1')];
    expect(summary(mergeLocalToolImages(rows))).toEqual([
      ['o1', []], ['b1', []], ['r1', paths('a.jpg', 'b.jpg', 'c.png', 'd.png')],
    ]);
  });

  it('never hosts images on an Agent Bus message the agent sends before its reply', () => {
    const rows = [images('i1', 'a.jpg'), bus('b1', LOCAL, REVIEWER), reply('r1')];
    expect(summary(mergeLocalToolImages(rows))).toEqual([['b1', []], ['r1', paths('a.jpg')]]);
    expect(summary(mergeLocalToolImages(filterAgentBusMessages(rows, false)))).toEqual([['r1', paths('a.jpg')]]);
  });

  it('keeps image rows standalone until a reply arrives', () => {
    const rows = [images('i1', 'a.jpg'), other('o1'), images('i2', 'b.jpg')];
    expect(mergeLocalToolImages(rows)).toBe(rows);
  });

  it('starts a new fold after each reply and leaves other agents\' replies alone', () => {
    const rows = [images('i1', 'a.jpg'), reply('o1', OTHER), reply('r1'), images('i2', 'b.jpg'), reply('r2')];
    expect(summary(mergeLocalToolImages(rows))).toEqual([
      ['o1', []], ['r1', paths('a.jpg')], ['r2', paths('b.jpg')],
    ]);
  });

  it('shows the same host under all four room filter combinations', () => {
    const rows = [images('i1', 'a.jpg'), other('o1'), images('i2', 'b.jpg'), bus('b1', REVIEWER, LOCAL),
      images('i3', 'c.png'), reply('r1')];
    const hosts = [true, false].flatMap((selectedAgent) => [true, false].map((showAgentToAgent) => {
      const scoped = selectedAgent ? rows.filter((message) => violetMessageMatchesAgentFilter(message, new Set([LOCAL]))) : rows;
      const visible = mergeLocalToolImages(filterAgentBusMessages(scoped, showAgentToAgent));
      expect(visible.some((message) => message.kind === 'artifact')).toBe(false);
      return visible.find((message) => message.id === 'r1');
    }));
    expect(hosts[0]?.toolImagePaths).toEqual(paths('a.jpg', 'b.jpg', 'c.png'));
    for (const host of hosts) expect(host).toEqual(hosts[0]);
  });
});
