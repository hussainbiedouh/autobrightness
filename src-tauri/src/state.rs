use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    pub method: String, // "WMI" | "DDC" | "Unsupported"
    pub supported: bool,
    pub brightness: Option<u32>,
    pub max_brightness: Option<u32>,
    pub trim: i32,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AmbientInfo {
    pub index: f32,
    pub mean_luma: f32,
    pub p05: f32,
    pub p95: f32,
    pub overexposed_ratio: f32,
    pub underexposed_ratio: f32,
    pub mode: String, // "locked" | "coarse" | "unavailable"
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PowerInfo {
    pub on_ac: Option<bool>,
    pub battery_percent: Option<u8>,
    pub battery_flag: Option<u8>,
    pub valid: bool,
    pub should_pause: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EngineState {
    pub paused: bool,
    pub target_brightness: Option<u32>,
    pub ambient: AmbientInfo,
    pub displays: Vec<DisplayInfo>,
    pub power: PowerInfo,
    pub last_error: Option<String>,
    pub last_tick_ms: Option<u64>,
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub interval_sec: u64,
    pub sensitivity: f32,
    pub trims: HashMap<String, i32>,
    pub autostart: bool,
    pub battery_pause_enabled: bool,
    pub battery_pause_threshold: u8,
    pub baseline_brightness: f32,
    pub baseline_ambient: f32,
    pub paused: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            interval_sec: 5,
            sensitivity: 0.45,
            trims: HashMap::new(),
            autostart: false,
            battery_pause_enabled: true,
            battery_pause_threshold: 25,
            baseline_brightness: 60.0,
            baseline_ambient: 50.0,
            paused: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkerCommand {
    SetPaused(bool),
    Calibrate,
    SetSettings(Settings),
    RefreshDisplays,
    ApplyNow,
    Quit,
}
