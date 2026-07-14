# Kota Website Design Requirements

This document defines the visual and structural requirements for the public
`kota.place` website. It is intended for website implementation and visual
design handoff.

## Product Context

Kota is a desktop workspace for working with multiple agents inside
project-scoped rooms. The website should make the product feel focused,
technical, elegant, and alive without becoming noisy or promotional.

Primary audience:

- developers and technical users evaluating whether to download Kota;
- existing users checking release notes, documentation, and updates;
- researchers, writers, and contributors looking for citations, papers, or
  public project material.

Primary language: English.

## Design Direction

The site should feel:

- simple, clean, and elegant;
- warm enough to suggest a crafted desktop product;
- precise enough for a technical tool;
- memorable through interaction and object design, not through heavy marketing
  copy.

Avoid generic SaaS landing-page patterns: oversized abstract gradients, vague
hero cards, floating blob backgrounds, stock imagery, and dense marketing
claims. Kota should read as a real desktop tool with a distinct product world.

The visual system should be cohesive with Kota's product identity. If additional
brand context is needed, ask for project aesthetic references before locking the
final color, typography, or object language.

## Technical Framework

Deployment target: Vercel.

Site architecture requirements:

- static-first website;
- Vercel root directory: `website/`;
- production domain: `kota.place`;
- serverless/API routes only where needed for the feedback surface;
- release data supplied by release automation;
- legal and policy pages generated from repository markdown where practical.

The website must expose:

- `/` for the main download and feature overview page;
- `/download` for the current macOS Apple Silicon download surface;
- `/whats-new` for release notes;
- `/docs/` for the unified documentation hub (getting started, guides,
  research);
- `/docs/research/` for papers, whitepapers, citation, and long-form material
  (unified under the `/docs/` hub; supersedes the earlier separate top-level
  `/research` route — BBS `thread-250e21204f13`);
- `/feedback` for the public wishing wall;
- `/legal` for license, notices, trademark, assets, security, privacy, and
  citation links;
- `/version.json` for app update metadata.

## Global Layout

The home page is the product page. It should open directly on the useful
experience: download, current release status, and product explanation.

Global elements:

- compact header with Kota word mark, primary navigation, GitHub link, and
  download CTA;
- sticky download CTA visible throughout the page;
- footer with GitHub repository, legal links, docs, research, feedback, and
  sister-project links;
- responsive layout that works cleanly on narrow mobile screens and wide
  desktop screens.

The sticky CTA must remain useful without obstructing content. On desktop it can
live in the header or as a restrained floating control. On mobile it should be a
bottom action bar or compact persistent button with enough safe-area spacing.

## Download CTA

Primary CTA label: Download.

Primary target: the latest signed and notarized macOS Apple Silicon `.dmg`.

The first public version only supports macOS on Apple Silicon. The site should
state this plainly near the download action without turning the limitation into
the main headline.

Download surface requirements:

- current version number;
- release date;
- file type and platform: macOS Apple Silicon `.dmg`;
- release notes or "What's New" summary;
- checksum/signing/notarization status when available;
- link to GitHub Releases for users who want artifacts and provenance.

## Home Page Content

The home page should combine:

- current download;
- "What's New" for the latest release;
- concise product positioning;
- feature introduction through stacked interactive cards;
- links to docs, research, feedback, GitHub, and sister projects.

The hero should use the Kota name and the current app download as first-viewport
signals. The value proposition belongs in supporting copy, not in a long slogan.

Suggested hero structure:

- Kota word mark or product name;
- one-sentence description: a desktop workspace for working with multiple agents
  inside project-scoped rooms;
- Download CTA;
- small release metadata line;
- one real app screenshot or product image once approved screenshots are
  available.

## Feature Card System

The feature section is the key custom component. It should support many cards,
not just a small marketing grid.

Design concept:

- one unified skeuomorphic collectible-object system;
- inspiration may come from archival media and physical ephemera such as
  cassettes, floppy disks, vinyl sleeves, game cartridges, and ticket stubs;
- the final design should choose one coherent object language rather than mixing
  unrelated object types per card.

Interaction goal:

- stacked cards should look layered, intentional, and browsable;
- hover and focus states should extend or unfold the card;
- image and text should trade emphasis during interaction, creating a
  text-image-text rhythm between collapsed and expanded states;
- collapsed state should still expose title, short detail, and a partial visual
  preview;
- expanded state should reveal more screenshot area and secondary copy;
- keyboard focus must provide the same information as hover.

Card content model:

- feature title;
- short summary;
- detailed description;
- real screenshot or approved visual asset;
- optional tag or system label;
- optional link to documentation.

The component must handle a large number of cards. The first few cards should
highlight primary features; later cards can represent narrower capabilities,
integrations, and workflow details. The stack must stay visually organized when
many cards are present.

Implementation expectations:

