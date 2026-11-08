use std::{fs, path::Path};

use rusqlite::{params, Connection};
use url::Url;

use crate::{
    error::{AppError, AppResult},
    models::{GameProcess, Server, Settings},
};

pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn open(path: &Path) -> AppResult<Self> {
        let connection = Connection::open(path)?;
        connection.execute_batch(include_str!("../migrations/001_init.sql"))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 2 {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(error) =
                connection.execute_batch(include_str!("../migrations/002_runtime.sql"))
            {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error.into());
            }
            connection.execute_batch("COMMIT")?;
        }
        if version < 3 {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(error) =
                connection.execute_batch(include_str!("../migrations/003_graphics_diagnostics.sql"))
            {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error.into());
            }
            connection.execute_batch("COMMIT")?;
        }
        if version < 4 {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(error) = connection
                .execute_batch(include_str!("../migrations/004_installation_receipts.sql"))
            {
                let _ = connection.execute_batch("ROLLBACK");
                return Err(error.into());
            }
            connection.execute_batch("COMMIT")?;
        }
        connection.execute("INSERT OR IGNORE INTO settings (id) VALUES ('default')", [])?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(Self { connection })
    }

    pub fn load_settings(&self) -> AppResult<Settings> {
        let settings = self.connection.query_row(
            "SELECT nickname, selected_server_id, auto_game_path, game_path_override,
                    manifest_url, download_base_url, portproton_path, auto_install_portproton,
                    installer_sha256, download_concurrency, hash_concurrency, graphics_preset, wine_build, graphics_api
             FROM settings WHERE id = 'default'",
            [],
            |row| {
                Ok(Settings {
                    nickname: row.get(0)?,
                    selected_server_id: row.get(1)?,
                    auto_game_path: row.get(2)?,
                    game_path_override: row.get(3)?,
                    manifest_url: row.get(4)?,
                    download_base_url: row.get(5)?,
                    portproton_path: row.get(6)?,
                    auto_install_portproton: row.get(7)?,
                    installer_sha256: row.get(8)?,
                    download_concurrency: row.get::<_, i64>(9)? as usize,
                    hash_concurrency: row.get::<_, i64>(10)? as usize,
                    graphics_preset: row.get(11)?,
                    wine_build: row.get(12)?,
                    graphics_api: row.get(13)?,
                })
            },
        )?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &Settings) -> AppResult<()> {
        validate_settings(settings)?;
        self.connection.execute(
            "UPDATE settings SET
                nickname = ?1, selected_server_id = ?2, auto_game_path = ?3,
                game_path_override = ?4, manifest_url = ?5, download_base_url = ?6,
                portproton_path = ?7, auto_install_portproton = ?8, installer_sha256 = ?9,
                download_concurrency = ?10, hash_concurrency = ?11, graphics_preset = ?12, wine_build = ?13, graphics_api = ?14,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = 'default'",
            params![
                settings.nickname,
                settings.selected_server_id,
                settings.auto_game_path,
                settings.game_path_override,
                settings.manifest_url,
                settings.download_base_url,
                settings.portproton_path,
                settings.auto_install_portproton,
                settings.installer_sha256,
                settings.download_concurrency as i64,
                settings.hash_concurrency as i64,
                settings.graphics_preset,
                settings.wine_build,
                settings.graphics_api,
            ],
        )?;
        Ok(())
    }

    pub fn cache_servers(&self, servers: &[Server]) -> AppResult<()> {
        for server in servers {
            self.connection.execute(
                "INSERT INTO servers (id, name, host, port, capacity)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                    host = excluded.host, port = excluded.port,
                    capacity = excluded.capacity,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
                params![
                    server.id,
                    server.name,
                    server.host,
                    server.port,
                    server.capacity
                ],
            )?;
        }
        Ok(())
    }

    pub fn record_process_start(&self, process: &GameProcess) -> AppResult<()> {
        self.connection.execute(
            "INSERT INTO process_runs
                (id, pid, executable, server_id, status, exit_code, log_path, started_at, ended_at, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                process.id,
                process.pid as i64,
                process.executable,
                process.server_id,
                process.status,
                process.exit_code,
                process.log_path,
                process.started_at,
                process.ended_at,
                serde_json::to_string(process)?,
            ],
        )?;
        Ok(())
    }

    pub fn record_process_finish(&self, process: &GameProcess) -> AppResult<()> {
        self.connection.execute(
            "UPDATE process_runs SET status = ?2, exit_code = ?3, ended_at = ?4, metadata = ?5,
                    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE id = ?1",
            params![
                process.id,
                process.status,
                process.exit_code,
                process.ended_at,
                serde_json::to_string(process)?
            ],
        )?;
        Ok(())
    }
}

