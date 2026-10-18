import { EventEmitter } from 'node:events';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';

export const PROVIDER = 'kota-fake';
export const MODEL = 'fixture';
const MAX_REQUEST_BYTES = 16 * 1024 * 1024;

function validateStep(step) {
  if (!step || !['text', 'tool', 'long', 'error'].includes(step.type)) {
    throw new Error('Fake response type must be text, tool, long, or error');
  }
  if (step.type === 'error' && (!Number.isInteger(step.status) || step.status < 400 || step.status > 599)) {
    throw new Error('Fake error status must be 400..599');
  }
  if (step.thinking !== undefined && typeof step.thinking !== 'string') throw new Error('thinking must be a string');
  if (step.type === 'tool' && !['read', 'bash', 'codemode', 'kota_agent_bus', 'kota_bbs', 'kota_ember', 'kota_bartender', 'kota_my_soul', 'kota_shadow_clone'].includes(step.name)) {
    throw new Error('Fake tool response names must be native or Kota fixture tools');
  }
  for (const key of ['chunkSize', 'repeat', 'holdAfterChunks']) {
    if (step[key] !== undefined && (!Number.isSafeInteger(step[key]) || step[key] < 1)) {
      throw new Error(`${key} must be a positive integer`);
    }
  }
  if (step.chunkDelayMs !== undefined && (!Number.isFinite(step.chunkDelayMs) || step.chunkDelayMs < 0)) {
    throw new Error('chunkDelayMs must be nonnegative');
  }
}

