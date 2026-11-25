//! ONNX Runtime (`ort` v2) multi-Execution-Provider inference manager for permissive
//! Apache-2.0 / MIT vision models:
//! - `rtmdet-s.onnx` (Person Detection, Apache-2.0)
//! - `rtmpose-s.onnx` (17-Keypoint COCO Pose, Apache-2.0)
//! - `osnet_x0_25.onnx` (512-dim Body Appearance Re-ID, MIT)
//!
//! Degrades gracefully when ONNX weights have not yet been downloaded into `models/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::info;
use stagelock_core::{Keypoint2D, NormalizedRect};

use crate::reid::AppearanceEmbedding;
use crate::tracker::RawDetection;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelFileStatus {
    pub role: String,
    pub filename: String,
    pub license: String,
    pub installed: bool,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionEngineStatus {
    pub models_dir: String,
    pub onnx_feature_compiled: bool,
    pub active_execution_provider: String,
    pub detector_ready: bool,
    pub pose_ready: bool,
    pub reid_ready: bool,
    pub models: Vec<ModelFileStatus>,
}

pub fn default_models_dir() -> PathBuf {
    if let Some(env_dir) = std::env::var("STAGELOCK_MODELS_DIR")
        .ok()
        .or_else(|| std::env::var("VC_MODELS_DIR").ok())
    {
        return PathBuf::from(env_dir);
    }
    let cwd_models = PathBuf::from("models");
    if cwd_models.exists() {
        return cwd_models;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let repo_models = root.join("models");
            if repo_models.exists() {
                return repo_models;
            }
        }
    }
    cwd_models
}

pub fn probe_vision_status(models_dir: &Path) -> VisionEngineStatus {
    let specs = [
        ("detector", "rtmdet-s.onnx", "Apache-2.0"),
        ("pose", "rtmpose-s.onnx", "Apache-2.0"),
        ("reid", "osnet_x0_25.onnx", "MIT"),
    ];

    let mut models = Vec::new();
    for (role, filename, license) in specs {
        let p = models_dir.join(filename);
        let meta = std::fs::metadata(&p).ok();
        models.push(ModelFileStatus {
            role: role.to_string(),
            filename: filename.to_string(),
            license: license.to_string(),
            installed: meta.is_some(),
            size_bytes: meta.map(|m| m.len()),
        });
    }

    let detector_ready = models[0].installed && cfg!(feature = "onnx");
    let pose_ready = models[1].installed && cfg!(feature = "onnx");
    let reid_ready = models[2].installed && cfg!(feature = "onnx");

    let active_ep = if cfg!(feature = "onnx-cuda") {
        "CUDA / TensorRT (ONNX Runtime)".to_string()
    } else if cfg!(feature = "onnx-directml") {
        "DirectML GPU (ONNX Runtime)".to_string()
    } else if cfg!(feature = "onnx-coreml") {
        "CoreML ANE/Metal (ONNX Runtime)".to_string()
    } else if cfg!(feature = "onnx") {
        "CPU (ONNX Runtime)".to_string()
    } else {
        "Heuristic + Spatial HSV Fallback (build with --features onnx for neural models)".to_string()
    };

    VisionEngineStatus {
        models_dir: models_dir.to_string_lossy().to_string(),
        onnx_feature_compiled: cfg!(feature = "onnx"),
        active_execution_provider: active_ep,
        detector_ready,
        pose_ready,
        reid_ready,
        models,
    }
}

/// Runtime ONNX inference engine with graceful fallback when models aren't installed.
pub struct OnnxVisionEngine {
    pub status: VisionEngineStatus,
    #[cfg(feature = "onnx")]
    detector: Option<ort::session::Session>,
    #[cfg(feature = "onnx")]
    pose: Option<ort::session::Session>,
    #[cfg(feature = "onnx")]
    reid: Option<ort::session::Session>,
}

