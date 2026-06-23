# AGENTS.md — Kota public release repository

Guidance for agents and humans working in this public repository
(`stabruriss/kota-app`).

This is the **public release space**. It is **not** the application source of truth.
Application development happens in the private repository (`stabruriss/kota`).

## Repository role

| Path | Purpose | Editable here? |
| --- | --- | --- |
| `website/` | The `kota.place` website (Vercel, Root Directory = `website/`). | Yes |
| `docs/` | Public documentation. | Yes |
| `legal/` | Public legal notices (trademark, privacy, third-party, citation). | Yes |
| `LICENSE`, `SECURITY.md` | License and security policy. | Yes |
| `README.md`, this file, `.gitignore` | Repo meta. | Yes |
| `app-v2/` | **Read-only mirror** of application source from the private repo. | **No** |

## Hard rules

1. **`app-v2/` is read-only.** It is mirrored from the private repository via
   `kota-public-sync`. Never edit application code here — changes are overwritten on the
   next sync. App-side changes (source, update banner, version check, release pipeline)
   are made in the private repository.

2. **Do not commit build artifacts.** Signed/notarized `.dmg` (and any `.app`, `.pkg`,
   `.ipa`) are distributed via **GitHub Releases**, never via git. `.gitignore` already
   excludes them.

3. **Do not publish or deploy without alignment.** Any deploy to `kota.place`, any
   change to the `version.json` update manifest contract, or any release action must be
   coordinated over the Kota BBS with the private repository side first.

4. **Public-facing copy only.** Documentation, legal text, and website copy must discuss
   the product, its license, downloads, citation, and security/trademark/privacy/network
   behavior only. Do **not** put internal motivations, roadmap, private `plans/`,
   project memory, raw logs, agent/worktree paths, or credentials into this repository.

## In scope here

- Website design, content, and Vercel validation (under `website/`).
- Public documentation (under `docs/`).
- Public legal/notice copy (under `legal/`): trademark, privacy, third-party notices,
  citation.
- Release manifest and validation tooling.
- Public README and this collaboration guide.

## Out of scope here (route back to the private repository)

- Application source changes.
- App auto-update UI and version-check behavior.
- Build, signing, and notarization pipeline.
- The `app-v2/` mirror contents (consumed read-only).

## When in doubt

Coordinate over the Kota BBS rather than guessing. Prefer concrete file references and
scoped changes. Re-read files before editing. Do not revert another contributor's work
without explicit agreement.
