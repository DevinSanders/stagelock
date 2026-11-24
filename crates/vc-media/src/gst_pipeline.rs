use vc_core::{HardwareAccelBackend, VideoSourceConfig};

/// Generates cross-platform GStreamer pipeline launch descriptions tuned to the detected GPU backend.
/// Keeps decode, scaling, color adjustment, preview encoding, and NDI re-broadcast in zero-copy paths
/// wherever supported by the host hardware.
pub fn build_ingest_pipeline_spec(
    source: &VideoSourceConfig,
    decode_backend: &HardwareAccelBackend,
    encode_backend: &HardwareAccelBackend,
    ndi_out_name: Option<&str>,
) -> String {
    let decoder = match decode_backend {
        HardwareAccelBackend::NvidiaNvcodec => "nvh264dec",
        HardwareAccelBackend::IntelQuickSync => "qsvh264dec",
        HardwareAccelBackend::WindowsD3d11 => "d3d11h264dec",
        HardwareAccelBackend::AppleVideoToolbox => "vtdec_hw",
        HardwareAccelBackend::LinuxVaapi => "vah264dec",
        HardwareAccelBackend::AmdAmf | HardwareAccelBackend::SoftwareCpu => "avdec_h264",
    };

    let encoder = match encode_backend {
        HardwareAccelBackend::NvidiaNvcodec => "nvh264enc preset=low-latency-hq zerolatency=true bitrate=4000",
        HardwareAccelBackend::AmdAmf => "amfh264enc usage=ultra-low-latency bitrate=4000",
        HardwareAccelBackend::IntelQuickSync => "qsvh264enc target-usage=7 low-latency=true bitrate=4000",
        HardwareAccelBackend::AppleVideoToolbox => "vtenc_h264_hw realtime=true bitrate=4000",
        HardwareAccelBackend::LinuxVaapi => "vaapih264enc rate-control=cbr bitrate=4000",
        HardwareAccelBackend::WindowsD3d11 | HardwareAccelBackend::SoftwareCpu => {
            "openh264enc bitrate=4000000 complexity=low"
        }
    };

    let src_branch = match source {
        VideoSourceConfig::TestPattern {
            pattern,
            width,
            height,
            fps,
        } => format!(
            "videotestsrc is-live=true pattern={pattern} ! video/x-raw,width={width},height={height},framerate={fps}/1"
        ),
        VideoSourceConfig::Ndi { source_name, .. } => {
            format!("ndisrc ndi-name=\"{source_name}\" ! ndisrcdemux name=demux demux.video ! queue")
        }
        VideoSourceConfig::Rtsp {
            url,
            latency_ms,
            prefer_tcp,
        } => {
            let proto = if *prefer_tcp { "protocols=tcp" } else { "" };
            format!(
                "rtspsrc location=\"{url}\" latency={latency_ms} {proto} ! rtph264depay ! h264parse ! {decoder}"
            )
        }
        VideoSourceConfig::Rtp {
            address,
            port,
            codec,
        } => format!(
            "udpsrc address={address} port={port} caps=\"application/x-rtp,media=video,encoding-name={codec}\" ! rtph264depay ! h264parse ! {decoder}"
        ),
        VideoSourceConfig::Onvif {
            resolved_rtsp_url,
            host,
            ..
        } => {
            let url = resolved_rtsp_url
                .as_deref()
                .unwrap_or(host.as_str());
            format!("rtspsrc location=\"{url}\" latency=80 protocols=tcp ! rtph264depay ! h264parse ! {decoder}")
        }
        VideoSourceConfig::WebRtc { whep_url, .. } => {
            format!("whepsrc whep-endpoint=\"{whep_url}\" ! {decoder}")
        }
        VideoSourceConfig::Usb {
            device_id,
            width,
            height,
            fps,
            ..
        } => {
            #[cfg(target_os = "windows")]
            let elem = format!("mfvideosrc device-path=\"{device_id}\"");
            #[cfg(target_os = "macos")]
            let elem = format!("avfvideosrc device-index={device_id}");
            #[cfg(target_os = "linux")]
            let elem = format!("v4l2src device=\"{device_id}\"");
            format!("{elem} ! videoconvert ! videoscale ! video/x-raw,width={width},height={height},framerate={fps}/1")
        }
        VideoSourceConfig::DeckLink {
            device_index,
            connection,
            mode,
        } => format!(
            "decklinkvideosrc device-number={device_index} connection={connection} mode={mode}"
        ),
    };

    let mut pipeline = format!(
        "{src_branch} ! videoconvert ! videobalance name=color_adj ! tee name=t \
         t. ! queue leaky=downstream max-size-buffers=2 ! videoscale ! video/x-raw,width=960,height=540 ! {encoder} ! appsink name=preview_sink drop=true max-buffers=1 \
         t. ! queue leaky=downstream max-size-buffers=2 ! videoscale ! video/x-raw,format=RGB,width=640,height=360 ! appsink name=cv_sink drop=true max-buffers=1"
    );

    if let Some(ndi_name) = ndi_out_name {
        pipeline.push_str(&format!(
            " t. ! queue leaky=downstream max-size-buffers=2 ! videoconvert ! video/x-raw,format=UYVY ! ndisink ndi-name=\"{ndi_name}\""
        ));
    }

    pipeline
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_nvcodec_rtsp_pipeline_with_ndi_rebroadcast() {
        let src = VideoSourceConfig::Rtsp {
            url: "rtsp://192.168.1.50:554/stream1".into(),
            latency_ms: 60,
            prefer_tcp: true,
        };
        let spec = build_ingest_pipeline_spec(
            &src,
            &HardwareAccelBackend::NvidiaNvcodec,
            &HardwareAccelBackend::NvidiaNvcodec,
            Some("VC - Stage Cam 1"),
        );
        assert!(spec.contains("nvh264dec"));
        assert!(spec.contains("nvh264enc"));
        assert!(spec.contains("ndisink ndi-name=\"VC - Stage Cam 1\""));
        assert!(spec.contains("cv_sink"));
    }

    #[test]
    fn builds_amd_amf_and_quicksync_pipelines() {
        let src = VideoSourceConfig::TestPattern {
            pattern: "smpte".into(),
            width: 1920,
            height: 1080,
            fps: 60,
        };
        let amd = build_ingest_pipeline_spec(
            &src,
            &HardwareAccelBackend::WindowsD3d11,
            &HardwareAccelBackend::AmdAmf,
            None,
        );
        assert!(amd.contains("amfh264enc"));

        let qsv = build_ingest_pipeline_spec(
            &src,
            &HardwareAccelBackend::IntelQuickSync,
            &HardwareAccelBackend::IntelQuickSync,
            None,
        );
        assert!(qsv.contains("qsvh264enc"));
    }

    #[test]
    fn builds_pipelines_for_all_sources_and_all_hardware_backends() {
        let sources = vec![
            VideoSourceConfig::Ndi { source_name: "CAM1".into(), low_bandwidth: false },
            VideoSourceConfig::Rtsp { url: "rtsp://10.0.0.1/1".into(), latency_ms: 25, prefer_tcp: false },
            VideoSourceConfig::Rtp { address: "239.0.0.1".into(), port: 5004, codec: "H264".into() },
            VideoSourceConfig::Onvif {
                host: "10.0.0.2".into(),
                port: 80,
                username: String::new(),
                password: String::new(),
                resolved_rtsp_url: None,
            },
            VideoSourceConfig::WebRtc { whep_url: "https://host/whep".into(), bearer_token: None },
            VideoSourceConfig::Usb { device_id: "0".into(), device_name: "Cam".into(), width: 1280, height: 720, fps: 30 },
            VideoSourceConfig::DeckLink { device_index: 0, connection: "sdi".into(), mode: "auto".into() },
        ];
        let backends = [
            HardwareAccelBackend::NvidiaNvcodec,
            HardwareAccelBackend::IntelQuickSync,
            HardwareAccelBackend::WindowsD3d11,
            HardwareAccelBackend::AppleVideoToolbox,
            HardwareAccelBackend::LinuxVaapi,
            HardwareAccelBackend::AmdAmf,
            HardwareAccelBackend::SoftwareCpu,
        ];
        for (i, src) in sources.iter().enumerate() {
            let b = &backends[i % backends.len()];
            let spec = build_ingest_pipeline_spec(src, b, b, None);
            assert!(spec.contains("preview_sink"));
            assert!(spec.contains("cv_sink"));
        }
    }
}
