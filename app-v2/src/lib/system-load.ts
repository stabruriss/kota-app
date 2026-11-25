import { beginResetRelayBeforeReload } from './reset-relay';

export type SystemLoadSample = {
  atMs: number;
  intervalMs: number | null;
  totalRamBytes: number;
  compressedBytes: number;
  kotaPageBytes: number | null;
  swapBytesPerSec: number | null;
};

export const BYTES_PER_MIB = 1024 ** 2;
export const BYTES_PER_GIB = 1024 ** 3;

export const SYSTEM_LOAD_CONSTANTS = {
  compressedEnterBytes: 5.5 * BYTES_PER_GIB,
  compressedLeaveBytes: 5.0 * BYTES_PER_GIB,
  kotaEnterBytes: 4.0 * BYTES_PER_GIB,
  kotaLeaveBytes: 3.5 * BYTES_PER_GIB,
  swapWindowSize: 3,
  swapEnterBytesPerSec: 5.0 * BYTES_PER_MIB,
  swapLeaveBytesPerSec: 2.5 * BYTES_PER_MIB,
  swapMaxBytesPerSec: 20.0 * BYTES_PER_MIB,
  yellowRiseMs: 4 * 60 * 60 * 1000,
  yellowFallMs: 4 * 60 * 60 * 1000,
  redRiseMs: 10 * 60 * 1000,
  redFallMs: 20 * 60 * 1000,
  yellowRgb: [239, 195, 111] as const,
  redRgb: [246, 77, 54] as const,
  neutralButtonRgb: [151, 169, 170] as const,
  resetPostLoadCooldownMs: 10_000,
  pageLoadedAtSessionKey: 'kota-v2.system-load.loaded-at',
} as const;

export type SystemLoadFlags = {
  compressed: boolean;
  kota: boolean;
  swap: boolean;
};

export type SystemLoadLamp = {
  y: number;
  r: number;
  light: number;
  heatRgb: readonly [number, number, number];
  buttonRgb: readonly [number, number, number];
  buttonBorderRgb: readonly [number, number, number];
};

export type SystemLoadState = {
  hasSample: boolean;
  totalRamBytes: number;
  compressedBytes: number;
  kotaPageBytes: number | null;
  swapRates: readonly number[];
  swapAvgBytesPerSec: number;
  flags: SystemLoadFlags;
  y: number;
  r: number;
  lamp: SystemLoadLamp;
};

export type SystemLoadDebugOverride = {
  totalRamGb?: number;
  compressedGb?: number;
  kotaGb?: number | null;
  swapMbps?: number;
  y?: number;
  r?: number;
};

export type SystemLoadBarGeometry = {
  fillPct: number;
  markPct: number;
  excessLeftPct: number;
  excessWidthPct: number;
};

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

export function latchHysteresis(
  before: boolean,
  value: number,
  enter: number,
  leave: number,
): boolean {
  return before ? value > leave : value >= enter;
}

export function computeSystemLoadLamp(yRaw: number, rRaw: number): SystemLoadLamp {
  const y = clamp01(yRaw);
  const r = clamp01(rRaw);
  const light = Math.max(y, r);
  const { yellowRgb, redRgb, neutralButtonRgb } = SYSTEM_LOAD_CONSTANTS;
  const heatRgb: [number, number, number] = [
    Math.round(yellowRgb[0] + (redRgb[0] - yellowRgb[0]) * r),
    Math.round(yellowRgb[1] + (redRgb[1] - yellowRgb[1]) * r),
    Math.round(yellowRgb[2] + (redRgb[2] - yellowRgb[2]) * r),
  ];
  const buttonRgb: [number, number, number] = [
    Math.round(neutralButtonRgb[0] + (heatRgb[0] - neutralButtonRgb[0]) * light),
    Math.round(neutralButtonRgb[1] + (heatRgb[1] - neutralButtonRgb[1]) * light),
    Math.round(neutralButtonRgb[2] + (heatRgb[2] - neutralButtonRgb[2]) * light),
  ];
  const buttonBorderRgb: [number, number, number] = [
    Math.round(buttonRgb[0] * 0.8),
    Math.round(buttonRgb[1] * 0.8),
    Math.round(buttonRgb[2] * 0.8),
  ];
  return { y, r, light, heatRgb, buttonRgb, buttonBorderRgb };
}

export function createInitialSystemLoadState(): SystemLoadState {
  return {
    hasSample: false,
    totalRamBytes: 0,
    compressedBytes: 0,
    kotaPageBytes: null,
    swapRates: [],
    swapAvgBytesPerSec: 0,
    flags: {
      compressed: false,
      kota: false,
      swap: false,
    },
    y: 0,
    r: 0,
    lamp: computeSystemLoadLamp(0, 0),
  };
}

