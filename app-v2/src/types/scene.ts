/** Scene types — mirror the structures CD ships in `canvas/data.jsx`.
 *  M1 uses mock data; M4+ replaces this with live PTY state + Violet log. */

import type { ReactNode } from 'react';

export type AgentId = string;

export interface Agent {
  name: string;
  emoji: string;
  role: string;
  hue: string;
  captain?: boolean;
  avatarId?: string | null;
  avatarClass?: string;
  lifecycleStatus?: 'archived' | 'left' | string;
}

export type SeatState = 'speaking' | 'thinking' | 'idle' | 'empty';

export interface SeatPosition {
  id: AgentId;
  left: number;
  top: number;
  facing: 'up' | 'down';
}

export type TerminalFocus = 'focused' | 'behind' | 'collapsed';
export type TerminalStatus = 'running' | 'blocked' | 'idle';

export type TerminalLineKind =
  | 'prompt' | 'cmd' | 'dim' | 'tool' | 'ok' | 'warn' | 'err'
  | 'file' | 'path' | 'str' | 'num' | 'kw' | 'ai'
  | 'diff-add' | 'diff-del' | 'mention';

export interface TerminalLine {
  kind: TerminalLineKind;
  text: string;
  cursor?: boolean;
  streaming?: boolean;
}

export interface Terminal {
  id: string;
  ownerId: AgentId;
  status: TerminalStatus;
  focus: TerminalFocus;
  behindRank?: number;
  z?: number;
  nudgeX?: number;
  nudgeY?: number;
  streaming?: boolean;
  lines: TerminalLine[];
}

export interface StripCard {
  ownerId: AgentId;
  live: boolean;
}

export interface DigestBlock {
  who: string;
  when: string;
  body: ReactNode | null;
}

export type HotMemKind = 'file' | 'quote' | 'code' | 'memo' | 'link';

export interface HotMemTile {
  who: string;
  kind: HotMemKind;
  label: string;
  hits: number;
  heat: number;       // 0..1
  last: string;
  note: string;
}

export interface HotMemory {
  totalRecords: number;
  topFile: string;
  topFileHits: number;
  tiles: HotMemTile[];
}

export interface ThreadPart {
  who: string;
  t: string;
  body: string;
  side?: boolean;
  think?: boolean;
  tool?: boolean;
  ok?: boolean;
  warn?: boolean;
}

export interface LogRow {
  id: string;
  who: string;
  time: string;
  live?: boolean;
  text: string;
  thread?: ThreadPart[];
}

export interface LogPhase {
  id: string;
  title: string;
  time?: string;
  date?: string;
  turns?: number;
  agents?: AgentId[];
  rows: LogRow[];
}

export interface MeetingLog {
  current: LogPhase;
  history: LogPhase[];
}

export type WhiteboardKind = 'geom' | 'layout' | 'tree' | 'heat';

export interface WhiteboardAlbum {
  id: string;
  title: string;
  owner: string;
  edited: string;
  thumbKind: WhiteboardKind;
}

export interface WhiteboardProposedEdit {
  by: string;
  when: string;
  summary: string;
  accept: string;
  decline: string;
}

export interface Whiteboard {
  activeId: string;
  albums: WhiteboardAlbum[];
  proposedEdit: WhiteboardProposedEdit;
}

export interface Scene {
  caption: { title: string; sub: string };
  seatStates: Record<AgentId, SeatState>;
  terminals: Terminal[];
  strip: StripCard[];
  digest: DigestBlock;
  hotMem: HotMemory;
  log: MeetingLog;
  whiteboard: Whiteboard;
}
