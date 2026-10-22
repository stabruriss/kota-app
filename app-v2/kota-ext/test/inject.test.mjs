import assert from 'node:assert/strict';
import test from 'node:test';
import { CUSTOM_TYPES, MAX_INJECTION_BYTES, parseInjection } from '../inject.ts';
import { registerInjection } from '../bridge.ts';
import { envelope } from './fixtures.mjs';

function errorFor(value) {
  try { parseInjection(typeof value === 'string' ? value : JSON.stringify(value)); }
  catch (error) { return error.message; }
  assert.fail('Expected validation failure');
}

function stubExtension() {
  const handlers = new Map();
  const commands = new Map();
  const messages = [];
  registerInjection({
    on: (event, handler) => handlers.set(event, handler),
    registerCommand: (name, command) => commands.set(name, command),
    sendMessage: (...args) => messages.push(args),
  });
  return { handlers, messages, commands, inject: raw => commands.get('kota-inject').handler(raw, {isIdle:()=>true}) };
}

test('all seven sources preserve original content and routing metadata', () => {
  for (const customType of CUSTOM_TYPES) {
    const input = envelope('msg-fixture', customType);
    input.details.sourceActorId = 'actor-fixture';
    input.details.sourceIntent = 'handoff';
    assert.deepEqual(parseInjection(JSON.stringify(input)), input);
  }
});

test('a composer routeModel is an optional ID-shaped detail and survives parsing unchanged', () => {
  const input = envelope('msg-route');
  input.details.routeModel = 'model-2';
  assert.deepEqual(parseInjection(JSON.stringify(input)), input);
  for (const routeModel of [2, '', 'model 2', 'm'.repeat(129)]) {
    const bad = envelope('msg-route');
    bad.details.routeModel = routeModel;
    assert.match(errorFor(bad), /code=invalid-details$/);
  }
});

test('accepts structured text/image content without rewriting it', () => {
  const input = envelope('msg-blocks', 'kota.user', [
    { type: 'text', text: ' do not trim ' },
    { type: 'image', data: 'aW1hZ2U=', mimeType: 'image/png' },
  ]);
  assert.deepEqual(parseInjection(JSON.stringify(input)).content, input.content);
});

test('a large valid base64 block stays within the transport limit without regex recursion', () => {
  const data = 'a'.repeat(8 * 1024 * 1024);
  const input = envelope('msg-large-image', 'kota.user', [{ type: 'image', data, mimeType: 'image/png' }]);
  assert.equal(parseInjection(JSON.stringify(input)).content[0].data, data);
});

test('malformed JSON never guesses an ID or leaks input', () => {
  assert.equal(errorFor('{"kotaId":"msg-fixture#agent-fixture", broken secret-body'), 'kota-inject-failed kotaId=unknown code=invalid-json');
  assert.equal(errorFor(null), 'kota-inject-failed kotaId=unknown code=invalid-envelope');
});

test('parsed validation errors retain only a syntactically valid encoded ID', () => {
  const input = envelope();
  input.customType = 'not-a-kota-source';
  assert.equal(errorFor(input), 'kota-inject-failed kotaId=msg-fixture%23agent-fixture code=invalid-envelope');
  input.kotaId = 'msg-fixture#agent-fixture\n forged error';
  assert.equal(errorFor(input), 'kota-inject-failed kotaId=unknown code=invalid-id');
});

test('rejects unknown fields, conflicting identities, invalid timestamps, and nested non-input blocks', () => {
  const unknown = envelope();
  unknown.details.kotaId = unknown.kotaId;
  assert.match(errorFor(unknown), /code=invalid-details$/);
  const mismatch = envelope();
  mismatch.details.messageId = 'another-message';
  assert.match(errorFor(mismatch), /code=invalid-id$/);
  const timestamp = envelope();
  timestamp.details.createdAt = '2026-02-31T00:00:00Z';
  assert.match(errorFor(timestamp), /code=invalid-details$/);
  for (const content of ['', [], [{ type: 'thinking', thinking: 'secret' }], [{ type: 'image', data: 'not base64', mimeType: 'image/png' }]]) {
    assert.match(errorFor(envelope('msg-content', 'kota.user', content)), /code=invalid-content$/);
  }
});

test('maximum injection length is measured in UTF-8 bytes', () => {
  assert.equal(errorFor('中'.repeat(Math.ceil(MAX_INJECTION_BYTES / 3))), 'kota-inject-failed kotaId=unknown code=too-large');
});

test('bridge registers only the commands and synchronous boundary hooks and sends once without an ACK', async () => {
  const fixture = stubExtension();
  assert.deepEqual([...fixture.handlers.keys()], ['turn_end', 'agent_before_settle']);
  const input = envelope();
  assert.equal(await fixture.inject(JSON.stringify(input)), undefined);
  assert.deepEqual(fixture.messages, [[{
    customType: input.customType, content: input.content, display: true,
    details: { ...input.details, kotaId: input.kotaId },
  }, { triggerTurn: true, deliverAs: 'steer' }]]);
});

