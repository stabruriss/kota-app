import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent, RefObject } from 'react';
import { createPortal } from 'react-dom';
import {
  workspaceDiffChanges,
  workspaceFileDiff,
  workspaceOpenTreePath,
  workspaceRevealTreePath,
} from '../pty-client';
import type {
  WorkspaceDiffChangeEntry,
  WorkspaceDiffScope,
  WorkspaceFileDiffResult,
  WorkspaceFileDiffSegment,
  WorkspaceTreeChangeParticipant,
} from '../types/tree';
import { HeroAvatarArt } from './HeroAvatarPicker';

export interface DiffMedalProps {
  projectId: string;
  sourceRoot?: string | null;
  scope: WorkspaceDiffScope;
  onClose: () => void;
}

interface DiffLoadState {
  loading: boolean;
  error: string | null;
  result: WorkspaceFileDiffResult | null;
}

interface DiffContextMenuState {
  x: number;
  y: number;
  entry: WorkspaceDiffChangeEntry;
}

export function DiffMedal({
  projectId,
  sourceRoot,
  scope,
  onClose,
}: DiffMedalProps) {
  const [changes, setChanges] = useState<WorkspaceDiffChangeEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expandedPath, setExpandedPath] = useState<string | null>(null);
  const [diffs, setDiffs] = useState<Map<string, DiffLoadState>>(() => new Map());
  const [contextMenu, setContextMenu] = useState<DiffContextMenuState | null>(null);
  const [activeChangeIndex, setActiveChangeIndex] = useState(0);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const scrollLockUntilRef = useRef(0);
  const scopeKey = useMemo(() => JSON.stringify(scope), [scope]);

  const loadChanges = useCallback(async () => {
    setLoading(true);
    setError(null);
    setContextMenu(null);
    try {
      const next = await workspaceDiffChanges({ projectId, scope });
      setChanges(next);
      setExpandedPath(null);
    } catch (err) {
      setChanges([]);
      setExpandedPath(null);
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, [projectId, scope]);

  const loadDiff = useCallback((path: string) => {
    setDiffs((current) => {
      const existing = current.get(path);
      if (existing?.loading || existing?.result) return current;
      const next = new Map(current);
      next.set(path, { loading: true, error: null, result: null });
      return next;
    });
    void workspaceFileDiff({ projectId, relativePath: path })
      .then((result) => {
        setDiffs((current) => {
          const next = new Map(current);
          next.set(path, { loading: false, error: null, result });
          return next;
        });
      })
      .catch((err) => {
        setDiffs((current) => {
          const next = new Map(current);
          next.set(path, { loading: false, error: String(err), result: null });
          return next;
        });
      });
  }, [projectId]);

  useEffect(() => {
    setDiffs(new Map());
    setExpandedPath(null);
    setActiveChangeIndex(0);
    void loadChanges();
  }, [loadChanges, scopeKey]);

  useEffect(() => {
    if (!expandedPath) return;
    loadDiff(expandedPath);
  }, [expandedPath, loadDiff]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (document.querySelector('.tavern-page')) return;
      event.preventDefault();
      event.stopPropagation();
      if (contextMenu) setContextMenu(null);
      else onClose();
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [contextMenu, onClose]);

  useEffect(() => {
    if (!contextMenu) return undefined;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Element && target.closest('[data-diff-context-menu="true"]')) return;
      setContextMenu(null);
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    return () => document.removeEventListener('pointerdown', onPointerDown, true);
  }, [contextMenu]);

  const toggleExpanded = (path: string) => {
    setExpandedPath((current) => (current === path ? null : path));
    setActiveChangeIndex(0);
    if (expandedPath !== path) loadDiff(path);
  };

  const runAction = async (action: 'open' | 'reveal' | 'copy') => {
    const entry = contextMenu?.entry;
    if (!entry) return;
    try {
      if (action === 'open') {
        await workspaceOpenTreePath({ projectId, rootKind: 'projectFiles', relativePath: entry.path });
      } else if (action === 'reveal') {
        await workspaceRevealTreePath({ projectId, rootKind: 'projectFiles', relativePath: entry.path });
      } else if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(entry.absolutePath);
      }
    } catch (err) {
      console.error(`[diff-medal] ${action} failed`, err);
      if (typeof window !== 'undefined' && typeof window.alert === 'function') {
        window.alert(`${action} failed: ${err}`);
      }
    } finally {
      setContextMenu(null);
    }
  };

  const expandedState = expandedPath ? (diffs.get(expandedPath) ?? null) : null;
  const expandedResult = expandedState?.result ?? null;
  const changeAnchorIds = useMemo(
    () => (expandedPath && expandedResult ? collectDiffChangeAnchorIds(expandedPath, expandedResult) : []),
    [expandedPath, expandedResult],
  );
  const activeAnchorId = changeAnchorIds[activeChangeIndex] ?? null;

  const goToChange = useCallback((index: number, behavior: ScrollBehavior = 'smooth') => {
    const body = bodyRef.current;
    if (!body || changeAnchorIds.length === 0) return;
    const nextIndex = Math.min(Math.max(index, 0), changeAnchorIds.length - 1);
    const anchor = getDiffAnchorElement(body, changeAnchorIds[nextIndex]);
    if (!anchor) return;
    setActiveChangeIndex(nextIndex);
    scrollLockUntilRef.current = performance.now() + (behavior === 'smooth' ? 430 : 80);
    const bodyRect = body.getBoundingClientRect();
    const anchorRect = anchor.getBoundingClientRect();
    const targetTop = body.scrollTop + anchorRect.top - bodyRect.top - 84;
    body.scrollTo({ top: Math.max(0, targetTop), behavior });
    window.setTimeout(() => {
      scrollLockUntilRef.current = 0;
    }, behavior === 'smooth' ? 440 : 90);
  }, [changeAnchorIds]);

  const handleBodyScroll = useCallback(() => {
    if (performance.now() < scrollLockUntilRef.current || changeAnchorIds.length === 0) return;
    const body = bodyRef.current;
    if (!body) return;
    const bodyRect = body.getBoundingClientRect();
    let nextIndex = activeChangeIndex;
    for (let index = 0; index < changeAnchorIds.length; index += 1) {
      const anchor = getDiffAnchorElement(body, changeAnchorIds[index]);
      if (!anchor) continue;
      const rect = anchor.getBoundingClientRect();
      const middle = rect.top + rect.height / 2;
      if (middle >= bodyRect.top + 58 && middle <= bodyRect.bottom - 24) {
        nextIndex = index;
        break;
      }
    }
    if (nextIndex !== activeChangeIndex) {
      setActiveChangeIndex(nextIndex);
    }
  }, [activeChangeIndex, changeAnchorIds]);

  useEffect(() => {
    setActiveChangeIndex(0);
    if (changeAnchorIds.length === 0) return undefined;
    const frame = window.requestAnimationFrame(() => goToChange(0, 'auto'));
    return () => window.cancelAnimationFrame(frame);
  }, [changeAnchorIds, goToChange]);

  const header = scopeHeader(scope, changes.length);
  const contextMenuPortal = contextMenu && typeof document !== 'undefined'
    ? createPortal(
      <div
        data-diff-context-menu="true"
        className="tree-context-menu diff-medal-context-menu"
        style={contextMenuPosition(contextMenu.x, contextMenu.y)}
        role="menu"
        onPointerDown={(event) => event.stopPropagation()}
        onMouseDown={(event) => event.stopPropagation()}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="diff-medal-context-path">{contextMenu.entry.path}</div>
        <button type="button" role="menuitem" onClick={() => void runAction('open')}>
          Open Default App
        </button>
        <button type="button" role="menuitem" onClick={() => void runAction('reveal')}>
          Reveal in Finder
        </button>
        <button type="button" role="menuitem" onClick={() => void runAction('copy')}>
          Copy Full Path
        </button>
      </div>,
      document.body,
    )
    : null;

  return (
    <>
      <div className="diff-medal-layer" role="presentation" onMouseDown={onClose}>
        <section
          className="diff-medal project-rules-medal"
          role="dialog"
          aria-modal="true"
          aria-label="Project files diff"
          onMouseDown={(event) => event.stopPropagation()}
          data-testid="diff-medal"
        >
          <header className="diff-medal-head project-rules-medal-head">
            <div>
              <div className="diff-medal-kicker project-rules-medal-kicker">
                Project files · diff <span>{header.pill}</span>
              </div>
              <h2>{header.title}</h2>
              <small title={sourceRoot ?? undefined}>{header.crumb}</small>
            </div>
            <button type="button" className="project-rules-close" onClick={onClose} aria-label="Close diff medal">
              ×
            </button>
          </header>

          <div className="diff-medal-body" ref={bodyRef} onScroll={handleBodyScroll}>
            {changeAnchorIds.length > 0 && (
              <DiffChangeNavigator
                activeIndex={activeChangeIndex}
                total={changeAnchorIds.length}
                activeAnchorId={activeAnchorId}
                bodyRef={bodyRef}
                resetKey={expandedPath ?? 'none'}
                onGoTo={goToChange}
              />
            )}
            {loading && <div className="diff-medal-empty loading">Loading diff...</div>}
            {error && (
              <div className="diff-medal-empty error">
                <b>Could not load changes.</b>
                <span>{error}</span>
                <button type="button" onClick={() => void loadChanges()}>Retry</button>
              </div>
            )}
            {!loading && !error && changes.length === 0 && (
              <div className="diff-medal-empty">No changes in this scope.</div>
            )}
            {!loading && !error && changes.map((entry) => {
              const isExpanded = expandedPath === entry.path;
              const state = diffs.get(entry.path) ?? null;
              return (
                <article
                  key={entry.path}
                  className={[
                    'diff-medal-row',
                    `status-${entry.fileChange.status}`,
                    isExpanded ? 'expanded' : '',
                  ].filter(Boolean).join(' ')}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    event.stopPropagation();
                    setContextMenu({ x: event.clientX, y: event.clientY, entry });
                  }}
                >
                  <button
                    type="button"
                    className="diff-medal-row-summary"
                    onClick={() => toggleExpanded(entry.path)}
                    aria-expanded={isExpanded}
                  >
                    <span className={`diff-medal-status ${entry.fileChange.status}`}>
                      {statusLabel(entry.fileChange.status)}
                    </span>
                    <span className="diff-medal-path">
                      <span>{dirname(entry.path)}</span>
                      <b>{basename(entry.path)}</b>
                    </span>
                    <OwnerStack participants={entry.fileChange.participants} />
                    <LineStat added={entry.fileChange.addedLines} deleted={entry.fileChange.deletedLines} />
                    <span className="diff-medal-chevron" aria-hidden>›</span>
                  </button>
                  {isExpanded && (
	                    <DiffDetails
	                      path={entry.path}
	                      state={state}
	                      activeAnchorId={activeAnchorId}
	                      onRetry={() => loadDiff(entry.path)}
	                    />
                  )}
                </article>
              );
            })}
          </div>

          <footer className="diff-medal-foot">
            Right-click any file for Open Default App, Reveal in Finder, or Copy Full Path.
          </footer>
        </section>
      </div>
      {contextMenuPortal}
    </>
  );
}

