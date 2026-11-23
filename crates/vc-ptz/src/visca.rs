use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket};
use tracing::debug;

use crate::driver::{FocusCommand, ImagingCommand, PtzDriver};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViscaTransport {
    /// Sony VISCA-over-IP (UDP with 8-byte payload header + sequence number, default port 52381)
    SonyUdp,
    /// Raw VISCA over TCP (PTZOptics 5678, BirdDog, Marshall, Lumens)
    RawTcp,
    /// Raw VISCA over UDP (PTZOptics 1259, Aver)
    RawUdp,
}

pub struct ViscaIpDriver {
    host: String,
    port: u16,
    transport: ViscaTransport,
    seq: AtomicU32,
}

impl ViscaIpDriver {
    pub fn new(host: impl Into<String>, port: u16, transport: ViscaTransport) -> Self {
        Self {
            host: host.into(),
            port,
            transport,
            seq: AtomicU32::new(1),
        }
    }

    /// Wrap a raw VISCA command in the Sony 8-byte VISCA-over-IP header.
    pub fn wrap_sony_header(payload: &[u8], sequence: u32) -> Vec<u8> {
        let len = payload.len() as u16;
        let mut packet = Vec::with_capacity(8 + payload.len());
        packet.extend_from_slice(&[0x01, 0x00]); // VISCA command payload type
        packet.extend_from_slice(&len.to_be_bytes());
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(payload);
        packet
    }

    /// Encode VISCA Pan/Tilt drive command (`81 01 06 01 VV WW DD EE FF`).
    pub fn encode_pan_tilt(pan: f32, tilt: f32) -> Vec<u8> {
        let pan = pan.clamp(-1.0, 1.0);
        let tilt = tilt.clamp(-1.0, 1.0);

        let pan_dir: u8 = if pan.abs() < 0.02 {
            0x03
        } else if pan < 0.0 {
            0x01
        } else {
            0x02
        };

        let tilt_dir: u8 = if tilt.abs() < 0.02 {
            0x03
        } else if tilt > 0.0 {
            0x01
        } else {
            0x02
        };

        let pan_speed = if pan_dir == 0x03 {
            0x01
        } else {
            ((pan.abs() * 23.0).round() as u8 + 1).clamp(1, 0x18)
        };
        let tilt_speed = if tilt_dir == 0x03 {
            0x01
        } else {
            ((tilt.abs() * 19.0).round() as u8 + 1).clamp(1, 0x14)
        };

        vec![0x81, 0x01, 0x06, 0x01, pan_speed, tilt_speed, pan_dir, tilt_dir, 0xFF]
    }

    /// Encode VISCA Zoom variable-speed command (`81 01 04 07 XX FF`).
    pub fn encode_zoom(zoom: f32) -> Vec<u8> {
        let z = zoom.clamp(-1.0, 1.0);
        let byte = if z.abs() < 0.02 {
            0x00
        } else {
            let speed = ((z.abs() * 7.0).round() as u8).clamp(0, 7);
            if z > 0.0 {
                0x20 | speed
            } else {
                0x30 | speed
            }
        };
        vec![0x81, 0x01, 0x04, 0x07, byte, 0xFF]
    }

    /// Encode VISCA Focus command.
    pub fn encode_focus(cmd: FocusCommand) -> Vec<u8> {
        match cmd {
            FocusCommand::Auto => vec![0x81, 0x01, 0x04, 0x38, 0x02, 0xFF],
            FocusCommand::Manual => vec![0x81, 0x01, 0x04, 0x38, 0x03, 0xFF],
            FocusCommand::OnePush => vec![0x81, 0x01, 0x04, 0x18, 0x01, 0xFF],
            FocusCommand::Far { speed } => {
                let s = ((speed.clamp(0.0, 1.0) * 7.0).round() as u8).clamp(0, 7);
                vec![0x81, 0x01, 0x04, 0x08, 0x20 | s, 0xFF]
            }
            FocusCommand::Near { speed } => {
                let s = ((speed.clamp(0.0, 1.0) * 7.0).round() as u8).clamp(0, 7);
                vec![0x81, 0x01, 0x04, 0x08, 0x30 | s, 0xFF]
            }
            FocusCommand::Stop => vec![0x81, 0x01, 0x04, 0x08, 0x00, 0xFF],
        }
    }

    pub fn encode_preset(store: bool, slot: u8) -> Vec<u8> {
        let mode = if store { 0x01 } else { 0x02 };
        vec![0x81, 0x01, 0x04, 0x3F, mode, slot.min(0x7F), 0xFF]
    }

