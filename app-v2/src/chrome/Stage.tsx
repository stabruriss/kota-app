/** Stage — round-table canvas (post-W3, no tile-mode terminals).
 *
 *  After W3 the round-table is purely scene decoration: seats, hearth,
 *  color picker, agent ribbon. Live agent terminals render as floating
 *  windows in <AgentWindowsLayer> (mounted by App.tsx as a sibling),
 *  not as tiles inside the desk.
 *
 *  Per HANDOFF-W-FloatingWindows.md: layoutMode (1/2/3/4 tiles) and the
 *  brass tether are retired — windows live in viewport coords, free of
 *  the scene's transform stack. Seats still show a "pill" affordance
 *  for any agent that has a live PTY (= a window somewhere). */

import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { AnimatePresence, motion } from 'framer-motion';
import type { Agent, AgentId, SeatState } from '../types/scene';
import { AGENTS, SCENES, SEAT_POSITIONS, type SceneKey } from '../mock/fixtures';
import { Hearth, type Centerpiece } from './Hearth';
import { ColorPicker, type DeskTheme, type RoomTheme } from './ColorPicker';
import { AgentRibbon } from './AgentRibbon';
import { PrivacyPopover } from './PrivacyPopover';
import type { AgentAtTable, OffTableAgent, WorkingHero } from '../types/agentbar';
import { WorkingHeroPicker } from './WorkingHeroPicker';
import { AgentCommendButton } from './AgentCommendButton';
import { ProjectAgentName } from './ProjectAgentName';
import { VioletRoomPanel, type VioletComposerRetryRequest } from './VioletRoomPanel';
import { ProjectRulesMedal } from './ProjectRulesMedal';
import type { ProjectAgentCommendSource, ProjectAgentRecord } from '../pty-client';
import { avatarClassForAgentFallback, avatarImageStyleForId } from '../lib/hero-avatars';
import { useFileTreeAgentHover } from '../lib/file-tree-agent-hover';
import { MAX_AGENT_SLOTS } from '../lib/agent-slots';

const SCENE_W = 1120;
const SCENE_H = 660;
const ROOM_RESTORE_OVERLAY_DELAY_MS = 500;
const ROOM_RESTORE_PROGRESS_DELAY_MS = 3000;
// S11-v2 — painted tag geometry. 176 wide, 35° tilt (softer than v1's
// 40° — the parchment + inked border start looking crushed past 40).
const SEAT_DRAG_THRESHOLD_PX = 5;
const SEAT_DRAG_HIT_RADIUS_PX = 95;

// SEAT_POSITIONS stay as-is; seats read as hand-painted wooden cards
// pinned flush to the desk with a brass tack instead of a stick + pin.
const SEAT_W = 176;

// Pill geometry (for open agents — when their terminal is on the desk).
const PILL_W = 176;

