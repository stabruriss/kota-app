# Pinned Pi runtime

The tracked archive is the only Pi runtime input for Kota builds. It contains Pi 1.3.0 for darwin-arm64, vendor revision r1. Build preparation validates its SHA-256 and inventory before extracting it into generated staging. No runtime download, external archive cache, system Pi installation, or Git LFS is required.

`pi-runtime.lock.json`, the checksum, provenance and adjacent `third-party/` notices belong to the same revision. The adjacent notices are byte-identical to the archive's `kota-pi/third-party/` files. Upstream URLs in provenance/source guidance document origins; the build never fetches them.

The executable, Photon WASM and native module are unmodified upstream inputs. Only `examples/extensions/doom-overlay` was removed; the remaining documentation and examples are preserved. Code signing happens on bundle copies, never on the tracked archive or input staging.

Run `npm run prepare:pi-runtime` from `app-v2` to validate and stage the pinned input. Tauri's development/build hooks run it before their existing commands. Local debug packaging overrides must include this preparation step too. Generated files, including temporary staging, live under the ignored `src-tauri/pi-runtime/` directory.

After creating a local debug bundle, run `npm run sign:pi-runtime:local -- /path/to/Kota.app` before the existing final App seal. This signs only the two Pi Mach-O files ad hoc, without production entitlements; it does not sign the outer App. No Pi program is executed by preparation or signing.

For an upgrade, prepare a new version/revision deterministically and replace the archive, lock/checksum, provenance and notices in one commit. Remove the old archive from the current tree; Git history retains it. Mirror the same frozen commit's files together and compare bytes. Do not update only the SHA or overwrite published artifacts. The existing public-source scan applies to the adjacent text files; there is no additional archive-expansion privacy gate in that workflow.

Pi 1.1 calls the Azure provider `azure`; its API remains `azure-openai-responses`. Kota leaves credentials and agent configurations using the retired provider ID intact. They do not link the new Azure row. Azure users must select a new model and sign in again with Resource URL; no keys are automatically copied or migrated.

See `third-party/THIRD_PARTY_NOTICES.md` and `third-party/SOURCE-GUIDE.md` for terms and fixed source references. The vendor documentation does not claim a completed binary SBOM or object/relink distribution audit.
