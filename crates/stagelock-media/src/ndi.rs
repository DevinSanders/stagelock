//! Dynamic runtime loader for Vizrt NDI® 6 (`libndi` / `Processing.NDI.Lib.x64.dll`).
//!
//! NDI® is a registered trademark of Vizrt NDI AB (https://ndi.video).
//! Loaded dynamically at runtime so StageLock compiles without proprietary SDK headers
//! and degrades gracefully if the NDI runtime is not installed.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_float, c_int, c_void};
use std::ptr;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::capabilities::detect_ndi_runtime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NdiDiscoveredSource {
    pub name: String,
    pub url_address: String,
}

// --- C ABI definitions matching Vizrt NDI 6 SDK headers ---

#[repr(C)]
#[derive(Copy, Clone)]
pub struct NdiSourceC {
    pub p_ndi_name: *const c_char,
    pub p_url_address: *const c_char,
}

#[repr(C)]
pub struct NdiFindCreateC {
    pub show_local_sources: bool,
    pub p_groups: *const c_char,
    pub p_extra_ips: *const c_char,
}

#[repr(C)]
pub struct NdiSendCreateC {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    pub clock_video: bool,
    pub clock_audio: bool,
}

#[repr(C)]
pub struct NdiRecvCreateV3C {
    pub source_to_connect_to: NdiSourceC,
    pub color_format: c_int,
    pub bandwidth: c_int,
    pub allow_video_fields: bool,
    pub p_ndi_recv_name: *const c_char,
}

