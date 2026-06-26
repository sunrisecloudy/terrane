# `forge/data/` — shared shell data catalog

Single source of truth for **cross-shell configuration and enumerations** that must stay
aligned across macOS, iOS, Windows, Linux, Android, and `tools/reference-host`.

## Authority rules

| Kind | Source of truth | Examples |
|------|-----------------|----------|
| **Generated from Rust** | `forge-domain` enums; JSON is checked in CI | `snapshot-types.json`, `app-status-enums.json`, `trust-levels.json` |
| **Authoritative JSON** | This directory | `bundled-apps.json`, `mime-types.json`, `runtime-config.json` |
| **Derived documentation** | Migrations / schema; never applied at runtime | `tables.json` |

Shells load these files once at startup through a per-platform loader (`ForgeDataCatalog`
on macOS). Packaged builds bundle `forge/data/` as resources; dev builds read from the
repo checkout.

## Contract export

App-visible files are inputs to `tools/export-public-contract.mjs` and must be
re-exported when changed. See `forge/spec/data-catalog.md`.