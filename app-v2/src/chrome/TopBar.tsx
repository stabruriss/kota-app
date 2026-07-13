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

import tabMoveLeftIcon from '../assets/tavern/icons/tab-move-left.svg';
import tabMoveRightIcon from '../assets/tavern/icons/tab-move-right.svg';
import tavernEntryIcon from '../assets/tavern/icons/tavern-entry.svg';
import type { Project, ProjectId } from '../types/project';
import type { GhAuthInfo } from '../pty-client';

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
  /** M6.B Step 1 — `gh auth status` snapshot. `null` while the
   *  initial probe is in flight. */
  ghAuth?: GhAuthInfo | null;
  /** Called when the user clicks the auth indicator pill — typically
   *  routes to a SmartTerminal tab running `gh auth login` (M6.B §B2). */
  onGhAuthClick?: () => void;
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
  ghAuth,
  onGhAuthClick,
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
      {!hideProjectTabs && (
        <div
          className="tb-tabs"
          data-tauri-drag-region="false"
        >
          {projects.map((p, index) => {
            const active = p.id === activeProjectId;
            const unreadCount = projectUnreadCounts?.[p.id] ?? 0;
            const title = projectTabTitle(p);
            const displayName = projectDisplayName(p.name);
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
        </div>
      )}

      <div className="tb-spacer" data-tauri-drag-region />

      {ghAuth !== undefined && <GhAuthPill ghAuth={ghAuth} onClick={onGhAuthClick} />}

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

function GhAuthPill({
  ghAuth,
  onClick,
}: {
  ghAuth: GhAuthInfo | null | undefined;
  onClick?: () => void;
}) {
  // null === probe in flight; undefined === probe not wired (e.g. test
  // env); both render the same neutral placeholder.
  if (ghAuth == null) {
    return (
      <span
        className="gh-pill probing"
        data-testid="gh-auth-pill"
        data-state="probing"
        data-tauri-drag-region="false"
        title="Checking gh auth status…"
      >
        <span className="gh-pill-dot" aria-hidden />
        GitHub
      </span>
    );
  }
  if (ghAuth.cliMissing) {
    return (
      <button
        type="button"
        className="gh-pill missing"
        data-testid="gh-auth-pill"
        data-state="missing"
        data-tauri-drag-region="false"
        title="GitHub CLI not found"
        onClick={onClick}
      >
        <span className="gh-pill-dot" aria-hidden />
        GitHub
      </button>
    );
  }
  if (!ghAuth.authenticated) {
    return (
      <button
        type="button"
        className="gh-pill setup"
        data-testid="gh-auth-pill"
        data-state="setup"
        data-tauri-drag-region="false"
        title={ghAuth.error ?? 'Sign in to GitHub via the terminal'}
        onClick={onClick}
      >
        <span className="gh-pill-dot" aria-hidden />
        GitHub
      </button>
    );
  }
  return (
    <button
      type="button"
      className="gh-pill ok"
      data-testid="gh-auth-pill"
      data-state="ok"
      data-tauri-drag-region="false"
      title={`Signed in as ${ghAuth.username ?? '?'} · ${ghAuth.scopes.join(' ') || 'no scopes'}`}
      onClick={onClick}
    >
      <span className="gh-pill-dot" aria-hidden />
      GitHub{ghAuth.username ? ` · ${ghAuth.username}` : ''}
    </button>
  );
}
