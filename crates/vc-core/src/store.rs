use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

use crate::camera::{
    CameraCalibration, CameraConfig, CameraId, CameraPreset, ColorAdjustments, FramingConfig,
    NdiOutputConfig, PtzConfig, VideoSourceConfig, VmixBinding,
};

const CURRENT_SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServerConfig {
    #[serde(default = "default_bind_addr")]
    pub bind_addr: String,
    #[serde(default = "default_lan_access")]
    pub lan_access_enabled: bool,
    #[serde(default = "default_http_port")]
    pub http_port: u16,
    #[serde(default = "default_osc_port")]
    pub osc_port: u16,
}

fn default_bind_addr() -> String {
    "0.0.0.0".to_string()
}
fn default_lan_access() -> bool {
    true
}
fn default_http_port() -> u16 {
    8765
}
fn default_osc_port() -> u16 {
    9000
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bind_addr(),
            lan_access_enabled: default_lan_access(),
            http_port: default_http_port(),
            osc_port: default_osc_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VmixConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_vmix_host")]
    pub host: String,
    #[serde(default = "default_vmix_http_port")]
    pub http_port: u16,
    #[serde(default = "default_vmix_tcp_port")]
    pub tcp_port: u16,
}

fn default_vmix_host() -> String {
    "127.0.0.1".to_string()
}
fn default_vmix_http_port() -> u16 {
    8088
}
fn default_vmix_tcp_port() -> u16 {
    8099
}

impl Default for VmixConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_vmix_host(),
            http_port: default_vmix_http_port(),
            tcp_port: default_vmix_tcp_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamDeckConfig {
    /// Enable direct USB HID Stream Deck control (disable when using Bitfocus Companion/Buttons)
    #[serde(default)]
    pub direct_hid_enabled: bool,
    #[serde(default = "default_brightness")]
    pub brightness: u8,
}

fn default_brightness() -> u8 {
    80
}

impl Default for StreamDeckConfig {
    fn default() -> Self {
        Self {
            direct_hid_enabled: false,
            brightness: default_brightness(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BitfocusButtonsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_buttons_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
}

fn default_buttons_url() -> String {
    "http://127.0.0.1:8000".to_string()
}

impl Default for BitfocusButtonsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_buttons_url(),
            api_key: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub vmix: VmixConfig,
    #[serde(default)]
    pub stream_deck: StreamDeckConfig,
    #[serde(default)]
    pub bitfocus_buttons: BitfocusButtonsConfig,
    #[serde(default)]
    pub cameras: Vec<CameraConfig>,
}

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION as u32
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION as u32,
            server: ServerConfig::default(),
            vmix: VmixConfig::default(),
            stream_deck: StreamDeckConfig::default(),
            bitfocus_buttons: BitfocusButtonsConfig::default(),
            cameras: vec![
                CameraConfig::new_demo("Cam 1 — Stage Wide", "smpte"),
                CameraConfig::new_demo("Cam 2 — Pulpit Follow", "ball"),
            ],
        }
    }
}

/// Embedded SQLite-backed instance settings and camera configuration store.
/// Uses WAL mode, foreign-key enforcement, versioned schema migrations, and automatic
/// one-time migration from legacy `config.json` files.
#[derive(Clone)]
pub struct ConfigStore {
    db_path: PathBuf,
    conn: Arc<Mutex<Connection>>,
    inner: Arc<RwLock<AppConfig>>,
}