// ───────────────────────────────────────────── Seat card ─────
function SeatCard({
  id,
  slotId,
  pos,
  state,
  onClick,
  onDoubleClick,
  pill,
  recruitPickerOpen,
  workingHeroes,
  agentMeta,
  liveAgents,
  unavailableHeroIds,
  onOpenRecruitPicker,
  onCloseRecruitPicker,
  onIncarnateHero,
  onContextMenu,
  onCommend,
  record,
  projectName,
  fileTreeHover,
  working,
  dreaming,
  seatIndex,
  dragging,
}: {
  id: AgentId;
  slotId?: string;
  pos: { left: number; top: number };
  state: SeatState;
  onClick: () => void;
  onDoubleClick?: () => void;
  pill?: boolean;
  recruitPickerOpen?: boolean;
  workingHeroes?: readonly WorkingHero[];
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  liveAgents?: ReadonlySet<AgentId>;
  unavailableHeroIds?: ReadonlySet<AgentId>;
  onOpenRecruitPicker?: () => void;
  onCloseRecruitPicker?: () => void;
  onIncarnateHero?: (hero: WorkingHero) => void;
  onContextMenu?: (id: AgentId, point: { x: number; y: number }) => void;
  onCommend?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  record?: ProjectAgentRecord;
  projectName?: string | null;
  fileTreeHover?: boolean;
  working?: boolean;
  dreaming?: boolean;
  seatIndex?: number;
  dragging?: 'origin' | 'target-swap' | 'target-move' | null;
}) {
  if (state === 'empty') {
    return (
      <div
        className={`seat empty ${recruitPickerOpen ? 'picker-open' : ''} ${dragging === 'target-move' ? 'drop-target drop-move' : ''}`}
        style={{ left: pos.left, top: pos.top }}
        data-seat-index={seatIndex}
        onMouseEnter={onOpenRecruitPicker}
        onMouseLeave={onCloseRecruitPicker}
        onClick={(event) => {
          event.stopPropagation();
          onOpenRecruitPicker?.();
          onClick();
        }}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            onOpenRecruitPicker?.();
          }
          if (event.key === 'Escape') onCloseRecruitPicker?.();
        }}
        role="button"
        tabIndex={0}
        aria-label="Open seat (add agent)"
        aria-expanded={!!recruitPickerOpen}
        data-testid={`seat-${slotId ?? id}`}
      >
        <div className="se-plus">+</div>
        <div className="se-lbl">Open seat</div>
        {recruitPickerOpen && workingHeroes && onIncarnateHero && (
          <WorkingHeroPicker
            heroes={workingHeroes}
            unavailableHeroIds={unavailableHeroIds ?? liveAgents}
            onSelect={onIncarnateHero}
            testIdPrefix={`incarnate-seat-${slotId ?? id}`}
          />
        )}
      </div>
    );
  }
  const agent = agentMeta?.[id] ?? AGENTS[id] ?? {
    name: id,
    emoji: '◇',
    role: 'Working agent',
    hue: 'var(--brass-bright)',
  };
  const pillLeft = pos.left + (SEAT_W - PILL_W) / 2;
  const left = pill ? pillLeft : pos.left;
  const effectiveState = working ? 'thinking' : state;
  return (
    <div
      className={[
        'seat',
        pill ? 'pill' : '',
        effectiveState,
        working ? 'working' : '',
        dreaming ? 'dreaming' : '',
        agent.captain ? 'captain' : '',
        fileTreeHover ? 'file-tree-hover' : '',
        dragging === 'origin' ? 'drag-origin' : '',
        dragging === 'target-swap' ? 'drop-target drop-swap' : '',
        dragging === 'target-move' ? 'drop-target drop-move' : '',
      ].filter(Boolean).join(' ')}
      style={{ left, top: pos.top }}
      data-seat-index={seatIndex}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={(event) => {
        if (!onContextMenu) return;
        event.preventDefault();
        event.stopPropagation();
        onContextMenu(id, { x: event.clientX, y: event.clientY });
      }}
      role="button"
      aria-label={pill ? `Switch focus to ${agent.name}` : `Open ${agent.name}'s terminal`}
      title={`${agent.name} · ${agent.role}`}
      data-pill={pill ? 'true' : 'false'}
      data-working={working ? 'true' : 'false'}
      data-dreaming={dreaming ? 'true' : 'false'}
      data-testid={`seat-${slotId ?? id}`}
    >
      <div className="seat-plate" aria-hidden />
      <div className="seat-front">
        <span
          className={`seat-avatar tavern-avatar-art ${agent.avatarClass ?? providerAvatarClass(id)}`}
          style={avatarImageStyleForId(agent.avatarId)}
          aria-hidden
        >
          <span />
          <i />
          <b />
        </span>
        <div className="seat-name">
          <ProjectAgentName name={agent.name} projectName={projectName} compact />
          {agent.captain && <span className="seat-star">★</span>}
        </div>
        {agent.captain && <div className="seat-lamp" aria-hidden>🕯</div>}
      </div>
      <div className="seat-tack" aria-hidden />
      <AgentCommendButton
        agentId={id}
        agentName={agent.name}
        source="table-card"
        count={record?.commends}
        onCommend={onCommend}
      />
    </div>
  );
}

// ──────────────────────── Fit wrapper (scene ↔ stage scale) ─────
function RtSceneFit({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const PAD_X = 40;
    const update = () => {
      const rect = el.getBoundingClientRect();
      const availW = rect.width  - PAD_X;
      const availH = rect.height;
      const sw = availW / SCENE_W;
      const sh = availH / SCENE_H;
      const s  = Math.min(sw, sh * 1.25, 0.95);
      el.style.setProperty('--rt-scale', Math.max(0.48, s).toFixed(3));
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    window.addEventListener('resize', update);
    return () => {
      ro.disconnect();
      window.removeEventListener('resize', update);
    };
  }, []);
  return <div ref={ref} className="rt-scene-fit">{children}</div>;
}

