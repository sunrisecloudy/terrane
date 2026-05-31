# Codex Master Prompt

Implement the Native AI Webapp Platform v0.1 from this repository specification.

Hard requirements:

- Generated webapps are build-free HTML/CSS/vanilla JS packages.
- Do not introduce React, Vite, TypeScript, JSX, Next.js, or npm dependencies for generated app runtime execution.
- Runtime web may use dev-only test tooling, but shipped/generated webapps must run without build steps.
- Native shells must expose only the documented bridge methods.
- Every bridge method must enforce permissions.
- Zig core must use a coarse JSON byte API for v0.1.
- All five example apps must keep working after each milestone.

Start with:

1. Create repo skeleton matching `docs/02_PROJECT_STRUCTURE.md`.
2. Implement Zig core fake state machine with tests.
3. Implement runtime web browser mock and launcher.
4. Load examples from `webapps/examples`.
5. Implement manifest validation and permissions.
6. Implement server endpoint.
7. Implement native shells one at a time.

After every change:

- Run relevant tests.
- Update docs/schemas if bridge API changes.
- Keep acceptance checklist current.

## v0.4 persistence directive

Implement the database layer before relying on in-memory package/storage state. Generated apps must never access SQL. Use SQLite for native/reference/dev and maintain Postgres-compatible schema for server production. Add DB tests for migrations, install transaction, storage CRUD, rollback, logs, snapshots, test runs, and backup/export/import.
