import { useEffect, useLayoutEffect, useSyncExternalStore, type CSSProperties } from 'react';
import {
  abortAllSidebarRailMotion,
  runSidebarRailMotion,
  syncCollapsedRailElement,
  type SidebarRailDomElements,
} from './sidebar-rail-motion';
import type { SystemLoadLamp } from './system-load';

export type SidebarRailSide = 'left' | 'right';
export type SidebarRailGlowKind = 'none' | 'gold' | 'yellow' | 'red';

export const LEFT_RAIL_COLLAPSED_STORAGE_KEY = 'kota-v2.sidebar.left-collapsed';
export const RIGHT_RAIL_COLLAPSED_STORAGE_KEY = 'kota-v2.sidebar.right-collapsed';

function railStorageKey(side: SidebarRailSide): string {
  return side === 'left' ? LEFT_RAIL_COLLAPSED_STORAGE_KEY : RIGHT_RAIL_COLLAPSED_STORAGE_KEY;
}

export function loadSidebarRailCollapsed(side: SidebarRailSide): boolean {
  try {
    return window.localStorage.getItem(railStorageKey(side)) === 'true';
  } catch {
    return false;
  }
}

export function saveSidebarRailCollapsed(side: SidebarRailSide, collapsed: boolean): void {
  try {
    window.localStorage.setItem(railStorageKey(side), String(collapsed));
  } catch {
    // Ignore storage failures; the toggle still works for the current render.
  }
}

export function leftRailPlateLabel(collapsed: boolean, latestVersion: string | null): string {
  if (!collapsed) return 'Collapse file tree';
  return latestVersion
    ? `Expand file tree · Kota ${latestVersion} is available`
    : 'Expand file tree';
}

export function rightRailPlateLabel(collapsed: boolean, lamp: SystemLoadLamp | null): string {
  if (!collapsed) return 'Collapse right sidebar';
  if (!lamp || lamp.light <= 0) return 'Expand right sidebar';
  return lamp.r > 0
    ? 'Expand right sidebar · System Load red reset'
    : 'Expand right sidebar · System Load yellow reset';
}

function resolveRightPlateGlow(lamp: SystemLoadLamp | null): {
  kind: SidebarRailGlowKind;
  style?: CSSProperties;
} {
  if (!lamp || lamp.light <= 0) return { kind: 'none' };
  const kind: SidebarRailGlowKind = lamp.r > 0 ? 'red' : 'yellow';
  return {
    kind,
    style: {
      '--sidebar-rail-glow-rgb': lamp.heatRgb.join(', '),
      '--sidebar-rail-glow-alpha': String(lamp.light),
    } as CSSProperties,
  };
}

export interface SidebarRailPlateView {
  collapsed: boolean;
  glow: SidebarRailGlowKind;
  style?: CSSProperties;
  label: string;
}

interface SidebarRailState {
  leftTargetCollapsed: boolean;
  leftSlotCollapsed: boolean;
  rightTargetCollapsed: boolean;
  rightSlotCollapsed: boolean;
  hasRoomComposer: boolean;
  leftUpdateVersion: string | null;
  rightSystemLoadLamp: SystemLoadLamp | null;
}

function sameLamp(a: SystemLoadLamp | null, b: SystemLoadLamp | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.light === b.light &&
    a.y === b.y &&
    a.r === b.r &&
    a.heatRgb[0] === b.heatRgb[0] &&
    a.heatRgb[1] === b.heatRgb[1] &&
    a.heatRgb[2] === b.heatRgb[2]
  );
}

const initialLeft = loadSidebarRailCollapsed('left');
const initialRight = loadSidebarRailCollapsed('right');

let state: SidebarRailState = {
  leftTargetCollapsed: initialLeft,
  leftSlotCollapsed: initialLeft,
  rightTargetCollapsed: initialRight,
  rightSlotCollapsed: initialRight,
  hasRoomComposer: true,
  leftUpdateVersion: null,
  rightSystemLoadLamp: null,
};

