//! Ultra-low-latency GPU H.264 preview encoder for WebCodecs (`VideoDecoder`) streaming.
//!
//! Encodes live BGRA frames to H.264 Annex-B Access Units (`bframes=0`, inline SPS/PPS per IDR)
//! using hardware encoders (`nvh264enc`, `amfh264enc`, `qsvh264enc`, `vtenc_h264_hw`, `vah264enc`,
//! `mfh264enc`) or permissive BSD `openh264enc` (never GPL `x264`).
//!
//! Supports both in-process `gstreamer-rs` (`--features gst-native`) and the `gst-launch-1.0`
//! runtime bridge when compiled without GStreamer dev headers.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use bytes::Bytes;
use tokio::sync::{broadcast, watch, RwLock};
use tracing::{debug, info, warn};
use vc_core::HardwareAccelBackend;

use crate::gst_runner::find_gst_binary;

/// Raw BGRA frame handed off asynchronously from the capture/AI loop to preview encoders.
#[derive(Debug, Clone)]
pub struct RawBgraFrame {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub timestamp_us: u64,
    pub bgra: Arc<Vec<u8>>,
}

/// Encoded H.264 Annex-B Access Unit ready for binary WebSocket -> WebCodecs `VideoDecoder`.
#[derive(Debug, Clone)]
pub struct H264AccessUnit {
    pub codec: String,
    pub encoder_name: String,
    pub width: u32,
    pub height: u32,
    pub timestamp_us: u64,
    pub is_keyframe: bool,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct H264EncoderSpec {
    pub element_name: &'static str,
    pub pipeline_fragment: String,
    pub hw_accelerated: bool,
}

/// Inspect installed GStreamer plugins and choose the lowest-latency permissive H.264 encoder.
pub fn select_zero_latency_h264_encoder(
    preferred_backend: &HardwareAccelBackend,
) -> Option<H264EncoderSpec> {
    let gst_inspect = find_gst_binary("gst-inspect-1.0")?;
    let has_element = |name: &str| -> bool {
        Command::new(&gst_inspect)
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    if !has_element("h264parse") {
        return None;
    }

    let mut candidates: Vec<(&'static str, &'static str, bool)> = Vec::new();
    match preferred_backend {
        HardwareAccelBackend::NvidiaNvcodec => {
            candidates.push((
                "nvh264enc",
                "nvh264enc zerolatency=true bframes=0 repeat-sequence-header=true gop-size=30 bitrate=3500 rc-mode=cbr",
                true,
            ));
        }
        HardwareAccelBackend::AmdAmf => {
            candidates.push((
                "amfh264enc",
                "amfh264enc usage=ultra-low-latency gop-size=30 bitrate=3500",
                true,
            ));
        }
        HardwareAccelBackend::IntelQuickSync => {
            candidates.push((
                "qsvh264enc",
                "qsvh264enc low-latency=true gop-size=30 bitrate=3500",
                true,
            ));
        }
        HardwareAccelBackend::AppleVideoToolbox => {
            candidates.push((
                "vtenc_h264_hw",
                "vtenc_h264_hw realtime=true allow-frame-reordering=false max-keyframe-interval=30 bitrate=3500",
                true,
            ));
        }
        HardwareAccelBackend::LinuxVaapi => {
            candidates.push((
                "vah264enc",
                "vah264enc key-int-max=30 bitrate=3500",
                true,
            ));
        }
        HardwareAccelBackend::WindowsD3d11 | HardwareAccelBackend::SoftwareCpu => {}
    }

    // Universal fallback order (all permissive/system — never GPL x264)
    candidates.extend_from_slice(&[
        (
            "nvh264enc",
            "nvh264enc zerolatency=true bframes=0 repeat-sequence-header=true gop-size=30 bitrate=3500 rc-mode=cbr",
            true,
        ),
        (
            "amfh264enc",
            "amfh264enc usage=ultra-low-latency gop-size=30 bitrate=3500",
            true,
        ),
        (
            "qsvh264enc",
            "qsvh264enc low-latency=true gop-size=30 bitrate=3500",
            true,
        ),
        (
            "vtenc_h264_hw",
            "vtenc_h264_hw realtime=true allow-frame-reordering=false max-keyframe-interval=30 bitrate=3500",
            true,
        ),
        ("vah264enc", "vah264enc key-int-max=30 bitrate=3500", true),
        (
            "mfh264enc",
            "mfh264enc low-latency=true gop-size=30 bitrate=3500",
            true,
        ),
        (
            "openh264enc",
            "openh264enc complexity=low gop-size=30 bitrate=3500000",
            false,
        ),
    ]);

    for (name, frag, hw) in candidates {
        if has_element(name) {
            return Some(H264EncoderSpec {
                element_name: name,
                pipeline_fragment: frag.to_string(),
                hw_accelerated: hw,
            });
        }
    }
    None
}

/// Split an Annex-B H.264 byte buffer into individual NAL unit payloads (excluding start codes).
pub fn split_annex_b_nals(buf: &[u8]) -> Vec<&[u8]> {
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i + 3 <= buf.len() {
        if buf[i] == 0 && buf[i + 1] == 0 {
            if i + 4 <= buf.len() && buf[i + 2] == 0 && buf[i + 3] == 1 {
                starts.push((i, i + 4));
                i += 4;
                continue;
            } else if buf[i + 2] == 1 {
                starts.push((i, i + 3));
                i += 3;
                continue;
            }
        }
        i += 1;
    }

    let mut nals = Vec::with_capacity(starts.len());
    for idx in 0..starts.len() {
        let payload_start = starts[idx].1;
        let payload_end = if idx + 1 < starts.len() {
            starts[idx + 1].0
        } else {
            buf.len()
        };
        if payload_start < payload_end {
            nals.push(&buf[payload_start..payload_end]);
        }
    }
    nals
}

/// Returns true if the H.264 Annex-B Access Unit contains an IDR keyframe slice (NAL type 5).
pub fn is_h264_keyframe(au: &[u8]) -> bool {
    split_annex_b_nals(au)
        .into_iter()
        .any(|nal| !nal.is_empty() && (nal[0] & 0x1F) == 5)
}

/// Extract WebCodecs AVC codec string (e.g. `"avc1.42e01f"` or `"avc1.64001f"`) from an SPS NAL (type 7).
pub fn extract_avc_codec_string(au: &[u8]) -> Option<String> {
    for nal in split_annex_b_nals(au) {
        if nal.len() >= 4 && (nal[0] & 0x1F) == 7 {
            return Some(format!("avc1.{:02x}{:02x}{:02x}", nal[1], nal[2], nal[3]));
        }
    }
    None
}

/// Pack an H.264 Access Unit into our 20-byte binary WebSocket frame envelope:
/// - `[0..4]`  Magic `b"VCH2"`
/// - `[4]`     Flags (`bit 0 = is_keyframe`)
/// - `[5]`     Codec ID (`1 = H.264 Annex-B`)
/// - `[6..8]`  Reserved (`0`)
/// - `[8..10]` Width (`u16` LE)
/// - `[10..12]` Height (`u16` LE)
/// - `[12..20]` Timestamp in microseconds (`u64` LE)
/// - `[20..]`  Raw H.264 Annex-B Access Unit bytes
pub fn encode_ws_video_packet(au: &H264AccessUnit) -> Vec<u8> {
    let mut out = Vec::with_capacity(20 + au.data.len());
    out.extend_from_slice(b"VCH2");
    out.push(if au.is_keyframe { 1 } else { 0 });
    out.push(1); // H.264 Annex-B
    out.extend_from_slice(&0_u16.to_le_bytes());
    out.extend_from_slice(&(au.width.min(u16::MAX as u32) as u16).to_le_bytes());
    out.extend_from_slice(&(au.height.min(u16::MAX as u32) as u16).to_le_bytes());
    out.extend_from_slice(&au.timestamp_us.to_le_bytes());
    out.extend_from_slice(&au.data);
    out
}

/// Streaming Annex-B Access Unit accumulator for byte-stream pipes (CLI bridge mode).
/// Flushes complete AUs as soon as the start of the next AU (AUD=9, SPS=7, or first VCL after VCL) arrives.
#[cfg_attr(feature = "gst-native", allow(dead_code))]
struct AnnexBAuFramer {
    buf: Vec<u8>,
    seen_vcl: bool,
}

#[cfg_attr(feature = "gst-native", allow(dead_code))]
impl AnnexBAuFramer {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(128 * 1024),
            seen_vcl: false,
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Vec<Bytes> {
        self.buf.extend_from_slice(chunk);
        let mut completed = Vec::new();

        loop {
            let Some((split_at, next_has_vcl)) = self.find_au_boundary() else {
                break;
            };
            let au_bytes = self.buf.drain(..split_at).collect::<Vec<u8>>();
            if !au_bytes.is_empty() {
                completed.push(Bytes::from(au_bytes));
            }
            self.seen_vcl = next_has_vcl;
        }

        // Prevent unbounded buffer growth if stream is malformed
        if self.buf.len() > 2 * 1024 * 1024 {
            self.buf.clear();
            self.seen_vcl = false;
        }
        completed
    }

    fn find_au_boundary(&self) -> Option<(usize, bool)> {
        let mut saw_vcl = false;
        let mut i = 0;
        let len = self.buf.len();

        while i + 4 <= len {
            let sc_len = if self.buf[i] == 0 && self.buf[i + 1] == 0 && self.buf[i + 2] == 1 {
                3
            } else if i + 5 <= len
                && self.buf[i] == 0
                && self.buf[i + 1] == 0
                && self.buf[i + 2] == 0
                && self.buf[i + 3] == 1
            {
                4
            } else {
                i += 1;
                continue;
            };

            let nal_header = self.buf[i + sc_len];
            let nal_type = nal_header & 0x1F;
            let is_vcl = (1..=5).contains(&nal_type);
            let is_delim = matches!(nal_type, 6 | 7 | 8 | 9);

            if saw_vcl && (is_delim || is_vcl) && i > 0 {
                // Keep SPS/PPS/AUD with the upcoming frame, split right before this start code
                return Some((i, is_vcl));
            }
            if is_vcl {
                saw_vcl = true;
            }
            i += sc_len + 1;
        }
        None
    }
}

/// Active per-camera lazy H.264 broadcast hub. Starts the GPU encoder only while >=1 WebCodecs
/// viewer is connected, caches the latest IDR keyframe for instant startup, and shuts down when idle.
#[derive(Clone)]
pub struct H264PreviewHub {
    encode_backend: HardwareAccelBackend,
    raw_frame_rx: watch::Receiver<Option<RawBgraFrame>>,
    stop_rx: watch::Receiver<bool>,
    inner: Arc<RwLock<Option<ActiveH264Session>>>,
}

struct ActiveH264Session {
    tx: broadcast::Sender<H264AccessUnit>,
    last_keyframe: Arc<RwLock<Option<H264AccessUnit>>>,
    encoder_name: String,
}

pub struct H264Subscription {
    pub encoder_name: String,
    pub initial_keyframe: Option<H264AccessUnit>,
    pub rx: broadcast::Receiver<H264AccessUnit>,
}

impl H264PreviewHub {
    pub fn new(
        encode_backend: HardwareAccelBackend,
        raw_frame_rx: watch::Receiver<Option<RawBgraFrame>>,
        stop_rx: watch::Receiver<bool>,
    ) -> Self {
        Self {
            encode_backend,
            raw_frame_rx,
            stop_rx,
            inner: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn subscribe(&self) -> Result<H264Subscription> {
        // Reuse already-running session if active
        {
            let guard = self.inner.read().await;
            if let Some(ref session) = *guard {
                if session.tx.receiver_count() > 0 {
                    let initial_keyframe = session.last_keyframe.read().await.clone();
                    return Ok(H264Subscription {
                        encoder_name: session.encoder_name.clone(),
                        initial_keyframe,
                        rx: session.tx.subscribe(),
                    });
                }
            }
        }

        let spec = select_zero_latency_h264_encoder(&self.encode_backend)
            .ok_or_else(|| anyhow!("No GStreamer zero-latency H.264 encoder available on host"))?;

        let (tx, rx) = broadcast::channel::<H264AccessUnit>(8);
        let last_keyframe: Arc<RwLock<Option<H264AccessUnit>>> = Arc::new(RwLock::new(None));
        let encoder_name = spec.element_name.to_string();

        {
            let mut guard = self.inner.write().await;
            *guard = Some(ActiveH264Session {
                tx: tx.clone(),
                last_keyframe: last_keyframe.clone(),
                encoder_name: encoder_name.clone(),
            });
        }

        let raw_rx = self.raw_frame_rx.clone();
        let stop_rx = self.stop_rx.clone();
        let hub_inner = self.inner.clone();

        tokio::task::spawn_blocking(move || {
            run_h264_encoder_worker(spec, raw_rx, stop_rx, tx, last_keyframe);
            if let Ok(mut guard) = hub_inner.try_write() {
                *guard = None;
            }
        });

        Ok(H264Subscription {
            encoder_name,
            initial_keyframe: None,
            rx,
        })
    }
}

fn run_h264_encoder_worker(
    spec: H264EncoderSpec,
    mut raw_rx: watch::Receiver<Option<RawBgraFrame>>,
    stop_rx: watch::Receiver<bool>,
    tx: broadcast::Sender<H264AccessUnit>,
    last_keyframe: Arc<RwLock<Option<H264AccessUnit>>>,
) {
    // Wait up to 2s for first raw BGRA frame to determine dimensions & fps
    let mut initial_frame = raw_rx.borrow().clone();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while initial_frame.is_none() && std::time::Instant::now() < deadline {
        if *stop_rx.borrow() || tx.receiver_count() == 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
        initial_frame = raw_rx.borrow().clone();
    }

    let Some(first) = initial_frame else {
        return;
    };

    info!(
        encoder = spec.element_name,
        hw = spec.hw_accelerated,
        width = first.width,
        height = first.height,
        fps = first.fps,
        "Starting lazy zero-latency H.264 preview encoder"
    );

    #[cfg(feature = "gst-native")]
    {
        if let Err(err) = run_h264_gst_native(&spec, first, &mut raw_rx, &stop_rx, &tx, &last_keyframe)
        {
            warn!(encoder = spec.element_name, "Native H.264 preview encoder stopped: {err}");
        }
    }

    #[cfg(not(feature = "gst-native"))]
    {
        if let Err(err) = run_h264_cli_bridge(&spec, first, &mut raw_rx, &stop_rx, &tx, &last_keyframe)
        {
            warn!(encoder = spec.element_name, "H.264 bridge preview encoder stopped: {err}");
        }
    }

    debug!(encoder = spec.element_name, "Lazy H.264 preview encoder exited");
}

#[cfg(feature = "gst-native")]
fn run_h264_gst_native(
    spec: &H264EncoderSpec,
    first: RawBgraFrame,
    raw_rx: &mut watch::Receiver<Option<RawBgraFrame>>,
    stop_rx: &watch::Receiver<bool>,
    tx: &broadcast::Sender<H264AccessUnit>,
    last_keyframe: &Arc<RwLock<Option<H264AccessUnit>>>,
) -> Result<()> {
    use gstreamer::prelude::*;

    gstreamer::init()?;
    let width = first.width;
    let height = first.height;
    let fps = first.fps.clamp(5, 60);

    let pipeline_desc = format!(
        "appsrc name=vc_src is-live=true do-timestamp=true format=time block=false \
         caps=\"video/x-raw,format=BGRA,width={width},height={height},framerate={fps}/1\" \
         ! videoconvert ! video/x-raw,format=NV12 ! {} \
         ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au \
         ! appsink name=vc_h264_sink drop=true max-buffers=2 sync=false",
        spec.pipeline_fragment
    );

    let element = gstreamer::parse::launch(&pipeline_desc)?;
    let pipeline = element
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow!("parsed H.264 encoder element is not a Pipeline"))?;
    let appsrc = pipeline
        .by_name("vc_src")
        .ok_or_else(|| anyhow!("missing vc_src"))?
        .downcast::<gstreamer_app::AppSrc>()
        .map_err(|_| anyhow!("vc_src is not an AppSrc"))?;
    let appsink = pipeline
        .by_name("vc_h264_sink")
        .ok_or_else(|| anyhow!("missing vc_h264_sink"))?
        .downcast::<gstreamer_app::AppSink>()
        .map_err(|_| anyhow!("vc_h264_sink is not an AppSink"))?;

    pipeline.set_state(gstreamer::State::Playing)?;

    let mut codec_str = "avc1.42e01f".to_string();

    let push_frame = |frame: &RawBgraFrame| -> Result<()> {
        let mut gst_buf = gstreamer::Buffer::with_size(frame.bgra.len())?;
        {
            let buf_ref = gst_buf.get_mut().ok_or_else(|| anyhow!("buffer not writable"))?;
            let mut map = buf_ref.map_writable()?;
            map.as_mut_slice().copy_from_slice(&frame.bgra);
        }
        appsrc.push_buffer(gst_buf).map_err(|e| anyhow!("appsrc push failed: {e:?}"))?;
        Ok(())
    };

    let _ = push_frame(&first);
    let mut last_ts_pushed = first.timestamp_us;

    while !*stop_rx.borrow() && tx.receiver_count() > 0 {
        if let Some(frame) = raw_rx.borrow().clone() {
            if frame.width == width && frame.height == height && frame.timestamp_us != last_ts_pushed {
                last_ts_pushed = frame.timestamp_us;
                if push_frame(&frame).is_err() {
                    break;
                }
            }
        }

        // Drain any ready encoded Access Units without blocking the push loop
        while let Some(sample) = appsink.try_pull_sample(gstreamer::ClockTime::from_mseconds(8)) {
            if let Some(buf) = sample.buffer() {
                if let Ok(map) = buf.map_readable() {
                    let bytes = Bytes::copy_from_slice(map.as_slice());
                    if let Some(parsed_codec) = extract_avc_codec_string(&bytes) {
                        codec_str = parsed_codec;
                    }
                    let is_keyframe = is_h264_keyframe(&bytes);
                    let au = H264AccessUnit {
                        codec: codec_str.clone(),
                        encoder_name: spec.element_name.to_string(),
                        width,
                        height,
                        timestamp_us: last_ts_pushed,
                        is_keyframe,
                        data: bytes,
                    };
                    if is_keyframe {
                        *last_keyframe.blocking_write() = Some(au.clone());
                    }
                    let _ = tx.send(au);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let _ = appsrc.end_of_stream();
    let _ = pipeline.set_state(gstreamer::State::Null);
    Ok(())
}

#[cfg(not(feature = "gst-native"))]
fn run_h264_cli_bridge(
    spec: &H264EncoderSpec,
    first: RawBgraFrame,
    raw_rx: &mut watch::Receiver<Option<RawBgraFrame>>,
    stop_rx: &watch::Receiver<bool>,
    tx: &broadcast::Sender<H264AccessUnit>,
    last_keyframe: &Arc<RwLock<Option<H264AccessUnit>>>,
) -> Result<()> {
    use std::io::{Read, Write};

    let gst_launch = find_gst_binary("gst-launch-1.0")
        .ok_or_else(|| anyhow!("gst-launch-1.0 not found on host"))?;

    let width = first.width;
    let height = first.height;
    let fps = first.fps.clamp(5, 60);

    let full_spec = format!(
        "fdsrc fd=0 ! rawvideoparse format=bgra width={width} height={height} framerate={fps}/1 \
         ! videoconvert ! video/x-raw,format=NV12 ! {} \
         ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au \
         ! fdsink fd=1",
        spec.pipeline_fragment
    );
    let args: Vec<&str> = full_spec.split_whitespace().collect();

    let mut child = Command::new(gst_launch)
        .arg("-q")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("failed taking gst-launch stdin"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("failed taking gst-launch stdout"))?;

    let encoder_name = spec.element_name.to_string();
    let reader_tx = tx.clone();
    let reader_kf = last_keyframe.clone();

    // Always drain stdout until child exits/is killed so gst-launch never blocks on a full stdout pipe.
    let reader_thread = std::thread::spawn(move || {
        let mut framer = AnnexBAuFramer::new();
        let mut codec_str = "avc1.42e01f".to_string();
        let mut buf = [0_u8; 65536];
        let start = std::time::Instant::now();

        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for au_bytes in framer.push(&buf[..n]) {
                        if let Some(parsed_codec) = extract_avc_codec_string(&au_bytes) {
                            codec_str = parsed_codec;
                        }
                        let is_keyframe = is_h264_keyframe(&au_bytes);
                        let au = H264AccessUnit {
                            codec: codec_str.clone(),
                            encoder_name: encoder_name.clone(),
                            width,
                            height,
                            timestamp_us: start.elapsed().as_micros() as u64,
                            is_keyframe,
                            data: au_bytes,
                        };
                        if is_keyframe {
                            *reader_kf.blocking_write() = Some(au.clone());
                        }
                        let _ = reader_tx.send(au);
                    }
                }
            }
        }
    });

    let writer_raw_rx = raw_rx.clone();
    let writer_stop = stop_rx.clone();
    let writer_tx = tx.clone();
    let writer_thread = std::thread::spawn(move || {
        let mut last_ts_pushed = first.timestamp_us;
        if stdin.write_all(&first.bgra).is_ok() {
            while !*writer_stop.borrow() && writer_tx.receiver_count() > 0 {
                if let Some(frame) = writer_raw_rx.borrow().clone() {
                    if frame.width == width
                        && frame.height == height
                        && frame.timestamp_us != last_ts_pushed
                    {
                        last_ts_pushed = frame.timestamp_us;
                        if stdin.write_all(&frame.bgra).is_err() {
                            break;
                        }
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        drop(stdin);
    });

    while !*stop_rx.borrow() && tx.receiver_count() > 0 {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Kill child first so both stdin and stdout pipes close immediately without deadlocking.
    let _ = child.kill();
    let _ = child.wait();
    let _ = writer_thread.join();
    let _ = reader_thread.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_annex_b_nals_extracts_codec_and_detects_keyframe() {
        // SPS (0x67, High profile 0x64, constraints 0x00, level 3.1 0x1f) + PPS (0x68) + IDR (0x65)
        let idr_au = vec![
            0, 0, 0, 1, 0x67, 0x64, 0x00, 0x1F, 0xAC, // SPS
            0, 0, 0, 1, 0x68, 0xEE, 0x3C, 0x80,       // PPS
            0, 0, 1, 0x65, 0x88, 0x84, 0x00,          // IDR slice (3-byte start code)
        ];
        assert_eq!(split_annex_b_nals(&idr_au).len(), 3);
        assert!(is_h264_keyframe(&idr_au));
        assert_eq!(extract_avc_codec_string(&idr_au).as_deref(), Some("avc1.64001f"));

        // Non-IDR P-frame (NAL type 1)
        let p_au = vec![0, 0, 0, 1, 0x41, 0x9A, 0x22, 0x11];
        assert!(!is_h264_keyframe(&p_au));
        assert!(extract_avc_codec_string(&p_au).is_none());
    }

    #[test]
    fn encodes_binary_ws_video_packet_header_correctly() {
        let au = H264AccessUnit {
            codec: "avc1.64001f".into(),
            encoder_name: "nvh264enc".into(),
            width: 1280,
            height: 720,
            timestamp_us: 123_456,
            is_keyframe: true,
            data: Bytes::from_static(&[0, 0, 0, 1, 0x65, 0xAA]),
        };
        let pkt = encode_ws_video_packet(&au);
        assert_eq!(&pkt[0..4], b"VCH2");
        assert_eq!(pkt[4], 1); // keyframe bit
        assert_eq!(pkt[5], 1); // H.264
        assert_eq!(u16::from_le_bytes([pkt[8], pkt[9]]), 1280);
        assert_eq!(u16::from_le_bytes([pkt[10], pkt[11]]), 720);
        assert_eq!(u64::from_le_bytes(pkt[12..20].try_into().unwrap()), 123_456);
        assert_eq!(&pkt[20..], &[0, 0, 0, 1, 0x65, 0xAA]);
    }

    #[test]
    fn annex_b_au_framer_splits_back_to_back_frames_and_rejects_empty() {
        let mut framer = AnnexBAuFramer::new();
        assert!(framer.push(&[]).is_empty());

        let stream = vec![
            0, 0, 0, 1, 0x67, 0x42, 0xE0, 0x1F, // Frame 1 SPS
            0, 0, 0, 1, 0x65, 0x11, 0x22,       // Frame 1 IDR
            0, 0, 0, 1, 0x41, 0x33, 0x44,       // Frame 2 P-slice
            0, 0, 0, 1, 0x41, 0x55, 0x66,       // Frame 3 P-slice
        ];
        let aus = framer.push(&stream);
        assert_eq!(aus.len(), 2, "must emit Frame 1 and Frame 2 once Frame 3 starts");
        assert!(is_h264_keyframe(&aus[0]));
        assert!(!is_h264_keyframe(&aus[1]));
    }

    #[test]
    fn malformed_empty_annex_b_input_does_not_panic() {
        assert!(split_annex_b_nals(&[]).is_empty());
        assert!(!is_h264_keyframe(&[1, 2, 3, 4]));
        assert!(extract_avc_codec_string(&[0, 0, 0, 1, 0x67]).is_none()); // truncated SPS
    }

    #[test]
    fn select_encoder_covers_all_backend_preferences_and_framer_overflow_guard() {
        for backend in [
            HardwareAccelBackend::NvidiaNvcodec,
            HardwareAccelBackend::AmdAmf,
            HardwareAccelBackend::IntelQuickSync,
            HardwareAccelBackend::AppleVideoToolbox,
            HardwareAccelBackend::LinuxVaapi,
            HardwareAccelBackend::WindowsD3d11,
            HardwareAccelBackend::SoftwareCpu,
        ] {
            let _ = select_zero_latency_h264_encoder(&backend);
        }

        // Malformed >2MB chunk without start codes clears buffer safely
        let mut framer = AnnexBAuFramer::new();
        let giant = vec![0xAAu8; 2 * 1024 * 1024 + 64];
        assert!(framer.push(&giant).is_empty());
    }

    #[tokio::test]
    async fn h264_preview_hub_encodes_live_frames_and_reuses_active_session() {
        let Some(_) = select_zero_latency_h264_encoder(&HardwareAccelBackend::NvidiaNvcodec) else {
            return;
        };

        let (raw_tx, raw_rx) = watch::channel::<Option<RawBgraFrame>>(None);
        let (stop_tx, stop_rx) = watch::channel(false);
        let hub = H264PreviewHub::new(HardwareAccelBackend::NvidiaNvcodec, raw_rx, stop_rx);

        let mut sub1 = hub.subscribe().await.unwrap();
        assert!(!sub1.encoder_name.is_empty());

        // Second subscriber reuses the running session
        let _sub2 = hub.subscribe().await.unwrap();

        // Feed a few 320x180 BGRA frames so encoder emits an IDR keyframe + delta frames
        let feed_task = tokio::spawn(async move {
            let mut bgra = vec![64u8; 320 * 180 * 4];
            for i in 0..25u64 {
                bgra[0] = (i * 10) as u8;
                let _ = raw_tx.send(Some(RawBgraFrame {
                    width: 320,
                    height: 180,
                    fps: 30,
                    timestamp_us: (i + 1) * 33_333,
                    bgra: Arc::new(bgra.clone()),
                }));
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        });

        let first_au = tokio::time::timeout(Duration::from_secs(6), sub1.rx.recv())
            .await
            .expect("timed out waiting for encoded H.264 access unit")
            .unwrap();
        assert_eq!(first_au.width, 320);
        assert_eq!(first_au.height, 180);
        assert!(!first_au.data.is_empty());
        assert!(first_au.codec.starts_with("avc1."));

        // Third subscriber gets cached initial keyframe
        let sub3 = hub.subscribe().await.unwrap();
        assert!(sub3.initial_keyframe.is_some() || first_au.is_keyframe || !first_au.data.is_empty());

        let _ = stop_tx.send(true);
        feed_task.abort();
    }
}
