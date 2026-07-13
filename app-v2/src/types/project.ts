/** Project tab model.
 *
 *  Phase 3 / S13 surface. One Kota window hosts N projects as tabs in
 *  the top bar; each project corresponds to one git repo on disk. The
 *  `id` is a stable slug (derived from the repo path). Real projects
 *  will be populated by a Tauri FS command in a later milestone — for
 *  now, fixtures in `app-v2/src/mock/fixtures.tsx` seed a few. */

export type ProjectId = string;

export interface Project {
  id: ProjectId;
  name: string;
  /** Optional absolute path to the repo root. */
  path?: string;
  /** Browser URL for the connected GitHub repo. */
  githubUrl?: string;
  /** Real local clone of the connected project repo. */
  sourcePath?: string;
  /** Kota account project state path under ~/Kota/Workspaces/{id}. */
  accountPath?: string;
  /** Uncommitted changes — shows a brass pip next to the name. */
  dirty?: boolean;
  /** A non-active tab with agent activity glows its top edge. */
  activity?: boolean;
}
