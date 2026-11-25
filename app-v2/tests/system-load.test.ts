import { describe, expect, it } from 'vitest';
import {
  BYTES_PER_GIB,
  BYTES_PER_MIB,
  SYSTEM_LOAD_CONSTANTS,
  computeBarGeometry,
  createInitialSystemLoadState,
  formatRamGiBNumber,
  formatSwapMbpsLabel,
  formatTotalRamLabel,
  isWithinResetPostLoadCooldown,
  recordSystemLoadPageLoadedAt,
  sanitizeSnapshotForPageLoad,
  stepSystemLoad,
  type SystemLoadSample,
} from '../src/lib/system-load';

function makeSample(overrides: Partial<SystemLoadSample> = {}): SystemLoadSample {
  return {
    atMs: 1_000_000,
    intervalMs: 30_000,
    totalRamBytes: 16 * BYTES_PER_GIB,
    compressedBytes: 2 * BYTES_PER_GIB,
    kotaPageBytes: 1.5 * BYTES_PER_GIB,
    swapBytesPerSec: 0,
    ...overrides,
  };
}

describe('system-load pure rules', () => {
  it('applies enter and leave hysteresis independently on COMPRESSED, KOTA, and 3-sample SWAP', () => {
    let state = createInitialSystemLoadState();

    // Below enter thresholds: all three stay dark.
    state = stepSystemLoad(
      state,
      makeSample({
        compressedBytes: 5.49 * BYTES_PER_GIB,
        kotaPageBytes: 3.99 * BYTES_PER_GIB,
        swapBytesPerSec: 4.9 * BYTES_PER_MIB,
      }),
    );
    expect(state.flags).toEqual({ compressed: false, kota: false, swap: false });

    // Exactly at enter thresholds for COMPRESSED (5.5 GB) and KOTA (4.0 GB);
    // SWAP uses 3-sample average: (4.9 + 5.1) / 2 = 5.0 MB/s -> enters yellow.
    state = stepSystemLoad(
      state,
      makeSample({
        compressedBytes: 5.5 * BYTES_PER_GIB,
        kotaPageBytes: 4.0 * BYTES_PER_GIB,
        swapBytesPerSec: 5.1 * BYTES_PER_MIB,
      }),
    );
    expect(state.flags).toEqual({ compressed: true, kota: true, swap: true });

    // Inside hysteresis deadband (> leave): all three remain latched yellow.
    // SWAP 3-sample window becomes [4.9, 5.1, 0] -> avg 3.33 MB/s > 2.5 MB/s.
    state = stepSystemLoad(
      state,
      makeSample({
        compressedBytes: 5.01 * BYTES_PER_GIB,
        kotaPageBytes: 3.51 * BYTES_PER_GIB,
        swapBytesPerSec: 0,
      }),
    );
    expect(state.flags).toEqual({ compressed: true, kota: true, swap: true });

    // At or below leave thresholds: COMPRESSED <= 5.0 GB, KOTA <= 3.5 GB;
    // SWAP window becomes [5.1, 0, 2.4] -> avg 2.5 MB/s <= 2.5 MB/s -> leaves yellow.
    state = stepSystemLoad(
      state,
      makeSample({
        compressedBytes: 5.0 * BYTES_PER_GIB,
        kotaPageBytes: 3.5 * BYTES_PER_GIB,
        swapBytesPerSec: 2.4 * BYTES_PER_MIB,
      }),
    );
    expect(state.flags).toEqual({ compressed: false, kota: false, swap: false });
  });

  it('drives Y to 1 in 4 hours while KOTA is yellow and cools back to 0 in 4 hours', () => {
    let state = createInitialSystemLoadState();
    const twoHoursMs = 2 * 60 * 60 * 1000;

    state = stepSystemLoad(
      state,
      makeSample({ intervalMs: twoHoursMs, kotaPageBytes: 4.2 * BYTES_PER_GIB }),
    );
    expect(state.y).toBeCloseTo(0.5, 6);
    expect(state.r).toBe(0);
    expect(state.lamp.light).toBeCloseTo(0.5, 6);
    expect(state.lamp.heatRgb).toEqual([239, 195, 111]);

    state = stepSystemLoad(
      state,
      makeSample({ intervalMs: twoHoursMs + 60_000, kotaPageBytes: 4.2 * BYTES_PER_GIB }),
    );
    expect(state.y).toBe(1);
    expect(state.lamp.light).toBe(1);

    // Drop KOTA below leave threshold (3.5 GB): cools at the same 4-hour rate.
    state = stepSystemLoad(
      state,
      makeSample({ intervalMs: twoHoursMs, kotaPageBytes: 3.2 * BYTES_PER_GIB }),
    );
    expect(state.flags.kota).toBe(false);
    expect(state.y).toBeCloseTo(0.5, 6);

    state = stepSystemLoad(
      state,
      makeSample({ intervalMs: twoHoursMs, kotaPageBytes: 3.2 * BYTES_PER_GIB }),
    );
    expect(state.y).toBe(0);
    expect(state.lamp.light).toBe(0);
  });

  it('drives R to 1 in 10 minutes when all three flags are yellow and cools in 20 minutes when any leaves', () => {
    let state = createInitialSystemLoadState();
    const fiveMinMs = 5 * 60 * 1000;
    const tenMinMs = 10 * 60 * 1000;

    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: fiveMinMs,
        compressedBytes: 6.0 * BYTES_PER_GIB,
        kotaPageBytes: 4.5 * BYTES_PER_GIB,
        swapBytesPerSec: 8.0 * BYTES_PER_MIB,
      }),
    );
    expect(state.flags).toEqual({ compressed: true, kota: true, swap: true });
    expect(state.r).toBeCloseTo(0.5, 6);

    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: fiveMinMs + 30_000,
        compressedBytes: 6.0 * BYTES_PER_GIB,
        kotaPageBytes: 4.5 * BYTES_PER_GIB,
        swapBytesPerSec: 8.0 * BYTES_PER_MIB,
      }),
    );
    expect(state.r).toBe(1);
    expect(state.lamp.light).toBe(1);
    expect(state.lamp.heatRgb).toEqual([246, 77, 54]);

    // Leave COMPRESSED while KOTA and SWAP stay yellow: R cools over 20 minutes.
    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: tenMinMs,
        compressedBytes: 4.8 * BYTES_PER_GIB,
        kotaPageBytes: 4.5 * BYTES_PER_GIB,
        swapBytesPerSec: 8.0 * BYTES_PER_MIB,
      }),
    );
    expect(state.flags).toEqual({ compressed: false, kota: true, swap: true });
    expect(state.r).toBeCloseTo(0.5, 6);

    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: tenMinMs,
        compressedBytes: 4.8 * BYTES_PER_GIB,
        kotaPageBytes: 4.5 * BYTES_PER_GIB,
        swapBytesPerSec: 8.0 * BYTES_PER_MIB,
      }),
    );
    expect(state.r).toBe(0);
  });

  it('does not advance Y or R when intervalMs is null (initial sample or >75s sleep gap)', () => {
    let state = createInitialSystemLoadState();
    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: null,
        compressedBytes: 7.0 * BYTES_PER_GIB,
        kotaPageBytes: 5.0 * BYTES_PER_GIB,
        swapBytesPerSec: null,
      }),
    );
    expect(state.flags.compressed).toBe(true);
    expect(state.flags.kota).toBe(true);
    expect(state.y).toBe(0);
    expect(state.r).toBe(0);

    // Advance Y slightly on a valid interval, then simulate a sleep gap (intervalMs: null).
    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: 30 * 60 * 1000,
        compressedBytes: 7.0 * BYTES_PER_GIB,
        kotaPageBytes: 5.0 * BYTES_PER_GIB,
        swapBytesPerSec: 10 * BYTES_PER_MIB,
      }),
    );
    const yBeforeSleep = state.y;
    const rBeforeSleep = state.r;
    expect(yBeforeSleep).toBeGreaterThan(0);
    expect(rBeforeSleep).toBe(1);

    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: null,
        compressedBytes: 2.0 * BYTES_PER_GIB,
        kotaPageBytes: 1.0 * BYTES_PER_GIB,
        swapBytesPerSec: null,
      }),
    );
    expect(state.y).toBe(yBeforeSleep);
    expect(state.r).toBe(rBeforeSleep);

    // When swap counter rolls back (swapBytesPerSec: null) on a valid intervalMs,
    // swapRates keeps prior valid samples while dt still advances Y/R normally.
    const nextValidInterval = stepSystemLoad(
      state,
      makeSample({
        intervalMs: 60_000,
        compressedBytes: 7.0 * BYTES_PER_GIB,
        kotaPageBytes: 5.0 * BYTES_PER_GIB,
        swapBytesPerSec: null,
      }),
    );
    expect(nextValidInterval.swapRates).toEqual([10 * BYTES_PER_MIB]);
    expect(nextValidInterval.y).toBeGreaterThan(yBeforeSleep);
  });

  it('clears the KOTA flag, hides bar fill, shows --, and never heats the lamp when kotaPageBytes is null', () => {
    let state = createInitialSystemLoadState();
    state = stepSystemLoad(
      state,
      makeSample({
        intervalMs: 60 * 60 * 1000,
        compressedBytes: 6.5 * BYTES_PER_GIB,
        kotaPageBytes: null,
        swapBytesPerSec: 12 * BYTES_PER_MIB,
      }),
    );
    expect(state.flags.kota).toBe(false);
    expect(state.y).toBe(0);
    expect(state.r).toBe(0);
    expect(state.lamp.light).toBe(0);
    expect(formatRamGiBNumber(state.kotaPageBytes, state.hasSample)).toBe('--');
    expect(formatTotalRamLabel(state.totalRamBytes, state.hasSample)).toBe('TOTAL RAM 16 GB');

    const kotaBar = computeBarGeometry(
      state.kotaPageBytes,
      state.totalRamBytes,
      SYSTEM_LOAD_CONSTANTS.kotaEnterBytes,
      state.flags.kota,
      state.hasSample,
    );
    expect(kotaBar.fillPct).toBe(0);
    expect(kotaBar.excessWidthPct).toBe(0);
  });

  it('caps R gain at 15% when the 90-second SWAP average spikes for 1.5 minutes and caps visual bar at 20 MB/s', () => {
    let state = createInitialSystemLoadState();
    // Pre-existing high compressed + kota with zero swap for 3 samples.
    for (let i = 0; i < 3; i += 1) {
      state = stepSystemLoad(
        state,
        makeSample({
          atMs: i * 30_000,
          intervalMs: i === 0 ? null : 30_000,
          compressedBytes: 6.2 * BYTES_PER_GIB,
          kotaPageBytes: 4.2 * BYTES_PER_GIB,
          swapBytesPerSec: 0,
        }),
      );
    }
    expect(state.r).toBe(0);

    // A 39 MB/s single-sample burst keeps the 3-sample (90s) moving average at 13 MB/s
    // for 1.5 minutes (3 samples) before returning to 0 on the 4th sample.
    const ratesMbps = [39, 0, 0, 0, 0];
    let peakR = 0;
    let firstSampleState = state;
    ratesMbps.forEach((mbps, index) => {
      state = stepSystemLoad(
        state,
        makeSample({
          atMs: 90_000 + (index + 1) * 30_000,
          intervalMs: 30_000,
          compressedBytes: 6.2 * BYTES_PER_GIB,
          kotaPageBytes: 4.2 * BYTES_PER_GIB,
          swapBytesPerSec: mbps * BYTES_PER_MIB,
        }),
      );
      if (index === 0) firstSampleState = state;
      peakR = Math.max(peakR, state.r);
    });

    expect(peakR).toBeCloseTo(0.15, 6);
    expect(state.flags.swap).toBe(false);
    expect(state.r).toBeLessThan(peakR);

    // Also verify >20 MB/s average clamps bar geometry at 100% while displaying real MBps.
    const highAvgState = stepSystemLoad(
      stepSystemLoad(
        firstSampleState,
        makeSample({ atMs: 300_000, intervalMs: 30_000, swapBytesPerSec: 39 * BYTES_PER_MIB }),
      ),
      makeSample({ atMs: 330_000, intervalMs: 30_000, swapBytesPerSec: 39 * BYTES_PER_MIB }),
    );
    expect(formatSwapMbpsLabel(highAvgState.swapAvgBytesPerSec)).toBe('39.00 MBps');
    const swapBar = computeBarGeometry(
      highAvgState.swapAvgBytesPerSec,
      SYSTEM_LOAD_CONSTANTS.swapMaxBytesPerSec,
      SYSTEM_LOAD_CONSTANTS.swapEnterBytesPerSec,
      highAvgState.flags.swap,
      highAvgState.hasSample,
    );
    expect(swapBar.fillPct).toBe(100);
    expect(swapBar.markPct).toBe(25);
    expect(swapBar.excessWidthPct).toBe(75);
  });

  it('enforces the 10-second post-load Reset cooldown in sessionStorage', () => {
    const bag = new Map<string, string>();
    const storage = {
      getItem: (key: string) => bag.get(key) ?? null,
      setItem: (key: string, value: string) => {
        bag.set(key, value);
      },
    };

    expect(isWithinResetPostLoadCooldown(50_000, storage)).toBe(false);
    recordSystemLoadPageLoadedAt(50_000, storage);
    expect(isWithinResetPostLoadCooldown(50_000, storage)).toBe(true);
    expect(isWithinResetPostLoadCooldown(59_999, storage)).toBe(true);
    expect(isWithinResetPostLoadCooldown(60_000, storage)).toBe(false);
  });

  it('nulls kotaPageBytes and intervalMs on snapshots sampled before page load while preserving machine readings', () => {
    const preReloadSnapshot = makeSample({
      atMs: 49_500,
      intervalMs: 30_000,
      totalRamBytes: 16 * BYTES_PER_GIB,
      compressedBytes: 6.1 * BYTES_PER_GIB,
      kotaPageBytes: 5.2 * BYTES_PER_GIB,
      swapBytesPerSec: 1.5 * BYTES_PER_MIB,
    });

    const sanitized = sanitizeSnapshotForPageLoad(preReloadSnapshot, 50_000);
    expect(sanitized).toEqual({
      atMs: 49_500,
      intervalMs: null,
      totalRamBytes: 16 * BYTES_PER_GIB,
      compressedBytes: 6.1 * BYTES_PER_GIB,
      kotaPageBytes: null,
      swapBytesPerSec: 1.5 * BYTES_PER_MIB,
    });

    // Post-load snapshot at or after loadedAtMs is kept intact.
    expect(sanitizeSnapshotForPageLoad({ ...preReloadSnapshot, atMs: 50_000 }, 50_000)).toEqual({
      ...preReloadSnapshot,
      atMs: 50_000,
    });
  });
});
