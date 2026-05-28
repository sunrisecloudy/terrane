# Codex Prompt: Implement v0.4 Database Persistence Layer

Implement the database layer specified in:

- `docs/27_DATABASE_SCHEMA.md`
- `docs/28_STORAGE_AND_MIGRATIONS.md`
- `docs/29_BACKUP_EXPORT_IMPORT.md`
- `docs/30_DATABASE_TEST_PLAN.md`
- `db/sqlite/*.sql`
- `db/postgres/*.sql`

Rules:

- Generated apps never access SQL.
- Storage bridge maps to `app_storage`.
- App install is transactional.
- App versions are immutable.
- Permissions are version-scoped.
- Rollback changes `apps.active_install_id`.
- Bridge/core logs and test runs persist in debug/test mode.
- Codex DB access uses safe control-plane tools only.
- No arbitrary SQL in the default plugin.

Implementation order:

1. Apply SQLite migrations in fake-host.
2. Add repository layer.
3. Implement storage CRUD.
4. Implement app install/activate/rollback.
5. Persist bridge/core/runtime/test logs.
6. Implement migration dry-run/apply.
7. Implement backup export/import.
8. Expose DB inspection tools through control plane and MCP.
9. Add tests from `tests/db`.
