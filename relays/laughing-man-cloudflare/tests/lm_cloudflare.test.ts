import { afterEach, expect, it, vi } from 'vitest';
import worker, { type Env } from '../src/index';

afterEach(() => vi.restoreAllMocks());
const request = (path = '/desktop/heartbeat') => new Request(`https://worker.example${path}`);
function environment(error: unknown) {
  const lm = vi.fn().mockRejectedValue(error);
  const bbs = vi.fn();
  return { lm, bbs, env: {
    RELAY_STATE: { idFromName: vi.fn().mockReturnValue('lm'), get: () => ({ fetch: lm }) },
    BBS_GROUP: { idFromName: vi.fn().mockReturnValue('bbs'), get: () => ({ fetch: bbs }) },
  } as unknown as Env };
}

it('returns the DO free-tier text, supplied flags and next UTC midnight', async () => {
  vi.spyOn(Date, 'now').mockReturnValue(Date.parse('2026-10-24T20:00:00Z'));
  const error = Object.assign(new Error('Exceeded allowed volume of requests in Durable Objects free tier.'), {retryable:false,overloaded:true});
  const f = environment(error);
  const response = await worker.fetch(request(), f.env);
  expect(response.status).toBe(503);
  expect(response.headers.get('content-type')).toContain('application/json');
  expect(await response.json()).toEqual({ok:false,error:'cloudflare_limit',source:'durable_objects',
    limit:'volume of requests',message:error.message,resetAt:'2026-10-25T00:00:00.000Z',retryable:false,overloaded:true});
  expect(f.bbs).not.toHaveBeenCalled();
});

it('preserves a non-quota exception without inventing a reset or flags', async () => {
  const f = environment(new Error('The object is unavailable.'));
  const response = await worker.fetch(request('/health'), f.env);
  expect(response.status).toBe(503);
  expect(await response.json()).toEqual({ok:false,error:'cloudflare_limit',source:'durable_objects',
    message:'The object is unavailable.',limit:null,resetAt:null,retryable:null,overloaded:null});
});

it('bounds the original message by characters without splitting Unicode', async () => {
  const f = environment(new Error('🙂'.repeat(201)));
  expect(await (await worker.fetch(request(),f.env)).json()).toMatchObject({message:'🙂'.repeat(200),resetAt:null});
});

it('leaves BBS responses and thrown exceptions on their original route', async () => {
  const f = environment(new Error('LM must not run'));
  const original = new Response(JSON.stringify({ok:false,protocolVersion:1,error:'fixture'}),{status:409});
  f.bbs.mockResolvedValueOnce(original);
  expect(await worker.fetch(request('/bbs/owner/status'),f.env)).toBe(original);
  const error = new Error('BBS original error');
  f.bbs.mockRejectedValueOnce(error);
  await expect(worker.fetch(request('/bbs/owner/status'),f.env)).rejects.toBe(error);
  expect(f.lm).not.toHaveBeenCalled();
});
