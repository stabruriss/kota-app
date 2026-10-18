import assert from 'node:assert/strict';
import test from 'node:test';
import { buildInvocation, InvalidToolArguments } from '../tool-command.ts';

const body = 'raw quotes: " \' $(not-a-command)\nsecond line\n';
const cases = [
  ['kota_agent_bus', { to: 'agent-fixture', body, intent: 'handoff' }, ['kota-agent-bus', ['send', '--to', 'agent-fixture', '--intent', 'handoff'], body]],
  ['kota_agent_bus', { to: 'Fixture', body }, ['kota-agent-bus', ['send', '--to', 'Fixture'], body]],
  ['kota_bbs', { action: 'agents' }, ['kota-bbs', ['agents'], '']],
  ['kota_bbs', { action: 'show', threadId: 'thread-fixture' }, ['kota-bbs', ['show', 'thread-fixture'], '']],
  ['kota_bbs', { action: 'new', body, projects: ['project-one', 'project-two'], at: 'local/project-one/agent-fixture', attachments: ['/fixture/one file.png', '/fixture/two.txt'] }, ['kota-bbs', ['new', '--projects', 'project-one', 'project-two', '--at', 'local/project-one/agent-fixture', '--attach', '/fixture/one file.png', '--attach', '/fixture/two.txt'], body]],
  ['kota_bbs', { action: 'new', body, broadcast: true }, ['kota-bbs', ['new', '--broadcast'], body]],
  ['kota_bbs', { action: 'reply', threadId: 'thread-fixture', body: '', attachments: ['fixture.txt'] }, ['kota-bbs', ['reply', 'thread-fixture', '--attach', 'fixture.txt'], '']],
  ['kota_ember', { action: 'list', draft: true }, ['kota-ember', ['list', '--json', '--draft'], '']],
  ['kota_ember', { action: 'show', id: 'schedule-fixture' }, ['kota-ember', ['show', 'schedule-fixture'], '']],
  ['kota_ember', { action: 'delete', id: 'draft-fixture' }, ['kota-ember', ['delete', 'draft-fixture'], '']],
  ['kota_ember', { action: 'add', draft: true, body }, ['kota-ember', ['add', '--draft'], body]],
  ['kota_ember', { action: 'add', to: ['Fixture', 'agent-fixture'], timing: { kind: 'idle' }, body }, ['kota-ember', ['add', '--to', 'Fixture,agent-fixture', '--idle'], body]],
  ['kota_ember', { action: 'update', id: 'schedule-fixture', timing: { kind: 'every', value: '10m' }, endAfter: 2, endAt: '2026-10-03 12:00' }, ['kota-ember', ['update', 'schedule-fixture', '--every', '10m', '--end-after', '2', '--end-at', '2026-10-03 12:00'], '']],
  ['kota_bartender', { action: 'sync' }, ['kota-bartender', ['sync', '--json'], '']],
  ['kota_my_soul', { action: 'add-mark', text: '被用户纠正时先复述他的原话再改方案。' }, ['kota-my-soul', ['add-mark', '--', '被用户纠正时先复述他的原话再改方案。'], '']],
  ['kota_my_soul', { action: 'add-mark', text: '--not an option' }, ['kota-my-soul', ['add-mark', '--', '--not an option'], '']],
  ['kota_my_soul', { action: 'edit-mark', id: 'm-20261014-7b3e', text: '-starts with a dash' }, ['kota-my-soul', ['edit-mark', 'm-20261014-7b3e', '--', '-starts with a dash'], '']],
  ['kota_my_soul', { action: 'delete-mark', id: 'm-20261014-7b3e' }, ['kota-my-soul', ['delete-mark', 'm-20261014-7b3e'], '']],
  ['kota_my_soul', { action: 'read-mark' }, ['kota-my-soul', ['read-mark'], '']],
  ['kota_my_soul', { action: 'search-episode', query: '固定流程 改动' }, ['kota-my-soul', ['search-episode', '--', '固定流程 改动'], '']],
  ['kota_my_soul', { action: 'search-episode', query: '-dash words', project: 'project-fixture' }, ['kota-my-soul', ['search-episode', '--project', 'project-fixture', '--', '-dash words'], '']],
  ['kota_my_soul', { action: 'search-episode', id: 'violet-46d2ec1194aec0a07' }, ['kota-my-soul', ['search-episode', '--id', 'violet-46d2ec1194aec0a07'], '']],
  ['kota_my_soul', { action: 'clear-heat', id: 'd-9f31c2a4' }, ['kota-my-soul', ['clear-heat', 'd-9f31c2a4'], '']],
];
for (const [name, args, expected] of cases) {
  test(`${name} ${args.action ?? 'send'} builds exact argv and unchanged stdin`, () => {
    const invocation = buildInvocation(name, args);
    assert.deepEqual([invocation.command, invocation.args, invocation.stdin], expected);
  });
}

for (const kind of ['in', 'every', 'at', 'cron']) {
  test(`Ember ${kind} timing remains one argv value`, () => {
    const value = kind === 'cron' ? '0 9 * * 1-5' : 'fixture-time';
    const invocation = buildInvocation('kota_ember', { action: 'add', to: ['Fixture'], timing: { kind, value }, body });
    assert.deepEqual(invocation.args, ['add', '--to', 'Fixture', `--${kind}`, value]);
  });
}

