import { useId, useMemo, useRef, useState, type ReactNode, type RefObject } from 'react';
import { linkedCatalogProviders, localAuthProvider, localAuthMonogram, type LocalModelCatalog } from '../lib/local-auth';
import { LocalModelDialog } from './LocalModelDialog';
import { localModelOption } from '../lib/local-magpie';
import { LocalModelCredit } from './LocalModelCredit';
import magpieIcon from '../assets/tavern/icons/providers/magpie.svg';
import engine from '../assets/auto-route/auto-string.webp';
import engine2x from '../assets/auto-route/auto-string@2x.webp';
import marionette from '../assets/puppeteer/string-marionette.webp';
import '../styles/local-model-picker.css';

/** A display choice, sourced from the same authenticated catalog for ordinary and String picks. */
export interface LocalModelChoice { providerId: string; modelId: string; name: string }
export const localModelKey = (choice: Pick<LocalModelChoice, 'providerId' | 'modelId'>) => `${choice.providerId}/${choice.modelId}`;
export const localProviderName = (id: string) => localAuthProvider(id)?.name ?? (id === 'magpie' ? 'Magpie' : id);
export function localModelName(providerId: string, name: string, modelId = name) {
  const option = localModelOption(providerId, modelId, name);
  return option.displayLabel ?? option.label;
}
export function localCatalogChoices(catalog: LocalModelCatalog | null): LocalModelChoice[] {
  return linkedCatalogProviders(catalog).filter(provider => provider.providerId !== 'kota').flatMap(provider => provider.models.map(model => ({
    providerId: provider.providerId, modelId: model.modelId, name: localModelName(provider.providerId, model.name, model.modelId),
  })));
}
export function LocalProviderIcon({ providerId }: { providerId: string }) {
  const provider = localAuthProvider(providerId);
  if (providerId === 'magpie') return <img className="local-model-provider-icon" src={magpieIcon} alt="" />;
  return provider?.icon ? <img className="local-model-provider-icon" src={provider.icon} alt="" />
    : <span className="local-model-monogram" aria-hidden="true">{localAuthMonogram(localProviderName(providerId))}</span>;
}
export function AutoRouteIcon() {
  return <img className="local-route-engine" src={engine} srcSet={`${engine} 1x, ${engine2x} 2x`} alt="" />;
}
export function StringIcon() {
  const inkId = useId();
  // The 64px source has transparent margins and translucent strokes. Crop its
  // visible bounds without stretching or resampling the approved artwork.
  return <svg className="local-route-engine" viewBox="19 11 27 43" aria-hidden="true" focusable="false">
    <defs><filter id={inkId} colorInterpolationFilters="sRGB"><feColorMatrix values="0 0 0 0 1  0 0 0 0 1  0 0 0 0 1  0 0 0 3 0" /></filter></defs>
    <image href={marionette} width="64" height="64" filter={`url(#${inkId})`} />
  </svg>;
}

