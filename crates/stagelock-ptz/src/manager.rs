use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use stagelock_core::{CameraConfig, CameraId, PtzConfig};
use stagelock_media::MediaEngine;

use crate::driver::PtzDriver;
use crate::http_drivers::{CanonXcDriver, PanasonicAwDriver};
use crate::mock::MockPtzDriver;
use crate::ndi_ptz::NdiPtzDriver;
use crate::onvif::OnvifDriver;
use crate::visca::{ViscaIpDriver, ViscaTransport};

#[derive(Clone)]
pub struct PtzManager {
    media: MediaEngine,
    drivers: Arc<RwLock<HashMap<CameraId, Arc<dyn PtzDriver>>>>,
}

impl PtzManager {
    pub fn new(media: MediaEngine) -> Self {
        Self {
            media,
            drivers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn configure_camera(&self, camera: &CameraConfig) {
        let driver: Option<Arc<dyn PtzDriver>> = match &camera.ptz {
            PtzConfig::None => None,
            PtzConfig::Mock => Some(Arc::new(MockPtzDriver::new())),
            PtzConfig::Ndi => Some(Arc::new(NdiPtzDriver::new(camera.id, self.media.clone()))),
            PtzConfig::ViscaIpSony { host, port } => Some(Arc::new(ViscaIpDriver::new(
                host.clone(),
                *port,
                ViscaTransport::SonyUdp,
            ))),
            PtzConfig::ViscaIpRawTcp { host, port } => Some(Arc::new(ViscaIpDriver::new(
                host.clone(),
                *port,
                ViscaTransport::RawTcp,
            ))),
            PtzConfig::ViscaIpRawUdp { host, port } => Some(Arc::new(ViscaIpDriver::new(
                host.clone(),
                *port,
                ViscaTransport::RawUdp,
            ))),
            PtzConfig::Onvif {
                host,
                port,
                username,
                password,
                profile_token,
            } => Some(Arc::new(OnvifDriver::new(
                host.clone(),
                *port,
                username.clone(),
                password.clone(),
                profile_token.clone(),
            ))),
            PtzConfig::CanonXc {
                host,
                port,
                username,
                password,
            } => Some(Arc::new(CanonXcDriver::new(
                host.clone(),
                *port,
                username.clone(),
                password.clone(),
            ))),
            PtzConfig::PanasonicAw { host, port } => {
                Some(Arc::new(PanasonicAwDriver::new(host.clone(), *port)))
            }
        };

        let mut map = self.drivers.write().await;
        if let Some(drv) = driver {
            map.insert(camera.id, drv);
        } else {
            map.remove(&camera.id);
        }
    }

    pub async fn remove_camera(&self, id: CameraId) {
        self.drivers.write().await.remove(&id);
    }

    pub async fn get(&self, id: CameraId) -> Option<Arc<dyn PtzDriver>> {
        self.drivers.read().await.get(&id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manager_configures_every_ptz_protocol_and_removes_cleanly() {
        let mgr = PtzManager::new(MediaEngine::new());
        let variants: Vec<(PtzConfig, Option<&str>)> = vec![
            (PtzConfig::Mock, Some("mock")),
            (PtzConfig::Ndi, Some("ndi_ptz")),
            (PtzConfig::ViscaIpSony { host: "127.0.0.1".into(), port: 52381 }, Some("visca_ip_sony")),
            (PtzConfig::ViscaIpRawTcp { host: "127.0.0.1".into(), port: 5678 }, Some("visca_ip_raw_tcp")),
            (PtzConfig::ViscaIpRawUdp { host: "127.0.0.1".into(), port: 1259 }, Some("visca_ip_raw_udp")),
            (
                PtzConfig::Onvif {
                    host: "127.0.0.1".into(),
                    port: 80,
                    username: "admin".into(),
                    password: "pass".into(),
                    profile_token: Some("P1".into()),
                },
                Some("onvif"),
            ),
            (
                PtzConfig::CanonXc {
                    host: "127.0.0.1".into(),
                    port: 80,
                    username: String::new(),
                    password: String::new(),
                },
                Some("canon_xc"),
            ),
            (
                PtzConfig::PanasonicAw { host: "127.0.0.1".into(), port: 80 },
                Some("panasonic_aw"),
            ),
            (PtzConfig::None, None),
        ];

        let mut cam = CameraConfig::new_demo("Cam", "smpte");
        for (ptz, expected) in variants {
            cam.ptz = ptz;
            mgr.configure_camera(&cam).await;
            let got = mgr.get(cam.id).await.map(|d| d.protocol_name());
            assert_eq!(got, expected);
        }

        cam.ptz = PtzConfig::Mock;
        mgr.configure_camera(&cam).await;
        assert!(mgr.get(cam.id).await.is_some());
        mgr.remove_camera(cam.id).await;
        assert!(mgr.get(cam.id).await.is_none());
    }
}
