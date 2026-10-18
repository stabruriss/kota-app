import { memo, useEffect, useRef, useState } from 'react';
import { readMermaidTheme, requestMermaidPreview, type MermaidPreview, type MermaidTheme } from '../lib/mermaid-preview';
import '../styles/mermaid-preview.css';

export const MermaidBlock = memo(function MermaidBlock({ source }: { source: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  const [theme, setTheme] = useState<MermaidTheme | null>(null);
  const [result, setResult] = useState<{ key: string; preview: MermaidPreview | null } | null>(null);
  const key = JSON.stringify([source, theme]);
  const current = result?.key === key ? result : null;

  useEffect(() => {
    if (!ref.current) return;
    if (typeof IntersectionObserver === 'undefined') { setVisible(true); return; }
    const observer = new IntersectionObserver(([entry]) => setVisible(entry.isIntersecting), { rootMargin: '480px 0px' });
    observer.observe(ref.current);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible || !ref.current) return;
    const element = ref.current;
    const read = () => {
      const next = readMermaidTheme(element);
      setTheme(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
    };
    read();
    // Only ancestor theme attributes, not message content/streaming mutations.
    const observer = new MutationObserver(read);
    for (let parent: HTMLElement | null = element.parentElement; parent; parent = parent.parentElement) {
      observer.observe(parent, { attributes: true, attributeFilter: ['class', 'style', 'data-theme'] });
    }
    return () => observer.disconnect();
  }, [visible]);

  useEffect(() => {
    if (!visible || !theme || current) return;
    let cancelled = false;
    const request = requestMermaidPreview(source, theme);
    void request.promise.then(preview => { if (!cancelled) setResult({ key, preview }); });
    return () => { cancelled = true; request.cancel(); };
  }, [source, theme, key, visible, current]);

  if (current && !current.preview) {
    return <div ref={ref}><pre className="violet-md-code"><code>{source}</code></pre></div>;
  }
  return (
    <div ref={ref} className="violet-mermaid">
      {current?.preview ? (
        <div className="violet-mermaid-viewport" role="region" aria-label="Mermaid diagram" tabIndex={0}>
          <img alt="Mermaid diagram" src={current.preview.src} width={current.preview.width} height={current.preview.height}
            onError={() => setResult({ key, preview: null })} />
        </div>
      ) : <div className="violet-mermaid-status" role="status">{visible ? 'Rendering diagram…' : 'Mermaid diagram'}</div>}
      <details>
        <summary>Mermaid source</summary>
        <pre className="violet-md-code"><code>{source}</code></pre>
      </details>
    </div>
  );
});
