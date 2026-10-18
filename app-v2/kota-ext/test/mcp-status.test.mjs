import test from 'node:test';
import assert from 'node:assert/strict';
import { completionRow, mcpSnapshot, parseMcpRequest } from '../mcp-status.ts';

test('native MCP descriptions project to bounded health and controlled reasons, never raw errors', () => {
  const item = description => ({ value: 'reconnect example', label: 'example', description });
  for (const description of ['connected · 0 tools', 'connected · 1 tool', 'connected · 2 tools · 1 resource', 'connected · 3 tools · 4 resources', 'connected · 0 tools · resources', 'connected · 1 tool · resources']) {
    assert.equal(completionRow(item(description)).status, 'connected');
  }
  assert.equal(completionRow(item('connecting…')).status, 'connecting');
  assert.equal(completionRow(item('needs sign-in')).status, 'needs_auth');
  for (const description of ['disconnected', 'closed', 'disabled']) assert.equal(completionRow(item(description)).status, 'not_connected');
  const failed = completionRow(item('failed: ECONNREFUSED https://fake.invalid/secret-token'));
  assert.deepEqual(failed, { name: 'example', status: 'failed', reason: 'connection_refused' });
  assert(!JSON.stringify(failed).includes('secret-token'));
  assert.equal(completionRow(item('a future Pi description')), undefined);
  assert.equal(completionRow(item('connected · 1 tool · resources · future')), undefined);
  assert.equal(completionRow({ ...item('connected · 1 tool'), value: 'login example' }), undefined);
});

test('Pi resource capability descriptions keep the whole health snapshot available', () => {
  assert.deepEqual(mcpSnapshot(() => [{ value: 'reconnect docs', label: 'docs', description: 'connected · 1 tool · resources' }], [], false), {
    available: true, servers: [{ name: 'docs', status: 'connected' }],
  });
});

test('missing/throwing/changed/duplicate completion callbacks report unknown without reconnecting', () => {
  const unknown = { available: false, servers: [] };
  assert.deepEqual(mcpSnapshot(undefined, [], false), unknown);
  assert.deepEqual(mcpSnapshot(() => { throw Error('fake secret'); }, [], false), unknown);
  assert.deepEqual(mcpSnapshot(() => Promise.reject(Error('fake future asynchronous callback')), [], false), unknown);
  assert.deepEqual(mcpSnapshot(() => [{ value: 'reconnect a', label: 'a', description: 'future' }], [], false), unknown);
  const row = { value: 'reconnect a', label: 'a', description: 'connected · 1 tool' };
  assert.deepEqual(mcpSnapshot(() => [row, row], [], false), unknown);
  let calls = 0;
  assert.deepEqual(mcpSnapshot(prefix => { assert.equal(prefix, 'reconnect '); calls++; return null; },
    [{ name: 'starting', enabled: true }, { name: 'disabled', enabled: false }], false), {
    available: true, servers: [{ name: 'starting', status: 'connecting' }, { name: 'disabled', status: 'not_connected' }],
  });
  assert.equal(calls, 1);
});

test('status command cannot request a path, mutation, connection or arbitrary text', () => {
  assert.deepEqual(parseMcpRequest('{"schema":1,"requestId":"local-1-2"}'), { schema: 1, requestId: 'local-1-2' });
  for (const text of ['broken', '{"schema":2,"requestId":"a"}', '{"schema":1,"requestId":"a","path":"/other"}',
    '{"schema":1,"requestId":"a","operation":"reconnect"}', '{"schema":1,"requestId":"a b"}']) assert.equal(parseMcpRequest(text), undefined);
});
