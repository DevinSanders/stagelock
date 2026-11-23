use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use vc_core::{CameraConfig, CameraId, PtzConfig};
use vc_media::MediaEngine;

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