- no layout shift when cards expand;
- responsive behavior for mobile touch interaction;
- stable card dimensions and predictable stacking rhythm;
- accessible focus order;
- reduced-motion mode;
- neutral empty-state rendering for feature records without approved screenshots.

## Initial Feature Buckets

Feature content is finalized with real screenshots during content integration.
The component should be designed to support at least these buckets:

- multi-agent project rooms;
- room chat and coordination;
- agent handoff and review workflows;
- source/worktree awareness;
- public documentation and release workflow;
- local-first workspace data;
- provider CLI and tool integration;
- update and download flow;
- security and privacy posture;
- research, citation, and long-form docs.

## Visual Assets

Use real product screenshots for finalized feature cards. Do not make
illustrations that imply product UI behavior not present in the app.

The design system should define:

- logo and word-mark placement rules;
- screenshot frame style;
- card object materials, shadows, edges, and depth;
- typography scale;
- color tokens;
- focus rings;
- motion timing;
- light/dark behavior if both are supported.

Avoid using protected third-party marks except for truthful links or integration
identification, such as the GitHub repository link.

## Feedback Surface

The site needs a friendly public feedback surface for non-technical users.

Surface name: Wishing Well.

Purpose:

- let visitors submit wishes, requests, questions, and discussion prompts from
  the website;
- present existing wishes in a browsable, lightweight wall;
- avoid requiring ordinary users to understand GitHub Issues.

Technical direction:

- website form or embedded feedback board;
- moderation or spam-control path before public display;
- optional GitHub Issue creation for project maintainers and technical users;
- public issue link remains available for technical feedback.

The Wishing Well should feel like part of the Kota site, not a generic support
form.

## Docs And Research

The site should include an expandable area for deeper public material:

- documentation;
- whitepapers;
- papers;
- citation metadata;
- release notes;
- legal and policy pages.

The research/docs surface should be quiet and readable. It should not compete
with the home page's interactive feature cards.

## Sister Projects And External Links

The footer and a small related-projects section should support about five links.
Names and URLs should be configured through content data.

Required external links:

- GitHub repository;
- GitHub Releases or artifact provenance page;
- public issue tracker for technical users.

Any sister-project names, logos, or marks must follow the repository trademark
and asset policies.

## Content Integration

Release automation should supply or update:

- latest version;
- release date;
- latest Apple Silicon `.dmg` URL;
- file size;
- checksum;
- signing/notarization status;
- "What's New" summary;
- release notes link;
- `/version.json` update metadata.

The visual design should be able to render development data and production data
through the same content model.

## Release Data Contract

Release automation updates two JSON files only. It must not edit website HTML,
CSS, component code, documentation, or `app-v2/` source. The website and the
desktop app read these files at runtime.

Files and deploy URLs:

- `website/public/version.json` → `https://kota.place/version.json` — app update
  manifest, read by the desktop app's update check.
- `website/public/whats-new.json` → `https://kota.place/whats-new.json` —
  release summary, read by the website "What's New" surface.

Sample files are committed at these paths and show the exact shape. Release
automation overwrites them per release.

### `version.json` fields

- `schema_version` (number): contract version; bump on a breaking change.
- `product` (string): `"kota"`.
- `channel` (string): release channel, e.g. `"stable"`.
- `version` (string): semver, e.g. `"0.1.1"`.
- `pub_date` (string): ISO 8601 UTC.
- `minimum_os_version` (string): minimum macOS version, from the app build.
- `release_url` (string): GitHub Release tag page.
- `release_notes_url` (string): canonical release notes (GitHub Release body).
- `artifacts[]` (array): one entry per downloadable build.
  - `platform` (string), e.g. `"macos"`.
  - `arch` (string), e.g. `"arm64"`.
  - `filename` (string): asset filename.
  - `download_url` (string): GitHub Releases asset URL.
  - `sha256` (string): hex digest; filled by build/automation.
  - `size_bytes` (number): filled by build/automation.

The first public release ships a single `macos`/`arm64` artifact. The array
allows future platforms without a schema change.

### `whats-new.json` fields

- `schema_version` (number): contract version.
- `version` (string): matches `version.json`.
- `date` (string): `YYYY-MM-DD`.
- `title` (string): short release title.
- `summary` (string): one-line summary.
- `highlights[]` (string array): bullet points for the What's New card.
- `release_notes_url` (string): canonical release notes.
- `download_url` (string): release page or current artifact.

### Boundaries

- DMG/installer artifacts are never committed; they live in GitHub Releases.
- Release automation writes only `version.json` and `whats-new.json`.
- Build-time values (`sha256`, `size_bytes`, `minimum_os_version`, and the
  artifact `download_url`) are populated by the build/release automation.
- Canonical release notes are the GitHub Release body; the website renders only
  the summary and highlights.
