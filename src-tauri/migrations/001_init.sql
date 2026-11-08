PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS servers (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    host TEXT NOT NULL,
    port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
    online INTEGER CHECK (online IN (0, 1) OR online IS NULL),
    players INTEGER CHECK (players IS NULL OR players >= 0),
    capacity INTEGER CHECK (capacity IS NULL OR capacity > 0),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS settings (
    id TEXT PRIMARY KEY DEFAULT 'default' CHECK (id = 'default'),
    nickname TEXT NOT NULL DEFAULT 'Name_Surname',
    selected_server_id TEXT REFERENCES servers(id) ON DELETE SET NULL,
    auto_game_path INTEGER NOT NULL DEFAULT 1 CHECK (auto_game_path IN (0, 1)),
    game_path_override TEXT,
    manifest_url TEXT,
    download_base_url TEXT,
    portproton_path TEXT,
    auto_install_portproton INTEGER NOT NULL DEFAULT 1 CHECK (auto_install_portproton IN (0, 1)),
    installer_sha256 TEXT CHECK (installer_sha256 IS NULL OR length(installer_sha256) = 64),
    download_concurrency INTEGER NOT NULL DEFAULT 4 CHECK (download_concurrency BETWEEN 1 AND 16),
    hash_concurrency INTEGER NOT NULL DEFAULT 4 CHECK (hash_concurrency BETWEEN 1 AND 32),
    graphics_preset TEXT NOT NULL DEFAULT 'enhanced' CHECK (graphics_preset IN ('standard', 'enhanced')),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_settings_selected_server ON settings(selected_server_id);

CREATE TABLE IF NOT EXISTS news_items (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    image_url TEXT,
    target_url TEXT,
    published_at TEXT,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_news_sort ON news_items(sort_order, published_at DESC);

CREATE TABLE IF NOT EXISTS game_manifests (
    id TEXT PRIMARY KEY,
    build TEXT NOT NULL UNIQUE,
    source_url TEXT,
    fetched_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS game_files (
    manifest_id TEXT NOT NULL REFERENCES game_manifests(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    size INTEGER NOT NULL CHECK (size >= 0),
    hash TEXT NOT NULL,
    hash_algorithm TEXT NOT NULL CHECK (hash_algorithm IN ('md5', 'sha256')),
    archive INTEGER NOT NULL DEFAULT -1 CHECK (archive >= -1),
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (manifest_id, path)
);
CREATE INDEX IF NOT EXISTS idx_game_files_archive ON game_files(manifest_id, archive);

CREATE TABLE IF NOT EXISTS process_runs (
    id TEXT PRIMARY KEY,
    pid INTEGER NOT NULL,
    executable TEXT NOT NULL,
    server_id TEXT REFERENCES servers(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'running' CHECK (status IN ('running', 'exited', 'failed')),
    exit_code INTEGER,
    log_path TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_process_runs_server ON process_runs(server_id, started_at DESC);
CREATE INDEX IF NOT EXISTS idx_process_runs_status ON process_runs(status, started_at DESC);
