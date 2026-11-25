//! ONVIF Profile S / T implementation:
//! - WS-Discovery LAN scanner (UDP 239.255.255.250:3702)
//! - WS-Security UsernameToken PasswordDigest authentication
//! - Device `GetCapabilities`, Media `GetProfiles` + `GetStreamUri`
//! - PTZ `ContinuousMove`, `Stop`, `GotoPreset`, `SetPreset`
//! - Imaging `Move`/`Stop` Focus & `SetImagingSettings` (WB, Exposure, Contrast, Sharpness, Brightness)

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use chrono::Utc;
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::debug;

use crate::driver::{FocusCommand, ImagingCommand, PtzDriver};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifDiscoveredDevice {
    pub xaddr: String,
    pub host: String,
    pub port: u16,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnvifResolvedInfo {
    pub media_xaddr: String,
    pub ptz_xaddr: String,
    pub imaging_xaddr: Option<String>,
    pub profile_token: String,
    pub video_source_token: Option<String>,
    pub rtsp_uri: Option<String>,
}

/// Broadcast an ONVIF WS-Discovery SOAP Probe to `239.255.255.250:3702` and collect LAN cameras.
pub async fn discover_onvif_devices(timeout_ms: u64) -> Vec<OnvifDiscoveredDevice> {
    let Ok(socket) = UdpSocket::bind("0.0.0.0:0").await else {
        return Vec::new();
    };
    let _ = socket.set_broadcast(true);

    let msg_id = format!("urn:uuid:{:016x}", rand::random::<u64>());
    let probe = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope"
            xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing"
            xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery"
            xmlns:dn="http://www.onvif.org/ver10/network/wsdl">
  <e:Header>
    <w:MessageID>{msg_id}</w:MessageID>
    <w:To e:mustUnderstand="true">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To>
    <w:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action>
  </e:Header>
  <e:Body>
    <d:Probe>
      <d:Types>dn:NetworkVideoTransmitter</d:Types>
    </d:Probe>
  </e:Body>
</e:Envelope>"#
    );

    let multicast: SocketAddr = "239.255.255.250:3702".parse().unwrap();
    let _ = socket.send_to(probe.as_bytes(), multicast).await;

    let mut seen = HashSet::new();
    let mut devices = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut buf = vec![0u8; 8192];

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, _peer))) => {
                if let Ok(xml) = std::str::from_utf8(&buf[..len]) {
                    for dev in parse_ws_discovery_response(xml) {
                        if seen.insert(dev.xaddr.clone()) {
                            devices.push(dev);
                        }
                    }
                }
            }
            _ => break,
        }
    }

    devices
}

pub fn parse_ws_discovery_response(xml: &str) -> Vec<OnvifDiscoveredDevice> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for node in doc.descendants().filter(|n| n.tag_name().name() == "ProbeMatch") {
        let xaddrs_text = node
            .descendants()
            .find(|n| n.tag_name().name() == "XAddrs")
            .and_then(|n| n.text())
            .unwrap_or("");
        let scopes_text = node
            .descendants()
            .find(|n| n.tag_name().name() == "Scopes")
            .and_then(|n| n.text())
            .unwrap_or("");

        let model_name = scopes_text
            .split_whitespace()
            .find_map(|s| {
                s.strip_prefix("onvif://www.onvif.org/name/")
                    .or_else(|| s.strip_prefix("onvif://www.onvif.org/hardware/"))
            })
            .map(|s| s.replace("%20", " "))
            .unwrap_or_else(|| "ONVIF Camera".to_string());

        for xaddr in xaddrs_text.split_whitespace() {
            if let Some((host, port)) = parse_http_host_port(xaddr) {
                out.push(OnvifDiscoveredDevice {
                    xaddr: xaddr.to_string(),
                    host,
                    port,
                    name: model_name.clone(),
                });
                break;
            }
        }
    }
    out
}

fn parse_http_host_port(url: &str) -> Option<(String, u16)> {
    let no_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let host_port = no_scheme.split('/').next()?;
    if let Some((h, p)) = host_port.rsplit_once(':') {
        Some((h.to_string(), p.parse().unwrap_or(80)))
    } else {
        Some((host_port.to_string(), 80))
    }
}

