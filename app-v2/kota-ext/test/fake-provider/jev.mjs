import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { pathToFileURL } from 'node:url';

/**
 * Loopback-only fake Jev (System One wire, POST /v1/systemone). Steps are consumed in request
 * order: { serious, casual } probabilities, { status } for an HTTP error, { hang: true } to
 * never answer, or { drop: true } to close the connection. It never forwards a request.
 */
export async function createFakeJev({ steps = [] } = {}) {
  const requests = [];
  const sockets = new Set();
  const server = createServer(async (request, response) => {
    const url = new URL(request.url, 'http://127.0.0.1');
    if (request.method !== 'POST' || url.pathname !== '/v1/systemone') {
      response.writeHead(404, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: { message: 'Unknown fake Jev route' } }));
      return;
    }
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    const step = steps[requests.length] ?? { serious: 0.5, casual: 0.5 };
    requests.push({ body, authorization: request.headers.authorization ?? null });
    if (step.hang) return;
    if (step.drop) { request.socket.destroy(); return; }
    if (step.status) {
      response.writeHead(step.status, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ error: { message: 'Synthetic fake Jev failure.' } }));
      return;
    }
    const question = Object.keys(body.questions)[0];
    const ids = Object.keys(body.questions[question].criteria);
    const probabilities = step.probabilities ?? Object.fromEntries(ids.map((id, i) => [id, i === 0 ? step.serious ?? 1/ids.length : step.casual ?? 1/ids.length]));
    const choice = step.choice ?? ids.reduce((best, id) => probabilities[id] > probabilities[best] ? id : best, ids[0]);
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({
      answers: { [question]: { type: 'choice', choice, probabilities, confidence: step.confidence ?? 1 } },
      usage: { input_tokens: 32, output_tokens: 1 },
    }));
  });
  server.on('connection', socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => { server.removeListener('error', reject); resolve(); });
  });
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    requests,
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise(resolve => server.close(resolve));
    },
  };
}

/** CLI for Rust's opt-in real-Pi tests: `node jev.mjs --script steps.json` prints one ready line. */
async function main() {
  let steps = [];
  for (let index = 2; index < process.argv.length; index++) {
    if (process.argv[index] === '--script') steps = JSON.parse(await readFile(process.argv[++index], 'utf8'));
    else throw new Error(`Unknown option: ${process.argv[index]}`);
  }
  const jev = await createFakeJev({ steps });
  process.stdout.write(`${JSON.stringify({ type: 'ready', baseUrl: jev.baseUrl })}\n`);
  const stop = () => { void jev.close().then(() => process.exit(0)); };
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
