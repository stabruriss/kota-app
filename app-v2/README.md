# `app-v2/` — read-only source mirror

> **Do not edit files in this directory.**

`app-v2/` is a **read-only mirror** of the Kota application source from the private
repository (`stabruriss/kota`). It is kept here so that the public release repository
reflects the published application source.

## How it gets here

This directory is populated and updated exclusively by `kota-public-sync`, which copies
the application source tree from the private repository. **Any edit made here will be
overwritten** by the next sync.

## If you need an application change

Application source changes, the auto-update UI, version-check behavior, and the
build/sign/notarize pipeline all live in the **private repository**. Please coordinate
the request over the Kota BBS so it can be made there and then mirrored back.

## Build artifacts

Signed and notarized `.dmg` builds are distributed via **GitHub Releases**. Build
artifacts are never committed to git (see the repository `.gitignore`).