function contextMenuPosition(x: number, y: number): { left: number; top: number } {
  if (typeof window === 'undefined') return { left: x, top: y };
  const margin = 10;
  const menuWidth = 240;
  const menuHeight = 142;
  return {
    left: Math.min(Math.max(margin, x), Math.max(margin, window.innerWidth - menuWidth - margin)),
    top: Math.min(Math.max(margin, y), Math.max(margin, window.innerHeight - menuHeight - margin)),
  };
}

function DiffChangeNavigator({
  activeIndex,
  total,
  activeAnchorId,
  bodyRef,
  resetKey,
  onGoTo,
}: {
  activeIndex: number;
  total: number;
  activeAnchorId: string | null;
  bodyRef: RefObject<HTMLDivElement | null>;
  resetKey: string;
  onGoTo: (index: number) => void;
}) {
  const navRef = useRef<HTMLDivElement | null>(null);
  const readRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<{
    pointerId: number;
    dx: number;
    dy: number;
    moved: boolean;
    target: HTMLElement;
  } | null>(null);
  const [manual, setManual] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const totalLabel = String(total);
  const currentLabel = String(activeIndex + 1).padStart(totalLabel.length, '0');

  const fitReadout = useCallback(() => {
    const read = readRef.current;
    const disc = read?.closest('.diff-change-nav-disc') as HTMLElement | null;
    if (!read || !disc) return;
    read.style.transform = 'translate(-50%, -50%) scale(1)';
    const available = disc.clientWidth * 0.82;
    const width = read.scrollWidth;
    read.style.transform = `translate(-50%, -50%) scale(${width > available ? available / width : 1})`;
  }, []);

  /* Clamp a viewport-coordinate position so the nav stays inside the diff
     body's visible rect (with a small margin), never escaping the medal. */
  const clampToBodyBounds = useCallback((left: number, top: number) => {
    const nav = navRef.current;
    const width = nav?.offsetWidth ?? 91;
    const height = nav?.offsetHeight ?? 42;
    const margin = 8;
    let minLeft = margin;
    let maxLeft = window.innerWidth - width - margin;
    let minTop = margin;
    let maxTop = window.innerHeight - height - margin;
    const bodyRect = bodyRef.current?.getBoundingClientRect();
    if (bodyRect) {
      minLeft = Math.max(minLeft, bodyRect.left + margin);
      maxLeft = Math.min(maxLeft, bodyRect.right - width - margin);
      minTop = Math.max(minTop, bodyRect.top + margin);
      maxTop = Math.min(maxTop, bodyRect.bottom - height - margin);
    }
    return {
      left: Math.round(Math.min(Math.max(left, minLeft), Math.max(minLeft, maxLeft))),
      top: Math.round(Math.min(Math.max(top, minTop), Math.max(minTop, maxTop))),
    };
  }, [bodyRef]);

  const followActiveAnchor = useCallback((force = false) => {
    if (manual && !force) return;
    const body = bodyRef.current;
    const anchor = body && activeAnchorId ? getDiffAnchorElement(body, activeAnchorId) : null;
    const nav = navRef.current;
    if (!body || !anchor || !nav) return;
    const bodyRect = body.getBoundingClientRect();
    const anchorRect = anchor.getBoundingClientRect();
    const width = nav.offsetWidth;
    const height = nav.offsetHeight;
    const gap = 13;
    let left = bodyRect.right - width - gap;
    if (anchorRect.right > bodyRect.right - width - gap - 28) {
      left = bodyRect.left + gap;
    }
    const centered = anchorRect.top + anchorRect.height / 2 - height / 2;
    setPosition(clampToBodyBounds(left, centered));
  }, [activeAnchorId, bodyRef, clampToBodyBounds, manual]);

  useLayoutEffect(() => {
    fitReadout();
  }, [currentLabel, fitReadout, totalLabel]);

  useEffect(() => {
    setManual(false);
  }, [resetKey]);

  useEffect(() => {
    if (manual) return undefined;
    const frame = window.requestAnimationFrame(() => followActiveAnchor());
    return () => window.cancelAnimationFrame(frame);
  }, [activeAnchorId, activeIndex, followActiveAnchor, manual, total]);

  useEffect(() => {
    const body = bodyRef.current;
    if (!body) return undefined;
    const reflow = () => {
      if (!manual) followActiveAnchor();
    };
    const onResize = () => {
      if (manual) {
        // Manual position is viewport-fixed; re-clamp so the medal's new
        // bounds never strand the nav outside the visible body.
        setPosition((current) => (current ? clampToBodyBounds(current.left, current.top) : current));
      } else {
        followActiveAnchor();
      }
    };
    body.addEventListener('scroll', reflow, { passive: true });
    window.addEventListener('resize', onResize);
    return () => {
      body.removeEventListener('scroll', reflow);
      window.removeEventListener('resize', onResize);
    };
  }, [bodyRef, clampToBodyBounds, followActiveAnchor, manual]);

  const beginDrag = (event: ReactPointerEvent<HTMLElement>) => {
    if ((event.target as HTMLElement).closest('.diff-change-nav-button')) return;
    const nav = navRef.current;
    if (!nav) return;
    const rect = nav.getBoundingClientRect();
    dragRef.current = {
      pointerId: event.pointerId,
      dx: event.clientX - rect.left,
      dy: event.clientY - rect.top,
      moved: false,
      target: event.currentTarget,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
    setDragging(true);
    event.preventDefault();
  };

  const moveDrag = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    const nav = navRef.current;
    if (!drag || !nav || event.pointerId !== drag.pointerId) return;
    drag.moved = true;
    setPosition(clampToBodyBounds(event.clientX - drag.dx, event.clientY - drag.dy));
  };

  const endDrag = (event: ReactPointerEvent<HTMLElement>) => {
    const drag = dragRef.current;
    if (!drag || event.pointerId !== drag.pointerId) return;
    if (drag.moved) setManual(true);
    drag.target.releasePointerCapture(event.pointerId);
    dragRef.current = null;
    setDragging(false);
  };

  const resumeFollow = () => {
    setManual(false);
    window.requestAnimationFrame(() => followActiveAnchor(true));
  };

  // Portaled to <body>: the medal's backdrop-filter makes it the containing
  // block for fixed-position descendants, which skewed every viewport-based
  // coordinate here (drag offset + clamping) and let the medal's
  // overflow:hidden clip the nav. On <body> the fixed math is true again.
  return createPortal(
    <div
      ref={navRef}
      className={`diff-change-nav ${dragging ? 'dragging' : ''}`}
      style={position ? { left: position.left, top: position.top } : { visibility: 'hidden' }}
      role="group"
      aria-label="Diff change navigation"
    >
      <div
        className="diff-change-nav-bar"
        onPointerDown={beginDrag}
        onPointerMove={moveDrag}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
      >
        <button
          type="button"
          className="diff-change-nav-button"
          aria-label="Previous change"
          disabled={activeIndex <= 0}
          onClick={() => onGoTo(activeIndex - 1)}
        >
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden>
            <path d="M18 6.5v11a.6.6 0 0 1-.93.5L9 12.83V17.5a.6.6 0 0 1-.93.5l-4.9-5.5a.6.6 0 0 1 0-1l4.9-5.5A.6.6 0 0 1 9 6.5v4.67l8.07-5.17A.6.6 0 0 1 18 6.5Z" />
          </svg>
        </button>
        <button
          type="button"
          className="diff-change-nav-button"
          aria-label="Next change"
          disabled={activeIndex >= total - 1}
          onClick={() => onGoTo(activeIndex + 1)}
        >
          <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden>
            <path d="M6 6.5v11a.6.6 0 0 0 .93.5L15 12.83V17.5a.6.6 0 0 0 .93.5l4.9-5.5a.6.6 0 0 0 0-1l-4.9-5.5A.6.6 0 0 0 15 6.5v4.67L6.93 6A.6.6 0 0 0 6 6.5Z" />
          </svg>
        </button>
      </div>
      <div
        className="diff-change-nav-disc"
        title="Drag to move. Double-click to follow the current change."
        onPointerDown={beginDrag}
        onPointerMove={moveDrag}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onDoubleClick={resumeFollow}
      >
        <div ref={readRef} className="diff-change-nav-read">
          <span className="cur">{currentLabel}</span>
          <span className="sep">/</span>
          <span className="tot">{totalLabel}</span>
        </div>
      </div>
    </div>,
    document.body,
  );
}

