import assert from 'node:assert/strict';
import test from 'node:test';
import { kotaSystemPrompt } from '../system-prompt.ts';
import { createFakeProvider } from './fake-provider/server.mjs';
import { createTestSession } from './fake-provider/rpc-client.mjs';
import { envelope, extensionPath, injectCommand } from './fixtures.mjs';

const PREAMBLE = 'You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.';
const TOOLS = '<tools>\n- read: Read file contents\n</tools>';
const RULES = '<rules>\n- Be concise in your responses\n</rules>';
const CONTEXT = '<project_context>\nProject-specific instructions and guidelines:\n\n<project_instructions path="/project/AGENTS.md">\nYou are Fixture, a Kota project agent.\n\n<docs>\nProject notes\n</docs>\n</project_instructions>\n</project_context>';
const CWD = '<cwd>\n/project\n</cwd>';
const DOCS = '<docs>\nPi documentation (read only when the user asks about pi itself):\n- Main documentation: /app/README.md\n</docs>';
const prompt = (...sections) => sections.join('\n\n');

test('leaves out Pi\'s own <docs> section but not a <docs> inside the project context', () => {
  assert.equal(kotaSystemPrompt(prompt(PREAMBLE, TOOLS, RULES, DOCS, CONTEXT, CWD)), prompt(TOOLS, RULES, CONTEXT, CWD));
});

test('leaves out the untagged preamble and keeps every tagged section byte for byte', () => {
  assert.equal(kotaSystemPrompt(prompt(PREAMBLE, TOOLS, RULES, CONTEXT, CWD)), prompt(TOOLS, RULES, CONTEXT, CWD));
  // Everything before the first section is Pi's preamble, however many paragraphs it runs to.
  assert.equal(kotaSystemPrompt(prompt(PREAMBLE, 'Loose text.', CWD)), CWD);
});

test('gives the same prompt for the same input on every run', () => {
  const input = prompt(PREAMBLE, TOOLS, RULES, CONTEXT, CWD);
  assert.equal(kotaSystemPrompt(input), kotaSystemPrompt(input));
  assert.equal(kotaSystemPrompt(kotaSystemPrompt(input)), kotaSystemPrompt(input));
});

test('sends a prompt of any other shape as Pi rendered it', () => {
  for (const other of [prompt(TOOLS, RULES), 'Only untagged text.', prompt(TOOLS, 'Loose text.', CWD), `${PREAMBLE}\n\n<tools>\nunclosed`]) {
    assert.equal(kotaSystemPrompt(other), other);
  }
});

const systemText = (body) => {
  const system = body.messages.find((message) => message.role === 'system' || message.role === 'developer');
  return typeof system.content === 'string' ? system.content : system.content.map((part) => part.text ?? '').join('');
};

test('every model request of a Kota run leaves out the preamble and docs while the transcript keeps Pi\'s sections', { timeout: 30_000 }, async (t) => {
  const provider = await createFakeProvider({ steps: [
    { type: 'tool', name: 'read', arguments: { path: 'fixture.txt' } },
    { type: 'text', text: 'fixture reply' },
  ] });
  t.after(() => provider.close());
  const session = await createTestSession({ baseUrl: provider.baseUrl, extensionPath });
  t.after(() => session.close());
  const after = session.client.records.length;
  assert.equal((await session.client.command('prompt', injectCommand(envelope('msg-identity')))).success, true);
  await session.client.waitFor((record) => record.type === 'agent_settled', { after });
  assert.equal(provider.requests.length, 2);
  const [first, second] = provider.requests.map(({ body }) => systemText(body));
  assert.equal(second, first, 'every request of a run carries the same system prompt');
  assert.equal(first.startsWith('<'), true);
  assert.equal(first.includes('operating inside pi'), false);
  assert.equal(/\bharness\b/i.test(first), false);
  // The test's own working directory is a path that may hold "pi"; every other section must not.
  assert.equal(/\bpi\b/i.test(first.replace(/<cwd>\n[\s\S]*?\n<\/cwd>/, '')), false);
  assert.equal(first.includes('<docs>'), false);
  for (const tag of ['<tools>', '<rules>', '<cwd>']) assert.equal(first.includes(tag), true, tag);
  const entries = (await session.client.command('get_entries')).data.entries;
  const recorded = entries.filter((entry) => entry.type === 'message' && entry.message.role === 'system');
  assert.equal(recorded.some((entry) => typeof entry.message.sections?.preamble === 'string'), true);
  assert.equal(recorded.some((entry) => typeof entry.message.sections?.docs === 'string'), true);
});
