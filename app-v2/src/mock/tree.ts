import type { TreeNode } from '../types/tree';

/** Mock file tree mirroring the current `app-v2/` structure so the
 *  left column reads realistic while a Tauri FS-listing command is
 *  still pending. Some files carry `hot: true` (Violet ember glow)
 *  and an inline `preview` string used by FilePreviewPopup. Keep
 *  previews short — they're placeholders for the real file content. */

export const MOCK_TREE: TreeNode[] = [
  {
    kind: 'folder',
    name: 'app-v2',
    path: 'app-v2',
    defaultOpen: true,
    children: [
      {
        kind: 'folder',
        name: 'src',
        path: 'app-v2/src',
        defaultOpen: true,
        children: [
          {
            kind: 'folder',
            name: 'chrome',
            path: 'app-v2/src/chrome',
            defaultOpen: true,
            children: [
              { kind: 'file', name: 'TopBar.tsx',    path: 'app-v2/src/chrome/TopBar.tsx',    hot: true,
                size: 2800, modifiedAt: '2026-04-23' },
              { kind: 'file', name: 'AgentBar.tsx',  path: 'app-v2/src/chrome/AgentBar.tsx',  hot: true,
                size: 3400, modifiedAt: '2026-04-23' },
              { kind: 'file', name: 'FileTree.tsx',  path: 'app-v2/src/chrome/FileTree.tsx',
                size: 4100, modifiedAt: '2026-04-23' },
              { kind: 'file', name: 'ColorPicker.tsx', path: 'app-v2/src/chrome/ColorPicker.tsx',
                size: 3200, modifiedAt: '2026-04-22' },
              { kind: 'file', name: 'Hearth.tsx',    path: 'app-v2/src/chrome/Hearth.tsx',
                size: 900,  modifiedAt: '2026-04-22' },
              { kind: 'file', name: 'Stage.tsx',     path: 'app-v2/src/chrome/Stage.tsx',
                size: 15200, modifiedAt: '2026-04-23' },
              { kind: 'file', name: 'Tether.tsx',    path: 'app-v2/src/chrome/Tether.tsx',
                size: 2600, modifiedAt: '2026-04-22' },
            ],
          },
          {
            kind: 'folder',
            name: 'popups',
            path: 'app-v2/src/popups',
            children: [
              { kind: 'file', name: 'HotMemoryPopup.tsx', path: 'app-v2/src/popups/HotMemoryPopup.tsx', size: 2100 },
              { kind: 'file', name: 'WhiteboardPanel.tsx', path: 'app-v2/src/popups/WhiteboardPanel.tsx', size: 1800 },
              { kind: 'file', name: 'RowPopup.tsx', path: 'app-v2/src/popups/RowPopup.tsx', size: 1400 },
            ],
          },
          { kind: 'file', name: 'App.tsx',  path: 'app-v2/src/App.tsx',  size: 4700, modifiedAt: '2026-04-23' },
          { kind: 'file', name: 'main.tsx', path: 'app-v2/src/main.tsx', size: 300 },
        ],
      },
      { kind: 'file', name: 'package.json', path: 'app-v2/package.json', size: 860, modifiedAt: '2026-04-21' },
      { kind: 'file', name: 'README.md',    path: 'app-v2/README.md',    size: 1600, modifiedAt: '2026-04-20' },
    ],
  },
  {
    kind: 'folder',
    name: 'product-design',
    path: 'product-design',
    children: [
      { kind: 'file', name: 'Memory System Design.md', path: 'product-design/Memory System Design.md',
        size: 28400, modifiedAt: '2026-04-20' },
      { kind: 'file', name: 'Village Map UI Design.md', path: 'product-design/Village Map UI Design.md',
        size: 9600, modifiedAt: '2026-04-18' },
    ],
  },
  {
    kind: 'folder',
    name: '.context',
    path: '.context',
    children: [
      {
        kind: 'folder',
        name: 'design-brief',
        path: '.context/design-brief',
        defaultOpen: true,
        children: [
          { kind: 'file', name: '01-ui-spec.md',  path: '.context/design-brief/01-ui-spec.md',  size: 24300 },
          { kind: 'file', name: '02-multi-terminal-layouts.md', path: '.context/design-brief/02-multi-terminal-layouts.md', size: 14800 },
          { kind: 'file', name: '03-UI-expansion-requirements.md',
            path: '.context/design-brief/03-UI-expansion-requirements.md',
            hot: true, size: 22000, modifiedAt: '2026-04-23',
            preview: `# Kota — UI Expansion Requirements

**Audience**: Claude Design (next CD iteration).
**Status**: 2026-04-23 — first cut after M1–M3 engineering lands and live
visual validation surfaces gaps.

## §1 Why we need this now

After M1 (CD static shell port), M2 (Fire/Magic/Flora + Room/Desk picker),
and M3 (swap animations), we've driven a live Kota.app and found four
blockers for real usage:

- Terminal occlusion blocks agent switching
- Seat cards are too visually heavy
- No document tree
- No project switcher, no Tavern entry

## §2 New surfaces

- **S10** — Agent Bar (floating right edge)
- **S11** — Tilted on-table seats
- **S12** — Left file tree + preview popup
- **S13** — Top bar with project tabs + Tavern

## §3 User-locked decisions

Tavern is the single entry for Recruit / Roster / Settings / Templates.
File tree uses warm Ghibli language, not cool VSCode chrome. Preview
popup opens in-place, not in an editor.
` },
        ],
      },
    ],
  },
];
