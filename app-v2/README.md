# app-v2

This directory contains the Kota desktop app source.

## Source Layout

The desktop app lives here so release builds, update metadata, and public source
history can be reviewed from the same repository.

Do not add installer artifacts here. Signed and notarized `.dmg` files belong in
GitHub Releases, not in git.

## Review Notes

Keep desktop app changes small and reviewable: one concern per change, a clear
changelog entry, and a quick smoke check on a fresh build before release. A
focused diff is easier to verify and safer to ship than a large one.

test
