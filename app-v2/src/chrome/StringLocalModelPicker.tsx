import { useEffect, useMemo, useRef, useState } from 'react';
import { catalogModel, type LocalModelCatalog } from '../lib/local-auth';
import type { JevSettings } from '../lib/jev';
import { AUTO_ROUTE_MODEL, stringDraftToGroup, stringErrorText, stringGroupToDraft,
  type AutoRouteGroup, type StringRoutingDraft } from '../lib/string-routing';
import { LocalModelPicker, localCatalogChoices, localModelName, type LocalModelChoice } from './LocalModelPicker';
import { StringRoutingEditor } from './StringRoutingEditor';
import { loadProjectAgentDetail } from '../pty-client';
import type { RuleFileTarget } from './CopyRuleFilePath';
import { LocalModelCredit } from './LocalModelCredit';
import type { StringSource } from '../lib/string-agent-targets';

export function StringRoutingSummary({ group, catalog, surface }: { group: AutoRouteGroup; catalog: LocalModelCatalog | null; surface: 'hero' | 'profile' }) {
  return <div className="string-selected-summary" aria-label="Auto String configuration">
    <small>String · {group.models.length} models</small><div className="string-summary-models">{group.models.map((item, index) => {
      const model = catalogModel(catalog, item.providerId, item.modelId);
      return <div key={item.id}><b>{localModelName(item.providerId, model?.name || item.modelId, item.modelId)} <LocalModelCredit providerId={item.providerId} /> · {item.thinkingLevel}</b><small>Model {index + 1}{item.id === group.defaultModel ? ' · Default' : ''}</small></div>;
    })}</div><div className="string-summary-threshold"><span>Probability threshold</span><b>{group.threshold.toFixed(2)}</b>{surface === 'hero' && <small>Copied to new incarnations.</small>}</div>
  </div>;
}

