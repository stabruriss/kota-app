# Kota (public release repository)

This repository is the **public release space** for [Kota](https://kota.place).

Kota ships from two repositories:

| Repository | Role |
| --- | --- |
| `stabruriss/kota` (private) | Product source of truth, dogfood, day-to-day development. |
| `stabruriss/kota-app` (**this repo**, public) | Public website, documentation, legal/public copy, release manifest, and validation. |

> **Product copy is a draft placeholder.** Final product description will land with the website (see [`website/`](./website)). This README documents the repository itself.

## What lives here

| Path | Purpose | Editable here? |
| --- | --- | --- |
| [`website/`](./website) | The `kota.place` website (Vercel, Root Directory = `website/`). | Yes |
| [`docs/`](./docs) | Public documentation. | Yes |
| [`legal/`](./legal) | Public legal notices (trademark, privacy, third-party, citation). | Yes |
| [`LICENSE`](./LICENSE) | Source license for this repository. | Yes |
| [`SECURITY.md`](./SECURITY.md) | Vulnerability reporting policy. | Yes |
| [`app-v2/`](./app-v2) | **Read-only mirror** of the application source from `stabruriss/kota`. | **No** |

## `app-v2/` is a read-only mirror

`app-v2/` is synchronized from the private repository via `kota-public-sync`.
**Do not edit application code here** — changes will be overwritten by the next sync.

- Application source changes are made in `stabruriss/kota`, then mirrored here.
- Build artifacts (signed/notarized `.dmg`) are distributed via **GitHub Releases**, never committed to git.
- See [`app-v2/README.md`](./app-v2/README.md).

## Downloads

Signed and notarized builds are published on the repository's **GitHub Releases** page.
The download page and the `https://kota.place/version.json` update manifest are served by the website (see [`website/`](./website)).

## License

Source code in this repository is licensed under **Apache-2.0** (see [`LICENSE`](./LICENSE)),
*except* that **Kota trademarks and official brand assets are not licensed** under the Apache-2.0
terms and are reserved. See [`legal/`](./legal) for the trademark and notice policy.

> License direction: Apache-2.0 for source code, with Kota marks / official assets reserved separately. Pending final sign-off.

## Contributing & coordination

This repository is coordinated with the private repository over the Kota BBS.
Before any publish/deploy action to `kota.place`, align over BBS first.
App-side changes (update banner, version check, release pipeline) are handled in the private repository.
