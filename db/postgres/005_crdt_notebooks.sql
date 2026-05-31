-- Postgres migration 005: platform-owned notebook CRDT persistence and sync audit.
CREATE TABLE IF NOT EXISTS crdt_notebooks (
  notebook_id TEXT NOT NULL,
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived','deleted')),
  created_by TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (app_id, notebook_id)
);

CREATE TABLE IF NOT EXISTS crdt_documents (
  document_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  snapshot_json JSONB NOT NULL,
  content_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  FOREIGN KEY (app_id, notebook_id) REFERENCES crdt_notebooks(app_id, notebook_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS crdt_updates (
  update_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  actor_kind TEXT NOT NULL CHECK (actor_kind IN ('human','ai','system')),
  seq INTEGER NOT NULL,
  operation_json JSONB,
  status TEXT NOT NULL CHECK (status IN ('accepted','rejected')),
  error_code TEXT,
  content_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  FOREIGN KEY (app_id, notebook_id) REFERENCES crdt_notebooks(app_id, notebook_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS crdt_heads (
  app_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  version INTEGER NOT NULL,
  frontier_json JSONB NOT NULL,
  content_hash TEXT NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (app_id, notebook_id),
  FOREIGN KEY (app_id, notebook_id) REFERENCES crdt_notebooks(app_id, notebook_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS crdt_actors (
  app_id TEXT NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
  actor_id TEXT NOT NULL,
  actor_kind TEXT NOT NULL CHECK (actor_kind IN ('human','ai','system')),
  display_name TEXT,
  policy_json JSONB,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (app_id, actor_id)
);

CREATE TABLE IF NOT EXISTS crdt_permissions (
  app_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  permission TEXT NOT NULL,
  granted BOOLEAN NOT NULL DEFAULT TRUE,
  granted_at TIMESTAMPTZ,
  PRIMARY KEY (app_id, notebook_id, actor_id, permission),
  FOREIGN KEY (app_id, notebook_id) REFERENCES crdt_notebooks(app_id, notebook_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS crdt_proposals (
  proposal_id TEXT NOT NULL,
  app_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending','accepted','rejected')),
  proposal_json JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (app_id, notebook_id, proposal_id),
  FOREIGN KEY (app_id, notebook_id) REFERENCES crdt_notebooks(app_id, notebook_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS crdt_sync_cursors (
  cursor_id TEXT PRIMARY KEY,
  app_id TEXT NOT NULL,
  notebook_id TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  last_seen_update_id TEXT,
  frontier_json JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL,
  FOREIGN KEY (app_id, notebook_id) REFERENCES crdt_notebooks(app_id, notebook_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_crdt_updates_notebook_seq ON crdt_updates(app_id, notebook_id, seq);
CREATE INDEX IF NOT EXISTS idx_crdt_updates_opid ON crdt_updates(app_id, notebook_id, ((operation_json ->> 'opId')));
CREATE INDEX IF NOT EXISTS idx_crdt_updates_status_created ON crdt_updates(status, created_at);
CREATE INDEX IF NOT EXISTS idx_crdt_documents_notebook_version ON crdt_documents(app_id, notebook_id, version);
CREATE INDEX IF NOT EXISTS idx_crdt_permissions_actor ON crdt_permissions(app_id, actor_id, permission);
CREATE INDEX IF NOT EXISTS idx_crdt_sync_cursors_notebook_actor ON crdt_sync_cursors(app_id, notebook_id, actor_id);
