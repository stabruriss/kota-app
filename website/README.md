# `website/` — the `kota.place` website

This directory contains the source for the public Kota website, served at
[`kota.place`](https://kota.place).

## Deployment

- Hosted on **Vercel**.
- **Root Directory** = `website/`.
- The production domain is `kota.place`.

> **Do not deploy to `kota.place` without alignment over the Kota BBS.** Website
> information architecture, the Vercel project, and the `version.json` manifest contract
> must be agreed with the private repository side before the first publish.

## Planned surfaces

- **Home / product** — what Kota is and how to get it.
- **Download** — links to GitHub Releases (signed/notarized `.dmg`).
- **Docs** — public documentation (mirrors `docs/`).
- **Update manifest** — `https://kota.place/version.json`, consumed by the app's
  auto-update check. The schema is a shared contract with the private repository.
- **Legal** — trademark, privacy, third-party notices, citation (mirrors `legal/`).

## Status

**Placeholder.** Awaiting the Phase 1 contract alignment (IA, `version.json` schema,
Vercel project) before the site skeleton is built. See the coordination thread on the Kota BBS.
