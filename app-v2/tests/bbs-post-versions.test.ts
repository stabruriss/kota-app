import { describe, expect, it } from 'vitest';
import {
  bbsLogicalDeleteTarget, bbsPostVersionKey, bbsRemoveDeletedItem, bbsSelectedTopic, bbsThreadVersions, bbsUnavailablePosts, bbsVersionDeleteTarget,
  type VersionedBbsPost,
} from '../src/bbs-post-versions';
import type { BbsSnapshot, BbsThread } from '../src/pty-client';

function post(id: string, versionId?: string, kind = 'reply', body = id): VersionedBbsPost {
  return {
    threadId: 'thread-one', postId: id, versionId, kind, body, preview: body,
    projectId: 'project-one', projectDisplayName: 'Project', agentId: 'agent-one', agentDisplayName: 'Agent',
    createdAt: '2026-09-12T00:00:00Z', state: 'none', external: false,
  };
}
const a = 'a'.repeat(64), b = 'b'.repeat(64), c = 'c'.repeat(64);

describe('BBS version presentation', () => {
  it('preserves the legacy root and reply order without showing Fork labels', () => {
    const input = [post('topic', undefined, 'topic'), post('reply-z'), post('reply-a')];
    const view = bbsThreadVersions(input);
    expect(view.topics.map((version) => version.post)).toEqual([input[0]]);
    expect(view.topics[0].forkNumber).toBeNull();
    expect(view.replies.map(({ postId, floor }) => [postId, floor])).toEqual([['reply-z', 2], ['reply-a', 3]]);
    expect(view.replies.every((reply) => reply.versions[0].forkNumber === null)).toBe(true);
    expect(view.replyCount).toBe(2);
  });

  it('treats different reply IDs as ordinary replies, even with identical text and timestamps', () => {
    const view = bbsThreadVersions([post('topic', a, 'topic'), post('reply-one', b, 'reply', 'Same'), post('reply-two', b, 'reply', 'Same')]);
    expect(view.replyCount).toBe(2);
    expect(view.replies.map((reply) => reply.versions.length)).toEqual([1, 1]);
    expect(view.replies.flatMap((reply) => reply.versions).map((version) => version.forkNumber)).toEqual([null, null]);
  });

  it('groups only same-ID versions, counts a reply once, and uses one reply pool for every root version', () => {
    const view = bbsThreadVersions([
      post('topic', c, 'topic'), post('reply-one', b), post('topic', a, 'topic'),
      post('reply-two', a), post('reply-one', a),
    ]);
    expect(view.topics.map((version) => [version.post.versionId, version.forkNumber])).toEqual([[a, 1], [c, 2]]);
    expect(view.replyCount).toBe(2);
    expect(view.replies.map(({ postId, floor }) => [postId, floor])).toEqual([['reply-one', 2], ['reply-two', 3]]);
    expect(view.replies[0].versions.map((version) => version.forkNumber)).toEqual([1, 2]);
    expect(bbsSelectedTopic(view, view.topics[1].key)).toBe(view.topics[1]);
  });

  it('does not change selection or identity when arrivals reorder display ordinals', () => {
    const selected = post('topic', c, 'topic');
    const key = bbsPostVersionKey(selected);
    const earlier = bbsThreadVersions([post('topic', b, 'topic'), selected]);
    const later = bbsThreadVersions([selected, post('topic', a, 'topic'), post('topic', b, 'topic')]);
    expect(bbsSelectedTopic(earlier, key)?.forkNumber).toBe(2);
    expect(bbsSelectedTopic(later, key)?.forkNumber).toBe(3);
    expect(bbsSelectedTopic(later, key)?.post).toBe(selected);
    expect(bbsSelectedTopic(later, 'removed-version-key')).toBe(later.topics[0]);
  });

  it('uses the supplied byte-version ID, not rendered body equality or attachment availability', () => {
    const left = post('topic', a, 'topic', 'Same Markdown body');
    const right = post('topic', b, 'topic', left.body);
    const view = bbsThreadVersions([left, right, { ...left, preview: 'Refreshed metadata' }]);
    expect(view.topics).toHaveLength(2);
    expect(view.topics.map((version) => version.post.versionId)).toEqual([a, b]);
    expect(view.replyCount).toBe(0);
  });

  it('does not mutate the backend snapshot and returns an empty view for no posts', () => {
    const first = Object.freeze(post('topic', c, 'topic'));
    const second = Object.freeze(post('topic', a, 'topic'));
    const input = Object.freeze([first, second]);
    bbsThreadVersions(input);
    expect(input).toEqual([first, second]);
    expect(bbsThreadVersions([])).toEqual({ topics: [], replies: [], replyCount: 0 });
    expect(bbsSelectedTopic(bbsThreadVersions([]), null)).toBeNull();
  });

  it('never promotes an available reply to OP when the root body is unavailable', () => {
    const replies = [post('reply-one', a), post('reply-two', b)];
    const view = bbsThreadVersions(replies);
    expect(view.topics).toEqual([]);
    expect(bbsSelectedTopic(view, null)).toBeNull();
    expect(view.replies.map(({ postId, floor }) => [postId, floor])).toEqual([['reply-one', 2], ['reply-two', 3]]);
    expect(view.replyCount).toBe(2);
  });

  it('keeps ordinary deletion logical and pins a visible Fork deletion to its exact version', () => {
    const normal = bbsThreadVersions([post('topic', a, 'topic')]).topics[0];
    expect(bbsVersionDeleteTarget(normal)).toEqual({ threadId: 'thread-one', postId: 'topic' });
    const fork = bbsThreadVersions([post('topic', b, 'topic'), post('topic', a, 'topic')]).topics[1];
    expect(bbsVersionDeleteTarget(fork)).toEqual({ threadId: 'thread-one', postId: 'topic', versionId: b });
    const reply = bbsThreadVersions([post('topic', a, 'topic'), post('reply-one', b), post('reply-one', a)]).replies[0].versions[1];
    expect(bbsVersionDeleteTarget(reply)).toEqual({ threadId: 'thread-one', postId: 'reply-one', versionId: b });
  });

  it('never widens a Fork delete when the version reference is missing or malformed', () => {
    const missing = bbsThreadVersions([post('topic', undefined, 'topic'), post('topic', a, 'topic')]).topics[0];
    expect(bbsVersionDeleteTarget(missing)).toBeNull();
    expect(bbsVersionDeleteTarget({ ...missing, post: post('topic', '../../other', 'topic') })).toBeNull();
  });

  it('keeps an explicit logical deletion separate from version deletion', () => {
    expect(bbsLogicalDeleteTarget(post('topic', a, 'topic'))).toEqual({ threadId: 'thread-one', postId: 'topic' });
    expect(bbsLogicalDeleteTarget(post('reply', b))).toEqual({ threadId: 'thread-one', postId: 'reply' });
  });

  it('projects only the confirmed deletion scope without dropping siblings or other threads', () => {
    const posts = [post('topic', a, 'topic'), post('topic', b, 'topic'), post('reply', a), post('reply', b), post('other-reply', c)];
    const thread = { threadId: 'thread-one', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
      createdByProject: 'project-one', createdByProjectLabel: 'Project', updatedAt: posts[0].createdAt,
      latestPostId: 'reply', isNew: false, relevant: true, posts };
    const snapshot: BbsSnapshot = { projectId: 'project-one', projectDisplayName: 'Project', root: '/tmp/bbs', newCount: 0,
      threads: [thread, { ...thread, threadId: 'untouched' }] };
    const version = bbsRemoveDeletedItem(snapshot, { threadId: 'thread-one', postId: 'topic', versionId: a })!;
    expect(version.threads[0].posts.map(bbsPostVersionKey)).toEqual(posts.slice(1).map(bbsPostVersionKey));
    expect(version.threads[1]).toBe(snapshot.threads[1]);
    expect(snapshot.threads[0].posts).toHaveLength(5);
    const absent = bbsRemoveDeletedItem(version, { threadId: 'thread-one', postId: 'topic', versionId: a })!;
    expect(absent.threads[0]).toBe(version.threads[0]);
    const reply = bbsRemoveDeletedItem(version, { threadId: 'thread-one', postId: 'reply' })!;
    expect(reply.threads[0].posts.map((item) => item.postId)).toEqual(['topic', 'other-reply']);
    expect(bbsRemoveDeletedItem(reply, { threadId: 'thread-one', postId: 'topic' })!.threads).toEqual([snapshot.threads[1]]);
    expect(bbsRemoveDeletedItem(null, { threadId: 'missing' })).toBeNull();
  });
});