// ─────────────────────────────────────────────── Stage root ─────
export interface StageProps {
  sceneKey: SceneKey;
  /** Which agents have a live PTY (= a floating window somewhere).
   *  Used only to flip the matching seat into its compact "pill" form
   *  so the desk reads as "this agent is recruited and around." */
  liveAgents: Set<AgentId>;
  workingAgents?: ReadonlySet<AgentId>;
  workingStartedAt?: ReadonlyMap<AgentId, string>;
  dreamingStatusAgents?: ReadonlySet<AgentId>;
  agentsHydrating?: boolean;
  agentHydrationProgress?: { completed: number; total: number } | null;
  /** W4 — minimized agents for the AgentRibbon taskbar. */
  minimizedAgents?: ReadonlySet<AgentId>;
  /** W4 — recruit-order list for ⌘N shortcuts. */
  shortcutAgentsOrdered?: readonly (AgentId | null)[];
  tableSlots?: readonly (AgentId | null)[];
  offTableAgents?: readonly AgentId[];
  targetAgent: AgentId | null;
  chatFilterTargetAgents?: readonly AgentId[];
  chatFilterOpenRequest?: { agentId: AgentId; nonce: number } | null;
  privateAgents?: ReadonlySet<AgentId>;
  privacyControlsEnabled?: boolean;
  onOpenAgent: (id: AgentId) => void;
  onOpenRibbonAgent?: (id: AgentId) => void;
  onTogglePrivacyAgent?: (id: AgentId) => void;
  onToggleAllPrivacy?: () => void;
  onAgentContextMenu?: (
    id: AgentId,
    point: { x: number; y: number },
    source?: ProjectAgentCommendSource,
  ) => void;
  onCommendAgent?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  centerpiece: Centerpiece;
  roomColor: string;
  deskColor: string;
  roomTheme: RoomTheme;
  deskTheme: DeskTheme;
  onChangeCenter: (c: Centerpiece) => void;
  onChangeRoom: (color: string) => void;
  onChangeDesk: (color: string) => void;
  onChangeRoomTheme: (theme: RoomTheme) => void;
  onChangeDeskTheme: (theme: DeskTheme) => void;
  workingHeroes?: readonly WorkingHero[];
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  agentRecords?: Readonly<Record<AgentId, ProjectAgentRecord>>;
  projectName?: string | null;
  onIncarnateHero?: (hero: WorkingHero, seatIndex?: number) => void;
  onOpenAgentAdd?: () => void;
  onOpenAgentSlotAdd?: (seatIndex: number) => void;
  /** Drag one seat card onto another to swap their positions. */
  onSwapSeats?: (fromIndex: number, toIndex: number) => void;
  unavailableHeroIds?: ReadonlySet<AgentId>;
  recruitSeatIndex?: number | null;
  onRecruitSeatIndexChange?: (index: number | null) => void;
  /** Double-click seat / ribbon → focus terminal. */
  onDblClickAgent?: (id: AgentId) => void;
  onOpenAgentTerminal?: (id: AgentId) => void;
  onRetryComposerMessage?: (request: VioletComposerRetryRequest) => boolean | void | Promise<boolean | void>;
  /** Group chat overlay. */
  groupChatOpen?: boolean;
  chatFilterActive?: boolean;
  groupChatUnreadCount?: number;
  unreadAgentIds?: ReadonlySet<AgentId>;
  onToggleGroupChat?: () => void;
  onChatFilterActiveChange?: (active: boolean) => void;
  projectRoot?: string | null;
  projectRulesDir?: string | null;
  children?: ReactNode;
  composer?: ReactNode;
  broadcastPopover?: ReactNode;
}