#[repr(C)]
pub struct NdiVideoFrameV2C {
    pub xres: c_int,
    pub yres: c_int,
    pub fourcc: u32,
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub picture_aspect_ratio: c_float,
    pub frame_format_type: c_int,
    pub timecode: i64,
    pub p_data: *mut u8,
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

pub const NDI_FOURCC_BGRA: u32 = u32::from_le_bytes(*b"BGRA");
pub const NDI_FOURCC_BGRX: u32 = u32::from_le_bytes(*b"BGRX");
pub const NDI_FOURCC_RGBA: u32 = u32::from_le_bytes(*b"RGBA");
pub const NDI_FOURCC_UYVY: u32 = u32::from_le_bytes(*b"UYVY");

pub const NDI_RECV_COLOR_FORMAT_BGRX_BGRA: c_int = 0;
pub const NDI_RECV_BANDWIDTH_HIGHEST: c_int = 100;
pub const NDI_RECV_BANDWIDTH_LOWEST: c_int = 0;
pub const NDI_FRAME_FORMAT_PROGRESSIVE: c_int = 1;
pub const NDI_FRAME_TYPE_VIDEO: c_int = 1;
pub const NDI_SEND_TIMECODE_SYNTHESIZE: i64 = i64::MAX;

type FnInitialize = unsafe extern "C" fn() -> bool;
type FnDestroy = unsafe extern "C" fn();
type FnFindCreateV2 = unsafe extern "C" fn(*const NdiFindCreateC) -> *mut c_void;
type FnFindDestroy = unsafe extern "C" fn(*mut c_void);
type FnFindWaitForSources = unsafe extern "C" fn(*mut c_void, u32) -> bool;
type FnFindGetCurrentSources = unsafe extern "C" fn(*mut c_void, *mut u32) -> *const NdiSourceC;
type FnSendCreate = unsafe extern "C" fn(*const NdiSendCreateC) -> *mut c_void;
type FnSendDestroy = unsafe extern "C" fn(*mut c_void);
type FnSendSendVideoV2 = unsafe extern "C" fn(*mut c_void, *const NdiVideoFrameV2C);
type FnRecvCreateV3 = unsafe extern "C" fn(*const NdiRecvCreateV3C) -> *mut c_void;
type FnRecvDestroy = unsafe extern "C" fn(*mut c_void);
type FnRecvCaptureV3 = unsafe extern "C" fn(
    *mut c_void,
    *mut NdiVideoFrameV2C,
    *mut c_void,
    *mut c_void,
    u32,
) -> c_int;
type FnRecvFreeVideoV2 = unsafe extern "C" fn(*mut c_void, *const NdiVideoFrameV2C);
type FnRecvPtzIsSupported = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzPanTiltSpeed = unsafe extern "C" fn(*mut c_void, c_float, c_float) -> bool;
type FnRecvPtzZoomSpeed = unsafe extern "C" fn(*mut c_void, c_float) -> bool;
type FnRecvPtzAutoFocus = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzFocusSpeed = unsafe extern "C" fn(*mut c_void, c_float) -> bool;
type FnRecvPtzStorePreset = unsafe extern "C" fn(*mut c_void, c_int) -> bool;
type FnRecvPtzRecallPreset = unsafe extern "C" fn(*mut c_void, c_int, c_float) -> bool;
type FnRecvPtzWhiteBalanceAuto = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzWhiteBalanceIndoor = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzWhiteBalanceOutdoor = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzWhiteBalanceOneshot = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzWhiteBalanceManual = unsafe extern "C" fn(*mut c_void, c_float, c_float) -> bool;
type FnRecvPtzExposureAuto = unsafe extern "C" fn(*mut c_void) -> bool;
type FnRecvPtzExposureManual = unsafe extern "C" fn(*mut c_void, c_float) -> bool;

struct NdiApi {
    _lib: Library,
    destroy: FnDestroy,
    find_create_v2: FnFindCreateV2,
    find_destroy: FnFindDestroy,
    find_wait_for_sources: FnFindWaitForSources,
    find_get_current_sources: FnFindGetCurrentSources,
    send_create: FnSendCreate,
    send_destroy: FnSendDestroy,
    send_send_video_v2: FnSendSendVideoV2,
    recv_create_v3: FnRecvCreateV3,
    recv_destroy: FnRecvDestroy,
    recv_capture_v3: FnRecvCaptureV3,
    recv_free_video_v2: FnRecvFreeVideoV2,
    recv_ptz_is_supported: FnRecvPtzIsSupported,
    recv_ptz_pan_tilt_speed: FnRecvPtzPanTiltSpeed,
    recv_ptz_zoom_speed: FnRecvPtzZoomSpeed,
    recv_ptz_auto_focus: FnRecvPtzAutoFocus,
    recv_ptz_focus_speed: FnRecvPtzFocusSpeed,
    recv_ptz_store_preset: FnRecvPtzStorePreset,
    recv_ptz_recall_preset: FnRecvPtzRecallPreset,
    recv_ptz_wb_auto: FnRecvPtzWhiteBalanceAuto,
    recv_ptz_wb_indoor: FnRecvPtzWhiteBalanceIndoor,
    recv_ptz_wb_outdoor: FnRecvPtzWhiteBalanceOutdoor,
    recv_ptz_wb_oneshot: FnRecvPtzWhiteBalanceOneshot,
    recv_ptz_wb_manual: FnRecvPtzWhiteBalanceManual,
    recv_ptz_exposure_auto: FnRecvPtzExposureAuto,
    recv_ptz_exposure_manual: FnRecvPtzExposureManual,
}

unsafe impl Send for NdiApi {}
unsafe impl Sync for NdiApi {}

impl Drop for NdiApi {
    fn drop(&mut self) {
        unsafe {
            (self.destroy)();
        }
    }
}

#[derive(Clone)]
pub struct NdiRuntime {
    api: Arc<NdiApi>,
    pub library_path: String,
}

static SHARED_NDI_RUNTIME: OnceLock<std::result::Result<NdiRuntime, String>> = OnceLock::new();

impl NdiRuntime {
    /// Attempt to dynamically load the NDI 6 runtime library from the host system.
    /// Cached process-wide so `NDIlib_initialize()` / `NDIlib_destroy()` are never torn down
    /// underneath concurrent cameras or test tasks.
    pub fn try_load() -> Result<Self> {
        SHARED_NDI_RUNTIME
            .get_or_init(|| Self::try_load_uncached().map_err(|e| e.to_string()))
            .clone()
            .map_err(|msg| anyhow!(msg))
    }

