import { useEffect, useRef, useState } from 'react';
import { readPuppeteerJevPrompts, type PuppeteerJevPrompt, type PuppeteerJevPrompts as Templates } from '../lib/jev';
import '../styles/jev-settings.css';

const CHOICES: Array<[PuppeteerJevPrompt, string]> = [
  ['stringFirst', 'String · First message'], ['stringLater', 'String · Following messages'],
  ['danceContext', 'Dance · Context based'], ['danceRule', 'Dance · Rule based'],
];
export function PuppeteerJevPrompts() {
  const [templates, setTemplates] = useState<Templates | null>(null);
  const [selected, setSelected] = useState<PuppeteerJevPrompt>('stringFirst');
  const [failed, setFailed] = useState(false);
  const [request, setRequest] = useState(0);
  const sequence = useRef(0);
  useEffect(() => {
    const serial = ++sequence.current; setFailed(false);
    void readPuppeteerJevPrompts().then(value => {
      if (serial === sequence.current) setTemplates(value);
    }).catch(() => { if (serial === sequence.current) setFailed(true); });
    return () => { ++sequence.current; };
  }, [request]);
  return <section className="puppeteer-jev-prompts" aria-label="Jev prompt"><header><h3>Prompt</h3>
    <select aria-label="Jev prompt template" value={selected} onChange={event => setSelected(event.target.value as PuppeteerJevPrompt)}>
      {CHOICES.map(([id, label]) => <option key={id} value={id}>{label}</option>)}
    </select></header>
    {failed ? <p role="status">Prompts unavailable. <button type="button" className="local-model-text" onClick={() => setRequest(value => value + 1)}>Retry</button></p>
      : templates ? <pre tabIndex={0}>{JSON.stringify(templates[selected], null, 2)}</pre> : <p role="status">Loading prompts…</p>}
  </section>;
}