export function Stage({
  sceneKey,
  liveAgents,
  workingAgents,
  workingStartedAt,
  dreamingStatusAgents,
  agentsHydrating = false,
  agentHydrationProgress,
  minimizedAgents,
  shortcutAgentsOrdered,
  tableSlots: tableSlotsProp,
  offTableAgents: offTableAgentsProp,
  targetAgent,
  chatFilterTargetAgents = [],
  chatFilterOpenRequest,
  privateAgents,
  privacyControlsEnabled = false,
  onOpenAgent,
  onOpenRibbonAgent,
  onTogglePrivacyAgent,
  onToggleAllPrivacy,
  onAgentContextMenu,
  onCommendAgent,
  centerpiece,
  roomColor,
  deskColor,
  roomTheme,
  deskTheme,
  onChangeCenter,
  onChangeRoom,
  onChangeDesk,
  onChangeRoomTheme,
  onChangeDeskTheme,
  workingHeroes = [],
  agentMeta,
  agentRecords,
  projectName,
  onIncarnateHero,
  onOpenAgentAdd,
  onOpenAgentSlotAdd,
  onSwapSeats,
  unavailableHeroIds,
  recruitSeatIndex: recruitSeatIndexProp,
  onRecruitSeatIndexChange,
  onDblClickAgent,
  onOpenAgentTerminal,
  onRetryComposerMessage,
  groupChatOpen = false,
  chatFilterActive: chatFilterActiveProp,
  groupChatUnreadCount = 0,
  unreadAgentIds,
  onToggleGroupChat,
  onChatFilterActiveChange,
  projectRoot,
  projectRulesDir,
  children,
  composer,
  broadcastPopover,
}: StageProps) {
  const [privacyOpen, setPrivacyOpen] = useState(false);
  const [projectRulesOpen, setProjectRulesOpen] = useState(false);
  const [internalRecruitSeatIndex, setInternalRecruitSeatIndex] = useState<number | null>(null);
  const [roomRestoreOverlayVisible, setRoomRestoreOverlayVisible] = useState(false);
  const [roomRestoreProgressVisible, setRoomRestoreProgressVisible] = useState(false);
  const privacyWrapRef = useRef<HTMLDivElement | null>(null);
  const fileTreeHoveredAgent = useFileTreeAgentHover();
  const scene = SCENES[sceneKey]!;
  const tableSlots = tableSlotsProp ?? SEAT_POSITIONS.map((s) => (
    scene.seatStates[s.id] && scene.seatStates[s.id] !== 'empty' ? s.id : null
  ));

  // Seat drag-swap. Press a seated card, move past the threshold to lift it,
  // drop on another seat (occupied = swap, empty = move). Ghost position is
  // driven imperatively; only over-target changes go through React state.
  const [seatDrag, setSeatDrag] = useState<{ fromIndex: number; overIndex: number; x: number; y: number } | null>(null);
  const seatPressRef = useRef<{ index: number; x: number; y: number } | null>(null);
  const seatDragRunRef = useRef<{ fromIndex: number; overIndex: number; rects: (DOMRect | null)[] } | null>(null);
  const seatGhostRef = useRef<HTMLDivElement | null>(null);
  const suppressSeatClickRef = useRef(false);
  const seatRingRef = useRef<HTMLDivElement | null>(null);
  const seatDragEnabled = !!onSwapSeats && !agentsHydrating;

  const endSeatDrag = useCallback((commit: boolean) => {
    const run = seatDragRunRef.current;
    seatPressRef.current = null;
    seatDragRunRef.current = null;
    if (!run) return;
    if (commit && run.overIndex >= 0 && run.overIndex !== run.fromIndex) {
      onSwapSeats?.(run.fromIndex, run.overIndex);
    }
    suppressSeatClickRef.current = true;
    setSeatDrag(null);
  }, [onSwapSeats]);

  useEffect(() => {
    const positionGhost = (x: number, y: number) => {
      const node = seatGhostRef.current;
      if (node) {
        node.style.transform = `translate(${x}px, ${y}px) translate(-50%, -60%) scale(1.06) rotate(1.6deg)`;
      }
    };
    const onMove = (event: PointerEvent) => {
      const press = seatPressRef.current;
      if (press && !seatDragRunRef.current) {
        if (Math.hypot(event.clientX - press.x, event.clientY - press.y) < SEAT_DRAG_THRESHOLD_PX) return;
        const ring = seatRingRef.current;
        if (!ring) {
          seatPressRef.current = null;
          return;
        }
        const rects: (DOMRect | null)[] = SEAT_POSITIONS.map(() => null);
        ring.querySelectorAll<HTMLElement>('[data-seat-index]').forEach((node) => {
          const index = Number(node.dataset.seatIndex);
          if (Number.isInteger(index) && index >= 0 && index < rects.length) {
            rects[index] = node.getBoundingClientRect();
          }
        });
        seatDragRunRef.current = { fromIndex: press.index, overIndex: -1, rects };
        seatPressRef.current = null;
        setSeatDrag({ fromIndex: press.index, overIndex: -1, x: event.clientX, y: event.clientY });
      }
      const run = seatDragRunRef.current;
      if (!run) return;
      positionGhost(event.clientX, event.clientY);
      let over = -1;
      let best = Number.POSITIVE_INFINITY;
      run.rects.forEach((rect, index) => {
        if (!rect || index === run.fromIndex) return;
        const distance = Math.hypot(
          event.clientX - (rect.left + rect.width / 2),
          event.clientY - (rect.top + rect.height / 2),
        );
        if (distance < SEAT_DRAG_HIT_RADIUS_PX && distance < best) {
          best = distance;
          over = index;
        }
      });
      if (over !== run.overIndex) {
        run.overIndex = over;
        setSeatDrag((prev) => (prev ? { ...prev, overIndex: over } : prev));
      }
    };
    const onUp = () => {
      if (seatDragRunRef.current) endSeatDrag(true);
      else seatPressRef.current = null;
    };
    const onCancel = () => endSeatDrag(false);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && seatDragRunRef.current) endSeatDrag(false);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onCancel);
    window.addEventListener('keydown', onKeyDown, true);
    return () => {
      window.removeEventListener('pointermove', onMove);
      window.removeEventListener('pointerup', onUp);
      window.removeEventListener('pointercancel', onCancel);
      window.removeEventListener('keydown', onKeyDown, true);
    };
  }, [endSeatDrag]);

  const onSeatRingPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!seatDragEnabled || event.button !== 0) return;
    const target = event.target instanceof Element ? event.target.closest('[data-seat-index]') : null;
    if (!(target instanceof HTMLElement)) return;
    const index = Number(target.dataset.seatIndex);
    if (!Number.isInteger(index) || !tableSlots[index]) return;
    seatPressRef.current = { index, x: event.clientX, y: event.clientY };
  };
  const onSeatRingClickCapture = (event: { preventDefault: () => void; stopPropagation: () => void }) => {
    if (!suppressSeatClickRef.current) return;
    suppressSeatClickRef.current = false;
    event.preventDefault();
    event.stopPropagation();
  };

  // S10 — derive the project-local table agent row. The current
  // product model has no user-visible off-table agents: one project can
  // expose at most MAX_AGENT_SLOTS active agents in the UI.
  const stateForAgent = (id: AgentId): Exclude<SeatState, 'empty'> => {
    const state = scene.seatStates[id];
    return state && state !== 'empty' ? state : 'idle';
  };
  const onTable: AgentAtTable[] = tableSlots
    .filter((id): id is AgentId => !!id)
    .map((id) => ({
      id,
      state: stateForAgent(id),
      captain: !!(agentMeta?.[id]?.captain ?? AGENTS[id]?.captain),
    }));
  const offTableIds: readonly AgentId[] = offTableAgentsProp ?? [];
  const groupChatAgentIds = onTable.map((agent) => agent.id);
  const groupChatAgentIdsKey = groupChatAgentIds.join('|');
  const offTable: OffTableAgent[] = offTableIds.map((id) => ({
    id,
    live: scene.strip.some((s) => s.ownerId === id && s.live),
  }));
  const privacySet = privateAgents ?? new Set<AgentId>();
  const hasPrivateAgents = privacySet.size > 0;
  const recruitSeatControlled = recruitSeatIndexProp !== undefined;
  const recruitSeatIndex = recruitSeatControlled ? recruitSeatIndexProp : internalRecruitSeatIndex;
  const chatFilterControlled = chatFilterActiveProp !== undefined;
  const [internalChatFilterActive, setInternalChatFilterActive] = useState(false);
  const chatFilterActive = chatFilterControlled ? !!chatFilterActiveProp : internalChatFilterActive;
  const setChatFilterActive = (active: boolean) => {
    if (!chatFilterControlled) setInternalChatFilterActive(active);
    onChatFilterActiveChange?.(active);
  };
  const chatFilterTargetIds = useMemo(() => (
    chatFilterTargetAgents.filter((id, index, ids) => (
      groupChatAgentIds.includes(id) && ids.indexOf(id) === index
    ))
  ), [chatFilterTargetAgents, groupChatAgentIdsKey]);
  const chatFilterEffectiveActive = chatFilterActive && chatFilterTargetIds.length > 0;
  const setRecruitSeatIndex = (index: number | null) => {
    if (!recruitSeatControlled) setInternalRecruitSeatIndex(index);
    onRecruitSeatIndexChange?.(index);
  };

  const toggleChatFilterMode = () => {
    const next = !chatFilterEffectiveActive;
    setChatFilterActive(next);
    if (!groupChatOpen) onToggleGroupChat?.();
  };

  useEffect(() => {
    if (!chatFilterOpenRequest) return;
    if (!chatFilterTargetIds.includes(chatFilterOpenRequest.agentId)) return;
    setChatFilterActive(true);
    if (!groupChatOpen) onToggleGroupChat?.();
  }, [
    chatFilterOpenRequest?.agentId,
    chatFilterOpenRequest?.nonce,
    chatFilterTargetIds,
    groupChatOpen,
    onToggleGroupChat,
  ]);

  useEffect(() => {
    if (!privacyOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        setPrivacyOpen(false);
      }
    };
    const onMouseDown = (e: MouseEvent) => {
      if (!privacyWrapRef.current?.contains(e.target as Node)) setPrivacyOpen(false);
    };
    window.addEventListener('keydown', onKey);
    window.addEventListener('mousedown', onMouseDown);
    return () => {
      window.removeEventListener('keydown', onKey);
      window.removeEventListener('mousedown', onMouseDown);
    };
  }, [privacyOpen]);

  useEffect(() => {
    if (!privacyControlsEnabled && privacyOpen) setPrivacyOpen(false);
  }, [privacyControlsEnabled, privacyOpen]);

  useEffect(() => {
    setRoomRestoreOverlayVisible(false);
    setRoomRestoreProgressVisible(false);
    if (!agentsHydrating) return undefined;

    const overlayTimer = window.setTimeout(() => {
      setRoomRestoreOverlayVisible(true);
    }, ROOM_RESTORE_OVERLAY_DELAY_MS);
    const progressTimer = window.setTimeout(() => {
      setRoomRestoreProgressVisible(true);
    }, ROOM_RESTORE_PROGRESS_DELAY_MS);

    return () => {
      window.clearTimeout(overlayTimer);
      window.clearTimeout(progressTimer);
    };
  }, [agentsHydrating, projectRoot]);

  useEffect(() => {
    if (recruitSeatIndex === null) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof HTMLElement && target.closest('.seat.empty.picker-open')) return;
      setRecruitSeatIndex(null);
    };
    window.addEventListener('pointerdown', onPointerDown);
    return () => window.removeEventListener('pointerdown', onPointerDown);
  }, [recruitSeatIndex]);

  return (
    <div
      className={[
        'stage',
        `room-theme-${roomTheme}`,
        agentsHydrating ? 'agents-hydrating' : '',
      ].join(' ')}
      aria-busy={agentsHydrating ? 'true' : undefined}
      style={{ '--room-tint-color': roomColor } as CSSProperties}
    >
      {/* Room tint lives at .stage level (not inside .rt-scene) so it
          fills the whole room area instead of only the 1120×660 scene
          rectangle — fixes the visible dark border around the desk. */}
      <div className="room-tint" data-testid="room-tint" aria-hidden />

      <div className="stage-room-area">
        {/* Painter stays near the room controls. Target selection lives
            below the table now, directly above the composer. */}
        <div className="stage-tools">
          <ColorPicker
            roomColor={roomColor}
            deskColor={deskColor}
            roomTheme={roomTheme}
            deskTheme={deskTheme}
            centerpiece={centerpiece}
            onChangeRoom={onChangeRoom}
            onChangeDesk={onChangeDesk}
            onChangeRoomTheme={onChangeRoomTheme}
            onChangeDeskTheme={onChangeDeskTheme}
            onChangeCenter={onChangeCenter}
          />
          {privacyControlsEnabled && (
            <div ref={privacyWrapRef} className="privacy-drawer">
              <button
                type="button"
                className={[
                  'picker-trigger',
                  'privacy-trigger',
                  privacyOpen ? 'open' : '',
                  hasPrivateAgents ? 'has-active' : '',
                ].filter(Boolean).join(' ')}
                aria-label="Privacy"
                title="Privacy"
                aria-expanded={privacyOpen}
                data-testid="privacy-trigger"
                onClick={() => setPrivacyOpen((v) => !v)}
              >
                <PrivacyIcon active={hasPrivateAgents} />
                {hasPrivateAgents && <span className="privacy-trigger-dot" aria-hidden />}
              </button>
              {privacyOpen && onTogglePrivacyAgent && onToggleAllPrivacy && (
                <PrivacyPopover
                  agents={onTable}
                  privateAgents={privacySet}
                  liveAgents={liveAgents}
                  agentMeta={agentMeta}
                  onToggleAgent={onTogglePrivacyAgent}
                  onToggleAll={onToggleAllPrivacy}
                />
              )}
            </div>
          )}
        </div>

        <div className="stage-floating-tools">
          <button
            type="button"
            className={`picker-trigger project-rules-trigger ${projectRulesOpen ? 'open' : ''}`}
            aria-label="Open project rules"
            title="Project rules"
            data-testid="project-rules-trigger"
            onClick={() => setProjectRulesOpen((open) => !open)}
          >
            <WhistleIcon />
          </button>
          {onToggleGroupChat && (
            <button
              type="button"
              className={`picker-trigger group-chat-trigger ${groupChatUnreadCount > 0 ? 'has-unread' : ''}`}
              aria-label={`Open Violet room${groupChatUnreadCount > 0 ? `, ${groupChatUnreadCount} updates` : ''}`}
              title={groupChatUnreadCount > 0
                ? `Violet room · ${groupChatUnreadCount} update${groupChatUnreadCount === 1 ? '' : 's'}`
                : 'Violet room'}
              data-testid="group-chat-trigger"
              onClick={onToggleGroupChat}
            >
              <span className="group-chat-trigger-icon" aria-hidden />
              {groupChatUnreadCount > 0 && <span className="group-chat-unread-dot" aria-hidden />}
            </button>
          )}
        </div>

        <div className="rt-stage">
          <RtSceneFit>
            <div className="rt-scene">
              <div
                className={`rt-desk desk-theme-${deskTheme}`}
                style={{ '--desk-tint-color': deskColor } as CSSProperties}
              >
                <div className="desk-tint" data-testid="desk-tint" aria-hidden />
                <div className="rt-hearth">
                  <div className="rt-hearth-ring" />
                </div>
              </div>
              {/* Hearth animation — rendered OUTSIDE the tilted .rt-desk
                  so the sprite stands up (faces camera) instead of being
                  squashed into the desk plane. */}
              <div
                id="hearth-anim-slot"
                className="rt-hearth-slot"
                aria-hidden
              >
                <AnimatePresence mode="sync">
                  <motion.div
                    key={centerpiece}
                    className="rt-hearth-fade"
                    initial={{ opacity: 0 }}
                    animate={{ opacity: 1 }}
                    exit={{ opacity: 0 }}
                    transition={{ duration: 0.22, ease: 'easeOut' }}
                  >
                    <Hearth centerpiece={centerpiece} />
                  </motion.div>
                </AnimatePresence>
              </div>
              <div
                className="seat-ring"
                ref={seatRingRef}
                onPointerDown={onSeatRingPointerDown}
                onClickCapture={onSeatRingClickCapture}
              >
                {SEAT_POSITIONS.map((p, index) => {
                  const agentId = tableSlots[index] ?? null;
                  return (
                    <SeatCard
                      key={p.id}
                      id={agentId ?? p.id}
                      slotId={p.id}
                      seatIndex={index}
                      dragging={seatDrag
                        ? (index === seatDrag.fromIndex
                          ? 'origin'
                          : index === seatDrag.overIndex
                            ? (agentId ? 'target-swap' : 'target-move')
                            : null)
                        : null}
                      pos={p}
                      state={agentId ? stateForAgent(agentId) : 'empty'}
                      onClick={() => {
                        if (!agentsHydrating && agentId) onOpenAgent(agentId);
                      }}
                      onDoubleClick={agentId && onDblClickAgent && !agentsHydrating ? () => onDblClickAgent(agentId) : undefined}
                      onContextMenu={
                        agentId && onAgentContextMenu && !agentsHydrating
                          ? (id, point) => onAgentContextMenu(id, point, 'table-card')
                          : undefined
                      }
                      onCommend={agentId && !agentsHydrating ? onCommendAgent : undefined}
                      record={agentId ? agentRecords?.[agentId] : undefined}
                      projectName={projectName}
                      pill={agentId ? liveAgents.has(agentId) : false}
                      fileTreeHover={agentId ? fileTreeHoveredAgent === agentId : false}
                      working={agentId ? workingAgents?.has(agentId) : false}
                      dreaming={agentId ? dreamingStatusAgents?.has(agentId) : false}
                      recruitPickerOpen={!agentId && recruitSeatIndex === index}
                      workingHeroes={workingHeroes}
                      agentMeta={agentMeta}
                      liveAgents={liveAgents}
                      unavailableHeroIds={unavailableHeroIds}
                      onOpenRecruitPicker={() => {
                        if (agentsHydrating) return;
                        // A drag pointer passes through the ghost onto empty
                        // seats; hovering a drop target must not pop recruit.
                        if (seatDragRunRef.current) return;
                        if (!agentId) setRecruitSeatIndex(index);
                        else onOpenAgent(agentId);
                      }}
                      onCloseRecruitPicker={() => {
                        if (recruitSeatIndex === index) setRecruitSeatIndex(null);
                      }}
                      onIncarnateHero={(hero) => {
                        if (agentsHydrating) return;
                        setRecruitSeatIndex(null);
                        onIncarnateHero?.(hero, index);
                      }}
                    />
                  );
                })}
                {seatDrag && (() => {
                  const ghostId = tableSlots[seatDrag.fromIndex] ?? null;
                  if (!ghostId) return null;
                  const ghostAgent = agentMeta?.[ghostId] ?? AGENTS[ghostId] ?? {
                    name: ghostId,
                    emoji: '◇',
                    role: 'Working agent',
                    hue: 'var(--brass-bright)',
                  };
                  // Portal to <body>: the round table sits under transformed
                  // ancestors (.rt-scene translate/scale), which would hijack
                  // position:fixed and offset the ghost from the pointer.
                  return createPortal(
                    <div
                      className="seat seat-drag-ghost"
                      ref={seatGhostRef}
                      style={{ transform: `translate(${seatDrag.x}px, ${seatDrag.y}px) translate(-50%, -60%) scale(1.06) rotate(1.6deg)` }}
                      aria-hidden
                    >
                      <div className="seat-plate" aria-hidden />
                      <div className="seat-front">
                        <span
                          className={`seat-avatar tavern-avatar-art ${ghostAgent.avatarClass ?? providerAvatarClass(ghostId)}`}
                          style={avatarImageStyleForId(ghostAgent.avatarId)}
                          aria-hidden
                        >
                          <span />
                          <i />
                          <b />
                        </span>
                        <div className="seat-name">{ghostAgent.name}</div>
                      </div>
                    </div>,
                    document.body,
                  );
                })()}
              </div>
            </div>
          </RtSceneFit>
        </div>

        {children}

        {groupChatOpen && (
          <div
            className={[
              'group-chat-overlay',
              chatFilterEffectiveActive ? 'chat-filter-active' : '',
            ].filter(Boolean).join(' ')}
            data-testid="group-chat-overlay"
            data-chat-filter-agents={chatFilterEffectiveActive ? chatFilterTargetIds.join('|') : undefined}
          >
            <VioletRoomPanel
              projectRoot={projectRoot}
              agentIds={groupChatAgentIds}
              chatFilterActive={chatFilterEffectiveActive}
              chatFilterAgentIds={chatFilterTargetIds}
              agentMeta={agentMeta}
              agentRecords={agentRecords}
              onAgentContextMenu={onAgentContextMenu}
              onCommendAgent={onCommendAgent}
              onOpenAgentTerminal={onOpenAgentTerminal}
              onRetryComposerMessage={onRetryComposerMessage}
              onClose={onToggleGroupChat}
            />
          </div>
        )}
        {projectRulesOpen && (
          <ProjectRulesMedal
            projectName={projectName}
            projectRoot={projectRoot}
            rulesDir={projectRulesDir}
            onClose={() => setProjectRulesOpen(false)}
          />
        )}
        {agentsHydrating && roomRestoreOverlayVisible && (
          <div className="room-restore-overlay" data-testid="room-restore-overlay">
            <div className="room-restore-status" role="status" aria-live="polite">
              <span className="room-restore-spinner" aria-hidden />
              <span className="room-restore-copy">
                <span className="room-restore-title">Restoring Room...</span>
                {roomRestoreProgressVisible && (agentHydrationProgress?.total ?? 0) > 0 && (
                  <span className="room-restore-progress" data-testid="room-restore-progress">
                    Restoring agents · {Math.min(
                      agentHydrationProgress?.completed ?? 0,
                      agentHydrationProgress?.total ?? 0,
                    )}/{agentHydrationProgress?.total ?? 0}
                  </span>
                )}
              </span>
            </div>
          </div>
        )}
      </div>

      <div className="stage-bottom-dock">
        <AgentRibbon
          onTable={onTable}
          offTable={offTable}
          targetAgent={targetAgent}
          privateAgents={privacySet}
          chatFilterActive={chatFilterEffectiveActive}
          chatFilterTargetAgents={chatFilterTargetIds}
          unreadAgentIds={unreadAgentIds}
          onOpenAgent={onOpenRibbonAgent ?? onOpenAgent}
          onToggleChatFilter={toggleChatFilterMode}
          liveAgents={liveAgents}
          workingAgents={workingAgents}
          workingStartedAt={workingStartedAt}
          dreamingStatusAgents={dreamingStatusAgents}
          minimized={minimizedAgents}
          shortcutOrder={shortcutAgentsOrdered}
          agentsHydrating={agentsHydrating}
          agentMeta={agentMeta}
          workingHeroes={workingHeroes}
          unavailableHeroIds={unavailableHeroIds}
          onIncarnateHero={(hero) => onIncarnateHero?.(hero)}
          onAddAgent={onOpenAgentAdd}
          onOpenSlotAdd={onOpenAgentSlotAdd}
          tableFull={tableSlots.slice(0, MAX_AGENT_SLOTS).every(Boolean)}
          onDblClickAgent={onDblClickAgent}
          onAgentContextMenu={
            onAgentContextMenu
              ? (id, point) => onAgentContextMenu(id, point, 'agent-bar')
              : undefined
          }
          onCommendAgent={onCommendAgent}
          agentRecords={agentRecords}
        />
        <div className="stage-composer-stack">
          {composer}
          {broadcastPopover}
        </div>
      </div>

    </div>
  );
}

