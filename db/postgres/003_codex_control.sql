-- Postgres migration 003: Codex/dev control, tests, and effect mocks.
CREATE TABLE IF NOT EXISTS control_sessions (
  control_session_id TEXT PRIMARY KEY,
  target TEXT NOT NULL,
  runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  actor TEXT NOT NULL DEFAULT 'codex',
  token_hash TEXT,
  started_at TIMESTAMPTZ NOT NULL,
  ended_at TIMESTAMPTZ,
  status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running','ended','failed')),
  metadata_json JSONB
);

CREATE TABLE IF NOT EXISTS control_commands (
  command_id TEXT PRIMARY KEY,
  control_session_id TEXT NOT NULL REFERENCES control_sessions(control_session_id) ON DELETE CASCADE,
  runtime_session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  tool TEXT NOT NULL,
  http_method TEXT,
  path TEXT,
  decision TEXT CHECK (decision IN ('accepted','rejected')),
  error_code TEXT,
  args_json JSONB,
  result_json JSONB,
  error_json JSONB,
  created_at TIMESTAMPTZ NOT NULL,
  duration_ms INTEGER
);

CREATE TABLE IF NOT EXISTS micro_tests (
  micro_test_id TEXT PRIMARY KEY,
  app_id TEXT,
  name TEXT NOT NULL,
  spec_json JSONB NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS test_runs (
  test_run_id TEXT PRIMARY KEY,
  micro_test_id TEXT REFERENCES micro_tests(micro_test_id) ON DELETE SET NULL,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE SET NULL,
  control_session_id TEXT REFERENCES control_sessions(control_session_id) ON DELETE SET NULL,
  app_id TEXT,
  status TEXT NOT NULL CHECK (status IN ('passed','failed','skipped','running','error')),
  started_at TIMESTAMPTZ NOT NULL,
  finished_at TIMESTAMPTZ,
  result_json JSONB,
  diagnostics_json JSONB
);

CREATE TABLE IF NOT EXISTS network_mocks (
  mock_id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
  app_id TEXT,
  method TEXT NOT NULL DEFAULT 'GET',
  url_pattern TEXT NOT NULL,
  response_json JSONB NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS dialog_mocks (
  mock_id TEXT PRIMARY KEY,
  session_id TEXT REFERENCES runtime_sessions(session_id) ON DELETE CASCADE,
  app_id TEXT,
  dialog_type TEXT NOT NULL CHECK (dialog_type IN ('openFile','saveFile')),
  response_json JSONB NOT NULL,
  enabled BOOLEAN NOT NULL DEFAULT TRUE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_control_commands_session_created ON control_commands(control_session_id, created_at);
CREATE INDEX IF NOT EXISTS idx_test_runs_session_started ON test_runs(session_id, started_at);
CREATE INDEX IF NOT EXISTS idx_test_runs_app_started ON test_runs(app_id, started_at);
CREATE INDEX IF NOT EXISTS idx_network_mocks_session_app ON network_mocks(session_id, app_id);
CREATE INDEX IF NOT EXISTS idx_dialog_mocks_session_app ON dialog_mocks(session_id, app_id);
