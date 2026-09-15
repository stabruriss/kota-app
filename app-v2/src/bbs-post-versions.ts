import type { BbsDeleteRequest, BbsPost, BbsSnapshot, BbsThread, BbsUnavailablePost } from './pty-client';

export const BBS_UNAVAILABLE_MESSAGE = 'File size too large to sync.';

/** Notices have no byte-version identity. Keep them out of all Fork helpers. */
export function bbsUnavailablePosts(thread: BbsThread): BbsUnavailablePost[] {
  const present = new Set(thread.posts.map((post) => post.postId));
  const notices = new Map<string, BbsUnavailablePost>();
  for (const item of thread.unavailablePosts ?? []) {
    if (!item || typeof item.postId !== 'string' || !item.postId
      || item.reason !== 'too_large_to_sync' || present.has(item.postId)) continue;
    const kind = item.kind === 'topic' || item.kind === 'reply' ? item.kind : undefined;
    const previous = notices.get(item.postId);
    if (!previous || (!previous.kind && kind)) {
      notices.set(item.postId, { postId: item.postId, reason: 'too_large_to_sync', ...(kind ? { kind } : {}) });
    }
  }
  return [...notices.values()];
}

/** Read projection only. The backend owns hashing, validation and Fork storage. */
export type VersionedBbsPost = BbsPost & { versionId?: string | null };

export interface BbsPostVersionView {
  post: VersionedBbsPost;
  /** Stable selection/React key. A display ordinal must never identify a version. */
  key: string;
  /** One-based, and present only when this logical post has multiple versions. */
  forkNumber: number | null;
}

export interface BbsReplyFloorView {
  postId: string;
  floor: number;
  versions: BbsPostVersionView[];
}

export interface BbsThreadVersionsView {
  topics: BbsPostVersionView[];
  replies: BbsReplyFloorView[];
  /** Logical replies, not the number of stored versions. */
  replyCount: number;
}

export function bbsPostVersionKey(post: VersionedBbsPost): string {
  return JSON.stringify([post.threadId, post.postId, post.versionId ?? null]);
}

function versionViews(posts: readonly VersionedBbsPost[]): BbsPostVersionView[] {
  // Hash IDs are opaque here; never compare/rewrite Markdown to infer a Fork.
  const unique = [...new Map(posts.map((post) => [bbsPostVersionKey(post), post])).values()];
  unique.sort((a, b) => {
    const left = a.versionId ?? '', right = b.versionId ?? '';
    return left < right ? -1 : left > right ? 1 : 0;
  });
  return unique.map((post, index) => ({
    post, key: bbsPostVersionKey(post), forkNumber: unique.length > 1 ? index + 1 : null,
  }));
}

/**
 * Keep existing logical reply order. Different postIds are separate replies;
 * only versions of the same postId share a floor. All root versions see the
 * same reply pool, not branches. Does not mutate the snapshot or perform IO.
 */
export function bbsThreadVersions(posts: readonly VersionedBbsPost[]): BbsThreadVersionsView {
  // A thread can have received replies while its root is too large to sync.
  // Never turn the first available reply into an invented OP.
  const rootId = posts.find((post) => post.kind === 'topic')?.postId;
  const groups = new Map<string, VersionedBbsPost[]>();
  for (const post of posts) {
    const versions = groups.get(post.postId);
    if (versions) versions.push(post);
    else groups.set(post.postId, [post]);
  }
  const topics = rootId ? versionViews(groups.get(rootId) ?? []) : [];
  const replies: BbsReplyFloorView[] = [];
  for (const [postId, versions] of groups) {
    if (postId === rootId) continue;
    replies.push({ postId, floor: replies.length + 2, versions: versionViews(versions) });
  }
  return { topics, replies, replyCount: replies.length };
}

export function bbsSelectedTopic(view: BbsThreadVersionsView, selectedKey: string | null): BbsPostVersionView | null {
  return view.topics.find((version) => version.key === selectedKey) ?? view.topics[0] ?? null;
}

/** Only a visible Fork action deletes a single version. Ordinary Delete retains
 * its logical-post/thread meaning even when the post carries a version hash. */
export function bbsVersionDeleteTarget(version: BbsPostVersionView): {
  threadId: string; postId: string; versionId?: string;
} | null {
  // A missing/invalid version reference must not widen a Fork delete to the
  // whole logical post. The backend should never emit this projection.
  if (version.forkNumber !== null && !/^[a-f0-9]{64}$/.test(version.post.versionId ?? '')) return null;
  return {
    threadId: version.post.threadId,
    postId: version.post.postId,
    ...(version.forkNumber !== null && version.post.versionId ? { versionId: version.post.versionId } : {}),
  };
}

/** A separate, explicitly labelled action, never a fallback for an invalid Fork. */
export function bbsLogicalDeleteTarget(post: BbsPost): BbsDeleteRequest {
  return { threadId: post.threadId, postId: post.postId };
}

/** Optimistic projection only AFTER the backend confirms deletion. Keep the
 * frozen request's scope even if the snapshot now has fewer/different Forks. */
export function bbsRemoveDeletedItem(snapshot: BbsSnapshot | null, request: BbsDeleteRequest): BbsSnapshot | null {
  if (!snapshot) return snapshot;
  const exactVersion = request.versionId != null;
  const threads = snapshot.threads.flatMap((thread) => {
    if (thread.threadId !== request.threadId) return [thread];
    if (!request.postId || (!exactVersion && thread.posts.some((post) => post.postId === request.postId && post.kind === 'topic'))) return [];
    const posts = thread.posts.filter((post) => post.postId !== request.postId
      || (exactVersion && post.versionId !== request.versionId));
    const unavailablePosts = exactVersion ? thread.unavailablePosts
      : thread.unavailablePosts?.filter((post) => post.postId !== request.postId);
    if (posts.length === thread.posts.length && unavailablePosts?.length === thread.unavailablePosts?.length) return [thread];
    const remainingNotices = bbsUnavailablePosts({ ...thread, posts, unavailablePosts });
    if (!remainingNotices.length && (posts.length === 0
      || (thread.posts.some((post) => post.kind === 'topic') && !posts.some((post) => post.kind === 'topic')))) return [];
    const latest = posts.reduce<BbsPost | null>((current, post) => !current || post.createdAt > current.createdAt ? post : current, null);
    return [{ ...thread, posts, ...(unavailablePosts ? { unavailablePosts } : {}),
      latestPostId: latest?.postId ?? remainingNotices[0]?.postId ?? thread.latestPostId,
      updatedAt: latest?.createdAt ?? thread.updatedAt,
      isNew: posts.some((post) => post.state === 'new') }];
  });
  return { ...snapshot, threads, newCount: threads.filter((thread) => thread.isNew).length };
}
