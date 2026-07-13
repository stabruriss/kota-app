/** Mock scene data — ported verbatim from
 *  `.context/attachments/cd-handoff-3/kota-ui/project/canvas/data.jsx`
 *  to give M1 a realistic static canvas to render. M4+ replaces this
 *  with live PTY state + Violet's normalized log + Hot Memory file. */

import type { Agent, AgentId, Scene, SeatPosition } from '../types/scene';
import type { Project } from '../types/project';
import type { WorkingHero } from '../types/agentbar';

/** Mock project tabs for the top bar. Real data will come from a
 *  Tauri FS command (scan ~/Kota/Workspaces/ or similar) later. */
export const MOCK_PROJECTS: readonly Project[] = [
  { id: 'quito',     name: 'quito' },
  { id: 'kota',      name: 'kota' },
  { id: 'velleity',  name: 'velleity', activity: true },
  { id: 'meridian',  name: 'meridian-dashboard-alpha', dirty: true },
  { id: 'scratch',   name: 'scratch' },
];
export const DEFAULT_PROJECT_ID = 'kota';

export const AGENTS: Record<AgentId, Agent> = {
  judy:   { name: 'JUDY',   emoji: '👻', role: 'Captain · Ops',      hue: 'var(--brass-bright)', captain: true },
  'hero-cc': { name: 'CC',  emoji: '◇', role: 'Claude Code',         hue: 'var(--copper)' },
  'hero-dex': { name: 'Dex', emoji: '✦', role: 'Codex',              hue: 'var(--lavender)' },
  'hero-gem': { name: 'Gem', emoji: '◆', role: 'Antigravity CLI',     hue: 'var(--sky)' },
  'hero-op': { name: 'Op', emoji: '◆', role: 'OpenCode',              hue: 'var(--sky)' },
  alice:  { name: 'CC',     emoji: '◇', role: 'Claude Code',         hue: 'var(--copper)' },
  bob:    { name: 'Dex',    emoji: '✦', role: 'Codex',               hue: 'var(--lavender)' },
  charlie: { name: 'Op',    emoji: '◆', role: 'OpenCode',            hue: 'var(--sky)' },
  david:  { name: 'Agy',    emoji: '✧', role: 'Antigravity CLI',     hue: 'var(--mist)' },
  violet: { name: 'VIOLET', emoji: '🪻', role: 'Librarian · Memory', hue: 'var(--mist)' },
  ember:  { name: 'EMBER',  emoji: '🔥', role: 'Watcher · Tests',    hue: 'var(--terra-hi)' },
  fen:    { name: 'FEN',    emoji: '🦉', role: 'Scout · Web',        hue: 'var(--sky)' },
};

export const WORKING_HEROES: readonly WorkingHero[] = [
  { id: 'hero-cc', cli: 'claude', name: 'CC', record: '42 turns' },
  { id: 'hero-dex', cli: 'codex', name: 'Dex', record: '31 runs' },
  { id: 'hero-gem', cli: 'antigravity', name: 'Gem', record: 'default', available: false },
  { id: 'hero-op', cli: 'opencode', name: 'Op', record: 'opencode/deepseek-v4-flash-free', available: false },
];

// Slot order = clockwise walk starting at the left wing, so the agent bar
// (and ⌘1-⌘8, which bind to slots) reads as one continuous sweep around
// the table: left wing → top row → right wing → bottom row (right to left).
export const SEAT_POSITIONS: SeatPosition[] = [
  { id: 'left-wing',  left: 20,  top: 405, facing: 'up' },
  { id: 'bob',    left: 200, top: 292, facing: 'up' },
  { id: 'judy',   left: 485, top: 292, facing: 'up' },
  { id: 'fen',    left: 770, top: 292, facing: 'up' },
  { id: 'right-wing', left: 924, top: 405, facing: 'up' },
  { id: 'ember',  left: 830, top: 520, facing: 'up' },
  { id: 'violet', left: 485, top: 555, facing: 'up' },
  { id: 'alice',  left: 140, top: 520, facing: 'up' },
];