impl Database {
    pub fn installation_receipt(&self, context: &str) -> AppResult<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .connection
            .query_row(
                "SELECT receipt FROM installation_receipts WHERE context=?1",
                [context],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn save_installation_receipt(&self, context: &str, receipt: Option<&str>) -> AppResult<()> {
        if let Some(receipt) = receipt {
            self.connection.execute("INSERT INTO installation_receipts(context,receipt) VALUES(?1,?2) ON CONFLICT(context) DO UPDATE SET receipt=excluded.receipt,prepared_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')", params![context,receipt])?;
        } else {
            self.connection.execute(
                "DELETE FROM installation_receipts WHERE context=?1",
                [context],
            )?;
        }
        Ok(())
    }
    pub fn save_runtime_selection(&self, settings: &Settings) -> AppResult<()> {
        let previous = self.load_settings()?;
        self.connection.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            self.connection.execute(
                "INSERT OR IGNORE INTO runtime_transition(id,previous_settings) VALUES(1,?1)",
                params![serde_json::to_string(&previous)?],
            )?;
            self.save_settings(settings)
        })();
        match result {
            Ok(()) => {
                self.connection.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
    pub fn complete_runtime_selection(&self) -> AppResult<()> {
        self.connection
            .execute("DELETE FROM runtime_transition", [])?;
        Ok(())
    }
    pub fn rollback_runtime_selection(&self) -> AppResult<()> {
        use rusqlite::OptionalExtension;
        let previous: Option<String> = self
            .connection
            .query_row(
                "SELECT previous_settings FROM runtime_transition WHERE id=1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            let previous: Settings = serde_json::from_str(&previous)?;
            let mut current = self.load_settings()?;
            current.wine_build = previous.wine_build;
            current.graphics_api = previous.graphics_api;
            current.portproton_path = previous.portproton_path;
            self.save_settings(&current)?;
            self.complete_runtime_selection()?;
        }
        Ok(())
    }
    pub fn active_processes(&self) -> AppResult<Vec<GameProcess>> {
        let mut query = self.connection.prepare("SELECT metadata FROM process_runs WHERE status IN ('starting','running','recovering') AND metadata IS NOT NULL ORDER BY started_at DESC")?;
        let rows = query.query_map([], |r| r.get::<_, String>(0))?;
        let mut result = Vec::new();
        for row in rows {
            if let Ok(value) = serde_json::from_str(&row?) {
                result.push(value);
            }
        }
        self.connection.execute("UPDATE process_runs SET status='interrupted', ended_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE status IN ('starting','running','recovering') AND metadata IS NULL", [])?;
        Ok(result)
    }
    pub fn begin_preparation(&self, id: &str, settings: &Settings) -> AppResult<()> {
        self.connection.execute("INSERT INTO preparation_runs(id,status,phase,settings,started_at) VALUES(?1,'running','preflight',?2,strftime('%Y-%m-%dT%H:%M:%fZ','now'))", params![id,serde_json::to_string(settings)?])?;
        Ok(())
    }
    pub fn graphics_failure(&self, context: &str) -> AppResult<Option<serde_json::Value>> {
        use rusqlite::OptionalExtension;
        let text: Option<String> = self
            .connection
            .query_row(
                "SELECT error FROM graphics_probe_failures WHERE context=?1",
                [context],
                |r| r.get(0),
            )
            .optional()?;
        text.map(|s| serde_json::from_str(&s).map_err(AppError::from))
            .transpose()
    }
    pub fn set_graphics_failure(&self, context: &str, error: Option<&AppError>) -> AppResult<()> {
        if let Some(error) = error {
            self.connection.execute("INSERT INTO graphics_probe_failures(context,error) VALUES(?1,?2) ON CONFLICT(context) DO UPDATE SET error=excluded.error, checked_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')", params![context, serde_json::to_string(error)?])?;
        } else {
            self.connection.execute(
                "DELETE FROM graphics_probe_failures WHERE context=?1",
                [context],
            )?;
        }
        Ok(())
    }
    pub fn finish_preparation(&self, id: &str, error: Option<&str>) -> AppResult<()> {
        self.connection.execute("UPDATE preparation_runs SET status=?2,phase=?2,error=?3,ended_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id=?1",params![id,if error.is_some(){"failed"}else{"ready"},error])?;
        Ok(())
    }
    pub fn interrupt_preparations(&self) -> AppResult<()> {
        self.connection.execute("UPDATE preparation_runs SET status='interrupted',ended_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE status='running'",[])?;
        Ok(())
    }
    pub fn preferences_imported(&self) -> AppResult<bool> {
        Ok(self.connection.query_row(
            "SELECT runtime_preferences_imported FROM settings WHERE id='default'",
            [],
            |r| r.get::<_, bool>(0),
        )?)
    }
    pub fn mark_preferences_imported(&self) -> AppResult<()> {
        self.connection.execute(
            "UPDATE settings SET runtime_preferences_imported=1 WHERE id='default'",
            [],
        )?;
        Ok(())
    }
    pub fn component_receipt(
        &self,
        root: &str,
        id: &str,
        version: &str,
    ) -> AppResult<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self.connection.query_row("SELECT receipt FROM runtime_components WHERE root=?1 AND component_id=?2 AND version=?3",params![root,id,version],|r|r.get(0)).optional()?)
    }
    pub fn record_component(
        &self,
        root: &str,
        id: &str,
        version: &str,
        hash: &str,
        receipt: &str,
    ) -> AppResult<()> {
        self.connection.execute("INSERT INTO runtime_components(root,component_id,version,sha256,receipt,verified_at) VALUES(?1,?2,?3,?4,?5,strftime('%Y-%m-%dT%H:%M:%fZ','now')) ON CONFLICT(root,component_id,version) DO UPDATE SET sha256=excluded.sha256,receipt=excluded.receipt,verified_at=excluded.verified_at",params![root,id,version,hash,receipt])?;
        Ok(())
    }
}