function DiffDetails({
  path,
  state,
  activeAnchorId,
  onRetry,
}: {
  path: string;
  state: DiffLoadState | null;
  activeAnchorId: string | null;
  onRetry: () => void;
}) {
  if (!state || state.loading) {
    return <div className="diff-medal-details skeleton">Loading diff...</div>;
  }
  if (state.error) {
    return (
      <div className="diff-medal-details error">
        <span>Could not load diff for {basename(path)}.</span>
        <button type="button" onClick={onRetry}>Retry</button>
      </div>
    );
  }
  const segments = state.result?.segments ?? [];
  if (segments.length === 0) {
    return <div className="diff-medal-details muted">No diff content available.</div>;
  }
  return (
    <div className="diff-medal-details">
      {segments.map((segment, segmentIndex) => (
        <section key={`${segment.kind}:${segment.actorId}:${segment.status}`} className="diff-medal-segment">
          <header>
            <OwnerAvatar owner={segment} />
            <b title={segment.displayName}>{ownerLabel(segment)}</b>
            <LineStat added={segment.addedLines} deleted={segment.deletedLines} compact />
          </header>
          {segment.binary ? (
            <div className="diff-medal-binary">Binary file diff is not rendered here.</div>
          ) : segment.hunks.length === 0 ? (
            <div className="diff-medal-binary">No textual hunks available.</div>
          ) : segment.hunks.map((hunk, hunkIndex) => (
            <DiffHunkView
              key={`${hunk.header}:${hunkIndex}`}
              path={path}
              segmentIndex={segmentIndex}
              hunkIndex={hunkIndex}
              hunk={hunk}
              activeAnchorId={activeAnchorId}
            />
          ))}
          {segment.truncated && <div className="diff-medal-omitted">Diff truncated for performance.</div>}
        </section>
      ))}
    </div>
  );
}

