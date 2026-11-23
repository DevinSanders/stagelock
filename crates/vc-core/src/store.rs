use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::camera::{CameraConfig, CameraId, ColorAdjustments, FramingConfig, NdiOutputConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
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

#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
    inner: Arc<RwLock<AppConfig>>,
}

impl ConfigStore {
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("VideoController")
            .join("config.json")
    }

    pub fn load_or_create(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let config = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("failed reading config file {}", path.display()))?;
            serde_json::from_str::<AppConfig>(&raw)
                .with_context(|| format!("failed parsing config file {}", path.display()))?
        } else {
            let cfg = AppConfig::default();
            Self::write_atomic(&path, &cfg)?;
            cfg
        };

        Ok(Self {
            path,
            inner: Arc::new(RwLock::new(config)),
        })
    }

    fn write_atomic(path: &Path, config: &AppConfig) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp_path = path.with_extension(format!("json.tmp.{}", Uuid::new_v4()));
        let serialized = serde_json::to_string_pretty(config)?;
        std::fs::write(&tmp_path, serialized)?;
        std::fs::rename(&tmp_path, path)?;
        Ok(())
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
        if let Some(existing) = guard.cameras.iter_mut().find(|c| c.id == camera.id) {
            *existing = camera.clone();
        } else {
            guard.cameras.push(camera.clone());
        }
        Self::write_atomic(&self.path, &guard)?;
        Ok(camera)
    }

    pub async fn delete_camera(&self, id: CameraId) -> Result<bool> {
        let mut guard = self.inner.write().await;
        let prev_len = guard.cameras.len();
        guard.cameras.retain(|c| c.id != id);
        let removed = guard.cameras.len() != prev_len;
        if removed {
            Self::write_atomic(&self.path, &guard)?;
        }
        Ok(removed)
    }

    pub async fn update_framing(&self, id: CameraId, framing: FramingConfig) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let updated = if let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) {
            cam.framing = framing;
            Some(cam.clone())
        } else {
            None
        };
        if updated.is_some() {
            Self::write_atomic(&self.path, &guard)?;
        }
        Ok(updated)
    }

    pub async fn update_color(&self, id: CameraId, color: ColorAdjustments) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let updated = if let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) {
            cam.color = color;
            Some(cam.clone())
        } else {
            None
        };
        if updated.is_some() {
            Self::write_atomic(&self.path, &guard)?;
        }
        Ok(updated)
    }

    pub async fn update_ndi_output(&self, id: CameraId, ndi_output: NdiOutputConfig) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let updated = if let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) {
            cam.ndi_output = ndi_output;
            Some(cam.clone())
        } else {
            None
        };
        if updated.is_some() {
            Self::write_atomic(&self.path, &guard)?;
        }
        Ok(updated)
    }

    pub async fn update_vmix_binding(&self, id: CameraId, input_ref: Option<String>) -> Result<Option<CameraConfig>> {
        let mut guard = self.inner.write().await;
        let updated = if let Some(cam) = guard.cameras.iter_mut().find(|c| c.id == id) {
            cam.vmix.input_ref = input_ref;
            Some(cam.clone())
        } else {
            None
        };
        if updated.is_some() {
            Self::write_atomic(&self.path, &guard)?;
        }
        Ok(updated)
    }

    pub async fn update_vmix_config(&self, vmix: VmixConfig) -> Result<VmixConfig> {
        let mut guard = self.inner.write().await;
        guard.vmix = vmix.clone();
        Self::write_atomic(&self.path, &guard)?;
        Ok(vmix)
    }

    pub async fn update_stream_deck_config(&self, sd: StreamDeckConfig) -> Result<StreamDeckConfig> {
        let mut guard = self.inner.write().await;
        guard.stream_deck = sd.clone();
        Self::write_atomic(&self.path, &guard)?;
        Ok(sd)
    }

    pub async fn update_bitfocus_buttons_config(&self, bf: BitfocusButtonsConfig) -> Result<BitfocusButtonsConfig> {
        let mut guard = self.inner.write().await;
        guard.bitfocus_buttons = bf.clone();
        Self::write_atomic(&self.path, &guard)?;
        Ok(bf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{FramingPreset, RuleOfThirds};

    #[tokio::test]
    async fn config_store_roundtrip_and_crud() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("config.json");

        let store = ConfigStore::load_or_create(&cfg_path).unwrap();
        let initial = store.list_cameras().await;
        assert_eq!(initial.len(), 2);

        let mut added = CameraConfig::new_demo("Cam 3 — Rear", "snow");
        added.framing.preset = FramingPreset::CloseUp;
        added.framing.rule_of_thirds = RuleOfThirds::LeftThird;
        let id = added.id;
        store.upsert_camera(added.clone()).await.unwrap();

        // Reload from disk to verify atomic persistence
        let reloaded = ConfigStore::load_or_create(&cfg_path).unwrap();
        let fetched = reloaded.get_camera(id).await.unwrap();
        assert_eq!(fetched.name, "Cam 3 — Rear");
        assert_eq!(fetched.framing.preset, FramingPreset::CloseUp);
        assert_eq!(fetched.framing.rule_of_thirds, RuleOfThirds::LeftThird);

        assert!(reloaded.delete_camera(id).await.unwrap());
        assert!(reloaded.get_camera(id).await.is_none());
    }
}
