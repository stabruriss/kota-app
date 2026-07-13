import { describe, expect, it, vi } from 'vitest';

import {
  coordinateExistingAgentLaunch,
  type ExistingAgentLaunchResult,
} from '../src/lib/existing-agent-launch';

describe('existing agent launch coordination', () => {
  it('shares one in-flight launch for the same agent', async () => {
    let resolveLaunch!: (launched: boolean) => void;
    const launch = vi.fn(() => new Promise<boolean>((resolve) => {
      resolveLaunch = resolve;
    }));
    const onFailure = vi.fn();
    const inFlight = new Map<string, Promise<ExistingAgentLaunchResult>>();

    const first = coordinateExistingAgentLaunch(inFlight, 'agent-1', launch, onFailure);
    const second = coordinateExistingAgentLaunch(inFlight, 'agent-1', launch, onFailure);

    expect(second).toBe(first);
    expect(launch).toHaveBeenCalledTimes(1);
    resolveLaunch(true);
    await expect(first).resolves.toEqual({ status: 'launched' });
    expect(inFlight.has('agent-1')).toBe(false);
  });

  it('treats a lease takeover cancellation as silent cancellation', async () => {
    const onFailure = vi.fn();
    const result = await coordinateExistingAgentLaunch(
      new Map(),
      'agent-1',
      async () => false,
      onFailure,
    );

    expect(result).toEqual({ status: 'cancelled' });
    expect(onFailure).not.toHaveBeenCalled();
  });

  it('reports a launch failure once and preserves its underlying error', async () => {
    const error = new Error('provider process exited before PTY initialization');
    const onFailure = vi.fn();
    const result = await coordinateExistingAgentLaunch(
      new Map(),
      'agent-1',
      async () => { throw error; },
      onFailure,
    );

    expect(result).toEqual({ status: 'failed', error });
    expect(onFailure).toHaveBeenCalledOnce();
    expect(onFailure).toHaveBeenCalledWith(error);
  });
});
