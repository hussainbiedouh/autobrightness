use auto_brightness_lib::brightness;
use auto_brightness_lib::camera;
use auto_brightness_lib::power;
use std::thread;
use std::time::Duration;

fn main() {
    println!("=== AutoBrightness Hardware Probe ===");

    // Power flags
    println!("\n--- POWER ---");
    let p = power::read_power();
    println!("on_ac: {:?}", p.on_ac);
    println!("battery_percent: {:?}", p.battery_percent);
    println!("battery_flag: {:?}", p.battery_flag);
    println!("valid_for_pause: {}", p.valid);
    println!("status: {}", p.status);

    // Displays enumerate
    println!("\n--- DISPLAYS ENUMERATE ---");
    match brightness::enumerate_displays() {
        Ok(displays) => {
            if displays.is_empty() {
                println!("No displays found");
            }
            for d in &displays {
                println!("Display: id={} name={} method={} supported={} brightness={:?} max={:?} status={}",
                    d.id, d.name, d.method, d.supported, d.brightness, d.max_brightness, d.status);
            }

            // DDC-specific tests per display
            for d in displays.iter().filter(|d| d.method == "DDC") {
                println!("\n--- DDC TEST: {} ---", d.id);
                match brightness::get_brightness(&d.id) {
                    Ok(v) => println!("Read brightness: {v}"),
                    Err(e) => println!("Read brightness failed: {e}"),
                }
                if d.supported {
                    if let Some(cur) = d.brightness {
                        let test_val = if cur >= 50 { cur.saturating_sub(10).max(10) } else { (cur + 10).min(100) };
                        println!("Test set to {test_val}...");
                        match brightness::set_brightness(&d.id, test_val) {
                            Ok(rb) => println!("Set ok, readback: {rb}"),
                            Err(e) => println!("Set failed: {e}"),
                        }
                        thread::sleep(Duration::from_millis(800));
                        println!("Restoring to {cur}...");
                        match brightness::set_brightness(&d.id, cur) {
                            Ok(rb) => println!("Restore readback: {rb}"),
                            Err(e) => println!("Restore failed: {e}"),
                        }
                    } else {
                        println!("No current brightness to test set/readback safely");
                    }
                } else {
                    println!("DDC unsupported for this display; no set test");
                }
            }

            // WMI tests
            let wmi_displays: Vec<_> = displays.iter().filter(|d| d.method == "WMI").cloned().collect();
            for d in wmi_displays {
                println!("\n--- WMI TEST: {} ---", d.id);
                match brightness::get_brightness(&d.id) {
                    Ok(v) => println!("Read brightness: {v}"),
                    Err(e) => println!("Read brightness failed: {e}"),
                }
                if d.supported {
                    println!("Setting 100...");
                    match brightness::set_brightness(&d.id, 100) {
                        Ok(rb) => println!("Set 100 readback: {rb}"),
                        Err(e) => println!("Set 100 failed: {e}"),
                    }
                    thread::sleep(Duration::from_millis(1200));
                    println!("Setting 60...");
                    match brightness::set_brightness(&d.id, 60) {
                        Ok(rb) => println!("Set 60 readback: {rb}"),
                        Err(e) => println!("Set 60 failed: {e}"),
                    }
                    thread::sleep(Duration::from_millis(1200));
                    println!("Restoring 100...");
                    match brightness::set_brightness(&d.id, 100) {
                        Ok(rb) => println!("Restore 100 readback: {rb}"),
                        Err(e) => println!("Restore 100 failed: {e}"),
                    }
                } else {
                    println!("WMI method instance missing; cannot set");
                }
            }
        }
        Err(e) => {
            println!("Display enumeration failed: {e}");
        }
    }

    // Camera
    println!("\n--- CAMERA ---");
    let cam = camera::probe_camera();
    println!("device_name: {:?}", cam.device_name);
    println!("opened: {}", cam.opened);
    if let Some((min,max,step,def,caps)) = cam.exposure_range {
        println!("exposure GetRange: min={min} max={max} step={step} default={def} caps={caps}");
    } else {
        println!("exposure GetRange: unavailable");
    }
    println!("manual_set_ok: {}", cam.manual_set_ok);
    println!("manual_flag_verified: {}", cam.manual_flag_verified);
    println!("samples: {}", cam.samples.len());
    for (i,s) in cam.samples.iter().enumerate() {
        println!("sample[{i}]: mean_luma={:.2} p05={:.2} p95={:.2} over={:.4} under={:.4} index={:.2} mode={} status={}",
            s.mean_luma, s.p05, s.p95, s.overexposed_ratio, s.underexposed_ratio, s.index, s.mode, s.status);
    }
    if let Some(e) = &cam.error { println!("camera_error: {e}"); }
    println!("camera_log:");
    for line in &cam.log { println!("  - {line}"); }

    println!("\n=== PROBE COMPLETE ===");
}