async function readJson(request) {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of request) {
    bytes += chunk.length;
    if (bytes > MAX_REQUEST_BYTES) throw new Error('Fake provider request is too large');
    chunks.push(chunk);
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

function json(response, status, value) {
  response.writeHead(status, { 'content-type': 'application/json' });
  response.end(JSON.stringify(value));
}

function resultFor(step, sequence, model) {
  const text = step.type === 'tool' ? null : (step.text ?? 'fixture reply').repeat(step.repeat ?? 1);
  const message = step.type === 'tool'
    ? { role: 'assistant', content: null, tool_calls: [{
      id: `call_fixture_${sequence}`, type: 'function',
      function: { name: step.name, arguments: JSON.stringify(step.arguments ?? {}) },
    }] }
    : { role: 'assistant', content: text, ...(step.thinking ? { reasoning_content: step.thinking } : {}) };
  return {
    id: `chatcmpl-fixture-${sequence}`, object: 'chat.completion', created: 1, model,
    choices: [{ index: 0, message, finish_reason: step.type === 'tool' ? 'tool_calls' : 'stop' }],
    usage: { prompt_tokens: 128, completion_tokens: Math.max(1, Math.ceil((text?.length ?? 16) / 4)), total_tokens: 128 + Math.max(1, Math.ceil((text?.length ?? 16) / 4)) },
  };
}

function delay(milliseconds, signal) {
  if (!milliseconds || signal.aborted) return Promise.resolve();
  return new Promise(resolve => {
    const finish = () => { clearTimeout(timer); signal.removeEventListener('abort', finish); resolve(); };
    const timer = setTimeout(finish, milliseconds);
    signal.addEventListener('abort', finish, { once: true });
  });
}

/** A loopback-only, scripted API fixture. It never forwards a request to a provider. */
export async function createFakeProvider({ port = 0, steps = [{ type: 'text', text: 'fixture reply' }] } = {}) {
  if (!Number.isSafeInteger(port) || port < 0 || port > 65535) throw new Error('Invalid port');
  if (!Array.isArray(steps) || steps.length === 0) throw new Error('At least one response step is required');
  steps.forEach(validateStep);
  const events = new EventEmitter();
  const requests = [];
  const controllers = new Set();

  function release(sequence) {
    const record = requests[sequence - 1];
    if (!record) return false;
    record.released = true;
    record.resume?.();
    return true;
  }

  async function hold(record, signal) {
    if (record.released || signal.aborted) return;
    await new Promise(resolve => {
      const finish = () => { record.resume = undefined; signal.removeEventListener('abort', finish); resolve(); };
      record.resume = finish;
      signal.addEventListener('abort', finish, { once: true });
    });
  }

  const server = createServer(async (request, response) => {
    try {
      const url = new URL(request.url, 'http://127.0.0.1');
      if (request.method === 'GET' && url.pathname === '/__test/state') {
        json(response, 200, { requests: requests.map(({ sequence, step, completed, aborted }) => ({ sequence, type: step.type, completed, aborted })) });
        return;
      }
      if (request.method === 'POST' && /^\/__test\/release\/\d+$/.test(url.pathname)) {
        json(response, 200, { released: release(Number(url.pathname.split('/').at(-1))) });
        return;
      }
      if (request.method === 'GET' && url.pathname === '/v1/models') {
        json(response, 200, { object: 'list', data: [{ id: MODEL, object: 'model', created: 1, owned_by: PROVIDER }] });
        return;
      }
      if (request.method !== 'POST' || url.pathname !== '/v1/chat/completions') {
        json(response, 404, { error: { message: 'Unknown fake-provider route' } });
        return;
      }
      const body = await readJson(request);
      if (!body || !Array.isArray(body.messages)) {
        json(response, 400, { error: { message: 'messages must be an array' } });
        return;
      }
      const sequence = requests.length + 1;
      // Script exhaustion is an error rather than a fallback that can mask an extra model turn.
      const step = steps[sequence - 1];
      if (!step) {
        json(response, 409, { error: { message: 'Fake response script exhausted' } });
        return;
      }
      const record = { sequence, step, body, completed: false, aborted: false, released: false };
      const controller = new AbortController();
      controllers.add(controller);
      response.once('close', () => {
        controllers.delete(controller);
        if (!record.completed) {
          record.aborted = true;
          controller.abort();
          events.emit('aborted', record);
        }
      });
      requests.push(record);
      events.emit('request', record);
      if (step.holdBeforeHeaders) await hold(record, controller.signal);
      if (controller.signal.aborted) return;
      if (step.type === 'error') {
        if (step.drop) { request.socket.destroy(); return; }
        record.completed = true;
        json(response, step.status, { error: { message: step.message ?? 'Synthetic fixture provider failure.', type: 'fixture_error', code: 'fixture_error' } });
        events.emit('completed', record);
        return;
      }
      const result = resultFor(step, sequence, body.model ?? MODEL);
      if (!body.stream) {
        record.completed = true;
        json(response, 200, result);
        events.emit('completed', record);
        return;
      }
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
      const send = (delta, finishReason = null, extra = {}) => response.write(`data: ${JSON.stringify({
        id: result.id, object: 'chat.completion.chunk', created: result.created, model: result.model,
        choices: [{ index: 0, delta, finish_reason: finishReason }], ...extra,
      })}\n\n`);
      send({ role: 'assistant', content: '' });
      if (step.thinking) send({ reasoning_content: step.thinking });
      const message = result.choices[0].message;
      let payload;
      if (step.type === 'tool') {
        const tool = message.tool_calls[0];
        send({ tool_calls: [{ index: 0, id: tool.id, type: 'function', function: { name: tool.function.name, arguments: '' } }] });
        payload = tool.function.arguments;
      } else {
        payload = message.content;
      }
      const chunkSize = step.chunkSize ?? (step.type === 'long' ? 32 : 256);
      let chunkCount = 0;
      for (let offset = 0; offset < payload.length; offset += chunkSize) {
        if (controller.signal.aborted) return;
        const piece = payload.slice(offset, offset + chunkSize);
        send(step.type === 'tool' ? { tool_calls: [{ index: 0, function: { arguments: piece } }] } : { content: piece });
        chunkCount++;
        if (step.holdAfterChunks === chunkCount) await hold(record, controller.signal);
        await delay(step.chunkDelayMs ?? 0, controller.signal);
      }
      if (controller.signal.aborted) return;
      send({}, result.choices[0].finish_reason, { usage: result.usage });
      response.end('data: [DONE]\n\n');
      record.completed = true;
      events.emit('completed', record);
    } catch (error) {
      if (response.destroyed) return;
      if (response.headersSent) response.destroy(error);
      else json(response, 400, { error: { message: error.message } });
    }
  });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => { server.removeListener('error', reject); resolve(); });
  });
  const baseUrl = `http://127.0.0.1:${server.address().port}/v1`;
  return {
    baseUrl, provider: PROVIDER, model: MODEL, requests, events, release,
    waitForRequest(sequence, timeoutMs = 10_000) {
      if (requests[sequence - 1]) return Promise.resolve(requests[sequence - 1]);
      return new Promise((resolve, reject) => {
        const cleanup = () => { clearTimeout(timer); events.removeListener('request', onRequest); };
        const onRequest = record => { if (record.sequence === sequence) { cleanup(); resolve(record); } };
        const timer = setTimeout(() => { cleanup(); reject(new Error(`Timed out waiting for fake request ${sequence}`)); }, timeoutMs);
        events.on('request', onRequest);
      });
    },
    async close() {
      for (const controller of controllers) controller.abort();
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    },
  };
}

async function main() {
  let port = 0;
  let steps;
  for (let index = 2; index < process.argv.length; index++) {
    const option = process.argv[index];
    if (option === '--port') port = Number(process.argv[++index]);
    else if (option === '--script') {
      const script = JSON.parse(await readFile(process.argv[++index], 'utf8'));
      steps = Array.isArray(script) ? script : script.steps;
    } else throw new Error(`Unknown option: ${option}`);
  }
  const provider = await createFakeProvider({ port, steps });
  process.stdout.write(`${JSON.stringify({ type: 'ready', baseUrl: provider.baseUrl, provider: provider.provider, model: provider.model })}\n`);
  const stop = () => { void provider.close().then(() => process.exit(0)); };
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
