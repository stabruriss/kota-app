import {
  onVioletRoomChanged,
  syncVioletRoom,
  type VioletRoomRequest,
  type VioletRoomState,
} from '../pty-client';

const BURST_DELAYS_MS = [0, 1500, 5000, 12000, 25000] as const;
const NATIVE_CHANGE_DEBOUNCE_MS = 180;
const WORKING_RECONCILE_MS = 10000;
const IDLE_RECONCILE_MS = 90000;
const DEFAULT_LIMIT = 100;
const PROJECT_AGENT_ID_IN_PATH = /\bagent-(?!workspaces\b)[a-z0-9]{10}\b/gi;

export interface VioletProjectSyncOptions {
  projectRoot: string | null;
  roomAgentIds: readonly string[];
  workingAgentIds: readonly string[];
  foreground?: boolean;
}

export interface VioletProjectSyncHandle {
  update(options: VioletProjectSyncOptions): void;
  dispose(): void;
}

type SyncReason = 'bootstrap' | 'prompt' | 'agent' | 'native-change' | 'working-burst' | 'reconcile';

interface PendingSync {
  reason: SyncReason;
  agentIds: Set<string> | null;
  watchAgentIds: Set<string> | null;
  limit: number;
}

interface EngineSubscription {
  projectRoot: string | null;
  roomAgentIds: string[];
  workingAgentIds: string[];
  foreground: boolean;
}

const ENGINES = new Map<string, VioletProjectSyncEngine>();

export function connectVioletProjectSyncEngine(
  options: VioletProjectSyncOptions,
): VioletProjectSyncHandle {
  const key = engineKey(options.projectRoot);
  let engine = ENGINES.get(key);
  if (!engine) {
    engine = new VioletProjectSyncEngine(options.projectRoot);
    ENGINES.set(key, engine);
  }
  const subscriptionId = engine.addSubscription(options);
  return {
    update(next) {
      engine?.updateSubscription(subscriptionId, next);
    },
    dispose() {
      if (!engine) return;
      if (engine.releaseSubscription(subscriptionId)) {
        ENGINES.delete(key);
      }
      engine = undefined;
    },
  };
}

export function requestVioletProjectPromptSync(
  projectRoot: string | null,
  targetAgentIds: readonly string[],
  roomAgentIds: readonly string[],
): void {
  const agentIds = normalizeIds(targetAgentIds);
  if (agentIds.length === 0) return;
  const engine = ENGINES.get(engineKey(projectRoot));
  if (engine) {
    engine.requestPrompt(agentIds, roomAgentIds);
    return;
  }
  void syncVioletRoom({
    projectRoot,
    limit: DEFAULT_LIMIT,
    agentIds,
    watchAgentIds: normalizeIds(roomAgentIds),
  }).catch(() => {});
}

export function requestVioletProjectAgentSync(
  projectRoot: string | null,
  targetAgentIds: readonly string[],
  roomAgentIds: readonly string[],
): void {
  const agentIds = normalizeIds(targetAgentIds);
  if (agentIds.length === 0) return;
  const engine = ENGINES.get(engineKey(projectRoot));
  if (engine) {
    engine.requestAgent(agentIds, roomAgentIds);
    return;
  }
  void syncVioletRoom({
    projectRoot,
    limit: DEFAULT_LIMIT,
    agentIds,
    watchAgentIds: normalizeIds(roomAgentIds),
  }).catch(() => {});
}

export function syncVioletProjectAgentsNow(
  projectRoot: string | null,
  targetAgentIds: readonly string[],
): Promise<VioletRoomState> {
  const agentIds = normalizeIds(targetAgentIds);
  const engine = ENGINES.get(engineKey(projectRoot));
  if (engine) return engine.syncAgentsNow(agentIds);
  // Without an active engine there is no authoritative frontend roster.
  // A full sync lets the backend discover the roster instead of replacing
  // the project watcher with only the agent that just exited.
  return syncVioletRoom({
    projectRoot,
    limit: DEFAULT_LIMIT,
    agentIds: null,
    watchAgentIds: null,
  });
}