export function stepSystemLoad(
  prev: SystemLoadState,
  sample: SystemLoadSample,
): SystemLoadState {
  const swapRates =
    sample.swapBytesPerSec !== null
      ? [...prev.swapRates, sample.swapBytesPerSec].slice(-SYSTEM_LOAD_CONSTANTS.swapWindowSize)
      : prev.swapRates;
  const swapAvgBytesPerSec =
    swapRates.length > 0
      ? swapRates.reduce((sum, rate) => sum + rate, 0) / swapRates.length
      : 0;

  const { totalRamBytes, compressedBytes, kotaPageBytes } = sample;

  const flags: SystemLoadFlags = {
    compressed: latchHysteresis(
      prev.flags.compressed,
      compressedBytes,
      SYSTEM_LOAD_CONSTANTS.compressedEnterBytes,
      SYSTEM_LOAD_CONSTANTS.compressedLeaveBytes,
    ),
    kota:
      kotaPageBytes === null
        ? false
        : latchHysteresis(
            prev.flags.kota,
            kotaPageBytes,
            SYSTEM_LOAD_CONSTANTS.kotaEnterBytes,
            SYSTEM_LOAD_CONSTANTS.kotaLeaveBytes,
          ),
    swap: latchHysteresis(
      prev.flags.swap,
      swapAvgBytesPerSec,
      SYSTEM_LOAD_CONSTANTS.swapEnterBytesPerSec,
      SYSTEM_LOAD_CONSTANTS.swapLeaveBytesPerSec,
    ),
  };

  const dt = sample.intervalMs ?? 0;
  const y = clamp01(
    prev.y + (flags.kota ? dt / SYSTEM_LOAD_CONSTANTS.yellowRiseMs : -dt / SYSTEM_LOAD_CONSTANTS.yellowFallMs),
  );
  const allYellow = flags.compressed && flags.kota && flags.swap;
  const r = clamp01(
    prev.r + (allYellow ? dt / SYSTEM_LOAD_CONSTANTS.redRiseMs : -dt / SYSTEM_LOAD_CONSTANTS.redFallMs),
  );

  return {
    hasSample: true,
    totalRamBytes,
    compressedBytes,
    kotaPageBytes,
    swapRates,
    swapAvgBytesPerSec,
    flags,
    y,
    r,
    lamp: computeSystemLoadLamp(y, r),
  };
}

export function forceSystemLoadState(
  prev: SystemLoadState,
  override: SystemLoadDebugOverride,
): SystemLoadState {
  const totalRamBytes =
    override.totalRamGb != null && Number.isFinite(override.totalRamGb) && override.totalRamGb > 0
      ? override.totalRamGb * BYTES_PER_GIB
      : prev.totalRamBytes > 0
        ? prev.totalRamBytes
        : 16 * BYTES_PER_GIB;
  const compressedBytes =
    override.compressedGb != null && Number.isFinite(override.compressedGb)
      ? Math.max(0, override.compressedGb) * BYTES_PER_GIB
      : prev.compressedBytes;
  const kotaPageBytes =
    override.kotaGb === null
      ? null
      : override.kotaGb !== undefined && Number.isFinite(override.kotaGb)
        ? Math.max(0, override.kotaGb) * BYTES_PER_GIB
        : prev.kotaPageBytes;
  const swapAvgBytesPerSec =
    override.swapMbps != null && Number.isFinite(override.swapMbps)
      ? Math.max(0, override.swapMbps) * BYTES_PER_MIB
      : prev.swapAvgBytesPerSec;
  const swapRates =
    override.swapMbps != null && Number.isFinite(override.swapMbps)
      ? [swapAvgBytesPerSec]
      : prev.swapRates;

  const flags: SystemLoadFlags = {
    compressed:
      override.compressedGb !== undefined
        ? latchHysteresis(
            false,
            compressedBytes,
            SYSTEM_LOAD_CONSTANTS.compressedEnterBytes,
            SYSTEM_LOAD_CONSTANTS.compressedLeaveBytes,
          )
        : prev.flags.compressed,
    kota:
      kotaPageBytes == null
        ? false
        : override.kotaGb !== undefined
          ? latchHysteresis(
              false,
              kotaPageBytes,
              SYSTEM_LOAD_CONSTANTS.kotaEnterBytes,
              SYSTEM_LOAD_CONSTANTS.kotaLeaveBytes,
            )
          : prev.flags.kota,
    swap:
      override.swapMbps !== undefined
        ? latchHysteresis(
            false,
            swapAvgBytesPerSec,
            SYSTEM_LOAD_CONSTANTS.swapEnterBytesPerSec,
            SYSTEM_LOAD_CONSTANTS.swapLeaveBytesPerSec,
          )
        : prev.flags.swap,
  };
  const y = override.y != null ? clamp01(override.y) : prev.y;
  const r = override.r != null ? clamp01(override.r) : prev.r;

  return {
    hasSample: true,
    totalRamBytes,
    compressedBytes,
    kotaPageBytes,
    swapRates,
    swapAvgBytesPerSec,
    flags,
    y,
    r,
    lamp: computeSystemLoadLamp(y, r),
  };
}

