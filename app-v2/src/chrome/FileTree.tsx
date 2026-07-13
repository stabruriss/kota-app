/** Workspace file tree — raw project inventory for the left column.
 *
 *  MVP scope is intentionally narrow: browse Project Files and Project
 *  Workspace, select paths, open/reveal/copy through system actions,
 *  and drag paths into the composer. No search, preview, editor, or diff. */

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import featherPenIcon from '../assets/tavern/icons/feather-pen.svg';
import {
  workspaceListTreePath,
  workspaceOpenTreePath,
  workspaceRevealTreePath,
} from '../pty-client';
import { DiffMedal } from './DiffMedal';
import { emitFileTreeAgentHover } from '../lib/file-tree-agent-hover';
import { useKotaUpdateCheck } from '../hooks/useKotaUpdateCheck';
import type {
  WorkspaceDiffScope,
  WorkspaceTreeChangeOverview,
  WorkspaceTreeChangeParticipant,
  WorkspaceTreeFileChange,
  WorkspaceTreeEntry,
  WorkspaceTreeListing,
  WorkspaceTreeRootKind,
} from '../types/tree';
import type { AgentId } from '../types/scene';
import { HeroAvatarArt } from './HeroAvatarPicker';

interface SelectedTreePath {
  rootKind: WorkspaceTreeRootKind;
  relativePath: string;
  absolutePath: string;
  kind: 'file' | 'folder' | 'symlink';
  isGhost?: boolean;
}

interface ContextMenuState {
  x: number;
  y: number;
  item: SelectedTreePath;
}

interface SymlinkLinkSegment {
  id: string;
  d: string;
  external: boolean;
}

interface SymlinkLinkOverlay {
  width: number;
  height: number;
  segments: SymlinkLinkSegment[];
}

interface SymlinkSourceMarker {
  target: string;
  label: string;
  displayPath: string;
  kind: 'file' | 'folder' | 'skillPool';
}

export interface FileTreeProps {
  projectId: string;
  repoName: string;
  sourceDir?: string | null;
  workspaceDir?: string | null;
  refreshToken?: number;
}

const ROOTS: Array<{ kind: WorkspaceTreeRootKind; label: string }> = [
  { kind: 'projectFiles', label: 'Project Files' },
  { kind: 'projectWorkspace', label: 'Project Workspace' },
];
const TREE_DRAG_MIME = 'application/x-kota-file-path';
const emptySymlinkOverlay: SymlinkLinkOverlay = { width: 0, height: 0, segments: [] };
const LOCAL_TEST_AGENT = import.meta.env.VITE_KOTA_LOCAL_TEST_AGENT?.trim();
const LOCAL_TEST_TIMESTAMP = import.meta.env.VITE_KOTA_LOCAL_TEST_TIMESTAMP?.trim();
const LOCAL_TEST_WATERMARK = LOCAL_TEST_AGENT
  ? ['local test version', LOCAL_TEST_AGENT, LOCAL_TEST_TIMESTAMP].filter(Boolean).join(' · ')
  : null;

declare global {
  interface Window {
    __KOTA_FILE_TREE_DRAG_PATHS__?: string[];
    __KOTA_FILE_TREE_DRAG_CLEAR__?: number;
  }
}

