export type ExistingAgentLaunchResult =
  | { status: 'launched' }
  | { status: 'cancelled' }
  | { status: 'failed'; error: unknown };

export function coordinateExistingAgentLaunch(
  inFlight: Map<string, Promise<ExistingAgentLaunchResult>>,
  agentId: string,
  launch: () => Promise<boolean>,
  onFailure: (error: unknown) => void,
): Promise<ExistingAgentLaunchResult> {
  const existing = inFlight.get(agentId);
  if (existing) return existing;

  const pending = (async (): Promise<ExistingAgentLaunchResult> => {
    try {
      return await launch()
        ? { status: 'launched' }
        : { status: 'cancelled' };
    } catch (error) {
      onFailure(error);
      return { status: 'failed', error };
    }
  })();
  inFlight.set(agentId, pending);
  void pending.then(
    () => {
      if (inFlight.get(agentId) === pending) inFlight.delete(agentId);
    },
    () => {
      if (inFlight.get(agentId) === pending) inFlight.delete(agentId);
    },
  );
  return pending;
}
