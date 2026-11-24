//! Hardware-accelerated GStreamer live video ingest and capture device enumeration.
//!
//! Supports both:
//! - In-process `gstreamer-rs` (`--features gst-native`) for zero-IPC AppSink frame pulls.
//! - Subprocess `gst-launch-1.0` / `gst-device-monitor-1.0` bridge when built without dev headers,
//!   still leveraging full GPU decode (NVDEC, D3D11, QuickSync, VideoToolbox, VAAPI).

#[cfg(not(feature = "gst-native"))]
use std::io::Read;
use std::path::PathBuf;
#[cfg(not(feature = "gst-native"))]
use std::process::Child;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::info;
#[cfg(not(feature = "gst-native"))]
use tracing::warn;
use vc_core::{HardwareAccelBackend, VideoSourceConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureDeviceInfo {
    pub device_id: String,
    pub name: String,
    pub api: String,
    pub max_width: u32,
    pub max_height: u32,
}

pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Locate a GStreamer CLI executable (`gst-launch-1.0`, `gst-device-monitor-1.0`, etc.).
pub fn find_gst_binary(base_name: &str) -> Option<String> {
    #[cfg(target_os = "windows")]
    let exe_name = format!("{base_name}.exe");
    #[cfg(not(target_os = "windows"))]
    let exe_name = base_name.to_string();

    let mut candidates = vec![base_name.to_string()];
    if let Ok(root) = std::env::var("GSTREAMER_1_0_ROOT_MSVC_X86_64") {
        candidates.push(PathBuf::from(root).join("bin").join(&exe_name).to_string_lossy().to_string());
    }
    #[cfg(target_os = "windows")]
    {
        candidates.push(format!(r"C:\Program Files\gstreamer\1.0\msvc_x86_64\bin\{exe_name}"));
        candidates.push(format!(r"C:\gstreamer\1.0\msvc_x86_64\bin\{exe_name}"));
    }

    for bin in candidates {
        if Command::new(&bin)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            return Some(bin);
        }
    }
    None
}