function DiffHunkView({
  path,
  segmentIndex,
  hunkIndex,
  hunk,
  activeAnchorId,
}: {
  path: string;
  segmentIndex: number;
  hunkIndex: number;
  hunk: WorkspaceFileDiffSegment['hunks'][number];
  activeAnchorId: string | null;
}) {
  let inChange = false;
  let blockIndex = -1;
  return (
    <div className="diff-medal-hunk">
      <div className="diff-medal-hunk-head">{hunk.header}</div>
      <pre>
        {hunk.lines.map((line, lineIndex) => {
          const isChange = isDiffChangeLine(line.kind);
          let anchorId: string | null = null;
          if (isChange && !inChange) {
            blockIndex += 1;
            anchorId = diffChangeAnchorId(path, segmentIndex, hunkIndex, blockIndex);
          }
          inChange = isChange;
          return (
            <span
              key={lineIndex}
              className={[
                'diff-line',
                line.kind,
                anchorId ? 'anchor' : '',
                anchorId && anchorId === activeAnchorId ? 'active' : '',
              ].filter(Boolean).join(' ')}
              data-diff-change-anchor={anchorId ?? undefined}
            >
              <span className="diff-line-number old">{line.oldLine ?? ''}</span>
              <span className="diff-line-number new">{line.newLine ?? ''}</span>
              <i>{lineSign(line.kind)}</i>
              <span className="diff-line-text">{line.text}</span>
            </span>
          );
        })}
      </pre>
      {hunk.omittedLines > 0 && (
        <div className="diff-medal-omitted">Remaining {hunk.omittedLines} lines omitted.</div>
      )}
    </div>
  );
}

