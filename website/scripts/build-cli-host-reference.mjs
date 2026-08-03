#!/usr/bin/env node
// Generates the public CLI host compatibility reference from its JSON source.

import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const websiteRoot = join(dirname(fileURLToPath(import.meta.url)), '..');
const repoRoot = join(websiteRoot, '..');
const dataPath = join(repoRoot, 'docs', 'reference', 'cli-host-capabilities.json');
const outPath = join(websiteRoot, 'public', 'docs', 'cli-host-compatibility', 'index.html');
const publicDataPath = join(websiteRoot, 'public', 'docs', 'cli-host-compatibility', 'data.json');
const checkOnly = process.argv.includes('--check');
const statuses = ['VERIFIED', 'PARTIAL', 'UNKNOWN', 'NOT_TESTED', 'NOT_EXPOSED'];

function fail(message) {
  console.error(`build-cli-host-reference: ${message}`);
  process.exit(1);
}

function esc(value) {
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function inline(value) {
  return String(value)
    .split('`')
    .map((part, index) => index % 2 ? `<code>${esc(part)}</code>` : esc(part))
    .join('');
}

let dataText;
let data;
try {
  dataText = readFileSync(dataPath, 'utf8');
  data = JSON.parse(dataText);
} catch (error) {
  fail(`cannot read data: ${error.message}`);
}

if (data.schema_version !== 1) fail(`unsupported schema_version: ${data.schema_version}`);
if (!Array.isArray(data.providers) || data.providers.length === 0) fail('providers must be a non-empty array');
if (!Array.isArray(data.fields) || data.fields.length === 0) fail('fields must be a non-empty array');
if (!data.snapshot?.observed_at || !data.snapshot?.kota_version) fail('snapshot metadata is incomplete');

const providerIds = new Set();
for (const provider of data.providers) {
  for (const key of ['id', 'label', 'binary', 'version']) {
    if (!provider[key]) fail(`provider is missing ${key}`);
  }
  if (providerIds.has(provider.id)) fail(`duplicate provider id: ${provider.id}`);
  providerIds.add(provider.id);
}

for (const field of data.fields) {
  if (!field.id || !field.label || !field.question || !field.cells) {
    fail(`field ${field.id ?? '?'} is incomplete`);
  }
  for (const provider of data.providers) {
    const cell = field.cells[provider.id];
    if (!cell) fail(`field ${field.id} is missing provider ${provider.id}`);
    if (!statuses.includes(cell.status)) fail(`field ${field.id}/${provider.id} has invalid status ${cell.status}`);
    if (!cell.value) fail(`field ${field.id}/${provider.id} is missing value`);
    for (const anchorId of cell.anchors ?? []) {
      if (!data.anchors?.[anchorId]) fail(`unknown anchor ${anchorId} in ${field.id}/${provider.id}`);
    }
  }
}

const statusLabel = (status) => status.replace('_', ' ');
const statusClass = (status) => status.toLowerCase().replace('_', '-');
const statusChip = (status) =>
  `<span class="status ${statusClass(status)}"><span aria-hidden="true"></span>${statusLabel(status)}</span>`;

function anchorLinks(anchorIds) {
  return (anchorIds ?? []).map((anchorId) => {
    const anchor = data.anchors[anchorId];
    return `<a href="${esc(anchor.href)}" target="_blank" rel="noreferrer">${esc(anchor.label)}&nbsp;↗</a>`;
  }).join('');
}

function evidence(cell) {
  const kinds = (cell.evidence ?? [])
    .map((kind) => `<span class="kind">${esc(kind.replace('_', ' '))}</span>`)
    .join('');
  const links = anchorLinks(cell.anchors);
  if (!kinds && !links && !cell.caveat) return '';
  return `<details class="evidence">
                  <summary>Evidence</summary>
                  <div class="evidence-body">
                    ${kinds ? `<div class="kinds">${kinds}</div>` : ''}
                    ${cell.caveat ? `<p class="caveat">${inline(cell.caveat)}</p>` : ''}
                    ${links ? `<div class="anchors">${links}</div>` : ''}
                  </div>
                </details>`;
}

function providerHeader(provider) {
  return `<th scope="col" class="provider-col" data-provider="${esc(provider.id)}">
              <span class="provider-name">${esc(provider.label)}</span>
              <span class="provider-meta"><code>${esc(provider.binary)}</code> · ${esc(provider.version)}</span>
            </th>`;
}

function matrixRow(field, index) {
  const cells = data.providers.map((provider) => {
    const cell = field.cells[provider.id];
    return `<td class="provider-cell" data-provider="${esc(provider.id)}">
              ${statusChip(cell.status)}
              <p class="cell-value">${inline(cell.value)}</p>
              ${evidence(cell)}
            </td>`;
  }).join('\n            ');
  return `<tr>
            <th scope="row" class="field-cell">
              <span class="field-number">${String(index + 1).padStart(2, '0')}</span>
              <span class="field-name">${esc(field.label)}</span>
              <span class="field-question">${esc(field.question)}</span>
            </th>
            ${cells}
          </tr>`;
}

const statusCounts = Object.fromEntries(statuses.map((status) => [status, 0]));
for (const field of data.fields) {
  for (const provider of data.providers) statusCounts[field.cells[provider.id].status] += 1;
}

const providerPicker = data.providers.map((provider, index) =>
  `<button type="button" role="tab" aria-selected="${index === 0}" data-provider-target="${esc(provider.id)}">${esc(provider.label)}</button>`
).join('\n          ');

const providerNotes = data.providers
  .filter((provider) => provider.note)
  .map((provider) => `<div class="provider-note">
          <dt>${esc(provider.label)}</dt>
          <dd>${inline(provider.note)}</dd>
        </div>`)
  .join('\n        ');

const mobileProviderSelectors = data.providers.map((provider) =>
  `.matrix[data-active-provider="${provider.id}"] [data-provider="${provider.id}"]{ display:table-cell; }`
).join('\n    ');

const snapshotDate = new Date(`${data.snapshot.observed_at}T12:00:00Z`).toLocaleDateString('en-US', {
  year: 'numeric', month: 'long', day: 'numeric', timeZone: 'UTC',
});

const page = `<!DOCTYPE html>
<!-- Generated by website/scripts/build-cli-host-reference.mjs. Do not edit by hand. -->
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="description" content="${esc(data.description)}">
<title>${esc(data.title)} — Kota Docs</title>
<link rel="icon" type="image/png" sizes="32x32" href="/assets/kota/app-icons/32x32.png">
<link rel="apple-touch-icon" href="/assets/kota/app-icons/icon.png">
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Hanken+Grotesk:wght@400;500;600;700;800&family=JetBrains+Mono:wght@400;500;600&display=swap" rel="stylesheet">
<script defer src="/_vercel/insights/script.js"></script>
<style>
  :root{
    --bg:#0c0b0a; --panel:#11100e; --panel-2:#151310; --ink:#f1ece5;
    --muted:#aaa39a; --dim:#716b63; --line:rgba(255,255,255,.09);
    --accent:#d98a4f; --paper:#e3d4b6; --verified:#9fbc8d; --partial:#e0b154;
    --unknown:#9aa7b5; --untested:#d08c87; --not-exposed:#8d88ad;
  }
  *{ box-sizing:border-box; }
  html{ scroll-behavior:smooth; }
  body{ margin:0; background:var(--bg); color:var(--ink); font-family:'Hanken Grotesk',system-ui,sans-serif; -webkit-font-smoothing:antialiased; text-rendering:optimizeLegibility; }
  a{ color:inherit; text-decoration:none; }
  button{ font:inherit; }
  ::selection{ background:rgba(217,138,79,.32); }
  .page{ min-height:100vh; background:radial-gradient(120% 70% at 45% -12%,#191510 0%,var(--bg) 60%); }

  header{ position:sticky; top:0; z-index:70; backdrop-filter:blur(14px); background:rgba(12,11,10,.82); border-bottom:1px solid rgba(255,255,255,.06); }
  .hrow{ margin:0 auto; padding:14px 28px; display:flex; align-items:center; gap:24px; }
  .brand{ display:flex; align-items:center; gap:9px; flex:0 0 auto; }
  .brand img{ display:block; border-radius:7px; }
  .brand span{ font-weight:700; font-size:18px; color:var(--ink); }
  nav{ display:flex; gap:22px; flex:1 1 auto; flex-wrap:wrap; font-size:14px; color:#b3aca2; }
  nav a:hover,nav a.now,.gh:hover{ color:#f4efe8; }
  .gh{ font-family:'JetBrains Mono',monospace; font-size:12.5px; color:#b3aca2; flex:0 0 auto; }

  .wrap{ max-width:1480px; margin:0 auto; padding:68px 28px 104px; }
  .breadcrumb{ display:flex; align-items:center; gap:8px; margin-bottom:22px; color:var(--dim); font-family:'JetBrains Mono',monospace; font-size:11px; letter-spacing:.12em; text-transform:uppercase; }
  .breadcrumb a:hover{ color:var(--paper); }
  .breadcrumb span{ color:#4f4a44; }
  .eyebrow{ color:var(--accent); font-family:'JetBrains Mono',monospace; font-size:11px; font-weight:600; letter-spacing:.18em; text-transform:uppercase; }
  h1{ margin:12px 0 16px; max-width:18ch; font-size:48px; line-height:1.04; font-weight:800; letter-spacing:0; }
  .lede{ max-width:72ch; margin:0; color:#b9b2a8; font-size:17px; line-height:1.65; }
  .scope-note{ max-width:88ch; margin:18px 0 0; color:#817a71; font-size:14px; line-height:1.65; }

  .snapshot{ margin:44px 0 0; padding:20px 0; display:grid; grid-template-columns:repeat(5,minmax(140px,1fr)); gap:0; border-top:1px solid var(--line); border-bottom:1px solid var(--line); }
  .snapshot-item{ min-width:0; padding:0 22px; border-left:1px solid var(--line); }
  .snapshot-item:first-child{ padding-left:0; border-left:0; }
  .snapshot-label{ display:block; margin-bottom:7px; color:var(--dim); font-family:'JetBrains Mono',monospace; font-size:9.5px; letter-spacing:.15em; text-transform:uppercase; }
  .snapshot-value{ display:block; color:#dcd5ca; font-size:14px; font-weight:600; overflow-wrap:anywhere; }
  .snapshot-value code{ color:var(--paper); font-family:'JetBrains Mono',monospace; font-size:12px; }

  .section{ margin-top:68px; }
  .section-head{ display:flex; align-items:flex-end; justify-content:space-between; gap:20px; margin-bottom:22px; }
  .section-kicker{ color:var(--dim); font-family:'JetBrains Mono',monospace; font-size:10px; letter-spacing:.18em; text-transform:uppercase; }
  h2{ margin:7px 0 0; font-size:27px; line-height:1.2; letter-spacing:0; }
  .section-note{ max-width:58ch; margin:0; color:#817a71; font-size:13.5px; line-height:1.55; text-align:right; }

  .legend{ display:flex; align-items:center; flex-wrap:wrap; gap:8px; }
  .legend-item{ display:flex; align-items:center; gap:7px; color:#817a71; font-family:'JetBrains Mono',monospace; font-size:10px; }
  .status{ display:inline-flex; width:max-content; align-items:center; gap:6px; border:1px solid currentColor; border-radius:999px; padding:3px 8px; font-family:'JetBrains Mono',monospace; font-size:9px; font-weight:600; line-height:1.2; letter-spacing:.08em; text-transform:uppercase; }
  .status > span{ width:5px; height:5px; border-radius:50%; background:currentColor; }
  .status.verified{ color:var(--verified); background:rgba(159,188,141,.07); }
  .status.partial{ color:var(--partial); background:rgba(224,177,84,.07); }
  .status.unknown{ color:var(--unknown); background:rgba(154,167,181,.07); }
  .status.not-tested{ color:var(--untested); background:rgba(208,140,135,.07); }
  .status.not-exposed{ color:var(--not-exposed); background:rgba(141,136,173,.07); }

  .provider-picker{ display:none; gap:7px; margin:0 0 12px; padding-bottom:4px; overflow:auto; scrollbar-width:none; }
  .provider-picker::-webkit-scrollbar{ display:none; }
  .provider-picker button{ flex:0 0 auto; border:1px solid var(--line); border-radius:999px; padding:8px 12px; color:#817a71; background:#0f0e0c; font-size:12px; cursor:pointer; }
  .provider-picker button[aria-selected="true"]{ color:#17120e; border-color:var(--paper); background:var(--paper); }

  .matrix-shell{ overflow:auto; max-height:74vh; border:1px solid var(--line); border-radius:8px; background:#0e0d0b; box-shadow:0 20px 70px rgba(0,0,0,.24); scrollbar-color:#3e3933 #13110f; }
  table{ width:100%; min-width:1180px; border-collapse:separate; border-spacing:0; table-layout:fixed; }
  th,td{ border-right:1px solid var(--line); border-bottom:1px solid var(--line); text-align:left; vertical-align:top; }
  tr:last-child th,tr:last-child td{ border-bottom:0; }
  th:last-child,td:last-child{ border-right:0; }
  thead th{ position:sticky; top:0; z-index:20; background:rgba(18,16,13,.98); box-shadow:0 1px 0 var(--line); }
  .field-col{ left:0; z-index:30; width:190px; padding:16px 14px; color:var(--dim); font-family:'JetBrains Mono',monospace; font-size:10px; letter-spacing:.15em; text-transform:uppercase; }
  .provider-col{ padding:15px 12px; }
  .provider-name{ display:block; color:#ebe4d9; font-size:15px; font-weight:700; }
  .provider-meta{ display:block; margin-top:5px; color:var(--dim); font-family:'JetBrains Mono',monospace; font-size:10.5px; }
  .provider-meta code{ color:#bfae8a; font:inherit; }
  .field-cell{ position:sticky; left:0; z-index:10; width:190px; padding:18px 14px; background:#12100e; }
  .field-number{ display:block; margin-bottom:10px; color:#5d564e; font-family:'JetBrains Mono',monospace; font-size:10px; }
  .field-name{ display:block; color:#e8e0d5; font-size:15px; line-height:1.3; }
  .field-question{ display:block; margin-top:8px; color:#797168; font-size:12.5px; font-weight:400; line-height:1.45; }
  .provider-cell{ padding:16px 12px 14px; background:#0e0d0b; }
  tbody tr:nth-child(even) .provider-cell{ background:#100f0d; }
  .cell-value{ margin:12px 0 0; color:#c3bcb2; font-size:12.5px; line-height:1.48; overflow-wrap:anywhere; }
  code{ color:#e3d4b6; font-family:'JetBrains Mono',monospace; font-size:.88em; overflow-wrap:anywhere; }
  details.evidence{ margin-top:14px; border-top:1px solid rgba(255,255,255,.065); padding-top:10px; }
  details.evidence summary{ list-style:none; cursor:pointer; color:#817a71; font-family:'JetBrains Mono',monospace; font-size:9.5px; letter-spacing:.11em; text-transform:uppercase; }
  details.evidence summary::-webkit-details-marker{ display:none; }
  details.evidence summary::after{ content:' +'; color:#5d564e; }
  details.evidence[open] summary::after{ content:' −'; }
  details.evidence summary:hover{ color:var(--paper); }
  .evidence-body{ padding-top:11px; }
  .kinds{ display:flex; flex-wrap:wrap; gap:5px; }
  .kind{ color:#82796c; border:1px solid rgba(255,255,255,.08); border-radius:3px; padding:3px 5px; font-family:'JetBrains Mono',monospace; font-size:8.5px; letter-spacing:.05em; }
  .caveat{ margin:9px 0 0; color:#8d857a; font-size:11.5px; line-height:1.5; }
  .anchors{ display:flex; flex-direction:column; align-items:flex-start; gap:7px; margin-top:10px; }
  .anchors a{ color:#a99772; border-bottom:1px solid rgba(169,151,114,.28); font-size:11px; line-height:1.35; }
  .anchors a:hover{ color:#eee4cf; }

  .provider-notes{ margin:0; border-top:1px solid var(--line); }
  .provider-note{ display:grid; grid-template-columns:210px 1fr; gap:26px; padding:17px 0; border-bottom:1px solid var(--line); }
  .provider-note dt{ color:#d8d0c4; font-weight:700; }
  .provider-note dd{ margin:0; color:#91897f; font-size:14px; line-height:1.58; }
  .source-row{ margin-top:30px; padding-top:18px; border-top:1px solid var(--line); display:flex; justify-content:space-between; gap:16px; color:#716b63; font-size:12px; line-height:1.5; }
  .source-row a{ color:#a99772; border-bottom:1px solid rgba(169,151,114,.28); }
  .source-row a:hover{ color:#eee4cf; }

  footer{ border-top:1px solid rgba(255,255,255,.06); }
  .frow{ margin:0 auto; padding:24px 28px; display:flex; align-items:center; justify-content:space-between; gap:16px; font-size:12.5px; color:#9a9389; }
  .frow a{ border-bottom:1px solid rgba(255,255,255,.18); }

  @media (max-width:1080px){
    .snapshot{ grid-template-columns:repeat(3,1fr); row-gap:20px; }
    .snapshot-item:nth-child(4){ padding-left:0; border-left:0; }
  }
  @media (max-width:1220px){
    .provider-picker{ display:flex; }
    .matrix-shell{ max-height:none; overflow:hidden; }
    table{ min-width:0; table-layout:fixed; }
    .field-col,.field-cell{ position:static; width:30%; }
    .provider-col,.provider-cell{ display:none; width:70%; }
    ${mobileProviderSelectors}
  }
  @media (max-width:760px){
    .hrow{ padding:12px 20px; gap:16px; }
    nav{ gap:15px; }
    .gh{ display:none; }
    .wrap{ padding:52px 18px 76px; }
    h1{ font-size:38px; }
    .lede{ font-size:16px; }
    .snapshot{ grid-template-columns:repeat(2,1fr); }
    .snapshot-item,.snapshot-item:nth-child(4){ padding:0 13px; border-left:1px solid var(--line); }
    .snapshot-item:nth-child(odd){ padding-left:0; border-left:0; }
    .section{ margin-top:56px; }
    .section-head{ align-items:flex-start; flex-direction:column; }
    .section-note{ text-align:left; }
    .legend{ gap:7px; }
    .field-col,.field-cell{ width:42%; }
    .provider-col,.provider-cell{ width:58%; }
    .field-col{ padding:14px 12px; }
    .provider-col{ padding:14px 12px; }
    .field-cell{ padding:16px 12px; }
    .field-name{ font-size:13.5px; }
    .field-question{ font-size:11.5px; }
    .provider-cell{ padding:15px 12px; }
    .cell-value{ min-height:0; font-size:12.5px; }
    .provider-note{ grid-template-columns:1fr; gap:7px; }
    .source-row{ flex-direction:column; }
  }
  @media (max-width:430px){
    .brand span{ display:none; }
    .snapshot{ grid-template-columns:1fr; gap:14px; }
    .snapshot-item,.snapshot-item:nth-child(4){ padding:0; border-left:0; }
    .field-col,.field-cell{ width:40%; }
    .provider-col,.provider-cell{ width:60%; }
  }
  @media (prefers-reduced-motion:reduce){ *{ scroll-behavior:auto!important; } }
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
    <div class="breadcrumb"><a href="/docs/">Docs</a><span>/</span><span>Reference</span></div>
    <div class="eyebrow">Evidence snapshot</div>
    <h1>${esc(data.title)}</h1>
    <p class="lede">${esc(data.description)}</p>
    <p class="scope-note">Not a ranking. Unknown means unverified, not unsupported.</p>

    <section class="snapshot" aria-label="Snapshot metadata">
      <div class="snapshot-item"><span class="snapshot-label">Observed</span><span class="snapshot-value">${esc(snapshotDate)} · ${esc(data.snapshot.timezone)}</span></div>
      <div class="snapshot-item"><span class="snapshot-label">Platform</span><span class="snapshot-value">${esc(data.snapshot.platform)}</span></div>
      <div class="snapshot-item"><span class="snapshot-label">Kota</span><span class="snapshot-value">v${esc(data.snapshot.kota_version)} · <code>${esc(data.snapshot.kota_snapshot_commit)}</code></span></div>
      <div class="snapshot-item"><span class="snapshot-label">Providers</span><span class="snapshot-value">${data.providers.length} hosted CLIs</span></div>
      <div class="snapshot-item"><span class="snapshot-label">Records</span><span class="snapshot-value">${data.fields.length * data.providers.length} evidence cells</span></div>
    </section>

    <section class="section" id="matrix">
      <div class="section-head">
        <div><div class="section-kicker">Comparison matrix</div><h2>What Kota has verified</h2></div>
        <div class="legend" aria-label="Evidence status legend">
          ${statuses.map((status) => `<span class="legend-item">${statusChip(status)} ${statusCounts[status]}</span>`).join('')}
        </div>
      </div>
      <div class="provider-picker" role="tablist" aria-label="Choose CLI provider">
        ${providerPicker}
      </div>
      <div class="matrix-shell" tabindex="0" aria-label="Scrollable CLI compatibility matrix">
        <table class="matrix" data-active-provider="${esc(data.providers[0].id)}">
          <thead><tr>
            <th scope="col" class="field-col">Capability</th>
            ${data.providers.map(providerHeader).join('\n            ')}
          </tr></thead>
          <tbody>
          ${data.fields.map(matrixRow).join('\n          ')}
          </tbody>
        </table>
      </div>
    </section>

    ${providerNotes ? `<section class="section" id="provider-notes">
      <div class="section-head"><div><div class="section-kicker">Mapping notes</div><h2>Provider notes</h2></div></div>
      <dl class="provider-notes">${providerNotes}</dl>
    </section>` : ''}

    <div class="source-row">
      <span>${esc(data.snapshot.caveat)}</span>
      <a href="/docs/cli-host-compatibility/data.json">Source data&nbsp;→</a>
    </div>
  </main>

  <footer><div class="frow"><span>© 2026 kota.place · <a href="https://github.com/stabruriss/kota-app/blob/main/LICENSE">License</a></span></div></footer>
</div>
<script>
  (() => {
    const table = document.querySelector('.matrix');
    const buttons = [...document.querySelectorAll('[data-provider-target]')];
    for (const button of buttons) {
      button.addEventListener('click', () => {
        table.dataset.activeProvider = button.dataset.providerTarget;
        for (const candidate of buttons) candidate.setAttribute('aria-selected', String(candidate === button));
      });
    }
  })();
</script>
</body>
</html>
`;

if (checkOnly) {
  const existing = existsSync(outPath) ? readFileSync(outPath, 'utf8') : null;
  const existingData = existsSync(publicDataPath) ? readFileSync(publicDataPath, 'utf8') : null;
  if (existing === page && existingData === dataText) {
    console.log('build-cli-host-reference: check OK');
  } else {
    fail('check FAILED; regenerate the page and public data');
  }
} else {
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, page);
  writeFileSync(publicDataPath, dataText);
  console.log(`build-cli-host-reference: wrote ${outPath}`);
  console.log(`build-cli-host-reference: wrote ${publicDataPath}`);
}