export function FileTree({
  projectId,
  repoName,
  sourceDir,
  workspaceDir,
  refreshToken,
}: FileTreeProps) {
  const [activeRoot, setActiveRoot] = useState<WorkspaceTreeRootKind>('projectFiles');
  const [openPaths, setOpenPaths] = useState<Set<string>>(() => new Set(['projectFiles:', 'projectWorkspace:']));
  const [listings, setListings] = useState<Map<string, WorkspaceTreeListing>>(() => new Map());
  const [loadingKeys, setLoadingKeys] = useState<Set<string>>(() => new Set());
  const [selected, setSelected] = useState<SelectedTreePath | null>(null);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [diffScope, setDiffScope] = useState<WorkspaceDiffScope | null>(null);
  const [symlinkOverlay, setSymlinkOverlay] = useState<SymlinkLinkOverlay>({
    width: 0,
    height: 0,
    segments: [],
  });
  const treeBodyRef = useRef<HTMLDivElement | null>(null);
  const mountedRef = useRef(true);
  const listingsRef = useRef(listings);
  const loadingKeysRef = useRef(loadingKeys);
  const refreshTokenRef = useRef(refreshToken);
  const {
    updateAvailable,
    latestVersion,
    openUpdateHome,
  } = useKotaUpdateCheck();

  useEffect(() => () => {
    mountedRef.current = false;
  }, []);

  useEffect(() => {
    listingsRef.current = listings;
  }, [listings]);

  useEffect(() => {
    loadingKeysRef.current = loadingKeys;
  }, [loadingKeys]);

  useEffect(() => {
    const nextListings = new Map<string, WorkspaceTreeListing>();
    const nextLoadingKeys = new Set<string>();
    listingsRef.current = nextListings;
    loadingKeysRef.current = nextLoadingKeys;
    refreshTokenRef.current = refreshToken;
    setListings(nextListings);
    setLoadingKeys(nextLoadingKeys);
    setOpenPaths(new Set(['projectFiles:', 'projectWorkspace:']));
    setSelected(null);
  }, [projectId, sourceDir, workspaceDir]);

  const listingKey = useCallback((rootKind: WorkspaceTreeRootKind, relativePath = '') => (
    `${rootKind}:${relativePath}`
  ), []);

  const loadListing = useCallback(async (
    rootKind: WorkspaceTreeRootKind,
    relativePath = '',
  ) => {
    const key = listingKey(rootKind, relativePath);
    if (listingsRef.current.has(key) || loadingKeysRef.current.has(key)) return;
    setLoadingKeys((prev) => {
      const next = new Set(prev).add(key);
      loadingKeysRef.current = next;
      return next;
    });
    try {
      const listing = await workspaceListTreePath({ projectId, rootKind, relativePath });
      if (!mountedRef.current) return;
      setListings((prev) => {
        const next = new Map(prev);
        next.set(key, listing);
        return next;
      });
      if (!selected && !relativePath) {
        setSelected({
          rootKind,
          relativePath: '',
          absolutePath: listing.root.absolutePath,
          kind: 'folder',
        });
      }
    } catch (err) {
      console.warn('[file-tree] list failed', err);
    } finally {
      if (mountedRef.current) {
        setLoadingKeys((prev) => {
          const next = new Set(prev);
          next.delete(key);
          loadingKeysRef.current = next;
          return next;
        });
      }
    }
  }, [listingKey, projectId, selected]);

  const refreshListings = useCallback(async () => {
    const rootKey = listingKey(activeRoot, '');
    const keys = Array.from(new Set([rootKey, ...listingsRef.current.keys()]));
    if (keys.length === 0) return;

    setLoadingKeys((prev) => {
      const next = new Set(prev);
      for (const key of keys) next.add(key);
      loadingKeysRef.current = next;
      return next;
    });

    const results = await Promise.allSettled(
      keys.map(async (key) => {
        const parsed = parseListingKey(key);
        if (!parsed) return null;
        const listing = await workspaceListTreePath({
          projectId,
          rootKind: parsed.rootKind,
          relativePath: parsed.relativePath,
        });
        return { key, listing };
      }),
    );

    if (!mountedRef.current) return;

    setListings((prev) => {
      const next = new Map(prev);
      results.forEach((result, index) => {
        const key = keys[index];
        if (!key) return;
        if (result.status === 'fulfilled' && result.value) {
          next.set(result.value.key, result.value.listing);
        } else {
          next.delete(key);
        }
      });
      listingsRef.current = next;
      return next;
    });

    setLoadingKeys((prev) => {
      const next = new Set(prev);
      for (const key of keys) next.delete(key);
      loadingKeysRef.current = next;
      return next;
    });
  }, [activeRoot, listingKey, projectId]);

  useEffect(() => {
    if (refreshToken === undefined) return;
    if (refreshTokenRef.current === refreshToken) return;
    refreshTokenRef.current = refreshToken;
    void refreshListings();
  }, [refreshListings, refreshToken]);

  useEffect(() => {
    const refresh = () => { void refreshListings(); };
    const refreshWhenVisible = () => {
      if (document.visibilityState === 'visible') refresh();
    };
    window.addEventListener('focus', refresh);
    window.addEventListener('kota:file-tree-refresh', refresh);
    document.addEventListener('visibilitychange', refreshWhenVisible);
    return () => {
      window.removeEventListener('focus', refresh);
      window.removeEventListener('kota:file-tree-refresh', refresh);
      document.removeEventListener('visibilitychange', refreshWhenVisible);
    };
  }, [refreshListings]);

  useEffect(() => {
    void loadListing(activeRoot, '');
  }, [activeRoot, loadListing]);

  const activeListing = listings.get(listingKey(activeRoot, ''));
  const activeRootPath = activeListing?.root.absolutePath
    ?? (activeRoot === 'projectFiles' ? sourceDir : workspaceDir)
    ?? 'Loading...';
  const rootStatus = activeListing?.root.changeOverview ?? null;
  const rootHasProjectChanges = activeRoot === 'projectFiles'
    && (hasOverviewChanges(rootStatus) || (activeListing?.entries ?? []).some(entryHasDiff));
  const externalSymlinkMarkers = useMemo(() => {
    if (activeRoot !== 'projectWorkspace' || !activeListing) return [];
    const markers = collectVisibleSymlinkMarkers(activeListing.entries, {
      rootKind: activeRoot,
      openPaths,
      listings,
      listingKey,
    });
    const seen = new Set<string>();
    return markers
      .filter((marker) => !isPathInsideRoot(marker.target, activeRootPath))
      .filter((marker) => {
        const normalized = normalizeFsPath(marker.target);
        if (seen.has(normalized)) return false;
        seen.add(normalized);
        return true;
      });
  }, [activeListing, activeRoot, activeRootPath, listingKey, listings, openPaths]);

  useLayoutEffect(() => {
    if (activeRoot !== 'projectWorkspace') {
      setSymlinkOverlay(emptySymlinkOverlay);
      return;
    }

    const measure = () => {
      const body = treeBodyRef.current;
      if (!body) {
        setSymlinkOverlay(emptySymlinkOverlay);
        return;
      }
      setSymlinkOverlay((prev) => {
        const next = measureSymlinkOverlay(body, activeRootPath);
        return sameSymlinkOverlay(prev, next) ? prev : next;
      });
    };

    measure();
    const timer = window.setTimeout(measure, 0);
    window.addEventListener('resize', measure);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener('resize', measure);
    };
  }, [activeRoot, activeRootPath, activeListing, externalSymlinkMarkers, listings, openPaths]);

  useEffect(() => {
    if (!activeListing) return;
    if (selected?.rootKind === activeRoot) return;
    setSelected({
      rootKind: activeRoot,
      relativePath: '',
      absolutePath: activeListing.root.absolutePath,
      kind: 'folder',
    });
  }, [activeListing, activeRoot, selected?.rootKind]);

  const selectPath = (item: SelectedTreePath) => {
    setSelected(item);
    setContextMenu(null);
  };

  const toggleFolder = async (entry: WorkspaceTreeEntry) => {
    if (entry.kind !== 'folder') return;
    const key = listingKey(activeRoot, entry.path);
    setOpenPaths((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
    await loadListing(activeRoot, entry.path);
  };

  const runSystemAction = async (
    item: SelectedTreePath,
    action: 'open' | 'reveal' | 'copy',
  ) => {
    try {
      if (action === 'open') {
        await workspaceOpenTreePath({
          projectId,
          rootKind: item.rootKind,
          relativePath: item.relativePath,
        });
      } else if (action === 'reveal') {
        await workspaceRevealTreePath({
          projectId,
          rootKind: item.rootKind,
          relativePath: item.relativePath,
        });
      } else if (navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(item.absolutePath);
      }
    } catch (err) {
      console.error(`[file-tree] ${action} failed`, err);
      if (typeof window !== 'undefined' && typeof window.alert === 'function') {
        window.alert(`${action} failed: ${err}`);
      }
    } finally {
      setContextMenu(null);
    }
  };

  const openDiffScope = (scope: WorkspaceDiffScope) => {
    setContextMenu(null);
    setDiffScope(scope);
  };

  const openUpdate = useCallback(() => {
    void openUpdateHome();
  }, [openUpdateHome]);

  return (
    <aside
      className="tree"
      aria-label="Project inventory"
      data-testid="file-tree"
      onClick={() => setContextMenu(null)}
    >
      <div className="tree-head">
        <span className="tree-title-stack">
          <span className="tree-repo">{projectDisplayName(repoName)}</span>
          {LOCAL_TEST_WATERMARK && (
            <span className="tree-local-test-watermark">{LOCAL_TEST_WATERMARK}</span>
          )}
          {updateAvailable && latestVersion && (
            <span className="tree-update-banner-row">
              <button
                type="button"
                className="tree-update-banner"
                onClick={openUpdate}
                title="Open https://kota.place"
              >
                <span className="tree-update-title">Kota {latestVersion} is available</span>
                <span className="tree-update-subtitle">Open kota.place</span>
              </button>
            </span>
          )}
        </span>
      </div>

      <div className="tree-tabs" role="tablist" aria-label="File tree sections">
        {ROOTS.map((root) => (
          <button
            key={root.kind}
            type="button"
            className={`tree-tab ${activeRoot === root.kind ? 'active' : ''}`}
            role="tab"
            aria-selected={activeRoot === root.kind}
            data-testid={`tree-tab-${root.kind}`}
            onClick={() => {
              setActiveRoot(root.kind);
              setContextMenu(null);
            }}
          >
            {root.label}
          </button>
        ))}
      </div>

      <div className="tree-section-meta">
        <div
          className="tree-root-path"
          aria-label={activeRootPath}
          data-full-path={activeRootPath}
          data-testid="tree-root-path"
        >
          <code>{activeRootPath}</code>
          {rootHasProjectChanges && (
            <FeatherChangeButton
              label="Open all project diffs"
              onClick={() => openDiffScope({ type: 'all' })}
            />
          )}
        </div>
      </div>

      <div className="tree-body" ref={treeBodyRef}>
        {symlinkOverlay.segments.length > 0 && (
          <svg
            className="tree-symlink-overlay"
            width={symlinkOverlay.width}
            height={symlinkOverlay.height}
            viewBox={`0 0 ${symlinkOverlay.width} ${symlinkOverlay.height}`}
            aria-hidden
          >
            {symlinkOverlay.segments.map((segment) => (
              <path
                key={segment.id}
                className={segment.external ? 'external' : undefined}
                d={segment.d}
              />
            ))}
          </svg>
        )}
        {activeRoot === 'projectWorkspace' && externalSymlinkMarkers.length > 0 && (
          <div className="tree-source-markers" aria-label="External symlink sources">
            {externalSymlinkMarkers.map((marker) => (
              <div
                key={normalizeFsPath(marker.target)}
                className={[
                  'tree-source-marker',
                  marker.kind === 'skillPool' ? 'skill-pool' : '',
                ].filter(Boolean).join(' ')}
                data-source-target={marker.target}
                title={marker.target}
                data-testid="tree-source-marker"
              >
                <span className="tree-source-marker-icon" aria-hidden>
                  {marker.kind === 'file' ? <FileIcon /> : <FolderIcon />}
                </span>
                <span className="tree-source-marker-label">{marker.label}</span>
                <span className="tree-source-marker-path">{marker.displayPath}</span>
              </div>
            ))}
          </div>
        )}
        <RootRow
          rootKind={activeRoot}
          label={activeRoot === 'projectFiles' ? basename(activeRootPath) : basename(activeRootPath)}
          absolutePath={activeRootPath}
          selected={selected?.rootKind === activeRoot && selected.relativePath === ''}
          hasChanges={rootHasProjectChanges}
          onSelect={selectPath}
          onOpen={(item) => { void runSystemAction(item, 'open'); }}
          onOpenDiff={() => openDiffScope({ type: 'all' })}
          onContextMenu={(state) => setContextMenu(state)}
        />
        {activeListing ? (
          activeListing.entries
            .map((entry) => (
              <TreeRow
                key={entry.path}
                entry={entry}
                level={1}
                rootKind={activeRoot}
                selectedPath={selected?.rootKind === activeRoot ? selected.relativePath : null}
                openPaths={openPaths}
                listings={listings}
                listingKey={listingKey}
                onSelect={selectPath}
                onToggleFolder={(folder) => { void toggleFolder(folder); }}
                onOpen={(item) => { void runSystemAction(item, 'open'); }}
                onOpenDiffScope={openDiffScope}
                onContextMenu={setContextMenu}
              />
            ))
        ) : (
          <div className="tree-loading tree-connecting" data-testid="tree-loading">
            <span className="tree-connecting-spinner" aria-hidden />
            <span>Connecting...</span>
          </div>
        )}
      </div>

      {contextMenu && (
        <div
          className="tree-context-menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          role="menu"
          onClick={(event) => event.stopPropagation()}
          data-testid="tree-context-menu"
        >
          {contextMenu.item.isGhost ? (
            <div className="tree-context-ghost-note" role="note">
              <strong>Agent Worktree only file</strong>
              <span>Sync room to see it here.</span>
            </div>
          ) : (
            <>
              <button type="button" role="menuitem" onClick={() => void runSystemAction(contextMenu.item, 'open')}>
                Open Default App
              </button>
              <button type="button" role="menuitem" onClick={() => void runSystemAction(contextMenu.item, 'reveal')}>
                Reveal in Finder
              </button>
              <button type="button" role="menuitem" onClick={() => void runSystemAction(contextMenu.item, 'copy')}>
                Copy Full Path
              </button>
            </>
          )}
        </div>
      )}
      {diffScope && (
        <DiffMedal
          projectId={projectId}
          sourceRoot={sourceDir ?? activeListing?.root.absolutePath ?? null}
          scope={diffScope}
          onClose={() => setDiffScope(null)}
        />
      )}
    </aside>
  );
}

function RootRow({
  rootKind,
  label,
  absolutePath,
  selected,
  hasChanges,
  onSelect,
  onOpen,
  onOpenDiff,
  onContextMenu,
}: {
  rootKind: WorkspaceTreeRootKind;
  label: string;
  absolutePath: string;
  selected: boolean;
  hasChanges?: boolean;
  onSelect: (item: SelectedTreePath) => void;
  onOpen: (item: SelectedTreePath) => void;
  onOpenDiff: () => void;
  onContextMenu: (state: ContextMenuState) => void;
}) {
  const item: SelectedTreePath = {
    rootKind,
    relativePath: '',
    absolutePath,
    kind: 'folder',
  };
  return (
    <div
      className={`t-row root folder open ${selected ? 'active' : ''}`}
      role="treeitem"
      aria-selected={selected}
      aria-expanded
      draggable
      data-testid={`tree-row-${rootKind}-root`}
      data-path={absolutePath}
      data-tree-row="true"
      data-tree-relative=""
      onClick={(event) => {
        event.stopPropagation();
        onSelect(item);
      }}
      onDoubleClick={() => onOpen(item)}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
        onSelect(item);
        onContextMenu({ x: event.clientX, y: event.clientY, item });
      }}
      onDragStart={(event) => setDragPath(event.dataTransfer, absolutePath)}
      onDragEnd={clearDragPaths}
    >
      <span className="t-chev" aria-hidden>
        <svg viewBox="0 0 8 8" fill="currentColor" width={8} height={8}>
          <path d="M2 1 l3 3 l-3 3 z" />
        </svg>
      </span>
      <span className="t-ico" aria-hidden><FolderIcon /></span>
      <span className="t-name">
        <span className="t-name-text" title={absolutePath}>{label || absolutePath}</span>
      </span>
      {rootKind === 'projectFiles' && hasChanges && (
        <FeatherChangeButton label="Open all project diffs" onClick={onOpenDiff} />
      )}
    </div>
  );
}