class VioletProjectSyncEngine {
  private nextSubscriptionId = 1;
  private subscriptions = new Map<number, EngineSubscription>();
  private foreground = false;
  private roomAgentIds: string[] = [];
  private workingAgentIds: string[] = [];
  private pending: PendingSync | null = null;
  private syncTimer: number | null = null;
  private reconcileTimer: number | null = null;
  private burstTimers: number[] = [];
  private burstAgentIds: string[] = [];
  private burstGeneration = 0;
  private inFlight = false;
  private rerunAfterFlight = false;
  private stopped = false;
  private unlistenChanged: (() => void) | null = null;
  private syncTail: Promise<void> = Promise.resolve();

  constructor(private readonly projectRoot: string | null) {
    void onVioletRoomChanged((payload) => {
      if (this.stopped) return;
      if (!violetSyncMatchesProject(this.projectRoot, payload.projectRoot)) return;
      const pathAgentIds = agentIdsFromChangedPaths(payload.paths, this.roomAgentIds);
      this.scheduleSync('native-change', NATIVE_CHANGE_DEBOUNCE_MS, {
        agentIds: payload.reason === 'actor-message'
          ? null
          : pathAgentIds.length > 0
            ? pathAgentIds
            : this.roomAgentIds.length > 0
              ? this.roomAgentIds
              : null,
        limit: DEFAULT_LIMIT,
      });
    }).then((unlisten) => {
      if (this.stopped) {
        void unlisten();
        return;
      }
      this.unlistenChanged = unlisten;
    });
  }

  addSubscription(options: VioletProjectSyncOptions): number {
    const id = this.nextSubscriptionId;
    this.nextSubscriptionId += 1;
    this.subscriptions.set(id, normalizeSubscription(options));
    this.applyMergedSubscriptions();
    return id;
  }

  releaseSubscription(id: number): boolean {
    this.subscriptions.delete(id);
    if (this.subscriptions.size > 0) {
      this.applyMergedSubscriptions();
      return false;
    }
    this.stop();
    return true;
  }

  updateSubscription(id: number, options: VioletProjectSyncOptions): void {
    if (!this.subscriptions.has(id)) return;
    this.subscriptions.set(id, normalizeSubscription(options));
    this.applyMergedSubscriptions();
  }

  private applyMergedSubscriptions(): void {
    if (this.stopped) return;
    const merged = mergeSubscriptions(this.subscriptions.values());
    const nextRoom = merged.roomAgentIds;
    const nextWorking = merged.workingAgentIds;
    const nextForeground = merged.foreground;
    const roomChanged = !sameIds(this.roomAgentIds, nextRoom);
    const workingChanged = !sameIds(this.workingAgentIds, nextWorking);
    const foregroundChanged = this.foreground !== nextForeground;

    this.foreground = nextForeground;
    this.roomAgentIds = nextRoom;
    this.workingAgentIds = nextWorking;

    if (roomChanged) {
      this.scheduleSync('bootstrap', 0, {
        agentIds: null,
        limit: DEFAULT_LIMIT,
      });
    }
    if (workingChanged || foregroundChanged) {
      this.scheduleWorkingBurst();
      this.scheduleReconcile();
    }
    if (!this.reconcileTimer) {
      this.scheduleReconcile();
    }
  }

  requestPrompt(targetAgentIds: readonly string[], roomAgentIds: readonly string[]): void {
    if (this.stopped) return;
    const promptAgentIds = normalizeIds(targetAgentIds);
    const nextRoom = normalizeIds(roomAgentIds);
    this.scheduleSync('prompt', 0, {
      agentIds: promptAgentIds,
      watchAgentIds: nextRoom.length > 0 ? nextRoom : null,
      limit: DEFAULT_LIMIT,
    });
    this.scheduleWorkingBurst(promptAgentIds);
    this.scheduleReconcile();
  }

  requestAgent(targetAgentIds: readonly string[], roomAgentIds: readonly string[]): void {
    if (this.stopped) return;
    const agentIds = normalizeIds(targetAgentIds);
    if (agentIds.length === 0) return;
    const nextRoom = normalizeIds(roomAgentIds);
    this.scheduleSync('agent', 0, {
      agentIds,
      watchAgentIds: nextRoom.length > 0 ? nextRoom : null,
      limit: DEFAULT_LIMIT,
    });
    this.scheduleReconcile();
  }

