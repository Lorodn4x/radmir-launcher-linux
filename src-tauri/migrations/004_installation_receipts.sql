CREATE TABLE installation_receipts (
    context TEXT PRIMARY KEY,
    receipt TEXT NOT NULL,
    prepared_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
);
PRAGMA user_version=4;