function TreeRow({
  entry,
  level,
  rootKind,
  selectedPath,
  openPaths,
  listings,
  listingKey,
  onSelect,
  onToggleFolder,
  onOpen,
  onOpenDiffScope,
  onContextMenu,
}: {
  entry: WorkspaceTreeEntry;
  level: number;
  rootKind: WorkspaceTreeRootKind;
  selectedPath: string | null;
  openPaths: Set<string>;
  listings: Map<string, WorkspaceTreeListing>;
  listingKey: (rootKind: WorkspaceTreeRootKind, relativePath?: string) => string;
  onSelect: (item: SelectedTreePath) => void;
  onToggleFolder: (entry: WorkspaceTreeEntry) => void;
  onOpen: (item: SelectedTreePath) => void;
  onOpenDiffScope: (scope: WorkspaceDiffScope) => void;
  onContextMenu: (state: ContextMenuState) => void;
}): ReactNode {
  const folder = entry.kind === 'folder';
  const open = folder && openPaths.has(listingKey(rootKind, entry.path));
  const selected = selectedPath === entry.path;
  const children = folder ? listings.get(listingKey(rootKind, entry.path))?.entries ?? null : null;
  const fileChange = entry.fileChange ?? null;
  const showFolderChange = folder && !!entry.treeHasChanges;
  const item: SelectedTreePath = {
    rootKind,
    relativePath: entry.path,
    absolutePath: entry.absolutePath,
    kind: entry.kind,
    isGhost: !!entry.isGhost,
  };
  const labelTitle = entry.symlinkTarget
    ? `Symlink source: ${entry.symlinkTarget}`
    : entry.worktreeSource
      ? `Worktree source: ${entry.worktreeSource}`
      : undefined;
  const agentDisplayName = rootKind === 'projectWorkspace' ? entry.agentDisplayName?.trim() : null;
  const agentId = agentDisplayName ? agentIdForWorkspaceEntry(entry) : null;
  const symlinkSource = symlinkSourceMarkerForEntry(entry);

  useEffect(() => () => {
    if (agentId) emitFileTreeAgentHover(agentId, false);
  }, [agentId]);

  return (
    <>
      <div
        className={[
          't-row',
          `lvl-${Math.min(level, 4)}`,
          folder ? 'folder' : 'leaf',
          open ? 'open' : '',
          selected ? 'active' : '',
          entry.isHidden ? 'hidden-file' : '',
          entry.kind === 'symlink' ? 'symlink' : '',
          agentDisplayName ? 'agent-workspace' : '',
          entry.isGhost ? 'ghost' : '',
          fileChange ? `change-${fileChange.status}` : '',
          entry.treeHasChanges ? 'tree-has-changes' : '',
        ].filter(Boolean).join(' ')}
        role="treeitem"
        aria-selected={selected}
        aria-expanded={folder ? open : undefined}
        draggable
        data-testid={`tree-row-${rootKind}-${entry.path}`}
        data-path={entry.absolutePath}
        data-tree-row="true"
        data-tree-relative={entry.path}
        data-symlink-target={entry.symlinkTarget ?? undefined}
        data-symlink-source-target={symlinkSource?.target}
        onClick={(event) => {
          event.stopPropagation();
          onSelect(item);
        }}
        onDoubleClick={() => {
          if (!entry.isGhost) onOpen(item);
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          event.stopPropagation();
          onSelect(item);
          onContextMenu({ x: event.clientX, y: event.clientY, item });
        }}
        onDragStart={(event) => setDragPath(event.dataTransfer, entry.absolutePath)}
        onDragEnd={clearDragPaths}
      >
        <button
          type="button"
          className="t-chev"
          aria-label={folder ? (open ? 'Collapse folder' : 'Expand folder') : undefined}
          disabled={!folder}
          onClick={(event) => {
            event.stopPropagation();
            onToggleFolder(entry);
          }}
        >
          {folder ? (
            <svg viewBox="0 0 8 8" fill="currentColor" width={8} height={8}>
              <path d="M2 1 l3 3 l-3 3 z" />
            </svg>
          ) : null}
        </button>
        <span className="t-ico" aria-hidden>
          {entry.kind === 'symlink' ? <LinkIcon /> : folder ? <FolderIcon /> : <FileIcon />}
        </span>
        {agentDisplayName && (
          <span
            className="t-agent-avatar"
            aria-label={agentDisplayName}
            data-agent-name={agentDisplayName}
            data-agent-id={agentId ?? undefined}
            data-testid={`tree-agent-avatar-${entry.path}`}
            onMouseEnter={() => {
              if (agentId) emitFileTreeAgentHover(agentId, true);
            }}
            onMouseLeave={() => {
              if (agentId) emitFileTreeAgentHover(agentId, false);
            }}
            onFocus={() => {
              if (agentId) emitFileTreeAgentHover(agentId, true);
            }}
            onBlur={() => {
              if (agentId) emitFileTreeAgentHover(agentId, false);
            }}
            tabIndex={0}
          >
            {agentInitials(agentDisplayName)}
          </span>
        )}
        <span className="t-name">
          <span className="t-name-text" title={entry.absolutePath}>{entry.name}</span>
          {entry.kind === 'symlink' && <span className="t-label symlink-label" title={labelTitle}>symlink</span>}
          {entry.isWorktree && <span className="t-label worktree-label" title={labelTitle}>worktree</span>}
          {fileChange && !folder && (
            <FileChangeBadge
              change={fileChange}
              onOpenDiff={() => onOpenDiffScope({ type: 'file', path: entry.path })}
            />
          )}
        </span>
        {showFolderChange && (
          <FeatherChangeButton
            label={`Open diffs under ${entry.name}`}
            onClick={() => onOpenDiffScope({ type: 'folder', prefix: entry.path })}
          />
        )}
      </div>
      {folder && open && (
        <div className="t-kids" role="group">
          {children ? (
            children
              .map((child) => (
                <TreeRow
                  key={child.path}
                  entry={child}
                  level={level + 1}
                  rootKind={rootKind}
                  selectedPath={selectedPath}
                  openPaths={openPaths}
                  listings={listings}
                  listingKey={listingKey}
                  onSelect={onSelect}
                  onToggleFolder={onToggleFolder}
                  onOpen={onOpen}
                  onOpenDiffScope={onOpenDiffScope}
                  onContextMenu={onContextMenu}
                />
              ))
          ) : (
            <div className={`tree-loading lvl-${Math.min(level + 1, 4)}`}>Loading...</div>
          )}
        </div>
      )}
    </>
  );
}