function OwnerStack({ participants }: { participants: WorkspaceTreeChangeParticipant[] }) {
  const shown = participants.slice(0, 3);
  const extra = participants.length - shown.length;
  return (
    <span className="diff-medal-owners" aria-label={participants.map((participant) => ownerLabel(participant)).join(', ')}>
      {shown.map((participant) => (
        <OwnerAvatar
          key={`${participant.kind}:${participant.actorId}:${participant.status}`}
          owner={participant}
          title={`${ownerLabel(participant)} · ${participant.status}`}
        />
      ))}
      {extra > 0 && <span className="diff-owner-more">+{extra}</span>}
    </span>
  );
}

function OwnerAvatar({
  owner,
  title,
}: {
  owner: Pick<WorkspaceTreeChangeParticipant, 'kind' | 'displayName' | 'provider' | 'avatarId'>;
  title?: string;
}) {
  if (owner.kind === 'human') {
    return <span className="diff-owner-avatar human" title={title}>H</span>;
  }
  return (
    <HeroAvatarArt
      avatarId={owner.avatarId}
      provider={owner.provider}
      className="diff-owner-avatar"
      title={title}
    />
  );
}

function LineStat({
  added,
  deleted,
  compact,
}: {
  added?: number | null;
  deleted?: number | null;
  compact?: boolean;
}) {
  if (added == null && deleted == null) {
    return <span className={`diff-line-stat ${compact ? 'compact' : ''}`} />;
  }
  return (
    <span className={`diff-line-stat ${compact ? 'compact' : ''}`}>
      {deleted ? <span className="del">-{deleted}</span> : null}
      {added ? <span className="add">+{added}</span> : null}
      {!added && !deleted ? <span className="zero">0</span> : null}
    </span>
  );
}