/// Build an ONVIF WS-Security UsernameToken PasswordDigest SOAP Header.
pub fn build_wsse_header(username: &str, password: &str) -> String {
    if username.is_empty() {
        return String::new();
    }
    let mut nonce_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let created = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    build_wsse_header_deterministic(username, password, &nonce_bytes, &created)
}

pub fn build_wsse_header_deterministic(
    username: &str,
    password: &str,
    nonce_bytes: &[u8],
    created: &str,
) -> String {
    let mut hasher = Sha1::new();
    hasher.update(nonce_bytes);
    hasher.update(created.as_bytes());
    hasher.update(password.as_bytes());
    let digest = BASE64.encode(hasher.finalize());
    let nonce_b64 = BASE64.encode(nonce_bytes);

    format!(
        r#"<s:Header>
  <wsse:Security s:mustUnderstand="1" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">
    <wsse:UsernameToken>
      <wsse:Username>{username}</wsse:Username>
      <wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{digest}</wsse:Password>
      <wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{nonce_b64}</wsse:Nonce>
      <wsu:Created>{created}</wsu:Created>
    </wsse:UsernameToken>
  </wsse:Security>
</s:Header>"#
    )
}

fn wrap_soap(header: &str, body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"
            xmlns:tds="http://www.onvif.org/ver10/device/wsdl"
            xmlns:trt="http://www.onvif.org/ver10/media/wsdl"
            xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl"
            xmlns:timg="http://www.onvif.org/ver20/imaging/wsdl"
            xmlns:tt="http://www.onvif.org/ver10/schema">
  {header}
  <s:Body>{body}</s:Body>
</s:Envelope>"#
    )
}

async fn send_soap(client: &Client, endpoint: &str, action: &str, envelope: String) -> Result<String> {
    let resp = client
        .post(endpoint)
        .header(
            "Content-Type",
            format!("application/soap+xml; charset=utf-8; action=\"{action}\""),
        )
        .body(envelope)
        .send()
        .await
        .with_context(|| format!("ONVIF SOAP request failed to {endpoint}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!("ONVIF HTTP {status} from {endpoint}: {}", &text[..text.len().min(200)]));
    }
    Ok(text)
}

/// Query ONVIF GetCapabilities + GetProfiles + GetStreamUri to auto-configure video & PTZ.
pub async fn resolve_onvif_camera(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<OnvifResolvedInfo> {
    let client = Client::builder().timeout(Duration::from_secs(4)).build()?;
    let device_url = format!("http://{host}:{port}/onvif/device_service");

    let caps_xml = send_soap(
        &client,
        &device_url,
        "http://www.onvif.org/ver10/device/wsdl/GetCapabilities",
        wrap_soap(
            &build_wsse_header(username, password),
            "<tds:GetCapabilities><tds:Category>All</tds:Category></tds:GetCapabilities>",
        ),
    )
    .await?;

    let (media_xaddr, ptz_xaddr, imaging_xaddr) = parse_capabilities_xaddrs(&caps_xml, host, port);

    let profiles_xml = send_soap(
        &client,
        &media_xaddr,
        "http://www.onvif.org/ver10/media/wsdl/GetProfiles",
        wrap_soap(&build_wsse_header(username, password), "<trt:GetProfiles/>"),
    )
    .await?;

    let (profile_token, video_source_token) = parse_primary_profile(&profiles_xml)
        .ok_or_else(|| anyhow!("ONVIF camera returned no media profiles"))?;

    let stream_uri_body = format!(
        r#"<trt:GetStreamUri>
  <trt:StreamSetup>
    <tt:Stream>RTP-Unicast</tt:Stream>
    <tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport>
  </trt:StreamSetup>
  <trt:ProfileToken>{profile_token}</trt:ProfileToken>
</trt:GetStreamUri>"#
    );

    let rtsp_uri = match send_soap(
        &client,
        &media_xaddr,
        "http://www.onvif.org/ver10/media/wsdl/GetStreamUri",
        wrap_soap(&build_wsse_header(username, password), &stream_uri_body),
    )
    .await
    {
        Ok(xml) => parse_stream_uri(&xml),
        Err(_) => None,
    };

    Ok(OnvifResolvedInfo {
        media_xaddr,
        ptz_xaddr,
        imaging_xaddr,
        profile_token,
        video_source_token,
        rtsp_uri,
    })
}

pub fn parse_capabilities_xaddrs(xml: &str, host: &str, port: u16) -> (String, String, Option<String>) {
    let default_media = format!("http://{host}:{port}/onvif/media_service");
    let default_ptz = format!("http://{host}:{port}/onvif/ptz_service");

    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return (default_media, default_ptz, None);
    };

    let find_xaddr = |section: &str| -> Option<String> {
        doc.descendants()
            .find(|n| n.tag_name().name() == section)
            .and_then(|sec| sec.descendants().find(|n| n.tag_name().name() == "XAddr"))
            .and_then(|n| n.text())
            .map(|s| s.trim().to_string())
    };

    (
        find_xaddr("Media").unwrap_or(default_media),
        find_xaddr("PTZ").unwrap_or(default_ptz),
        find_xaddr("Imaging"),
    )
}

pub fn parse_primary_profile(xml: &str) -> Option<(String, Option<String>)> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let profile = doc.descendants().find(|n| n.tag_name().name() == "Profiles")?;
    let token = profile.attribute("token")?.to_string();
    let video_source_token = profile
        .descendants()
        .find(|n| n.tag_name().name() == "SourceToken")
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string());
    Some((token, video_source_token))
}