    /// Encode Layer-1 hardware imaging commands into one or more VISCA command frames.
    pub fn encode_imaging(cmd: ImagingCommand) -> Vec<Vec<u8>> {
        match cmd {
            ImagingCommand::WhiteBalanceAuto => vec![vec![0x81, 0x01, 0x04, 0x35, 0x00, 0xFF]],
            ImagingCommand::WhiteBalanceIndoor => vec![vec![0x81, 0x01, 0x04, 0x35, 0x01, 0xFF]],
            ImagingCommand::WhiteBalanceOutdoor => vec![vec![0x81, 0x01, 0x04, 0x35, 0x02, 0xFF]],
            ImagingCommand::WhiteBalanceOnePush => vec![
                vec![0x81, 0x01, 0x04, 0x35, 0x03, 0xFF], // WB mode = One-Push
                vec![0x81, 0x01, 0x04, 0x10, 0x05, 0xFF], // One-Push WB Trigger
            ],
            ImagingCommand::WhiteBalanceManual { red, blue } => {
                let r = (red.clamp(0.0, 1.0) * 255.0).round() as u8;
                let b = (blue.clamp(0.0, 1.0) * 255.0).round() as u8;
                vec![
                    vec![0x81, 0x01, 0x04, 0x35, 0x05, 0xFF], // Manual WB
                    vec![0x81, 0x01, 0x04, 0x43, 0x00, 0x00, (r >> 4) & 0x0F, r & 0x0F, 0xFF],
                    vec![0x81, 0x01, 0x04, 0x44, 0x00, 0x00, (b >> 4) & 0x0F, b & 0x0F, 0xFF],
                ]
            }
            ImagingCommand::ExposureAuto => vec![vec![0x81, 0x01, 0x04, 0x39, 0x00, 0xFF]],
            ImagingCommand::ExposureManual { level } => {
                // Enable Exposure Comp / Bright direct level (0x00..=0x0E)
                let step = (level.clamp(0.0, 1.0) * 14.0).round() as u8;
                vec![
                    vec![0x81, 0x01, 0x04, 0x3E, 0x02, 0xFF], // ExpComp On
                    vec![0x81, 0x01, 0x04, 0x4E, 0x00, 0x00, 0x00, step & 0x0F, 0xFF],
                ]
            }
            ImagingCommand::BacklightComp { enabled } => {
                vec![vec![0x81, 0x01, 0x04, 0x33, if enabled { 0x02 } else { 0x03 }, 0xFF]]
            }
            ImagingCommand::Sharpness { level } => {
                let ap = (level.clamp(0.0, 1.0) * 15.0).round() as u8;
                vec![vec![0x81, 0x01, 0x04, 0x42, 0x00, 0x00, 0x00, ap & 0x0F, 0xFF]]
            }
            ImagingCommand::Contrast { level } => {
                let c = (level.clamp(0.0, 1.0) * 14.0).round() as u8;
                vec![vec![0x81, 0x01, 0x04, 0xA2, 0x00, 0x00, 0x00, c & 0x0F, 0xFF]]
            }
            ImagingCommand::Brightness { level } => {
                let b = (level.clamp(0.0, 1.0) * 14.0).round() as u8;
                vec![vec![0x81, 0x01, 0x04, 0xA1, 0x00, 0x00, 0x00, b & 0x0F, 0xFF]]
            }
        }
    }

    async fn send_payload(&self, raw_visca: &[u8]) -> Result<()> {
        let addr = format!("{}:{}", self.host, self.port);
        match self.transport {
            ViscaTransport::SonyUdp => {
                let seq = self.seq.fetch_add(1, Ordering::Relaxed);
                let packet = Self::wrap_sony_header(raw_visca, seq);
                let sock = UdpSocket::bind("0.0.0.0:0").await?;
                sock.send_to(&packet, &addr).await?;
                debug!(addr = %addr, seq, bytes = packet.len(), "Sent Sony VISCA UDP");
            }
            ViscaTransport::RawUdp => {
                let sock = UdpSocket::bind("0.0.0.0:0").await?;
                sock.send_to(raw_visca, &addr).await?;
                debug!(addr = %addr, bytes = raw_visca.len(), "Sent Raw VISCA UDP");
            }
            ViscaTransport::RawTcp => {
                let mut stream = TcpStream::connect(&addr).await?;
                stream.write_all(raw_visca).await?;
                debug!(addr = %addr, bytes = raw_visca.len(), "Sent Raw VISCA TCP");
            }
        }
        Ok(())
    }
}