    fn try_load_uncached() -> Result<Self> {
        let (available, path) = detect_ndi_runtime();
        let path = match (available, path) {
            (true, Some(p)) => p,
            _ => return Err(anyhow!("NDI runtime library not found (install NDI Tools from https://ndi.video/tools)")),
        };

        unsafe {
            let lib = Library::new(&path)
                .with_context(|| format!("failed loading NDI library from {path}"))?;

            let initialize: Symbol<FnInitialize> = lib.get(b"NDIlib_initialize\0")?;
            if !initialize() {
                return Err(anyhow!("NDIlib_initialize() returned false (unsupported CPU or runtime)"));
            }

            let api = NdiApi {
                destroy: *lib.get::<FnDestroy>(b"NDIlib_destroy\0")?,
                find_create_v2: *lib.get::<FnFindCreateV2>(b"NDIlib_find_create_v2\0")?,
                find_destroy: *lib.get::<FnFindDestroy>(b"NDIlib_find_destroy\0")?,
                find_wait_for_sources: *lib.get::<FnFindWaitForSources>(b"NDIlib_find_wait_for_sources\0")?,
                find_get_current_sources: *lib.get::<FnFindGetCurrentSources>(b"NDIlib_find_get_current_sources\0")?,
                send_create: *lib.get::<FnSendCreate>(b"NDIlib_send_create\0")?,
                send_destroy: *lib.get::<FnSendDestroy>(b"NDIlib_send_destroy\0")?,
                send_send_video_v2: *lib.get::<FnSendSendVideoV2>(b"NDIlib_send_send_video_v2\0")?,
                recv_create_v3: *lib.get::<FnRecvCreateV3>(b"NDIlib_recv_create_v3\0")?,
                recv_destroy: *lib.get::<FnRecvDestroy>(b"NDIlib_recv_destroy\0")?,
                recv_capture_v3: *lib.get::<FnRecvCaptureV3>(b"NDIlib_recv_capture_v3\0")?,
                recv_free_video_v2: *lib.get::<FnRecvFreeVideoV2>(b"NDIlib_recv_free_video_v2\0")?,
                recv_ptz_is_supported: *lib.get::<FnRecvPtzIsSupported>(b"NDIlib_recv_ptz_is_supported\0")?,
                recv_ptz_pan_tilt_speed: *lib.get::<FnRecvPtzPanTiltSpeed>(b"NDIlib_recv_ptz_pan_tilt_speed\0")?,
                recv_ptz_zoom_speed: *lib.get::<FnRecvPtzZoomSpeed>(b"NDIlib_recv_ptz_zoom_speed\0")?,
                recv_ptz_auto_focus: *lib.get::<FnRecvPtzAutoFocus>(b"NDIlib_recv_ptz_auto_focus\0")?,
                recv_ptz_focus_speed: *lib.get::<FnRecvPtzFocusSpeed>(b"NDIlib_recv_ptz_focus_speed\0")?,
                recv_ptz_store_preset: *lib.get::<FnRecvPtzStorePreset>(b"NDIlib_recv_ptz_store_preset\0")?,
                recv_ptz_recall_preset: *lib.get::<FnRecvPtzRecallPreset>(b"NDIlib_recv_ptz_recall_preset\0")?,
                recv_ptz_wb_auto: *lib.get::<FnRecvPtzWhiteBalanceAuto>(b"NDIlib_recv_ptz_white_balance_auto\0")?,
                recv_ptz_wb_indoor: *lib.get::<FnRecvPtzWhiteBalanceIndoor>(b"NDIlib_recv_ptz_white_balance_indoor\0")?,
                recv_ptz_wb_outdoor: *lib.get::<FnRecvPtzWhiteBalanceOutdoor>(b"NDIlib_recv_ptz_white_balance_outdoor\0")?,
                recv_ptz_wb_oneshot: *lib.get::<FnRecvPtzWhiteBalanceOneshot>(b"NDIlib_recv_ptz_white_balance_oneshot\0")?,
                recv_ptz_wb_manual: *lib.get::<FnRecvPtzWhiteBalanceManual>(b"NDIlib_recv_ptz_white_balance_manual\0")?,
                recv_ptz_exposure_auto: *lib.get::<FnRecvPtzExposureAuto>(b"NDIlib_recv_ptz_exposure_auto\0")?,
                recv_ptz_exposure_manual: *lib.get::<FnRecvPtzExposureManual>(b"NDIlib_recv_ptz_exposure_manual\0")?,
                _lib: lib,
            };

            info!(path = %path, "Loaded NDI 6 runtime dynamically");
            Ok(Self {
                api: Arc::new(api),
                library_path: path,
            })
        }
    }