interface DiffPopoverPosition {
  left: number;
  top: number;
}

function FileChangeBadge({
  change,
  onOpenDiff,
}: {
  change: WorkspaceTreeFileChange;
  onOpenDiff: () => void;
}) {
  const badgeRef = useRef<HTMLSpanElement | null>(null);
  const [visible, setVisible] = useState(false);
  const [position, setPosition] = useState<DiffPopoverPosition | null>(null);
  const label = fileChangeLabel(change);
  const updatePosition = useCallback(() => {
    const badge = badgeRef.current;
    if (!badge) return;
    const rect = badge.getBoundingClientRect();
    const gap = 6;
    const margin = 10;
    const width = 246;
    const estimatedHeight = Math.min(260, Math.max(44, change.participants.length * 27 + 18));
    const maxLeft = Math.max(margin, window.innerWidth - width - margin);
    const left = Math.min(Math.max(rect.left, margin), maxLeft);
    const belowTop = rect.bottom + gap;
    const aboveTop = rect.top - gap - estimatedHeight;
    const top = belowTop + estimatedHeight > window.innerHeight - margin
      ? Math.max(margin, aboveTop)
      : belowTop;
    setPosition({ left, top });
  }, [change.participants.length]);

  useLayoutEffect(() => {
    if (!visible) return undefined;
    updatePosition();
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [updatePosition, visible]);

  const showPopover = () => {
    setVisible(true);
    updatePosition();
  };
  const hidePopover = () => setVisible(false);

  return (
    <span
      ref={badgeRef}
      className={[
        't-label',
        'diff-label',
        change.status,
        label.kind === 'lines' ? 'line-stat' : '',
      ].filter(Boolean).join(' ')}
      tabIndex={0}
      onMouseEnter={showPopover}
      onMouseLeave={hidePopover}
      onFocus={showPopover}
      onBlur={hidePopover}
      onClick={(event) => {
        event.stopPropagation();
        onOpenDiff();
      }}
      onKeyDown={(event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        event.stopPropagation();
        onOpenDiff();
      }}
      role="button"
      aria-label="Open file diff"
    >
      {label.kind === 'lines' ? (
        <TreeLineStat added={change.addedLines} deleted={change.deletedLines} />
      ) : label.text}
      {visible && position && createPortal(
        <span
          className="tree-diff-popover visible"
          role="tooltip"
          style={{ left: position.left, top: position.top }}
        >
          {change.participants.map((participant) => (
            <span
              key={`${participant.kind}:${participant.actorId}:${participant.status}`}
              className={`tree-diff-pill ${participant.kind}`}
            >
              <DiffParticipantAvatar participant={participant} />
              <span className="tree-diff-name" title={participant.displayName}>
                {shortDisplayName(diffParticipantLabel(participant))}
              </span>
              <span className={`tree-diff-status ${participant.status}`}>
                {participant.status}
              </span>
            </span>
          ))}
        </span>,
        document.body,
      )}
    </span>
  );
}

function FeatherChangeButton({
  label,
  onClick,
}: {
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="tree-folder-diff feather"
      title={label}
      aria-label={label}
      onClick={(event) => {
        event.stopPropagation();
        onClick();
      }}
    >
      <img src={featherPenIcon} alt="" aria-hidden />
    </button>
  );
}

function hasOverviewChanges(overview?: WorkspaceTreeChangeOverview | null): boolean {
  if (!overview) return false;
  return overview.added > 0 || overview.modified > 0 || overview.deleted > 0 || overview.untracked > 0;
}

function entryHasDiff(entry: WorkspaceTreeEntry): boolean {
  return !!entry.fileChange || !!entry.treeHasChanges || hasOverviewChanges(entry.changeOverview);
}

function fileChangeLabel(change: WorkspaceTreeFileChange): { kind: 'text'; text: string } | { kind: 'lines' } {
  if (change.status === 'added') return { kind: 'text', text: 'added' };
  if (change.status === 'deleted') return { kind: 'text', text: 'deleted' };
  if (change.status === 'untracked') return { kind: 'text', text: 'untracked' };
  if (change.addedLines != null || change.deletedLines != null) return { kind: 'lines' };
  return { kind: 'text', text: 'changed' };
}

function TreeLineStat({
  added,
  deleted,
}: {
  added?: number | null;
  deleted?: number | null;
}) {
  return (
    <span className="tree-line-stat">
      {deleted ? <span className="del">-{deleted}</span> : null}
      {added ? <span className="add">+{added}</span> : null}
      {!added && !deleted ? <span className="zero">0</span> : null}
    </span>
  );
}

function DiffParticipantAvatar({ participant }: { participant: WorkspaceTreeChangeParticipant }) {
  const title = `${diffParticipantLabel(participant)} · ${participant.status}`;
  if (participant.kind === 'human') {
    return <span className="tree-diff-avatar human" title={title} aria-hidden>H</span>;
  }
  return (
    <HeroAvatarArt
      avatarId={participant.avatarId}
      provider={participant.provider}
      className="tree-diff-avatar"
      title={title}
    />
  );
}

function collectVisibleSymlinkMarkers(
  entries: WorkspaceTreeEntry[],
  ctx: {
    rootKind: WorkspaceTreeRootKind;
    openPaths: Set<string>;
    listings: Map<string, WorkspaceTreeListing>;
    listingKey: (rootKind: WorkspaceTreeRootKind, relativePath?: string) => string;
  },
): SymlinkSourceMarker[] {
  const markers: SymlinkSourceMarker[] = [];
  for (const entry of entries) {
    const marker = symlinkSourceMarkerForEntry(entry);
    if (marker) {
      markers.push(marker);
    }
    if (entry.kind !== 'folder') continue;
    const key = ctx.listingKey(ctx.rootKind, entry.path);
    if (!ctx.openPaths.has(key)) continue;
    const children = ctx.listings.get(key)?.entries;
    if (children) {
      markers.push(...collectVisibleSymlinkMarkers(children, ctx));
    }
  }
  return markers;
}

function parseListingKey(key: string): {
  rootKind: WorkspaceTreeRootKind;
  relativePath: string;
} | null {
  const index = key.indexOf(':');
  if (index < 0) return null;
  const rootKind = key.slice(0, index);
  if (rootKind !== 'projectFiles' && rootKind !== 'projectWorkspace') return null;
  return {
    rootKind,
    relativePath: key.slice(index + 1),
  };
}

function measureSymlinkOverlay(body: HTMLElement, activeRootPath: string): SymlinkLinkOverlay {
  const width = Math.max(body.scrollWidth, body.clientWidth);
  const height = Math.max(body.scrollHeight, body.clientHeight);
  const bodyRect = body.getBoundingClientRect();
  const rows = Array.from(body.querySelectorAll<HTMLElement>('[data-tree-row="true"]'));
  const rowsByPath = new Map<string, HTMLElement>();
  for (const row of rows) {
    const path = row.dataset.path;
    if (path) rowsByPath.set(normalizeFsPath(path), row);
  }

  const markersByTarget = new Map<string, HTMLElement>();
  for (const marker of Array.from(body.querySelectorAll<HTMLElement>('[data-source-target]'))) {
    const target = marker.dataset.sourceTarget;
    if (target) markersByTarget.set(normalizeFsPath(target), marker);
  }

  const segments: SymlinkLinkSegment[] = [];
  rows.forEach((row, index) => {
    const rawTarget = row.dataset.symlinkTarget;
    if (!rawTarget) return;
    const sourceTarget = row.dataset.symlinkSourceTarget || rawTarget;
    const normalizedRawTarget = normalizeFsPath(rawTarget);
    const normalizedSourceTarget = normalizeFsPath(sourceTarget);
    const sourceRow = rowsByPath.get(normalizedRawTarget);
    const external = !isPathInsideRoot(sourceTarget, activeRootPath);
    const targetElement = sourceRow && sourceRow !== row
      ? sourceRow
      : external
        ? markersByTarget.get(normalizedSourceTarget)
        : null;
    if (!targetElement) return;

    const from = elementRightAnchor(row, body, bodyRect, '.symlink-label');
    const to = sourceRow && sourceRow !== row
      ? elementRightAnchor(targetElement, body, bodyRect, '.t-name-text')
      : elementRightAnchor(targetElement, body, bodyRect, '.tree-source-marker-path');
    const gutterX = Math.min(width - 8, Math.max(from.x + 18, to.x + 10));
    segments.push({
      id: `${row.dataset.treeRelative ?? row.dataset.path ?? index}->${normalizedSourceTarget}`,
      external,
      d: [
        `M ${roundSvg(from.x)} ${roundSvg(from.y)}`,
        `H ${roundSvg(gutterX)}`,
        `V ${roundSvg(to.y)}`,
        `H ${roundSvg(to.x)}`,
      ].join(' '),
    });
  });

  return { width, height, segments };
}

function symlinkSourceMarkerForEntry(entry: WorkspaceTreeEntry): SymlinkSourceMarker | null {
  if (entry.kind !== 'symlink' || !entry.symlinkTarget) return null;
  const skillPool = accountSkillPoolForSymlink(entry);
  if (skillPool) {
    return {
      target: skillPool,
      label: 'account skills',
      displayPath: compactSourcePath(skillPool),
      kind: 'skillPool',
    };
  }
  const target = entry.symlinkTarget;
  return {
    target,
    label: 'source',
    displayPath: compactSourcePath(target),
    kind: looksLikeFilePath(target) ? 'file' : 'folder',
  };
}

function accountSkillPoolForSymlink(entry: WorkspaceTreeEntry): string | null {
  const relative = normalizeFsPath(entry.path);
  const target = normalizeFsPath(entry.symlinkTarget ?? '');
  const isSkillProjection = relative.endsWith('/.agents/skills')
    || relative.includes('/.agents/skills/')
    || relative.endsWith('/.claude/skills')
    || relative.includes('/.claude/skills/');
  if (!isSkillProjection) return null;
  if (target.endsWith('/skills')) return target;
  const index = target.lastIndexOf('/skills/');
  if (index < 0) return parentPath(target);
  return target.slice(0, index + '/skills'.length);
}

function parentPath(path: string): string | null {
  const normalized = normalizeFsPath(path);
  const index = normalized.lastIndexOf('/');
  if (index <= 0) return null;
  return normalized.slice(0, index);
}

function elementRightAnchor(
  element: HTMLElement,
  scrollParent: HTMLElement,
  parentRect: DOMRect,
  selector: string,
) {
  const anchor = element.querySelector<HTMLElement>(selector) ?? element;
  const rect = anchor.getBoundingClientRect();
  return {
    x: rect.right - parentRect.left + scrollParent.scrollLeft,
    y: rect.top - parentRect.top + scrollParent.scrollTop + rect.height / 2,
  };
}

function sameSymlinkOverlay(a: SymlinkLinkOverlay, b: SymlinkLinkOverlay) {
  if (a.width !== b.width || a.height !== b.height || a.segments.length !== b.segments.length) {
    return false;
  }
  return a.segments.every((segment, index) => (
    segment.id === b.segments[index]?.id
    && segment.d === b.segments[index]?.d
    && segment.external === b.segments[index]?.external
  ));
}

function roundSvg(value: number): number {
  return Math.round(value * 10) / 10;
}

function normalizeFsPath(path: string): string {
  const normalized = path.replace(/\\/g, '/').replace(/\/+/g, '/');
  return normalized.length > 1 ? normalized.replace(/\/+$/, '') : normalized;
}

function isPathInsideRoot(path: string, root: string): boolean {
  const normalizedPath = normalizeFsPath(path);
  const normalizedRoot = normalizeFsPath(root);
  return normalizedPath === normalizedRoot || normalizedPath.startsWith(`${normalizedRoot}/`);
}

function compactSourcePath(path: string): string {
  const normalized = normalizeFsPath(path);
  const parts = normalized.split('/').filter(Boolean);
  if (parts.length <= 2) return normalized;
  return `.../${parts.slice(-2).join('/')}`;
}

function looksLikeFilePath(path: string): boolean {
  return basename(path).includes('.');
}

function agentIdForWorkspaceEntry(entry: WorkspaceTreeEntry): AgentId | null {
  const parts = normalizeFsPath(entry.path).split('/').filter(Boolean);
  const agentId = parts.length === 2 && parts[0] === '.agent-workspaces'
    ? parts[1]
    : entry.name;
  return agentId ? (agentId as AgentId) : null;
}

function agentInitials(name: string): string {
  const words = name
    .replace(/\b(dr|mr|mrs|ms|prof)\.?\b/gi, ' ')
    .split(/[\s._-]+/)
    .map((word) => word.trim())
    .filter(Boolean);
  const source = words.length > 0 ? words : [name.trim()];
  return source
    .slice(0, 2)
    .map((word) => word[0]?.toUpperCase() ?? '')
    .join('') || 'A';
}

function diffParticipantLabel(participant: WorkspaceTreeChangeParticipant): string {
  if (participant.kind === 'human') return 'Human';
  const aka = participant.aka?.trim();
  if (aka) return aka;
  const trimmed = participant.displayName.trim();
  return trimmed.split(' v. ')[0]?.trim() || trimmed || 'Agent';
}

function shortDisplayName(name: string): string {
  const trimmed = name.trim();
  if (trimmed.length <= 12) return trimmed;
  return `${trimmed.slice(0, 10)}…`;
}

function basename(path: string): string {
  const trimmed = path.replace(/\/+$/, '');
  const index = trimmed.lastIndexOf('/');
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

function projectDisplayName(name: string): string {
  const trimmed = name.trim();
  const parts = trimmed.split(/[\\/]/).filter(Boolean);
  return parts.at(-1) ?? trimmed;
}

function setDragPath(dataTransfer: DataTransfer, path: string) {
  beginDragPaths([path]);
  dataTransfer.effectAllowed = 'copy';
  setTransferData(dataTransfer, TREE_DRAG_MIME, path);
  setTransferData(dataTransfer, 'text/plain', path);
  setTransferData(dataTransfer, 'text/uri-list', pathToFileUri(path));
}

function setTransferData(dataTransfer: DataTransfer, type: string, value: string) {
  try {
    dataTransfer.setData(type, value);
  } catch {
    // WebKit may reject some MIME types for internal drags; the window-level
    // fallback above keeps same-WebView composer drops working.
  }
}

function beginDragPaths(paths: string[]) {
  if (typeof window === 'undefined') return;
  if (window.__KOTA_FILE_TREE_DRAG_CLEAR__) {
    window.clearTimeout(window.__KOTA_FILE_TREE_DRAG_CLEAR__);
  }
  window.__KOTA_FILE_TREE_DRAG_PATHS__ = paths;
}

function clearDragPaths() {
  if (typeof window === 'undefined') return;
  window.__KOTA_FILE_TREE_DRAG_CLEAR__ = window.setTimeout(() => {
    delete window.__KOTA_FILE_TREE_DRAG_PATHS__;
    delete window.__KOTA_FILE_TREE_DRAG_CLEAR__;
  }, 1200);
}

function pathToFileUri(path: string): string {
  return `file://${path.split('/').map((part) => encodeURIComponent(part)).join('/')}`;
}

function FolderIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
         strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 5 Q2 3 4 3 H6 L7.5 4.5 H13 Q14 4.5 14 6 V12 Q14 13 13 13 H3 Q2 13 2 12 Z" />
    </svg>
  );
}

function FileIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
         strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
      <path d="M4 2 H10 L13 5 V13 Q13 14 12 14 H4 Q3 14 3 13 V3 Q3 2 4 2 Z" />
      <path d="M10 2 V5 H13" />
    </svg>
  );
}

function LinkIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor"
         strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6.8 5.2 8 4a3 3 0 0 1 4.2 4.2l-1.4 1.4" />
      <path d="M9.2 10.8 8 12a3 3 0 0 1-4.2-4.2l1.4-1.4" />
      <path d="M6.3 9.7 9.7 6.3" />
    </svg>
  );
}