pub fn parse_stream_uri(xml: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants()
        .find(|n| n.tag_name().name() == "Uri")
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
}

pub struct OnvifDriver {
    host: String,
    port: u16,
    username: String,
    password: String,
    client: Client,
    resolved: Arc<RwLock<Option<OnvifResolvedInfo>>>,
    fallback_profile: String,
}

impl OnvifDriver {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        username: impl Into<String>,
        password: impl Into<String>,
        profile_token: Option<String>,
    ) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            host: host.into(),
            port,
            username: username.into(),
            password: password.into(),
            client,
            resolved: Arc::new(RwLock::new(None)),
            fallback_profile: profile_token.unwrap_or_else(|| "Profile_1".to_string()),
        }
    }

    async fn endpoints(&self) -> (String, Option<String>, String, Option<String>) {
        if let Some(info) = self.resolved.read().await.clone() {
            return (
                info.ptz_xaddr,
                info.imaging_xaddr,
                info.profile_token,
                info.video_source_token,
            );
        }
        if let Ok(info) = resolve_onvif_camera(&self.host, self.port, &self.username, &self.password).await {
            let res = (
                info.ptz_xaddr.clone(),
                info.imaging_xaddr.clone(),
                info.profile_token.clone(),
                info.video_source_token.clone(),
            );
            *self.resolved.write().await = Some(info);
            return res;
        }
        (
            format!("http://{}:{}/onvif/ptz_service", self.host, self.port),
            Some(format!("http://{}:{}/onvif/imaging_service", self.host, self.port)),
            self.fallback_profile.clone(),
            Some("VideoSource_1".to_string()),
        )
    }
}

