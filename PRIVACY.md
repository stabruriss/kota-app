# Privacy Policy

This document describes Kota privacy and network behavior for the desktop app,
the `kota.place` website, downloads, update metadata, and related public
services.

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
credentials, API keys, tokens, account sign-in, or BYOK features, their storage
and use depend on the relevant Kota feature, provider tool, operating system
credential store, and user configuration.

## Update Checks

Kota may check `https://kota.place/version.json` for app update metadata, such
as the latest version, release notes, and download URL.

Kota does not silently install updates. A user-visible action is required before
downloading or installing a new app version.

## Website

The `kota.place` website serves downloads, documentation, release metadata,
legal pages, and related assets.

The website may use hosted-service logs and analytics to understand traffic,
reliability, abuse prevention, and downloads. Depending on provider
configuration, these services may use cookies or similar technologies, where
used, and may process standard request metadata such as IP address, user agent,
requested URL, referrer, timestamp, and coarse device/browser information.
Website analytics are not used to collect project files, local workspace data,
chat history, provider credentials, or coding-agent content from the desktop
app.

Currently, page analytics on `kota.place` use the hosting provider's
aggregated, cookieless measurement of page views, referrers, and coarse
device/browser information. No analytics cookies are set, and no user-level
profiles are built.

Embedded product videos on the website are served from YouTube's
privacy-enhanced domain (`youtube-nocookie.com`) and load only after you
press play; before that interaction, no request is made to YouTube.

## Security Reports

Please do not include secrets, API keys, private project data, or vulnerability
details in public issues. Use the security reporting process described in
`SECURITY.md`.

## Children's Privacy

Kota is a developer-focused desktop tool and is not directed to children. Kota
does not request or knowingly collect personal information from children.

## Contact

For privacy questions, open an issue in the public repository for non-sensitive
matters. Do not include confidential information in public issues.

Responsible party: Nan Wu.
