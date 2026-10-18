import { useId, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type RefObject, type ReactNode } from 'react';
import { catalogModel, localThinkingForModel, type LocalModelCatalog } from '../lib/local-auth';
import { StringIcon, LocalModelList, LocalProviderIcon, localCatalogChoices, localModelKey, localModelName, localProviderName, type LocalModelChoice } from './LocalModelPicker';
import { LocalModelDialog } from './LocalModelDialog';
import { CopyRuleFilePath, type RuleFileTarget } from './CopyRuleFilePath';
import { LocalModelCredit } from './LocalModelCredit';
import { StringTemplates } from './StringTemplates';
import { StringApplyDialog } from './StringApplyDialog';
import type { StringSource } from '../lib/string-agent-targets';
import type { ProjectAgentDetail } from '../pty-client';
import { newStringRoutingDraft, stringDraftComplete, stringDraftIssue, stringDraftToGroup, stringGroupToDraft, STRING_DEFAULT_INTENTS, STRING_MAX_MODELS,
  type StringModelCard, type StringRoutingDraft } from '../lib/string-routing';
import '../styles/string-routing.css';

export { newStringRoutingDraft, stringDraftComplete, STRING_DEFAULT_INTENTS };
export type { StringModelCard, StringRoutingDraft };

function IntentInput({ value, label, onChange }: { value: string; label: string; onChange: (value: string) => void }) {
  const input = useRef<HTMLTextAreaElement>(null);
  const grow = (element: HTMLTextAreaElement) => {
    element.style.height = 'auto';
    element.style.height = `${element.scrollHeight + 2}px`;
  };
  useLayoutEffect(() => {
    if (input.current) grow(input.current);
  }, [value]);
  useLayoutEffect(() => {
    const element = input.current;
    if (!element) return;
    let previousWidth = element.clientWidth;
    const observer = new ResizeObserver(entries => {
      const width = entries[0]?.contentRect.width;
      if (width !== previousWidth) { previousWidth = width; grow(element); }
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return <textarea ref={input} rows={2} aria-label={label} placeholder="When should this model be used?" value={value}
    onChange={event => onChange([...event.target.value].slice(0, 200).join(''))} />;
}

export function StringRoutingEditor({ draft, catalog, owner, surface, enabled = true, busy = false, error, onChange, onClose, onCatalogOpen, returnFocus, onConfirm, extraAction, ruleFile, ruleFileDirty, onRuleFilePrepared, applySource, onApplied }: {
  draft: StringRoutingDraft; catalog: LocalModelCatalog | null; owner: string; surface: 'workbench' | 'hero' | 'profile' | 'tavern';
  enabled?: boolean; busy?: boolean; error?: string | null; onChange: (draft: StringRoutingDraft) => void;
  onClose: () => void; onCatalogOpen?: () => void;
  returnFocus?: RefObject<HTMLElement>;
  onConfirm?: () => void; extraAction?: ReactNode;
  ruleFile?: RuleFileTarget; ruleFileDirty?: boolean; onRuleFilePrepared?: () => void | Promise<unknown>;
  applySource?: StringSource; onApplied?: (root: string, detail: ProjectAgentDetail) => void;
}) {
  const prefix = useId();
  const [expanded, setExpanded] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [templates, setTemplates] = useState<'save' | 'import' | null>(null);
  const [templateNotice, setTemplateNotice] = useState<string | null>(null);
  const [applying, setApplying] = useState(false);
  const buttons = useRef(new Map<string, HTMLButtonElement>());
  const choices = useMemo(() => localCatalogChoices(catalog), [catalog]);
  const complete = stringDraftComplete(draft);
  const group = stringDraftToGroup(draft);
  const locked = busy && surface !== 'workbench';
  const issue = stringDraftIssue(draft);
  const updateCard = (key: string, patch: Partial<StringModelCard>) => onChange({ ...draft, cards: draft.cards.map(card => card.key === key ? { ...card, ...patch } : card) });
  const closeList = (key: string) => { setExpanded(null); buttons.current.get(key)?.focus(); };
  const choose = (card: StringModelCard, choice: LocalModelChoice) => {
    const levels = catalogModel(catalog, choice.providerId, choice.modelId)?.thinkingLevels ?? [];
    if (!levels.length) return;
    const thinking = localThinkingForModel(levels, card.thinking);
    setNotice(card.thinking && card.thinking !== thinking ? `Thinking adjusted to ${thinking} for this model.` : null);
    updateCard(card.key, { model: { providerId: choice.providerId, modelId: choice.modelId }, thinking });
    closeList(card.key);
  };
  const add = () => {
    if (draft.cards.length >= STRING_MAX_MODELS) return;
    const key = crypto.randomUUID();
    onChange({ ...draft, cards: [...draft.cards, { key, model: null, thinking: null, intent: '' }] });
    setExpanded(null);
  };
  const remove = (key: string) => {
    if (draft.cards.length <= 2) return;
    const cards = draft.cards.filter(card => card.key !== key);
    if (expanded === key) setExpanded(null);
    onChange({ ...draft, cards, defaultKey: draft.defaultKey === key ? cards[0].key : draft.defaultKey });
  };
  return <><LocalModelDialog title="Auto String" art={<StringIcon />} returnFocus={returnFocus}
    titleAction={surface !== 'hero' && ruleFile && <CopyRuleFilePath target={ruleFile} dirty={ruleFileDirty} disabled={busy} onPrepared={onRuleFilePrepared} />}
    subtitle={surface === 'hero' ? 'For this Hero template. Copied to new incarnations. · Decisions by Jev' : `For ${owner} only. · Decisions by Jev`}
    onClose={onClose} onEscape={() => expanded ? closeList(expanded) : onClose()} footer={<><div className="string-editor-status"><p className={error || !complete ? 'local-model-warning' : ''} role="status">
      {error || issue || (busy ? 'Saving…' : surface === 'workbench' ? 'Saved automatically.'
        : surface === 'tavern' && ruleFileDirty === false ? 'No changes.' : 'Unsaved changes')}
    </p><small>{surface === 'workbench' ? 'Changes apply to the next message.' : surface === 'tavern' ? 'Click OK to save.' : `Click OK, then ${surface === 'hero' ? 'Save Shell' : 'Save'}.`}</small></div>
    {extraAction}<div className="template-editor-actions">
      {templateNotice && <span className="template-toast" role="status">{templateNotice}</span>}
      <button type="button" className="local-model-text" disabled={locked} onClick={event => { event.currentTarget.focus(); setTemplateNotice(null); setTemplates('import'); }}>Import</button>
      <button type="button" className="local-model-text" disabled={!complete || locked} onClick={event => { event.currentTarget.focus(); setTemplateNotice(null); setTemplates('save'); }}>Save as template</button>
      <button type="button" className="local-model-text" disabled={!complete || !enabled || locked} onClick={event => { event.currentTarget.focus(); setTemplateNotice(null); setApplying(true); }}>Apply to</button>
    </div><button type="button" className="string-ok" disabled={!complete || !enabled || locked} onClick={onConfirm ?? onClose}>OK</button></>}>
    <div className="string-routing-body"><fieldset disabled={!enabled || (busy && surface !== 'workbench')} aria-label="String routing settings">
      <div className="string-threshold"><div><label htmlFor={`${prefix}-threshold`}>The certainty must be higher than this to switch model.</label><output htmlFor={`${prefix}-threshold`}>{draft.threshold.toFixed(2)}</output></div>
        <input id={`${prefix}-threshold`} type="range" aria-label="String probability threshold" min="0.05" max="0.95" step="0.05" value={draft.threshold}
          style={{ '--string-threshold-fill': `${(draft.threshold - .05) / .9 * 100}%` } as CSSProperties}
          onChange={event => onChange({ ...draft, threshold: Number(event.target.value) })} /></div>
      <div className="string-models-head"><h3>Models <small>{draft.cards.length} / {STRING_MAX_MODELS}</small></h3><button type="button" className="string-add" disabled={draft.cards.length >= STRING_MAX_MODELS} onClick={add}>+ Add model</button></div>
      <div className="string-model-cards">{draft.cards.map((card, index) => {
        const entry = card.model ? catalogModel(catalog, card.model.providerId, card.model.modelId) : null;
        const linked = !!card.model && choices.some(choice => localModelKey(choice) === localModelKey(card.model!));
        const picking = expanded === card.key;
        const listId = `${prefix}-models-${index}`;
        const name = card.model ? localModelName(card.model.providerId, entry?.name || card.model.modelId, card.model.modelId) : 'Choose a model';
        return <section key={card.key} className={`string-model-card${card.key === draft.defaultKey ? ' is-default' : ''}${picking ? ' is-picking' : ''}`} aria-label={`Model ${index + 1}`}>
          <header><h3>Model {index + 1}</h3><label className="string-default"><input type="radio" name={`${prefix}-default`} checked={card.key === draft.defaultKey} aria-label={`Default model ${index + 1}`}
            onChange={() => onChange({ ...draft, defaultKey: card.key })} />Default</label></header>
          <div className="string-model-fields"><div><span>Model</span><button type="button" ref={button => { if (button) buttons.current.set(card.key, button); else buttons.current.delete(card.key); }}
            className="local-model-trigger" title={card.model ? name : undefined} aria-label={`Model ${index + 1} model`} aria-haspopup="listbox" aria-expanded={picking} aria-controls={picking ? listId : undefined}
            onClick={() => { setExpanded(picking ? null : card.key); if (!picking) onCatalogOpen?.(); }}>
            {card.model && <LocalProviderIcon providerId={card.model.providerId} />}<span>{name}{card.model && <LocalModelCredit providerId={card.model.providerId} />}</span><i aria-hidden="true">{picking ? '⌃' : '⌄'}</i>
          </button></div><label><span>Thinking</span><select aria-label={`Model ${index + 1} thinking`} value={card.thinking ?? ''} disabled={!entry?.thinkingLevels.length}
            onChange={event => { if (entry?.thinkingLevels.includes(event.target.value)) updateCard(card.key, { thinking: event.target.value }); }}>
            {!entry?.thinkingLevels.includes(card.thinking ?? '') && <option value={card.thinking ?? ''}>{card.thinking ?? 'Choose a model'}</option>}
            {entry?.thinkingLevels.map(level => <option key={level} value={level}>{level}</option>)}
          </select></label></div>
          {picking && <div className="string-inline-models"><LocalModelList id={listId} choices={choices} value={card.model ? localModelKey(card.model) : undefined}
            label={`Available models for Model ${index + 1}`} searchLabel={`Search models for Model ${index + 1}`} onSelect={choice => choose(card, choice)} onEscape={() => closeList(card.key)} /></div>}
          <small className={`string-provider-note${card.model && !linked ? ' local-model-warning' : ''}`}>{card.model ? `${localProviderName(card.model.providerId)} · ${linked ? 'Linked' : 'Not linked'}` : 'Only linked providers are available.'}</small>
          <label className="string-intent"><span>Intent description <small>{[...card.intent].length} / 200</small></span><IntentInput label={`Model ${index + 1} intent`} value={card.intent}
            onChange={intent => updateCard(card.key, { intent })} /></label>
          <footer><button type="button" className="local-model-text string-remove" disabled={draft.cards.length <= 2} onClick={() => remove(card.key)}>Remove</button>
            <small>{draft.cards.length <= 2 ? 'Keep at least 2 models.' : card.key === draft.defaultKey ? 'Removing this model makes the first remaining model default.' : ''}</small></footer>
        </section>;
      })}</div>{notice && <p className="string-adjustment" role="status">{notice}</p>}
    </fieldset></div>
  </LocalModelDialog>
    {templates && <StringTemplates mode={templates} group={group} catalog={catalog} onClose={() => setTemplates(null)}
      onSaved={name => setTemplateNotice(`Saved “${name}”.`)} onImport={(imported, name) => {
        setExpanded(null); onChange(stringGroupToDraft(imported)); setTemplateNotice(`Imported “${name}”.`);
      }} />}
    {applying && group && <StringApplyDialog group={group} sourceName={surface === 'hero' ? `Hero ${owner}` : owner}
      exclude={surface === 'hero' ? undefined : applySource} ready={enabled} onClose={() => setApplying(false)} onApplied={onApplied} />}
  </>;
}
