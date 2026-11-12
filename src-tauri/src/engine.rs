use crate::brightness;
use crate::camera;
use crate::power;
use crate::settings;
use crate::state::{AmbientInfo, DisplayInfo, EngineState, Settings, WorkerCommand};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::Emitter;

pub struct EngineHandle {
    pub tx: Sender<WorkerCommand>,
}

pub fn spawn_engine(app: tauri::AppHandle, initial: Settings, shared: Arc<Mutex<EngineState>>) -> EngineHandle {
    let (tx, rx) = mpsc::channel::<WorkerCommand>();
    thread::spawn(move || {
        worker_loop(app, initial, shared, rx);
    });
    EngineHandle { tx }
}

pub fn initial_state(settings: &Settings) -> EngineState {
    EngineState {
        paused: settings.paused,
        ambient: AmbientInfo {
            mode: "unavailable".into(),
            status: "Waiting for first sample".into(),
            index: f32::NAN,
            ..Default::default()
        },
        settings: settings.clone(),
        ..Default::default()
    }
}

pub fn compute_target(
    _current_avg: f32,
    ambient: f32,
    baseline_brightness: f32,
    baseline_ambient: f32,
    sensitivity: f32,
) -> f32 {
    let b0 = baseline_brightness.max(1.0);
    let a0 = baseline_ambient.max(1.0);
    let a = ambient.max(1.0);
    let target = b0 * (a / a0).powf(sensitivity.clamp(0.1, 2.0));
    target.clamp(10.0, 100.0)
}

pub fn ema(prev: Option<f32>, sample: f32, alpha: f32) -> f32 {
    match prev {
        Some(p) => p + alpha.clamp(0.0, 1.0) * (sample - p),
        None => sample,
    }
}

pub fn apply_deadband_and_step(current: f32, desired: f32, deadband_pct: f32, max_step_pct: f32) -> f32 {
    let diff = desired - current;
    if diff.abs() < deadband_pct {
        return current;
    }
    let step = diff.clamp(-max_step_pct, max_step_pct);
    (current + step).clamp(0.0, 100.0)
}

pub fn apply_trim(value: f32, trim: i32) -> u32 {
    (value.round() as i32 + trim.clamp(-20, 20)).clamp(0, 100) as u32
}

fn average_controllable_brightness(displays: &[DisplayInfo]) -> Option<f32> {
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    for d in displays {
        if d.supported {
            if let Some(b) = d.brightness {
                sum += b as f32;
                n += 1.0;
            }
        }
    }
    if n > 0.0 { Some(sum / n) } else { None }
}