const domRegistry: Record<SidebarRailSide, SidebarRailDomElements> = {
  left: { slotEl: null, plateEl: null },
  right: { slotEl: null, plateEl: null },
};

const slotCollapseCallbacks: Record<SidebarRailSide, (() => void) | null> = {
  left: null,
  right: null,
};

let onCollapseRightAppCallback: (() => void) | null = null;
const listeners = new Set<() => void>();

function buildLeftPlateView(s: SidebarRailState): SidebarRailPlateView {
  const collapsed = s.hasRoomComposer && s.leftTargetCollapsed;
  return {
    collapsed,
    glow: s.leftUpdateVersion ? 'gold' : 'none',
    label: leftRailPlateLabel(collapsed, s.leftUpdateVersion),
  };
}

function buildRightPlateView(s: SidebarRailState): SidebarRailPlateView {
  const collapsed = s.hasRoomComposer && s.rightTargetCollapsed;
  const glow = resolveRightPlateGlow(s.rightSystemLoadLamp);
  return {
    collapsed,
    glow: glow.kind,
    style: glow.style,
    label: rightRailPlateLabel(collapsed, s.rightSystemLoadLamp),
  };
}

let cachedLeftKey = {
  collapsed: state.hasRoomComposer && state.leftTargetCollapsed,
  version: state.leftUpdateVersion,
};
let cachedLeftPlate: SidebarRailPlateView = buildLeftPlateView(state);

let cachedRightKey = {
  collapsed: state.hasRoomComposer && state.rightTargetCollapsed,
  lamp: state.rightSystemLoadLamp,
};
let cachedRightPlate: SidebarRailPlateView = buildRightPlateView(state);

function syncCachedSnapshots(): void {
  const nextLeftCollapsed = state.hasRoomComposer && state.leftTargetCollapsed;
  if (
    cachedLeftKey.collapsed !== nextLeftCollapsed ||
    cachedLeftKey.version !== state.leftUpdateVersion
  ) {
    cachedLeftKey = { collapsed: nextLeftCollapsed, version: state.leftUpdateVersion };
    cachedLeftPlate = buildLeftPlateView(state);
  }
  const nextRightCollapsed = state.hasRoomComposer && state.rightTargetCollapsed;
  if (
    cachedRightKey.collapsed !== nextRightCollapsed ||
    !sameLamp(cachedRightKey.lamp, state.rightSystemLoadLamp)
  ) {
    cachedRightKey = { collapsed: nextRightCollapsed, lamp: state.rightSystemLoadLamp };
    cachedRightPlate = buildRightPlateView(state);
  }
}

