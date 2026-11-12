use crate::state::DisplayInfo;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use wmi::Variant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Wmi,
    Ddc,
    Unsupported,
}

#[derive(Debug, Clone)]
struct DdcCacheEntry {
    supported: bool,
    brightness: Option<u32>,
    max_brightness: Option<u32>,
    status: String,
}

static DDC_CACHE: OnceLock<Mutex<HashMap<String, DdcCacheEntry>>> = OnceLock::new();
static DDC_LOGGED: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn ddc_cache() -> &'static Mutex<HashMap<String, DdcCacheEntry>> {
    DDC_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ddc_logged() -> &'static Mutex<HashSet<String>> {
    DDC_LOGGED.get_or_init(|| Mutex::new(HashSet::new()))
}

pub fn enumerate_displays() -> Result<Vec<DisplayInfo>, String> {
    let mut result = Vec::new();

    match read_wmi_displays() {
        Ok(mut v) => result.append(&mut v),
        Err(e) => {
            eprintln!("WMI enumerate error: {e}");
        }
    }

    match read_ddc_displays() {
        Ok(mut v) => result.append(&mut v),
        Err(e) => {
            eprintln!("DDC enumerate error: {e}");
        }
    }

    if result.is_empty() {
        Err("No displays detected via WMI or DDC".into())
    } else {
        Ok(result)
    }
}

fn read_wmi_displays() -> Result<Vec<DisplayInfo>, String> {
    use wmi::WMIConnection;
    let wmi_con = WMIConnection::with_namespace_path("root\\wmi")
        .map_err(|e| format!("WMI root\\wmi: {e}"))?;

    let brightness: Vec<HashMap<String, Variant>> = wmi_con
        .raw_query("SELECT InstanceName, CurrentBrightness, Levels FROM WmiMonitorBrightness")
        .map_err(|e| format!("WmiMonitorBrightness query: {e}"))?;
    let methods: Vec<HashMap<String, Variant>> = wmi_con
        .raw_query("SELECT __PATH, InstanceName FROM WmiMonitorBrightnessMethods")
        .map_err(|e| format!("WmiMonitorBrightnessMethods query: {e}"))?;
    let monitor_ids = read_wmi_monitor_ids(&wmi_con);

    let mut out = Vec::new();
    for (i, b) in brightness.iter().enumerate() {
        let instance = variant_to_string(b.get("InstanceName")).unwrap_or_else(|| format!("wmi-{i}"));
        let current = variant_to_u32(b.get("CurrentBrightness"));
        let levels = variant_to_u32_vec(b.get("Levels")).or_else(|| variant_to_u32_vec(b.get("Level")));
        let max = levels.as_ref().and_then(|v| v.last().copied()).unwrap_or(100);
        let method_path = methods.get(i).and_then(|m| variant_to_string(m.get("__PATH")))
            .or_else(|| methods.iter().find_map(|m| {
                let mi = variant_to_string(m.get("InstanceName"))?;
                if mi == instance { variant_to_string(m.get("__PATH")) } else { None }
            }))
            .or_else(|| {
                let has = methods.iter().any(|m| variant_to_string(m.get("InstanceName")).as_deref() == Some(instance.as_str()));
                if has { Some(wmi_method_path(&instance)) } else { None }
            });
        let supported = method_path.is_some();
        let kind = if supported { BackendKind::Wmi } else { BackendKind::Unsupported };
        let friendly = friendly_wmi_name(&instance, &monitor_ids);
        let status = if supported {
            "Ready (WMI)".into()
        } else {
            format!("No WmiMonitorBrightnessMethods instance · ID: {}", instance)
        };
        out.push(DisplayInfo {
            id: instance.clone(),
            name: friendly,
            method: match kind {
                BackendKind::Wmi => "WMI".into(),
                BackendKind::Ddc => "DDC".into(),
                BackendKind::Unsupported => "Unsupported".into(),
            },
            supported,
            brightness: current,
            max_brightness: Some(max),
            trim: 0,
            status,
        });
    }
    Ok(out)
}