/// Discover local USB (UVC) webcams, virtual capture devices (e.g. vMix Video), and HDMI/SDI dongles.
pub fn discover_capture_devices() -> Vec<CaptureDeviceInfo> {
    let Some(bin) = find_gst_binary("gst-device-monitor-1.0") else {
        return Vec::new();
    };
    let Ok(output) = Command::new(bin).arg("Video/Source").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_device_monitor_output(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_device_monitor_output(raw: &str) -> Vec<CaptureDeviceInfo> {
    let mut devices = Vec::new();
    let mut cur_name: Option<String> = None;
    let mut cur_path: Option<String> = None;
    let mut cur_api: Option<String> = None;
    let mut max_w: u32 = 1280;
    let mut max_h: u32 = 720;

    let flush = |devices: &mut Vec<CaptureDeviceInfo>,
                 name: &mut Option<String>,
                 path: &mut Option<String>,
                 api: &mut Option<String>,
                 mw: &mut u32,
                 mh: &mut u32| {
        if let Some(n) = name.take() {
            let id = path.take().unwrap_or_else(|| n.clone());
            let api_str = api.take().unwrap_or_else(|| "system".to_string());
            devices.push(CaptureDeviceInfo {
                device_id: id,
                name: n,
                api: api_str,
                max_width: *mw,
                max_height: *mh,
            });
        }
        *mw = 1280;
        *mh = 720;
    };

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed == "Device found:" {
            flush(&mut devices, &mut cur_name, &mut cur_path, &mut cur_api, &mut max_w, &mut max_h);
        } else if let Some(rest) = trimmed.strip_prefix("name  :") {
            cur_name = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("device.path =") {
            cur_path = Some(rest.trim().to_string());
        } else if let Some(rest) = trimmed.strip_prefix("device.api =") {
            cur_api = Some(rest.trim().to_string());
        } else if trimmed.contains("width=") && trimmed.contains("height=") {
            if let (Some(w), Some(h)) = (extract_u32_prop(trimmed, "width="), extract_u32_prop(trimmed, "height=")) {
                if w >= max_w {
                    max_w = w;
                    max_h = h;
                }
            }
        }
    }
    flush(&mut devices, &mut cur_name, &mut cur_path, &mut cur_api, &mut max_w, &mut max_h);
    devices
}

fn extract_u32_prop(line: &str, key: &str) -> Option<u32> {
    let idx = line.find(key)?;
    let rest = &line[idx + key.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Build the hardware-accelerated GStreamer BGRA capture pipeline description for any supported source.
pub fn build_bgra_capture_pipeline(
    source: &VideoSourceConfig,
    decode_backend: &HardwareAccelBackend,
    out_width: u32,
    out_height: u32,
    out_fps: u32,
) -> Result<(String, String, bool)> {
    let (decoder, hw_accel) = match decode_backend {
        HardwareAccelBackend::NvidiaNvcodec => ("nvh264dec", true),
        HardwareAccelBackend::IntelQuickSync => ("qsvh264dec", true),
        HardwareAccelBackend::WindowsD3d11 => ("d3d11h264dec", true),
        HardwareAccelBackend::AppleVideoToolbox => ("vtdec_hw", true),
        HardwareAccelBackend::LinuxVaapi => ("vah264dec", true),
        HardwareAccelBackend::AmdAmf | HardwareAccelBackend::SoftwareCpu => ("avdec_h264", false),
    };

    let (src_spec, label, is_hw) = match source {
        VideoSourceConfig::Rtsp {
            url,
            latency_ms,
            prefer_tcp,
        } => {
            let proto = if *prefer_tcp { "protocols=tcp" } else { "protocols=udp" };
            (
                format!(
                    "rtspsrc location=\"{url}\" latency={latency_ms} {proto} buffer-mode=none drop-on-latency=true ! rtph264depay ! h264parse ! {decoder}"
                ),
                format!("GStreamer RTSP ({decoder})"),
                hw_accel,
            )
        }
        VideoSourceConfig::Rtp {
            address,
            port,
            codec,
        } => (
            format!(
                "udpsrc address={address} port={port} caps=\"application/x-rtp,media=video,encoding-name={codec}\" ! rtph264depay ! h264parse ! {decoder}"
            ),
            format!("GStreamer RTP ({decoder})"),
            hw_accel,
        ),
        VideoSourceConfig::Onvif {
            resolved_rtsp_url,
            host,
            port,
            ..
        } => {
            let url = resolved_rtsp_url
                .clone()
                .unwrap_or_else(|| format!("rtsp://{host}:{port}/stream1"));
            (
                format!(
                    "rtspsrc location=\"{url}\" latency=25 protocols=tcp buffer-mode=none drop-on-latency=true ! rtph264depay ! h264parse ! {decoder}"
                ),
                format!("GStreamer ONVIF ({decoder})"),
                hw_accel,
            )
        }
        VideoSourceConfig::WebRtc { whep_url, .. } => (
            format!("whepsrc whep-endpoint=\"{whep_url}\" ! decodebin"),
            "GStreamer WebRTC (WHEP)".to_string(),
            hw_accel,
        ),
        VideoSourceConfig::Usb { device_id, .. } => {
            #[cfg(target_os = "windows")]
            let elem = if device_id.trim().is_empty() || device_id == "0" {
                "mfvideosrc".to_string()
            } else {
                let escaped = device_id.replace('\\', "\\\\");
                format!("mfvideosrc device-path=\"{escaped}\"")
            };
            #[cfg(target_os = "macos")]
            let elem = format!("avfvideosrc device-index={device_id}");
            #[cfg(target_os = "linux")]
            let elem = format!("v4l2src device=\"{device_id}\"");

            (elem, "GStreamer USB / MediaFoundation".to_string(), true)
        }
        VideoSourceConfig::DeckLink {
            device_index,
            connection,
            mode,
        } => (
            format!(
                "decklinkvideosrc device-number={device_index} connection={connection} mode={mode}"
            ),
            "Blackmagic DeckLink SDI/HDMI".to_string(),
            true,
        ),
        _ => return Err(anyhow!("source kind does not require GStreamer runner")),
    };

    let full_spec = format!(
        "{src_spec} ! videoconvert ! videoscale ! videorate ! video/x-raw,format=BGRA,width={out_width},height={out_height},framerate={out_fps}/1"
    );
    Ok((full_spec, label, is_hw))
}

/// Live GStreamer video frame source (uses in-process `gstreamer-rs` when compiled with `gst-native`,
/// or `gst-launch-1.0` raw stdout pipe otherwise).
pub struct GstFrameSource {
    pub width: u32,
    pub height: u32,
    pub decoder_label: String,
    pub hw_accelerated: bool,
    #[cfg(feature = "gst-native")]
    pipeline: gstreamer::Pipeline,
    #[cfg(feature = "gst-native")]
    appsink: gstreamer_app::AppSink,
    #[cfg(not(feature = "gst-native"))]
    child: Child,
}

impl GstFrameSource {
    pub fn start(
        source: &VideoSourceConfig,
        decode_backend: &HardwareAccelBackend,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<Self> {
        let (base_pipeline, decoder_label, hw_accelerated) =
            build_bgra_capture_pipeline(source, decode_backend, width, height, fps)?;

        #[cfg(feature = "gst-native")]
        {
            use gstreamer::prelude::*;
            gstreamer::init().context("failed initializing GStreamer")?;
            let spec = format!(
                "{base_pipeline} ! appsink name=vc_sink drop=true max-buffers=1 sync=false"
            );
            info!(pipeline = %spec, "Starting native gstreamer-rs pipeline");
            let element = gstreamer::parse::launch(&spec)
                .with_context(|| format!("failed parsing GStreamer pipeline: {spec}"))?;
            let pipeline = element
                .downcast::<gstreamer::Pipeline>()
                .map_err(|_| anyhow!("parsed GStreamer element is not a Pipeline"))?;
            let appsink = pipeline
                .by_name("vc_sink")
                .ok_or_else(|| anyhow!("missing vc_sink appsink"))?
                .downcast::<gstreamer_app::AppSink>()
                .map_err(|_| anyhow!("vc_sink is not an AppSink"))?;

            pipeline
                .set_state(gstreamer::State::Playing)
                .context("failed transitioning GStreamer pipeline to Playing")?;

            Ok(Self {
                width,
                height,
                decoder_label,
                hw_accelerated,
                pipeline,
                appsink,
            })
        }

        #[cfg(not(feature = "gst-native"))]
        {
            let gst_launch = find_gst_binary("gst-launch-1.0")
                .ok_or_else(|| anyhow!("gst-launch-1.0 not found on system"))?;
            let full_spec = format!("{base_pipeline} ! fdsink fd=1");
            info!(pipeline = %full_spec, "Starting GStreamer bridge pipeline");

            let args = split_pipeline_args(&full_spec);
            let child = Command::new(gst_launch)
                .arg("-q")
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .context("failed spawning gst-launch-1.0")?;

            Ok(Self {
                width,
                height,
                decoder_label,
                hw_accelerated,
                child,
            })
        }
    }

    pub fn next_frame(&mut self) -> Option<CapturedFrame> {
        #[cfg(feature = "gst-native")]
        {
            let sample = self
                .appsink
                .try_pull_sample(gstreamer::ClockTime::from_mseconds(250))?;
            let buffer = sample.buffer()?;
            let map = buffer.map_readable().ok()?;
            let expected = (self.width * self.height * 4) as usize;
            if map.len() < expected {
                return None;
            }
            Some(CapturedFrame {
                width: self.width,
                height: self.height,
                bgra: map.as_slice()[..expected].to_vec(),
            })
        }

        #[cfg(not(feature = "gst-native"))]
        {
            let frame_bytes = (self.width * self.height * 4) as usize;
            let mut buf = vec![0u8; frame_bytes];
            let stdout = self.child.stdout.as_mut()?;
            match stdout.read_exact(&mut buf) {
                Ok(()) => Some(CapturedFrame {
                    width: self.width,
                    height: self.height,
                    bgra: buf,
                }),
                Err(err) => {
                    warn!("GStreamer bridge stream ended: {err}");
                    None
                }
            }
        }
    }
}

impl Drop for GstFrameSource {
    fn drop(&mut self) {
        #[cfg(feature = "gst-native")]
        {
            use gstreamer::prelude::*;
            let _ = self.pipeline.set_state(gstreamer::State::Null);
        }
        #[cfg(not(feature = "gst-native"))]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(not(feature = "gst-native"))]
fn split_pipeline_args(spec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in spec.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gst_device_monitor_mediafoundation_output() {
        let sample = r#"
Probing devices...

Device found:

	name  : vMix Video
	class : Source/Video
	caps  : video/x-raw, format=UYVY, width=3840, height=2160, framerate=60000/1001
	properties:
		device.api = mediafoundation
		device.path = @device:sw:{860BB310}\{8E14549A}
		device.name = vMix Video

Device found:

	name  : USB HDMI Capture
	class : Source/Video
	caps  : video/x-raw, format=YUY2, width=1920, height=1080, framerate=60/1
	properties:
		device.api = mediafoundation
		device.path = \\?\usb#vid_534d
"#;
        let parsed = parse_device_monitor_output(sample);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "vMix Video");
        assert_eq!(parsed[0].max_width, 3840);
        assert_eq!(parsed[0].max_height, 2160);
        assert_eq!(parsed[1].device_id, r"\\?\usb#vid_534d");
        assert_eq!(parsed[1].max_width, 1920);
    }

    #[test]
    fn malformed_or_empty_device_monitor_output_returns_empty() {
        assert!(parse_device_monitor_output("").is_empty());
        assert!(parse_device_monitor_output("ERROR: failed probing\nDevice found:\n").is_empty());
        // Device with non-numeric caps falls back safely to default 1280x720
        let partial = "Device found:\n\tname  : WeirdCam\n\tcaps : width=abc, height=xyz\n";
        let devs = parse_device_monitor_output(partial);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].device_id, "WeirdCam");
        assert_eq!(devs[0].max_width, 1280);
        assert_eq!(devs[0].max_height, 720);
    }

    #[test]
    fn build_bgra_capture_pipeline_rejects_non_gstreamer_sources() {
        let test_src = VideoSourceConfig::TestPattern {
            pattern: "smpte".into(),
            width: 640,
            height: 360,
            fps: 30,
        };
        assert!(
            build_bgra_capture_pipeline(&test_src, &HardwareAccelBackend::SoftwareCpu, 640, 360, 30)
                .is_err()
        );

        let ndi_src = VideoSourceConfig::Ndi {
            source_name: "STUDIO (CAM 1)".into(),
            low_bandwidth: false,
        };
        assert!(
            build_bgra_capture_pipeline(&ndi_src, &HardwareAccelBackend::NvidiaNvcodec, 1280, 720, 30)
                .is_err()
        );
    }
}
