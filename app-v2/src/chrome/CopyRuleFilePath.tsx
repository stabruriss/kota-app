import { useId, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { violetCopyFilePath } from '../pty-client';
import '../styles/copy-rule-file-path.css';

export interface RuleFileTarget { projectRoot: string; kind: 'string' | 'dance'; agentId?: string }

export function CopyRuleFilePath({ target, dirty = false, disabled = false, onPrepared }: {
  target: RuleFileTarget; dirty?: boolean; disabled?: boolean; onPrepared?: () => void | Promise<unknown>;
}) {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [failed, setFailed] = useState(false);
  const inFlight = useRef(false);
  const tooltipId = useId();
  const copy = async () => {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setMessage(''); setFailed(false);
    let prepared = false;
    try {
      const { path } = await invoke<{ path: string }>('puppeteer_prepare_setup', { ...target });
      prepared = true;
      await violetCopyFilePath(path);
      setMessage('Path copied.');
    } catch {
      setFailed(true);
      setMessage(prepared ? 'Could not copy the file path. Try again.' : 'Could not prepare the configuration file. Try again.');
    } finally {
      inFlight.current = false; setBusy(false);
      // The owner refreshes its saved snapshot without replacing a dirty editor.
      if (prepared) void onPrepared?.();
    }
  };
  return <span className="rule-file-copy">
    <button type="button" aria-label="Copy rule file path" aria-describedby={tooltipId} disabled={disabled || busy} onClick={() => void copy()}>
      <svg viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="1.2" aria-hidden><rect x="5" y="5" width="8" height="8" rx="1.5"/><path d="M10 3H4a1 1 0 0 0-1 1v6"/></svg>
    </button>
    <span className={`rule-file-copy-tooltip${failed ? ' local-model-warning' : ''}`} id={tooltipId} role="tooltip">
      <span role="status">{message || 'Copy rule file path'}</span>
      {dirty && <small>This file does not include your unsaved changes.</small>}
    </span>
  </span>;
}