#[async_trait]
impl PtzDriver for OnvifDriver {
    fn protocol_name(&self) -> &'static str {
        "onvif"
    }

    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()> {
        let (ptz_url, _, profile, _) = self.endpoints().await;
        let pan = pan.clamp(-1.0, 1.0);
        let tilt = tilt.clamp(-1.0, 1.0);
        let body = format!(
            r#"<tptz:ContinuousMove>
  <tptz:ProfileToken>{profile}</tptz:ProfileToken>
  <tptz:Velocity><tt:PanTilt x="{pan:.3}" y="{tilt:.3}"/></tptz:Velocity>
</tptz:ContinuousMove>"#
        );
        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        debug!(endpoint = %ptz_url, pan, tilt, "ONVIF ContinuousMove PT");
        send_soap(&self.client, &ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove", env).await?;
        Ok(())
    }

    async fn zoom_speed(&self, zoom: f32) -> Result<()> {
        let (ptz_url, _, profile, _) = self.endpoints().await;
        let zoom = zoom.clamp(-1.0, 1.0);
        let body = format!(
            r#"<tptz:ContinuousMove>
  <tptz:ProfileToken>{profile}</tptz:ProfileToken>
  <tptz:Velocity><tt:Zoom x="{zoom:.3}"/></tptz:Velocity>
</tptz:ContinuousMove>"#
        );
        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        send_soap(&self.client, &ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove", env).await?;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let (ptz_url, _, profile, _) = self.endpoints().await;
        let body = format!(
            r#"<tptz:Stop>
  <tptz:ProfileToken>{profile}</tptz:ProfileToken>
  <tptz:PanTilt>true</tptz:PanTilt>
  <tptz:Zoom>true</tptz:Zoom>
</tptz:Stop>"#
        );
        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        send_soap(&self.client, &ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/Stop", env).await?;
        Ok(())
    }

    async fn focus(&self, cmd: FocusCommand) -> Result<()> {
        let (_, img_opt, _, vsrc_opt) = self.endpoints().await;
        let Some(img_url) = img_opt else {
            return Err(anyhow!("ONVIF camera does not advertise an Imaging service"));
        };
        let vsrc = vsrc_opt.unwrap_or_else(|| "VideoSource_1".to_string());

        let (action, body) = match cmd {
            FocusCommand::Auto | FocusCommand::OnePush => (
                "http://www.onvif.org/ver20/imaging/wsdl/SetImagingSettings",
                format!(
                    r#"<timg:SetImagingSettings>
  <timg:VideoSourceToken>{vsrc}</timg:VideoSourceToken>
  <timg:ImagingSettings><tt:Focus><tt:AutoFocusMode>AUTO</tt:AutoFocusMode></tt:Focus></timg:ImagingSettings>
</timg:SetImagingSettings>"#
                ),
            ),
            FocusCommand::Manual => (
                "http://www.onvif.org/ver20/imaging/wsdl/SetImagingSettings",
                format!(
                    r#"<timg:SetImagingSettings>
  <timg:VideoSourceToken>{vsrc}</timg:VideoSourceToken>
  <timg:ImagingSettings><tt:Focus><tt:AutoFocusMode>MANUAL</tt:AutoFocusMode></tt:Focus></timg:ImagingSettings>
</timg:SetImagingSettings>"#
                ),
            ),
            FocusCommand::Near { speed } => (
                "http://www.onvif.org/ver20/imaging/wsdl/Move",
                format!(
                    r#"<timg:Move>
  <timg:VideoSourceToken>{vsrc}</timg:VideoSourceToken>
  <timg:Focus><tt:Continuous><tt:Speed>{:.3}</tt:Speed></tt:Continuous></timg:Focus>
</timg:Move>"#,
                    -speed.abs().clamp(0.05, 1.0)
                ),
            ),
            FocusCommand::Far { speed } => (
                "http://www.onvif.org/ver20/imaging/wsdl/Move",
                format!(
                    r#"<timg:Move>
  <timg:VideoSourceToken>{vsrc}</timg:VideoSourceToken>
  <timg:Focus><tt:Continuous><tt:Speed>{:.3}</tt:Speed></tt:Continuous></timg:Focus>
</timg:Move>"#,
                    speed.abs().clamp(0.05, 1.0)
                ),
            ),
            FocusCommand::Stop => (
                "http://www.onvif.org/ver20/imaging/wsdl/Stop",
                format!(r#"<timg:Stop><timg:VideoSourceToken>{vsrc}</timg:VideoSourceToken></timg:Stop>"#),
            ),
        };

        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        send_soap(&self.client, &img_url, action, env).await?;
        Ok(())
    }

    async fn recall_preset(&self, slot: u8) -> Result<()> {
        let (ptz_url, _, profile, _) = self.endpoints().await;
        let body = format!(
            r#"<tptz:GotoPreset>
  <tptz:ProfileToken>{profile}</tptz:ProfileToken>
  <tptz:PresetToken>{slot}</tptz:PresetToken>
</tptz:GotoPreset>"#
        );
        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        send_soap(&self.client, &ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/GotoPreset", env).await?;
        Ok(())
    }

    async fn store_preset(&self, slot: u8) -> Result<()> {
        let (ptz_url, _, profile, _) = self.endpoints().await;
        let body = format!(
            r#"<tptz:SetPreset>
  <tptz:ProfileToken>{profile}</tptz:ProfileToken>
  <tptz:PresetName>Preset_{slot}</tptz:PresetName>
  <tptz:PresetToken>{slot}</tptz:PresetToken>
</tptz:SetPreset>"#
        );
        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        send_soap(&self.client, &ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/SetPreset", env).await?;
        Ok(())
    }

    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()> {
        let (_, img_opt, _, vsrc_opt) = self.endpoints().await;
        let Some(img_url) = img_opt else {
            return Err(anyhow!("ONVIF camera does not advertise an Imaging service"));
        };
        let vsrc = vsrc_opt.unwrap_or_else(|| "VideoSource_1".to_string());

        let setting_xml = match cmd {
            ImagingCommand::WhiteBalanceAuto
            | ImagingCommand::WhiteBalanceIndoor
            | ImagingCommand::WhiteBalanceOutdoor
            | ImagingCommand::WhiteBalanceOnePush => {
                "<tt:WhiteBalance><tt:Mode>AUTO</tt:Mode></tt:WhiteBalance>".to_string()
            }
            ImagingCommand::WhiteBalanceManual { red, blue } => format!(
                "<tt:WhiteBalance><tt:Mode>MANUAL</tt:Mode><tt:CrGain>{:.1}</tt:CrGain><tt:CbGain>{:.1}</tt:CbGain></tt:WhiteBalance>",
                red.clamp(0.0, 1.0) * 100.0,
                blue.clamp(0.0, 1.0) * 100.0
            ),
            ImagingCommand::ExposureAuto => {
                "<tt:Exposure><tt:Mode>AUTO</tt:Mode></tt:Exposure>".to_string()
            }
            ImagingCommand::ExposureManual { level } => format!(
                "<tt:Exposure><tt:Mode>MANUAL</tt:Mode><tt:Gain>{:.1}</tt:Gain></tt:Exposure>",
                level.clamp(0.0, 1.0) * 100.0
            ),
            ImagingCommand::BacklightComp { enabled } => format!(
                "<tt:BacklightCompensation><tt:Mode>{}</tt:Mode></tt:BacklightCompensation>",
                if enabled { "ON" } else { "OFF" }
            ),
            ImagingCommand::Sharpness { level } => {
                format!("<tt:Sharpness>{:.1}</tt:Sharpness>", level.clamp(0.0, 1.0) * 100.0)
            }
            ImagingCommand::Contrast { level } => {
                format!("<tt:Contrast>{:.1}</tt:Contrast>", level.clamp(0.0, 1.0) * 100.0)
            }
            ImagingCommand::Brightness { level } => {
                format!("<tt:Brightness>{:.1}</tt:Brightness>", level.clamp(0.0, 1.0) * 100.0)
            }
        };

        let body = format!(
            r#"<timg:SetImagingSettings>
  <timg:VideoSourceToken>{vsrc}</timg:VideoSourceToken>
  <timg:ImagingSettings>{setting_xml}</timg:ImagingSettings>
</timg:SetImagingSettings>"#
        );
        let env = wrap_soap(&build_wsse_header(&self.username, &self.password), &body);
        send_soap(
            &self.client,
            &img_url,
            "http://www.onvif.org/ver20/imaging/wsdl/SetImagingSettings",
            env,
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsse_password_digest_matches_onvif_specification() {
        let nonce = b"0123456789abcdef";
        let created = "2026-11-23T00:00:00Z";
        let header = build_wsse_header_deterministic("admin", "secret", nonce, created);
        assert!(header.contains("<wsse:Username>admin</wsse:Username>"));
        assert!(header.contains("<wsu:Created>2026-11-23T00:00:00Z</wsu:Created>"));
        // Verify deterministic SHA1 digest matches manual formula
        let mut h = Sha1::new();
        h.update(nonce);
        h.update(created.as_bytes());
        h.update(b"secret");
        let expected = BASE64.encode(h.finalize());
        assert!(header.contains(&expected));
    }

    #[test]
    fn parses_ws_discovery_probe_match_xml() {
        let xml = r#"<Envelope xmlns="http://www.w3.org/2003/05/soap-envelope" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery">
          <Body>
            <d:ProbeMatches>
              <d:ProbeMatch>
                <d:Scopes>onvif://www.onvif.org/name/PTZOptics%20Move%204K onvif://www.onvif.org/Profile/Streaming</d:Scopes>
                <d:XAddrs>http://192.168.1.88:80/onvif/device_service</d:XAddrs>
              </d:ProbeMatch>
            </d:ProbeMatches>
          </Body>
        </Envelope>"#;
        let devs = parse_ws_discovery_response(xml);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].host, "192.168.1.88");
        assert_eq!(devs[0].port, 80);
        assert_eq!(devs[0].name, "PTZOptics Move 4K");
    }

    #[test]
    fn parses_onvif_profiles_and_rtsp_uri() {
        let profiles_xml = r#"<Envelope><Body><GetProfilesResponse>
          <Profiles token="MainStreamToken">
            <VideoSourceConfiguration><SourceToken>VSRC_0</SourceToken></VideoSourceConfiguration>
          </Profiles>
        </GetProfilesResponse></Body></Envelope>"#;
        let (pt, vst) = parse_primary_profile(profiles_xml).unwrap();
        assert_eq!(pt, "MainStreamToken");
        assert_eq!(vst.as_deref(), Some("VSRC_0"));

        let uri_xml = r#"<Envelope><Body><GetStreamUriResponse>
          <MediaUri><Uri>rtsp://192.168.1.88:554/1</Uri></MediaUri>
        </GetStreamUriResponse></Body></Envelope>"#;
        assert_eq!(parse_stream_uri(uri_xml).as_deref(), Some("rtsp://192.168.1.88:554/1"));
    }

    #[test]
    fn malformed_xml_and_missing_tags_fail_safely_without_panic() {
        assert!(parse_ws_discovery_response("<unclosed><xml").is_empty());
        assert!(parse_ws_discovery_response("<Envelope><Body/></Envelope>").is_empty());
        // ProbeMatch with non-http XAddr is ignored cleanly
        let bad_xaddr = "<Envelope><ProbeMatch><XAddrs>ftp://bad-scheme</XAddrs></ProbeMatch></Envelope>";
        assert!(parse_ws_discovery_response(bad_xaddr).is_empty());

        assert!(parse_primary_profile("<broken").is_none());
        assert!(parse_primary_profile("<Envelope><Profiles/></Envelope>").is_none());
        assert!(parse_stream_uri("<broken").is_none());
        assert!(parse_stream_uri("<Envelope><MediaUri/></Envelope>").is_none());

        let (media, ptz, img) = parse_capabilities_xaddrs("<not-xml", "10.0.0.5", 8080);
        assert_eq!(media, "http://10.0.0.5:8080/onvif/media_service");
        assert_eq!(ptz, "http://10.0.0.5:8080/onvif/ptz_service");
        assert!(img.is_none());
    }

    #[test]
    fn empty_username_returns_empty_wsse_header() {
        assert!(build_wsse_header("", "secret").is_empty());
    }

    async fn spawn_onvif_mock_server(
        include_imaging: bool,
        status_override: Option<&'static str>,
    ) -> (u16, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    if let Some(err_status) = status_override {
                        let resp = format!(
                            "HTTP/1.1 {err_status}\r\nContent-Length: 5\r\nConnection: close\r\n\r\nFault"
                        );
                        let _ = stream.write_all(resp.as_bytes()).await;
                        return;
                    }
                    let body = if req.contains("GetCapabilities") {
                        let img = if include_imaging {
                            format!("<Imaging><XAddr>http://127.0.0.1:{port}/onvif/imaging_service</XAddr></Imaging>")
                        } else {
                            String::new()
                        };
                        format!(
                            "<Envelope><Body><GetCapabilitiesResponse><Capabilities>\
                             <Media><XAddr>http://127.0.0.1:{port}/onvif/media_service</XAddr></Media>\
                             <PTZ><XAddr>http://127.0.0.1:{port}/onvif/ptz_service</XAddr></PTZ>\
                             {img}\
                             </Capabilities></GetCapabilitiesResponse></Body></Envelope>"
                        )
                    } else if req.contains("GetProfiles") {
                        "<Envelope><Body><GetProfilesResponse>\
                         <Profiles token=\"Prof_1\"><VideoSourceConfiguration><SourceToken>VS_1</SourceToken></VideoSourceConfiguration></Profiles>\
                         </GetProfilesResponse></Body></Envelope>"
                            .to_string()
                    } else if req.contains("GetStreamUri") {
                        "<Envelope><Body><GetStreamUriResponse><MediaUri><Uri>rtsp://127.0.0.1:554/stream1</Uri></MediaUri></GetStreamUriResponse></Body></Envelope>"
                            .to_string()
                    } else {
                        "<Envelope><Body><Ok/></Body></Envelope>".to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/soap+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                });
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn onvif_resolve_and_driver_cover_all_ptz_focus_imaging_and_errors() {
        let (ok_port, ok_srv) = spawn_onvif_mock_server(true, None).await;
        let info = resolve_onvif_camera("127.0.0.1", ok_port, "admin", "pass")
            .await
            .unwrap();
        assert_eq!(info.profile_token, "Prof_1");
        assert_eq!(info.video_source_token.as_deref(), Some("VS_1"));
        assert_eq!(info.rtsp_uri.as_deref(), Some("rtsp://127.0.0.1:554/stream1"));
        assert!(info.imaging_xaddr.is_some());

        let drv = OnvifDriver::new("127.0.0.1", ok_port, "admin", "pass", None);
        assert_eq!(drv.protocol_name(), "onvif");
        drv.pan_tilt_speed(0.4, -0.3).await.unwrap();
        drv.zoom_speed(0.5).await.unwrap();
        drv.stop().await.unwrap();
        drv.recall_preset(1).await.unwrap();
        drv.store_preset(2).await.unwrap();

        for f in [
            FocusCommand::Auto,
            FocusCommand::OnePush,
            FocusCommand::Manual,
            FocusCommand::Near { speed: 0.4 },
            FocusCommand::Far { speed: 0.4 },
            FocusCommand::Stop,
        ] {
            drv.focus(f).await.unwrap();
        }

        for img in [
            ImagingCommand::WhiteBalanceAuto,
            ImagingCommand::WhiteBalanceIndoor,
            ImagingCommand::WhiteBalanceOutdoor,
            ImagingCommand::WhiteBalanceOnePush,
            ImagingCommand::WhiteBalanceManual { red: 0.5, blue: 0.6 },
            ImagingCommand::ExposureAuto,
            ImagingCommand::ExposureManual { level: 0.7 },
            ImagingCommand::BacklightComp { enabled: true },
            ImagingCommand::BacklightComp { enabled: false },
            ImagingCommand::Sharpness { level: 0.5 },
            ImagingCommand::Contrast { level: 0.5 },
            ImagingCommand::Brightness { level: 0.5 },
        ] {
            drv.set_imaging(img).await.unwrap();
        }

        // Camera without Imaging service -> focus & set_imaging return clean error
        let (noimg_port, noimg_srv) = spawn_onvif_mock_server(false, None).await;
        let drv_noimg = OnvifDriver::new("127.0.0.1", noimg_port, "", "", None);
        assert!(drv_noimg.focus(FocusCommand::Auto).await.is_err());
        assert!(drv_noimg.set_imaging(ImagingCommand::ExposureAuto).await.is_err());

        // HTTP 500 failure path -> resolve_onvif_camera & driver return error
        let (err_port, err_srv) = spawn_onvif_mock_server(true, Some("500 Internal Server Error")).await;
        assert!(resolve_onvif_camera("127.0.0.1", err_port, "admin", "pass").await.is_err());
        let drv_err = OnvifDriver::new("127.0.0.1", err_port, "admin", "pass", Some("P_FALLBACK".into()));
        assert!(drv_err.stop().await.is_err());

        // Quick WS-Discovery timeout returns cleanly
        let _ = discover_onvif_devices(25).await;

        ok_srv.abort();
        noimg_srv.abort();
        err_srv.abort();
    }
}
