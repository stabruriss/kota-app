import type { ReactNode } from 'react';
import { LedLog } from './LedLog';
import { danceErrorText } from '../lib/dance-settings';
import { useDanceEvents, type DanceEvent } from '../lib/dance-events';

const REASON = { empty_text: 'Empty message', no_candidates: 'No candidates', target_left: 'Target left', only_seat: 'Only seat', only_candidate: 'Only candidate' };
export function danceEventText(event: DanceEvent): string {
  if (event.kind !== 'selection') return danceErrorText(event.code);
  const target = event.target.name;
  if (event.outcome === 'selected' && event.candidate) return `${target} selected • Certainty ${event.candidate.probability.toFixed(2)}`;
  if (event.reason === 'below_threshold' && event.candidate) return `${target} sentinel • Candidate ${event.candidate.name} ${event.candidate.probability.toFixed(2)} ≤ ${event.threshold.toFixed(2)}`;
  return `${target} ${event.outcome}${event.reason && event.reason !== 'below_threshold' ? ` • ${REASON[event.reason]}` : ''}`;
}
const format = (event: DanceEvent) => ({ type: event.kind === 'selection' ? 'Dance Selection' : event.kind === 'jev_error' ? 'Jev Error' : 'Dance Error',
  tone: event.kind === 'dance_error' ? 'string_error' : event.kind, text: danceEventText(event) });
export function DanceRouteLog({ projectRoot, footer }: { projectRoot?: string | null; footer: ReactNode }) {
  const log = useDanceEvents(projectRoot);
  return <div className="dance-route-log"><LedLog events={log.events} format={format} label="Dance"
    emptyText={log.loading ? 'Loading Dance events…' : log.failed ? 'Dance log unavailable.' : 'Waiting for a Dance decision…'}
    footer={<>{footer}{log.failed && <button type="button" className="local-model-text" onClick={log.retry}>Retry log</button>}</>} /></div>;
}
