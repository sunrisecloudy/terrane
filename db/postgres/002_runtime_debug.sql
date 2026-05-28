-- Postgres migration 002: runtime/debug logs and replay snapshots.
CREATE TABLE IF NOT EXISTS runtime_sessions (
  session_id TEXT PRIMARY KEY,
  target TEXT NOT NULL,
  platform TEXT NOT NULL,
  runtime_version TEXT NOT NULL,
  active_app_id TEXT,
  active_install_id TEXT,
  started_at TIMESTAMPTZ NOT NULL,
  ended_at TIMESTAMPTZ,
  status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','ended','failed')),
  capabilities_json JSONB,
  resource_high_water_json JSONB,
  metadata_json JSONB
);

CREATE TABLE IF NOT EXISTS bridge_calls (
  bridge_call_id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
  app_id TEXT,
  install_id TEXT,
  method TEXT NOT NULL,
  params_json JSONB,
  result_json JSONB,
  error_json JSONB,
  duration_ms INTEGER,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS core_events (
  event_id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  app_id TEXT,
  install_id TEXT,
  state_version_before INTEGER,
  event_json JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS core_actions (
  action_id TEXT PRIMARY KEY,
  event_id TEXT NOT NULL REFERENCES core_events(event_id) ON DELETE CASCADE,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  app_id TEXT,
  action_json JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_snapshots (
  snapshot_id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  app_id TEXT,
  install_id TEXT,
  type TEXT NOT NULL CHECK (type IN ('bug-report','pre-install','pre-migration','post-test','golden','manual','debug-bundle')),
  snapshot_json JSONB NOT NULL,
  content_hash TEXT NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bridge_calls_session_created ON bridge_calls(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_bridge_calls_app_method ON bridge_calls(app_id, method);
CREATE INDEX IF NOT EXISTS idx_core_events_session_created ON core_events(session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_core_actions_event_created ON core_actions(event_id, created_at);
CREATE INDEX IF NOT EXISTS idx_runtime_snapshots_session_created ON runtime_snapshots(session_id, created_at);