fn read_wmi_monitor_ids(wmi_con: &wmi::WMIConnection) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let rows: Vec<HashMap<String, Variant>> = match wmi_con.raw_query("SELECT InstanceName, UserFriendlyName, ManufacturerName, ProductCodeID FROM WmiMonitorID") {
        Ok(r) => r,
        Err(_) => return map,
    };
    for r in rows {
        let Some(instance) = variant_to_string(r.get("InstanceName")) else { continue };
        let user = variant_to_string_name_array(r.get("UserFriendlyName"));
        let manu = variant_to_string_name_array(r.get("ManufacturerName"));
        let prod = variant_to_string_name_array(r.get("ProductCodeID"));
        let mut label = None;
        if let Some(u) = user.filter(|s| !s.trim().is_empty()) {
            label = Some(u.trim().to_string());
        } else if manu.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false) || prod.as_ref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
            let m = manu.unwrap_or_default();
            let p = prod.unwrap_or_default();
            let combined = format!("{} {}", m.trim(), p.trim()).trim().to_string();
            if !combined.is_empty() { label = Some(combined); }
        }
        if let Some(l) = label {
            map.insert(instance, l);
        }
    }
    map
}

fn friendly_wmi_name(instance: &str, monitor_ids: &HashMap<String, String>) -> String {
    if let Some(name) = monitor_ids.get(instance) {
        if !name.trim().is_empty() {
            return format!("Built-in display ({})", name.trim());
        }
    }
    // Extract EDID panel code like BOE06A5 from DISPLAY\BOE06A5\...
    let code = instance.split('\\').nth(1).unwrap_or("");
    if code.len() >= 3 && code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return format!("Built-in display ({})", code);
    }
    "Built-in display".to_string()
}

fn read_ddc_displays() -> Result<Vec<DisplayInfo>, String> {
    use ddc_hi::Ddc;
    let mut displays = ddc_hi::Display::enumerate();
    let mut out = Vec::new();
    for d in displays.iter_mut() {
        let raw_id = d.info.id.clone();
        let id = format!("ddc:{}", raw_id);
        let name = friendly_ddc_name(&d.info);

        // Check cache first
        if let Some(entry) = ddc_cache().lock().ok().and_then(|c| c.get(&id).cloned()) {
            if !entry.supported {
                out.push(DisplayInfo {
                    id: id.clone(),
                    name,
                    method: "DDC".into(),
                    supported: false,
                    brightness: None,
                    max_brightness: None,
                    trim: 0,
                    status: entry.status.clone(),
                });
                continue;
            }
            match d.handle.get_vcp_feature(0x10) {
                Ok(v) => {
                    let entry_new = DdcCacheEntry {
                        supported: true,
                        brightness: Some(v.value() as u32),
                        max_brightness: Some(v.maximum() as u32),
                        status: "DDC ready".into(),
                    };
                    if let Ok(mut c) = ddc_cache().lock() { c.insert(id.clone(), entry_new.clone()); }
                    out.push(DisplayInfo {
                        id: id.clone(),
                        name,
                        method: "DDC".into(),
                        supported: true,
                        brightness: entry_new.brightness,
                        max_brightness: entry_new.max_brightness,
                        trim: 0,
                        status: "DDC ready".into(),
                    });
                }
                Err(e) => {
                    let status = format!("DDC read failed: {e} · ID: {raw_id}");
                    out.push(DisplayInfo {
                        id: id.clone(),
                        name,
                        method: "DDC".into(),
                        supported: false,
                        brightness: None,
                        max_brightness: None,
                        trim: 0,
                        status,
                    });
                }
            }
            continue;
        }

        // First probe this session
        match d.handle.get_vcp_feature(0x10) {
            Ok(v) => {
                let entry = DdcCacheEntry {
                    supported: true,
                    brightness: Some(v.value() as u32),
                    max_brightness: Some(v.maximum() as u32),
                    status: "DDC ready".into(),
                };
                if let Ok(mut c) = ddc_cache().lock() { c.insert(id.clone(), entry.clone()); }
                out.push(DisplayInfo {
                    id: id.clone(),
                    name,
                    method: "DDC".into(),
                    supported: true,
                    brightness: entry.brightness,
                    max_brightness: entry.max_brightness,
                    trim: 0,
                    status: "DDC ready".into(),
                });
            }
            Err(e) => {
                let raw = e.to_string();
                let hresult_hex = extract_hresult_hex(&raw);
                let status = if let Some(hx) = &hresult_hex {
                    format!("Unsupported (DDC/CI unavailable, HRESULT {hx}): {raw} · ID: {id}")
                } else {
                    format!("Unsupported (DDC/CI unavailable): {raw} · ID: {id}")
                };
                log_ddc_once(&id, &status);
                let entry = DdcCacheEntry {
                    supported: false,
                    brightness: None,
                    max_brightness: None,
                    status: status.clone(),
                };
                if let Ok(mut c) = ddc_cache().lock() { c.insert(id.clone(), entry); }
                out.push(DisplayInfo {
                    id: id.clone(),
                    name,
                    method: "DDC".into(),
                    supported: false,
                    brightness: None,
                    max_brightness: None,
                    trim: 0,
                    status,
                });
            }
        }
    }
    Ok(out)
}

