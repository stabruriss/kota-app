/** Top bar — 48px brass band hosting macOS traffic lights (left),
 *  Chrome-style project tabs (middle), and the Tavern entry (right).
 *
 *  `data-tauri-drag-region` on <header> makes the bar drag the window;
 *  interactive children opt out via `data-tauri-drag-region="false"`.
 *  Traffic lights are positioned by tauri.conf.json (x:14, y:18) —
 *  the 84px left padding reserves empty space for them to float over.
 *
 *  Phase 3 design reference:
 *    .context/design-brief/03-UI-expansion-requirements.md §7 (S13)  */

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import tabMoveLeftIcon from '../assets/tavern/icons/tab-move-left.svg';
import tabMoveRightIcon from '../assets/tavern/icons/tab-move-right.svg';
import tavernEntryIcon from '../assets/tavern/icons/tavern-entry.svg';
import type { Project, ProjectId } from '../types/project';

interface TopBarProps {
  projects: readonly Project[];
  activeProjectId: ProjectId | null;
  projectUnreadCounts?: Readonly<Record<ProjectId, number>>;
  onSelectProject: (id: ProjectId) => void;
  onReorderProjects?: (projectIds: ProjectId[]) => void;
  onNewProject: () => void;
  onCloseProject: (id: ProjectId) => void;
  onOpenTavern: () => void;
  tavernOpen?: boolean;
  tavernPreparing?: boolean;
  hideProjectTabs?: boolean;
}

