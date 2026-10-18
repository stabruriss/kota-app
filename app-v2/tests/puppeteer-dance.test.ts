import { describe, expect, it } from 'vitest';
import type { VioletChatMessage } from '../src/pty-client';
import { buildDanceRoster, withoutDanceNativeEchoes } from '../src/lib/puppeteer-dance';
const seats = [{id:'agent-a',name:'A'},{id:'agent-b',name:'B'},{id:'agent-c',name:'C'}];
const row = (id:string, patch:Partial<VioletChatMessage> = {}): VioletChatMessage => ({ id,sessionId:'s',agentId:'agent-a',shell:'codex',
  role:'user',kind:'message',timestamp:'2026-10-09T00:00:00Z',text:'same text',...patch });
const original = (id:string, at='2026-10-09T00:00:00Z') => row(id,{agentId:'user',sessionId:'composer',shell:'system',messageOrigin:'puppeteer',targetAgentIds:['agent-a'],timestamp:at});

describe('Dance bounded, already-loaded roster', () => {
  it('keeps empty seated agents and only two latest direct inputs plus one final reply', () => {
    const messages=[row('u1',{text:'one'}),row('u2',{text:'two',timestamp:'2026-10-09T00:01:00Z'}),
      row('u3',{text:'😀'.repeat(250),timestamp:'2026-10-09T00:02:00Z'}),
      row('final',{role:'assistant',text:'final answer',timestamp:'2026-10-09T00:03:00Z'}),
      row('progress',{role:'assistant',kind:'commentary',text:'not final',timestamp:'2026-10-09T00:04:00Z'}),
      row('tool',{role:'assistant',kind:'tool',text:'not reply'}),row('bus',{role:'assistant',shell:'system',actorIntent:'task',text:'not reply'}),
      {...row('private',{text:'not input'}),private:true},row('child',{role:'assistant',messageOrigin:'subagent',text:'not reply'}),
    ];
    const roster=buildDanceRoster(seats,messages);
    expect(roster[0]!.recentUserMessages.map(m=>m.text)).toEqual(['two','😀'.repeat(200)]);
    expect(roster[0]!.lastReply?.text).toBe('final answer');expect(roster[1]!.lastReply).toBeNull();expect(roster[2]!.recentUserMessages).toEqual([]);
  });
  it('includes clean LM and Puppeteer originals, follows actual recipients, and preserves Unicode names', () => {
    const roster=buildDanceRoster([{id:'agent-a',name:'😀'.repeat(70)},...seats.slice(1)], [
      original('dance'),row('lm',{agentId:'laughing-man',shell:'system',role:'assistant',actorIntent:'telegram',targetAgentIds:['agent-b'],text:'clean telegram'}),
      row('bbs',{agentId:'bbs',shell:'system',role:'assistant',actorIntent:'bbs-thread',targetAgentIds:['agent-b'],text:'not input'}),
    ]);
    expect(Array.from(roster[0]!.name)).toHaveLength(64);expect(roster[0]!.recentUserMessages[0]!.text).toBe('same text');
    expect(roster[1]!.recentUserMessages.map(m=>m.text)).toEqual(['clean telegram']);
  });
  it('counts Puppeteer originals once when native echoes are supplied', () => {
    const messages = [row('older', {text: 'earlier input'}),
      {...original('dance', '2026-10-09T00:01:00Z'), text: 'routed input'},
      row('echo', {shell: 'kota-local', nativeEventId: 'dance#agent-a', text: 'routed input', timestamp: '2026-10-09T00:02:00Z'})];
    expect(buildDanceRoster(seats, messages)[0]!.recentUserMessages.map(m => m.text)).toEqual(['earlier input', 'routed input']);
  });
  it('omits ordinary temporal-gap native echoes without losing their composer original', () => {
    const temporal = '<KOTA_TEMPORAL_GAP v="1" current_time="2026-10-09T00:01:00Z">\nIt has been over 24 hours since your last completed response in this room.\n</KOTA_TEMPORAL_GAP>\n';
    const messages = [row('older', {text: 'earlier input'}),
      row('composer', {agentId: 'user', shell: 'composer', sessionId: 'composer', targetAgentIds: ['agent-a'], text: 'ordinary input', timestamp: '2026-10-09T00:01:00Z'}),
      row('echo', {text: temporal + 'ordinary input', timestamp: '2026-10-09T00:02:00Z'})];
    expect(buildDanceRoster(seats, messages)[0]!.recentUserMessages.map(m => m.text)).toEqual(['earlier input', 'ordinary input']);
  });
  it('chooses by timestamp even if paged input is not sorted, and caps the roster at eight', () => {
    const roster=buildDanceRoster(Array.from({length:10},(_,i)=>({id:`agent-${i}`,name:String(i)})),[
      row('latest',{agentId:'agent-0',role:'assistant',timestamp:'2026-10-09T00:02:00Z',text:'latest'}),
      row('old',{agentId:'agent-0',role:'assistant',text:'old'}),
    ]);
    expect(roster).toHaveLength(8);expect(roster[0]!.lastReply?.text).toBe('latest');
  });
  it('carries room order separately from reply text for same-second backend continuation', () => {
    const result = buildDanceRoster(seats, [
      row('b', { agentId: 'agent-b', role: 'assistant', timestamp: '2026-10-09T00:01:00.789Z' }),
      row('a', { agentId: 'agent-a', role: 'assistant', timestamp: '2026-10-09T00:01:00Z' }),
      row('old', { agentId: 'agent-a', role: 'assistant', timestamp: '2026-10-09T00:00:00Z' }),
    ]);
    expect(result[0].lastReplyOrder).toBe(1); expect(result[1].lastReplyOrder).toBe(0);
    expect(result[2].lastReplyOrder).toBeUndefined();
    expect(result[0].lastReply).toEqual({ at: '2026-10-09T00:01:00Z', text: 'same text' });
  });
});
describe('Dance native echo index', () => {
  it.each(['codex', 'kota-local'])('still dedupes %s echoes with explicit user origin, not other origins', shell => {
    const messages = [original('m'),
      row('special', { shell, messageOrigin: 'subagent', nativeEventId: 'm#agent-a' }),
      row('tagged', { shell, messageOrigin: 'user', nativeEventId: 'm#agent-a' }),
      row('later', { shell, messageOrigin: 'user', nativeEventId: 'later#agent-a', timestamp: '2026-10-09T00:01:00Z' })];
    expect(withoutDanceNativeEchoes(messages).map(message => message.id)).toEqual(['m', 'special', 'later']);
  });
  it('uses exact Local messageId#targetAgentId without fuzzy text or an age limit', () => {
    const messages=[original('m'),row('wrong',{shell:'kota-local',nativeEventId:'other#agent-a'}),
      row('right',{shell:'kota-local',nativeEventId:'m#agent-a',text:'provider decoration',timestamp:'2026-10-10T00:00:00Z'})];
    expect(withoutDanceNativeEchoes(messages).map(m=>m.id)).toEqual(['m','wrong']);
  });
  it('does not consume a Local identity belonging to another agent', () => {
    const messages=[original('m'),row('foreign',{agentId:'agent-b',shell:'kota-local',nativeEventId:'m#agent-a'})];
    expect(withoutDanceNativeEchoes(messages)).toHaveLength(2);
  });
  it('pairs each terminal original with just the first later native user row, even hours later', () => {
    const messages=[row('before'),original('one','2026-10-09T00:01:00Z'),original('two','2026-10-09T00:02:00Z'),
      row('n1',{timestamp:'2026-10-09T06:00:00Z'}),row('n2',{timestamp:'2026-10-09T07:00:00Z'}),row('third',{timestamp:'2026-10-09T08:00:00Z'})];
    expect(withoutDanceNativeEchoes(messages).map(m=>m.id)).toEqual(['before','one','two','third']);
  });
  it('normalizes attachment decorations and temporal context without dropping another agent or a reply', () => {
    const temporal='<KOTA_TEMPORAL_GAP v="1" current_time="2026-10-09T00:00:00Z">\nIt has been over 24 hours since your last completed response in this room.\n</KOTA_TEMPORAL_GAP>\n';
    const messages=[{...original('one'),text:'check project-memory/attachments/composer/a.png'},
      row('other',{agentId:'agent-b',text:'check [Image #1]'}),row('reply',{role:'assistant',text:'check [Image #1]'}),
      row('echo',{text:temporal+'check [Image #1]',timestamp:'2026-10-10T00:00:00Z'})];
    expect(withoutDanceNativeEchoes(messages).map(m=>m.id)).toEqual(['one','other','reply']);
  });
  it('matches terminal echoes with second-resolution timestamps in the same second, not the previous second', () => {
    const messages = [original('one', '2026-10-09T00:00:01.789Z'),
      row('earlier', {timestamp: '2026-10-09T00:00:00Z'}), row('echo', {timestamp: '2026-10-09T00:00:01Z'})];
    expect(withoutDanceNativeEchoes(messages).map(m => m.id)).toEqual(['one', 'earlier']);
  });
  it('preserves the original readonly input and ordinary composer rows', () => {
    const ordinary=row('composer',{agentId:'user',shell:'system',sessionId:'composer'});
    const messages=Object.freeze([ordinary,row('native')]);
    expect(withoutDanceNativeEchoes(messages)).toEqual(messages);expect(messages).toHaveLength(2);
  });
});