#[async_trait]
impl PtzDriver for ViscaIpDriver {
    fn protocol_name(&self) -> &'static str {
        match self.transport {
            ViscaTransport::SonyUdp => "visca_ip_sony",
            ViscaTransport::RawTcp => "visca_ip_raw_tcp",
            ViscaTransport::RawUdp => "visca_ip_raw_udp",
        }
    }

    async fn pan_tilt_speed(&self, pan: f32, tilt: f32) -> Result<()> {
        self.send_payload(&Self::encode_pan_tilt(pan, tilt)).await
    }

    async fn zoom_speed(&self, zoom: f32) -> Result<()> {
        self.send_payload(&Self::encode_zoom(zoom)).await
    }

    async fn stop(&self) -> Result<()> {
        self.send_payload(&Self::encode_pan_tilt(0.0, 0.0)).await?;
        self.send_payload(&Self::encode_zoom(0.0)).await?;
        Ok(())
    }

    async fn focus(&self, cmd: FocusCommand) -> Result<()> {
        self.send_payload(&Self::encode_focus(cmd)).await
    }

    async fn recall_preset(&self, slot: u8) -> Result<()> {
        self.send_payload(&Self::encode_preset(false, slot)).await
    }

    async fn store_preset(&self, slot: u8) -> Result<()> {
        self.send_payload(&Self::encode_preset(true, slot)).await
    }

    async fn set_imaging(&self, cmd: ImagingCommand) -> Result<()> {
        for frame in Self::encode_imaging(cmd) {
            self.send_payload(&frame).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visca_pan_tilt_encoding_matches_spec() {
        assert_eq!(
            ViscaIpDriver::encode_pan_tilt(0.0, 0.0),
            vec![0x81, 0x01, 0x06, 0x01, 0x01, 0x01, 0x03, 0x03, 0xFF]
        );
        assert_eq!(
            ViscaIpDriver::encode_pan_tilt(1.0, -1.0),
            vec![0x81, 0x01, 0x06, 0x01, 0x18, 0x14, 0x02, 0x02, 0xFF]
        );
        assert_eq!(
            ViscaIpDriver::encode_pan_tilt(-0.5, 0.5),
            vec![0x81, 0x01, 0x06, 0x01, 0x0D, 0x0B, 0x01, 0x01, 0xFF]
        );
    }

    #[test]
    fn visca_zoom_and_focus_encoding_matches_spec() {
        assert_eq!(ViscaIpDriver::encode_zoom(0.0), vec![0x81, 0x01, 0x04, 0x07, 0x00, 0xFF]);
        assert_eq!(ViscaIpDriver::encode_zoom(1.0), vec![0x81, 0x01, 0x04, 0x07, 0x27, 0xFF]);
        assert_eq!(ViscaIpDriver::encode_zoom(-1.0), vec![0x81, 0x01, 0x04, 0x07, 0x37, 0xFF]);
        assert_eq!(
            ViscaIpDriver::encode_focus(FocusCommand::OnePush),
            vec![0x81, 0x01, 0x04, 0x18, 0x01, 0xFF]
        );
    }

    #[test]
    fn visca_imaging_encoding_matches_spec() {
        assert_eq!(
            ViscaIpDriver::encode_imaging(ImagingCommand::WhiteBalanceOnePush),
            vec![
                vec![0x81, 0x01, 0x04, 0x35, 0x03, 0xFF],
                vec![0x81, 0x01, 0x04, 0x10, 0x05, 0xFF],
            ]
        );
        assert_eq!(
            ViscaIpDriver::encode_imaging(ImagingCommand::Sharpness { level: 1.0 }),
            vec![vec![0x81, 0x01, 0x04, 0x42, 0x00, 0x00, 0x00, 0x0F, 0xFF]]
        );
        assert_eq!(
            ViscaIpDriver::encode_imaging(ImagingCommand::BacklightComp { enabled: true }),
            vec![vec![0x81, 0x01, 0x04, 0x33, 0x02, 0xFF]]
        );
    }

    #[test]
    fn sony_header_wraps_payload_with_big_endian_len_and_seq() {
        let cmd = ViscaIpDriver::encode_preset(false, 3);
        let pkt = ViscaIpDriver::wrap_sony_header(&cmd, 42);
        assert_eq!(&pkt[0..8], &[0x01, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x2A]);
        assert_eq!(&pkt[8..], &cmd[..]);
    }

    #[test]
    fn extreme_speeds_and_preset_slots_clamp_within_visca_bounds() {
        let pt = ViscaIpDriver::encode_pan_tilt(99.0, -99.0);
        assert_eq!(pt[4], 0x18, "pan speed must clamp at VISCA max 0x18");
        assert_eq!(pt[5], 0x14, "tilt speed must clamp at VISCA max 0x14");

        let z_in = ViscaIpDriver::encode_zoom(42.0);
        assert_eq!(z_in[4], 0x27, "tele zoom speed must clamp at 7");
        let z_out = ViscaIpDriver::encode_zoom(-42.0);
        assert_eq!(z_out[4], 0x37, "wide zoom speed must clamp at 7");

        let preset = ViscaIpDriver::encode_preset(true, 255);
        assert_eq!(preset[5], 0x7F, "preset slot must clamp at VISCA 7-bit max 0x7F");

        let sharp = ViscaIpDriver::encode_imaging(ImagingCommand::Sharpness { level: 5.0 });
        assert_eq!(sharp[0][7], 0x0F, "sharpness nibble must clamp to 0x0F");
        let sharp_low = ViscaIpDriver::encode_imaging(ImagingCommand::Sharpness { level: -5.0 });
        assert_eq!(sharp_low[0][7], 0x00, "negative sharpness must clamp to 0x00");
    }
}
