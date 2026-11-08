CREATE TABLE graphics_probe_failures (
    context TEXT PRIMARY KEY,
    error TEXT NOT NULL,
    checked_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
PRAGMA user_version=3;
