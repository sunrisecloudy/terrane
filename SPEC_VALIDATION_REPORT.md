# Spec Validation Report

Checks performed while preparing this archive:

- Parsed every JSON file in the package.
- Applied SQLite migrations to an in-memory SQLite database.
- Verified required SQLite tables exist.
- Statically checked Postgres schema for required tables and JSONB usage.
- Validated DB record fixtures against the new DB schemas where possible.
- Verified `examples/` and `webapps/examples/` manifests are synchronized.
- Verified the final ZIP can be listed after packaging.

## v0.4 validation counts

| Area | Count |
|---|---:|
| JSON files | 97 |
| SQLite migrations | 4 |
| Postgres migrations | 4 |
| DB test fixtures | 8 |
| DB record fixtures | 4 |
| DB docs | 4 |
| DB schemas | 4 |

## Results

| Check | Errors |
|---|---:|
| JSON parse | 0 |
| DB schema fixture validation | 0 |
| SQLite migration execution | 0 |
| Postgres static schema check | 0 |

Result: **0 validation issues** for the intended v0.4 schema/fixture sets.

Codex should turn these checks into executable CI tasks during implementation.