    /// Discover active NDI sources on the local network (blocks briefly up to `timeout_ms`).
    pub fn discover_sources(&self, timeout_ms: u32) -> Result<Vec<NdiDiscoveredSource>> {
        unsafe {
            let settings = NdiFindCreateC {
                show_local_sources: true,
                p_groups: ptr::null(),
                p_extra_ips: ptr::null(),
            };
            let finder = (self.api.find_create_v2)(&settings);
            if finder.is_null() {
                return Err(anyhow!("NDIlib_find_create_v2 returned null"));
            }

            let _ = (self.api.find_wait_for_sources)(finder, timeout_ms);
            let mut count: u32 = 0;
            let sources_ptr = (self.api.find_get_current_sources)(finder, &mut count);

            let mut results = Vec::new();
            if !sources_ptr.is_null() && count > 0 {
                let slice = std::slice::from_raw_parts(sources_ptr, count as usize);
                for src in slice {
                    let name = if src.p_ndi_name.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(src.p_ndi_name).to_string_lossy().to_string()
                    };
                    let url_address = if src.p_url_address.is_null() {
                        String::new()
                    } else {
                        CStr::from_ptr(src.p_url_address).to_string_lossy().to_string()
                    };
                    if !name.is_empty() {
                        results.push(NdiDiscoveredSource { name, url_address });
                    }
                }
            }

            (self.api.find_destroy)(finder);
            Ok(results)
        }
    }

    /// Create an NDI sender to re-broadcast any video source over NDI®.
    pub fn create_sender(&self, stream_name: &str, groups: Option<&str>) -> Result<NdiSender> {
        let c_name = CString::new(stream_name)?;
        let c_groups = match groups {
            Some(g) if !g.trim().is_empty() => Some(CString::new(g)?),
            _ => None,
        };

        unsafe {
            let settings = NdiSendCreateC {
                p_ndi_name: c_name.as_ptr(),
                p_groups: c_groups.as_ref().map_or(ptr::null(), |s| s.as_ptr()),
                clock_video: false,
                clock_audio: false,
            };
            let handle = (self.api.send_create)(&settings);
            if handle.is_null() {
                return Err(anyhow!("NDIlib_send_create failed for '{stream_name}'"));
            }
            info!(stream = %stream_name, "Started NDI sender");
            Ok(NdiSender {
                api: self.api.clone(),
                handle,
                stream_name: stream_name.to_string(),
            })
        }
    }

    /// Create an NDI receiver connected to a named NDI source.
    pub fn create_receiver(&self, source_name: &str, low_bandwidth: bool) -> Result<NdiReceiver> {
        let c_source_name = CString::new(source_name)?;
        let c_recv_name = CString::new(format!("StageLock ({source_name})"))?;

        unsafe {
            let settings = NdiRecvCreateV3C {
                source_to_connect_to: NdiSourceC {
                    p_ndi_name: c_source_name.as_ptr(),
                    p_url_address: ptr::null(),
                },
                color_format: NDI_RECV_COLOR_FORMAT_BGRX_BGRA,
                bandwidth: if low_bandwidth {
                    NDI_RECV_BANDWIDTH_LOWEST
                } else {
                    NDI_RECV_BANDWIDTH_HIGHEST
                },
                allow_video_fields: false,
                p_ndi_recv_name: c_recv_name.as_ptr(),
            };
            let handle = (self.api.recv_create_v3)(&settings);
            if handle.is_null() {
                return Err(anyhow!("NDIlib_recv_create_v3 failed for '{source_name}'"));
            }
            info!(source = %source_name, "Connected NDI receiver");
            Ok(NdiReceiver {
                api: self.api.clone(),
                handle,
            })
        }
    }
}

