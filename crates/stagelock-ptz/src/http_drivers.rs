//! Extensible HTTP/CGI PTZ drivers for broadcast camera families:
//! - Canon XC Protocol (CR-N100 / CR-N300 / CR-N500 / CR-N700)
//! - Panasonic AW Protocol (AW-UE / AW-HE series)

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use reqwest::Client;

use crate::driver::{FocusCommand, ImagingCommand, PtzDriver};

/// Canon XC Protocol over HTTP/CGI (`/-wvhttp-01-/control.cgi`).
pub struct CanonXcDriver {
    base_url: String,
    username: String,
    password: String,
    client: Client,
}

impl CanonXcDriver {
    pub fn new(host: impl Into<String>, port: u16, username: impl Into<String>, password: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            base_url: format!("http://{}:{}/-wvhttp-01-/control.cgi", host.into(), port),
            username: username.into(),
            password: password.into(),
            client,
        }
    }

    async fn send_params(&self, params: &[(&str, String)]) -> Result<()> {
        let mut req = self.client.get(&self.base_url).query(params);
        if !self.username.is_empty() {
            req = req.basic_auth(&self.username, Some(&self.password));
        }
        req.send().await?.error_for_status()?;
        Ok(())
    }
}

#[async_trait]
impl PtzDriver for CanonXcDriver {
    fn protocol_name(&self) -> &'static str {
        "canon_xc"
    }

    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()> {
        let p_dir = if pan.abs() < 0.02 { "stop" } else if pan < 0.0 { "left" } else { "right" };
        let t_dir = if tilt.abs() < 0.02 { "stop" } else if tilt > 0.0 { "up" } else { "down" };
        let p_spd = ((pan.abs().clamp(0.0, 1.0) * 10000.0).round() as u32).max(100);
        let t_spd = ((tilt.abs().clamp(0.0, 1.0) * 10000.0).round() as u32).max(100);
        self.send_params(&[
            ("pan", p_dir.to_string()),
            ("pan.speed.dir", p_spd.to_string()),
            ("tilt", t_dir.to_string()),
            ("tilt.speed.dir", t_spd.to_string()),
        ])
        .await
    }

    async fn zoom_speed(&self, zoom: f32) -> Result<()> {
        let dir = if zoom.abs() < 0.02 { "stop" } else if zoom > 0.0 { "tele" } else { "wide" };
        let spd = ((zoom.abs().clamp(0.0, 1.0) * 15.0).round() as u32).clamp(1, 15);
        self.send_params(&[("zoom", dir.to_string()), ("zoom.speed.dir", spd.to_string())])
            .await
    }

    async fn stop(&self) -> Result<()> {
        self.send_params(&[
            ("pan", "stop".into()),
            ("tilt", "stop".into()),
            ("zoom", "stop".into()),
        ])
        .await
    }

    async fn focus(&self, cmd: FocusCommand) -> Result<()> {
        let params: Vec<(&str, String)> = match cmd {
            FocusCommand::Auto => vec![("f", "auto".into())],
            FocusCommand::Manual => vec![("f", "manual".into())],
            FocusCommand::OnePush => vec![("f", "one_shot".into())],
            FocusCommand::Near { .. } => vec![("f.action", "near".into())],
            FocusCommand::Far { .. } => vec![("f.action", "far".into())],
            FocusCommand::Stop => vec![("f.action", "stop".into())],
        };
        self.send_params(&params).await
    }

    async fn recall_preset(&self, slot: u8) -> Result<()> {
        self.send_params(&[("p", slot.max(1).to_string())]).await
    }

    async fn store_preset(&self, slot: u8) -> Result<()> {
        self.send_params(&[("p.set", slot.max(1).to_string())]).await
    }

    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()> {
        let params: Vec<(&str, String)> = match cmd {
            ImagingCommand::WhiteBalanceAuto => vec![("wb", "auto".into())],
            ImagingCommand::WhiteBalanceIndoor => vec![("wb", "tungsten".into())],
            ImagingCommand::WhiteBalanceOutdoor => vec![("wb", "daylight".into())],
            ImagingCommand::WhiteBalanceOnePush => vec![("wb.action", "one_shot".into())],
            ImagingCommand::ExposureAuto => vec![("exp", "auto".into())],
            ImagingCommand::ExposureManual { .. } => vec![("exp", "manual".into())],
            _ => return Ok(()),
        };
        self.send_params(&params).await
    }
}

