ALTER TABLE settings ADD COLUMN wine_build TEXT NOT NULL DEFAULT 'PROTON_LG_10-28';
ALTER TABLE settings ADD COLUMN graphics_api TEXT NOT NULL DEFAULT 'latest';
ALTER TABLE settings ADD COLUMN runtime_preferences_imported INTEGER NOT NULL DEFAULT 0;
ALTER TABLE process_runs RENAME TO process_runs_v1;
CREATE TABLE process_runs (
    id TEXT PRIMARY KEY,
    pid INTEGER NOT NULL,
    executable TEXT NOT NULL,
    server_id TEXT REFERENCES servers(id) ON DELETE SET NULL,
    status TEXT NOT NULL CHECK (status IN ('starting','running','recovering','exited','failed','interrupted')),
    exit_code INTEGER,
    log_path TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    metadata TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
INSERT INTO process_runs (id,pid,executable,server_id,status,exit_code,log_path,started_at,ended_at,created_at,updated_at)
SELECT id,pid,executable,server_id,status,exit_code,log_path,started_at,ended_at,created_at,updated_at FROM process_runs_v1;
DROP TABLE process_runs_v1;
CREATE INDEX idx_process_runs_server ON process_runs(server_id,started_at DESC);
CREATE INDEX idx_process_runs_status ON process_runs(status,started_at DESC);
CREATE TABLE runtime_components (
    root TEXT NOT NULL, component_id TEXT NOT NULL, version TEXT NOT NULL,
    sha256 TEXT NOT NULL, receipt TEXT NOT NULL, verified_at TEXT NOT NULL,
    PRIMARY KEY(root,component_id,version)
);
CREATE TABLE preparation_runs (
    id TEXT PRIMARY KEY, status TEXT NOT NULL, phase TEXT NOT NULL,
    settings TEXT NOT NULL, error TEXT, started_at TEXT NOT NULL, ended_at TEXT
);
CREATE TABLE runtime_transition (
    id INTEGER PRIMARY KEY CHECK(id=1), previous_settings TEXT NOT NULL
);
PRAGMA user_version=2;