impl OnnxVisionEngine {
    pub fn load_from_dir(models_dir: &Path) -> Self {
        let status = probe_vision_status(models_dir);

        #[cfg(feature = "onnx")]
        {
            let _ = ort::init().commit();
            let load_session = |filename: &str| -> Option<ort::session::Session> {
                let path = models_dir.join(filename);
                if !path.exists() {
                    return None;
                }
                let builder = ort::session::Session::builder().ok()?.with_intra_threads(4).ok()?;
                match builder.commit_from_file(&path) {
                    Ok(s) => {
                        info!(model = %path.display(), "Loaded ONNX model session");
                        Some(s)
                    }
                    Err(err) => {
                        tracing::warn!(model = %path.display(), "Failed loading ONNX model: {err}");
                        None
                    }
                }
            };

            let detector = load_session("rtmdet-s.onnx");
            let pose = load_session("rtmpose-s.onnx");
            let reid = load_session("osnet_x0_25.onnx");
            return Self {
                status,
                detector,
                pose,
                reid,
            };
        }

        #[cfg(not(feature = "onnx"))]
        {
            info!(
                models_dir = %status.models_dir,
                "Initialized VisionEngine (tracking + spatial Re-ID + ROI sharpness active)"
            );
            Self { status }
        }
    }

    pub fn has_neural_detector(&self) -> bool {
        #[cfg(feature = "onnx")]
        {
            self.detector.is_some()
        }
        #[cfg(not(feature = "onnx"))]
        {
            false
        }
    }

    /// Run person detection via RTMDet ONNX (`[1, 3, 640, 640]`), returning normalized person detections.
    pub fn detect_persons(&mut self, bgra: &[u8], width: u32, height: u32) -> Vec<RawDetection> {
        #[cfg(feature = "onnx")]
        {
            let Some(ref mut sess) = self.detector else {
                return Vec::new();
            };
            let (input_vec, scale, pad_x, pad_y) = preprocess_letterbox_640(bgra, width, height);
            let Ok(tensor) = ort::value::Tensor::from_array(([1usize, 3, 640, 640], input_vec)) else {
                return Vec::new();
            };
            let Ok(outputs) = sess.run(ort::inputs![tensor]) else {
                return Vec::new();
            };
            // RTMDet ONNX standard outputs: dets [1, N, 5] (x1, y1, x2, y2, score) + labels [1, N]
            if outputs.len() >= 2 {
                if let (Ok((dets_shape, dets_data)), Ok((_, labels_data))) = (
                    outputs[0].try_extract_tensor::<f32>(),
                    outputs[1].try_extract_tensor::<i64>(),
                ) {
                    let n = if dets_shape.len() == 3 { dets_shape[1] as usize } else { 0 };
                    let mut out = Vec::new();
                    for i in 0..n {
                        if labels_data.get(i).copied().unwrap_or(-1) != 0 {
                            continue; // COCO class 0 = person
                        }
                        let base = i * 5;
                        let score = dets_data[base + 4];
                        if score < 0.25 {
                            continue;
                        }
                        let x1 = ((dets_data[base] - pad_x) / scale / width as f32).clamp(0.0, 1.0);
                        let y1 = ((dets_data[base + 1] - pad_y) / scale / height as f32).clamp(0.0, 1.0);
                        let x2 = ((dets_data[base + 2] - pad_x) / scale / width as f32).clamp(0.0, 1.0);
                        let y2 = ((dets_data[base + 3] - pad_y) / scale / height as f32).clamp(0.0, 1.0);
                        if x2 > x1 + 0.01 && y2 > y1 + 0.01 {
                            out.push(RawDetection {
                                bbox: NormalizedRect {
                                    x: x1,
                                    y: y1,
                                    width: x2 - x1,
                                    height: y2 - y1,
                                },
                                confidence: score,
                                keypoints: Vec::new(),
                                embedding: None,
                            });
                        }
                    }
                    return out;
                }
            }
            Vec::new()
        }

        #[cfg(not(feature = "onnx"))]
        {
            let _ = (bgra, width, height);
            Vec::new()
        }
    }

