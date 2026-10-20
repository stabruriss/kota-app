import type { EmberDreamParticipant, EmberDreamRunStatus } from '../pty-client';

const IDLE_CHECK_MS = 120_000;
const DEADLINE_MS = 300_000;
const POLL_MS = 1_000;

/** Watch only successful deliveries. A separate deadline survives a stalled IPC. */
export function watchDreamSubmissions(options: {
  startedAt: number;
  participants: readonly EmberDreamParticipant[];
  readStatus: () => Promise<EmberDreamRunStatus[]>;
  onReady: () => void;
}): () => void {
  let stopped = false;
  let waitForDeadline = false;
  let poll: ReturnType<typeof setTimeout> | undefined;
  const finish = () => {
    if (stopped) return;
    stopped = true;
    clearTimeout(deadline);
    clearTimeout(poll);
    options.onReady();
  };
  const deadline = setTimeout(finish, Math.max(0, options.startedAt + DEADLINE_MS - Date.now()));
  const check = async () => {
    try {
      const statuses = await options.readStatus();
      if (stopped) return;
      const pending = options.participants.map((participant) => statuses.find((status) => (
        status.agentId === participant.agentId && status.projectRoot === participant.projectRoot
      ))).filter((status) => !status?.submitted);
      if (options.participants.length > 0 && pending.length === 0) {
        finish();
        return;
      }
      if (!waitForDeadline && Date.now() >= options.startedAt + IDLE_CHECK_MS) {
        // Missing status is unknown, not evidence that an agent is idle.
        waitForDeadline = pending.some((status) => !status || status.working);
        if (!waitForDeadline) {
          finish();
          return;
        }
      }
    } catch {
      // An unavailable inbox must not turn into a false "all submitted" result.
      if (Date.now() >= options.startedAt + IDLE_CHECK_MS) waitForDeadline = true;
    }
    if (!stopped) poll = setTimeout(() => { void check(); }, POLL_MS);
  };
  void check();
  return () => {
    stopped = true;
    clearTimeout(poll);
    clearTimeout(deadline);
  };
}
