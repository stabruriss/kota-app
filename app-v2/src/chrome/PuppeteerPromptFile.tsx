import { useEffect, useState } from 'react';
import { readSystemPrompt, type SystemPromptReadResult } from '../pty-client';
import bundledPrompts from '../../kota-ext/puppeteer-prompts.json';

export function PuppeteerPromptFile() {
  const [file, setFile] = useState<SystemPromptReadResult | null>(null);
  const [failed, setFailed] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true; setFailed(false);
    void readSystemPrompt({ path: 'puppeteer-prompts.json' }, JSON.stringify(bundledPrompts, null, 2)).then(next => { if (active) setFile(next); }).catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [revision]);
  return <section className="puppeteer-jev-prompts" aria-label="Puppeteer prompt file"><header><h3>Prompt</h3><span>Read only</span></header>
    {file && <><p className="tavern-profile-hint">Prompt file: <code>{file.path}</code></p>{file.warning && <p role="status" className="local-model-warning">{file.warning}</p>}
      <pre tabIndex={0} aria-label="Puppeteer prompt preview">{file.content}</pre></>}
    {failed ? <p role="status">Prompts unavailable. <button type="button" className="local-model-text" onClick={() => setRevision(value => value + 1)}>Retry</button></p> : !file && <p role="status">Loading prompts…</p>}
  </section>;
}