pub struct NdiSender {
    api: Arc<NdiApi>,
    handle: *mut c_void,
    pub stream_name: String,
}

unsafe impl Send for NdiSender {}

impl NdiSender {
    pub fn send_bgra_frame(&self, width: u32, height: u32, fps: u32, bgra: &[u8]) {
        if bgra.len() < (width * height * 4) as usize {
            warn!("NDI sender skipped undersized BGRA frame");
            return;
        }
        let frame = NdiVideoFrameV2C {
            xres: width as c_int,
            yres: height as c_int,
            fourcc: NDI_FOURCC_BGRA,
            frame_rate_n: (fps.max(1) * 1000) as c_int,
            frame_rate_d: 1000,
            picture_aspect_ratio: width as f32 / height.max(1) as f32,
            frame_format_type: NDI_FRAME_FORMAT_PROGRESSIVE,
            timecode: NDI_SEND_TIMECODE_SYNTHESIZE,
            p_data: bgra.as_ptr() as *mut u8,
            line_stride_in_bytes: (width * 4) as c_int,
            p_metadata: ptr::null(),
            timestamp: 0,
        };
        unsafe {
            (self.api.send_send_video_v2)(self.handle, &frame);
        }
    }
}

impl Drop for NdiSender {
    fn drop(&mut self) {
        unsafe {
            (self.api.send_destroy)(self.handle);
        }
    }
}

pub struct NdiReceiver {
    api: Arc<NdiApi>,
    handle: *mut c_void,
}

unsafe impl Send for NdiReceiver {}
unsafe impl Sync for NdiReceiver {}

pub struct ReceivedBgraFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl NdiReceiver {
    pub fn capture_bgra(&self, timeout_ms: u32) -> Option<ReceivedBgraFrame> {
        unsafe {
            let mut frame = NdiVideoFrameV2C {
                xres: 0,
                yres: 0,
                fourcc: 0,
                frame_rate_n: 0,
                frame_rate_d: 0,
                picture_aspect_ratio: 0.0,
                frame_format_type: 0,
                timecode: 0,
                p_data: ptr::null_mut(),
                line_stride_in_bytes: 0,
                p_metadata: ptr::null(),
                timestamp: 0,
            };

            let kind = (self.api.recv_capture_v3)(
                self.handle,
                &mut frame,
                ptr::null_mut(),
                ptr::null_mut(),
                timeout_ms,
            );

            if kind != NDI_FRAME_TYPE_VIDEO || frame.p_data.is_null() || frame.xres <= 0 || frame.yres <= 0 {
                return None;
            }

            let width = frame.xres as u32;
            let height = frame.yres as u32;
            let stride = frame.line_stride_in_bytes.max(frame.xres * 4) as usize;
            let row_bytes = (width * 4) as usize;
            let mut out = vec![0u8; row_bytes * height as usize];

            for y in 0..height as usize {
                let src_row = std::slice::from_raw_parts(frame.p_data.add(y * stride), row_bytes);
                out[y * row_bytes..(y + 1) * row_bytes].copy_from_slice(src_row);
            }

            (self.api.recv_free_video_v2)(self.handle, &frame);
            Some(ReceivedBgraFrame {
                width,
                height,
                bgra: out,
            })
        }
    }

    pub fn ptz_is_supported(&self) -> bool {
        unsafe { (self.api.recv_ptz_is_supported)(self.handle) }
    }

    pub fn ptz_pan_tilt_speed(&self, pan: f32, tilt: f32) -> bool {
        unsafe { (self.api.recv_ptz_pan_tilt_speed)(self.handle, pan.clamp(-1.0, 1.0), tilt.clamp(-1.0, 1.0)) }
    }

    pub fn ptz_zoom_speed(&self, zoom: f32) -> bool {
        unsafe { (self.api.recv_ptz_zoom_speed)(self.handle, zoom.clamp(-1.0, 1.0)) }
    }

    pub fn ptz_auto_focus(&self) -> bool {
        unsafe { (self.api.recv_ptz_auto_focus)(self.handle) }
    }