test('busy inputs can be precisely cancelled or flushed synchronously, with held-only dedupe', async () => {
 const f=stubExtension();const inject=f.commands.get('kota-inject').handler;
 const a=envelope('held-a'),b=envelope('held-b');
 await inject(JSON.stringify(a),{isIdle:()=>false});await inject(JSON.stringify(b),{isIdle:()=>false});
 await assert.rejects(inject(JSON.stringify(a),{isIdle:()=>false}),/code=duplicate$/);
 await f.commands.get('kota-cancel').handler(a.kotaId);assert.equal(f.messages.length,0);
 f.handlers.get('turn_end')();assert.equal(f.messages.length,1);assert.equal(f.messages[0][0].details.kotaId,b.kotaId);
 await assert.rejects(f.commands.get('kota-cancel').handler(b.kotaId),/kota-cancel-too-late/);
 await inject(JSON.stringify(a),{isIdle:()=>false});f.handlers.get('agent_before_settle')();assert.equal(f.messages.length,2);
 await inject(JSON.stringify(a),{isIdle:()=>false});await f.commands.get('kota-clear-held').handler();
 f.handlers.get('turn_end')();assert.equal(f.messages.length,2);
});
test('held list has defensive count and byte bounds, with credit released by cancellation', async () => {
 const f=stubExtension();const inject=f.commands.get('kota-inject').handler;
 for(let i=0;i<64;i++)await inject(JSON.stringify(envelope(`held-${i}`)),{isIdle:()=>false});
 await assert.rejects(inject(JSON.stringify(envelope('overflow')),{isIdle:()=>false}),/code=queue-limit$/);
 await f.commands.get('kota-cancel').handler(envelope('held-0').kotaId);
 await inject(JSON.stringify(envelope('overflow')),{isIdle:()=>false});
 await f.commands.get('kota-clear-held').handler();
 for(let i=0;i<2;i++)await inject(JSON.stringify(envelope(`large-${i}`,'kota.user','x'.repeat(12*1024*1024))),{isIdle:()=>false});
 await assert.rejects(inject(JSON.stringify(envelope('large-2','kota.user','x'.repeat(12*1024*1024))),{isIdle:()=>false}),/code=queue-limit$/);
});

test('pause fences both hooks and idle injection until one synchronous ordered resume', async () => {
 const f=stubExtension(),events=[];
 const ctx={isIdle:()=>true,ui:{notify:message=>events.push(JSON.parse(message.slice(14)))}};
 const a=envelope('pause-a'),b=envelope('pause-b');
 await f.commands.get('kota-inject').handler(JSON.stringify(a),{isIdle:()=>false});
 const pause=f.commands.get('kota-hold-pause').handler('{"requestId":"pause-1"}',ctx);
 // The handler has already installed the fence before returning its Promise.
 f.handlers.get('turn_end')();f.handlers.get('agent_before_settle')();
 await f.commands.get('kota-inject').handler(JSON.stringify(b),ctx);
 assert.equal(f.messages.length,0);await pause;
 assert.deepEqual(events[0],{schema:1,requestId:'pause-1',action:'pause',kotaIds:[a.kotaId]});
 await assert.rejects(f.commands.get('kota-hold-resume').handler('{"requestId":"resume-busy"}',{...ctx,isIdle:()=>false}));
 assert.equal(f.messages.length,0);
 const resume=f.commands.get('kota-hold-resume').handler('{"requestId":"resume-1"}',ctx);
 assert.deepEqual(f.messages.map(([m])=>m.details.kotaId),[a.kotaId,b.kotaId]);await resume;
 assert.deepEqual(events[1],{schema:1,requestId:'resume-1',action:'resume',kotaIds:[a.kotaId,b.kotaId]});
 await assert.rejects(f.commands.get('kota-hold-pause').handler('{"requestId":"x","extra":true}',ctx));
 await assert.rejects(f.commands.get('kota-hold-pause').handler('{"requestId":"bad\\nidentity"}',ctx));
 assert.equal(events.length,2);
});

test('reorder atomically permutes the full held set, preserving new input at the tail', async () => {
 const f=stubExtension(), inject=f.commands.get('kota-inject').handler;
 const [a,b,c,d]=['A','B','C','D'].map(x=>envelope(x));
 for(const x of [a,b,c]) await inject(JSON.stringify(x),{isIdle:()=>false});
 const command=f.commands.get('kota-reorder').handler;
 for(const ids of [[a.kotaId,b.kotaId],[a.kotaId,b.kotaId,d.kotaId]]) {
  await assert.rejects(command(JSON.stringify({requestId:'req',ids})),/kota-reorder-changed requestId=req/);
 }
 await assert.rejects(command(JSON.stringify({requestId:'req',ids:[a.kotaId,a.kotaId,c.kotaId]})),/Invalid Kota queue reorder/);
 await command(JSON.stringify({requestId:'req',ids:[c.kotaId,a.kotaId,b.kotaId]}));
 await inject(JSON.stringify(d),{isIdle:()=>false});f.handlers.get('turn_end')();
 assert.deepEqual(f.messages.map(x=>x[0].details.kotaId),[c,a,b,d].map(x=>x.kotaId));
 // Flush won this time: no old message can be reconstructed/replayed.
 await assert.rejects(command(JSON.stringify({requestId:'late',ids:[c.kotaId,a.kotaId,b.kotaId]})),/kota-reorder-changed/);
 f.handlers.get('agent_before_settle')();assert.equal(f.messages.length,4);
});

test('pause rejects reorder without disturbing held order or the Stop handshake', async () => {
 const f=stubExtension(), inject=f.commands.get('kota-inject').handler;
 const a=envelope('A'),b=envelope('B');
 for(const x of [a,b]) await inject(JSON.stringify(x),{isIdle:()=>false});
 const ctx={isIdle:()=>true,ui:{notify(){}}};
 await f.commands.get('kota-hold-pause').handler('{"requestId":"pause"}',ctx);
 await assert.rejects(f.commands.get('kota-reorder').handler(JSON.stringify({requestId:'reorder',ids:[b.kotaId,a.kotaId]})),/kota-reorder-changed/);
 await f.commands.get('kota-hold-resume').handler('{"requestId":"resume"}',ctx);
 assert.deepEqual(f.messages.map(x=>x[0].details.kotaId),[a.kotaId,b.kotaId]);
});
