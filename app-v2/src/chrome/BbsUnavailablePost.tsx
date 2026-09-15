import { BBS_UNAVAILABLE_MESSAGE } from '../bbs-post-versions';
import type { BbsUnavailablePost as UnavailablePost } from '../pty-client';

/** Read-only notice: no guessed author, version, attachment or post action. */
export function BbsUnavailablePost({ post }: { post: UnavailablePost }) {
  return (
    <div className="bbs-unavailable-post" role="note" aria-label="Unavailable BBS post" data-bbs-post-id={post.postId}>
      <span>{BBS_UNAVAILABLE_MESSAGE}</span>
      <code className="bbs-unavailable-id">{post.postId}</code>
    </div>
  );
}