- `app-v2/` source sync is a separate track from release-metadata updates.

## Document Library Contract

The `/docs/` surface is an extensible document library. Contract agreed with
the publication owner (One Alpha Docs) in BBS `thread-250e21204f13`.

### Routes

- `/docs/` → `website/public/docs/index.html` — unified documentation hub
  and the research browse surface; research documents render first (full
  bibliographic record folded per card), plus getting-started and legal
  links. The first three documents are visible; further documents sit
  behind a pure-HTML overflow fold, so the hub stays the single browse
  page. There is no standalone `/docs/research/` index page in v1. Card
  actions: `Read` (primary) plus a quiet secondary `PDF` link when the
  record has a `pdfUrl` (owner decision; supersedes the earlier
  Read-only-card rule).
- `/docs/research/<slug>/` — stable unversioned resolver; a temporary
  (non-308) `vercel.json` redirect to the current version directory. A new
  version edits only this redirect; old assets are untouched.
- `/docs/research/<slug>/<version>/` — immutable versioned document. The
  publication owner's self-contained HTML is served byte-verbatim as its own
  page; the site never rewrites or injects into it.
- `/docs/research/<slug>/<version>/<pdf-filename>` — immutable same-version
  PDF in the same directory (Google Scholar `citation_pdf_url` target).

First document (identifiers confirmed by the publication owner):

- slug `long-term-teammates`, version directory `v1` (document version
  `1.0`);
- PDF filename `kota-ai-agents-long-term-teammates-v1.pdf`.

### `docs/manifest.json` fields

`website/public/docs/manifest.json` is the single machine-readable record of
library items. Per document:

- `slug` (string): stable; never regenerated from a title change.
- `type` (string): `whitepaper`, `paper`, `technical-report`, etc.
- `title` (string), `authors[]` (string array), `abstract` (string).
- `datePublished` (string): `YYYY-MM-DD`; empty until release.
- `version` (string): document version, e.g. `"1.0"`.
- `visibility` (string): `draft` | `published`.
- `revisionStatus` (string): `current` | `superseded`.
- `htmlUrl` (string), `pdfUrl` (string, optional).
- `sha256Html` / `sha256Pdf` (string): release checksums; empty until release.
- `citation.plain` / `citation.bibtex` (string): supplied with the release.
- `order` (number): index ordering; lower renders first.

Optional future fields: `externalCanonicalUrl`, `role`
(`canonical` / `preprint` / `author-copy`), `versionHistory[]`.

### Rendering rule

Hub and browse pages are generated from the manifest at commit time
(`website/scripts/build-docs-index.mjs`) and committed as static HTML. Google
Scholar requires plain crawlable `<a href>` links from browse pages;
client-side fetch-and-render is JS-only discovery and is not acceptable for
research listings. Nothing document-specific is hard-coded in page structure;
adding a document = add a manifest record, rerun the generator, commit.

Two detail modes:

1. full self-contained HTML — the delivered artifact is the page;
2. abstract-only landing generated from the record (future venue papers),
   emitting `citation_*` meta and external canonical/PDF links.

### Publication handoff split

The website serves the delivered HTML byte-verbatim, so the artifact itself
must contain: Highwire `citation_*` meta tags (including an absolute
same-directory `citation_pdf_url`), the visible `Download PDF` action, a
minimal back-link to `/docs/`, and a self-canonical to its own versioned URL.
The unversioned slug route is a current-version resolver, not the
bibliographic canonical.

Intake checks before a release commit: searchable, unencrypted PDF under
5 MB; `sha256` values match the manifest; versioned paths are new (an
existing version directory is never overwritten).

### Boundaries

- `visibility: draft` removes a record from rendered pages, but any file
  deployed under `public/` is URL-reachable. The enforceable gate is that
  draft artifacts are never committed to main or deployed.
- Versioned document directories are served with
  `Cache-Control: public, max-age=31536000, immutable`; `docs/manifest.json`
  revalidates like the release JSON files.
- No live deployment of a candidate document without explicit owner release
  GO.

## Accessibility And Quality

Requirements:

- semantic HTML for navigation, main content, forms, and cards;
- visible focus states;
- keyboard-operable feature cards and feedback UI;
- no text hidden only behind hover;
- sufficient contrast;
- readable line lengths in docs and research pages;
- reduced-motion support;
- mobile safe-area handling for sticky CTA;
- no content overlap at common mobile and desktop breakpoints.

## Design Package Handoff

The visual design package should provide:

- homepage composition;
- sticky download CTA behavior across desktop and mobile;
- feature card component in collapsed, hover, focus, expanded, and mobile states;
- screenshot treatment;
- Wishing Well layout;
- docs/research page style;
- footer and related-project link style;
- color and typography tokens;
- motion guidance;
- responsive breakpoints;
- implementation notes for Vercel-hosted static pages.