/// Panasonic AW HTTP/CGI protocol (`/cgi-bin/aw_ptz` & `/cgi-bin/aw_cam`).
pub struct PanasonicAwDriver {
    host: String,
    port: u16,
    client: Client,
}

impl PanasonicAwDriver {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            host: host.into(),
            port,
            client,
        }
    }

    /// Encode normalized speed (-1.0..1.0) into Panasonic AW 01..99 speed (50 = stop).
    pub fn encode_aw_speed(val: f32) -> u8 {
        let v = val.clamp(-1.0, 1.0);
        if v.abs() < 0.02 {
            50
        } else {
            (50.0 + v * 49.0).round().clamp(1.0, 99.0) as u8
        }
    }

    async fn send_ptz(&self, cmd: &str) -> Result<()> {
        let url = format!("http://{}:{}/cgi-bin/aw_ptz?cmd={cmd}&res=1", self.host, self.port);
        self.client.get(&url).send().await?.error_for_status()?;
        Ok(())
    }

    async fn send_cam(&self, cmd: &str) -> Result<()> {
        let url = format!("http://{}:{}/cgi-bin/aw_cam?cmd={cmd}&res=1", self.host, self.port);
        self.client.get(&url).send().await?.error_for_status()?;
        Ok(())
    }
}

#[async_trait]
impl PtzDriver for PanasonicAwDriver {
    fn protocol_name(&self) -> &'static str {
        "panasonic_aw"
    }

    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()> {
        let p = Self::encode_aw_speed(pan);
        let t = Self::encode_aw_speed(tilt);
        self.send_ptz(&format!("%23PTS{p:02}{t:02}")).await
    }

    async fn zoom_speed(&self, zoom: f32) -> Result<()> {
        let z = Self::encode_aw_speed(zoom);
        self.send_ptz(&format!("%23Z{z:02}")).await
    }

    async fn stop(&self) -> Result<()> {
        self.send_ptz("%23PTS5050").await?;
        self.send_ptz("%23Z50").await?;
        Ok(())
    }

    async fn focus(&self, cmd: FocusCommand) -> Result<()> {
        match cmd {
            FocusCommand::Auto => self.send_ptz("%23D11").await,
            FocusCommand::Manual => self.send_ptz("%23D10").await,
            FocusCommand::OnePush => self.send_cam("OSE:69:1").await,
            FocusCommand::Near { speed } => {
                let s = Self::encode_aw_speed(-speed.abs());
                self.send_ptz(&format!("%23F{s:02}")).await
            }
            FocusCommand::Far { speed } => {
                let s = Self::encode_aw_speed(speed.abs());
                self.send_ptz(&format!("%23F{s:02}")).await
            }
            FocusCommand::Stop => self.send_ptz("%23F50").await,
        }
    }

    async fn recall_preset(&self, slot: u8) -> Result<()> {
        self.send_ptz(&format!("%23R{:02}", slot.min(99))).await
    }

    async fn store_preset(&self, slot: u8) -> Result<()> {
        self.send_ptz(&format!("%23M{:02}", slot.min(99))).await
    }

    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()> {
        match cmd {
            ImagingCommand::WhiteBalanceAuto => self.send_cam("OAW:0").await,
            ImagingCommand::WhiteBalanceIndoor => self.send_cam("OAW:4").await,
            ImagingCommand::WhiteBalanceOutdoor => self.send_cam("OAW:5").await,
            ImagingCommand::WhiteBalanceOnePush => self.send_cam("OWS").await,
            ImagingCommand::ExposureAuto => self.send_ptz("%23D31").await,
            ImagingCommand::ExposureManual { .. } => self.send_ptz("%23D30").await,
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panasonic_aw_speed_encoding_centered_at_50() {
        assert_eq!(PanasonicAwDriver::encode_aw_speed(0.0), 50);
        assert_eq!(PanasonicAwDriver::encode_aw_speed(1.0), 99);
        assert_eq!(PanasonicAwDriver::encode_aw_speed(-1.0), 1);
    }

    #[test]
    fn panasonic_aw_extreme_and_deadband_speeds_clamp_safely() {
        assert_eq!(PanasonicAwDriver::encode_aw_speed(50.0), 99);
        assert_eq!(PanasonicAwDriver::encode_aw_speed(-50.0), 1);
        assert_eq!(PanasonicAwDriver::encode_aw_speed(0.01), 50);
        assert_eq!(PanasonicAwDriver::encode_aw_speed(-0.015), 50);
    }

    async fn spawn_http_mock(status: &'static str) -> (u16, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let _ = stream.read(&mut buf).await;
                    let resp = format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK");
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn canon_xc_and_panasonic_aw_cover_all_commands_and_http_errors() {
        let (ok_port, ok_srv) = spawn_http_mock("200 OK").await;
        let (err_port, err_srv) = spawn_http_mock("500 Internal Server Error").await;

        // Canon XC happy path (with basic auth)
        let canon = CanonXcDriver::new("127.0.0.1", ok_port, "admin", "secret");
        assert_eq!(canon.protocol_name(), "canon_xc");
        canon.pan_tilt_speed(0.6, -0.4).await.unwrap();
        canon.pan_tilt_speed(-0.5, 0.5).await.unwrap();
        canon.pan_tilt_speed(0.0, 0.0).await.unwrap();
        canon.zoom_speed(0.5).await.unwrap();
        canon.zoom_speed(-0.5).await.unwrap();
        canon.zoom_speed(0.0).await.unwrap();
        canon.stop().await.unwrap();
        for f in [
            FocusCommand::Auto,
            FocusCommand::Manual,
            FocusCommand::OnePush,
            FocusCommand::Near { speed: 0.4 },
            FocusCommand::Far { speed: 0.4 },
            FocusCommand::Stop,
        ] {
            canon.focus(f).await.unwrap();
        }
        canon.recall_preset(1).await.unwrap();
        canon.store_preset(2).await.unwrap();
        for img in [
            ImagingCommand::WhiteBalanceAuto,
            ImagingCommand::WhiteBalanceIndoor,
            ImagingCommand::WhiteBalanceOutdoor,
            ImagingCommand::WhiteBalanceOnePush,
            ImagingCommand::ExposureAuto,
            ImagingCommand::ExposureManual { level: 0.5 },
            ImagingCommand::Sharpness { level: 0.5 }, // no-op branch
        ] {
            canon.set_imaging(img).await.unwrap();
        }

        // Canon XC failure path (HTTP 500)
        let canon_err = CanonXcDriver::new("127.0.0.1", err_port, "", "");
        assert!(canon_err.stop().await.is_err());

        // Panasonic AW happy path
        let pana = PanasonicAwDriver::new("127.0.0.1", ok_port);
        assert_eq!(pana.protocol_name(), "panasonic_aw");
        pana.pan_tilt_speed(0.5, -0.5).await.unwrap();
        pana.zoom_speed(0.4).await.unwrap();
        pana.stop().await.unwrap();
        for f in [
            FocusCommand::Auto,
            FocusCommand::Manual,
            FocusCommand::OnePush,
            FocusCommand::Near { speed: 0.5 },
            FocusCommand::Far { speed: 0.5 },
            FocusCommand::Stop,
        ] {
            pana.focus(f).await.unwrap();
        }
        pana.recall_preset(3).await.unwrap();
        pana.store_preset(4).await.unwrap();
        for img in [
            ImagingCommand::WhiteBalanceAuto,
            ImagingCommand::WhiteBalanceIndoor,
            ImagingCommand::WhiteBalanceOutdoor,
            ImagingCommand::WhiteBalanceOnePush,
            ImagingCommand::ExposureAuto,
            ImagingCommand::ExposureManual { level: 0.5 },
            ImagingCommand::Brightness { level: 0.5 }, // no-op branch
        ] {
            pana.set_imaging(img).await.unwrap();
        }

        // Panasonic AW failure path (HTTP 500)
        let pana_err = PanasonicAwDriver::new("127.0.0.1", err_port);
        assert!(pana_err.stop().await.is_err());

        ok_srv.abort();
        err_srv.abort();
    }
}