const SCENE_CONV: Scene = {
  caption: { title: 'Three voices.', sub: 'Judy · Alice · Bob · live' },
  seatStates: {
    judy:   'speaking',
    alice:  'thinking',
    bob:    'idle',
    violet: 'idle',
    ember:  'empty',
    fen:    'empty',
  },
  terminals: [
    { id: 'alice', ownerId: 'alice', status: 'running', focus: 'focused', streaming: true,
      lines: [
        { kind: 'prompt', text: 'alice ~ kota-ui/src $ ' },
        { kind: 'cmd',    text: 'claude "add a ThinkingBlock to ChatModal"' },
        { kind: 'tool',   text: '● Read(src/components/ChatModal.tsx)' },
        { kind: 'ok',     text: '  ⎿ 412 lines · 9.8kb' },
        { kind: 'tool',   text: '● Read(src/types/messages.ts)' },
        { kind: 'ok',     text: '  ⎿ 88 lines' },
        { kind: 'tool',   text: '● Read(docs/design-system-v2.md)' },
        { kind: 'ok',     text: '  ⎿ 1,240 lines · §thinking-block' },
        { kind: 'ai',     text: "  I'll start with a new ThinkingBlock component —" },
        { kind: 'ai',     text: '  dial gauge, depth 0–3, 90° sweep per the v2 spec.' },
        { kind: 'tool',   text: '● Edit(src/components/messages/ThinkingBlock.tsx)' },
        { kind: 'diff-add', text: '  + export function ThinkingBlock({ depth, text }: Props) {' },
        { kind: 'diff-add', text: '  +   return <div className="thinking" data-depth={depth}>' },
        { kind: 'diff-add', text: '  +     <DialGauge sweep={-90} steps={3} value={depth}/>' },
        { kind: 'diff-add', text: '  +     <span className="thinking-text">{text}</span>' },
        { kind: 'ai',     text: "  Now wiring it into ChatModal's render switch", streaming: true },
      ],
    },
    { id: 'judy', ownerId: 'judy', status: 'blocked', focus: 'behind',
      lines: [
        { kind: 'prompt', text: 'judy ~ kota-captain $ ' },
        { kind: 'cmd',    text: 'route --from user --to alice' },
        { kind: 'ok',     text: '✓ injected · awaiting return summary' },
        { kind: 'dim',    text: '# blocked on @alice (142s)' },
        { kind: 'prompt', text: 'judy ~ kota-captain $ ', cursor: true },
      ],
    },
    { id: 'bob', ownerId: 'bob', status: 'running', focus: 'behind',
      lines: [
        { kind: 'prompt', text: 'bob ~ kota-review $ ' },
        { kind: 'cmd',    text: 'claude --agent reviewer "spec-check: ThinkingBlock"' },
        { kind: 'tool',   text: '● Read(docs/design-system-v2.md#thinking-block)' },
        { kind: 'ai',     text: '  Dial gauge preferred — depth 0–3, 90° sweep.' },
        { kind: 'tool',   text: '● Comment(draft)' },
      ],
    },
  ],
  strip: [
    { ownerId: 'violet', live: true  },
    { ownerId: 'charlie', live: false },
    { ownerId: 'david',   live: false },
    { ownerId: 'fen',    live: false },
    { ownerId: 'ember',  live: false },
  ],
  digest: {
    who: 'Violet',
    when: 'live · 4 unread since you stepped away',
    body: (
      <>
        <b>Judy</b> asked <b>Alice</b> to wire the new ThinkingBlock and <b>Bob</b> to spec-check it against v2.
        Alice is editing <code>ThinkingBlock.tsx</code> now; Bob has the dial-gauge interpretation on draft comment.
        <br/><br/>
        No one has opened the auth memo this session.
      </>
    ),
  },
  hotMem: {
    totalRecords: 15,
    topFile: 'ChatModal.tsx',
    topFileHits: 8,
    tiles: [
      { who: 'ALICE',  kind: 'file',  label: 'src/components/ChatModal.tsx',              hits: 8, heat: 1.00, last: '2 min ago',  note: 'Currently editing — ThinkingBlock import landing.' },
      { who: 'ALICE',  kind: 'file',  label: 'src/components/messages/ThinkingBlock.tsx', hits: 6, heat: 0.92, last: '1 min ago',  note: 'New component · dial gauge, depth 0–3.' },
      { who: 'BOB',    kind: 'quote', label: '"Dial gauge preferred — 90° sweep."',       hits: 5, heat: 0.78, last: '3 min ago',  note: 'Reviewer verdict · ds-v2 §thinking-block.' },
      { who: 'JUDY',   kind: 'quote', label: '"Land it today; Bob reviews."',             hits: 4, heat: 0.70, last: '8 min ago',  note: 'Captain framing for the ThinkingBlock task.' },
      { who: 'ALICE',  kind: 'code',  label: 'DialGauge component',                       hits: 4, heat: 0.65, last: '4 min ago',  note: 'Leaf shape · SVG arc, 90° sweep.' },
      { who: 'VIOLET', kind: 'memo',  label: 'memo/auth-decision.md',                     hits: 3, heat: 0.55, last: '15 min ago', note: 'Distilled decision · Supabase Auth, 2–3h.' },
      { who: 'BOB',    kind: 'file',  label: 'docs/design-system-v2.md',                  hits: 3, heat: 0.50, last: '6 min ago',  note: 'Canonical thinking-block section.' },
      { who: 'ALICE',  kind: 'file',  label: 'src/types/messages.ts',                     hits: 2, heat: 0.42, last: '5 min ago',  note: 'Added Thinking message variant.' },
      { who: 'EMBER',  kind: 'quote', label: '"Snapshot on depth-2 failing."',            hits: 2, heat: 0.38, last: '12 min ago', note: 'From the earlier dial-gauge pass.' },
      { who: 'JUDY',   kind: 'quote', label: '"Violet, keep one-line digest only."',      hits: 2, heat: 0.32, last: '22 min ago', note: 'Digest brevity rule, captain.' },
      { who: 'VIOLET', kind: 'memo',  label: 'memo/routing-proposal.md',                  hits: 1, heat: 0.24, last: 'yesterday',  note: 'Earlier routing discussion.' },
      { who: 'ALICE',  kind: 'code',  label: 'ChatModal render switch',                   hits: 1, heat: 0.20, last: '9 min ago',  note: 'Where new message types plug in.' },
      { who: 'FEN',    kind: 'link',  label: 'tauri.app/docs/deep-linking',               hits: 1, heat: 0.16, last: '1h ago',     note: 'OAuth callback confirmed working.' },
      { who: 'BOB',    kind: 'quote', label: '"Avoid 3-dot meter."',                      hits: 1, heat: 0.12, last: '6 min ago',  note: 'What the dial is NOT.' },
      { who: 'JUDY',   kind: 'quote', label: '"Supabase: low complexity."',               hits: 1, heat: 0.08, last: '2h ago',     note: 'From the auth memo discussion.' },
    ],
  },
  log: {
    current: {
      id: 'now-thinking-block',
      title: 'Now · thinking-block feature',
      time: 'live',
      rows: [
        { id: 'r1', who: 'JUDY',  time: '15:41', live: true,
          text: '@alice please wire the new ThinkingBlock into ChatModal — dial gauge style, depth 0–3.',
          thread: [
            { who: 'JUDY',  t: '15:41', body: '@alice please wire the new ThinkingBlock into ChatModal — dial gauge style, depth 0–3.' },
            { who: 'JUDY',  t: '15:41', body: 'Spec reference: docs/design-system-v2.md §thinking-block. Bob will review after.', side: true },
            { who: 'ALICE', t: '15:41', body: "On it. Reading ChatModal + the v2 spec first, then I'll sketch the component." },
          ],
        },
        { id: 'r2', who: 'ALICE', time: '15:41',
          text: 'Thought (depth 2) · Need to read the v2 spec before touching the component. Will also check how ChatModal imports messages today.',
          thread: [
            { who: 'ALICE', t: '15:41', body: 'Thought (depth 2) · Need to read the v2 spec before touching the component. Will also check how ChatModal imports messages today.', think: true },
            { who: 'ALICE', t: '15:41', body: 'Read(src/components/ChatModal.tsx) · 412 lines', tool: true },
            { who: 'ALICE', t: '15:42', body: 'Read(docs/design-system-v2.md#thinking-block) · 1,240 lines', tool: true },
          ],
        },
        { id: 'r3', who: 'ALICE', time: '15:43',
          text: 'Edited src/components/messages/ThinkingBlock.tsx · +38 −0 · new component scaffolded.',
          thread: [
            { who: 'ALICE', t: '15:43', body: 'Edit(src/components/messages/ThinkingBlock.tsx) · +38 −0', tool: true, ok: true },
            { who: 'ALICE', t: '15:43', body: "First cut — dial gauge, depth 0..3, 90° sweep. DialGauge is a leaf SVG component; I'll split it out if reuse appears." },
          ],
        },
        { id: 'r4', who: 'JUDY', time: '15:43',
          text: '@bob can you spec-check the ThinkingBlock Alice is wiring? See design system v2, thinking-block section.',
          thread: [
            { who: 'JUDY', t: '15:43', body: '@bob can you spec-check the ThinkingBlock Alice is wiring? See design system v2, thinking-block section.' },
            { who: 'BOB',  t: '15:43', body: "Picking up now. Reading the spec + Alice's draft diff." },
          ],
        },
        { id: 'r5', who: 'BOB', time: '15:44',
          text: 'Thought (depth 1) · Spec says: dial gauge, 90° sweep, depth 0 = invisible, depth 3 = full red-line.',
          thread: [
            { who: 'BOB', t: '15:44', body: 'Thought (depth 1) · Spec says: dial gauge, 90° sweep, depth 0 = invisible, depth 3 = full red-line.', think: true },
            { who: 'BOB', t: '15:44', body: 'Draft comment: "Dial gauge preferred — confirm it\'s not a 3-dot meter."', tool: true, warn: true },
          ],
        },
      ],
    },
    history: [
      { id: 'dialgauge-fix', title: 'Dial-gauge off-by-one', date: '2026-04-22', time: 'earlier today · 16:00', turns: 8,
        agents: ['ember','alice','bob','judy'],
        rows: [
          { id: 'h1', who: 'EMBER', time: '15:58',
            text: 'Snapshot failed on depth-2 — angle came out -30 instead of -45. Midpoint off-by-one.',
            thread: [
              { who: 'EMBER', t: '15:58', body: 'pnpm test --filter chat · 1 failed, 14 passed', tool: true, warn: true },
              { who: 'EMBER', t: '15:58', body: 'Snapshot failed on depth-2 — angle came out -30 instead of -45. Midpoint off-by-one I think.' },
            ],
          },
          { id: 'h2', who: 'ALICE', time: '15:59',
            text: 'Thought (depth 3) · depth maps 0..3 to 0..-90°; step is -30, not -22.5.',
            thread: [
              { who: 'ALICE', t: '15:59', body: 'If depth maps 0..3 to 0..-90°, the step should be -30, not -22.5.', think: true },
              { who: 'ALICE', t: '16:00', body: 'Edit(ThinkingBlock.tsx) · angle = depth × −30', tool: true, ok: true },
            ],
          },
          { id: 'h3', who: 'BOB', time: '16:00',
            text: '@alice yes — ds-v2 spec: 0°, −30°, −60°, −90°. Your fix is right.',
            thread: [ { who: 'BOB', t: '16:00', body: '@alice yes — ds-v2 spec: 0°, −30°, −60°, −90°. Your fix is right.' } ],
          },
        ],
      },
      { id: 'auth-memo', title: 'Supabase Auth decision', date: '2026-04-21', time: 'yesterday', turns: 137,
        agents: ['judy','alice','violet','fen'],
        rows: [
          { id: 'a1', who: 'VIOLET', time: '14:38',
            text: 'Distilled memo/auth-decision.md from 3 threads · Supabase Auth, 2–3h, low complexity.',
            thread: [ { who: 'VIOLET', t: '14:38', body: 'Saved memo/auth-decision.md · distilled from 3 threads.', tool: true, ok: true } ],
          },
          { id: 'a2', who: 'JUDY', time: '14:42',
            text: "Let's pick this back up after the team's return. The hearth is banked.",
            thread: [ { who: 'JUDY', t: '14:42', body: "Let's pick this back up after the team's return. The hearth is banked." } ],
          },
        ],
      },
      { id: 'tauri-oauth', title: 'Tauri deep-link OAuth', date: '2026-04-20', time: '2 days ago', turns: 42,
        agents: ['fen','alice'],
        rows: [
          { id: 't1', who: 'FEN', time: '11:14',
            text: 'Tauri deep-link OAuth callback confirmed working on macOS + Linux.',
            thread: [ { who: 'FEN', t: '11:14', body: 'Tauri deep-link OAuth callback confirmed working on macOS + Linux. Windows still pending sign.' } ],
          },
        ],
      },
    ],
  },
  whiteboard: {
    activeId: 'dial-gauge-sketch',
    albums: [
      { id: 'dial-gauge-sketch',     title: 'Dial gauge · geometry',       owner: 'ALICE',  edited: '2 min ago', thumbKind: 'geom' },
      { id: 'thinking-block-layout', title: 'ThinkingBlock · layout',      owner: 'ALICE',  edited: '6 min ago', thumbKind: 'layout' },
      { id: 'routing-tree',          title: 'Routing tree · captain model', owner: 'JUDY',   edited: '1h ago',    thumbKind: 'tree' },
      { id: 'memory-heatmap',        title: 'Memory heatmap · sketch',     owner: 'VIOLET', edited: 'yesterday', thumbKind: 'heat' },
    ],
    proposedEdit: {
      by: 'BOB',
      when: 'just now',
      summary: 'Adjust midpoint tick to sit at 45° — currently drawn at 40°.',
      accept: 'Accept',
      decline: 'Decline',
    },
  },
};

export const SCENES: Record<string, Scene> = { conversation: SCENE_CONV };

export type SceneKey = keyof typeof SCENES;
