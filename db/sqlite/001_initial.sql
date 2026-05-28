-- SQLite migration 001: app registry, app package storage, permissions, installation state, generated app storage.
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS apps (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'enabled' CHECK (status IN ('enabled','disabled','quarantined','uninstalled')),
  active_install_id TEXT,
  active_version TEXT,
  data_version INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS app_versions (
  install_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  version TEXT NOT NULL,
  runtime_version TEXT NOT NULL,
  data_version INTEGER NOT NULL,
  manifest_json TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  signature_json TEXT,
  trust_level TEXT NOT NULL DEFAULT 'user-generated',
  status TEXT NOT NULL DEFAULT 'installed' CHECK (status IN ('installed','enabled','disabled','quarantined','rolled-back','uninstalled')),
  created_at TEXT NOT NULL,
  activated_at TEXT
);

CREATE TABLE IF NOT EXISTS app_files (
  install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  content_text TEXT,
  content_hash TEXT NOT NULL,
  size_bytes INTEGER NOT NULL DEFAULT 0,
  mime TEXT NOT NULL DEFAULT 'text/plain',
  created_at TEXT NOT NULL,
  PRIMARY KEY (install_id, path)
);

CREATE TABLE IF NOT EXISTS app_permissions (
  install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  permission TEXT NOT NULL,
  requested INTEGER NOT NULL DEFAULT 1,
  approved INTEGER NOT NULL DEFAULT 0,
  approved_at TEXT,
  reason TEXT,
  PRIMARY KEY (install_id, permission)
);

CREATE TABLE IF NOT EXISTS app_installations (
  installation_event_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  install_id TEXT NOT NULL REFERENCES app_versions(install_id) ON DELETE CASCADE,
  action TEXT NOT NULL CHECK (action IN ('install','activate','disable','rollback','quarantine','uninstall','import')),
  previous_install_id TEXT,
  actor TEXT NOT NULL DEFAULT 'system',
  report_id TEXT,
  created_at TEXT NOT NULL,
  details_json TEXT
);

CREATE TABLE IF NOT EXISTS app_storage (
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value_json TEXT,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (app_id, key)
);

CREATE INDEX IF NOT EXISTS idx_app_versions_app_version ON app_versions(app_id, version);
CREATE INDEX IF NOT EXISTS idx_app_versions_app_status ON app_versions(app_id, status);
CREATE INDEX IF NOT EXISTS idx_app_files_install_path ON app_files(install_id, path);
CREATE INDEX IF NOT EXISTS idx_app_permissions_install_perm ON app_permissions(install_id, permission);
CREATE INDEX IF NOT EXISTS idx_app_installations_app_created ON app_installations(app_id, created_at);
CREATE INDEX IF NOT EXISTS idx_app_storage_app_updated ON app_storage(app_id, updated_at);
