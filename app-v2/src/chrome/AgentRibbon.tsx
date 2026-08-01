/** Agent Ribbon (S10-v2 + W4 taskbar) — horizontal chip strip above
 *  the input bar.
 *
 *  W4 — chips for live agents double as a macOS-style taskbar:
 *    - on-table agent → ⌘N follows recruit order, shown on the bar edge
 *    - minimized live agent → chip is highlighted; click → restore
 *    - non-minimized live agent → chip click → bring window to front
 *    - offline agent → chip stays visible and can be resumed by the slot shortcut
 *    - minimized live agent → chip stays visually muted until restored
 *
 *  Design source:
 *    .context/design-brief/04-phase3-iteration.md §S10-v2
 *    .context/HANDOFF-W-FloatingWindows.md §W4                       */

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type Ref,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type WheelEvent as ReactWheelEvent,
} from 'react';
import { motion } from 'framer-motion';
import type { AgentAtTable, OffTableAgent, WorkingHero } from '../types/agentbar';
import type { Agent, AgentId } from '../types/scene';
import { AGENTS } from '../mock/fixtures';
import { WorkingHeroPicker } from './WorkingHeroPicker';
import { AgentCommendButton } from './AgentCommendButton';
import type { ProjectAgentCommendSource, ProjectAgentRecord } from '../pty-client';
import { splitProjectAgentName } from './ProjectAgentName';
import { avatarClassForAgentFallback, avatarImageStyleForId } from '../lib/hero-avatars';
import { useFileTreeAgentHover } from '../lib/file-tree-agent-hover';
import { MAX_AGENT_SLOTS } from '../lib/agent-slots';

const EMPTY_AGENT_ID_SET: ReadonlySet<AgentId> = new Set();
const EMPTY_UNSUPPORTED_AGENT_PROVIDERS: ReadonlyMap<AgentId, string> = new Map();

export interface AgentRibbonProps {
  onTable: readonly AgentAtTable[];
  offTable: readonly OffTableAgent[];
  targetAgent: AgentId | null;
  privateAgents?: ReadonlySet<AgentId>;
  chatFilterActive?: boolean;
  chatFilterTargetAgents?: readonly AgentId[];
  unreadAgentIds?: ReadonlySet<AgentId>;
  onOpenAgent: (id: AgentId) => void;
  onToggleChatFilter?: () => void;
  workingHeroes?: readonly WorkingHero[];
  onIncarnateHero?: (hero: WorkingHero) => void;
  onAddAgent?: () => void;
  onOpenSlotAdd?: (slotIndex: number) => void;
  tableFull?: boolean;
  unavailableHeroIds?: ReadonlySet<AgentId>;
  onDblClickAgent?: (id: AgentId) => void;
  onAgentContextMenu?: (id: AgentId, point: { x: number; y: number }) => void;
  onCommendAgent?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  unsupportedAgentProviders?: ReadonlyMap<AgentId, string>;
  agentRecords?: Readonly<Record<AgentId, ProjectAgentRecord>>;
  /** W4 — agents that have a live PTY (= a window somewhere). */
  liveAgents?: ReadonlySet<AgentId>;
  /** Agents whose current PTY session is actively working on a turn. */
  workingAgents?: ReadonlySet<AgentId>;
  /** Start timestamp for each visible working turn. */
  workingStartedAt?: ReadonlyMap<AgentId, string>;
  /** Presentation-only overlay: current working turn belongs to Ember Dream. */
  dreamingStatusAgents?: ReadonlySet<AgentId>;
  /** Project agent details are hydrating; show saved seats but block target changes. */
  agentsHydrating?: boolean;
  /** W4 — live agents whose windows are currently minimized. */
  minimized?: ReadonlySet<AgentId>;
  /** W4 — agents that have new snapshot output since last restore. */
  /** Recruit-order list for ⌘ slot shortcuts. */
  shortcutOrder?: readonly (AgentId | null)[];
}