function emit(): void {
  syncCachedSnapshots();
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getEffectiveLeftSlotCollapsed(): boolean {
  return state.hasRoomComposer && state.leftSlotCollapsed;
}

function getEffectiveRightSlotCollapsed(): boolean {
  return state.hasRoomComposer && state.rightSlotCollapsed;
}

function getLeftPlateSnapshot(): SidebarRailPlateView {
  return cachedLeftPlate;
}

function getRightPlateSnapshot(): SidebarRailPlateView {
  return cachedRightPlate;
}

export function registerSidebarRailSlotDom(
  side: SidebarRailSide,
  slotEl: HTMLDivElement | null,
  onCollapse?: () => void,
): void {
  domRegistry[side].slotEl = slotEl;
  slotCollapseCallbacks[side] = slotEl ? (onCollapse ?? null) : null;
}

export function registerSidebarRailPlateDom(
  side: SidebarRailSide,
  plateEl: HTMLButtonElement | null,
): void {
  domRegistry[side].plateEl = plateEl;
}

function runCollapseSideEffects(side: SidebarRailSide): void {
  slotCollapseCallbacks[side]?.();
  if (side === 'right') {
    onCollapseRightAppCallback?.();
  }
}

/** Option 乙 (and reduced-motion path): 0ms instant layout commit. */
export function commitSidebarRailInstant(side: SidebarRailSide, nextCollapsed: boolean): void {
  abortAllSidebarRailMotion();
  const dom = domRegistry[side];
  const asideEl = (dom.slotEl?.firstElementChild as HTMLElement | null) ?? null;
  if (dom.slotEl) {
    dom.slotEl.setAttribute('data-collapsed', nextCollapsed ? 'true' : 'false');
    dom.slotEl.removeAttribute('data-animating');
  }
  syncCollapsedRailElement(asideEl, nextCollapsed);

  if (side === 'left') {
    state = {
      ...state,
      leftTargetCollapsed: nextCollapsed,
      leftSlotCollapsed: nextCollapsed,
    };
  } else {
    state = {
      ...state,
      rightTargetCollapsed: nextCollapsed,
      rightSlotCollapsed: nextCollapsed,
    };
  }
  saveSidebarRailCollapsed(side, nextCollapsed);
  if (nextCollapsed) {
    runCollapseSideEffects(side);
  }
  emit();
}

export function toggleSidebarRail(side: SidebarRailSide): void {
  const currentTarget = side === 'left' ? state.leftTargetCollapsed : state.rightTargetCollapsed;
  const currentSlot = side === 'left' ? state.leftSlotCollapsed : state.rightSlotCollapsed;
  const nextCollapsed = !currentTarget;

  const startedMotion = runSidebarRailMotion({
    side,
    nextCollapsed,
    slotCollapsed: state.hasRoomComposer && currentSlot,
    dom: domRegistry[side],
    onStartTargetCommit: () => {
      if (side === 'left') {
        state = { ...state, leftTargetCollapsed: nextCollapsed };
      } else {
        state = { ...state, rightTargetCollapsed: nextCollapsed };
      }
      saveSidebarRailCollapsed(side, nextCollapsed);
      if (nextCollapsed) {
        runCollapseSideEffects(side);
      }
      emit();
    },
    onSettleSlotCommit: (settledCollapsed) => {
      const prevSlot = side === 'left' ? state.leftSlotCollapsed : state.rightSlotCollapsed;
      if (prevSlot === settledCollapsed) return;
      if (side === 'left') {
        state = { ...state, leftSlotCollapsed: settledCollapsed };
      } else {
        state = { ...state, rightSlotCollapsed: settledCollapsed };
      }
      emit();
    },
  });

  if (!startedMotion) {
    commitSidebarRailInstant(side, nextCollapsed);
  }
}

export function publishLeftRailUpdateVersion(version: string | null): void {
  if (state.leftUpdateVersion === version) return;
  state = { ...state, leftUpdateVersion: version };
  emit();
}

export function publishRightRailSystemLoadLamp(lamp: SystemLoadLamp | null): void {
  if (sameLamp(state.rightSystemLoadLamp, lamp)) return;
  state = { ...state, rightSystemLoadLamp: lamp };
  emit();
}

export function useSidebarRailAppSync(
  hasRoomComposer: boolean,
  onCollapseRight?: () => void,
): void {
  useLayoutEffect(() => {
    if (state.hasRoomComposer === hasRoomComposer) return;
    abortAllSidebarRailMotion();
    state = {
      ...state,
      hasRoomComposer,
      leftSlotCollapsed: state.leftTargetCollapsed,
      rightSlotCollapsed: state.rightTargetCollapsed,
    };
    emit();
  }, [hasRoomComposer]);

  useEffect(() => {
    onCollapseRightAppCallback = onCollapseRight ?? null;
    return () => {
      abortAllSidebarRailMotion();
      if (onCollapseRightAppCallback === onCollapseRight) {
        onCollapseRightAppCallback = null;
      }
    };
  }, [onCollapseRight]);
}

export function useSidebarRailCollapsed(side: SidebarRailSide): boolean {
  return useSyncExternalStore(
    subscribe,
    side === 'left' ? getEffectiveLeftSlotCollapsed : getEffectiveRightSlotCollapsed,
  );
}

export function useSidebarRailPlate(side: SidebarRailSide): SidebarRailPlateView {
  return useSyncExternalStore(
    subscribe,
    side === 'left' ? getLeftPlateSnapshot : getRightPlateSnapshot,
  );
}
