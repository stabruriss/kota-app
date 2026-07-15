#!/usr/bin/env node
// Generates website/public/docs/index.html from website/public/docs/manifest.json.
//
// Google Scholar requires research links to be plain crawlable HTML anchors,
// so this page is rendered at commit time, not in the browser. Adding a
// document = add a manifest record, rerun this script, commit both files.
//
// Usage:
//   node website/scripts/build-docs-index.mjs                  release build: published records only;
//                                                              fails if a draft record has assets on disk
//   node website/scripts/build-docs-index.mjs --include-drafts preview build: drafts render with a DRAFT chip
//   node website/scripts/build-docs-index.mjs --check [...]    verify the committed page matches a fresh
//                                                              render with the same flags

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const VISIBLE_LIMIT = 3; // research entries shown before the overflow fold

const websiteRoot = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifestPath = join(websiteRoot, 'public', 'docs', 'manifest.json');
const outPath = join(websiteRoot, 'public', 'docs', 'index.html');

const includeDrafts = process.argv.includes('--include-drafts');
const checkOnly = process.argv.includes('--check');

function fail(msg) {
  console.error(`build-docs-index: ${msg}`);
  process.exit(1);
}

function esc(value) {
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function localAssetPath(url) {
  return join(websiteRoot, 'public', ...url.replace(/^\//, '').split('/'), 'index.html');
}

// --- load + validate manifest ---------------------------------------------

let manifest;
try {
  manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
} catch (err) {
  fail(`cannot read manifest: ${err.message}`);
}
if (manifest.schema_version !== 1) fail(`unsupported schema_version: ${manifest.schema_version}`);
if (!Array.isArray(manifest.documents)) fail('manifest.documents must be an array');

const seenSlugs = new Set();
for (const doc of manifest.documents) {
  const where = `document "${doc.slug ?? '?'}"`;
  for (const field of ['slug', 'type', 'title', 'authors', 'version', 'visibility', 'order']) {
    if (doc[field] === undefined || doc[field] === '') fail(`${where}: missing required field "${field}"`);
  }
  if (!Array.isArray(doc.authors) || doc.authors.length === 0) fail(`${where}: authors must be a non-empty array`);
  if (!['draft', 'published'].includes(doc.visibility)) fail(`${where}: visibility must be draft|published`);
  if (seenSlugs.has(doc.slug)) fail(`duplicate slug: ${doc.slug}`);
  seenSlugs.add(doc.slug);

  if (!doc.htmlUrl && !doc.externalCanonicalUrl) fail(`${where}: needs htmlUrl or externalCanonicalUrl`);

  if (doc.visibility === 'published') {
    for (const field of ['datePublished']) {
      if (!doc[field]) fail(`${where}: published record missing "${field}"`);
    }
    if (doc.htmlUrl) {
      if (!doc.sha256Html) fail(`${where}: published record missing sha256Html`);
      if (!existsSync(localAssetPath(doc.htmlUrl))) fail(`${where}: published but no asset at ${doc.htmlUrl}`);
    }
    if (doc.pdfUrl && !doc.sha256Pdf) fail(`${where}: published record with pdfUrl missing sha256Pdf`);
    if (!doc.citation || !doc.citation.plain || !doc.citation.bibtex) {
      fail(`${where}: published record missing citation.plain/citation.bibtex`);
    }
  } else if (!includeDrafts && doc.htmlUrl && existsSync(localAssetPath(doc.htmlUrl))) {
    // A deployed file is URL-reachable even when hidden from this page, so a
    // release build refuses draft records whose assets are already on disk.
    fail(`${where}: draft record has assets on disk at ${doc.htmlUrl}; publish it or remove the assets`);
  }
}

const docs = manifest.documents
  .filter((doc) => doc.visibility === 'published' || includeDrafts)
  .sort((a, b) => a.order - b.order);

// --- render -----------------------------------------------------------------

function chipRow(doc) {
  const chips = [
    `<span class="chip type">${esc(doc.type).toUpperCase()}</span>`,
    `<span class="chip">v${esc(doc.version)}</span>`,
  ];
  if (doc.revisionStatus === 'superseded') {
    chips.push('<span class="chip">SUPERSEDED</span>');
  } else if (doc.revisionStatus === 'current' && (doc.versionHistory?.length ?? 0) > 1) {
    chips.push('<span class="chip cur">CURRENT</span>');
  }
  if (doc.visibility === 'draft') {
    chips.push('<span class="chip draft">DRAFT — NOT PUBLISHED</span>');
  }
  return chips.join('\n          ');
}

function recordDetails(doc) {
  const dim = (v) => (v ? `<dd>${esc(v)}</dd>` : '<dd class="dim">pending release</dd>');
  const rows = [`<dt>PUBLISHED</dt>${dim(doc.datePublished)}`];
  if (doc.htmlUrl) {
    rows.push(`<dt>STABLE LINK</dt><dd>kota.place/docs/research/${esc(doc.slug)}/</dd>`);
    rows.push(`<dt>SHA-256 · HTML</dt>${dim(doc.sha256Html)}`);
  }
  if (doc.externalCanonicalUrl) {
    rows.push(`<dt>CANONICAL</dt><dd>${esc(doc.externalCanonicalUrl)}</dd>`);
  }
  if (doc.pdfUrl) {
    const pdfName = doc.pdfUrl.split('/').pop();
    rows.push(`<dt>SHA-256 · PDF</dt>${dim(doc.sha256Pdf)}`);
    rows.push(`<dt>VERIFY</dt><dd><code>shasum -a 256 ${esc(pdfName)}</code></dd>`);
  }
  const cite = (text) =>
    text ? `<pre>${esc(text)}</pre>` : '<pre><i>Supplied with the release bundle.</i></pre>';
  return `<details class="rec">
          <summary>RECORD &amp; CITATION</summary>
          <div class="rec-body">
            <dl class="meta">
              ${rows.join('\n              ')}
            </dl>
            <div class="cite-lbl">PLAIN CITATION</div>
            ${cite(doc.citation?.plain)}
            <div class="cite-lbl">BIBTEX</div>
            ${cite(doc.citation?.bibtex)}
          </div>
        </details>`;
}

function card(doc) {
  const href = doc.htmlUrl || doc.externalCanonicalUrl;
  const year = doc.datePublished ? ` <span>· ${esc(doc.datePublished.slice(0, 4))}</span>` : '';
  const pdf = doc.pdfUrl ? `<a class="pdf" href="${esc(doc.pdfUrl)}">PDF&nbsp;↓</a>
          ` : '';
  return `<article class="doc-card">
        <div class="chips">
          ${chipRow(doc)}
        </div>
        <h2 class="doc-title"><a href="${esc(href)}">${esc(doc.title)}</a></h2>
        <div class="authors">${esc(doc.authors.join(', '))}${year}</div>
        <p class="abstract">${esc(doc.abstract)}</p>
        ${recordDetails(doc)}
        <div class="doc-foot">
          ${pdf}<a class="read" href="${esc(href)}">Read&nbsp;→</a>
        </div>
      </article>`;
}

function researchSection() {
  if (docs.length === 0) return '';
  const visible = docs.slice(0, VISIBLE_LIMIT).map(card);
  const folded = docs.slice(VISIBLE_LIMIT).map(card);
  const fold = folded.length
    ? `
        <details class="more">
          <summary><span class="closed-lbl">SHOW ${folded.length} MORE DOCUMENT${folded.length > 1 ? 'S' : ''} ▾</span><span class="open-lbl">SHOWING ALL ${docs.length} DOCUMENTS ▴</span></summary>
          <div class="more-inner">
            ${folded.join('\n            ')}
          </div>
        </details>`
    : '';
  return `
    <section class="sect">
      <div class="sect-head">
        <div class="sect-title">RESEARCH</div>
        <span class="sect-count">${docs.length} DOCUMENT${docs.length > 1 ? 'S' : ''}</span>
      </div>
      <div class="doclist">
        ${visible.join('\n        ')}${fold}
      </div>
      <p class="immut">Versioned documents are immutable: a revision publishes a new version directory, and existing files and their hashes never change.</p>
    </section>`;
}

const page = `<!DOCTYPE html>
<!-- Generated by website/scripts/build-docs-index.mjs from docs/manifest.json. Do not edit by hand. -->${includeDrafts ? '\n<!-- PREVIEW BUILD: includes draft records. Regenerate without --include-drafts for release. -->' : ''}
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Documentation — Kota</title>
<link rel="icon" type="image/png" sizes="32x32" href="/assets/kota/app-icons/32x32.png">
<link rel="icon" type="image/png" sizes="128x128" href="/assets/kota/app-icons/128x128.png">
<link rel="apple-touch-icon" href="/assets/kota/app-icons/icon.png">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Hanken+Grotesk:ital,wght@0,400;0,500;0,600;0,700;0,800;1,500;1,600&family=JetBrains+Mono:wght@400;500;600&display=swap" rel="stylesheet">
<style>
  *{ box-sizing:border-box; }
  body{ margin:0; background:#0c0b0a; color:#f1ece5; font-family:'Hanken Grotesk',system-ui,sans-serif; -webkit-font-smoothing:antialiased; text-rendering:optimizeLegibility; }
  a{ color:inherit; text-decoration:none; }
  ::selection{ background:rgba(217,138,79,0.32); }

  .page{ min-height:100vh; background:radial-gradient(140% 80% at 50% -10%, #15120f 0%, #0c0b0a 60%); }

  header{ position:sticky; top:0; z-index:60; backdrop-filter:blur(14px); background:rgba(12,11,10,0.72); border-bottom:1px solid rgba(255,255,255,0.06); }
  .hrow{ margin:0 auto; padding:14px 28px; display:flex; align-items:center; gap:24px; }
  .brand{ display:flex; align-items:center; gap:9px; flex:0 0 auto; }
  .brand img{ display:block; border-radius:7px; }
  .brand span{ font-weight:700; font-size:18px; letter-spacing:-0.02em; color:#f1ece5; }
  nav{ display:flex; gap:22px; flex:1 1 auto; flex-wrap:wrap; font-size:14px; color:#b3aca2; }
  nav a:hover{ color:#f4efe8; }
  nav a.now{ color:#f4efe8; }
  .gh{ font-family:'JetBrains Mono',monospace; font-size:12.5px; color:#b3aca2; flex:0 0 auto; }
  .gh:hover{ color:#f4efe8; }

  .wrap{ max-width:920px; margin:0 auto; padding:76px 28px 96px; }

  h1{ margin:0 0 12px; font-size:46px; font-weight:800; letter-spacing:-0.03em; line-height:1.04; }
  .lede{ margin:0; color:#b3aca2; font-size:16.5px; line-height:1.6; max-width:58ch; }

  .sect{ margin-top:64px; }
  .sect-head{ display:flex; align-items:baseline; justify-content:space-between; gap:16px; border-bottom:1px solid rgba(255,255,255,0.07); padding-bottom:11px; margin-bottom:24px; }
  .sect-title{ font-family:'JetBrains Mono',monospace; font-weight:600; font-size:12px; letter-spacing:0.22em; color:#8a847a; }
  .sect-count{ font-family:'JetBrains Mono',monospace; font-size:11px; letter-spacing:0.16em; color:#6f6960; }

  .doclist{ display:flex; flex-direction:column; gap:18px; }

  .doc-card{ border:1px solid rgba(255,255,255,0.08); background:linear-gradient(180deg, rgba(255,255,255,0.028) 0%, rgba(255,255,255,0.012) 100%); border-radius:16px; padding:26px 30px 24px; }
  .doc-card:hover{ border-color:rgba(227,212,182,0.28); }
  .chips{ display:flex; flex-wrap:wrap; gap:8px; align-items:center; }
  .chip{ font-family:'JetBrains Mono',monospace; font-weight:500; font-size:10.5px; letter-spacing:0.14em; padding:4px 10px; border-radius:999px; border:1px solid rgba(255,255,255,0.12); color:#8a847a; }
  .chip.type{ color:#d98a4f; border-color:rgba(217,138,79,0.45); }
  .chip.cur{ color:#9db08a; border-color:rgba(157,176,138,0.4); }
  .chip.draft{ color:#e0b154; border-color:rgba(224,177,84,0.5); background:rgba(224,177,84,0.07); }
  .doc-title{ margin:16px 0 0; font-size:24px; font-weight:750; letter-spacing:-0.02em; line-height:1.26; max-width:34ch; }
  .doc-title a:hover{ color:#f6efe0; border-bottom:1px solid rgba(217,138,79,0.55); }
  .authors{ margin-top:11px; font-weight:600; font-size:13.5px; letter-spacing:0.02em; color:#a99a76; }
  .authors span{ color:#6f6960; }
  .abstract{ margin:14px 0 0; color:#b3aca2; font-size:15px; line-height:1.64; max-width:66ch; }

  details.rec{ margin-top:18px; border:1px solid rgba(255,255,255,0.07); border-radius:12px; background:rgba(0,0,0,0.22); }
  details.rec summary{ list-style:none; cursor:pointer; padding:11px 16px; font-family:'JetBrains Mono',monospace; font-weight:600; font-size:10.5px; letter-spacing:0.2em; color:#a99a76; }
  details.rec summary::-webkit-details-marker{ display:none; }
  details.rec summary::after{ content:"▾"; float:right; color:#6f6960; }
  details.rec[open] summary::after{ content:"▴"; }
  details.rec summary:hover{ color:#f4efe8; }
  .rec-body{ padding:4px 16px 16px; }
  .meta{ display:grid; grid-template-columns:158px 1fr; row-gap:9px; column-gap:16px; font-family:'JetBrains Mono',monospace; font-size:12px; }
  .meta dt{ margin:0; color:#6f6960; letter-spacing:0.16em; font-size:10.5px; padding-top:1px; }
  .meta dd{ margin:0; color:#cfc8bd; overflow-wrap:anywhere; letter-spacing:0.02em; }
  .meta dd.dim{ color:#7e7870; }
  .meta code{ font-family:inherit; color:#e3d4b6; background:rgba(255,255,255,0.045); border:1px solid rgba(255,255,255,0.07); border-radius:6px; padding:2px 7px; }
  .cite-lbl{ font-family:'JetBrains Mono',monospace; font-size:10px; letter-spacing:0.2em; color:#6f6960; margin:14px 0 6px; }
  .rec-body pre{ margin:0; padding:12px 14px; background:#0f0d0b; border:1px solid rgba(255,255,255,0.06); border-radius:9px; font-family:'JetBrains Mono',monospace; font-size:11.5px; line-height:1.6; color:#cfc8bd; white-space:pre-wrap; overflow-wrap:anywhere; }
  .rec-body pre i{ color:#6f6960; font-style:italic; }

  .doc-foot{ margin-top:18px; display:flex; justify-content:flex-end; align-items:center; gap:10px; }
  .read{ display:inline-flex; align-items:center; gap:8px; font-weight:600; font-size:13.5px; letter-spacing:0.02em; color:#ece1c8; border:1px solid rgba(227,212,182,0.42); border-radius:999px; padding:9px 18px; transition:background 0.15s ease, color 0.15s ease; }
  .read:hover{ background:#e3d4b6; color:#16110e; }
  .pdf{ display:inline-flex; align-items:center; font-weight:600; font-size:13px; letter-spacing:0.02em; color:#8a847a; border:1px solid rgba(255,255,255,0.10); border-radius:999px; padding:9px 15px; transition:color 0.15s ease, border-color 0.15s ease; }
  .pdf:hover{ color:#ece1c8; border-color:rgba(227,212,182,0.42); }

  details.more{ border:1px solid rgba(255,255,255,0.07); border-radius:12px; background:transparent; }
  details.more > summary{ list-style:none; cursor:pointer; padding:13px 18px; text-align:center; font-family:'JetBrains Mono',monospace; font-weight:600; font-size:10.5px; letter-spacing:0.2em; color:#a99a76; }
  details.more > summary::-webkit-details-marker{ display:none; }
  details.more > summary:hover{ color:#f4efe8; }
  details.more[open] > summary{ border-bottom:1px solid rgba(255,255,255,0.06); margin-bottom:18px; }
  details.more[open] > summary .closed-lbl{ display:none; }
  details.more:not([open]) > summary .open-lbl{ display:none; }
  .more-inner{ display:flex; flex-direction:column; gap:18px; padding:0 18px 18px; }

  .immut{ margin-top:26px; font-size:13px; line-height:1.65; color:#6f6960; max-width:66ch; }

  .grid2{ display:grid; grid-template-columns:1fr 1fr; gap:20px; }
  .panel{ border:1px solid rgba(255,255,255,0.07); border-radius:14px; padding:8px 24px 12px; background:rgba(255,255,255,0.014); }
  .rows a{ display:flex; align-items:baseline; justify-content:space-between; gap:16px; padding:14px 2px; border-bottom:1px solid rgba(255,255,255,0.055); }
  .rows a:last-child{ border-bottom:none; }
  .rows .lbl{ font-size:15px; color:#e6dfd4; }
  .rows a:hover .lbl{ color:#ffffff; }
  .rows .src{ font-family:'JetBrains Mono',monospace; font-size:10.5px; letter-spacing:0.14em; color:#6f6960; flex:0 0 auto; }
  .rows a:hover .src{ color:#a99a76; }

  footer{ border-top:1px solid rgba(255,255,255,0.06); }
  .frow{ margin:0 auto; padding:24px 28px; display:flex; align-items:center; justify-content:space-between; gap:16px; font-size:12.5px; color:#9a9389; }
  .frow a{ border-bottom:1px solid rgba(255,255,255,0.18); padding-bottom:1px; }
  .frow a:hover{ color:#f4efe8; }

  @media (max-width:760px){
    .wrap{ padding:56px 20px 72px; }
    h1{ font-size:36px; }
    .grid2{ grid-template-columns:1fr; }
    .doc-card{ padding:22px 20px 20px; }
    .hrow{ padding:12px 20px; }
    .meta{ grid-template-columns:1fr; row-gap:3px; }
    .meta dt{ margin-top:10px; }
  }
</style>
</head>
<body>
<div class="page">

  <header>
    <div class="hrow">
      <a class="brand" href="/"><img src="/assets/kota/app-icons/icon.png" alt="Kota" width="27" height="27"><span>kota</span></a>
      <nav>
        <a href="/#features">Features</a>
        <a href="/docs/" class="now">Docs</a>
        <a href="https://github.com/stabruriss/kota-app/discussions">Feedback</a>
      </nav>
      <a class="gh" href="https://github.com/stabruriss/kota-app">GitHub&nbsp;↗</a>
    </div>
  </header>

  <main class="wrap">
    <h1>Documents</h1>
    <p class="lede">Research, guides, and reference material for Kota.</p>
${researchSection()}
    <section class="sect">
      <div class="sect-head">
        <div class="sect-title">GETTING STARTED</div>
      </div>
      <div class="grid2">
        <div class="panel">
          <div class="rows">
            <a href="/"><span class="lbl">Download Kota for macOS</span><span class="src">KOTA.PLACE</span></a>
            <a href="https://github.com/stabruriss/kota-app#readme"><span class="lbl">Install &amp; first run</span><span class="src">GITHUB ↗</span></a>
            <a href="https://github.com/stabruriss/kota-app/tree/main/app-v2"><span class="lbl">Build from source</span><span class="src">GITHUB ↗</span></a>
            <a href="https://github.com/stabruriss/kota-app/releases"><span class="lbl">Release notes</span><span class="src">GITHUB ↗</span></a>
          </div>
        </div>
        <div class="panel">
          <div class="rows">
            <a href="https://github.com/stabruriss/kota-app/blob/main/LICENSE"><span class="lbl">License — Apache-2.0</span><span class="src">LEGAL</span></a>
            <a href="https://github.com/stabruriss/kota-app/blob/main/PRIVACY.md"><span class="lbl">Privacy &amp; network behavior</span><span class="src">LEGAL</span></a>
            <a href="https://github.com/stabruriss/kota-app/blob/main/SECURITY.md"><span class="lbl">Security reporting</span><span class="src">LEGAL</span></a>
            <a href="https://github.com/stabruriss/kota-app/blob/main/CITATION.cff"><span class="lbl">Citing Kota</span><span class="src">LEGAL</span></a>
          </div>
        </div>
      </div>
    </section>
  </main>

  <footer>
    <div class="frow">
      <span>© 2026 kota.place · <a href="https://github.com/stabruriss/kota-app/blob/main/LICENSE">License</a></span>
    </div>
  </footer>

</div>
</body>
</html>
`;

// --- write / check -----------------------------------------------------------

if (checkOnly) {
  const existing = existsSync(outPath) ? readFileSync(outPath, 'utf8') : null;
  if (existing === page) {
    console.log('build-docs-index: check OK — committed page matches a fresh render');
  } else {
    fail('check FAILED — committed docs/index.html differs from a fresh render; rerun the script and commit');
  }
} else {
  writeFileSync(outPath, page);
  console.log(`build-docs-index: wrote ${outPath} (${docs.length} document${docs.length === 1 ? '' : 's'}${includeDrafts ? ', preview build' : ''})`);
}