fn worker_loop(app: tauri::AppHandle, mut settings: Settings, shared: Arc<Mutex<EngineState>>, rx: Receiver<WorkerCommand>) {
    settings::sanitize_settings(&mut settings);
    let mut displays = brightness::enumerate_displays().unwrap_or_default();
    // Camera capability probe once at startup (best effort)
    let cam_probe = camera::probe_camera();
    let camera_manual_lock = cam_probe.manual_flag_verified;
    let _camera_coarse = cam_probe.coarse_mode;
    let mut camera_status = if cam_probe.opened {
        "Waiting for first sample".to_string()
    } else {
        cam_probe.error.clone().unwrap_or_else(|| "No camera".to_string())
    };
    let mut camera_backoff_until: Option<Instant> = None;
    let mut camera_backoff_secs: u64 = 5;
    let mut has_real_sample = false;

    let mut ambient_ema: Option<f32> = None;
    let mut current_applied: Option<f32> = average_controllable_brightness(&displays).or(Some(settings.baseline_brightness));
    let mut last_ambient = AmbientInfo {
        mode: if camera_manual_lock { "locked".into() } else if cam_probe.opened { "coarse".into() } else { "unavailable".into() },
        status: camera_status.clone(),
        index: f32::NAN,
        ..Default::default()
    };
    let mut last_power = power::read_power();
    let mut bogus_power_logged = false;
    let mut last_error: Option<String> = None;
    let mut last_target: Option<u32> = current_applied.map(|v| v.round().clamp(0.0, 100.0) as u32);

    let emit_snapshot = |app: &tauri::AppHandle,
                         shared: &Arc<Mutex<EngineState>>,
                         settings: &Settings,
                         automation_paused: bool,
                         target_brightness: Option<u32>,
                         ambient: &AmbientInfo,
                         displays: &Vec<DisplayInfo>,
                         power_info: &crate::state::PowerInfo,
                         last_error: &Option<String>| {
        let state = EngineState {
            paused: automation_paused,
            target_brightness,
            ambient: ambient.clone(),
            displays: displays.clone(),
            power: power_info.clone(),
            last_error: last_error.clone(),
            last_tick_ms: None,
            settings: settings.clone(),
        };
        if let Ok(mut s) = shared.lock() {
            *s = state.clone();
        }
        let _ = app.emit("state", &state);
    };

    let mut next_tick = Instant::now();

    loop {
        // Handle commands (non-blocking drain, but wait until next tick)
        let timeout = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(timeout) {
            Ok(cmd) => {
                match cmd {
                    WorkerCommand::Quit => break,
                    WorkerCommand::SetPaused(p) => {
                        settings.paused = p;
                        let _ = settings::save_settings_user_action(&settings);
                    }
                    WorkerCommand::SetSettings(mut s) => {
                        settings::sanitize_settings(&mut s);
                        settings = s;
                        let _ = settings::save_settings_user_action(&settings);
                    }
                    WorkerCommand::RefreshDisplays => {
                        displays = brightness::enumerate_displays().unwrap_or(displays);
                    }
                    WorkerCommand::Calibrate => {
                        displays = brightness::enumerate_displays().unwrap_or(displays);
                        let avg = average_controllable_brightness(&displays).unwrap_or(settings.baseline_brightness);
                        let amb = if has_real_sample {
                            ambient_ema.unwrap_or(last_ambient.index).max(1.0)
                        } else {
                            settings.baseline_ambient.max(1.0)
                        };
                        settings.baseline_brightness = avg.clamp(10.0, 100.0);
                        settings.baseline_ambient = amb.clamp(1.0, 100.0);
                        current_applied = Some(settings.baseline_brightness);
                        let _ = settings::save_settings_user_action(&settings);
                    }
                    WorkerCommand::ApplyNow => {
                        next_tick = Instant::now();
                        continue;
                    }
                }
                // Echo accepted settings/state immediately (no spurious tick side effects)
                let automation_paused = settings.paused || last_power.should_pause;
                for d in displays.iter_mut() {
                    d.trim = settings.trims.get(&d.id).copied().unwrap_or(0).clamp(-20, 20);
                }
                emit_snapshot(&app, &shared, &settings, automation_paused, last_target, &last_ambient, &displays, &last_power, &last_error);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // tick
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // Tick
        let interval = Duration::from_secs(settings.interval_sec.max(1));
        next_tick = Instant::now() + interval;

        // Refresh displays periodically? enumerate_displays is cheap enough and uses DDC cache
        displays = brightness::enumerate_displays().unwrap_or(displays);

        // Power
        let mut pinfo = power::read_power();
        let should_pause_power = power::should_pause(
            pinfo.on_ac,
            pinfo.battery_percent,
            pinfo.battery_flag,
            settings.battery_pause_enabled,
            settings.battery_pause_threshold,
        );
        pinfo.should_pause = should_pause_power;
        if !pinfo.valid && !bogus_power_logged && (pinfo.battery_percent.is_some() || pinfo.battery_flag.is_some()) {
            eprintln!("Power: invalid/bogus reading, battery-pause inert (status: {})", pinfo.status);
            bogus_power_logged = true;
        }
        last_power = pinfo.clone();

        // Ambient sample (if not paused and not power-paused)
        let automation_paused = settings.paused || should_pause_power;
        if !automation_paused {
            let now = Instant::now();
            let can_sample = camera_backoff_until.map(|t| now >= t).unwrap_or(true);
            if can_sample {
                match camera::sample_ambient(camera_manual_lock) {
                    Ok(mut sample) => {
                        if !camera_manual_lock {
                            sample.mode = "coarse".into();
                            if sample.status == "ok" || sample.status == "Exposure locked" {
                                sample.status = "Coarse mode - exposure not locked".into();
                            }
                        } else if sample.status == "ok" {
                            sample.status = "Exposure locked".into();
                        }
                        last_ambient = sample.clone();
                        ambient_ema = Some(ema(ambient_ema, sample.index, 0.25));
                        camera_status = sample.status.clone();
                        camera_backoff_until = None;
                        camera_backoff_secs = 5;
                        has_real_sample = true;
                    }
                    Err(e) => {
                        // Camera busy/unavailable: hold last target, surface status, retry with backoff
                        let msg = e.to_string();
                        camera_status = if msg.to_lowercase().contains("no camera") {
                            "No camera".to_string()
                        } else {
                            format!("Camera unavailable: {}", msg)
                        };
                        last_ambient.status = camera_status.clone();
                        if !has_real_sample {
                            last_ambient.index = f32::NAN;
                        }
                        if last_ambient.mode == "unavailable" {
                            last_ambient.mode = if camera_manual_lock { "locked".into() } else { "coarse".into() };
                        }
                        camera_backoff_until = Some(Instant::now() + Duration::from_secs(camera_backoff_secs));
                        camera_backoff_secs = (camera_backoff_secs * 2).min(60);
                    }
                }
            } else {
                // keep last ambient/status while backing off
                last_ambient.status = camera_status.clone();
            }
        } else {
            last_ambient.status = if settings.paused {
                "Paused".into()
            } else if should_pause_power {
                "Paused (battery)".into()
            } else {
                last_ambient.status.clone()
            };
        }

        // Compute and apply target
        let mut target_brightness: Option<u32> = last_target;
        if !automation_paused {
            if let Some(amb) = ambient_ema {
                let desired = compute_target(
                    current_applied.unwrap_or(settings.baseline_brightness),
                    amb,
                    settings.baseline_brightness,
                    settings.baseline_ambient,
                    settings.sensitivity,
                );
                let cur = current_applied.unwrap_or(desired);
                let new_applied = apply_deadband_and_step(cur, desired, 3.0, 12.0);
                current_applied = Some(new_applied);
                target_brightness = Some(new_applied.round().clamp(0.0, 100.0) as u32);

                // Apply to controllable displays only
                for d in displays.iter_mut() {
                    if !d.supported { continue; }
                    let trim = settings.trims.get(&d.id).copied().unwrap_or(0).clamp(-20, 20);
                    let pct = apply_trim(new_applied, trim);
                    match brightness::set_brightness(&d.id, pct) {
                        Ok(rb) => {
                            d.brightness = Some(rb);
                            let method_label = if d.method == "DDC" { "DDC" } else { "WMI" };
                            if rb != pct {
                                d.status = format!("{method_label} readback {rb}% (set {pct}%)");
                                last_error = Some(format!("{}: readback {} != set {}", d.id, rb, pct));
                            } else {
                                d.status = format!("{method_label} set verified");
                            }
                        }
                        Err(e) => {
                            d.status = format!("Set failed: {e}");
                            last_error = Some(format!("{}: {e}", d.id));
                        }
                    }
                }
            }
        } else {
            target_brightness = current_applied.map(|v| v.round().clamp(0.0, 100.0) as u32);
            // Keep display info fresh
            for d in displays.iter_mut() {
                if !d.supported { continue; }
                if let Ok(b) = brightness::get_brightness(&d.id) {
                    d.brightness = Some(b);
                }
                // Keep status meaningful
                if d.status == "ok" || d.status.is_empty() {
                    let method_label = if d.method == "DDC" { "DDC" } else { "WMI" };
                    d.status = format!("{method_label} ready");
                }
            }
        }
        last_target = target_brightness;

        // Ensure trims reflected in display info
        for d in displays.iter_mut() {
            d.trim = settings.trims.get(&d.id).copied().unwrap_or(0).clamp(-20, 20);
            if d.supported && d.status == "ok" {
                let method_label = if d.method == "DDC" { "DDC" } else { "WMI" };
                d.status = format!("{method_label} ready");
            }
        }

        emit_snapshot(&app, &shared, &settings, automation_paused, target_brightness, &last_ambient, &displays, &pinfo, &last_error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_target_clamps() {
        let t = compute_target(50.0, 0.0, 60.0, 50.0, 0.45);
        assert!(t >= 10.0 && t <= 100.0);
        let t2 = compute_target(50.0, 1000.0, 60.0, 10.0, 0.45);
        assert!(t2 >= 10.0 && t2 <= 100.0);
    }

    #[test]
    fn ema_works() {
        assert!((ema(None, 40.0, 0.25) - 40.0).abs() < 1e-6);
        let v = ema(Some(40.0), 80.0, 0.25);
        assert!((v - 50.0).abs() < 1e-6);
    }

    #[test]
    fn deadband_and_step() {
        assert!((apply_deadband_and_step(50.0, 52.0, 3.0, 12.0) - 50.0).abs() < 1e-6);
        assert!((apply_deadband_and_step(50.0, 70.0, 3.0, 12.0) - 62.0).abs() < 1e-6);
        assert!((apply_deadband_and_step(50.0, 20.0, 3.0, 12.0) - 38.0).abs() < 1e-6);
    }

    #[test]
    fn trim_clamps() {
        assert_eq!(apply_trim(50.0, 10), 60);
        assert_eq!(apply_trim(95.0, 20), 100);
        assert_eq!(apply_trim(5.0, -20), 0);
        assert_eq!(apply_trim(50.0, 30), 70); // trim input clamped to 20
    }
}