    pub fn ptz_focus_speed(&self, speed: f32) -> bool {
        unsafe { (self.api.recv_ptz_focus_speed)(self.handle, speed.clamp(-1.0, 1.0)) }
    }

    pub fn ptz_store_preset(&self, slot: i32) -> bool {
        unsafe { (self.api.recv_ptz_store_preset)(self.handle, slot) }
    }

    pub fn ptz_recall_preset(&self, slot: i32, speed: f32) -> bool {
        unsafe { (self.api.recv_ptz_recall_preset)(self.handle, slot, speed.clamp(0.0, 1.0)) }
    }

    pub fn ptz_white_balance_auto(&self) -> bool {
        unsafe { (self.api.recv_ptz_wb_auto)(self.handle) }
    }

    pub fn ptz_white_balance_indoor(&self) -> bool {
        unsafe { (self.api.recv_ptz_wb_indoor)(self.handle) }
    }

    pub fn ptz_white_balance_outdoor(&self) -> bool {
        unsafe { (self.api.recv_ptz_wb_outdoor)(self.handle) }
    }

    pub fn ptz_white_balance_oneshot(&self) -> bool {
        unsafe { (self.api.recv_ptz_wb_oneshot)(self.handle) }
    }

    pub fn ptz_white_balance_manual(&self, red: f32, blue: f32) -> bool {
        unsafe { (self.api.recv_ptz_wb_manual)(self.handle, red.clamp(0.0, 1.0), blue.clamp(0.0, 1.0)) }
    }

    pub fn ptz_exposure_auto(&self) -> bool {
        unsafe { (self.api.recv_ptz_exposure_auto)(self.handle) }
    }

    pub fn ptz_exposure_manual(&self, level: f32) -> bool {
        unsafe { (self.api.recv_ptz_exposure_manual)(self.handle, level.clamp(0.0, 1.0)) }
    }
}

impl Drop for NdiReceiver {
    fn drop(&mut self) {
        unsafe {
            (self.api.recv_destroy)(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ndi_runtime_sender_receiver_and_ptz_ffi_roundtrip_when_installed() {
        let Ok(rt) = NdiRuntime::try_load() else {
            // Host without NDI runtime installed still verified graceful Err above
            return;
        };
        assert!(!rt.library_path.is_empty());

        // Quick discovery call
        let _ = rt.discover_sources(100).unwrap();

        // Create sender with groups and send valid + undersized frame (guard path)
        let sender = rt
            .create_sender("StageLock-UnitTest-Feed", Some("public"))
            .unwrap();
        assert_eq!(sender.stream_name, "StageLock-UnitTest-Feed");
        sender.send_bgra_frame(64, 64, 30, &[0u8; 16]); // undersized guard
        let valid_frame = vec![128u8; 64 * 64 * 4];
        sender.send_bgra_frame(64, 64, 30, &valid_frame);

        // Create receiver (low_bandwidth true & false) and exercise capture timeout + all PTZ FFI methods
        let rx_low = rt.create_receiver("NONEXISTENT_NDI_SOURCE_TEST", true).unwrap();
        assert!(rx_low.capture_bgra(30).is_none());
        let _ = rx_low.ptz_is_supported();
        let _ = rx_low.ptz_pan_tilt_speed(0.5, -0.5);
        let _ = rx_low.ptz_zoom_speed(0.3);
        let _ = rx_low.ptz_auto_focus();
        let _ = rx_low.ptz_focus_speed(0.2);
        let _ = rx_low.ptz_store_preset(1);
        let _ = rx_low.ptz_recall_preset(1, 0.8);
        let _ = rx_low.ptz_white_balance_auto();
        let _ = rx_low.ptz_white_balance_indoor();
        let _ = rx_low.ptz_white_balance_outdoor();
        let _ = rx_low.ptz_white_balance_oneshot();
        let _ = rx_low.ptz_white_balance_manual(0.5, 0.5);
        let _ = rx_low.ptz_exposure_auto();
        let _ = rx_low.ptz_exposure_manual(0.5);

        let rx_hi = rt.create_receiver("NONEXISTENT_NDI_SOURCE_TEST_2", false).unwrap();
        assert!(rx_hi.capture_bgra(20).is_none());
    }
}