  syncAgentsNow(targetAgentIds: readonly string[]): Promise<VioletRoomState> {
    const agentIds = normalizeIds(targetAgentIds);
    const hasAuthoritativeRoster = this.roomAgentIds.length > 0;
    return this.enqueueSync({
      projectRoot: this.projectRoot,
      limit: DEFAULT_LIMIT,
      agentIds: hasAuthoritativeRoster ? agentIds : null,
      watchAgentIds: hasAuthoritativeRoster ? this.roomAgentIds : null,
    });
  }

  private scheduleWorkingBurst(agentIds: readonly string[] = this.workingAgentIds): void {
    this.burstAgentIds = normalizeIds([
      ...this.burstAgentIds,
      ...agentIds,
      ...this.workingAgentIds,
    ]);
    this.clearBurstTimers();
    const burstAgentIds = this.burstAgentIds;
    if (burstAgentIds.length === 0) return;
    const generation = ++this.burstGeneration;
    const finalDelay = BURST_DELAYS_MS[BURST_DELAYS_MS.length - 1];
    this.burstTimers = BURST_DELAYS_MS.map((delay) => window.setTimeout(() => {
      const agentIds = normalizeIds([
        ...this.burstAgentIds,
        ...this.workingAgentIds,
      ]);
      this.scheduleSync('working-burst', 0, {
        agentIds,
        limit: DEFAULT_LIMIT,
      });
      if (delay === finalDelay && this.burstGeneration === generation) {
        this.burstAgentIds = [];
      }
    }, delay));
  }

  private scheduleReconcile(): void {
    if (this.reconcileTimer !== null) {
      window.clearTimeout(this.reconcileTimer);
      this.reconcileTimer = null;
    }
    const delay = this.workingAgentIds.length > 0 ? WORKING_RECONCILE_MS : IDLE_RECONCILE_MS;
    this.reconcileTimer = window.setTimeout(() => {
      this.reconcileTimer = null;
      const reconcileAgentIds = this.foreground && this.roomAgentIds.length > 0
        ? this.roomAgentIds
        : this.workingAgentIds.length > 0
          ? this.workingAgentIds
          : null;
      this.scheduleSync('reconcile', 0, {
        agentIds: reconcileAgentIds,
        limit: DEFAULT_LIMIT,
      });
      this.scheduleReconcile();
    }, delay);
  }

  private scheduleSync(
    reason: SyncReason,
    delayMs: number,
    options: { agentIds: readonly string[] | null; watchAgentIds?: readonly string[] | null; limit: number },
  ): void {
    if (this.stopped) return;
    this.pending = mergePendingSync(this.pending, {
      reason,
      agentIds: options.agentIds ? new Set(normalizeIds(options.agentIds)) : null,
      watchAgentIds: options.watchAgentIds ? new Set(normalizeIds(options.watchAgentIds)) : null,
      limit: options.limit,
    });
    if (this.syncTimer !== null) {
      if (delayMs > 0) return;
      window.clearTimeout(this.syncTimer);
    }
    this.syncTimer = window.setTimeout(() => {
      this.syncTimer = null;
      void this.flush();
    }, delayMs);
  }

  private async flush(): Promise<void> {
    if (this.stopped || !this.pending) return;
    if (this.inFlight) {
      this.rerunAfterFlight = true;
      return;
    }
    const pending = this.pending;
    this.pending = null;
    this.inFlight = true;
    try {
      const request: VioletRoomRequest = {
        projectRoot: this.projectRoot,
        limit: pending.limit,
        agentIds: pending.agentIds ? Array.from(pending.agentIds) : null,
        watchAgentIds: pending.watchAgentIds
          ? Array.from(pending.watchAgentIds)
          : this.roomAgentIds.length > 0
            ? this.roomAgentIds
            : null,
      };
      await this.enqueueSync(request);
    } catch {
      // Best-effort; the next native-change/reconcile/prompt will retry.
    } finally {
      this.inFlight = false;
      if (this.rerunAfterFlight) {
        this.rerunAfterFlight = false;
        this.scheduleSync(pending.reason, 0, {
          agentIds: pending.agentIds ? Array.from(pending.agentIds) : null,
          watchAgentIds: pending.watchAgentIds ? Array.from(pending.watchAgentIds) : null,
          limit: pending.limit,
        });
      }
    }
  }

