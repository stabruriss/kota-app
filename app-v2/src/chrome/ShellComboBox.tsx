/*  ShellComboBox — shared free-text + dropdown combo for SHELL model/effort
 *  editing. Extracted verbatim from ProjectAgentProfileOverlay so the Tavern
 *  hero profile and the project-agent profile share one behavior surface
 *  (fuzzy match, keyboard, aria, source pills). Styling intentionally keeps
 *  the existing .project-agent-combo* classes from canvas.css.               */
import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';

export type ShellComboOption = {
  id: string;
  label: string;
  source?: string;
};

const SHELL_COMBO_LIMIT = 12;

export function ShellComboBox({
  value,
  options,
  placeholder,
  disabled,
  status,
  refreshing,
  onChange,
  onRefresh,
}: {
  value: string;
  options: ShellComboOption[];
  placeholder: string;
  disabled?: boolean;
  status?: string;
  refreshing?: boolean;
  onChange: (value: string) => void;
  onRefresh?: () => void;
}) {
  const reactId = useId();
  const listboxId = `${reactId}-listbox`;
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [open, setOpen] = useState(false);
  const [activeIndex, setActiveIndex] = useState(0);
  const matches = useMemo(() => shellComboMatches(value, options), [options, value]);
  const clampedActiveIndex = Math.min(activeIndex, Math.max(matches.length - 1, 0));
  const activeOption = matches[clampedActiveIndex];

  useEffect(() => {
    if (!open) return undefined;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (target instanceof Node && rootRef.current?.contains(target)) return;
      setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    return () => document.removeEventListener('pointerdown', onPointerDown, true);
  }, [open]);

  useEffect(() => {
    setActiveIndex(0);
  }, [matches.length, value]);

  const chooseOption = (option: ShellComboOption) => {
    onChange(option.id);
    setOpen(false);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setOpen(true);
      setActiveIndex((index) => Math.min(index + 1, Math.max(matches.length - 1, 0)));
      return;
    }
    if (event.key === 'ArrowUp') {
      event.preventDefault();
      setOpen(true);
      setActiveIndex((index) => Math.max(index - 1, 0));
      return;
    }
    if (event.key === 'Enter' && open && activeOption) {
      event.preventDefault();
      chooseOption(activeOption);
      return;
    }
    if (event.key === 'Escape') {
      if (open) {
        event.preventDefault();
        event.stopPropagation();
      }
      setOpen(false);
    }
  };

  return (
    <div
      className={`project-agent-combo ${onRefresh ? 'has-refresh' : ''}`}
      data-open={open ? 'true' : undefined}
      ref={rootRef}
    >
      <div className="project-agent-combo-input-row">
        <input
          value={value}
          placeholder={placeholder}
          disabled={disabled}
          role="combobox"
          aria-expanded={open}
          aria-autocomplete="list"
          aria-controls={open && matches.length > 0 ? listboxId : undefined}
          aria-activedescendant={open && activeOption ? `${listboxId}-option-${clampedActiveIndex}` : undefined}
          onFocus={() => setOpen(true)}
          onChange={(event) => {
            onChange(event.currentTarget.value);
            setOpen(true);
          }}
          onKeyDown={onKeyDown}
        />
        {open && matches.length > 0 && (
          <div className="project-agent-combo-menu" id={listboxId} role="listbox">
            {matches.map((option, index) => (
              <button
                type="button"
                key={`${option.id}:${option.source ?? ''}`}
                id={`${listboxId}-option-${index}`}
                className={`project-agent-combo-option ${index === clampedActiveIndex ? 'active' : ''}`}
                onMouseDown={(event) => event.preventDefault()}
                onMouseEnter={() => setActiveIndex(index)}
                onClick={() => chooseOption(option)}
                role="option"
                aria-selected={index === clampedActiveIndex}
                title={option.id}
              >
                <span className="project-agent-combo-option-main">
                  <b>{option.id}</b>
                </span>
                {option.source && (
                  <span className="project-agent-combo-source" title={shellComboSourceLabel(option.source)}>
                    {shellComboSourceLabel(option.source)}
                  </span>
                )}
              </button>
            ))}
          </div>
        )}
      </div>
      {(status || onRefresh) && (
        <div className="project-agent-combo-status-row">
          {status && <span className="project-agent-combo-status">{status}</span>}
          {onRefresh && (
            <button
              type="button"
              className="project-agent-combo-refresh"
              disabled={disabled || refreshing}
              onClick={onRefresh}
              data-refreshing={refreshing ? 'true' : undefined}
              aria-label="Update model list"
              title={refreshing ? 'Updating model list' : 'Update model list'}
            >
              ↻
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export function uniqueShellComboOptions(
  values: Array<ShellComboOption | null | undefined>,
): ShellComboOption[] {
  const options = new Map<string, ShellComboOption>();
  for (const option of values) {
    const id = option?.id.trim() ?? '';
    if (!id || options.has(id)) continue;
    options.set(id, {
      id,
      label: option?.label?.trim() || id,
      source: option?.source?.trim() || undefined,
    });
  }
  return Array.from(options.values());
}

function shellComboSourceLabel(source: string): string {
  const normalized = source.trim().toLowerCase();
  if (!normalized) return 'Kota';
  if (normalized === 'current' || normalized === 'manual' || normalized === 'typed' || normalized === 'selected') {
    return 'Manual';
  }
  if (normalized === 'models.dev' || normalized === 'models-dev' || normalized === 'modelsdev') {
    return 'Models.dev';
  }
  if (
    normalized === 'cli' ||
    normalized === 'provider' ||
    normalized.includes(' --') ||
    normalized.endsWith(' models')
  ) {
    return 'Provider';
  }
  return 'Kota';
}

function shellComboMatches(query: string, options: ShellComboOption[]): ShellComboOption[] {
  const trimmed = query.trim();
  if (!trimmed) return options.slice(0, SHELL_COMBO_LIMIT);
  return options
    .map((option, index) => {
      const score = shellComboFuzzyScore(trimmed, `${option.id} ${option.label}`);
      return score == null ? null : { option, score, index };
    })
    .filter((item): item is { option: ShellComboOption; score: number; index: number } => !!item)
    .sort((left, right) => right.score - left.score || left.index - right.index)
    .slice(0, SHELL_COMBO_LIMIT)
    .map((item) => item.option);
}

function shellComboFuzzyScore(query: string, text: string): number | null {
  const normalizedQuery = query.toLowerCase();
  const normalizedText = text.toLowerCase();
  const positions = shellComboSubsequencePositions(normalizedQuery, normalizedText);
  if (!positions) return null;
  const ordered = Array.from(positions);
  let score = 100;
  let previous = -1;
  for (const position of ordered) {
    if (previous >= 0 && position === previous + 1) score += 8;
    const before = position === 0 ? '' : normalizedText[position - 1];
    if (position === 0 || before === '/' || before === '-' || before === '_' || before === ' ') score += 6;
    if (previous >= 0) score -= Math.max(0, position - previous - 1);
    previous = position;
  }
  if (normalizedText.startsWith(normalizedQuery)) score += 50;
  else if (normalizedText.includes(normalizedQuery)) score += 30;
  return score - normalizedText.length * 0.04;
}

function shellComboSubsequencePositions(query: string, text: string): Set<number> | null {
  const compactQuery = query.trim().toLowerCase();
  if (!compactQuery) return new Set();
  const compactText = text.toLowerCase();
  const positions = new Set<number>();
  let queryIndex = 0;
  for (let index = 0; index < compactText.length && queryIndex < compactQuery.length; index += 1) {
    if (compactText[index] !== compactQuery[queryIndex]) continue;
    positions.add(index);
    queryIndex += 1;
  }
  return queryIndex === compactQuery.length ? positions : null;
}
