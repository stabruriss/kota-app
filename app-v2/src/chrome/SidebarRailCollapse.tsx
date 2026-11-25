import { useLayoutEffect, useRef, type ReactNode } from 'react';
import {
  registerSidebarRailPlateDom,
  registerSidebarRailSlotDom,
  toggleSidebarRail,
  useSidebarRailCollapsed,
  useSidebarRailPlate,
  type SidebarRailSide,
} from '../lib/sidebar-rail';
import { syncCollapsedRailElement } from '../lib/sidebar-rail-motion';
import '../styles/sidebar-rail-collapse.css';

export function SidebarRailSlot({
  side,
  onCollapse,
  children,
}: {
  side: SidebarRailSide;
  onCollapse?: () => void;
  children: ReactNode;
}) {
  const collapsed = useSidebarRailCollapsed(side);
  const slotRef = useRef<HTMLDivElement | null>(null);
  const prevCollapsedRef = useRef(collapsed);
  const onCollapseRef = useRef(onCollapse);
  onCollapseRef.current = onCollapse;

  useLayoutEffect(() => {
    registerSidebarRailSlotDom(side, slotRef.current, () => onCollapseRef.current?.());
    return () => {
      registerSidebarRailSlotDom(side, null);
    };
  }, [side]);

  useLayoutEffect(() => {
    const slotEl = slotRef.current;
    const aside = (slotEl?.firstElementChild as HTMLElement | null) ?? null;
    if (!slotEl?.hasAttribute('data-animating')) {
      syncCollapsedRailElement(aside, collapsed);
    }
    const wasCollapsed = prevCollapsedRef.current;
    prevCollapsedRef.current = collapsed;
    if (wasCollapsed !== collapsed) {
      window.dispatchEvent(new Event('resize'));
    }
  }, [collapsed]);

  return (
    <div
      ref={slotRef}
      className={`sidebar-rail-slot sidebar-rail-slot--${side}`}
      data-collapsed={collapsed ? 'true' : 'false'}
      data-testid={`sidebar-rail-slot-${side}`}
    >
      {children}
    </div>
  );
}

const PLATE_POINTS_LEFT = '0,0.6 11.2,0.6 17.2,6.6 17.2,165.4 11.2,171.4 0,171.4';
const PLATE_STROKE_LEFT = '0.35,0.75 11.1,0.75 17.1,6.75 17.1,165.25 11.1,171.25 0.35,171.25';
const PLATE_BEVEL_LEFT = '1.9,2.6 10.3,2.6 15.2,7.5 15.2,164.5 10.3,169.4 1.9,169.4';
const PLATE_POINTS_RIGHT = '6.8,0.6 18,0.6 18,171.4 6.8,171.4 0.8,165.4 0.8,6.6';
const INTAGLIO_MARK_POINTS = '-2.6,-5.2 -0.5,-5.2 2.9,0 -0.5,5.2 -2.6,5.2 0.5,0';

const handleToggleLeft = () => toggleSidebarRail('left');
const handleToggleRight = () => toggleSidebarRail('right');

export function SidebarRailPlate({ side }: { side: SidebarRailSide }) {
  const { collapsed, glow, style, label } = useSidebarRailPlate(side);
  const isLeft = side === 'left';
  const suffix = isLeft ? 'l' : 'r';
  const btnRef = useRef<HTMLButtonElement | null>(null);

  useLayoutEffect(() => {
    registerSidebarRailPlateDom(side, btnRef.current);
    return () => {
      registerSidebarRailPlateDom(side, null);
    };
  }, [side]);

  return (
    <button
      ref={btnRef}
      type="button"
      className={`sidebar-rail-plate sidebar-rail-plate--${side}`}
      data-testid={`sidebar-rail-toggle-${side}`}
      data-side={side}
      data-collapsed={collapsed ? 'true' : 'false'}
      data-glow={glow}
      style={style}
      aria-expanded={!collapsed}
      aria-controls={isLeft ? 'file-tree' : 'right-column'}
      aria-label={label}
      title={label}
      onMouseDown={(event) => event.preventDefault()}
      onClick={isLeft ? handleToggleLeft : handleToggleRight}
    >
      <span className="sidebar-rail-underglow" aria-hidden="true" />
      <svg className="sidebar-rail-underglow-poly" viewBox="0 0 18 172" aria-hidden="true">
        <polygon points={isLeft ? PLATE_POINTS_LEFT : PLATE_POINTS_RIGHT} />
      </svg>
      <svg className="sidebar-rail-svg" viewBox="0 0 18 172" aria-hidden="true">
        <defs>
          <linearGradient id={`kota-rail-plate-body-${suffix}`} x1={isLeft ? '0' : '1'} y1="0" x2={isLeft ? '1' : '0'} y2="1">
            <stop offset="0%" stopColor="#15181d" />
            <stop offset="52%" stopColor="#0b0d10" />
            <stop offset="100%" stopColor="#060709" />
          </linearGradient>
          <linearGradient id={`kota-rail-plate-rim-${suffix}`} x1={isLeft ? '1' : '0'} y1="0" x2={isLeft ? '0' : '1'} y2="1">
            <stop offset="0%" stopColor="#cfc6b0" stopOpacity="0.22" />
            <stop offset="55%" stopColor="#6e6a60" stopOpacity="0.11" />
            <stop offset="100%" stopColor="#45433e" stopOpacity="0.08" />
          </linearGradient>
          <pattern
            id={`kota-rail-plate-weave-${suffix}`}
            width="5"
            height="5"
            patternUnits="userSpaceOnUse"
            patternTransform={isLeft ? 'rotate(-45)' : 'rotate(45)'}
          >
            <line x1="0" y1="0" x2="0" y2="5" stroke="#ffffff" strokeOpacity="0.02" strokeWidth="1.1" />
          </pattern>
        </defs>

        <g transform={isLeft ? undefined : 'translate(18, 0) scale(-1, 1)'}>
          <polygon className="sidebar-rail-plate-base" points={PLATE_POINTS_LEFT} fill={`url(#kota-rail-plate-body-${suffix})`} />
          <polygon points={PLATE_POINTS_LEFT} fill={`url(#kota-rail-plate-weave-${suffix})`} />
          <polygon className="sidebar-rail-plate-outer-stroke" points={PLATE_STROKE_LEFT} />
          <polygon className="sidebar-rail-plate-bevel" points={PLATE_BEVEL_LEFT} stroke={`url(#kota-rail-plate-rim-${suffix})`} />
          <line className="sidebar-rail-chamfer-glint" x1="11.4" y1="1.1" x2="16.8" y2="6.5" />
          <g className="sidebar-rail-rivets">
            <circle cx="8.2" cy="8.5" r="0.85" />
            <circle cx="8.2" cy="163.5" r="0.85" />
          </g>
        </g>

        <g className="sidebar-rail-glyph-center" transform={isLeft ? 'translate(8.4, 86)' : 'translate(9.6, 86)'}>
          <g className={`sidebar-rail-glyph sidebar-rail-glyph--${side}`}>
            <polygon className="sidebar-rail-hollow-mark" points={INTAGLIO_MARK_POINTS} />
          </g>
        </g>
      </svg>
    </button>
  );
}

export function StageComposerRailHost({ children }: { children: ReactNode }) {
  return (
    <div className="sidebar-rail-host" data-testid="sidebar-rail-host">
      <SidebarRailPlate side="left" />
      {children}
      <SidebarRailPlate side="right" />
    </div>
  );
}