pub fn validate_settings(settings: &Settings) -> AppResult<()> {
    validate_nickname(&settings.nickname)?;
    if settings.wine_build.is_empty()
        || settings.wine_build.contains("..")
        || settings.wine_build.len() > 96
        || !settings
            .wine_build
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(AppError::coded(
            "invalid_settings",
            "Некорректное имя сборки Wine.",
        ));
    }
    if !["latest", "stable", "sarek", "opengl"].contains(&settings.graphics_api.as_str()) {
        return Err(AppError::coded("invalid_settings", "Неизвестный 3D API."));
    }
    if !(1..=16).contains(&settings.download_concurrency) {
        return Err(AppError::new(
            "Число потоков загрузки должно быть от 1 до 16.",
        ));
    }
    if !(1..=32).contains(&settings.hash_concurrency) {
        return Err(AppError::new(
            "Число потоков проверки должно быть от 1 до 32.",
        ));
    }
    if !matches!(settings.graphics_preset.as_str(), "standard" | "enhanced") {
        return Err(AppError::new("Неизвестный пресет графики."));
    }
    for url in [&settings.manifest_url, &settings.download_base_url] {
        if let Some(value) = url {
            validate_https_url(value)?;
        }
    }
    if let Some(expected) = &settings.installer_sha256 {
        if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new(
                "SHA256 установщика должен содержать 64 hex-символа.",
            ));
        }
    }
    if !settings.auto_game_path && settings.game_path_override.is_none() {
        return Err(AppError::new(
            "Укажите путь к игре или включите автоматический путь.",
        ));
    }
    Ok(())
}

