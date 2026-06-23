# Privacy And Network Behavior

Status: pre-release disclosure draft.

This document describes the intended public privacy and network-behavior
disclosure for Kota. It is not a complete privacy policy for hosted services,
analytics, update checks, or future account features.

## Desktop App

Kota is a desktop app for working with user-configured coding agents,
provider CLIs, project files, and integrations.

Kota may store app configuration, project metadata, chat history, logs, agent
state, and related workspace data on the user's machine as part of normal app
operation.

This workspace data stays on the user's machine unless the user chooses to move
or share it — for example by pushing a project to their own git remote, or by
sending content to a configured provider tool.

## Provider Tools And Integrations

Kota can run or coordinate user-configured tools such as model-provider CLIs,
source hosting tools, package managers, shells, and other integrations. Those
tools may contact their own services according to their own configuration,
accounts, credentials, and terms.

Kota's installer does not include provider API keys. If a user configures
credentials, API keys, tokens, account sign-in, or future BYOK features, their
storage and use depend on the relevant Kota feature, provider tool, operating
system credential store, and user configuration.

## Update Checks

Future versions of Kota may check `kota.place` for app update metadata, such as
the latest version, release notes, and download URL.

Kota should not silently install updates. A user-visible action should be
required before downloading or installing a new app version.

## Website

The `kota.place` website may serve downloads, documentation, release metadata,
legal pages, and related assets.

Website privacy behavior, analytics, fonts, logs, and any hosted services
should be documented separately when the website implementation is finalized.

## Security Reports

Please do not include secrets, API keys, private project data, or vulnerability
details in public issues. Use the security reporting process described in
`SECURITY.md`.

## Children's Privacy

Kota is a developer-focused desktop tool and is not directed to children. Kota
does not request or knowingly collect personal information from children.

## Contact

For privacy questions before a dedicated channel is published, open an issue in
the public repository for non-sensitive matters. Do not include confidential
information in public issues.

Responsible party: Nan Wu.