fn friendly_ddc_name(info: &ddc_hi::DisplayInfo) -> String {
    let model = info.model_name.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty() && *s != "Generic PnP Monitor");
    let manu = info.manufacturer_id.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty());
    match (model, manu) {
        (Some(m), Some(man)) => {
            if m.to_lowercase().contains(&man.to_lowercase()) { m.to_string() } else { format!("{man} {m}") }
        }
        (Some(m), None) => m.to_string(),
        (None, Some(man)) => format!("{man} display"),
        (None, None) => "External display".to_string(),
    }
}

fn log_ddc_once(id: &str, status: &str) {
    if let Ok(mut logged) = ddc_logged().lock() {
        if logged.insert(id.to_string()) {
            eprintln!("DDC unsupported for {id}: {status}");
        }
    }
}

fn extract_hresult_hex(err: &str) -> Option<String> {
    let marker = "os error ";
    let idx = err.find(marker)?;
    let rest = &err[idx + marker.len()..];
    let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
    if num_str.is_empty() { return None; }
    let val: i64 = num_str.parse().ok()?;
    let u = val as u32;
    Some(format!("0x{u:08X}"))
}

pub fn get_brightness(display_id: &str) -> Result<u32, String> {
    if display_id.starts_with("ddc:") {
        get_ddc_brightness(display_id)
    } else {
        get_wmi_brightness(display_id)
    }
}

pub fn set_brightness(display_id: &str, percent: u32) -> Result<u32, String> {
    let pct = percent.clamp(0, 100);
    if display_id.starts_with("ddc:") {
        set_ddc_brightness(display_id, pct)
    } else {
        set_wmi_brightness(display_id, pct as u8).map(|_| pct)
    }
}

fn get_wmi_brightness(instance_name: &str) -> Result<u32, String> {
    use wmi::WMIConnection;
    let wmi_con = WMIConnection::with_namespace_path("root\\wmi")
        .map_err(|e| format!("WMI root\\wmi: {e}"))?;
    let rows: Vec<HashMap<String, Variant>> = wmi_con
        .raw_query("SELECT InstanceName, CurrentBrightness FROM WmiMonitorBrightness")
        .map_err(|e| format!("WmiMonitorBrightness query: {e}"))?;
    for r in rows {
        let name = variant_to_string(r.get("InstanceName")).unwrap_or_default();
        if name == instance_name || instance_name.is_empty() {
            if let Some(v) = variant_to_u32(r.get("CurrentBrightness")) {
                return Ok(v);
            }
        }
    }
    Err(format!("WMI brightness not found for {instance_name}"))
}

fn set_wmi_brightness(instance_name: &str, percent: u8) -> Result<(), String> {
    use wmi::WMIConnection;
    let wmi_con = WMIConnection::with_namespace_path("root\\wmi")
        .map_err(|e| format!("WMI root\\wmi: {e}"))?;

    let methods: Vec<HashMap<String, Variant>> = wmi_con
        .raw_query("SELECT __PATH, InstanceName FROM WmiMonitorBrightnessMethods")
        .map_err(|e| format!("WmiMonitorBrightnessMethods query: {e}"))?;
    let path = methods.iter().find_map(|m| {
        let name = variant_to_string(m.get("InstanceName"))?;
        if instance_name.is_empty() || name == instance_name {
            variant_to_string(m.get("__PATH")).or_else(|| Some(wmi_method_path(&name)))
        } else {
            None
        }
    }).or_else(|| {
        if !instance_name.is_empty() && methods.iter().any(|m| variant_to_string(m.get("InstanceName")).as_deref() == Some(instance_name)) {
            Some(wmi_method_path(instance_name))
        } else if instance_name.is_empty() {
            methods.first().and_then(|m| variant_to_string(m.get("InstanceName"))).map(|n| wmi_method_path(&n))
        } else {
            None
        }
    }).ok_or_else(|| format!("No WmiMonitorBrightnessMethods for {instance_name}"))?;

    let class = wmi_con
        .get_object("WmiMonitorBrightnessMethods")
        .map_err(|e| format!("get WmiMonitorBrightnessMethods class: {e}"))?;
    let in_params_def = class
        .get_method("WmiSetBrightness")
        .map_err(|e| format!("get_method WmiSetBrightness: {e}"))?;
    let in_params = in_params_def
        .ok_or_else(|| "WmiSetBrightness has no in-params definition".to_string())?
        .spawn_instance()
        .map_err(|e| format!("spawn in-params: {e}"))?;
    in_params
        .put_property("Timeout", Variant::UI4(0))
        .map_err(|e| format!("put Timeout: {e}"))?;
    in_params
        .put_property("Brightness", Variant::UI1(percent))
        .map_err(|e| format!("put Brightness: {e}"))?;

    let _out = wmi_con
        .exec_method(&path, "WmiSetBrightness", Some(&in_params))
        .map_err(|e| format!("exec WmiSetBrightness: {e}"))?;
    Ok(())
}