pub fn validate_nickname(nickname: &str) -> AppResult<()> {
    let valid_length = (3..=20).contains(&nickname.len());
    let valid_characters = nickname
        .bytes()
        .all(|byte| byte.is_ascii_alphabetic() || byte == b'_');
    if !valid_length || !valid_characters {
        return Err(AppError::new(
            "Никнейм: 3–20 символов, только латинские буквы и подчёркивание.",
        ));
    }
    Ok(())
}

pub fn validate_https_url(value: &str) -> AppResult<Url> {
    let url = Url::parse(value)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(AppError::new(
            "Разрешены только HTTPS URL без учётных данных.",
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_keeps_legacy_settings_and_process_history_and_is_idempotent() {
        let root = std::env::temp_dir().join(format!("radmir-db-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("db");
        let legacy = Connection::open(&path).unwrap();
        legacy
            .execute_batch(include_str!("../migrations/001_init.sql"))
            .unwrap();
        legacy.execute("INSERT INTO settings(id,nickname,graphics_preset) VALUES('default','Old_Player','standard')",[]).unwrap();
        legacy.execute("INSERT INTO process_runs(id,pid,executable,status,log_path,started_at) VALUES('old',123,'runner','exited','log','2026-01-01')",[]).unwrap();
        drop(legacy);
        for _ in 0..2 {
            let db = Database::open(&path).unwrap();
            let settings = db.load_settings().unwrap();
            assert_eq!(settings.nickname, "Old_Player");
            assert_eq!(settings.graphics_preset, "standard");
            assert_eq!(settings.wine_build, "PROTON_LG_10-28");
            assert_eq!(
                db.connection
                    .query_row("SELECT COUNT(*) FROM process_runs", [], |r| r
                        .get::<_, u32>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                db.connection
                    .query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))
                    .unwrap(),
                4
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn launch_metadata_round_trips_starting_and_running() {
        let f = crate::test_support::Fixture::new();
        let db = f.state.db.lock().unwrap();
        let mut process = GameProcess {
            id: "session".into(),
            pid: std::process::id(),
            executable: "runner.exe".into(),
            server_id: None,
            status: "starting".into(),
            exit_code: None,
            log_path: "log".into(),
            started_at: "2026-11-08".into(),
            ended_at: None,
            windows_pid: None,
            identity: crate::locks::ProcessIdentity::read(std::process::id()).ok(),
            token_port: Some(12345),
            status_path: Some("status".into()),
            prefix: Some("prefix".into()),
        };
        db.record_process_start(&process).unwrap();
        let found = db.active_processes().unwrap();
        assert_eq!(found[0].token_port, Some(12345));
        assert_eq!(found[0].status, "starting");
        process.status = "running".into();
        process.windows_pid = Some(44);
        db.record_process_finish(&process).unwrap();
        assert_eq!(db.active_processes().unwrap()[0].windows_pid, Some(44));
    }
    #[test]
    fn failed_switch_restores_runtime_without_losing_other_settings() {
        let f = crate::test_support::Fixture::new();
        let db = f.state.db.lock().unwrap();
        let old = db.load_settings().unwrap();
        let mut selected = old.clone();
        selected.graphics_api = "sarek".into();
        db.save_runtime_selection(&selected).unwrap();
        selected.nickname = "New_Player".into();
        db.save_settings(&selected).unwrap();
        db.rollback_runtime_selection().unwrap();
        let restored = db.load_settings().unwrap();
        assert_eq!(restored.graphics_api, old.graphics_api);
        assert_eq!(restored.nickname, "New_Player");
        selected.graphics_api = "opengl".into();
        db.save_runtime_selection(&selected).unwrap();
        db.complete_runtime_selection().unwrap();
        db.rollback_runtime_selection().unwrap();
        assert_eq!(db.load_settings().unwrap().graphics_api, "opengl");
    }
}