/** Inline, bounded list: never launches a second dialog from a model card. */
export function LocalModelList({ choices, value, onSelect, label = 'Available models', searchLabel = 'Search models', autoRoute, onEscape, initialFocus = true, id }: {
  choices: LocalModelChoice[]; value?: string; onSelect: (choice: LocalModelChoice) => void;
  label?: string; searchLabel?: string; autoRoute?: ReactNode; onEscape?: () => void; initialFocus?: boolean; id?: string;
}) {
  const generatedId = useId();
  const listId = id ?? generatedId;
  const [query, setQuery] = useState('');
  const list = useRef<HTMLDivElement>(null);
  const groups = useMemo(() => {
    const term = query.trim().toLowerCase();
    const grouped = new Map<string, LocalModelChoice[]>();
    for (const choice of choices) {
      if (!`${localProviderName(choice.providerId)} ${choice.name} ${choice.modelId}`.toLowerCase().includes(term)) continue;
      grouped.set(choice.providerId, [...(grouped.get(choice.providerId) ?? []), choice]);
    }
    return [...grouped.entries()];
  }, [choices, query]);
  return <div className="local-model-list" onKeyDown={event => {
    if (event.key === 'Escape' && onEscape) { event.preventDefault(); event.stopPropagation(); onEscape(); return; }
    if (!['ArrowDown', 'ArrowUp'].includes(event.key)) return;
    const options = [...(list.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
    if (!options.length) return;
    event.preventDefault();
    const current = options.indexOf(document.activeElement as HTMLButtonElement);
    const next = current < 0 ? event.key === 'ArrowDown' ? 0 : options.length - 1
      : (current + (event.key === 'ArrowDown' ? 1 : -1) + options.length) % options.length;
    options[next].focus();
  }}>
    <input type="search" aria-label={searchLabel} aria-controls={listId} placeholder="Search models" value={query}
      data-initial-focus={initialFocus ? '' : undefined} autoFocus={initialFocus} onChange={event => setQuery(event.target.value)} />
    <div ref={list} id={listId} className="local-model-results" role="listbox" aria-label={label}>
      {autoRoute && 'auto string'.includes(query.trim().toLowerCase()) ? autoRoute : null}
      {groups.map(([providerId, models]) => <div role="group" aria-label={localProviderName(providerId)} key={providerId}>
        <div className="local-model-group-label">{localProviderName(providerId)}</div>
        {models.map(choice => <button type="button" role="option" className="local-model-option" key={localModelKey(choice)}
          aria-selected={value === localModelKey(choice)} onClick={() => onSelect(choice)}>
          <LocalProviderIcon providerId={choice.providerId} /><span><strong>{choice.name} <LocalModelCredit providerId={choice.providerId} /></strong><small>{localProviderName(choice.providerId)}</small></span>
        </button>)}
      </div>)}
      {!groups.length && !(autoRoute && 'auto string'.includes(query.trim().toLowerCase())) && <p className="local-model-empty">No matching models.</p>}
    </div>
  </div>;
}

export function LocalModelPicker({ value, selectedName, choices, disabled, label = 'Local model', onSelect, onOpen, autoRoute, triggerRef }: {
  value: string; selectedName?: string; choices: LocalModelChoice[]; disabled?: boolean; label?: string;
  onSelect: (choice: LocalModelChoice) => void; onOpen?: () => void;
  triggerRef?: RefObject<HTMLButtonElement>;
  autoRoute?: { ready: boolean; rejected?: boolean; checking?: boolean; unavailable?: boolean; summary?: ReactNode; note?: string; onEdit: () => void; onSettings: () => void };
}) {
  const [open, setOpen] = useState(false);
  const slash = value.indexOf('/');
  const selectedProvider = slash < 0 ? '' : value.slice(0, slash);
  const automatic = value === 'kota/auto';
  const choice = choices.find(item => localModelKey(item) === value);
  return <div className="local-model-picker">
    <button ref={triggerRef} type="button" className="local-model-trigger" aria-label={label} aria-haspopup="dialog" disabled={disabled}
      onClick={() => { setOpen(true); onOpen?.(); }}>
      {automatic ? <StringIcon /> : selectedProvider ? <LocalProviderIcon providerId={selectedProvider} /> : null}
      <span>{automatic ? 'Auto String' : selectedName || choice?.name || value || 'Choose a model'}{!automatic && <LocalModelCredit providerId={selectedProvider} />}</span><i aria-hidden="true">⌄</i>
    </button>
    {open && <LocalModelDialog title="Choose a Local model" subtitle="Choose Auto String or a linked model." onClose={() => setOpen(false)}
      footer={autoRoute ? <><p className={!autoRoute.ready && !autoRoute.checking && !autoRoute.unavailable ? 'local-model-warning' : ''}>{autoRoute.checking ? 'Checking Jev…' : autoRoute.unavailable ? 'Jev settings unavailable. Retry in Puppeteer.' : autoRoute.ready ? autoRoute.note ?? 'For this agent only.' : autoRoute.rejected ? 'Jev key rejected. Update it in Puppeteer.' : 'Set up Jev to use Auto String'}</p><button type="button" className="local-model-text" onClick={() => { setOpen(false); autoRoute.onSettings(); }}>Jev settings</button></> : undefined}>
      <LocalModelList choices={choices} value={value} onSelect={next => { setOpen(false); onSelect(next); }}
        autoRoute={autoRoute ? <div role="group" aria-label="Kota Local"><div className="local-model-group-label">Kota Local</div>
          <button type="button" className="local-model-option" role="option" disabled={!autoRoute.ready} aria-selected={automatic}
            onClick={() => { setOpen(false); autoRoute.onEdit(); }}><StringIcon /><span><strong>Auto String</strong><small>{autoRoute.summary ?? 'String · Decisions by Jev'}</small></span><small>{autoRoute.checking ? 'Checking…' : autoRoute.unavailable ? 'Unavailable' : autoRoute.ready ? 'Edit routing' : autoRoute.rejected ? 'Update key' : 'Set up Jev'}</small></button>
        </div> : undefined} />
    </LocalModelDialog>}
  </div>;
}
