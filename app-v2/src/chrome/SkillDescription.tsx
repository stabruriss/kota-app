import { useState, type FocusEvent, type MouseEvent } from 'react';
import { createPortal } from 'react-dom';

interface SkillDescriptionProps {
  text: string;
  fallback?: string;
  className: string;
}

interface TooltipState {
  text: string;
  left: number;
  top: number;
  width: number;
  placement: 'above' | 'below';
}

const TOOLTIP_MARGIN = 12;
const TOOLTIP_MAX_WIDTH = 420;
const TOOLTIP_ESTIMATED_HEIGHT = 220;

export function SkillDescription({ text, fallback = '', className }: SkillDescriptionProps) {
  const copy = text.trim();
  const visible = copy || fallback;
  const [tooltip, setTooltip] = useState<TooltipState | null>(null);

  const showTooltip = (target: HTMLElement) => {
    if (!copy || typeof window === 'undefined') return;
    const rect = target.getBoundingClientRect();
    const width = Math.max(220, Math.min(TOOLTIP_MAX_WIDTH, window.innerWidth - TOOLTIP_MARGIN * 2));
    const left = Math.min(
      Math.max(TOOLTIP_MARGIN, rect.left),
      Math.max(TOOLTIP_MARGIN, window.innerWidth - width - TOOLTIP_MARGIN),
    );
    const belowTop = rect.bottom + 8;
    const placeAbove =
      belowTop + TOOLTIP_ESTIMATED_HEIGHT > window.innerHeight &&
      rect.top > TOOLTIP_ESTIMATED_HEIGHT + TOOLTIP_MARGIN;
    setTooltip({
      text: copy,
      left,
      top: placeAbove ? rect.top - 8 : belowTop,
      width,
      placement: placeAbove ? 'above' : 'below',
    });
  };

  const onMouseEnter = (event: MouseEvent<HTMLSpanElement>) => showTooltip(event.currentTarget);
  const onFocus = (event: FocusEvent<HTMLSpanElement>) => showTooltip(event.currentTarget);
  const hideTooltip = () => setTooltip(null);

  return (
    <>
      <span
        className={className}
        tabIndex={copy ? 0 : undefined}
        onMouseEnter={onMouseEnter}
        onMouseLeave={hideTooltip}
        onFocus={onFocus}
        onBlur={hideTooltip}
        aria-label={copy || undefined}
      >
        {visible}
      </span>
      {tooltip && typeof document !== 'undefined' && createPortal(
        <div
          className={`skill-description-tooltip ${tooltip.placement}`}
          role="tooltip"
          style={{
            left: tooltip.left,
            top: tooltip.top,
            width: tooltip.width,
          }}
        >
          {tooltip.text}
        </div>,
        document.body,
      )}
    </>
  );
}
