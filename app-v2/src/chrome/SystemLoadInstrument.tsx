import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type RefObject,
} from 'react';
import { onSystemLoadSample, systemLoadSnapshot } from '../pty-client';
import {
  SYSTEM_LOAD_CONSTANTS,
  computeBarGeometry,
  createInitialSystemLoadState,
  forceSystemLoadState,
  formatRamGiBNumber,
  formatSwapMbpsLabel,
  formatTotalRamLabel,
  isSameLampAppearance,
  lampCssVariables,
  sanitizeSnapshotForPageLoad,
  stepSystemLoad,
  type SystemLoadDebugOverride,
  type SystemLoadLamp,
  type SystemLoadSample,
  type SystemLoadState,
} from '../lib/system-load';

export type SystemLoadBarKey = 'compressed' | 'kota' | 'swap';

export type SystemLoadConfirmKind = 'reset' | 'dream';

export interface SystemLoadDebugController {
  set: (override: SystemLoadDebugOverride, options?: { lock?: boolean }) => SystemLoadState;
  step: (sample: SystemLoadSample) => SystemLoadState;
  clear: () => SystemLoadState;
  get: () => SystemLoadState;
}

declare global {
  interface Window {
    __KOTA_SYSTEM_LOAD_DEBUG__?: SystemLoadDebugController;
  }
}

const TIP_LABELS: Readonly<Record<SystemLoadBarKey, string>> = {
  compressed: 'COMPRESSED RAM',
  kota: 'KOTA RAM',
  swap: 'SWAP BURDEN',
};

