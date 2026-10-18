export interface MermaidTheme {
  background: string;
  surface: string;
  text: string;
  muted: string;
  border: string;
  accent: string;
  font: string;
}

export interface MermaidPreview {
  src: string;
  width: number;
  height: number;
}

// Completed previews (including invalid diagrams) are bounded. Concurrent users
// share work, but Mermaid's global configuration/rendering runs one at a time.
const CACHE_LIMIT = 64;
const cache = new Map<string, MermaidPreview | null>();
const pending = new Map<string, { consumers: Set<symbol>; promise: Promise<MermaidPreview | null> }>();
let queue: Promise<unknown> = Promise.resolve();
let sequence = 0;

export function readMermaidTheme(element: HTMLElement): MermaidTheme {
  const style = getComputedStyle(element);
  const token = (name: string) => style.getPropertyValue(name).trim();
  return {
    background: token('--bg-panel'),
    surface: token('--bg-elevated'),
    text: token('--text') || style.color,
    muted: token('--text-2'),
    border: token('--brass-lo'),
    accent: token('--terra'),
    // SVG images cannot use the page's web fonts. Use the same installed serif
    // stack while measuring and displaying, including a macOS Chinese fallback.
    font: 'Georgia, "Songti SC", serif',
  };
}

async function renderPreview(source: string, theme: MermaidTheme, active: () => boolean): Promise<MermaidPreview | null> {
  if (!source.trim() || source.length > 50_000 || !active()) return null;
  // The self-contained bundle owns separate config/parser state from the core
  // entry used by Excalidraw's converter, which caches its last initialize().
  const { default: mermaid } = await import('mermaid/dist/mermaid.esm.min.mjs');
  if (!active()) return null;
  mermaid.initialize({
    startOnLoad: false,
    securityLevel: 'strict',
    suppressErrorRendering: true,
    htmlLabels: false,
    maxTextSize: 50_000,
    maxEdges: 500,
    secure: ['secure', 'securityLevel', 'startOnLoad', 'suppressErrorRendering', 'htmlLabels',
      'maxTextSize', 'maxEdges', 'theme', 'themeVariables', 'themeCSS', 'fontFamily', 'altFontFamily'],
    theme: 'base',
    fontFamily: theme.font,
    themeVariables: {
      background: theme.background,
      primaryColor: theme.surface,
      primaryTextColor: theme.text,
      primaryBorderColor: theme.accent,
      secondaryColor: theme.background,
      secondaryTextColor: theme.text,
      secondaryBorderColor: theme.border,
      tertiaryColor: theme.surface,
      tertiaryTextColor: theme.text,
      tertiaryBorderColor: theme.border,
      lineColor: theme.muted,
      textColor: theme.text,
      edgeLabelBackground: theme.background,
      clusterBkg: theme.background,
      clusterBorder: theme.border,
      fontSize: '14px',
    },
  });
  const mount = document.createElement('div');
  // Attached for text measurement, never visible or interactive. A fixed layout
  // width also makes parent-sized diagram types independent of the chat column.
  mount.style.cssText = 'position:fixed;left:0;top:0;width:720px;opacity:0;pointer-events:none;z-index:-1';
  mount.setAttribute('aria-hidden', 'true');
  document.body.appendChild(mount);
  try {
    const { svg } = await mermaid.render(`kota-mermaid-${++sequence}`, source, mount);
    const root = new DOMParser().parseFromString(svg, 'image/svg+xml').firstElementChild;
    if (root?.localName !== 'svg') throw new Error('Invalid SVG');
    const viewBox = root.getAttribute('viewBox')?.trim().split(/[\s,]+/).map(Number);
    if (!viewBox || viewBox.length !== 4 || !viewBox.every(Number.isFinite) || viewBox[2] <= 0 || viewBox[3] <= 0) {
      throw new Error('Invalid SVG dimensions');
    }
    const naturalWidth = /max-width:\s*([\d.]+)px/i.exec(root.getAttribute('style') ?? '');
    const width = Math.ceil(Number(naturalWidth?.[1]) || viewBox[2]);
    const height = Math.ceil(viewBox[3] * width / viewBox[2]);
    root.setAttribute('width', String(width));
    root.setAttribute('height', String(height));
    const serialized = new XMLSerializer().serializeToString(root);
    // Display only an isolated image, never bindFunctions or inline SVG nodes.
    return { src: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(serialized)}`, width, height };
  } finally {
    mount.remove();
  }
}

export function requestMermaidPreview(source: string, theme: MermaidTheme) {
  if (!source.trim() || source.length > 50_000) {
    return { promise: Promise.resolve(null), cancel: () => {} };
  }
  const key = JSON.stringify([source, theme]);
  if (cache.has(key)) {
    const preview = cache.get(key)!;
    cache.delete(key);
    cache.set(key, preview);
    return { promise: Promise.resolve(preview), cancel: () => {} };
  }
  let job = pending.get(key);
  if (job?.consumers.size === 0) job = undefined;
  if (!job) {
    const consumers = new Set<symbol>();
    const promise = queue.then(async () => {
      if (!consumers.size) return null;
      const preview = await renderPreview(source, theme, () => consumers.size > 0).catch(() => null);
      if (consumers.size) {
        cache.set(key, preview);
        if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
      }
      return preview;
    }).finally(() => { if (pending.get(key)?.promise === promise) pending.delete(key); });
    job = { consumers, promise };
    pending.set(key, job);
    queue = promise;
  }
  const consumer = Symbol();
  job.consumers.add(consumer);
  return { promise: job.promise, cancel: () => { job.consumers.delete(consumer); } };
}
