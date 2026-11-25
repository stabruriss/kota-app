export const RESET_RELAY_HANDOFF_KEY = 'kota-v2.reset-handoff';
export const RESET_RELAY_ELEMENT_ID = 'kota-reset-relay';
export const RESET_RELAY_TITLE = 'RESET KOTA';
export const RESET_RELAY_HARD_TIMEOUT_MS = 12_000;
export const RESET_RELAY_COMPLETE_HOLD_MS = 180;

export const RESET_RELAY_STAGE_LABELS = [
  'CLOSING PAGE',
  'LOADING UI',
  'OPENING ROOM',
  'ROOM READY',
] as const;

export type ResetRelayStage = 1 | 2 | 3 | 4;

export interface ResetRelayRuntimeState {
  active: boolean;
  stage: ResetRelayStage;
  hardTimerId: number;
  holdTimerId: number;
  onChange: ((active: boolean) => void) | null;
}

declare global {
  interface Window {
    __KOTA_RESET_RELAY__?: ResetRelayRuntimeState;
  }
}

export interface ResetRelayStageDescriptor {
  stage: ResetRelayStage;
  title: string;
  label: string;
  fraction: string;
  litSegments: readonly [boolean, boolean, boolean, boolean];
  headIndex: number;
}

export type ResetRelayConvergenceAction =
  | { kind: 'hold_stage_2' }
  | { kind: 'advance_stage_3' }
  | { kind: 'complete_stage_4' }
  | { kind: 'dismiss_immediately' };

export function describeResetRelayStage(stage: ResetRelayStage): ResetRelayStageDescriptor {
  return {
    stage,
    title: RESET_RELAY_TITLE,
    label: RESET_RELAY_STAGE_LABELS[stage - 1],
    fraction: `${stage}/4`,
    litSegments: [stage >= 1, stage >= 2, stage >= 3, stage >= 4],
    headIndex: stage - 1,
  };
}

export function resolveResetRelayAction(params: {
  hasTauri: boolean;
  workspaceInitialLoadComplete: boolean;
  hasActiveWorkspace: boolean;
  agentsHydrating: boolean;
  hasHydrationFailure: boolean;
}): ResetRelayConvergenceAction {
  if (!params.hasTauri) {
    return { kind: 'dismiss_immediately' };
  }
  if (!params.workspaceInitialLoadComplete) {
    return { kind: 'hold_stage_2' };
  }
  if (!params.hasActiveWorkspace) {
    return { kind: 'dismiss_immediately' };
  }
  if (params.agentsHydrating) {
    return { kind: 'advance_stage_3' };
  }
  if (params.hasHydrationFailure) {
    return { kind: 'dismiss_immediately' };
  }
  return { kind: 'complete_stage_4' };
}

export function renderResetRelayStage(el: HTMLElement, stage: ResetRelayStage): void {
  const desc = describeResetRelayStage(stage);
  el.setAttribute('data-stage', String(desc.stage));

  const labelEl = el.querySelector<HTMLElement>('[data-relay-label]');
  if (labelEl) {
    labelEl.textContent = desc.label;
  }

  const countEl = el.querySelector<HTMLElement>('[data-relay-count]');
  if (countEl) {
    countEl.textContent = desc.fraction;
  }

  const segs = el.querySelectorAll<HTMLElement>('.relay-seg');
  segs.forEach((seg, index) => {
    seg.setAttribute('data-lit', desc.litSegments[index] ? 'true' : 'false');
    seg.setAttribute('data-head', index === desc.headIndex ? 'true' : 'false');
  });

  const ticks = el.querySelectorAll<HTMLElement>('.relay-ticks i');
  ticks.forEach((tick, index) => {
    tick.setAttribute('data-lit', desc.litSegments[index] ? 'true' : 'false');
  });
}

