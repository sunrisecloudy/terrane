-- Postgres migration 004: app migrations, install reports, backup/export/import metadata.
CREATE TABLE IF NOT EXISTS app_migrations (
  migration_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  from_data_version INTEGER NOT NULL,
  to_data_version INTEGER NOT NULL,
  migration_json JSONB NOT NULL,
  content_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  UNIQUE(app_id, from_data_version, to_data_version)
);

CREATE TABLE IF NOT EXISTS migration_runs (
  migration_run_id TEXT PRIMARY KEY,
  migration_id TEXT REFERENCES app_migrations(migration_id) ON DELETE SET NULL,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  install_id TEXT REFERENCES app_versions(install_id) ON DELETE SET NULL,
  mode TEXT NOT NULL CHECK (mode IN ('dry-run','apply','rollback')),
  status TEXT NOT NULL CHECK (status IN ('passed','failed','running','rolled-back')),
  pre_snapshot_id TEXT REFERENCES runtime_snapshots(snapshot_id) ON DELETE SET NULL,
  post_snapshot_id TEXT REFERENCES runtime_snapshots(snapshot_id) ON DELETE SET NULL,
  report_json JSONB,
  started_at TIMESTAMPTZ NOT NULL,
  finished_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS app_install_reports (
  report_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  install_id TEXT REFERENCES app_versions(install_id) ON DELETE SET NULL,
  status TEXT NOT NULL CHECK (status IN ('accepted','accepted-with-warnings','rejected','failed','requires-approval')),
  validation_json JSONB,
  security_json JSONB,
  permissions_json JSONB,
  compatibility_json JSONB,
  smoke_test_json JSONB,
  content_hash TEXT,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS backup_exports (
  export_id TEXT PRIMARY KEY,
  type TEXT NOT NULL CHECK (type IN ('backup','debug-bundle','test-fixture','import')),
  source_platform TEXT NOT NULL,
  runtime_version TEXT NOT NULL,
  export_json JSONB NOT NULL,
  content_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  imported_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_app_migrations_app_versions ON app_migrations(app_id, from_data_version, to_data_version);
CREATE INDEX IF NOT EXISTS idx_migration_runs_app_started ON migration_runs(app_id, started_at);
CREATE INDEX IF NOT EXISTS idx_app_install_reports_app_created ON app_install_reports(app_id, created_at);
CREATE INDEX IF NOT EXISTS idx_backup_exports_created ON backup_exports(created_at);
