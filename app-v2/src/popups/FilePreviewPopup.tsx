/** File preview popup (S12).
 *
 *  Opens in-place when the user clicks a file in the tree. Renders
 *  the file content with a rendered/raw toggle (markdown-aware for
 *  .md files, plain monospace otherwise). Not a full editor — per
 *  the spec the preview is read-only and secondary to external
 *  tooling; the "Open" button will hand off to the user's $VISUAL
 *  once a Tauri shell command is wired.
 *
 *  For MVP, file bodies come from mock `TreeFile.preview`. When the
 *  FS-read Tauri command lands, swap the source in `handleOpen`.   */

import { useEffect, useState } from 'react';
import type { TreeFile } from '../types/tree';
import { useEsc } from './useEsc';

interface FilePreviewPopupProps {
  file: TreeFile;
  onClose: () => void;
}

type Mode = 'rendered' | 'raw';

export function FilePreviewPopup({ file, onClose }: FilePreviewPopupProps) {
  useEsc(onClose);

  const isMarkdown = /\.mdx?$/i.test(file.name);
  const [mode, setMode] = useState<Mode>(isMarkdown ? 'rendered' : 'raw');

  // If the file type changes (user clicks a different tree row while
  // the popup is open), reset to the default mode for the new type.
  useEffect(() => {
    setMode(isMarkdown ? 'rendered' : 'raw');
  }, [isMarkdown, file.path]);

  const body = file.preview ?? defaultPreviewBody(file);
  const pathHead = splitPath(file.path);

  return (
    <div className="pv-backdrop" role="presentation" onClick={onClose}>
      <div
        className="pv"
        role="dialog"
        aria-modal="true"
        aria-label={`Preview: ${file.name}`}
        data-testid="file-preview-popup"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="pv-head">
          <span className="pv-path" title={file.path}>
            <span className="pv-dim">{pathHead.dir}</span>
            {pathHead.name}
          </span>
          {isMarkdown && (
            <div className="pv-toggle">
              <button
                className={`pv-tgl ${mode === 'rendered' ? 'on' : ''}`}
                onClick={() => setMode('rendered')}
                data-testid="pv-tgl-rendered"
              >
                Rendered
              </button>
              <button
                className={`pv-tgl ${mode === 'raw' ? 'on' : ''}`}
                onClick={() => setMode('raw')}
                data-testid="pv-tgl-raw"
              >
                Raw
              </button>
            </div>
          )}
          <button className="pv-btn" disabled aria-disabled title="External editor — not wired yet">
            Open
          </button>
          <button className="pv-btn" disabled aria-disabled title="Reveal in Finder — not wired yet">
            Reveal
          </button>
          <button className="pv-close" aria-label="Close preview" onClick={onClose}>
            ×
          </button>
        </div>

        <div className="pv-body" data-mode={mode}>
          {isMarkdown && mode === 'rendered'
            ? renderMarkdown(body)
            : <pre className="pv-raw">{body}</pre>}
        </div>

        <div className="pv-foot">
          <span>{file.size ? humanSize(file.size) : '—'}</span>
          <span className="dim">·</span>
          <span>modified {file.modifiedAt ?? '—'}</span>
          {file.hot && (
            <>
              <span className="dim">·</span>
              <span className="hot">last read by Violet · recently</span>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

// ───────────────────────────── helpers ─────────────────────────────

function splitPath(p: string): { dir: string; name: string } {
  const i = p.lastIndexOf('/');
  if (i < 0) return { dir: '', name: p };
  return { dir: p.slice(0, i + 1), name: p.slice(i + 1) };
}

function humanSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function defaultPreviewBody(file: TreeFile): string {
  return `// Preview placeholder for ${file.path}.
//
// The live Tauri FS-read command hasn't landed yet, so this file
// only has a body if its fixture in app-v2/src/mock/tree.ts set one.
// Real content will show here once the shell command is wired (M5+).
`;
}

/** Minimal markdown renderer — covers the subset we actually use in
 *  project docs: ATX headings (# / ##), paragraphs, unordered lists,
 *  inline \`code\`, \*\*bold\*\*, and _italic_ / \*italic\*.
 *  This is intentionally NOT a full parser — real parsing arrives
 *  with `react-markdown` when we drop mock data. */
function renderMarkdown(md: string): React.ReactNode {
  const lines = md.split('\n');
  const out: React.ReactNode[] = [];
  let para: string[] = [];
  let list: string[] = [];

  const flushPara = () => {
    if (para.length) {
      out.push(<p key={`p${out.length}`}>{renderInline(para.join(' '))}</p>);
      para = [];
    }
  };
  const flushList = () => {
    if (list.length) {
      out.push(
        <ul key={`ul${out.length}`}>
          {list.map((li, i) => <li key={i}>{renderInline(li)}</li>)}
        </ul>,
      );
      list = [];
    }
  };

  for (const raw of lines) {
    const line = raw.replace(/\s+$/, '');
    if (/^# /.test(line))         { flushPara(); flushList(); out.push(<h1 key={`h${out.length}`}>{renderInline(line.slice(2))}</h1>); continue; }
    if (/^## /.test(line))        { flushPara(); flushList(); out.push(<h2 key={`h${out.length}`}>{renderInline(line.slice(3))}</h2>); continue; }
    if (/^### /.test(line))       { flushPara(); flushList(); out.push(<h3 key={`h${out.length}`}>{renderInline(line.slice(4))}</h3>); continue; }
    if (/^- /.test(line))         { flushPara(); list.push(line.slice(2)); continue; }
    if (line === '')              { flushPara(); flushList(); continue; }
    para.push(line);
  }
  flushPara();
  flushList();
  return <>{out}</>;
}

/** Inline markdown: backtick code, **bold**, *italic*. */
function renderInline(text: string): React.ReactNode {
  // Tokenize around backticks first so bold/italic inside code stays literal.
  const parts = text.split(/(`[^`]+`)/g);
  return parts.map((part, i) => {
    if (part.startsWith('`') && part.endsWith('`')) {
      return <code key={i}>{part.slice(1, -1)}</code>;
    }
    // Bold then italic (simple left-to-right, no proper AST).
    const bold = part.split(/(\*\*[^*]+\*\*)/g).map((chunk, j) => {
      if (chunk.startsWith('**') && chunk.endsWith('**')) {
        return <strong key={j}>{chunk.slice(2, -2)}</strong>;
      }
      return chunk.split(/(\*[^*]+\*)/g).map((sub, k) => {
        if (sub.startsWith('*') && sub.endsWith('*') && sub.length > 2) {
          return <em key={k}>{sub.slice(1, -1)}</em>;
        }
        return sub;
      });
    });
    return <span key={i}>{bold}</span>;
  });
}
