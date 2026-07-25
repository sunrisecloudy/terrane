# Terrane privacy

Terrane is a local-first desktop application. The open-source macOS build does
not require a Terrane account and does not include a Terrane-operated analytics
or advertising service.

## Data stored locally

Terrane stores the workspace event log, installed apps, app data, capability
cache, permissions, model configuration, and local operational metadata on the
user's Mac. The default workspace is `~/.terrane`.

Camera and microphone access are requested only when a user invokes features
that need them. macOS controls those permissions. Passwords and provider
credentials use the host's protected credential paths where supported and are
not included in Control Room catalog output.

## Network access

Some capabilities can connect to external services when the user configures
and invokes them. Examples include model providers, web requests, external MCP
servers, sync endpoints, and web publishing. Those services receive the data
needed for the requested operation and apply their own privacy terms.

Installing or running the base application does not automatically enroll the
user in a Terrane cloud service.

## User control

Users can inspect installed apps, capability availability, grants, and safe
operational metadata in Control Room. Removing `~/.terrane` deletes the default
local workspace after Terrane is closed; back up any data that must be retained
first.

Security issues should be reported privately through the repository's GitHub
Security Advisory interface rather than a public issue.