function clearRelayRuntimeTimers(win: Window, state: ResetRelayRuntimeState | undefined): void {
  if (!state) return;
  if (state.hardTimerId) {
    win.clearTimeout(state.hardTimerId);
    state.hardTimerId = 0;
  }
  if (state.holdTimerId) {
    win.clearTimeout(state.holdTimerId);
    state.holdTimerId = 0;
  }
}

export function beginResetRelayBeforeReload(options?: {
  win?: Window;
  doc?: Document;
  storage?: Pick<Storage, 'setItem'> | null;
  reload?: () => void;
}): void {
  const win = options?.win ?? (typeof window !== 'undefined' ? window : undefined);
  if (!win) return;
  const doc = options?.doc ?? win.document;
  const storage = options?.storage === undefined ? win.sessionStorage : options.storage;
  const reload = options?.reload ?? (() => win.location.reload());

  if (storage) {
    try {
      storage.setItem(RESET_RELAY_HANDOFF_KEY, '1');
    } catch {
      // Ignore storage quota or sandbox errors; still proceed with reload.
    }
  }

  const el = doc?.getElementById(RESET_RELAY_ELEMENT_ID);
  if (el instanceof HTMLElement) {
    renderResetRelayStage(el, 1);
    el.hidden = false;
  }

  win.requestAnimationFrame(() => {
    win.requestAnimationFrame(() => {
      reload();
    });
  });
}

export function isResetRelayActive(win: Window | undefined = typeof window !== 'undefined' ? window : undefined): boolean {
  return Boolean(win?.__KOTA_RESET_RELAY__?.active);
}

export function subscribeResetRelayActive(
  listener: (active: boolean) => void,
  win: Window | undefined = typeof window !== 'undefined' ? window : undefined,
): () => void {
  const state = win?.__KOTA_RESET_RELAY__;
  if (!state) return () => {};
  state.onChange = listener;
  return () => {
    if (state.onChange === listener) {
      state.onChange = null;
    }
  };
}

export function setResetRelayStage(
  stage: ResetRelayStage,
  win: Window | undefined = typeof window !== 'undefined' ? window : undefined,
  doc: Document | undefined = win?.document,
): void {
  const state = win?.__KOTA_RESET_RELAY__;
  if (!state?.active || state.holdTimerId) return;
  if (state.stage !== stage) {
    state.stage = stage;
  }
  const el = doc?.getElementById(RESET_RELAY_ELEMENT_ID);
  if (el instanceof HTMLElement && el.getAttribute('data-stage') !== String(stage)) {
    renderResetRelayStage(el, stage);
  }
}

export function dismissResetRelayImmediately(
  win: Window | undefined = typeof window !== 'undefined' ? window : undefined,
  doc: Document | undefined = win?.document,
): void {
  if (!win) return;
  const state = win.__KOTA_RESET_RELAY__;
  clearRelayRuntimeTimers(win, state);
  const el = doc?.getElementById(RESET_RELAY_ELEMENT_ID);
  if (el instanceof HTMLElement) {
    el.hidden = true;
  }
  if (state?.active) {
    state.active = false;
    state.onChange?.(false);
  }
}

export function completeResetRelayAndDismiss(
  win: Window | undefined = typeof window !== 'undefined' ? window : undefined,
  doc: Document | undefined = win?.document,
  holdMs: number = RESET_RELAY_COMPLETE_HOLD_MS,
): void {
  if (!win) return;
  const state = win.__KOTA_RESET_RELAY__;
  if (!state?.active || state.holdTimerId) return;
  state.stage = 4;
  const el = doc?.getElementById(RESET_RELAY_ELEMENT_ID);
  if (el instanceof HTMLElement) {
    renderResetRelayStage(el, 4);
  }
  if (holdMs <= 0) {
    dismissResetRelayImmediately(win, doc);
    return;
  }
  state.holdTimerId = win.setTimeout(() => {
    dismissResetRelayImmediately(win, doc);
  }, holdMs);
}