export function formatTotalRamLabel(totalRamBytes: number, hasSample: boolean): string {
  if (!hasSample || !(totalRamBytes > 0)) return 'TOTAL RAM -- GB';
  const gib = totalRamBytes / BYTES_PER_GIB;
  const rounded = Math.round(gib * 10) / 10;
  const text = Number.isInteger(rounded) ? String(rounded) : rounded.toFixed(1);
  return `TOTAL RAM ${text} GB`;
}

export function formatRamGiBNumber(bytes: number | null, hasSample: boolean): string {
  if (!hasSample || bytes == null || !Number.isFinite(bytes)) return '--';
  return (Math.max(0, bytes) / BYTES_PER_GIB).toFixed(1);
}

export function formatSwapMbpsLabel(bytesPerSec: number): string {
  const value = Number.isFinite(bytesPerSec) ? Math.max(0, bytesPerSec) / BYTES_PER_MIB : 0;
  return `${value.toFixed(2)} MBps`;
}

export function computeBarGeometry(
  value: number | null,
  capacity: number,
  enterThreshold: number,
  yellow: boolean,
  hasSample: boolean,
): SystemLoadBarGeometry {
  if (!(capacity > 0)) {
    return { fillPct: 0, markPct: 0, excessLeftPct: 0, excessWidthPct: 0 };
  }
  const markRatio = clamp01(enterThreshold / capacity);
  if (!hasSample || value == null || !Number.isFinite(value)) {
    return {
      fillPct: 0,
      markPct: markRatio * 100,
      excessLeftPct: markRatio * 100,
      excessWidthPct: 0,
    };
  }
  const usedRatio = clamp01(value / capacity);
  const excessRatio = yellow ? Math.max(0, usedRatio - markRatio) : 0;
  return {
    fillPct: usedRatio * 100,
    markPct: markRatio * 100,
    excessLeftPct: markRatio * 100,
    excessWidthPct: excessRatio * 100,
  };
}

export function lampCssVariables(lamp: SystemLoadLamp): Record<string, string> {
  return {
    '--heat-rgb': lamp.heatRgb.join(','),
    '--light': String(lamp.light),
    '--light-soft': String(lamp.light * 0.28),
    '--light-line': String(lamp.light * 0.55),
    '--light-core': String(lamp.light * 0.85),
    '--button-rgb': `rgb(${lamp.buttonRgb.join(',')})`,
    '--button-border': `rgb(${lamp.buttonBorderRgb.join(',')})`,
  };
}

export function isSameLampAppearance(left: SystemLoadLamp, right: SystemLoadLamp): boolean {
  if (left.light === 0 && right.light === 0) return true;
  return left.y === right.y && left.r === right.r && left.light === right.light;
}

export function recordSystemLoadPageLoadedAt(
  nowMs: number = Date.now(),
  storage?: Pick<Storage, 'setItem'> | null,
): void {
  const target = storage ?? (typeof window !== 'undefined' ? window.sessionStorage : null);
  if (!target) return;
  try {
    target.setItem(SYSTEM_LOAD_CONSTANTS.pageLoadedAtSessionKey, String(nowMs));
  } catch {
    // Ignore storage quota or sandbox errors.
  }
}

export function readSystemLoadPageLoadedAt(
  storage?: Pick<Storage, 'getItem'> | null,
): number | null {
  const target = storage ?? (typeof window !== 'undefined' ? window.sessionStorage : null);
  if (!target) return null;
  try {
    const raw = target.getItem(SYSTEM_LOAD_CONSTANTS.pageLoadedAtSessionKey);
    if (!raw) return null;
    const loadedAt = Number(raw);
    return Number.isFinite(loadedAt) ? loadedAt : null;
  } catch {
    return null;
  }
}

export function sanitizeSnapshotForPageLoad(
  snapshot: SystemLoadSample,
  pageLoadedAtMs: number | null = readSystemLoadPageLoadedAt(),
): SystemLoadSample {
  if (pageLoadedAtMs !== null && snapshot.atMs < pageLoadedAtMs) {
    return {
      ...snapshot,
      intervalMs: null,
      kotaPageBytes: null,
    };
  }
  return snapshot;
}

export function isWithinResetPostLoadCooldown(
  nowMs: number = Date.now(),
  storage?: Pick<Storage, 'getItem'> | null,
): boolean {
  const loadedAt = readSystemLoadPageLoadedAt(storage);
  if (loadedAt === null) return false;
  return nowMs >= loadedAt && nowMs - loadedAt < SYSTEM_LOAD_CONSTANTS.resetPostLoadCooldownMs;
}

export function reloadKotaWindow(reload?: () => void): void {
  if (typeof window !== 'undefined') {
    beginResetRelayBeforeReload({ reload });
  }
}
