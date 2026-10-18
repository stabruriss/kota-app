import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { executeCommand, OUTPUT_LIMIT_BYTES, COMMAND_TIMEOUT_MS, BARTENDER_TIMEOUT_MS, timeoutForTool, FAILURE_COPY as copy } from '../cli.ts';
const cwd = fileURLToPath(new URL('.', import.meta.url));
const node = (script, stdin = '', args = []) => ({ command: process.execPath, args: ['-e', script, '--', ...args], stdin });

test('CLI wrapper passes shell-like body through raw UTF-8 stdin and preserves output', async () => {
  const body = '$(not-a-command)\n"quoted" 中文\u2028\u2029\n';
  const argument = 'literal ; $(not-a-command)';
  const result = await executeCommand(node("let text='';process.stdin.setEncoding('utf8');process.stdin.on('data',v=>text+=v);process.stdin.on('end',()=>{process.stdout.write(JSON.stringify({text,arg:process.argv[1]}));process.stderr.write('fixture stderr');});", body, [argument]), { cwd, copy });
  assert.equal(result.exitCode, 0);
  assert.deepEqual(JSON.parse(result.stdout), { text: body, arg: argument });
  assert.equal(result.stderr, 'fixture stderr');
  assert.equal(result.error, undefined);
});

test('nonzero exit retains the existing CLI output, not a fabricated delivery claim', async () => {
  const result = await executeCommand(node("process.stdout.write('fixture stdout');process.stderr.write('fixture failure');process.exitCode=7;"), { cwd, copy });
  assert.deepEqual(result, { exitCode: 7, stdout: 'fixture stdout', stderr: 'fixture failure' });
});

test('spawn errors use fixed copy without launch paths or exceptions', async () => {
  const result = await executeCommand({ command: 'kota-test-nonexistent-command', args: [], stdin: '' }, { cwd, copy });
  assert.equal(result.error, 'spawn_failed');
  assert.equal(result.stderr, copy.spawn_failed);
  assert.equal(result.stdout, '');
});

test('an already-aborted operation never starts a child', async () => {
  const result = await executeCommand(node("process.stdout.write('must not run');"), { cwd, copy, signal: AbortSignal.abort() });
  assert.equal(result.error, 'cancelled');
  assert.equal(result.stdout, '');
});

test('tool AbortSignal cancels a running child and closes its streams', { timeout: 5_000 }, async () => {
  const controller = new AbortController();
  const pending = executeCommand(node('setInterval(()=>{},1000);'), { cwd, copy, signal: controller.signal });
  controller.abort();
  const result = await pending;
  assert.equal(result.error, 'cancelled');
  assert.equal(result.stderr, copy.cancelled);
});

test('timeout terminates the tool rather than returning successful partial output', { timeout: 5_000 }, async () => {
  const result = await executeCommand(node('setInterval(()=>{},1000);'), { cwd, copy, timeoutMs: 100 });
  assert.equal(result.error, 'timeout');
  assert.equal(result.stderr, copy.timeout);
});

for (const stream of ['stdout', 'stderr']) {
  test(`${stream} capture has its own byte limit and reports incomplete output`, { timeout: 5_000 }, async () => {
    const result = await executeCommand(node(`process.${stream}.write('x'.repeat(20000));setInterval(()=>{},1000);`), { cwd, copy, outputLimit: 128 });
    assert.equal(result.error, 'output_limit');
    assert.equal(result.stdout.length, stream === 'stdout' ? 128 : 0);
    assert.equal(result.stderr.endsWith(copy.output_limit), true);
    assert.notEqual(result.exitCode, 0);
  });
}

test('production timeout and output limits follow the contract', () => {
  assert.equal(COMMAND_TIMEOUT_MS, 120_000);
  assert.equal(OUTPUT_LIMIT_BYTES, 1024 * 1024);
  assert.equal(BARTENDER_TIMEOUT_MS, 1_815_000);
  assert.equal(timeoutForTool('kota_bartender'), BARTENDER_TIMEOUT_MS);
  for (const name of ['kota_agent_bus', 'kota_bbs', 'kota_ember']) assert.equal(timeoutForTool(name), COMMAND_TIMEOUT_MS);
});
