/** Workspace file-tree model.
 *
 *  The left column is a raw filesystem inventory, not a preview/editor
 *  surface. Paths are relative to one of the two allowed workspace roots:
 *  Project Files (`sourceDir`) or Project Workspace (`localRoot`). */

export type WorkspaceTreeRootKind = 'projectFiles' | 'projectWorkspace';
export type TreePath = string;

export interface WorkspaceTreeChangeOverview {
  added: number;
  modified: number;
  deleted: number;
  untracked: number;
}

export type WorkspaceTreeChangeStatus = 'added' | 'modified' | 'deleted' | 'untracked' | string;

export interface WorkspaceTreeChangeParticipant {
  actorId: string;
  displayName: string;
  aka?: string | null;
  kind: 'human' | 'agent' | string;
  provider?: string | null;
  avatarId?: string | null;
  status: WorkspaceTreeChangeStatus;
  addedLines?: number | null;
  deletedLines?: number | null;
}

export interface WorkspaceTreeFileChange {
  status: WorkspaceTreeChangeStatus;
  addedLines?: number | null;
  deletedLines?: number | null;
  participants: WorkspaceTreeChangeParticipant[];
}

export type WorkspaceDiffScope =
  | { type: 'all' }
  | { type: 'folder'; prefix: string }
  | { type: 'file'; path: string };

export interface WorkspaceDiffChangeEntry {
  path: TreePath;
  absolutePath: string;
  fileChange: WorkspaceTreeFileChange;
}

export interface WorkspaceFileDiffRequest {
  projectId: string;
  relativePath: string;
  actorId?: string | null;
}

export interface WorkspaceFileDiffResult {
  path: TreePath;
  segments: WorkspaceFileDiffSegment[];
}

export interface WorkspaceFileDiffSegment {
  actorId: string;
  displayName: string;
  aka?: string | null;
  kind: 'human' | 'agent' | string;
  provider?: string | null;
  avatarId?: string | null;
  status: WorkspaceTreeChangeStatus;
  addedLines?: number | null;
  deletedLines?: number | null;
  binary: boolean;
  truncated: boolean;
  hunks: WorkspaceFileDiffHunk[];
}

export interface WorkspaceFileDiffHunk {
  header: string;
  lines: WorkspaceFileDiffLine[];
  omittedLines: number;
}

export interface WorkspaceFileDiffLine {
  kind: 'add' | 'del' | 'ctx' | 'meta' | string;
  text: string;
  oldLine?: number | null;
  newLine?: number | null;
}

export interface WorkspaceTreeRootInfo {
  kind: WorkspaceTreeRootKind;
  label: string;
  absolutePath: string;
  changeOverview?: WorkspaceTreeChangeOverview | null;
}

export interface WorkspaceTreeEntry {
  name: string;
  /** Slash-joined path relative to the section root. Empty only for roots. */
  path: TreePath;
  absolutePath: string;
  kind: 'file' | 'folder' | 'symlink';
  isHidden: boolean;
  size?: number | null;
  modifiedAt?: string | null;
  symlinkTarget?: string | null;
  isWorktree?: boolean;
  worktreeSource?: string | null;
  agentDisplayName?: string | null;
  changeOverview?: WorkspaceTreeChangeOverview | null;
  treeHasChanges?: boolean;
  isGhost?: boolean;
  fileChange?: WorkspaceTreeFileChange | null;
}

export interface WorkspaceTreeListing {
  root: WorkspaceTreeRootInfo;
  entries: WorkspaceTreeEntry[];
}

export interface WorkspaceTreePathRequest {
  projectId: string;
  rootKind: WorkspaceTreeRootKind;
  relativePath?: string;
}

// Legacy preview-tree types are kept so old static preview fixtures keep
// compiling while the live left column moves to WorkspaceTreeEntry.
export interface TreeFile {
  kind: 'file';
  name: string;
  path: TreePath;
  hot?: boolean;
  size?: number;
  modifiedAt?: string;
  preview?: string;
}

export interface TreeFolder {
  kind: 'folder';
  name: string;
  path: TreePath;
  children: TreeNode[];
  defaultOpen?: boolean;
}

export type TreeNode = TreeFile | TreeFolder;