  private stop(): void {
    this.stopped = true;
    if (this.syncTimer !== null) window.clearTimeout(this.syncTimer);
    if (this.reconcileTimer !== null) window.clearTimeout(this.reconcileTimer);
    this.clearBurstTimers();
    this.burstAgentIds = [];
    this.burstGeneration += 1;
    if (this.unlistenChanged) void this.unlistenChanged();
    this.syncTimer = null;
    this.reconcileTimer = null;
    this.unlistenChanged = null;
    this.pending = null;
  }

  private clearBurstTimers(): void {
    for (const timer of this.burstTimers) window.clearTimeout(timer);
    this.burstTimers = [];
  }

  private enqueueSync(request: VioletRoomRequest): Promise<VioletRoomState> {
    const run = this.syncTail
      .catch(() => undefined)
      .then(() => syncVioletRoom(request));
    this.syncTail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }
}

function mergePendingSync(left: PendingSync | null, right: PendingSync): PendingSync {
  if (!left) return right;
  const agentIds = left.agentIds === null || right.agentIds === null
    ? null
    : new Set([...left.agentIds, ...right.agentIds]);
  const watchAgentIds = mergeOptionalIdSets(left.watchAgentIds, right.watchAgentIds);
  return {
    reason: right.reason,
    agentIds,
    watchAgentIds,
    limit: Math.max(left.limit, right.limit),
  };
}

function mergeOptionalIdSets(left: Set<string> | null, right: Set<string> | null): Set<string> | null {
  if (!left || !right) return null;
  return new Set([...left, ...right]);
}

function normalizeSubscription(options: VioletProjectSyncOptions): EngineSubscription {
  return {
    projectRoot: options.projectRoot,
    roomAgentIds: normalizeIds(options.roomAgentIds),
    workingAgentIds: normalizeIds(options.workingAgentIds),
    foreground: Boolean(options.foreground),
  };
}

function mergeSubscriptions(
  subscriptions: Iterable<EngineSubscription>,
): EngineSubscription {
  const roomAgentIds = new Set<string>();
  const workingAgentIds = new Set<string>();
  let foreground = false;
  let projectRoot: string | null = null;
  for (const subscription of subscriptions) {
    projectRoot = subscription.projectRoot;
    foreground ||= subscription.foreground;
    for (const agentId of subscription.roomAgentIds) roomAgentIds.add(agentId);
    for (const agentId of subscription.workingAgentIds) workingAgentIds.add(agentId);
  }
  return {
    projectRoot,
    roomAgentIds: normalizeIds(Array.from(roomAgentIds)),
    workingAgentIds: normalizeIds(Array.from(workingAgentIds)),
    foreground,
  };
}

function normalizeIds(ids: readonly string[]): string[] {
  return Array.from(new Set(ids.filter(Boolean))).sort();
}

function sameIds(left: readonly string[], right: readonly string[]): boolean {
  if (left.length !== right.length) return false;
  return left.every((item, index) => item === right[index]);
}

function agentIdsFromChangedPaths(
  paths: readonly string[] | undefined,
  roomAgentIds: readonly string[],
): string[] {
  if (!paths || paths.length === 0) return [];
  const found = new Set<string>();
  const roomIds = normalizeIds(roomAgentIds);
  if (roomIds.length > 0) {
    for (const path of paths) {
      for (const agentId of roomIds) {
        if (path.includes(agentId)) found.add(agentId);
      }
    }
    return normalizeIds(Array.from(found));
  }

  for (const path of paths) {
    for (const match of path.matchAll(PROJECT_AGENT_ID_IN_PATH)) {
      found.add(match[0].toLowerCase());
    }
  }
  return normalizeIds(Array.from(found));
}

function engineKey(projectRoot: string | null): string {
  return projectRoot ?? '__active_workspace__';
}

function violetSyncMatchesProject(left: string | null, right: string | null): boolean {
  if (!left || !right) return true;
  return left === right;
}