/** Each owner retains its own editor. Only complete groups leave this component. */
export function StringLocalModelPicker({ ownerKey, owner, surface, model, group, catalog, jev, disabled, busy,
  onSelect, onAutoRouteChange, onValidityChange, onSavingChange, onOpen, onJevSettings, ruleFile, ruleFileDirty, applySource }: {
  ownerKey: string; owner: string; surface: 'workbench' | 'hero' | 'profile'; model: string;
  group?: AutoRouteGroup | null; catalog: LocalModelCatalog | null;
  jev: { settings: JevSettings | null; loading: boolean; failed: boolean };
  ruleFile?: RuleFileTarget; ruleFileDirty?: boolean;
  applySource?: StringSource;
  disabled?: boolean; busy?: boolean; onSelect: (choice: LocalModelChoice) => void;
  onAutoRouteChange: (group: AutoRouteGroup) => AutoRouteGroup | void | Promise<AutoRouteGroup | void>;
  onValidityChange?: (valid: boolean) => void; onSavingChange?: (saving: boolean) => void; onOpen: () => void; onJevSettings: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(() => stringGroupToDraft(group));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const savedDraft = useRef(JSON.stringify(stringGroupToDraft(group)));
  const modelTrigger = useRef<HTMLButtonElement>(null);
  const restoreModelFocus = useRef(false);
  useEffect(() => { onSavingChange?.(saving); return () => onSavingChange?.(false); }, [saving, onSavingChange]);
  const latest = useRef({ group, draft, editing, onAutoRouteChange });
  latest.current = { group, draft, editing, onAutoRouteChange };
  const generation = useRef(0), running = useRef<number | null>(null);
  const pending = useRef<AutoRouteGroup | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout>>();
  useEffect(() => {
    const gen = ++generation.current;
    restoreModelFocus.current = false;
    savedDraft.current = JSON.stringify(stringGroupToDraft(latest.current.group));
    setDraft(stringGroupToDraft(latest.current.group)); setEditing(false); setSaving(false); setError(null);
    return () => { if (generation.current === gen) ++generation.current; clearTimeout(timer.current); pending.current = null; };
  }, [ownerKey]);
  useEffect(() => {
    if (!latest.current.editing && !pending.current && running.current !== generation.current) {
      const next = stringGroupToDraft(group);
      savedDraft.current = JSON.stringify(next); setDraft(next);
    }
  }, [group]);
  useEffect(() => {
    if (editing || saving || disabled || !restoreModelFocus.current) return;
    restoreModelFocus.current = false;
    // A pending save disables the trigger during dialog cleanup. Return focus
    // once it is usable, unless the user has already moved to another control.
    if (document.activeElement === document.body) modelTrigger.current?.focus();
  }, [editing, saving, disabled]);
  const choices = useMemo(() => localCatalogChoices(catalog), [catalog]);
  const slash = model.indexOf('/');
  const entry = catalogModel(catalog, model.slice(0, slash), model.slice(slash + 1));
  const selectedName = entry ? localModelName(model.slice(0, slash), entry.name, entry.modelId) : undefined;
  const ready = !!jev.settings?.ready && !jev.failed && !jev.loading;
  const rejected = jev.settings?.keys.find(key => key.providerId === jev.settings!.provider)?.status === 'rejected';

  async function flush() {
    clearTimeout(timer.current);
    const gen = generation.current;
    if (running.current === gen || !pending.current) return;
    running.current = gen;
    try {
      while (pending.current && gen === generation.current) {
        const sent = pending.current; pending.current = null;
        try {
          const saved = await latest.current.onAutoRouteChange(sent);
          if (gen !== generation.current) return;
          setError(null);
          // Preserve card keys/caret and newer edits. Only the still-current
          // submitted draft accepts the backend's native thinking clamp.
          if (saved && JSON.stringify(stringDraftToGroup(latest.current.draft)) === JSON.stringify(sent)) {
            const next = { ...latest.current.draft, cards: latest.current.draft.cards.map((card, index) => ({
              ...card, thinking: saved.models[index]?.thinkingLevel ?? card.thinking,
            })) };
            latest.current.draft = next; setDraft(next);
            savedDraft.current = JSON.stringify(next);
          }
        } catch (failure) { if (gen === generation.current) setError(stringErrorText(failure)); }
      }
    } finally {
      if (running.current === gen) running.current = null;
      if (gen === generation.current) setSaving(false);
    }
  }
  function update(next: StringRoutingDraft) {
    latest.current.draft = next; setDraft(next);
    const complete = stringDraftToGroup(next);
    onValidityChange?.(!!complete);
    clearTimeout(timer.current);
    if (!complete) { pending.current = null; if (running.current !== generation.current) setSaving(false); return; }
    if (surface !== 'workbench') {
      // These owners already have Save Shell / Save; editing only stages data.
      void latest.current.onAutoRouteChange(complete);
      return;
    }
    pending.current = complete; setSaving(true); setError(null);
    // Match the existing short autosave interval without one IPC per keystroke.
    timer.current = setTimeout(() => void flush(), 250);
  }
  function closeEditor() {
    restoreModelFocus.current = true;
    setEditing(false);
    onValidityChange?.(model !== AUTO_ROUTE_MODEL || !!stringDraftToGroup(latest.current.draft));
    if (surface === 'workbench') {
      void flush();
      if (!stringDraftToGroup(latest.current.draft)) setDraft(stringGroupToDraft(latest.current.group));
    }
  }
  async function readRuleFile() {
    if (!ruleFile || surface === 'hero' || !ruleFile.agentId) return false;
    const gen = generation.current;
    const before = JSON.stringify(latest.current.draft);
    try {
      const detail = await loadProjectAgentDetail({ projectRoot: ruleFile.projectRoot, agentId: ruleFile.agentId }, { readOnly: true });
      if (gen !== generation.current) return false;
      if (detail.provider !== 'kota-local') throw { code: 'local_provider_changed' };
      // Only a clean editor takes the saved file. Reading never submits the draft.
      if (!ruleFileDirty && before === savedDraft.current && JSON.stringify(latest.current.draft) === before) {
        const next = stringGroupToDraft(detail.autoRoute);
        savedDraft.current = JSON.stringify(next); latest.current.draft = next; setDraft(next);
      }
      return true;
    } catch (failure) { if (gen === generation.current) setError(stringErrorText(failure)); return false; }
  }
  function openEditor() {
    setEditing(true); setError(null);
    const gen = generation.current;
    const select = () => { if (model !== AUTO_ROUTE_MODEL && stringDraftToGroup(latest.current.draft)) update(latest.current.draft); };
    if (ruleFile && surface !== 'hero') void readRuleFile().then(ok => { if (ok && gen === generation.current && latest.current.editing) select(); });
    else select();
  }
  const summary = group?.models.map(item => {
    const m = catalogModel(catalog, item.providerId, item.modelId);
    return <span className="string-picker-summary-line" key={item.id}>{localModelName(item.providerId, m?.name || item.modelId, item.modelId)} · {item.thinkingLevel}</span>;
  });
  return <>
    <LocalModelPicker triggerRef={modelTrigger} value={model} selectedName={selectedName} choices={choices} disabled={disabled || saving} onOpen={onOpen}
      onSelect={choice => { setDraft(stringGroupToDraft(latest.current.group)); onValidityChange?.(true); setError(null); onSelect(choice); }}
      autoRoute={{ ready, rejected, checking: jev.loading, unavailable: jev.failed, summary: summary?.length ? <>{summary}</> : undefined,
        onEdit: openEditor, onSettings: onJevSettings }} />
    {model === AUTO_ROUTE_MODEL && !ready && <p className="local-model-warning" role="status">{jev.loading ? 'Checking Jev…' : jev.failed ? 'Jev settings unavailable. Retry in Puppeteer.' : rejected ? 'Jev key rejected. Update it in Puppeteer.' : 'Set up Jev to use Auto String'} <button type="button" className="local-model-text" onClick={onJevSettings}>Jev settings</button></p>}
    {model === AUTO_ROUTE_MODEL && surface !== 'profile' && <button type="button" className="local-model-text string-edit-routing" disabled={disabled || !ready || saving}
      onClick={openEditor}>Edit routing</button>}
    {model === AUTO_ROUTE_MODEL && group && surface === 'hero' && <StringRoutingSummary group={group} catalog={catalog} surface={surface} />}
    {error && !editing && <p className="local-model-warning" role="status">{error}</p>}
    {editing && <StringRoutingEditor draft={draft} catalog={catalog} owner={owner} surface={surface} enabled={ready && !disabled} applySource={applySource}
      busy={busy || saving} error={error} ruleFile={ruleFile} ruleFileDirty={ruleFileDirty || saving || JSON.stringify(draft) !== savedDraft.current} onRuleFilePrepared={readRuleFile} onChange={update} onClose={closeEditor} onCatalogOpen={onOpen} returnFocus={modelTrigger} />}
  </>;
}