impl ConfigStore {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("VideoController")
            .join("videocontroller.db")
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Opens or initializes the embedded SQLite database at `path`.
    ///
    /// - If `path` ends in `.json`, a SQLite database is created alongside it (`<stem>.db`)
    ///   and the JSON file is strictly imported on first run (returning an error if invalid).
    /// - Otherwise, `path` is opened directly as SQLite, and if empty, any sibling `config.json`
    ///   is automatically imported (falling back to defaults with a warning if unreadable).
    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let requested = path.as_ref();
        let is_explicit_json = requested
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("json"))
            .unwrap_or(false);

        let (db_path, legacy_json_path) = if is_explicit_json {
            (requested.with_extension("db"), Some(requested.to_path_buf()))
        } else {
            (
                requested.to_path_buf(),
                requested.parent().map(|p| p.join("config.json")),
            )
        };

        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed creating config directory {}", parent.display())
                })?;
            }
        }

        let mut conn = Connection::open(&db_path)
            .with_context(|| format!("failed opening SQLite database {}", db_path.display()))?;

        Self::configure_pragmas(&conn)
            .with_context(|| format!("invalid or corrupt SQLite database {}", db_path.display()))?;
        Self::run_migrations(&mut conn)
            .with_context(|| format!("failed migrating SQLite database {}", db_path.display()))?;

        let settings_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM instance_settings", [], |row| row.get(0))
            .context("failed checking instance_settings table")?;

        if settings_count == 0 {
            let seed = Self::resolve_initial_seed(legacy_json_path.as_deref(), is_explicit_json)?;
            Self::seed_database(&mut conn, &seed)
                .context("failed seeding initial SQLite configuration")?;
        }

        let config = Self::load_all_from_db(&conn)
            .with_context(|| format!("failed reading configuration from {}", db_path.display()))?;

        Ok(Self {
            db_path,
            conn: Arc::new(Mutex::new(conn)),
            inner: Arc::new(RwLock::new(config)),
        })
    }

    fn configure_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
        )?;
        Ok(())
    }

    fn run_migrations(conn: &mut Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL
            )",
            [],
        )?;

        let current_version: i64 = conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )?;

        if current_version < 1 {
            let tx = conn.transaction()?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS instance_settings (
                    key TEXT PRIMARY KEY,
                    value_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS cameras (
                    id TEXT PRIMARY KEY,
                    sort_order INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    enabled INTEGER NOT NULL,
                    source_json TEXT NOT NULL,
                    ptz_json TEXT NOT NULL,
                    ndi_output_json TEXT NOT NULL,
                    framing_json TEXT NOT NULL,
                    color_json TEXT NOT NULL,
                    vmix_input_ref TEXT,
                    calibration_json TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS camera_presets (
                    camera_id TEXT NOT NULL,
                    slot INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    PRIMARY KEY (camera_id, slot),
                    FOREIGN KEY (camera_id) REFERENCES cameras(id) ON DELETE CASCADE
                );",
            )?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![1_i64, Utc::now().to_rfc3339()],
            )?;
            tx.commit()?;
        }

        Ok(())
    }

    fn resolve_initial_seed(legacy_json: Option<&Path>, strict: bool) -> Result<AppConfig> {
        if let Some(json_path) = legacy_json {
            if json_path.exists() {
                let raw_result = std::fs::read_to_string(json_path).with_context(|| {
                    format!("failed reading legacy config file {}", json_path.display())
                });
                let parsed = raw_result.and_then(|raw| {
                    serde_json::from_str::<AppConfig>(&raw).with_context(|| {
                        format!("failed parsing legacy config file {}", json_path.display())
                    })
                });
                match parsed {
                    Ok(mut cfg) => {
                        cfg.schema_version = CURRENT_SCHEMA_VERSION as u32;
                        info!(path = %json_path.display(), "Imported legacy JSON config into SQLite database");
                        return Ok(cfg);
                    }
                    Err(err) if strict => return Err(err),
                    Err(err) => {
                        warn!(
                            path = %json_path.display(),
                            "Legacy config.json could not be imported ({err}); initializing defaults"
                        );
                    }
                }
            }
        }
        Ok(AppConfig::default())
    }

    fn seed_database(conn: &mut Connection, cfg: &AppConfig) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let tx = conn.transaction()?;

        Self::put_setting_tx(&tx, "server", &cfg.server, &now)?;
        Self::put_setting_tx(&tx, "vmix", &cfg.vmix, &now)?;
        Self::put_setting_tx(&tx, "stream_deck", &cfg.stream_deck, &now)?;
        Self::put_setting_tx(&tx, "bitfocus_buttons", &cfg.bitfocus_buttons, &now)?;

        for (idx, cam) in cfg.cameras.iter().enumerate() {
            Self::upsert_camera_tx(&tx, cam, idx as i64, &now)?;
        }

        tx.commit()?;
        Ok(())
    }

    fn put_setting_tx<T: Serialize>(
        tx: &rusqlite::Transaction<'_>,
        key: &str,
        value: &T,
        now: &str,
    ) -> Result<()> {
        let json = serde_json::to_string(value)?;
        tx.execute(
            "INSERT INTO instance_settings (key, value_json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
            params![key, json, now],
        )?;
        Ok(())
    }

    fn upsert_camera_tx(
        tx: &rusqlite::Transaction<'_>,
        cam: &CameraConfig,
        sort_order: i64,
        now: &str,
    ) -> Result<()> {
        let id_str = cam.id.to_string();
        tx.execute(
            "INSERT INTO cameras (
                id, sort_order, name, enabled,
                source_json, ptz_json, ndi_output_json, framing_json,
                color_json, vmix_input_ref, calibration_json, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
                sort_order = excluded.sort_order,
                name = excluded.name,
                enabled = excluded.enabled,
                source_json = excluded.source_json,
                ptz_json = excluded.ptz_json,
                ndi_output_json = excluded.ndi_output_json,
                framing_json = excluded.framing_json,
                color_json = excluded.color_json,
                vmix_input_ref = excluded.vmix_input_ref,
                calibration_json = excluded.calibration_json,
                updated_at = excluded.updated_at",
            params![
                id_str,
                sort_order,
                cam.name,
                if cam.enabled { 1_i64 } else { 0_i64 },
                serde_json::to_string(&cam.source)?,
                serde_json::to_string(&cam.ptz)?,
                serde_json::to_string(&cam.ndi_output)?,
                serde_json::to_string(&cam.framing)?,
                serde_json::to_string(&cam.color)?,
                cam.vmix.input_ref,
                serde_json::to_string(&cam.calibration)?,
                now,
            ],
        )?;

        tx.execute(
            "DELETE FROM camera_presets WHERE camera_id = ?1",
            params![id_str],
        )?;
        for preset in &cam.presets {
            tx.execute(
                "INSERT INTO camera_presets (camera_id, slot, name) VALUES (?1, ?2, ?3)",
                params![id_str, preset.slot as i64, preset.name],
            )?;
        }
        Ok(())
    }

    fn load_setting<T: for<'de> Deserialize<'de> + Default>(
        conn: &Connection,
        key: &str,
    ) -> Result<T> {
        let mut stmt =
            conn.prepare("SELECT value_json FROM instance_settings WHERE key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            let raw: String = row.get(0)?;
            let parsed = serde_json::from_str::<T>(&raw)
                .with_context(|| format!("failed deserializing setting '{key}'"))?;
            Ok(parsed)
        } else {
            Ok(T::default())
        }
    }

    fn load_all_from_db(conn: &Connection) -> Result<AppConfig> {
        let schema_version: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 1) FROM schema_migrations",
                [],
                |row| row.get::<_, i64>(0),
            )?
            .max(1) as u32;

        let server: ServerConfig = Self::load_setting(conn, "server")?;
        let vmix: VmixConfig = Self::load_setting(conn, "vmix")?;
        let stream_deck: StreamDeckConfig = Self::load_setting(conn, "stream_deck")?;
        let bitfocus_buttons: BitfocusButtonsConfig =
            Self::load_setting(conn, "bitfocus_buttons")?;

        let mut cam_stmt = conn.prepare(
            "SELECT id, name, enabled, source_json, ptz_json, ndi_output_json,
                    framing_json, color_json, vmix_input_ref, calibration_json
             FROM cameras
             ORDER BY sort_order ASC, rowid ASC",
        )?;
        let mut preset_stmt = conn.prepare(
            "SELECT slot, name FROM camera_presets WHERE camera_id = ?1 ORDER BY slot ASC",
        )?;

        let mut cameras = Vec::new();
        let mut cam_rows = cam_stmt.query([])?;
        while let Some(row) = cam_rows.next()? {
            let id_str: String = row.get(0)?;
            let id = Uuid::parse_str(&id_str)
                .with_context(|| format!("invalid camera UUID in SQLite: '{id_str}'"))?;
            let name: String = row.get(1)?;
            let enabled = row.get::<_, i64>(2)? != 0;
            let source: VideoSourceConfig = serde_json::from_str(&row.get::<_, String>(3)?)
                .with_context(|| format!("invalid source_json for camera {id}"))?;
            let ptz: PtzConfig = serde_json::from_str(&row.get::<_, String>(4)?)
                .with_context(|| format!("invalid ptz_json for camera {id}"))?;
            let ndi_output: NdiOutputConfig = serde_json::from_str(&row.get::<_, String>(5)?)
                .with_context(|| format!("invalid ndi_output_json for camera {id}"))?;
            let framing: FramingConfig = serde_json::from_str(&row.get::<_, String>(6)?)
                .with_context(|| format!("invalid framing_json for camera {id}"))?;
            let color: ColorAdjustments = serde_json::from_str(&row.get::<_, String>(7)?)
                .with_context(|| format!("invalid color_json for camera {id}"))?;
            let vmix_input_ref: Option<String> = row.get(8)?;
            let calibration: CameraCalibration =
                serde_json::from_str(&row.get::<_, String>(9)?)
                    .with_context(|| format!("invalid calibration_json for camera {id}"))?;

            let mut presets = Vec::new();
            let mut preset_rows = preset_stmt.query(params![id_str])?;
            while let Some(prow) = preset_rows.next()? {
                let slot: i64 = prow.get(0)?;
                let pname: String = prow.get(1)?;
                presets.push(CameraPreset {
                    slot: slot.clamp(0, u8::MAX as i64) as u8,
                    name: pname,
                });
            }

            cameras.push(CameraConfig {
                id,
                name,
                enabled,
                source,
                ptz,
                ndi_output,
                framing,
                color,
                vmix: VmixBinding {
                    input_ref: vmix_input_ref,
                },
                presets,
                calibration,
            });
        }

        Ok(AppConfig {
            schema_version,
            server,
            vmix,
            stream_deck,
            bitfocus_buttons,
            cameras,
        })
    }

    fn with_conn<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut Connection) -> Result<R>,
    {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow!("SQLite connection mutex poisoned"))?;
        f(&mut conn)
    }

    pub async fn snapshot(&self) -> AppConfig {
        self.inner.read().await.clone()
    }

    pub async fn list_cameras(&self) -> Vec<CameraConfig> {
        self.inner.read().await.cameras.clone()
    }

    pub async fn get_camera(&self, id: CameraId) -> Option<CameraConfig> {
        self.inner
            .read()
            .await
            .cameras
            .iter()
            .find(|c| c.id == id)
            .cloned()
    }

    pub async fn upsert_camera(&self, camera: CameraConfig) -> Result<CameraConfig> {
        let mut guard = self.inner.write().await;
        let sort_order = guard
            .cameras
            .iter()
            .position(|c| c.id == camera.id)
            .unwrap_or(guard.cameras.len()) as i64;
        let now = Utc::now().to_rfc3339();

        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            Self::upsert_camera_tx(&tx, &camera, sort_order, &now)?;
            tx.commit()?;
            Ok(())
        })?;

        if let Some(existing) = guard.cameras.iter_mut().find(|c| c.id == camera.id) {
            *existing = camera.clone();
        } else {
            guard.cameras.push(camera.clone());
        }
        Ok(camera)
    }

    pub async fn delete_camera(&self, id: CameraId) -> Result<bool> {
        let mut guard = self.inner.write().await;
        let id_str = id.to_string();
        let rows_deleted = self.with_conn(|conn| {
            let n = conn.execute("DELETE FROM cameras WHERE id = ?1", params![id_str])?;
            Ok(n)
        })?;

        let removed = rows_deleted > 0;
        if removed {
            guard.cameras.retain(|c| c.id != id);
        }
        Ok(removed)
    }

    pub async fn update_framing(
        &self,
        id: CameraId,
        framing: FramingConfig,
    ) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) else {
            return Ok(None);
        };
        let json = serde_json::to_string(&framing)?;
        let now = Utc::now().to_rfc3339();
        let id_str = id.to_string();

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE cameras SET framing_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now, id_str],
            )?;
            Ok(())
        })?;

        cam.framing = framing;
        Ok(Some(cam.clone()))
    }

    pub async fn update_color(
        &self,
        id: CameraId,
        color: ColorAdjustments,
    ) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) else {
            return Ok(None);
        };
        let json = serde_json::to_string(&color)?;
        let now = Utc::now().to_rfc3339();
        let id_str = id.to_string();

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE cameras SET color_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now, id_str],
            )?;
            Ok(())
        })?;

        cam.color = color;
        Ok(Some(cam.clone()))
    }

    pub async fn update_ndi_output(
        &self,
        id: CameraId,
        ndi_output: NdiOutputConfig,
    ) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) else {
            return Ok(None);
        };
        let json = serde_json::to_string(&ndi_output)?;
        let now = Utc::now().to_rfc3339();
        let id_str = id.to_string();

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE cameras SET ndi_output_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now, id_str],
            )?;
            Ok(())
        })?;

        cam.ndi_output = ndi_output;
        Ok(Some(cam.clone()))
    }

    pub async fn update_vmix_binding(
        &self,
        id: CameraId,
        input_ref: Option<String>,
    ) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) else {
            return Ok(None);
        };
        let now = Utc::now().to_rfc3339();
        let id_str = id.to_string();

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE cameras SET vmix_input_ref = ?1, updated_at = ?2 WHERE id = ?3",
                params![input_ref, now, id_str],
            )?;
            Ok(())
        })?;

        cam.vmix.input_ref = input_ref;
        Ok(Some(cam.clone()))
    }

    pub async fn update_calibration(
        &self,
        id: CameraId,
        calibration: CameraCalibration,
    ) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) else {
            return Ok(None);
        };
        let json = serde_json::to_string(&calibration)?;
        let now = Utc::now().to_rfc3339();
        let id_str = id.to_string();

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE cameras SET calibration_json = ?1, updated_at = ?2 WHERE id = ?3",
                params![json, now, id_str],
            )?;
            Ok(())
        })?;

        cam.calibration = calibration;
        Ok(Some(cam.clone()))
    }

    pub async fn update_vmix_config(&self, vmix: VmixConfig) -> Result<VmixConfig> {
        let mut guard = self.inner.write().await;
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            Self::put_setting_tx(&tx, "vmix", &vmix, &now)?;
            tx.commit()?;
            Ok(())
        })?;
        guard.vmix = vmix.clone();
        Ok(vmix)
    }

    pub async fn update_stream_deck_config(
        &self,
        sd: StreamDeckConfig,
    ) -> Result<StreamDeckConfig> {
        let mut guard = self.inner.write().await;
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            Self::put_setting_tx(&tx, "stream_deck", &sd, &now)?;
            tx.commit()?;
            Ok(())
        })?;
        guard.stream_deck = sd.clone();
        Ok(sd)
    }

    pub async fn update_bitfocus_buttons_config(
        &self,
        bf: BitfocusButtonsConfig,
    ) -> Result<BitfocusButtonsConfig> {
        let mut guard = self.inner.write().await;
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            Self::put_setting_tx(&tx, "bitfocus_buttons", &bf, &now)?;
            tx.commit()?;
            Ok(())
        })?;
        guard.bitfocus_buttons = bf.clone();
        Ok(bf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{FramingPreset, RuleOfThirds};

    #[tokio::test]
    async fn sqlite_store_roundtrip_crud_and_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("videocontroller.db");

        let store = ConfigStore::load_or_create(&db_path).unwrap();
        assert_eq!(store.db_path(), db_path.as_path());
        let initial = store.list_cameras().await;
        assert_eq!(initial.len(), 2);

        let mut added = CameraConfig::new_demo("Cam 3 — Rear", "snow");
        added.framing.preset = FramingPreset::CloseUp;
        added.framing.rule_of_thirds = RuleOfThirds::LeftThird;
        let id = added.id;
        store.upsert_camera(added.clone()).await.unwrap();

        let cal = CameraCalibration {
            pan_scale: 0.92,
            tilt_scale: 0.88,
            zoom_scale: 0.95,
            latency_ms: 145.0,
            min_effective_speed: 0.04,
        };
        store.update_calibration(id, cal.clone()).await.unwrap();

        let vmix_cfg = VmixConfig {
            enabled: true,
            host: "10.0.0.50".into(),
            http_port: 8088,
            tcp_port: 8099,
        };
        store.update_vmix_config(vmix_cfg.clone()).await.unwrap();

        // Reload fresh connection from SQLite file to verify durable persistence
        let reloaded = ConfigStore::load_or_create(&db_path).unwrap();
        let fetched = reloaded.get_camera(id).await.unwrap();
        assert_eq!(fetched.name, "Cam 3 — Rear");
        assert_eq!(fetched.framing.preset, FramingPreset::CloseUp);
        assert_eq!(fetched.framing.rule_of_thirds, RuleOfThirds::LeftThird);
        assert_eq!(fetched.calibration, cal);
        assert_eq!(fetched.presets.len(), 4);
        assert_eq!(reloaded.snapshot().await.vmix, vmix_cfg);

        // Deleting camera cascades to camera_presets cleanly
        assert!(reloaded.delete_camera(id).await.unwrap());
        assert!(reloaded.get_camera(id).await.is_none());

        let conn = Connection::open(&db_path).unwrap();
        let preset_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM camera_presets WHERE camera_id = ?1",
                params![id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(preset_count, 0, "ON DELETE CASCADE must remove orphan presets");
    }

    #[tokio::test]
    async fn auto_imports_legacy_config_json_into_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_json = dir.path().join("config.json");
        let db_path = dir.path().join("videocontroller.db");

        let mut legacy = AppConfig::default();
        legacy.server.http_port = 8899;
        legacy.cameras = vec![CameraConfig::new_demo("Legacy Cam", "smpte")];
        std::fs::write(&legacy_json, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        let store = ConfigStore::load_or_create(&db_path).unwrap();
        assert!(db_path.exists());
        let snap = store.snapshot().await;
        assert_eq!(snap.server.http_port, 8899);
        assert_eq!(snap.cameras.len(), 1);
        assert_eq!(snap.cameras[0].name, "Legacy Cam");
    }

    #[tokio::test]
    async fn rejects_corrupt_sqlite_database_file() {
        let dir = tempfile::tempdir().unwrap();
        let corrupt_db = dir.path().join("corrupt.db");
        std::fs::write(&corrupt_db, b"this is definitely not a valid sqlite database header!")
            .unwrap();

        let err = ConfigStore::load_or_create(&corrupt_db)
            .err()
            .expect("corrupt SQLite file must fail to open");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("corrupt") || msg.contains("not a database"),
            "unexpected error message: {msg}"
        );
    }

    #[tokio::test]
    async fn rejects_explicit_corrupt_json_seed_but_recovers_from_corrupt_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let broken_json = dir.path().join("broken.json");
        std::fs::write(&broken_json, "{ not valid json !!!").unwrap();

        // Explicit .json path must return an error
        assert!(
            ConfigStore::load_or_create(&broken_json).is_err(),
            "explicit corrupt JSON seed must return Err"
        );

        // Implicit corrupt sibling config.json next to videocontroller.db should warn & fall back to defaults
        let dir2 = tempfile::tempdir().unwrap();
        std::fs::write(dir2.path().join("config.json"), "{ broken sibling json").unwrap();
        let db_path = dir2.path().join("videocontroller.db");
        let store = ConfigStore::load_or_create(&db_path).unwrap();
        assert_eq!(store.list_cameras().await.len(), 2);
    }

    #[tokio::test]
    async fn nonexistent_camera_mutations_return_none_or_false_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::load_or_create(dir.path().join("test.db")).unwrap();
        let missing_id = Uuid::new_v4();

        assert!(store.get_camera(missing_id).await.is_none());
        assert!(!store.delete_camera(missing_id).await.unwrap());
        assert!(store
            .update_framing(missing_id, FramingConfig::default())
            .await
            .unwrap()
            .is_none());
        assert!(store
            .update_color(missing_id, ColorAdjustments::default())
            .await
            .unwrap()
            .is_none());
        assert!(store
            .update_ndi_output(missing_id, NdiOutputConfig::default())
            .await
            .unwrap()
            .is_none());
        assert!(store
            .update_vmix_binding(missing_id, Some("1".into()))
            .await
            .unwrap()
            .is_none());
        assert!(store
            .update_calibration(missing_id, CameraCalibration::default())
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn detects_corrupt_camera_row_json_on_reload() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("bad_row.db");
        let store = ConfigStore::load_or_create(&db_path).unwrap();
        let cam_id = store.list_cameras().await[0].id;
        drop(store);

        // Manually corrupt framing_json directly in SQLite
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE cameras SET framing_json = '{bad_json' WHERE id = ?1",
            params![cam_id.to_string()],
        )
        .unwrap();
        drop(conn);

        let err = ConfigStore::load_or_create(&db_path)
            .err()
            .expect("corrupt row JSON must return Err on reload");
        assert!(
            format!("{err:#}").contains("framing_json"),
            "error should identify corrupt column: {err:#}"
        );
    }
}
