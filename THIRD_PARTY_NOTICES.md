# Third-Party Notices

This file covers third-party font assets, bundled defaults, GitHub mark/icon
assets, and dependency-license summaries identified for Kota 0.1.1.

This file does not modify the Apache License, Version 2.0 for Kota source code.
Third-party assets remain subject to their own licenses and brand policies.

## Bundled Default Rules And Skills

Kota includes bundled default rules and skills for user convenience. Some are
adapted from or derived from third-party or open-source materials and remain
subject to their own licenses and notices. Kota does not claim ownership of
third-party names, marks, or upstream content included for interoperability,
examples, or convenience.

The Kota app bundle includes these default account rules:

- `account-language-always.md`: Kota default account guidance.
- `rules-for-coding.md`: Kota-adapted coding guidance derived from
  `multica-ai/andrej-karpathy-skills` `CLAUDE.md`.

`rules-for-coding.md` records its source in file frontmatter:

```text
https://github.com/multica-ai/andrej-karpathy-skills/blob/main/CLAUDE.md
```

The upstream `multica-ai/andrej-karpathy-skills` repository identifies its
license as MIT.

The Kota app bundle includes these default account skills:

- `frontend-design`: includes `LICENSE.txt` with Apache License, Version 2.0.
- `github`: Kota GitHub integration skill; includes GitHub-related mark/icon
  assets listed under "GitHub Marks And Icons" below.
- `skill-creator`: includes `LICENSE.txt` with Apache License, Version 2.0 and
  a copyright notice for Anthropic, PBC.

Source distributions include upstream license or notice files shipped inside
bundled defaults where applicable.

## Tavern SVG Icons

Kota includes seven Tavern SVG icons:

- `app-v2/src/assets/tavern/icons/commends.svg`
- `app-v2/src/assets/tavern/icons/ghost.svg`
- `app-v2/src/assets/tavern/icons/incarnation.svg`
- `app-v2/src/assets/tavern/icons/shell.svg`
- `app-v2/src/assets/tavern/icons/skills.svg`
- `app-v2/src/assets/tavern/icons/token.svg`
- `app-v2/src/assets/tavern/icons/turns.svg`

SVG Repo License — https://www.svgrepo.com/page/licensing/

## Fonts

Kota includes self-hosted font files under:

- `app-v2/public/kota-design/fonts/`
- `app-v2/src/assets/ember-dream/press-start-2p.ttf`

These fonts are not licensed by Kota under Apache-2.0. They are redistributed
under their upstream font licenses.

Google Fonts states that Google Fonts are open source, free to use, usable
commercially, and may be included in sold products subject to the font license:
https://developers.google.com/fonts/faq

The full SIL Open Font License 1.1 text is included in `OFL-1.1.txt`.

### Cinzel

Files:

- `app-v2/public/kota-design/fonts/cinzel-*.woff2`

Notice:

```text
Copyright 2020 The Cinzel Project Authors
(https://github.com/NDISCOVER/Cinzel)
```

License:

- SIL Open Font License 1.1
- Local font metadata points to: https://scripts.sil.org/OFL
- Upstream notice: https://github.com/google/fonts/blob/main/ofl/cinzel/OFL.txt

### Cormorant Garamond

Files:

- `app-v2/public/kota-design/fonts/cormorant-*.woff2`

Notice:

```text
Copyright 2015 The Cormorant Project Authors
(github.com/CatharsisFonts/Cormorant)
```

License:

- SIL Open Font License 1.1
- Local font metadata points to: https://openfontlicense.org
- Upstream project: https://github.com/catharsisfonts/cormorant

### Nunito

Files:

- `app-v2/public/kota-design/fonts/nunito-*.woff2`

Notice:

```text
Copyright 2014 The Nunito Project Authors
(https://github.com/googlefonts/nunito)
```

License:

- SIL Open Font License 1.1
- Local font metadata points to: https://scripts.sil.org/OFL
- Upstream project: https://github.com/googlefonts/nunito

### JetBrains Mono

Files:

