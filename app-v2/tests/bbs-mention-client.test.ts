import { createHash } from 'node:crypto';
import { beforeEach, describe, expect, it, vi } from 'vitest';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(), isTauri: vi.fn() }));
import { invoke, isTauri } from '@tauri-apps/api/core';
import { bbsHumanPost, bbsHumanReply } from '../src/pty-client';
import { BbsMentionClientError, bbsMentionError } from '../src/bbs-mentions';
import fixture from './fixtures/bbs-mention-runtime.json';

// Unmodified BBS_MENTION_FIXTURE from 29a441168d6f926d82d37e34c49ccb92339cd9ef:
// bbs/notify/tests.rs::human_commands_emit_actual_safe_errors_and_durable_bridge_fixture.
// SHA-256 69b8b5ec37b01648801df7e9cbcfa3345f994a9cb21af58ea75473ad256bffbf.
// The backend test records a real durable room notice; wake means target resolution,
// not an actual PTY launch. These tests stub IPC, never perform native publication.

const mentions = [{ deviceId: 'local', projectId: 'p', agentId: 'a' }, { deviceId: 'remote', projectId: 'p', agentId: 'a' }];
const request = { projectId: 'p', projectDisplayName: 'Kota', projectTags: [], threadId: 'thread-one', body: 'Original body', attachments: [], mentions };
beforeEach(() => { vi.clearAllMocks(); vi.mocked(isTauri).mockReturnValue(true); });
describe('human publication mentions IPC', () => {
  it('matches the actual Human request/result and backend-only prefix, recipient room and durable notification key', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(fixture.threadId);
    expect(await bbsHumanPost(fixture.request)).toBe(fixture.threadId);
    expect(invoke).toHaveBeenCalledExactlyOnceWith('bbs_human_post', { request: fixture.request });
    expect(fixture.request.body).toBe('Original human input');
    const inputTarget = fixture.request.mentions[0], normalized = fixture.mentions[0];
    expect(inputTarget.deviceId).toBe('local'); expect(normalized.deviceId).toMatch(/^[a-f0-9]{64}$/);
    expect(normalized).toEqual({ ...inputTarget, deviceId: normalized.deviceId });
    expect(fixture.body).toBe(`@Receiver\n\n${fixture.request.body}`);
    expect(fixture.delivery).toMatchObject({ senderAgentId: 'bbs', intent: 'bbs-thread', projectId: normalized.projectId, target: normalized.agentId });
    expect(fixture.delivery.projectId).not.toBe(fixture.request.projectId);
    const key = createHash('sha256').update(JSON.stringify([
      fixture.threadId, fixture.postId, normalized.deviceId, normalized.projectId, normalized.agentId,
    ])).digest('hex');
    expect(fixture.delivery.eventId).toBe(`bbs-mention:${key}`);
    expect(fixture.delivery.dedupeKey).toBe(fixture.delivery.eventId); expect(fixture.processedCount).toBe(1);
    expect(fixture.delivery.text).toContain(`kota-bbs show ${fixture.threadId}`);
    expect(fixture.delivery.text.endsWith(fixture.body)).toBe(true);
  });
  it('maps all three actual Rust rejection objects through both command wrappers without retrying', async () => {
    const expected = [
      [fixture.errors.unjoined, 'This agent is on another device, but this device is not in a sync group. No thread was created.'],
      [fixture.errors.privateThread, 'This agent is on another device, but this thread is not shared. No reply was posted. Start a new thread to @ this agent.'],
      [fixture.errors.notSynced, 'The agent list for this device has not synced yet. Try again after syncing.'],
    ] as const;
    for (const call of [bbsHumanPost, bbsHumanReply]) for (const [raw, message] of expected) {
      vi.mocked(invoke).mockRejectedValueOnce(raw);
      const error = await call(request).catch(error => error);
      expect(error).toBeInstanceOf(BbsMentionClientError); expect(error.message).toBe(message);
      expect(error.code).toBe(raw.code); expect(error).not.toHaveProperty('cause');
    }
    expect(invoke).toHaveBeenCalledTimes(6);
  });
  it('uses existing commands and never prefixes the body or starts a separate bus action', async () => {
    vi.mocked(invoke).mockResolvedValue('result-id');
    expect(await bbsHumanPost(request)).toBe('result-id'); expect(await bbsHumanReply(request)).toBe('result-id');
    expect(vi.mocked(invoke).mock.calls).toEqual([['bbs_human_post', { request }], ['bbs_human_reply', { request }]]);
    expect(request.body).toBe('Original body'); expect(request.mentions).toEqual(mentions);
  });
  it('maps exact safety codes for both commands without raw cause and without retrying', async () => {
    for (const call of [bbsHumanPost, bbsHumanReply]) for (const code of ['mention_requires_group', 'mention_thread_not_shared', 'agent_roster_not_synced'] as const) {
      vi.mocked(invoke).mockRejectedValueOnce({ code });
      const error = await call(request).catch(error => error);
      expect(error).toBeInstanceOf(BbsMentionClientError); expect(error.code).toBe(code); expect(error).not.toHaveProperty('cause');
      if (code === 'mention_thread_not_shared') expect(error.message).toBe('This agent is on another device, but this thread is not shared. No reply was posted. Start a new thread to @ this agent.');
    }
    expect(invoke).toHaveBeenCalledTimes(6);
  });
  it('does not identify codes in strings or extra fields, and preserves the established attachment error path', async () => {
    for (const error of [null, [], 'mention_requires_group', new Error('mention_requires_group'),
      { code: 'mention_requires_group', token: 'SECRET' }, { code: '__proto__' }, { code: 'unknown' }]) expect(bbsMentionError(error)).toBeNull();
    const failure = new Error('could not attach file.pdf: disk full'); vi.mocked(invoke).mockRejectedValueOnce(failure);
    await expect(bbsHumanReply(request)).rejects.toBe(failure);
    vi.mocked(isTauri).mockReturnValue(false);
    await expect(bbsHumanPost(request)).rejects.toThrow('requires the Kota runtime'); expect(invoke).toHaveBeenCalledTimes(1);
  });
});