export function AgentRibbon({
  onTable,
  offTable,
  targetAgent,
  privateAgents,
  chatFilterActive,
  chatFilterTargetAgents = [],
  unreadAgentIds = EMPTY_AGENT_ID_SET,
  onOpenAgent,
  onToggleChatFilter,
  workingHeroes = [],
  onIncarnateHero,
  onAddAgent,
  onOpenSlotAdd,
  tableFull,
  unavailableHeroIds,
  onDblClickAgent,
  onAgentContextMenu,
  onCommendAgent,
  agentMeta,
  unsupportedAgentProviders = EMPTY_UNSUPPORTED_AGENT_PROVIDERS,
  agentRecords,
  liveAgents,
  workingAgents,
  workingStartedAt,
  dreamingStatusAgents,
  agentsHydrating = false,
  minimized,
  shortcutOrder,
}: AgentRibbonProps) {
  const fileTreeHoveredAgent = useFileTreeAgentHover();
  const liveSet = liveAgents ?? new Set<AgentId>();
  const workingSet = workingAgents ?? new Set<AgentId>();
  const dreamingSet = dreamingStatusAgents ?? new Set<AgentId>();
  const minSet = minimized ?? new Set<AgentId>();
  const privateSet = privateAgents ?? new Set<AgentId>();
  const chatFilterTargetSet = new Set(chatFilterTargetAgents);
  const agentUnreadSet = unreadAgentIds;
  const showUnreadForAgent = (id: AgentId) => !!chatFilterActive && id !== targetAgent && agentUnreadSet.has(id);
  const heroById = new Map<AgentId, WorkingHero>();
  for (const hero of workingHeroes) heroById.set(hero.id, hero);
  const onTableById = new Map<AgentId, AgentAtTable>();
  for (const agent of onTable) onTableById.set(agent.id, agent);
  const shortcutKey = shortcutOrder?.slice(0, MAX_AGENT_SLOTS).join('|') ?? '';
  // Memoized on the semantic key: a fresh array identity every render used to
  // re-arm the cmd-plate layout measurement (and its observers) each render.
  const fixedSlots = useMemo(
    () => Array.from({ length: MAX_AGENT_SLOTS }, (_, index) => shortcutOrder?.[index] ?? null),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [shortcutKey],
  );
  const hasOff = offTable.length > 0;
  const [expanded, setExpanded] = useState(false);
  const showOff = hasOff && expanded;
  const onTableRowRef = useRef<HTMLDivElement | null>(null);
  const onTableStripRef = useRef<HTMLDivElement | null>(null);
  const chipNodeRef = useRef(new Map<number, HTMLSpanElement>());
  const stripDragRef = useRef({
    active: false,
    pointerId: -1,
    startX: 0,
    startScrollLeft: 0,
    moved: false,
  });
  const suppressStripClickRef = useRef(false);
  const [cmdPlates, setCmdPlates] = useState<readonly { slot: number; x: number }[]>([]);

  const setChipNode = useCallback((slot: number, node: HTMLSpanElement | null) => {
    if (node) chipNodeRef.current.set(slot, node);
    else chipNodeRef.current.delete(slot);
  }, []);

  const measureCmdPlates = useCallback(() => {
    const row = onTableRowRef.current;
    if (!row) {
      setCmdPlates([]);
      return;
    }
    const rowRect = row.getBoundingClientRect();
    const next = Array.from({ length: MAX_AGENT_SLOTS }, (_, index) => index + 1).flatMap((slot) => {
      const node = chipNodeRef.current.get(slot);
      if (!node) return [];
      const rect = node.getBoundingClientRect();
      return [{
        slot,
        x: rect.left - rowRect.left + rect.width / 2,
      }];
    });
    setCmdPlates((prev) => {
      if (
        prev.length === next.length &&
        prev.every((item, index) => {
          const candidate = next[index];
          return candidate && item.slot === candidate.slot && Math.abs(item.x - candidate.x) < 0.5;
        })
      ) {
        return prev;
      }
      return next;
    });
  }, [shortcutKey]);

  useLayoutEffect(() => {
    measureCmdPlates();
    const row = onTableRowRef.current;
    if (!row) return;
    const strip = onTableStripRef.current;
    window.addEventListener('resize', measureCmdPlates);
    strip?.addEventListener('scroll', measureCmdPlates, { passive: true });
    let resizeObserver: ResizeObserver | null = null;
    if (typeof ResizeObserver !== 'undefined') {
      resizeObserver = new ResizeObserver(measureCmdPlates);
      resizeObserver.observe(row);
      chipNodeRef.current.forEach((node) => resizeObserver?.observe(node));
    }
    return () => {
      window.removeEventListener('resize', measureCmdPlates);
      strip?.removeEventListener('scroll', measureCmdPlates);
      resizeObserver?.disconnect();
    };
  }, [measureCmdPlates, showOff]);

  const scrollOnTableStrip = useCallback((delta: number) => {
    const strip = onTableStripRef.current;
    if (!strip || delta === 0) return false;
    const maxScrollLeft = strip.scrollWidth - strip.clientWidth;
    if (maxScrollLeft <= 0) return false;
    const next = Math.max(0, Math.min(maxScrollLeft, strip.scrollLeft + delta));
    if (Math.abs(next - strip.scrollLeft) < 0.5) return false;
    strip.scrollLeft = next;
    window.requestAnimationFrame(measureCmdPlates);
    return true;
  }, [measureCmdPlates]);

  const handleStripWheel = useCallback((event: ReactWheelEvent<HTMLDivElement>) => {
    const dominantDelta = Math.abs(event.deltaX) > Math.abs(event.deltaY)
      ? event.deltaX
      : event.deltaY;
    if (scrollOnTableStrip(dominantDelta)) event.preventDefault();
  }, [scrollOnTableStrip]);

  const handleStripPointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    if (event.target instanceof Element && event.target.closest('button, [role="button"], a, input, textarea, select')) {
      return;
    }
    const strip = onTableStripRef.current;
    if (!strip || strip.scrollWidth <= strip.clientWidth) return;
    stripDragRef.current = {
      active: true,
      pointerId: event.pointerId,
      startX: event.clientX,
      startScrollLeft: strip.scrollLeft,
      moved: false,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }, []);

  const handleStripPointerMove = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = stripDragRef.current;
    if (!drag.active || drag.pointerId !== event.pointerId) return;
    const strip = onTableStripRef.current;
    if (!strip) return;
    const dx = event.clientX - drag.startX;
    if (Math.abs(dx) > 3) {
      drag.moved = true;
      suppressStripClickRef.current = true;
    }
    strip.scrollLeft = drag.startScrollLeft - dx;
    window.requestAnimationFrame(measureCmdPlates);
    if (drag.moved) event.preventDefault();
  }, [measureCmdPlates]);

  const endStripPointerDrag = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = stripDragRef.current;
    if (!drag.active || drag.pointerId !== event.pointerId) return;
    stripDragRef.current = {
      active: false,
      pointerId: -1,
      startX: 0,
      startScrollLeft: 0,
      moved: false,
    };
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }, []);

  const handleStripClickCapture = useCallback((event: ReactMouseEvent<HTMLDivElement>) => {
    if (!suppressStripClickRef.current) return;
    suppressStripClickRef.current = false;
    event.preventDefault();
    event.stopPropagation();
  }, []);


  const centerBySlot = new Map(cmdPlates.map((plate) => [plate.slot, plate.x]));
  const workPlates = fixedSlots.flatMap((id, index) => {
    if (!id || !workingSet.has(id)) return [];
    const slot = index + 1;
    const x = centerBySlot.get(slot);
    if (x == null) return [];
    return [{
      agentId: id,
      startedAt: workingStartedAt?.get(id) ?? null,
      slot,
      target: id === targetAgent,
      chatFilter: !!chatFilterActive && chatFilterTargetSet.has(id),
      dreaming: dreamingSet.has(id),
      x,
    }];
  });

  return (
    <div className="ribbon-wrap" data-testid="agent-ribbon">
      <div className="ribbon">
        <RibbonRow
          className="on-table"
          rowRef={onTableRowRef}
          stripRef={onTableStripRef}
          onStripWheel={handleStripWheel}
          onStripPointerDown={handleStripPointerDown}
          onStripPointerMove={handleStripPointerMove}
          onStripPointerUp={endStripPointerDrag}
          onStripPointerCancel={endStripPointerDrag}
          onStripClickCapture={handleStripClickCapture}
          label={
            <ChatFilterToggleButton
              active={!!chatFilterActive}
              onClick={onToggleChatFilter}
            />
          }
          testId="ribbon-row-on"
          topAdornment={
            workPlates.length > 0 ? (
              <div className="ribbon-work-nameplates" aria-hidden>
                {workPlates.map((plate) => (
                  <span
                    key={plate.agentId}
                    className={[
                      'ribbon-work-nameplate',
                      plate.target ? 'target' : '',
                      plate.chatFilter ? 'chat-filter' : '',
                      plate.dreaming ? 'dreaming' : '',
                    ].filter(Boolean).join(' ')}
                    style={{ left: `${plate.x}px` }}
                  >
                    <i />
                    <WorkElapsedLabel startedAt={plate.startedAt} dreaming={plate.dreaming} />
                  </span>
                ))}
              </div>
            ) : null
          }
          bottomAdornment={
            cmdPlates.length > 0 ? (
              <div className="ribbon-cmd-nameplates" aria-hidden>
                {cmdPlates.map((plate) => (
                  <span
                    key={plate.slot}
                    className="ribbon-cmd-nameplate"
                    style={{ left: `${plate.x}px` }}
                  >
                    ⌘{plate.slot}
                  </span>
                ))}
              </div>
            ) : null
          }
          rightAdornment={
            <>
              {/* `+ add` lives at the right end of whichever row is
                  currently "bottom-most visible" — on-table when the
                  ribbon is collapsed, off-table when expanded. */}
              {!showOff && (
                <AddChip
                  heroes={workingHeroes}
                  unavailableHeroIds={unavailableHeroIds}
                  onIncarnate={onIncarnateHero}
                  onAddAgent={onAddAgent}
                  tableFull={tableFull}
                  disabled={agentsHydrating}
                />
              )}
              {hasOff && (
                <button
                  className={`chev-btn ${expanded ? 'open' : ''}`}
                  onClick={() => setExpanded((v) => !v)}
                  aria-label={expanded ? 'Hide off-table agents' : 'Show off-table agents'}
                  aria-expanded={expanded}
                  data-testid="ribbon-chevron"
                >
                  {!expanded && <span className="chev-ember" aria-hidden />}
                  <svg viewBox="0 0 16 16" fill="none"
                       stroke="currentColor" strokeWidth="1.5"
                       strokeLinecap="round" strokeLinejoin="round">
                    <path d="M4 6 Q6 7.2 8 9 Q10 7.2 12 6" />
                  </svg>
                </button>
              )}
            </>
          }
        >
          {fixedSlots.map((id, index) => {
            if (!id) {
              return (
                <EmptySlotChip
                  key={`empty-${index}`}
                  slot={index + 1}
                  hostRef={(node) => setChipNode(index + 1, node)}
                  disabled={agentsHydrating}
                  onClick={() => {
                    if (!agentsHydrating) onOpenSlotAdd?.(index);
                  }}
                />
              );
            }
            const a: AgentAtTable = onTableById.get(id) ?? { id, state: 'idle' };
            return (
              <AgentChip
                key={id}
                id={id}
                hero={heroById.get(id)}
                agentMeta={agentMeta}
                unsupportedProvider={unsupportedAgentProviders.get(id)}
                state={a.state}
                captain={a.captain}
                target={id === targetAgent}
                chatFilterActive={!!chatFilterActive && chatFilterTargetSet.has(id)}
                unread={showUnreadForAgent(id)}
                privateMode={privateSet.has(id)}
                live={liveSet.has(id)}
                working={workingSet.has(id)}
                dreaming={dreamingSet.has(id)}
                minimized={minSet.has(id)}
                disabled={agentsHydrating}
                cmdSlot={index + 1}
                fileTreeHover={fileTreeHoveredAgent === id}
                hostRef={(node) => setChipNode(index + 1, node)}
                onClick={() => {
                  if (!agentsHydrating) onOpenAgent(id);
                }}
                onDoubleClick={onDblClickAgent && !agentsHydrating ? () => onDblClickAgent(id) : undefined}
                onContextMenu={agentsHydrating ? undefined : onAgentContextMenu}
                onCommend={agentsHydrating ? undefined : onCommendAgent}
                record={agentRecords?.[id]}
              />
            );
          })}
        </RibbonRow>

        {showOff && (
          <motion.div
            className="ribbon-row"
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            transition={{ duration: 0.24, ease: 'easeOut' }}
            data-testid="ribbon-row-off"
          >
            <div className="ribbon-label off">In the alcove</div>
            <div className="chip-strip-wrap">
              <div className="chip-strip">
                {offTable.map((a) => (
                  <AgentChip
                    key={a.id}
                    id={a.id}
                    hero={heroById.get(a.id)}
                    agentMeta={agentMeta}
                    unsupportedProvider={unsupportedAgentProviders.get(a.id)}
                    off
                    live={liveSet.has(a.id)}
                    working={workingSet.has(a.id)}
                    dreaming={dreamingSet.has(a.id)}
                    privateMode={privateSet.has(a.id)}
                    target={a.id === targetAgent}
                    chatFilterActive={!!chatFilterActive && chatFilterTargetSet.has(a.id)}
                    unread={showUnreadForAgent(a.id)}
                    minimized={minSet.has(a.id)}
                    disabled={agentsHydrating}
                    fileTreeHover={fileTreeHoveredAgent === a.id}
                    onClick={() => {
                      if (!agentsHydrating) onOpenAgent(a.id);
                    }}
                    onDoubleClick={onDblClickAgent && !agentsHydrating ? () => onDblClickAgent(a.id) : undefined}
                    onContextMenu={agentsHydrating ? undefined : onAgentContextMenu}
                    onCommend={agentsHydrating ? undefined : onCommendAgent}
                    record={agentRecords?.[a.id]}
                  />
                ))}
                <AddChip
                  heroes={workingHeroes}
                  unavailableHeroIds={unavailableHeroIds}
                  onIncarnate={onIncarnateHero}
                  onAddAgent={onAddAgent}
                  tableFull={tableFull}
                  disabled={agentsHydrating}
                />
              </div>
            </div>
          </motion.div>
        )}
      </div>
    </div>
  );
}