describe('BBS unavailable notices stay outside byte versions', () => {
  const thread = (overrides: Partial<BbsThread> = {}): BbsThread => ({
    threadId: 'thread-one', visibility: 'broadcast', projectTags: [], projectTagLabels: [],
    createdByProject: 'project-one', createdByProjectLabel: 'Project', updatedAt: '2026-09-12T00:00:00Z',
    latestPostId: 'topic', isNew: false, relevant: true, posts: [], ...overrides,
  });
  const snapshot = (item: BbsThread): BbsSnapshot => ({ projectId: 'project-one', projectDisplayName: 'Project',
    root: '/tmp/bbs', newCount: 0, threads: [item] });

  it('prefers real posts, deduplicates IDs and only refines an unknown kind without mutating input', () => {
    const item = thread({ posts: [post('real', a, 'topic')], unavailablePosts: [
      { postId: 'real', reason: 'too_large_to_sync', kind: 'topic' },
      { postId: 'unknown', reason: 'too_large_to_sync' },
      { postId: 'unknown', reason: 'too_large_to_sync', kind: 'reply' },
      { postId: 'unknown', reason: 'too_large_to_sync', kind: 'topic' },
      { postId: 'unparsed-id', reason: 'too_large_to_sync' },
    ] });
    const before = structuredClone(item);
    expect(bbsUnavailablePosts(item)).toEqual([
      { postId: 'unknown', reason: 'too_large_to_sync', kind: 'reply' },
      { postId: 'unparsed-id', reason: 'too_large_to_sync' },
    ]);
    expect(item).toEqual(before);
    expect(bbsUnavailablePosts(thread())).toEqual([]);
  });

  it('does not impose a total notice count limit or invent versions for a growing board', () => {
    const item = thread({ unavailablePosts: Array.from({ length: 5000 }, (_, index) => ({
      postId: `post-${index}`, reason: 'too_large_to_sync',
    })) });
    const notices = bbsUnavailablePosts(item);
    expect(notices).toHaveLength(5000);
    expect(notices[4999].postId).toBe('post-4999');
    expect(bbsThreadVersions(item.posts)).toEqual({ topics: [], replies: [], replyCount: 0 });
  });

  it('keeps a neutral thread after the last real reply is deleted and never drops a notice on version deletion', () => {
    const item = thread({ posts: [post('reply', a)], unavailablePosts: [
      { postId: 'topic', reason: 'too_large_to_sync', kind: 'topic' },
    ] });
    const before = snapshot(item);
    const after = bbsRemoveDeletedItem(before, { threadId: item.threadId, postId: 'reply' })!;
    expect(after.threads).toHaveLength(1);
    expect(after.threads[0].posts).toEqual([]);
    expect(after.threads[0].unavailablePosts).toEqual(item.unavailablePosts);
    expect(after.threads[0].latestPostId).toBe('topic');
    expect(bbsRemoveDeletedItem(after, { threadId: item.threadId, postId: 'topic', versionId: a })!.threads[0]).toBe(after.threads[0]);
    expect(bbsRemoveDeletedItem(after, { threadId: item.threadId, postId: 'topic' })!.threads).toEqual([]);
    expect(bbsRemoveDeletedItem(before, { threadId: item.threadId })!.threads).toEqual([]);
  });
});