    /// Run 17-keypoint COCO pose estimation via RTMPose SimCC (`[1, 3, 256, 192]`) on a person crop.
    pub fn estimate_pose(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        bbox: &NormalizedRect,
    ) -> Vec<Keypoint2D> {
        #[cfg(feature = "onnx")]
        {
            let Some(ref mut sess) = self.pose else {
                return Vec::new();
            };
            let input = crop_resize_normalize_chw(bgra, width, height, bbox, 192, 256);
            let Ok(tensor) = ort::value::Tensor::from_array(([1usize, 3, 256, 192], input)) else {
                return Vec::new();
            };
            let Ok(outputs) = sess.run(ort::inputs![tensor]) else {
                return Vec::new();
            };
            if outputs.len() >= 2 {
                if let (Ok((sx, dx)), Ok((sy, dy))) = (
                    outputs[0].try_extract_tensor::<f32>(),
                    outputs[1].try_extract_tensor::<f32>(),
                ) {
                    if sx.len() == 3 && sy.len() == 3 {
                        let num_kps = sx[1] as usize;
                        let bins_x = sx[2] as usize;
                        let bins_y = sy[2] as usize;
                        let mut kps = Vec::with_capacity(num_kps);
                        for k in 0..num_kps {
                            let x_slice = &dx[k * bins_x..(k + 1) * bins_x];
                            let y_slice = &dy[k * bins_y..(k + 1) * bins_y];
                            let (bx, mx) = argmax_f32(x_slice);
                            let (by, my) = argmax_f32(y_slice);
                            let rx = bx as f32 / bins_x.max(1) as f32;
                            let ry = by as f32 / bins_y.max(1) as f32;
                            kps.push(Keypoint2D {
                                x: (bbox.x + rx * bbox.width).clamp(0.0, 1.0),
                                y: (bbox.y + ry * bbox.height).clamp(0.0, 1.0),
                                score: ((mx + my) * 0.5).clamp(0.0, 1.0),
                            });
                        }
                        return kps;
                    }
                }
            }
            Vec::new()
        }

        #[cfg(not(feature = "onnx"))]
        {
            let _ = (bgra, width, height, bbox);
            Vec::new()
        }
    }

    /// Extract 512-dim body appearance Re-ID embedding via OSNet (`[1, 3, 256, 128]`).
    pub fn extract_reid_embedding(
        &mut self,
        bgra: &[u8],
        width: u32,
        height: u32,
        bbox: &NormalizedRect,
    ) -> Option<AppearanceEmbedding> {
        #[cfg(feature = "onnx")]
        {
            let sess = self.reid.as_mut()?;
            let input = crop_resize_normalize_chw(bgra, width, height, bbox, 128, 256);
            let tensor = ort::value::Tensor::from_array(([1usize, 3, 256, 128], input)).ok()?;
            let outputs = sess.run(ort::inputs![tensor]).ok()?;
            let (_, data) = outputs[0].try_extract_tensor::<f32>().ok()?;
            Some(AppearanceEmbedding::from_raw(data.to_vec()))
        }

        #[cfg(not(feature = "onnx"))]
        {
            let _ = (bgra, width, height, bbox);
            None
        }
    }
}

#[cfg(feature = "onnx")]
fn argmax_f32(slice: &[f32]) -> (usize, f32) {
    let mut best_i = 0;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &v) in slice.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best_i = i;
        }
    }
    (best_i, best_v)
}

#[cfg(feature = "onnx")]
fn preprocess_letterbox_640(bgra: &[u8], width: u32, height: u32) -> (Vec<f32>, f32, f32, f32) {
    let dst = 640usize;
    let scale = (dst as f32 / width.max(1) as f32).min(dst as f32 / height.max(1) as f32);
    let new_w = (width as f32 * scale).round() as usize;
    let new_h = (height as f32 * scale).round() as usize;
    let pad_x = (dst - new_w) / 2;
    let pad_y = (dst - new_h) / 2;

    let mean = [123.675_f32, 116.28, 103.53];
    let std = [58.395_f32, 57.12, 57.375];
    let plane = dst * dst;
    let mut out = vec![0.0_f32; 3 * plane];
    let w = width as usize;

    for dy in 0..new_h {
        let sy = ((dy as f32 / scale) as usize).min(height as usize - 1);
        for dx in 0..new_w {
            let sx = ((dx as f32 / scale) as usize).min(width as usize - 1);
            let si = (sy * w + sx) * 4;
            let di = (dy + pad_y) * dst + (dx + pad_x);
            out[di] = (bgra[si + 2] as f32 - mean[0]) / std[0];
            out[plane + di] = (bgra[si + 1] as f32 - mean[1]) / std[1];
            out[2 * plane + di] = (bgra[si] as f32 - mean[2]) / std[2];
        }
    }
    (out, scale, pad_x as f32, pad_y as f32)
}