const optionValues = [
  ['kota_agent_bus', 'to', value => ({ to: value, body })],
  ['kota_agent_bus', 'intent', value => ({ to: 'Fixture', body, intent: value })],
  ['kota_bbs', 'show.threadId', value => ({ action: 'show', threadId: value })],
  ['kota_bbs', 'reply.threadId', value => ({ action: 'reply', threadId: value, body })],
  ['kota_bbs', 'projects', value => ({ action: 'new', projects: ['project-one', value], body })],
  ['kota_bbs', 'at', value => ({ action: 'new', broadcast: true, at: value, body })],
  ['kota_bbs', 'attachments', value => ({ action: 'reply', threadId: 'thread-fixture', attachments: ['fixture.txt', value], body })],
  ['kota_ember', 'show.id', value => ({ action: 'show', id: value })],
  ['kota_ember', 'delete.id', value => ({ action: 'delete', id: value })],
  ['kota_ember', 'update.id', value => ({ action: 'update', id: value })],
  ['kota_ember', 'add.to', value => ({ action: 'add', to: ['Fixture', value], timing: { kind: 'idle' }, body })],
  ['kota_ember', 'update.to', value => ({ action: 'update', id: 'schedule-fixture', to: ['Fixture', value] })],
  ...['in', 'every', 'at', 'cron'].map(kind => ['kota_ember', `timing.${kind}`, value => ({ action: 'add', to: ['Fixture'], timing: { kind, value }, body })]),
  ['kota_ember', 'update.timing', value => ({ action: 'update', id: 'schedule-fixture', timing: { kind: 'in', value } })],
  ['kota_ember', 'add.endAt', value => ({ action: 'add', to: ['Fixture'], timing: { kind: 'idle' }, endAt: value, body })],
  ['kota_ember', 'update.endAt', value => ({ action: 'update', id: 'schedule-fixture', endAt: value })],
  ['kota_my_soul', 'edit-mark.id', value => ({ action: 'edit-mark', id: value, text: 'mark' })],
  ['kota_my_soul', 'delete-mark.id', value => ({ action: 'delete-mark', id: value })],
  ['kota_my_soul', 'clear-heat.id', value => ({ action: 'clear-heat', id: value })],
  ['kota_my_soul', 'search-episode.id', value => ({ action: 'search-episode', id: value })],
  ['kota_my_soul', 'search-episode.project', value => ({ action: 'search-episode', query: 'words', project: value })],
];
for (const [name, field, args] of optionValues) {
  test(`${name} rejects leading-dash argv value in ${field}`, () => {
    for (const value of ['--project-root=/x', '-x', '--', '-']) {
      assert.throws(() => buildInvocation(name, args(value)), InvalidToolArguments);
    }
  });
}

test('a leading-dash body remains unchanged on stdin for every body-bearing action', () => {
  const body = '--project-root=/x\n-x "quoted" $(not-a-command)\n';
  for (const [name, args] of [
    ['kota_agent_bus', { to: 'Fixture', body }],
    ['kota_bbs', { action: 'new', broadcast: true, body }],
    ['kota_bbs', { action: 'reply', threadId: 'thread-fixture', body }],
    ['kota_ember', { action: 'add', draft: true, body }],
    ['kota_ember', { action: 'add', to: ['Fixture'], timing: { kind: 'idle' }, body }],
    ['kota_ember', { action: 'update', id: 'schedule-fixture', body }],
  ]) assert.equal(buildInvocation(name, args).stdin, body);
});

test('rejects unknown identity/context fields and invalid cross-field combinations', () => {
  const invalid = [
    ['kota_agent_bus', { to: 'Fixture', body, cwd: '/not-allowed' }],
    ['kota_agent_bus', { to: 'Fixture', body, env: {} }],
    ['kota_agent_bus', { to: '', body }],
    ['kota_bbs', { action: 'new', body, projects: ['project-one'], broadcast: true }],
    ['kota_bbs', { action: 'new', body }],
    ['kota_bbs', { action: 'new', body, projects: [] }],
    ['kota_bbs', { action: 'reply', threadId: 'thread-fixture', body: '', attachments: [] }],
    ['kota_bbs', { action: 'reply', threadId: 'thread-fixture', body, attachments: Array(10).fill('fixture.txt') }],
    ['kota_ember', { action: 'add', draft: true, body, to: ['Fixture'] }],
    ['kota_ember', { action: 'add', to: [], timing: { kind: 'idle' }, body }],
    ['kota_ember', { action: 'add', to: ['Fixture'], timing: { kind: 'idle', value: 'invalid' }, body }],
    ['kota_ember', { action: 'update', id: 'schedule-fixture', endAfter: 0 }],
    ['kota_bartender', { action: 'status' }],
    ['kota_bartender', { action: 'sync', projectRoot: '/not-allowed' }],
    ['kota_my_soul', { action: 'add-mark' }],
    ['kota_my_soul', { action: 'add-mark', text: '' }],
    ['kota_my_soul', { action: 'add-mark', text: 'mark', id: 'm-20261014-7b3e' }],
    ['kota_my_soul', { action: 'edit-mark', id: 'm-20261014-7b3e' }],
    ['kota_my_soul', { action: 'read-mark', id: 'm-20261014-7b3e' }],
    ['kota_my_soul', { action: 'search-episode' }],
    ['kota_my_soul', { action: 'search-episode', id: 'violet-fixture', query: 'words' }],
    ['kota_my_soul', { action: 'search-episode', id: 'violet-fixture', project: 'project-fixture' }],
    ['kota_my_soul', { action: 'clear-heat' }],
    ['kota_my_soul', { action: 'forget', id: 'm-20261014-7b3e' }],
    ['kota_my_soul', { action: 'read-mark', cwd: '/not-allowed' }],
  ];
  for (const [name, args] of invalid) assert.throws(() => buildInvocation(name, args), InvalidToolArguments);
});
