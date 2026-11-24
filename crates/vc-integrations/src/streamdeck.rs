//! Direct Elgato Stream Deck control surface abstraction (keys + Stream Deck + rotary dials).
//!
//! Explicitly toggleable via `StreamDeckConfig::direct_hid_enabled` so it NEVER steals the USB
//! HID interface when Bitfocus Companion / Buttons is managing connected Stream Deck hardware.

use serde::{Deserialize, Serialize};
use vc_core::{FramingPreset, RuleOfThirds, StreamDeckConfig};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDeckStatus {
    pub direct_hid_enabled: bool,
    pub brightness: u8,
    pub connected_devices: Vec<String>,
    pub mode_note: String,
    pub key_bindings: Vec<StreamDeckBindingInfo>,
    pub dial_bindings: Vec<StreamDeckBindingInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDeckBindingInfo {
    pub slot: String,
    pub label: String,
    pub action: StreamDeckAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamDeckAction {
    ToggleAutoTrack { camera_index: usize },
    SetFramingPreset { camera_index: usize, preset: FramingPreset },
    SetRuleOfThirds { camera_index: usize, thirds: RuleOfThirds },
    TriggerOnePushAf { camera_index: usize },
    RecallPreset { camera_index: usize, slot: u8 },
    ToggleNdiOut { camera_index: usize },
    DialZoomStep { camera_index: usize, delta: f32 },
    DialFocusStep { camera_index: usize, delta: f32 },
    DialExposureStep { camera_index: usize, delta: f32 },
    DialWhiteBalanceStep { camera_index: usize, delta: f32 },
    VmixCut { input: String },
    VmixFade { input: String, duration_ms: u32 },
}

#[derive(Clone, Default)]
pub struct StreamDeckController;

impl StreamDeckController {
    pub fn new() -> Self {
        Self
    }

    pub fn default_key_bindings() -> Vec<StreamDeckBindingInfo> {
        vec![
            StreamDeckBindingInfo {
                slot: "Key 1".into(),
                label: "Cam 1 Track".into(),
                action: StreamDeckAction::ToggleAutoTrack { camera_index: 0 },
            },
            StreamDeckBindingInfo {
                slot: "Key 2".into(),
                label: "Close-Up".into(),
                action: StreamDeckAction::SetFramingPreset {
                    camera_index: 0,
                    preset: FramingPreset::CloseUp,
                },
            },
            StreamDeckBindingInfo {
                slot: "Key 3".into(),
                label: "Waist-Up".into(),
                action: StreamDeckAction::SetFramingPreset {
                    camera_index: 0,
                    preset: FramingPreset::WaistUp,
                },
            },
            StreamDeckBindingInfo {
                slot: "Key 4".into(),
                label: "Full Height".into(),
                action: StreamDeckAction::SetFramingPreset {
                    camera_index: 0,
                    preset: FramingPreset::FullHeight,
                },
            },
            StreamDeckBindingInfo {
                slot: "Key 5".into(),
                label: "Left 1/3".into(),
                action: StreamDeckAction::SetRuleOfThirds {
                    camera_index: 0,
                    thirds: RuleOfThirds::LeftThird,
                },
            },
            StreamDeckBindingInfo {
                slot: "Key 6".into(),
                label: "Center".into(),
                action: StreamDeckAction::SetRuleOfThirds {
                    camera_index: 0,
                    thirds: RuleOfThirds::Center,
                },
            },
            StreamDeckBindingInfo {
                slot: "Key 7".into(),
                label: "Right 1/3".into(),
                action: StreamDeckAction::SetRuleOfThirds {
                    camera_index: 0,
                    thirds: RuleOfThirds::RightThird,
                },
            },
            StreamDeckBindingInfo {
                slot: "Key 8".into(),
                label: "1-Push AF".into(),
                action: StreamDeckAction::TriggerOnePushAf { camera_index: 0 },
            },
        ]
    }

    pub fn default_dial_bindings() -> Vec<StreamDeckBindingInfo> {
        vec![
            StreamDeckBindingInfo {
                slot: "Dial 1 (SD+)".into(),
                label: "Optical Zoom".into(),
                action: StreamDeckAction::DialZoomStep {
                    camera_index: 0,
                    delta: 0.2,
                },
            },
            StreamDeckBindingInfo {
                slot: "Dial 2 (SD+)".into(),
                label: "Manual Focus".into(),
                action: StreamDeckAction::DialFocusStep {
                    camera_index: 0,
                    delta: 0.25,
                },
            },
            StreamDeckBindingInfo {
                slot: "Dial 3 (SD+)".into(),
                label: "Exposure EV".into(),
                action: StreamDeckAction::DialExposureStep {
                    camera_index: 0,
                    delta: 0.1,
                },
            },
            StreamDeckBindingInfo {
                slot: "Dial 4 (SD+)".into(),
                label: "WB Temp".into(),
                action: StreamDeckAction::DialWhiteBalanceStep {
                    camera_index: 0,
                    delta: 0.1,
                },
            },
        ]
    }

    pub fn status(&self, cfg: &StreamDeckConfig) -> StreamDeckStatus {
        let mode_note = if cfg.direct_hid_enabled {
            "Direct USB HID mode enabled — make sure Bitfocus Companion/Buttons is NOT claiming the Stream Deck USB device.".to_string()
        } else {
            "Direct USB HID paused so Bitfocus Companion / Buttons owns the Stream Deck cleanly via our companion module or OSC :9000.".to_string()
        };

        StreamDeckStatus {
            direct_hid_enabled: cfg.direct_hid_enabled,
            brightness: cfg.brightness,
            connected_devices: Vec::new(),
            mode_note,
            key_bindings: Self::default_key_bindings(),
            dial_bindings: Self::default_dial_bindings(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_deck_status_reflects_direct_hid_coexistence_modes_and_bindings() {
        let ctrl = StreamDeckController::new();
        let disabled = ctrl.status(&StreamDeckConfig {
            direct_hid_enabled: false,
            brightness: 80,
        });
        assert!(!disabled.direct_hid_enabled);
        assert!(disabled.mode_note.contains("paused"));
        assert_eq!(disabled.key_bindings.len(), 8);
        assert_eq!(disabled.dial_bindings.len(), 4);

        let enabled = ctrl.status(&StreamDeckConfig {
            direct_hid_enabled: true,
            brightness: 55,
        });
        assert!(enabled.direct_hid_enabled);
        assert_eq!(enabled.brightness, 55);
        assert!(enabled.mode_note.contains("Direct USB HID mode enabled"));

        // Verify JSON serialization roundtrip for key + dial + vMix actions
        let action = StreamDeckAction::VmixFade {
            input: "2".into(),
            duration_ms: 500,
        };
        let raw = serde_json::to_string(&action).unwrap();
        let back: StreamDeckAction = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, action);
    }
}
