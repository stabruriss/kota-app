# Source and rebuilding references

This runtime is repackaged from the official Pi 1.3.0 darwin-arm64 release. Its source and executable code have not been rebuilt or patched. Only the DOOM overlay example directory is excluded, and notices/provenance are added. Application packaging replaces Mach-O signatures separately.

## Fixed Pi inputs

- Source commit: https://github.com/earendil-works/pi/tree/71023aec1546d6d677a65669c5665c0927ce0dd0
- Official source archive: https://github.com/earendil-works/pi/releases/download/v1.3.0/pi-1.3.0-source.tar.gz
- Source archive SHA-256: `7761ae59cb6d84c1e162d3d638f81816c66d3f75862e941432ea2d7a8e3bf40f`
- Official binary input SHA-256: `1f9ae61cdeae04dd68caf1a1e8ab8ec9b292062f6ccb3252e0a9cbe93b5b16f1`
- Fixed binary build workflow: https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/.github/workflows/build-binaries.yml
- Fixed standalone build script: https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/scripts/build-binaries.sh
- Fixed dependency lock: https://github.com/earendil-works/pi/blob/71023aec1546d6d677a65669c5665c0927ce0dd0/package-lock.json

## Bun and statically linked LGPL components

Pi's official workflow pins Bun 1.3.14. Bun's original license statement, including its component list and relinking guidance, is reproduced in `Bun-UPSTREAM-LICENSE.md` without alteration. Do not describe the whole executable as only MIT-licensed.

- Bun source: https://github.com/oven-sh/bun/tree/0d9b296af33f2b851fcbf4df3e9ec89751734ba4 (tag `bun-v1.3.14`)
- Bun original license statement: https://github.com/oven-sh/bun/blob/0d9b296af33f2b851fcbf4df3e9ec89751734ba4/LICENSE.md
- JavaScriptCore/WebKit source pin: https://github.com/oven-sh/WebKit/tree/5488984d20e0dbfe4be2c3ba8fb18eb81a5e0e8b
- WebKit source/build selector: https://github.com/oven-sh/bun/blob/0d9b296af33f2b851fcbf4df3e9ec89751734ba4/scripts/build/deps/webkit.ts
- JavaScriptCore library terms are reproduced in `JavaScriptCore-COPYING.LIB`, with the GNU LGPL 2.0 text in `LGPL-2.0.txt`.
- TinyCC source pin: https://github.com/oven-sh/tinycc/tree/12882eee073cfe5c7621bcfadf679e1372d4537b
- TinyCC build selector and patch list: https://github.com/oven-sh/bun/blob/0d9b296af33f2b851fcbf4df3e9ec89751734ba4/scripts/build/deps/tinycc.ts
- Bun's TinyCC patch: https://github.com/oven-sh/bun/blob/0d9b296af33f2b851fcbf4df3e9ec89751734ba4/patches/tinycc/tcc.h.patch
- TinyCC's original terms are reproduced in `TinyCC-COPYING`, with GNU LGPL 2.1 text in `LGPL-2.1.txt`.

For a modified library/relinked runtime, start with the fixed Bun source and its corresponding library pins and patches, follow that version's build selectors for a local source build, then use the fixed Pi source and standalone build workflow with the rebuilt Bun. Bun's original statement explains the library modification/relinking requirement; these links document the source inputs and build references. Rebuilding/relinking has not been exercised in this preparation. Complete object/relink materials and applicable dependency notices must be independently verified before the formal app release; this guide does not assert that links alone discharge every distribution obligation.

## Photon

- Fixed package: https://registry.npmjs.org/@silvia-odwyer%2fphoton-node/0.3.4
- Source: https://github.com/silvia-odwyer/photon
- The supplied WASM SHA-256 `10468181565c56004c867f3a4af96f89a0ef5a63a72f2b5fb12c1f1992a3615c` is byte-identical to the pinned npm package.
- Full Apache 2.0 terms from that package are reproduced in `Photon-Apache-2.0.txt`.

## Other dependencies

`PI-DEPENDENCY-INVENTORY.json` records dependency names, versions, upstream package URLs, and declared licenses from Pi's fixed lockfile, excluding entries marked development-only. It is a lockfile inventory, not a claim that every entry is embedded in the standalone binary or that a completed binary SBOM/license audit has been performed. Bun's embedded libraries and polyfills are listed in its unchanged original statement. Preserve the applicable upstream copyrights and notices when completing the distribution inventory.