- `app-v2/public/kota-design/fonts/jetbrains-mono-*.woff2`

Notice:

```text
Copyright 2020 The JetBrains Mono Project Authors
(https://github.com/JetBrains/JetBrainsMono)
```

License:

- SIL Open Font License 1.1
- Local font metadata points to: https://scripts.sil.org/OFL
- Upstream license page: https://www.jetbrains.com/lp/mono/

### Press Start 2P

File:

- `app-v2/src/assets/ember-dream/press-start-2p.ttf`

Notice:

```text
Copyright 2012 The Press Start 2P Project Authors (cody@zone38.net),
with Reserved Font Name "Press Start 2P"
```

License:

- SIL Open Font License 1.1
- Local font metadata includes the OFL 1.1 license statement.
- Upstream Google Fonts directory:
  https://github.com/google/fonts/tree/main/ofl/pressstart2p

## GitHub Marks And Icons

Kota includes GitHub-related image assets in the bundled default GitHub skill:

- `app-v2/src-tauri/defaults/account-skills/github/assets/github-small.svg`
- `app-v2/src-tauri/defaults/account-skills/github/assets/github.png`

These files are treated as GitHub brand/mark assets or GitHub-related icon
assets. They are not Kota-owned official assets, and Kota does not grant rights
to GitHub marks.

GitHub's logo policy says GitHub logos may be used on websites or third-party
applications in some scenarios, and points to GitHub's logo usage guidelines:
https://docs.github.com/en/site-policy/other-site-policies/github-logo-policy

GitHub's brand toolkit describes logo usage constraints, including consistent
use and color guidance:
https://brand.github.com/foundations/logo

Octicons are published separately by GitHub's Primer project:
https://github.com/primer/octicons

Kota uses these GitHub assets only for truthful GitHub integration
identification and follows GitHub logo usage guidance.

## Dependency License Summary

This section summarizes the 0.1.1 lockfile audit. Source package metadata and
upstream license files remain authoritative for individual dependency licenses.

### npm

The 0.1.1 `app-v2/package-lock.json` contains 455 non-root package entries.
The observed license fields are permissive/notice licenses, with no GPL-only,
AGPL, or SSPL package found in the lockfile pass.

Summary:

- MIT: 367
- ISC: 44
- Apache-2.0: 14
- Apache-2.0 OR MIT: 13
- BSD-3-Clause: 7
- BSD-2-Clause: 2
- other permissive/notice licenses: Zlib, MPL-2.0 OR Apache-2.0, CC0-1.0,
  CC-BY-4.0, 0BSD, Unlicense

Two package entries omit a `license` field in `package-lock.json`, but their
installed package metadata or bundled license files indicate MIT:

- `fuzzy@0.1.3`
- `khroma@2.1.0`

### Cargo

`cargo metadata --locked --manifest-path app-v2/src-tauri/Cargo.toml` completed
successfully against the tracked `Cargo.lock`. The metadata pass reported 544
packages including the local Kota crate. No GPL-only, AGPL, or SSPL package was
found.

Most packages use MIT, Apache-2.0, or MIT/Apache-2.0-style license expressions.
Notice-bearing or review-worthy non-default expressions include:

- Unicode-3.0 and combined Unicode-3.0 expressions;
- MPL-2.0;
- CDLA-Permissive-2.0;
- BSD-family expressions;
- CC0-1.0;
- LGPL-2.1-or-later as one option in multi-license expressions where permissive
  alternatives are also available.

The only missing license field in the Cargo metadata pass is the local
`kota-v2` crate, which is covered by the root public repository license
metadata.

### Tauri Sidecars

The bundled sidecar binaries are local Kota binaries built from this same
source tree:

- `kota-bbs`
- `kota-agent-bus`
- `kota-bartender`

They do not introduce separate third-party project licenses beyond the Cargo
dependency graph summarized above.

## Release Verification

The 0.1.1 release notice review checked npm and Cargo lockfile metadata, bundled
font licenses, bundled default rules and skills, and GitHub mark/icon assets.
Dedicated license and secret scanning run against each public release tree as
part of the release process.