#[cfg(feature = "onnx")]
fn crop_resize_normalize_chw(
    bgra: &[u8],
    width: u32,
    height: u32,
    bbox: &NormalizedRect,
    dst_w: usize,
    dst_h: usize,
) -> Vec<f32> {
    let mean = [123.675_f32, 116.28, 103.53];
    let std = [58.395_f32, 57.12, 57.375];
    let plane = dst_w * dst_h;
    let mut out = vec![0.0_f32; 3 * plane];
    let w = width as usize;
    let h = height as usize;

    let x0 = (bbox.x * width as f32).clamp(0.0, (w.saturating_sub(1)) as f32);
    let y0 = (bbox.y * height as f32).clamp(0.0, (h.saturating_sub(1)) as f32);
    let bw = (bbox.width * width as f32).max(1.0);
    let bh = (bbox.height * height as f32).max(1.0);

    for dy in 0..dst_h {
        let sy = (y0 + (dy as f32 / dst_h as f32) * bh).clamp(0.0, (h - 1) as f32) as usize;
        for dx in 0..dst_w {
            let sx = (x0 + (dx as f32 / dst_w as f32) * bw).clamp(0.0, (w - 1) as f32) as usize;
            let si = (sy * w + sx) * 4;
            let di = dy * dst_w + dx;
            out[di] = (bgra[si + 2] as f32 - mean[0]) / std[0];
            out[plane + di] = (bgra[si + 1] as f32 - mean[1]) / std[1];
            out[2 * plane + di] = (bgra[si] as f32 - mean[2]) / std[2];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_and_load_handle_missing_and_present_models_dir() {
        let dir = std::env::temp_dir().join(format!(
            "stagelock_vision_onnx_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Missing models => not ready
        let status = probe_vision_status(&dir);
        assert_eq!(status.models.len(), 3);
        assert!(!status.detector_ready);
        assert!(!status.pose_ready);
        assert!(!status.reid_ready);
        assert!(!status.default_models_dir_is_empty_check_helper(&status));

        // Load from empty directory succeeds gracefully (fallback mode)
        let mut engine = OnnxVisionEngine::load_from_dir(&dir);
        assert!(!engine.has_neural_detector());
        let frame = vec![128u8; 64 * 64 * 4];
        let dets = engine.detect_persons(&frame, 64, 64);
        assert!(dets.is_empty());
        let bbox = NormalizedRect {
            x: 0.2,
            y: 0.2,
            width: 0.4,
            height: 0.5,
        };
        assert!(engine.estimate_pose(&frame, 64, 64, &bbox).is_empty());
        assert!(engine.extract_reid_embedding(&frame, 64, 64, &bbox).is_none());

        // Create dummy non-empty files for all 3 models (tests file detection & invalid ONNX error path)
        for model in &status.models {
            std::fs::write(dir.join(&model.filename), b"not-a-valid-onnx-model").unwrap();
        }
        let status_present = probe_vision_status(&dir);
        assert!(status_present
            .models
            .iter()
            .all(|m| m.installed && m.size_bytes.unwrap_or(0) > 0));
        let engine_invalid = OnnxVisionEngine::load_from_dir(&dir);
        assert!(!engine_invalid.has_neural_detector());

        let def_dir = default_models_dir();
        assert!(!def_dir.as_os_str().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    trait VisionStatusExt {
        fn default_models_dir_is_empty_check_helper(&self, s: &VisionEngineStatus) -> bool {
            s.models_dir.is_empty()
        }
    }
    impl VisionStatusExt for VisionEngineStatus {}
}
