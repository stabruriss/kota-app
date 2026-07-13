# Kota App

This directory contains the Kota desktop app source.

Kota is a Tauri 2 application with a React frontend and a Rust backend. It runs
user-configured agent CLIs in project-scoped PTYs, projects provider-native logs
into a shared room view, and coordinates local project state for multi-agent
work.

In the public release repository, `app-v2/` is a synced application source
mirror. Application source changes should go through the project's normal
maintainer workflow. Public repo work such as the website, docs, legal files,
release pages, and public copy should happen outside `app-v2/`.

## Requirements

- macOS for local Tauri development.
- Node.js and npm.
- Rust and Cargo.
- Tauri 2 development prerequisites for macOS.
- User-provided agent CLIs and provider accounts for real agent sessions.

Kota does not bundle model-provider access, API keys, or provider accounts.

## Development

```bash
cd app-v2
npm install
npm run dev
npm run typecheck
npm test
```

Run a local Tauri development window:

```bash
cd app-v2
npm run tauri dev
```

Build local sidecar binaries used by the desktop app:

```bash
cd app-v2
npm run build:sidecars
```

Build the frontend bundle:

```bash
cd app-v2
npm run build
```

Signed, notarized, public DMG releases are handled by the release pipeline and
are not required for local development.

## Testing

Frontend tests use Vitest, React Testing Library, and happy-dom:

```bash
cd app-v2
npm test
```

Backend tests live under `src-tauri/` and can be run with Cargo:

```bash
cd app-v2/src-tauri
cargo test
```

For Rust dependency and lockfile checks:

```bash
cd app-v2
cargo metadata --locked --manifest-path src-tauri/Cargo.toml
```

## Source Layout

```text
app-v2/
  index.html
  package.json
  vite.config.ts
  tsconfig.json
  public/
    kota-design/
  src/
    App.tsx
    main.tsx
    assets/
    chrome/
    hooks/
    lib/
    mock/
    popups/
    styles/
    types/
  src-tauri/
    Cargo.toml
    Cargo.lock
    tauri.conf.json
    build.rs
    capabilities/
    defaults/
    prompts/
    src/
      agent_bus.rs
      bartender.rs
      bbs.rs
      ember.rs
      integrations/
      laughing_man.rs
      lib.rs
      pty/
      violet/
  tests/
```

## Public Release Notes

The public repository may include this app source under `app-v2/`, but official
Kota names, marks, visual identity, generated media, avatars, and other official
assets are governed separately from the Apache-2.0 source license. See the
public repository root legal files for the current source, trademark, asset,
third-party notice, security, and citation policies.

Release artifacts should not be committed to git. Public downloads are expected
to be distributed through GitHub Releases and surfaced through `kota.place`.
