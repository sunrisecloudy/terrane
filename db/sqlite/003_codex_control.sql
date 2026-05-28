-- SQLite migration 003: Codex/dev control, tests, and effect mocks.
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS control_sessions (
  control_session_id TEXT PRIMARY KEY,
  target TEXT NOT NULL,
  runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  actor TEXT NOT NULL DEFAULT 'codex',
  token_hash TEXT,
  started_at TEXT NOT NULL,
  ended_at TEXT,
  status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','ended','failed')),
  metadata_json TEXT
);

CREATE TABLE IF NOT EXISTS control_commands (
  command_id TEXT PRIMARY KEY,
  control_session_id TEXT NOT NULL REFERENCES control_sessions(control_session_id) ON DELETE CASCADE,
  runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  tool TEXT NOT NULL,
  args_json TEXT,
  result_json TEXT,
  error_json TEXT,
  created_at TEXT NOT NULL,
  duration_ms INTEGER
);

CREATE TABLE IF NOT EXISTS micro_tests (
  micro_test_id TEXT PRIMARY KEY,
  app_id TEXT,
  name TEXT NOT NULL,
  spec_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS test_runs (
  test_run_id TEXT PRIMARY KEY,
  micro_test_id TEXT REFERENCES micro_tests(micro_test_id) ON DELETE SET NULL,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  control_session_id TEXT REFERENCES control_sessions(control_session_id) ON DELETE SET NULL,
  app_id TEXT,
  status TEXT NOT NULL CHECK (status IN ('passed','failed','skipped','running','error')),
  started_at TEXT NOT NULL,
  finished_at TEXT,
  result_json TEXT,
  diagnostics_json TEXT
);

CREATE TABLE IF NOT EXISTS network_mocks (
  mock_id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
  app_id TEXT,
  method TEXT NOT NULL DEFAULT 'GET',
  url_pattern TEXT NOT NULL,
  response_json TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS dialog_mocks (
  mock_id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
  app_id TEXT,
  dialog_type TEXT NOT NULL CHECK (dialog_type IN ('openFile','saveFile')),
  response_json TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_control_commands_session_created ON control_commands(control_session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_test_runs_session_started ON test_runs(session_id, started_at);
CREATE INDEX IF NOT EXISTS idx_test_runs_app_started ON test_runs(app_id, started_at);
CREATE INDEX IF NOT EXISTS idx_network_mocks_session_app ON network_mocks(session_id, app_id);
CREATE INDEX IF NOT EXISTS idx_dialog_mocks_session_app ON dialog_mocks(session_id, app_id);
