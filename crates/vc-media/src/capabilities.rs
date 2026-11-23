use std::path::PathBuf;
use std::process::Command;

use vc_core::{HardwareAccelBackend, InferenceProvider, SystemCapabilities};

/// Probe local GPU adapters, GStreamer installation, hardware codec plugins, and NDI 6 runtime.
pub fn probe_system_capabilities() -> SystemCapabilities {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();

    let gpu_names = detect_gpu_names();
    let has_nvidia = gpu_names
        .iter()
        .any(|g| g.to_lowercase().contains("nvidia"));
    let has_amd = gpu_names
        .iter()
        .any(|g| g.to_lowercase().contains("amd") || g.to_lowercase().contains("radeon"));
    let has_intel = gpu_names
        .iter()
        .any(|g| g.to_lowercase().contains("intel"));

    let (gstreamer_bin, gstreamer_version) = detect_gstreamer();
    let gstreamer_available = gstreamer_bin.is_some();
    let (ndi_runtime_available, ndi_runtime_path) = detect_ndi_runtime();

    let mut available_decode_backends = Vec::new();
    let mut available_encode_backends = Vec::new();

    if os == "windows" {
        if has_nvidia && (!gstreamer_available || gst_element_exists(gstreamer_bin.as_deref(), "nvh264dec")) {
            available_decode_backends.push(HardwareAccelBackend::NvidiaNvcodec);
        }
        if has_nvidia && (!gstreamer_available || gst_element_exists(gstreamer_bin.as_deref(), "nvh264enc")) {
            available_encode_backends.push(HardwareAccelBackend::NvidiaNvcodec);
        }
        if has_intel && (!gstreamer_available || gst_element_exists(gstreamer_bin.as_deref(), "qsvh264dec")) {
            available_decode_backends.push(HardwareAccelBackend::IntelQuickSync);
        }
        if has_intel && (!gstreamer_available || gst_element_exists(gstreamer_bin.as_deref(), "qsvh264enc")) {
            available_encode_backends.push(HardwareAccelBackend::IntelQuickSync);
        }
        if has_amd {
            available_decode_backends.push(HardwareAccelBackend::WindowsD3d11);
            if !gstreamer_available || gst_element_exists(gstreamer_bin.as_deref(), "amfh264enc") {
                available_encode_backends.push(HardwareAccelBackend::AmdAmf);
            }
        }
        if !available_decode_backends.contains(&HardwareAccelBackend::WindowsD3d11) {
            available_decode_backends.push(HardwareAccelBackend::WindowsD3d11);
        }
    } else if os == "macos" {
        available_decode_backends.push(HardwareAccelBackend::AppleVideoToolbox);
        available_encode_backends.push(HardwareAccelBackend::AppleVideoToolbox);
    } else {
        if has_nvidia {
            available_decode_backends.push(HardwareAccelBackend::NvidiaNvcodec);
            available_encode_backends.push(HardwareAccelBackend::NvidiaNvcodec);
        }
        if has_intel || has_amd {
            available_decode_backends.push(HardwareAccelBackend::LinuxVaapi);
            available_encode_backends.push(HardwareAccelBackend::LinuxVaapi);
        }
    }

    available_decode_backends.push(HardwareAccelBackend::SoftwareCpu);
    available_encode_backends.push(HardwareAccelBackend::SoftwareCpu);

    let decode_backend = available_decode_backends
        .first()
        .cloned()
        .unwrap_or(HardwareAccelBackend::SoftwareCpu);
    let encode_backend = available_encode_backends
        .first()
        .cloned()
        .unwrap_or(HardwareAccelBackend::SoftwareCpu);

    let inference_provider = if os == "macos" {
        InferenceProvider::CoreMl
    } else if has_nvidia {
        InferenceProvider::Cuda
    } else if os == "windows" && (has_amd || has_intel) {
        InferenceProvider::DirectMl
    } else if has_intel {
        InferenceProvider::OpenVino
    } else if has_amd {
        InferenceProvider::Rocm
    } else {
        InferenceProvider::Cpu
    };

    SystemCapabilities {
        os,
        arch,
        gpu_names,
        decode_backend,
        encode_backend,
        available_decode_backends,
        available_encode_backends,
        inference_provider,
        gstreamer_available,
        gstreamer_version,
        ndi_runtime_available,
        ndi_runtime_path,
    }
}