// ───────────────────────── internal bits ─────────────────────────

/** Self-ticking elapsed label. Owns its own interval so the 8-chip ribbon no
 *  longer re-renders on every tick: 100ms while the tenths digit is visible
 *  (first minute), 1s once the display is seconds-granular. */
function WorkElapsedLabel({ startedAt, dreaming }: { startedAt: string | null; dreaming?: boolean }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const parsed = Date.parse(startedAt ?? '');
    let timer: number | null = null;
    const periodFor = () => {
      const elapsed = Number.isFinite(parsed) ? Math.max(0, Date.now() - parsed) : Number.POSITIVE_INFINITY;
      return elapsed < 60_000 ? 100 : 1000;
    };
    const arm = () => {
      const period = periodFor();
      timer = window.setInterval(() => {
        setNow(Date.now());
        if (period === 100 && periodFor() !== 100 && timer != null) {
          window.clearInterval(timer);
          arm();
        }
      }, period);
    };
    setNow(Date.now());
    arm();
    return () => {
      if (timer != null) window.clearInterval(timer);
    };
  }, [startedAt]);
  const label = formatWorkElapsed(startedAt, now);
  return <>{dreaming ? `dreaming · ${label}` : label}</>;
}

function formatWorkElapsed(startedAt: string | null | undefined, now: number): string {
  const parsed = Date.parse(startedAt ?? '');
  const elapsedMs = Number.isFinite(parsed) ? Math.max(0, now - parsed) : 0;
  if (elapsedMs < 60_000) {
    return `${(elapsedMs / 1000).toFixed(1)}s`;
  }
  const totalSeconds = Math.floor(elapsedMs / 1000);
  const totalMinutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${totalMinutes}m${seconds}s`;
}

function RibbonRow({
  className,
  rowRef,
  stripRef,
  label,
  testId,
  topAdornment,
  bottomAdornment,
  rightAdornment,
  onStripWheel,
  onStripPointerDown,
  onStripPointerMove,
  onStripPointerUp,
  onStripPointerCancel,
  onStripClickCapture,
  children,
}: {
  className?: string;
  rowRef?: Ref<HTMLDivElement>;
  stripRef?: Ref<HTMLDivElement>;
  label: ReactNode;
  testId?: string;
  topAdornment?: ReactNode;
  bottomAdornment?: ReactNode;
  rightAdornment?: ReactNode;
  onStripWheel?: (event: ReactWheelEvent<HTMLDivElement>) => void;
  onStripPointerDown?: (event: ReactPointerEvent<HTMLDivElement>) => void;
  onStripPointerMove?: (event: ReactPointerEvent<HTMLDivElement>) => void;
  onStripPointerUp?: (event: ReactPointerEvent<HTMLDivElement>) => void;
  onStripPointerCancel?: (event: ReactPointerEvent<HTMLDivElement>) => void;
  onStripClickCapture?: (event: ReactMouseEvent<HTMLDivElement>) => void;
  children: ReactNode;
}) {
  return (
    <div ref={rowRef} className={['ribbon-row', className].filter(Boolean).join(' ')} data-testid={testId}>
      <div className="ribbon-label">{label}</div>
      <div className="chip-strip-wrap">
        <div
          ref={stripRef}
          className="chip-strip"
          onWheel={onStripWheel}
          onPointerDown={onStripPointerDown}
          onPointerMove={onStripPointerMove}
          onPointerUp={onStripPointerUp}
          onPointerCancel={onStripPointerCancel}
          onClickCapture={onStripClickCapture}
        >
          {children}
        </div>
      </div>
      {topAdornment}
      {bottomAdornment}
      {rightAdornment}
    </div>
  );
}

function ChatFilterToggleButton({
  active,
  onClick,
}: {
  active: boolean;
  onClick?: () => void;
}) {
  const currentMode = active ? 'Filtered' : 'All chats';
  return (
    <button
      type="button"
      className={`ribbon-filter-clear ${active ? 'active' : ''}`}
      onClick={(event) => {
        event.stopPropagation();
        onClick?.();
      }}
      aria-label={`Toggle chat filter. Current: ${currentMode}.`}
      data-chat-filter-mode={active ? 'filter' : 'all'}
      data-testid="ribbon-filter-clear"
    >
      <span className="ribbon-filter-tooltip" role="tooltip">
        <b>Click to toggle</b>
        <span className={active ? 'current' : ''}>
          <strong>Filtered</strong>
          <em>{active ? 'Selected agents · current' : 'Selected agents'}</em>
        </span>
        <span className={!active ? 'current' : ''}>
          <strong>All chats</strong>
          <em>{!active ? 'Every agent · current' : 'Every agent'}</em>
        </span>
      </span>
    </button>
  );
}

function AgentChip({
  id,
  hero,
  captain,
  target,
  chatFilterActive,
  unread,
  off,
  live,
  working,
  dreaming,
  minimized,
  privateMode,
  fileTreeHover,
  disabled,
  hostRef,
  agentMeta,
  unsupportedProvider,
  onClick,
  onDoubleClick,
  onContextMenu,
  onCommend,
  record,
}: {
  id: AgentId;
  hero?: WorkingHero;
  state?: AgentAtTable['state'];
  captain?: boolean;
  target?: boolean;
  chatFilterActive?: boolean;
  unread?: boolean;
  off?: boolean;
  live?: boolean;
  working?: boolean;
  dreaming?: boolean;
  minimized?: boolean;
  privateMode?: boolean;
  cmdSlot?: number;
  fileTreeHover?: boolean;
  disabled?: boolean;
  hostRef?: (node: HTMLSpanElement | null) => void;
  agentMeta?: Readonly<Record<AgentId, Agent>>;
  unsupportedProvider?: string;
  onClick: () => void;
  onDoubleClick?: () => void;
  onContextMenu?: (id: AgentId, point: { x: number; y: number }) => void;
  onCommend?: (id: AgentId, source: ProjectAgentCommendSource) => void;
  record?: ProjectAgentRecord;
}) {
  const agent = agentMeta?.[id] ?? AGENTS[id] ?? {
    name: id,
    emoji: '◇',
    role: 'Working agent',
    hue: 'var(--brass-bright)',
  };
  const displayName = agent.name;
  const nameParts = splitProjectAgentName(displayName);
  const avatarClass = agent.avatarClass ?? hero?.avatarClass ?? providerAvatarClass(hero?.cli, id);
  const avatarStyle = avatarImageStyleForId(agent.avatarId ?? hero?.avatarId);
  const offline = !live;
  const unsupported = !!unsupportedProvider;
  const interactionDisabled = !!disabled || unsupported;

  return (
    <span ref={hostRef} className="agent-commend-host chip-commend-host">
      <button
        className={[
          'chip',
          target ? 'target' : '',
          chatFilterActive ? 'chat-filter-target' : '',
          off ? 'off' : '',
          offline ? 'offline' : '',
          live ? 'live' : '',
          working ? 'working' : '',
          dreaming ? 'dreaming' : '',
          minimized ? 'minimized' : '',
          unread ? 'unread' : '',
          privateMode ? 'private' : '',
          fileTreeHover ? 'file-tree-hover' : '',
          disabled ? 'hydrating' : '',
          unsupported ? 'unsupported' : '',
        ].filter(Boolean).join(' ')}
        disabled={interactionDisabled}
        onClick={onClick}
        onDoubleClick={unsupported ? undefined : onDoubleClick}
        onContextMenu={(event) => {
          if (unsupported) {
            event.preventDefault();
            event.stopPropagation();
            return;
          }
          if (!onContextMenu) return;
          event.preventDefault();
          event.stopPropagation();
          onContextMenu(id, { x: event.clientX, y: event.clientY });
        }}
        aria-pressed={!!target}
        aria-label={unsupported
          ? `${displayName} (Unsupported provider: ${unsupportedProvider})`
          : `${displayName}${target ? ' (target)' : ''}${chatFilterActive ? ' (chat filter)' : ''}${unread ? ' (unread)' : ''}${privateMode ? ' (private)' : ''}${dreaming ? ' (dreaming)' : ''}${live ? (minimized ? ' (minimized)' : ' (live)') : ' (offline)'}`}
        title={unsupported ? `Unsupported provider: ${unsupportedProvider}` : undefined}
        data-testid={`chip-${id}`}
        data-live={live ? 'true' : 'false'}
        data-working={working ? 'true' : 'false'}
        data-dreaming={dreaming ? 'true' : 'false'}
        data-minimized={minimized ? 'true' : 'false'}
        data-hydrating={disabled ? 'true' : 'false'}
        data-unsupported-provider={unsupportedProvider}
      >
        <span className={`chip-avatar tavern-avatar-art ${avatarClass}`} style={avatarStyle} aria-hidden>
          <span />
          <i />
          <b />
        </span>
        <span className="chip-name">
          {nameParts.title && (
            <span className={`project-agent-name-title chip-title title-${nameParts.title.id}`}>
              {nameParts.title.abbr}
            </span>
          )}
          <span className="chip-name-base">{nameParts.base}</span>
          {captain && <span className="chip-star" aria-label="captain">★</span>}
          {privateMode && <LockMini />}
        </span>
        {unsupported && <span className="chip-unsupported">Unsupported</span>}
        {unread && <span className="chip-unread-dot" aria-hidden />}
      </button>
      {!unsupported && (
        <AgentCommendButton
          agentId={id}
          agentName={displayName}
          source="agent-bar"
          count={record?.commends}
          onCommend={onCommend}
        />
      )}
    </span>
  );
}

function EmptySlotChip({
  slot,
  hostRef,
  disabled,
  onClick,
}: {
  slot: number;
  hostRef?: (node: HTMLSpanElement | null) => void;
  disabled?: boolean;
  onClick?: () => void;
}) {
  return (
    <span ref={hostRef} className="chip-empty-host">
      <button
        type="button"
        className={`chip chip-empty-slot ${disabled ? 'hydrating' : ''}`}
        disabled={disabled}
        onClick={onClick}
        title={`Add agent to seat ${slot} · ⌘${slot}`}
        aria-label={`Add agent to seat ${slot}`}
        data-testid={`chip-empty-${slot}`}
      >
        <span className="chip-empty-plus">＋</span>
        <span className="chip-empty-name">Seat {slot}</span>
      </button>
    </span>
  );
}

function LockMini() {
  return (
    <svg className="chip-private-lock" viewBox="0 0 12 12" fill="none" aria-hidden>
      <path d="M3.8 5.2V4.1c0-1.4.9-2.3 2.2-2.3s2.2.9 2.2 2.3v1.1" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
      <rect x="2.8" y="5" width="6.4" height="5" rx="1.1" stroke="currentColor" strokeWidth="1" />
    </svg>
  );
}

function AddChip({
  heroes,
  unavailableHeroIds,
  onIncarnate,
  onAddAgent,
  tableFull,
  disabled,
}: {
  heroes: readonly WorkingHero[];
  unavailableHeroIds?: ReadonlySet<AgentId>;
  onIncarnate?: (hero: WorkingHero) => void;
  onAddAgent?: () => void;
  tableFull?: boolean;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!wrapRef.current?.contains(event.target as Node)) setOpen(false);
    };
    window.addEventListener('pointerdown', onPointerDown);
    return () => window.removeEventListener('pointerdown', onPointerDown);
  }, [open]);

  if (tableFull) {
    return (
      <div className="chip-add-wrap table-full" data-testid="ribbon-table-full">
        <span className="chip-add-full">table full</span>
      </div>
    );
  }

  return (
    <div
      ref={wrapRef}
      className={`chip-add-wrap ${open ? 'open' : ''} ${disabled ? 'hydrating' : ''}`}
      onMouseEnter={() => { if (!disabled && !onAddAgent) setOpen(true); }}
      onFocus={() => { if (!disabled && !onAddAgent) setOpen(true); }}
      onKeyDown={(event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          setOpen(false);
        }
      }}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setOpen(false);
      }}
    >
      <button
        type="button"
        className="chip-add"
        disabled={disabled}
        onClick={(event) => {
          event.stopPropagation();
          if (disabled) return;
          if (onAddAgent) {
            setOpen(false);
            onAddAgent();
            return;
          }
          setOpen(true);
        }}
        aria-label="Show active working heroes"
        aria-expanded={open}
        data-testid="ribbon-add"
      >
        <span className="chip-add-plus">＋</span> add
      </button>
      {open && onIncarnate && (
        <WorkingHeroPicker
          heroes={heroes}
          unavailableHeroIds={unavailableHeroIds}
          onSelect={(hero) => {
            setOpen(false);
            onIncarnate(hero);
          }}
          testIdPrefix="incarnate-ribbon"
        />
      )}
    </div>
  );
}

function providerAvatarClass(cli: WorkingHero['cli'] | undefined, id: AgentId): string {
  return avatarClassForAgentFallback(cli, id);
}
