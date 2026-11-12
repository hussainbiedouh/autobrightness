use crate::state::Settings;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static PERSIST_ALLOWED: AtomicBool = AtomicBool::new(true);

pub fn settings_path() -> Result<PathBuf, String> {
    let appdata = std::env::var("APPDATA").map_err(|e| format!("APPDATA missing: {e}"))?;
    let dir = PathBuf::from(appdata).join("AutoBrightness");
    fs::create_dir_all(&dir).map_err(|e| format!("create settings dir: {e}"))?;
    Ok(dir.join("settings.json"))
}

pub fn load_settings() -> Settings {
    load_settings_with_persist().0
}

/// Returns (settings, persist_allowed). Persist is disallowed when recovering from a bad/unreadable
/// existing settings file to avoid clobbering user data until an explicit user action occurs.
pub fn load_settings_with_persist() -> (Settings, bool) {
    let path = match settings_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Settings path unavailable: {e}");
            PERSIST_ALLOWED.store(false, Ordering::SeqCst);
            return (Settings::default(), false);
        }
    };

    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                PERSIST_ALLOWED.store(true, Ordering::SeqCst);
                return (Settings::default(), true);
            }
            eprintln!(
                "Settings read failed ({}): {e}. Keeping defaults in-memory; not overwriting until user changes settings.",
                path.display()
            );
            PERSIST_ALLOWED.store(false, Ordering::SeqCst);
            return (Settings::default(), false);
        }
    };

    let stripped = data.strip_prefix('\u{FEFF}').unwrap_or(&data);
    match serde_json::from_str::<Settings>(stripped) {
        Ok(mut s) => {
            sanitize_settings(&mut s);
            PERSIST_ALLOWED.store(true, Ordering::SeqCst);
            (s, true)
        }
        Err(e) => {
            let bak = path.with_extension("json.bak");
            match fs::rename(&path, &bak) {
                Ok(_) => eprintln!(
                    "Settings parse failed ({}): {e}. Backed up to {} and using defaults in-memory; not overwriting until user changes settings.",
                    path.display(),
                    bak.display()
                ),
                Err(re) => eprintln!(
                    "Settings parse failed ({}): {e}. Backup rename failed: {re}. Using defaults in-memory; not overwriting until user changes settings.",
                    path.display()
                ),
            }
            PERSIST_ALLOWED.store(false, Ordering::SeqCst);
            (Settings::default(), false)
        }
    }
}

pub fn sanitize_settings(s: &mut Settings) {
    let defaults = Settings::default();
    // Reject persisted 0/negative values with defaults (not min clamp); allow any positive value
    if s.interval_sec == 0 {
        s.interval_sec = defaults.interval_sec;
    } else {
        s.interval_sec = s.interval_sec.max(1);
    }
    if !s.sensitivity.is_finite() || s.sensitivity <= 0.0 {
        s.sensitivity = defaults.sensitivity;
    } else {
        s.sensitivity = s.sensitivity.clamp(0.1, 2.0);
    }
    if s.battery_pause_threshold == 0 || s.battery_pause_threshold > 100 {
        s.battery_pause_threshold = defaults.battery_pause_threshold;
    }
    for v in s.trims.values_mut() {
        *v = (*v).clamp(-20, 20);
    }
    if !s.baseline_brightness.is_finite() || s.baseline_brightness <= 0.0 {
        s.baseline_brightness = defaults.baseline_brightness;
    } else {
        s.baseline_brightness = s.baseline_brightness.clamp(10.0, 100.0);
    }
    if !s.baseline_ambient.is_finite() || s.baseline_ambient <= 0.0 {
        s.baseline_ambient = defaults.baseline_ambient;
    } else {
        s.baseline_ambient = s.baseline_ambient.clamp(1.0, 100.0);
    }
}

pub fn enable_persist() {
    PERSIST_ALLOWED.store(true, Ordering::SeqCst);
}

pub fn persist_allowed() -> bool {
    PERSIST_ALLOWED.load(Ordering::SeqCst)
}

pub fn save_settings(settings: &Settings) -> Result<(), String> {
    if !PERSIST_ALLOWED.load(Ordering::SeqCst) {
        return Ok(());
    }
    let path = settings_path()?;
    let data = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

/// Save as an explicit user action (enables persistence if it was disabled after parse recovery).
pub fn save_settings_user_action(settings: &Settings) -> Result<(), String> {
    PERSIST_ALLOWED.store(true, Ordering::SeqCst);
    let path = settings_path()?;
    let data = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, data).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let mut s = Settings::default();
        s.interval_sec = 7;
        s.sensitivity = 0.6;
        s.trims.insert("disp1".into(), -5);
        s.battery_pause_threshold = 30;
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.interval_sec, 7);
        assert!((back.sensitivity - 0.6).abs() < 1e-6);
        assert_eq!(back.trims.get("disp1").copied(), Some(-5));
        assert_eq!(back.battery_pause_threshold, 30);
    }
}