fn detect_gpu_names() -> Vec<String> {
    let mut gpus = Vec::new();

    // Fast NVIDIA probe works across Windows & Linux
    if let Ok(output) = Command::new("nvidia-smi")
        .args(["--query-gpu=name", "--format=csv,noheader"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    gpus.push(trimmed.to_string());
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if gpus.is_empty() {
            gpus.push("Apple Silicon / Metal GPU".to_string());
        }
    }

    gpus
}

fn detect_gstreamer() -> (Option<String>, Option<String>) {
    let mut candidates: Vec<String> = vec!["gst-inspect-1.0".to_string()];
    if let Ok(root) = std::env::var("GSTREAMER_1_0_ROOT_MSVC_X86_64") {
        candidates.push(PathBuf::from(root).join("bin").join("gst-inspect-1.0.exe").to_string_lossy().to_string());
    }
    candidates.push(r"C:\Program Files\gstreamer\1.0\msvc_x86_64\bin\gst-inspect-1.0.exe".to_string());
    candidates.push(r"C:\gstreamer\1.0\msvc_x86_64\bin\gst-inspect-1.0.exe".to_string());

    for bin in candidates {
        if let Ok(output) = Command::new(&bin).arg("--version").output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                let version = text
                    .lines()
                    .find(|l| l.contains("GStreamer"))
                    .map(|s| s.trim().to_string())
                    .or_else(|| Some("GStreamer 1.x".to_string()));
                return (Some(bin), version);
            }
        }
    }

    (None, None)
}

fn gst_element_exists(gst_inspect_bin: Option<&str>, element: &str) -> bool {
    let Some(bin) = gst_inspect_bin else {
        return false;
    };
    Command::new(bin)
        .arg(element)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn detect_ndi_runtime() -> (bool, Option<String>) {
    let env_vars = [
        "NDI_RUNTIME_DIR_V6",
        "NDI_RUNTIME_DIR_V5",
        "NDI_RUNTIME_DIR_V4",
    ];

    #[cfg(target_os = "windows")]
    let lib_name = "Processing.NDI.Lib.x64.dll";
    #[cfg(target_os = "macos")]
    let lib_name = "libndi.dylib";
    #[cfg(target_os = "linux")]
    let lib_name = "libndi.so.6";

    let mut search_dirs: Vec<PathBuf> = Vec::new();
    for var in env_vars {
        if let Ok(val) = std::env::var(var) {
            search_dirs.push(PathBuf::from(val));
        }
    }

    // App-local runtime directory (complies with Vizrt DLL isolation guidance)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            search_dirs.push(dir.join("runtime"));
            search_dirs.push(dir.to_path_buf());
        }
    }

    #[cfg(target_os = "windows")]
    {
        search_dirs.push(PathBuf::from(r"C:\Program Files\NDI\NDI 6 Tools\Runtime"));
        search_dirs.push(PathBuf::from(r"C:\Program Files\NDI\NDI 6 Runtime\v6"));
        search_dirs.push(PathBuf::from(r"C:\Program Files\NDI\NDI 5 Runtime\v5"));
    }

    #[cfg(target_os = "macos")]
    {
        search_dirs.push(PathBuf::from("/usr/local/lib"));
        search_dirs.push(PathBuf::from("/Library/NDI SDK for Apple/lib/macOS"));
    }

    #[cfg(target_os = "linux")]
    {
        search_dirs.push(PathBuf::from("/usr/local/lib"));
        search_dirs.push(PathBuf::from("/usr/lib"));
    }

    for dir in search_dirs {
        let candidate = dir.join(lib_name);
        if candidate.exists() {
            return (true, Some(candidate.to_string_lossy().to_string()));
        }
    }

    (false, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_probe_always_includes_software_fallback() {
        let caps = probe_system_capabilities();
        assert!(!caps.os.is_empty());
        assert!(caps
            .available_decode_backends
            .contains(&HardwareAccelBackend::SoftwareCpu));
        assert!(caps
            .available_encode_backends
            .contains(&HardwareAccelBackend::SoftwareCpu));
    }
}
