import { useCallback, useEffect, useRef, useState } from 'react';
import type { AutoRouteGroup } from '../lib/string-routing';
import type { ProjectAgentNameFields } from './ProjectAgentName';
import '../styles/profile-sheet.css';

/** Whether two sets of name fields are the same once trimmed. */
export function profileNameFieldsEqual(
  left: ProjectAgentNameFields | null | undefined,
  right: ProjectAgentNameFields | null | undefined,
): boolean {
  return (
    (left?.titleId ?? null) === (right?.titleId ?? null) &&
    (left?.given ?? '').trim() === (right?.given ?? '').trim() &&
    (left?.middle ?? '').trim() === (right?.middle ?? '').trim() &&
    (left?.surname ?? '').trim() === (right?.surname ?? '').trim()
  );
}

/**
 * Whether two Auto String groups route the same way. The editor renumbers
 * model ids and writes its keys in its own order, so neither counts.
 */
export function sameAutoRoute(left: AutoRouteGroup | null | undefined, right: AutoRouteGroup | null | undefined): boolean {
  if (!left || !right) return !left && !right;
  const shape = (group: AutoRouteGroup) => JSON.stringify([
    group.threshold,
    group.models.findIndex((item) => item.id === group.defaultModel),
    group.models.map((item) => [item.providerId, item.modelId, item.thinkingLevel, item.intent]),
  ]);
  return shape(left) === shape(right);
}

/** "name", "name and GHOST", "name, SHELL and GHOST". */
export function listProfileSections(sections: readonly string[]): string {
  if (sections.length <= 1) return sections.join('');
  return `${sections.slice(0, -1).join(', ')} and ${sections[sections.length - 1]}`;
}

/** Text that shows for a while and then fades. It stays in the DOM while it fades out. */
export function useFadingText(ms: number): [{ text: string; show: boolean }, (text: string) => void] {
  const [state, setState] = useState({ text: '', show: false });
  const timer = useRef<number>();
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const show = useCallback((text: string) => {
    window.clearTimeout(timer.current);
    setState({ text, show: !!text });
    if (text) timer.current = window.setTimeout(() => setState((current) => ({ ...current, show: false })), ms);
  }, [ms]);
  return [state, show];
}

/**
 * Asks before unsaved profile edits are dropped. `leave` is Back from the edit
 * state; `discard` is Cancel. The profile overlay owns Escape and maps it to
 * `onKeep`.
 */
export function ProfileUnsavedDialog({ kind, sections, onKeep, onDiscard, onSave }: {
  kind: 'leave' | 'discard';
  sections: readonly string[];
  onKeep: () => void;
  onDiscard: () => void;
  onSave: () => void;
}) {
  const title = kind === 'leave' ? 'Save changes before leaving?' : 'Discard changes?';
  const changed = listProfileSections(sections);
  return (
    <div className="kota-confirm-layer profile-unsaved-layer" role="presentation" onMouseDown={onKeep}>
      <section
        className="kota-confirm-card plain-copy"
        role="alertdialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <h2>{title}</h2>
        <pre>{kind === 'leave' ? `You have unsaved changes to ${changed}.` : `Your changes to ${changed} will be lost.`}</pre>
        <div className="kota-confirm-actions">
          <button type="button" className="profile-unsaved-discard" onClick={onDiscard}>Discard Changes</button>
          <button type="button" autoFocus onClick={onKeep}>Keep Editing</button>
          {kind === 'leave' && <button type="button" className="confirm" onClick={onSave}>Save and Leave</button>}
        </div>
      </section>
    </div>
  );
}