function PrivacyIcon({ active }: { active?: boolean }) {
  return (
    <svg viewBox="0 0 18 18" fill="none" aria-hidden>
      <path
        d={active ? 'M6.2 7.6V5.9c0-2 1.2-3.3 3-3.3 1.3 0 2.3.7 2.8 1.8' : 'M5.8 7.6V5.9c0-2 1.3-3.3 3.2-3.3s3.2 1.3 3.2 3.3v1.7'}
        stroke="currentColor"
        strokeWidth="1.45"
        strokeLinecap="round"
      />
      <rect
        x="4.1"
        y="7.2"
        width="9.8"
        height="7.2"
        rx="1.7"
        stroke="currentColor"
        strokeWidth="1.45"
      />
      <path d="M9 10.1v2" stroke="currentColor" strokeWidth="1.45" strokeLinecap="round" />
    </svg>
  );
}

function WhistleIcon() {
  return (
    <svg viewBox="0 0 512 512" aria-hidden>
      <path
        fill="currentColor"
        d="M93.75 81.443c-5.38 0-12.368 2.49-22.358 8.967 3.966 4.682 8.167 9.687 16.47 19.256 5.782 6.663 11.618 13.29 16.026 18.088.038.042.055.055.092.096l30.894-17.932-14.652-14.148c-11.292-9.404-18.644-13.866-25.418-14.293-.345-.022-.696-.034-1.055-.034zm120.08 15.082c-.885-.01-1.767-.006-2.643.01-10.46.193-20.2 2.23-26.742 5.424l-67.262 39.038c2.45.544 4.885 1.196 7.287 2.02 17.275 5.923 33.093 18.223 49.568 34.7l216.44 213.5 80.978-44.433L258.54 111.38c-8.656-7.84-22.49-12.908-36.693-14.394-2.677-.28-5.363-.43-8.018-.46zM58.192 102.74c-17.543 20.723-20.57 37.186-15.326 57.004.692 2.618 3.057 6.357 6.373 10.47 2.195-3.144 4.55-6.304 7.086-9.478 3.99-4.995 8.385-9.183 13.085-12.558l-.106-.2 2.768-1.61c1.354-.862 2.73-1.66 4.13-2.393l11.868-6.89c-4.175-4.618-8.94-10.017-13.803-15.622-5.956-6.864-11.732-13.62-16.074-18.723zm184.093 13.438l58.415 61.67c-46.086-5.037-56.79 13.2-69.027 34.2l-57.334-59.304 67.946-36.566zM103.702 157.23c-.714-.016-1.43-.016-2.15.002-6.976.18-14.207 2.058-22.252 5.885-3.035 2.29-5.99 5.196-8.91 8.852-25.77 32.264-30.45 59.135-25.484 83.477 4.965 24.343 20.536 46.656 37.916 66.455 13.314 15.168 28.86 23.992 48.472 27.93 19.614 3.94 43.438 2.708 71.98-3.475 33.246-7.2 66.01 8.42 95.81 27.665 26.118 16.868 50.676 37.09 70.98 49.95l8.79-18.935-217.52-214.57-.022-.022c-15.524-15.524-29.565-25.905-42.682-30.402-5.02-1.722-9.925-2.695-14.928-2.813zm367.08 210.456l-73.45 40.304-10.48 22.567 70.833-38.41 13.096-24.46z"
      />
    </svg>
  );
}

function providerAvatarClass(id: AgentId): string {
  const baseId = id.replace(/-bunshin.*$/i, '').replace(/-\d+$/, '');
  return avatarClassForAgentFallback(null, baseId);
}
