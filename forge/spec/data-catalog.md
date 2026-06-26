# Data catalog (`forge/data/`)

Normative inventory of shared JSON consumed by native shells and `reference-host`.
Phase A (forge-core-plan) introduced this directory; later phases add consumers but
must not fork hard-coded copies in shell source.

## Loader contract

- **Dev:** resolve `forge/data/<file>.json` from the repo root (walk up from CWD / bundle).
- **Packaged:** read the same paths from bundled resources (`forge/data/`).
- Load once at process start; treat load failure as fatal for required files.
- Generated files must match `forge-domain` (see `cargo test -p forge-domain`).

## Files

| File | Authority | Phase | App-visible |
|------|-----------|-------|-------------|
| `bundled-apps.json` | JSON | A2 | yes |
| `mime-types.json` | JSON | A2 | no |
| `env-variables.json` | JSON | A2 | no |
| `control-plane-config.json` | JSON | A2 | no |
| `runtime-config.json` | JSON (version canonical at `0.4.0`) | A2 | yes |
| `engine-room-tables.json` | JSON | A2 | no |
| `snapshot-types.json` | generated (`SnapshotType`) | A3 | yes |
| `app-status-enums.json` | generated (`PackageAppStatus`, `PackageVersionStatus`) | A3 | yes |
| `trust-levels.json` | generated (`TrustLevel`) | A3 | yes |
| `package-manifest.json` | JSON | A3 | yes |
| `control-commands.json` | JSON | A4 | yes |
| `control-response-schema.json` | JSON Schema | A4 | yes |
| `tables.json` | derived from `db/sqlite/` migrations | A5 | no |

## Namespaces (Q8)

Legacy webapp registry statuses use `PackageAppStatus` / `PackageVersionStatus` in
`forge-domain`. v1 workspace applet lifecycle remains `AppletLifecycle` / `applet.*`
commands — do not conflate the two models.