export function TopBar({
  projects,
  activeProjectId,
  projectUnreadCounts,
  onSelectProject,
  onReorderProjects,
  onNewProject,
  onCloseProject,
  onOpenTavern,
  tavernOpen = false,
  tavernPreparing = false,
  hideProjectTabs = false,
}: TopBarProps) {
  const tavernButtonLabel = tavernPreparing
    ? 'Preparing Tavern'
    : tavernOpen
    ? 'Back to project'
    : 'Open Tavern — Recruit, Roster, Settings';
  const tavernButtonTitle = tavernPreparing
    ? 'Preparing Tavern'
    : tavernOpen
    ? 'Back to project'
    : 'Tavern — Recruit · Roster · Settings';
  const projectIds = projects.map((project) => project.id);
  const canReorderProjects = projects.length > 1 && !!onReorderProjects;

  const moveProjectTab = (projectId: ProjectId, direction: -1 | 1) => {
    if (!onReorderProjects) return;
    const index = projectIds.indexOf(projectId);
    const nextIndex = index + direction;
    if (index < 0 || nextIndex < 0 || nextIndex >= projectIds.length) return;
    const next = [...projectIds];
    [next[index], next[nextIndex]] = [next[nextIndex]!, next[index]!];
    onReorderProjects(next);
  };

  return (
    <header
      className="topbar"
      role="banner"
      aria-label="Project bar"
      data-tauri-drag-region
    >
      {!hideProjectTabs && (<>
        <TabStrip active={activeProjectId} order={projectIds.join('\n')}>
          {projects.map((p, index) => {
            const active = p.id === activeProjectId;
            const unreadCount = projectUnreadCounts?.[p.id] ?? 0;
            const title = projectTabTitle(p);
            const displayName = p.displayName || projectDisplayName(p.name);
            const canMoveLeft = active && canReorderProjects && index > 0;
            const canMoveRight = active && canReorderProjects && index < projects.length - 1;
            return (
              <div
                key={p.id}
                className={[
                  'tab',
                  active ? 'active' : '',
                  p.activity ? 'activity' : '',
                  unreadCount > 0 ? 'unread' : '',
                ].filter(Boolean).join(' ')}
                role="tab"
                aria-selected={active}
                aria-label={`Project ${displayName}${unreadCount > 0 ? `, ${unreadCount} Violet updates` : ''}`}
                title={unreadCount > 0 ? `${title}\n${unreadCount} Violet update${unreadCount === 1 ? '' : 's'}` : title}
                data-testid={`tab-${p.id}`}
                data-project-id={p.id}
                onClick={() => onSelectProject(p.id)}
              >
                {canMoveLeft && (
                  <button
                    type="button"
                    className="tab-move left"
                    aria-label={`Move ${displayName} left`}
                    title="Move left"
                    onPointerDown={(event) => event.stopPropagation()}
                    onClick={(event) => {
                      event.stopPropagation();
                      moveProjectTab(p.id, -1);
                    }}
                  >
                    <img src={tabMoveLeftIcon} alt="" aria-hidden />
                  </button>
                )}
                <span className="tab-git" aria-hidden />
                <span className="tab-name" title={title}>{displayName}</span>
                {p.dirty && <span className="tab-dirty" title="Unsaved changes" />}
                <button
                  className="tab-close"
                  aria-label={`Archive ${displayName}`}
                  title={`Archive ${displayName}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    onCloseProject(p.id);
                  }}
                  onPointerDown={(e) => e.stopPropagation()}
                >
                  ×
                </button>
                {canMoveRight && (
                  <button
                    type="button"
                    className="tab-move right"
                    aria-label={`Move ${displayName} right`}
                    title="Move right"
                    onPointerDown={(event) => event.stopPropagation()}
                    onClick={(event) => {
                      event.stopPropagation();
                      moveProjectTab(p.id, 1);
                    }}
                  >
                    <img src={tabMoveRightIcon} alt="" aria-hidden />
                  </button>
                )}
              </div>
            );
          })}
        </TabStrip>
        <button
          className="tab-plus"
          aria-label="Open project"
          title="Open project"
          data-testid="tab-plus"
          data-tauri-drag-region="false"
          onClick={onNewProject}
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none"
               stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
            <path d="M8 3v10M3 8h10" />
          </svg>
        </button>
      </>)}

      <div className="tb-spacer" data-tauri-drag-region />

      <button
        className={[
          'tavern-btn',
          tavernOpen ? 'back' : '',
          tavernPreparing ? 'preparing' : '',
        ].filter(Boolean).join(' ')}
        aria-label={tavernButtonLabel}
        title={tavernButtonTitle}
        data-testid="tavern-btn"
        data-tauri-drag-region="false"
        disabled={tavernPreparing}
        onClick={onOpenTavern}
      >
        {tavernOpen ? (
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor"
               strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
            <path d="M15 6l-6 6 6 6" />
            <path d="M10 12h9" />
          </svg>
        ) : (
          <img className="tavern-entry-icon" src={tavernEntryIcon} alt="" aria-hidden />
        )}
      </button>
    </header>
  );
}

/** .tb-edge's width, and how far .tab-move stands out from the active tab with a little air. */
const EDGE = 64;
const MOVE_BUTTON = 12;

/** The scrolling tabs. An edge shows on the side that hides one; the active tab is kept clear of both. */
function TabStrip({ active, order, children }: { active: ProjectId | null; order: string; children: ReactNode }) {
  const strip = useRef<HTMLDivElement>(null);
  // Two booleans, not one object: replayed updates would make a new object each time and never settle.
  const [hiddenLeft, setHiddenLeft] = useState(false);
  const [hiddenRight, setHiddenRight] = useState(false);
  const measure = () => {
    const box = strip.current!.getBoundingClientRect();
    const tabs = strip.current!.children;
    if (!tabs.length) return; // no project is open
    setHiddenLeft(tabs[0]!.getBoundingClientRect().left < box.left - 0.5);
    setHiddenRight(tabs[tabs.length - 1]!.getBoundingClientRect().right > box.right + 0.5);
  };
  const scroll = (left: number, smooth = true) => strip.current!.scrollTo({
    left,
    behavior: smooth && !window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'smooth' : 'auto',
  });

  // A tab may have come, gone or changed its width with this render; the window changes ours without one.
  useLayoutEffect(measure);
  useEffect(() => {
    const observer = new ResizeObserver(measure);
    observer.observe(strip.current!);
    return () => observer.disconnect();
  }, []);

  const shown = useRef(false);
  useLayoutEffect(() => {
    const tab = strip.current!.querySelector<HTMLElement>('.tab.active');
    if (!tab) return;
    const { scrollLeft, clientWidth } = strip.current!;
    const left = tab.offsetLeft - EDGE - MOVE_BUTTON;
    const right = tab.offsetLeft + tab.offsetWidth + EDGE + MOVE_BUTTON;
    if (left < scrollLeft) scroll(left, shown.current);
    else if (right > scrollLeft + clientWidth) scroll(right - clientWidth, shown.current);
    shown.current = true;
  }, [active, order]);

  const edge = (side: 'left' | 'right', on: boolean) => {
    const label = side === 'left' ? 'Earlier projects' : 'More projects';
    const step = side === 'left' ? -1 : 1;
    return (
      <div className={`tb-edge ${side}${on ? ' on' : ''}`}>
        <button
          type="button"
          tabIndex={-1}
          disabled={!on}
          title={label}
          aria-label={label}
          data-tauri-drag-region="false"
          onClick={() => scroll(strip.current!.scrollLeft + step * (strip.current!.clientWidth - 2 * EDGE))}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor"
               strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
            <path d={side === 'left' ? 'M10 3.5 5.5 8l4.5 4.5' : 'M6 3.5 10.5 8 6 12.5'} />
          </svg>
        </button>
      </div>
    );
  };
  return (
    <div className="tb-strip" data-tauri-drag-region="false">
      <div ref={strip} className="tb-tabs" data-tauri-drag-region="false" onScroll={measure}>{children}</div>
      {edge('left', hiddenLeft)}
      {edge('right', hiddenRight)}
    </div>
  );
}

function projectDisplayName(name: string): string {
  const trimmed = name.trim();
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? trimmed;
}

function projectTabTitle(project: Project): string {
  const details = [
    project.githubUrl ? `GitHub: ${project.githubUrl}` : null,
    project.sourcePath ? `Local: ${project.sourcePath}` : project.path ? `Local: ${project.path}` : null,
    project.accountPath ? `Account: ${project.accountPath}` : null,
  ].filter((item): item is string => item != null);
  return details.length > 0 ? `${project.name}\n${details.join('\n')}` : project.name;
}
