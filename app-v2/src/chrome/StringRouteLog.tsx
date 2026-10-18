import type { StringRouteEvent } from '../lib/string-routing';
import { LedLog } from './LedLog';

const TYPE = { selection: 'String Selection', model_error: 'Model Error', jev_error: 'Jev Error', string_error: 'String Error' };
export function stringEventText(event: StringRouteEvent): string {
  if (event.kind === 'selection') {
    const model = `${event.selected.provider}/${event.selected.model}[${event.selected.thinkingLevel}]`;
    return event.outcome === 'selected' ? `${model} selected • Certainty ${event.candidate.probability.toFixed(2)}`
      : `${model} ${event.outcome} • Candidate ${event.candidate.provider}/${event.candidate.model} ${event.candidate.probability.toFixed(2)} ≤ ${event.threshold.toFixed(2)}`;
  }
  const model = 'provider' in event && event.provider && event.model ? `${event.provider}/${event.model} • ` : '';
  return model + event.message;
}
const format = (event: StringRouteEvent) => ({ type: TYPE[event.kind], tone: event.kind, text: stringEventText(event) });
export function StringRouteLog({ events, jevStatus }: { events: StringRouteEvent[]; jevStatus: string }) {
  return <LedLog events={events} format={format} label="Auto String" emptyText="Waiting for an Auto String event…"
    footer={<div className={`string-log-jev${jevStatus === 'Configured' ? ' ready' : ''}`} role="status"><i />Jev · {jevStatus}</div>} />;
}