function scopeHeader(scope: WorkspaceDiffScope, count: number): { pill: string; title: string; crumb: string } {
  if (scope.type === 'all') {
    return { pill: 'ALL', title: 'All Changes', crumb: `${count} changed file${count === 1 ? '' : 's'}` };
  }
  if (scope.type === 'folder') {
    const prefix = scope.prefix.replace(/^\/+|\/+$/g, '');
    return {
      pill: 'FOLDER',
      title: `${basename(prefix) || 'Project Files'}/`,
      crumb: `${prefix || 'Project Files'} · ${count} changed file${count === 1 ? '' : 's'}`,
    };
  }
  return {
    pill: 'FILE',
    title: basename(scope.path),
    crumb: `${scope.path} · ${count || 1} file`,
  };
}

function statusLabel(status: string): string {
  if (status === 'added') return 'added';
  if (status === 'deleted') return 'deleted';
  if (status === 'untracked') return 'untracked';
  return 'changed';
}

function ownerLabel(owner: Pick<WorkspaceTreeChangeParticipant, 'kind' | 'displayName' | 'aka'>): string {
  if (owner.kind === 'human') return 'Human';
  const aka = owner.aka?.trim();
  if (aka) return aka;
  const displayName = owner.displayName.trim();
  return displayName.split(' v. ')[0]?.trim() || displayName || 'Agent';
}

