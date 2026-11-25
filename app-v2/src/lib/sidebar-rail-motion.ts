import type { SidebarRailSide } from './sidebar-rail';

export const SIDEBAR_RAIL_DETENT_MS = 210;

const RAIL_WIDTHS: Record<SidebarRailSide, number> = {
  left: 240,
  right: 300,
};

const COLLAPSE_EASING = 'cubic-bezier(0.28, 0.08, 0.22, 1)';
const EXPAND_EASING = 'cubic-bezier(0.18, 0.86, 0.24, 1)';

export interface SidebarRailDomElements {
  slotEl: HTMLDivElement | null;
  plateEl: HTMLButtonElement | null;
}

interface ActiveRailAnimation {
  abort: () => void;
}

const activeAnimations: Record<SidebarRailSide, ActiveRailAnimation | null> = {
  left: null,
  right: null,
};

export function syncCollapsedRailElement(element: HTMLElement | null, collapsed: boolean): void {
  if (!element) return;
  if (collapsed) {
    const active = document.activeElement;
    if (active instanceof HTMLElement && element.contains(active)) {
      active.blur();
    }
  }
  element.toggleAttribute('inert', collapsed);
  if (collapsed) {
    element.setAttribute('aria-hidden', 'true');
  } else {
    element.removeAttribute('aria-hidden');
  }
  element.style.visibility = collapsed ? 'hidden' : '';
}

function readCurrentVisibleWidth(
  side: SidebarRailSide,
  asideEl: HTMLElement,
  slotCollapsed: boolean,
  hasActiveRun: boolean,
): number {
  const fullW = RAIL_WIDTHS[side];
  if (!hasActiveRun) return slotCollapsed ? 0 : fullW;
  const transform = getComputedStyle(asideEl).transform;
  const tx = transform && transform !== 'none' ? new DOMMatrixReadOnly(transform).m41 : 0;
  const visible = side === 'left' ? fullW + tx : fullW - tx;
  return Math.max(0, Math.min(fullW, visible));
}

function buildWidthStops(
  fromW: number,
  toW: number,
  nextCollapsed: boolean,
): Array<{ w: number; offset?: number }> {
  if (Math.abs(fromW - toW) > 40) {
    if (nextCollapsed) {
      const unlatchW = Math.max(0, fromW - 7);
      return [
        { w: fromW },
        { w: unlatchW, offset: 0.14 },
        { w: unlatchW, offset: 0.21 },
        { w: 0 },
      ];
    }
    const preLatchW = Math.max(fromW, toW - 5);
    return [
      { w: fromW },
      { w: preLatchW, offset: 0.76 },
      { w: preLatchW, offset: 0.85 },
      { w: toW },
    ];
  }
  return [{ w: fromW }, { w: toW }];
}

export function abortSidebarRailMotion(side: SidebarRailSide): void {
  const active = activeAnimations[side];
  if (active) {
    activeAnimations[side] = null;
    active.abort();
  }
}

export function abortAllSidebarRailMotion(): void {
  abortSidebarRailMotion('left');
  abortSidebarRailMotion('right');
}

/** Option 甲: 210ms compositor translateX slide first, 1-step slot/room width commit on settle.
 *  Returns false when reduced-motion is enabled (or in happy-dom unit tests without WAAPI), so
 *  the caller falls straight through to Option 乙 (instant commit). */
export function runSidebarRailMotion({
  side,
  nextCollapsed,
  slotCollapsed,
  dom,
  onStartTargetCommit,
  onSettleSlotCommit,
}: {
  side: SidebarRailSide;
  nextCollapsed: boolean;
  slotCollapsed: boolean;
  dom: SidebarRailDomElements;
  onStartTargetCommit: () => void;
  onSettleSlotCommit: (settledCollapsed: boolean) => void;
}): boolean {
  const { slotEl, plateEl } = dom;
  const asideEl = (slotEl?.firstElementChild as HTMLElement | null) ?? null;

  // happy-dom unit tests do not implement Element.prototype.animate; fall through to instant commit.
  if (
    window.matchMedia('(prefers-reduced-motion: reduce)').matches ||
    !slotEl ||
    !asideEl ||
    !plateEl ||
    typeof asideEl.animate !== 'function' ||
    typeof plateEl.animate !== 'function'
  ) {
    abortSidebarRailMotion(side);
    return false;
  }

  const fullW = RAIL_WIDTHS[side];
  const outSign = side === 'left' ? -1 : 1;
  const hadActiveRun = activeAnimations[side] !== null;
  const fromW = readCurrentVisibleWidth(side, asideEl, slotCollapsed, hadActiveRun);
  const toW = nextCollapsed ? 0 : fullW;

  abortSidebarRailMotion(side);

  if (Math.abs(fromW - toW) < 0.5) {
    slotEl.setAttribute('data-collapsed', nextCollapsed ? 'true' : 'false');
    slotEl.removeAttribute('data-animating');
    syncCollapsedRailElement(asideEl, nextCollapsed);
    onStartTargetCommit();
    onSettleSlotCommit(nextCollapsed);
    return true;
  }

  if (nextCollapsed) {
    const active = document.activeElement;
    if (active instanceof HTMLElement && asideEl.contains(active)) {
      active.blur();
    }
  }
  asideEl.toggleAttribute('inert', nextCollapsed);
  if (nextCollapsed) {
    asideEl.setAttribute('aria-hidden', 'true');
  } else {
    asideEl.removeAttribute('aria-hidden');
  }
  asideEl.style.visibility = 'visible';
  slotEl.setAttribute('data-animating', 'true');

  const stops = buildWidthStops(fromW, toW, nextCollapsed);
  const asideKeyframes: Keyframe[] = stops.map(({ w, offset }) => ({
    transform: `translateX(${(outSign * (fullW - w)).toFixed(2)}px)`,
    ...(offset !== undefined ? { offset } : {}),
  }));
  const plateKeyframes: Keyframe[] = stops.map(({ w, offset }) => {
    const tx = slotCollapsed ? -outSign * w : outSign * (fullW - w);
    return {
      transform: `translateX(${tx.toFixed(2)}px)`,
      ...(offset !== undefined ? { offset } : {}),
    };
  });

  const easing = nextCollapsed ? COLLAPSE_EASING : EXPAND_EASING;
  const asideAnim = asideEl.animate(asideKeyframes, {
    duration: SIDEBAR_RAIL_DETENT_MS,
    easing,
    fill: 'forwards',
  });
  const plateAnim = plateEl.animate(plateKeyframes, {
    duration: SIDEBAR_RAIL_DETENT_MS,
    easing,
    fill: 'forwards',
  });

  let aborted = false;
  activeAnimations[side] = {
    abort() {
      aborted = true;
      asideAnim.cancel();
      plateAnim.cancel();
      slotEl.removeAttribute('data-animating');
      syncCollapsedRailElement(asideEl, slotEl.getAttribute('data-collapsed') === 'true');
    },
  };

  onStartTargetCommit();

  void Promise.all([asideAnim.finished, plateAnim.finished]).then(
    () => {
      if (aborted) return;
      activeAnimations[side] = null;
      slotEl.setAttribute('data-collapsed', nextCollapsed ? 'true' : 'false');
      slotEl.removeAttribute('data-animating');
      asideAnim.cancel();
      plateAnim.cancel();
      syncCollapsedRailElement(asideEl, nextCollapsed);
      onSettleSlotCommit(nextCollapsed);
    },
    () => {},
  );

  return true;
}
