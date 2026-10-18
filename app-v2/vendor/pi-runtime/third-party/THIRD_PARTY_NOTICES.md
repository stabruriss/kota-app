# Pi runtime third-party notices

This Kota vendor runtime packages the official Pi 1.3.0 darwin-arm64 binary and supporting resources. Pi executable code, Photon WASM, and the native Darwin module are unchanged. The `examples/extensions/doom-overlay` directory is excluded; the remaining examples and documentation are preserved. Added files contain notices, fixed source references, and provenance. Mach-O re-signing is performed later by application packaging.

| Component | Upstream declaration | Included terms and references |
| --- | --- | --- |
| Pi 1.3.0, Mario Zechner and contributors | MIT | `Pi-MIT.txt`; fixed Pi source in `SOURCE-GUIDE.md` |
| Photon 0.3.4, Silvia Odwyer and contributors | Apache License 2.0 | `Photon-Apache-2.0.txt`; fixed npm package in `SOURCE-GUIDE.md` |
| Bun 1.3.14 and contributors | Bun itself is MIT; embedded components have additional licenses | `Bun-UPSTREAM-LICENSE.md` reproduced without alteration |
| JavaScriptCore/WebKit | LGPL as stated by Bun; source files also carry their own terms | `JavaScriptCore-COPYING.LIB`, `LGPL-2.0.txt`; pinned patched source and build references in `SOURCE-GUIDE.md` |
| TinyCC | LGPL 2.1, with exceptions/other terms stated in its original COPYING | `TinyCC-COPYING`, `LGPL-2.1.txt`; pinned source and Bun patch/build references in `SOURCE-GUIDE.md` |

The Pi MIT declaration does not replace the licenses of its dependencies. Bun's original notice lists further statically linked libraries and polyfills. Fixed dependency metadata and source/rebuilding guidance accompany this notice. The DOOM overlay binary example is not part of this archive. The source archive referenced for Pi is upstream's source artifact, not another binary runtime installed by the build.

The original upstream notices are preserved. This preparation does not claim a completed source/object/relink distribution audit; that verification remains part of the formal app release prerequisites described in `SOURCE-GUIDE.md`.