function dirname(path: string): string {
  const index = path.lastIndexOf('/');
  return index >= 0 ? `${path.slice(0, index + 1)}` : '';
}

function basename(path: string): string {
  const normalized = path.replace(/\/+$/g, '');
  const index = normalized.lastIndexOf('/');
  return index >= 0 ? normalized.slice(index + 1) : normalized;
}

function collectDiffChangeAnchorIds(path: string, result: WorkspaceFileDiffResult): string[] {
  const anchors: string[] = [];
  result.segments.forEach((segment, segmentIndex) => {
    segment.hunks.forEach((hunk, hunkIndex) => {
      let inChange = false;
      let blockIndex = -1;
      hunk.lines.forEach((line) => {
        const isChange = isDiffChangeLine(line.kind);
        if (isChange && !inChange) {
          blockIndex += 1;
          anchors.push(diffChangeAnchorId(path, segmentIndex, hunkIndex, blockIndex));
        }
        inChange = isChange;
      });
    });
  });
  return anchors;
}

function getDiffAnchorElement(root: HTMLElement, anchorId: string): HTMLElement | null {
  const anchors = root.querySelectorAll<HTMLElement>('[data-diff-change-anchor]');
  for (const anchor of anchors) {
    if (anchor.dataset.diffChangeAnchor === anchorId) return anchor;
  }
  return null;
}

function diffChangeAnchorId(path: string, segmentIndex: number, hunkIndex: number, blockIndex: number): string {
  return `${path}::${segmentIndex}:${hunkIndex}:${blockIndex}`;
}

function isDiffChangeLine(kind: string): boolean {
  return kind === 'add' || kind === 'del';
}

function lineSign(kind: string): string {
  if (kind === 'add') return '+';
  if (kind === 'del') return '-';
  return ' ';
}
