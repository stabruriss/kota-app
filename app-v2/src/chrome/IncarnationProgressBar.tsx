export type IncarnationProgressStepId =
  | 'profile'
  | 'source'
  | 'worktree'
  | 'config'
  | 'skills'
  | 'launch';

export type IncarnationProgressPhase = 'running' | 'success' | 'error';

export interface IncarnationProgressView {
  id: string;
  heroName: string;
  projectName: string;
  stepId: IncarnationProgressStepId;
  message: string;
  phase: IncarnationProgressPhase;
  errorMessage?: string;
  copied?: boolean;
}

export interface IncarnationProgressStep {
  id: IncarnationProgressStepId;
  label: string;
}

export const INCARNATION_PROGRESS_STEPS: readonly IncarnationProgressStep[] = [
  { id: 'profile', label: 'Profile' },
  { id: 'source', label: 'Source' },
  { id: 'worktree', label: 'Worktree' },
  { id: 'config', label: 'Config' },
  { id: 'skills', label: 'Skills' },
  { id: 'launch', label: 'Launch' },
];

export function normalizeIncarnationProgressStep(step: string): IncarnationProgressStepId {
  if (
    step === 'profile' ||
    step === 'source' ||
    step === 'worktree' ||
    step === 'config' ||
    step === 'skills' ||
    step === 'launch'
  ) {
    return step;
  }
  return 'profile';
}

export function IncarnationProgressBar({
  progress,
  onRetry,
  onDismiss,
  onCopyError,
}: {
  progress: IncarnationProgressView;
  onRetry: () => void;
  onDismiss: () => void;
  onCopyError: () => void;
}) {
  const currentIndex = Math.max(
    0,
    INCARNATION_PROGRESS_STEPS.findIndex((step) => step.id === progress.stepId),
  );
  const percent =
    progress.phase === 'success'
      ? 100
      : Math.max(8, Math.round((currentIndex / (INCARNATION_PROGRESS_STEPS.length - 1)) * 100));

  return (
    <div className="incarnation-progress-layer" role="status" aria-live="polite">
      <section className={`incarnation-progress-card ${progress.phase}`}>
        <header className="incarnation-progress-head">
          <div className="incarnation-progress-orbit" aria-hidden>
            <span />
            <span />
            <span />
          </div>
          <div className="incarnation-progress-title">
            <b>{progress.heroName}</b>
            <span>Incarnation to</span>
            <b>{progress.projectName}</b>
          </div>
        </header>

        <div className="incarnation-progress-track" aria-hidden>
          <div className="incarnation-progress-fill" style={{ width: `${percent}%` }} />
        </div>

        <div className="incarnation-progress-steps" aria-hidden>
          {INCARNATION_PROGRESS_STEPS.map((step, index) => {
            const done = progress.phase === 'success' || index < currentIndex;
            const active = progress.phase !== 'success' && index === currentIndex;
            const failed = progress.phase === 'error' && index === currentIndex;
            return (
              <span
                key={step.id}
                className={`${done ? 'done' : ''} ${active ? 'active' : ''} ${failed ? 'failed' : ''}`}
              >
                {step.label}
              </span>
            );
          })}
        </div>

        <div className="incarnation-progress-status">
          <div className="incarnation-progress-status-kicker">
            {progress.phase === 'error'
              ? 'Stopped'
              : progress.phase === 'success'
                ? 'Ready'
                : INCARNATION_PROGRESS_STEPS[currentIndex]?.label ?? 'Working'}
          </div>
          <div className="incarnation-progress-status-text">
            {progress.phase === 'error'
              ? progress.errorMessage || progress.message
              : progress.message}
          </div>
        </div>

        {progress.phase === 'error' && (
          <div className="incarnation-progress-actions">
            <button type="button" onClick={onRetry}>
              Retry
            </button>
            <button type="button" onClick={onCopyError}>
              {progress.copied ? 'Copied' : 'Copy error'}
            </button>
            <button type="button" className="ghost" onClick={onDismiss}>
              Dismiss
            </button>
          </div>
        )}
      </section>
    </div>
  );
}