export function useSystemLoadController(expanded: boolean): {
  lamp: SystemLoadLamp;
  readState: () => SystemLoadState;
} {
  const stateRef = useRef<SystemLoadState | null>(null);
  if (stateRef.current === null) {
    stateRef.current = createInitialSystemLoadState();
  }
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const debugLockedRef = useRef(false);
  const lastAtMsRef = useRef(-1);

  const [lamp, setLamp] = useState<SystemLoadLamp>(() => stateRef.current!.lamp);
  const [, setPanelVersion] = useState(0);

  const applySample = useCallback((sample: SystemLoadSample) => {
    if (debugLockedRef.current) return;
    if (sample.atMs <= lastAtMsRef.current) return;
    lastAtMsRef.current = sample.atMs;
    const prev = stateRef.current!;
    const next = stepSystemLoad(prev, sample);
    stateRef.current = next;
    if (!isSameLampAppearance(prev.lamp, next.lamp)) {
      setLamp(next.lamp);
    }
    if (expandedRef.current) {
      setPanelVersion((version) => version + 1);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    void onSystemLoadSample((sample) => {
      if (cancelled) return;
      applySample(sample);
    })
      .then((stop) => {
        if (cancelled) {
          void stop();
        } else {
          unlisten = stop;
        }
      })
      .catch(() => {});

    void systemLoadSnapshot()
      .then((snapshot) => {
        if (cancelled || !snapshot) return;
        applySample(sanitizeSnapshotForPageLoad(snapshot));
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      if (unlisten) void unlisten();
    };
  }, [applySample]);

  useEffect(() => {
    if (!import.meta.env.VITE_KOTA_LOCAL_TEST_AGENT) return undefined;

    const controller: SystemLoadDebugController = {
      set: (override, options) => {
        debugLockedRef.current = options?.lock ?? true;
        const next = forceSystemLoadState(stateRef.current!, override);
        stateRef.current = next;
        setLamp(next.lamp);
        if (expandedRef.current) {
          setPanelVersion((version) => version + 1);
        }
        return next;
      },
      step: (sample) => {
        const next = stepSystemLoad(stateRef.current!, sample);
        stateRef.current = next;
        setLamp(next.lamp);
        if (expandedRef.current) {
          setPanelVersion((version) => version + 1);
        }
        return next;
      },
      clear: () => {
        debugLockedRef.current = false;
        lastAtMsRef.current = -1;
        const reset = createInitialSystemLoadState();
        stateRef.current = reset;
        setLamp(reset.lamp);
        if (expandedRef.current) {
          setPanelVersion((version) => version + 1);
        }
        void systemLoadSnapshot()
          .then((snapshot) => {
            if (snapshot && !debugLockedRef.current) {
              applySample(sanitizeSnapshotForPageLoad(snapshot));
            }
          })
          .catch(() => {});
        return reset;
      },
      get: () => stateRef.current!,
    };

    window.__KOTA_SYSTEM_LOAD_DEBUG__ = controller;
    return () => {
      if (window.__KOTA_SYSTEM_LOAD_DEBUG__ === controller) {
        delete window.__KOTA_SYSTEM_LOAD_DEBUG__;
      }
    };
  }, [applySample]);

  const readState = useCallback(() => stateRef.current!, []);

  return { lamp, readState };
}

export function SystemLoadPanel({
  state,
  anchorRef,
  expandButtonRef,
  resetDisabled,
  hidden = false,
  onRequestReset,
  onCollapse,
}: {
  state: SystemLoadState;
  anchorRef: RefObject<HTMLElement>;
  expandButtonRef: RefObject<HTMLButtonElement>;
  resetDisabled: boolean;
  hidden?: boolean;
  onRequestReset: (opener: HTMLButtonElement) => void;
  onCollapse: (restoreFocus: boolean) => void;
}) {
  const panelRef = useRef<HTMLElement | null>(null);
  const tipRef = useRef<HTMLDivElement | null>(null);
  const compressedRowRef = useRef<HTMLDivElement | null>(null);
  const kotaRowRef = useRef<HTMLDivElement | null>(null);
  const swapRowRef = useRef<HTMLDivElement | null>(null);

  const [panelPosition, setPanelPosition] = useState<{
    width: number;
    left: number;
    top: number;
    maxHeight: number;
  }>({ width: 292, left: 12, top: 56, maxHeight: 400 });
  const [activeTip, setActiveTip] = useState<SystemLoadBarKey | null>(null);
  const [tipPosition, setTipPosition] = useState<{ left: number; top: number }>({ left: 8, top: 8 });

  const updatePanelPosition = useCallback(() => {
    const vw = window.innerWidth || 1280;
    const vh = window.innerHeight || 800;
    const width = Math.min(292, Math.max(180, vw - 24));
    const anchor = anchorRef.current?.getBoundingClientRect();
    const left = anchor
      ? Math.max(12, Math.min(vw - width - 12, anchor.right - width - 4))
      : Math.max(12, vw - width - 12);
    const top = anchor
      ? Math.max(12, Math.min(anchor.bottom + 8, vh - 110))
      : 56;
    const maxHeight = Math.max(90, vh - top - 12);
    setPanelPosition((prev) => (
      prev.width === width && prev.left === left && prev.top === top && prev.maxHeight === maxHeight
        ? prev
        : { width, left, top, maxHeight }
    ));
  }, [anchorRef]);

  useLayoutEffect(() => {
    updatePanelPosition();
  }, [updatePanelPosition]);

  useEffect(() => {
    const handleViewport = () => {
      setActiveTip(null);
      updatePanelPosition();
    };
    window.addEventListener('resize', handleViewport);
    window.addEventListener('scroll', handleViewport, { passive: true });
    return () => {
      window.removeEventListener('resize', handleViewport);
      window.removeEventListener('scroll', handleViewport);
    };
  }, [updatePanelPosition]);

  useEffect(() => {
    if (hidden) {
      setActiveTip(null);
      return undefined;
    }
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (!target) return;
      if (panelRef.current?.contains(target)) return;
      if (expandButtonRef.current?.contains(target)) return;
      onCollapse(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      onCollapse(true);
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [expandButtonRef, hidden, onCollapse]);

  useLayoutEffect(() => {
    if (!activeTip) return;
    const rowEl =
      activeTip === 'compressed'
        ? compressedRowRef.current
        : activeTip === 'kota'
          ? kotaRowRef.current
          : swapRowRef.current;
    const tipEl = tipRef.current;
    if (!rowEl || !tipEl) return;
    const rowRect = rowEl.getBoundingClientRect();
    const tipRect = tipEl.getBoundingClientRect();
    const vw = window.innerWidth || 1280;
    const left = Math.max(8, Math.min(vw - tipRect.width - 8, rowRect.left + (rowRect.width - tipRect.width) / 2));
    const top = rowRect.top - tipRect.height - 7 >= 8
      ? rowRect.top - tipRect.height - 7
      : rowRect.bottom + 7;
    setTipPosition((prev) => (prev.left === left && prev.top === top ? prev : { left, top }));
  }, [activeTip]);

  const compressedBar = computeBarGeometry(
    state.compressedBytes,
    state.totalRamBytes,
    SYSTEM_LOAD_CONSTANTS.compressedEnterBytes,
    state.flags.compressed,
    state.hasSample,
  );
  const kotaBar = computeBarGeometry(
    state.kotaPageBytes,
    state.totalRamBytes,
    SYSTEM_LOAD_CONSTANTS.kotaEnterBytes,
    state.flags.kota,
    state.hasSample,
  );
  const swapBar = computeBarGeometry(
    state.swapAvgBytesPerSec,
    SYSTEM_LOAD_CONSTANTS.swapMaxBytesPerSec,
    SYSTEM_LOAD_CONSTANTS.swapEnterBytesPerSec,
    state.flags.swap,
    state.hasSample,
  );

  const lampVars = lampCssVariables(state.lamp);
  const panelStyle: CSSProperties = {
    ...(lampVars as unknown as CSSProperties),
    width: `${panelPosition.width}px`,
    left: `${panelPosition.left}px`,
    top: `${panelPosition.top}px`,
    maxHeight: `${panelPosition.maxHeight}px`,
  };

  const unitClassName = [
    'hud-unit',
    state.flags.compressed ? 'compressed-yellow' : '',
    state.flags.kota ? 'kota-yellow' : '',
    state.flags.swap ? 'swap-yellow' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <>
      <section
        id="system-load-panel"
        ref={panelRef}
        className="ram-panel"
        role="dialog"
        aria-label="SYSTEM LOAD"
        hidden={hidden}
        style={panelStyle}
      >
        <div className="hud-screen">
          <header className="hud-caption">
            <span className="hud-title">SYSTEM LOAD</span>
            <svg className="hud-rail" viewBox="0 0 100 8" preserveAspectRatio="none" aria-hidden="true">
              <path className="rail-base" d="M0 6.5H30L35 2.5H65L70 6.5H100" />
              <path className="rail-heat" d="M0 6.5H30L35 2.5H65L70 6.5H100" />
            </svg>
            <span>{formatTotalRamLabel(state.totalRamBytes, state.hasSample)}</span>
          </header>
          <div className={unitClassName}>
            <span className="reset-shell">
              <button
                type="button"
                className="hud-reset"
                aria-label="RESET KOTA"
                disabled={resetDisabled}
                onClick={(event) => onRequestReset(event.currentTarget)}
              >
                <svg className="reset-edge" viewBox="0 0 46 28" aria-hidden="true">
                  <path d="M.5 .5H45.5V27.5H11.5L.5 16.5Z" />
                </svg>
                <span>RESET</span>
                <span>KOTA</span>
              </button>
            </span>
            <div className="hud-tracks">
              <div
                ref={compressedRowRef}
                className="hud-row compressed-row"
                tabIndex={0}
                role="img"
                aria-label="COMPRESSED RAM"
                aria-describedby={activeTip === 'compressed' ? 'system-load-bar-tip' : undefined}
                data-yellow={String(state.flags.compressed)}
                onPointerEnter={() => setActiveTip('compressed')}
                onPointerLeave={() => setActiveTip((current) => (current === 'compressed' ? null : current))}
                onFocus={() => setActiveTip('compressed')}
                onBlur={() => setActiveTip((current) => (current === 'compressed' ? null : current))}
              >
                <div className="hud-track compressed-track">
                  <span className="bar-fill" style={{ width: `${compressedBar.fillPct}%` }} />
                  <span
                    className="bar-excess"
                    style={{ left: `${compressedBar.excessLeftPct}%`, width: `${compressedBar.excessWidthPct}%` }}
                  />
                  <i className="bar-mark" style={{ left: `${compressedBar.markPct}%` }} />
                </div>
              </div>
              <div
                ref={kotaRowRef}
                className="hud-row kota-row"
                tabIndex={0}
                role="img"
                aria-label="KOTA RAM"
                aria-describedby={activeTip === 'kota' ? 'system-load-bar-tip' : undefined}
                data-yellow={String(state.flags.kota)}
                onPointerEnter={() => setActiveTip('kota')}
                onPointerLeave={() => setActiveTip((current) => (current === 'kota' ? null : current))}
                onFocus={() => setActiveTip('kota')}
                onBlur={() => setActiveTip((current) => (current === 'kota' ? null : current))}
              >
                <div className="hud-track kota-track">
                  <span className="bar-fill" style={{ width: `${kotaBar.fillPct}%` }} />
                  <span
                    className="bar-excess"
                    style={{ left: `${kotaBar.excessLeftPct}%`, width: `${kotaBar.excessWidthPct}%` }}
                  />
                  <i className="bar-mark" style={{ left: `${kotaBar.markPct}%` }} />
                </div>
              </div>
              <div
                ref={swapRowRef}
                className="hud-row swap-row"
                tabIndex={0}
                role="img"
                aria-label="SWAP BURDEN"
                aria-describedby={activeTip === 'swap' ? 'system-load-bar-tip' : undefined}
                data-yellow={String(state.flags.swap)}
                onPointerEnter={() => setActiveTip('swap')}
                onPointerLeave={() => setActiveTip((current) => (current === 'swap' ? null : current))}
                onFocus={() => setActiveTip('swap')}
                onBlur={() => setActiveTip((current) => (current === 'swap' ? null : current))}
              >
                <div className="hud-track swap-track">
                  <span className="bar-fill" style={{ width: `${swapBar.fillPct}%` }} />
                  <span
                    className="bar-excess"
                    style={{ left: `${swapBar.excessLeftPct}%`, width: `${swapBar.excessWidthPct}%` }}
                  />
                  <i className="bar-mark" style={{ left: `${swapBar.markPct}%` }} />
                </div>
              </div>
            </div>
            <div className="hud-numbers">
              <span className="compressed-inline">
                {formatRamGiBNumber(state.compressedBytes, state.hasSample)}
              </span>
              <strong className="kota-number">
                {formatRamGiBNumber(state.kotaPageBytes, state.hasSample)}
              </strong>
              <span className="swap-inline">
                {formatSwapMbpsLabel(state.swapAvgBytesPerSec)}
              </span>
            </div>
          </div>
        </div>
      </section>
      {!hidden && activeTip !== null && (
        <div
          id="system-load-bar-tip"
          ref={tipRef}
          className="hud-tooltip"
          role="tooltip"
          style={{ left: `${tipPosition.left}px`, top: `${tipPosition.top}px` }}
        >
          <span>{TIP_LABELS[activeTip]}</span>
        </div>
      )}
    </>
  );
}

export function SystemLoadConfirmDialog({
  kind,
  opener,
  onClose,
}: {
  kind: SystemLoadConfirmKind;
  opener: HTMLElement | null;
  onClose: (confirmed: boolean) => void;
}) {
  const cancelRef = useRef<HTMLButtonElement | null>(null);
  const proceedRef = useRef<HTMLButtonElement | null>(null);
  const title =
    kind === 'dream'
      ? 'Dream will clear composer drafts.'
      : 'Reset will clear composer drafts.';

  useLayoutEffect(() => {
    cancelRef.current?.focus({ preventScroll: true });
  }, []);

  const finish = useCallback(
    (confirmed: boolean) => {
      onClose(confirmed);
      if (!confirmed && opener) {
        queueMicrotask(() => {
          if (opener.isConnected && !(opener as HTMLButtonElement).disabled) {
            opener.focus({ preventScroll: true });
          }
        });
      }
    },
    [onClose, opener],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        finish(false);
        return;
      }
      if (event.key === 'Tab') {
        event.preventDefault();
        if (document.activeElement === cancelRef.current) {
          proceedRef.current?.focus({ preventScroll: true });
        } else {
          cancelRef.current?.focus({ preventScroll: true });
        }
      }
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, [finish]);

  return (
    <div
      className="kota-confirm-layer system-load-confirm-layer"
      role="presentation"
      onPointerDown={(event) => {
        if (event.target === event.currentTarget) finish(false);
      }}
    >
      <section
        className="kota-confirm-card plain-copy"
        role="dialog"
        aria-modal="true"
        aria-labelledby="system-load-confirm-title"
        onPointerDown={(event) => event.stopPropagation()}
      >
        <h2 id="system-load-confirm-title">{title}</h2>
        <div className="kota-confirm-actions">
          <button ref={cancelRef} type="button" onClick={() => finish(false)}>
            Cancel
          </button>
          <button ref={proceedRef} type="button" className="confirm" onClick={() => finish(true)}>
            Proceed
          </button>
        </div>
      </section>
    </div>
  );
}