fn get_ddc_brightness(display_id: &str) -> Result<u32, String> {
    if let Some(entry) = ddc_cache().lock().ok().and_then(|c| c.get(display_id).cloned()) {
        if !entry.supported {
            return Err(entry.status);
        }
    }
    use ddc_hi::Ddc;
    let wanted = display_id.strip_prefix("ddc:").unwrap_or(display_id);
    let mut displays = ddc_hi::Display::enumerate();
    for d in displays.iter_mut() {
        if d.info.id == wanted {
            let v = d.handle.get_vcp_feature(0x10).map_err(|e| format!("DDC get: {e}"))?;
            return Ok(v.value() as u32);
        }
    }
    Err(format!("DDC display not found: {wanted}"))
}

fn set_ddc_brightness(display_id: &str, percent: u32) -> Result<u32, String> {
    if let Some(entry) = ddc_cache().lock().ok().and_then(|c| c.get(display_id).cloned()) {
        if !entry.supported {
            return Err(entry.status);
        }
    }
    use ddc_hi::Ddc;
    let wanted = display_id.strip_prefix("ddc:").unwrap_or(display_id);
    let mut displays = ddc_hi::Display::enumerate();
    for d in displays.iter_mut() {
        if d.info.id == wanted {
            let current = d.handle.get_vcp_feature(0x10).map_err(|e| format!("DDC get before set: {e}"))?;
            let max = current.maximum() as u32;
            let value = if max == 0 { percent.min(100) as u16 } else { ((percent.min(100) * max) / 100) as u16 };
            d.handle.set_vcp_feature(0x10, value).map_err(|e| format!("DDC set: {e}"))?;
            let readback = d.handle.get_vcp_feature(0x10).map_err(|e| format!("DDC readback: {e}"))?;
            return Ok(readback.value() as u32);
        }
    }
    Err(format!("DDC display not found: {wanted}"))
}

fn wmi_method_path(instance_name: &str) -> String {
    format!("WmiMonitorBrightnessMethods.InstanceName=\"{}\"", instance_name.replace('\\', "\\\\"))
}

fn variant_to_string(v: Option<&Variant>) -> Option<String> {
    match v? {
        Variant::String(s) => Some(s.clone()),
        other => Some(format!("{other:?}")),
    }
}

fn variant_to_u32(v: Option<&Variant>) -> Option<u32> {
    match v? {
        Variant::UI1(n) => Some(*n as u32),
        Variant::UI2(n) => Some(*n as u32),
        Variant::UI4(n) => Some(*n),
        Variant::UI8(n) => Some(*n as u32),
        Variant::I1(n) => Some(*n as u32),
        Variant::I2(n) => Some(*n as u32),
        Variant::I4(n) => Some(*n as u32),
        Variant::I8(n) => Some(*n as u32),
        _ => None,
    }
}

fn variant_to_u32_vec(v: Option<&Variant>) -> Option<Vec<u32>> {
    match v? {
        Variant::Array(arr) => {
            let mut out = Vec::new();
            for item in arr {
                match item {
                    Variant::UI1(n) => out.push(*n as u32),
                    Variant::UI2(n) => out.push(*n as u32),
                    Variant::UI4(n) => out.push(*n),
                    Variant::I4(n) => out.push(*n as u32),
                    _ => {}
                }
            }
            Some(out)
        }
        _ => None,
    }
}

fn variant_to_string_name_array(v: Option<&Variant>) -> Option<String> {
    let arr = match v? {
        Variant::Array(a) => a,
        Variant::String(s) => return Some(s.clone()),
        _ => return None,
    };
    let mut chars: Vec<u16> = Vec::new();
    for item in arr {
        match item {
            Variant::UI2(n) => chars.push(*n),
            Variant::I2(n) => chars.push(*n as u16),
            Variant::UI1(n) => chars.push(*n as u16),
            Variant::I4(n) => chars.push(*n as u16),
            Variant::UI4(n) => chars.push(*n as u16),
            _ => {}
        }
    }
    if chars.is_empty() { return None; }
    // Trim trailing nulls/zeros
    while chars.last().copied() == Some(0) { chars.pop(); }
    let s = String::from_utf16_lossy(&chars);
    let trimmed = s.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}
