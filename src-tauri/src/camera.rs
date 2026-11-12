use crate::state::AmbientInfo;

#[derive(Debug, Clone)]
pub struct CameraProbeResult {
    pub device_name: Option<String>,
    pub opened: bool,
    pub exposure_range: Option<(i32, i32, i32, i32, i32)>, // min,max,step,default,caps
    pub manual_set_ok: bool,
    pub manual_flag_verified: bool,
    pub coarse_mode: bool,
    pub samples: Vec<AmbientInfo>,
    pub error: Option<String>,
    pub log: Vec<String>,
}

pub fn probe_camera() -> CameraProbeResult {
    unsafe { probe_camera_impl() }
}

/// Sample ambient light. `manual_lock` indicates whether exposure is manually locked (capability verified).
/// When false, coarse mode is used (clipping-ratio-weighted index only).
pub fn sample_ambient(manual_lock: bool) -> Result<AmbientInfo, String> {
    unsafe { sample_ambient_impl(manual_lock) }
}

unsafe fn probe_camera_impl() -> CameraProbeResult {
    use windows::core::Interface;
    use windows::Win32::Media::DirectShow::{
        CameraControl_Exposure, CameraControl_Flags_Manual, IAMCameraControl,
    };
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::*;

    let mut log = Vec::new();
    let mut result = CameraProbeResult {
        device_name: None,
        opened: false,
        exposure_range: None,
        manual_set_ok: false,
        manual_flag_verified: false,
        coarse_mode: false,
        samples: Vec::new(),
        error: None,
        log: Vec::new(),
    };

    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) {
        result.error = Some(format!("MFStartup failed: {e}"));
        result.log = log;
        return result;
    }
    log.push("MFStartup ok".into());

    let mut attrs: Option<IMFAttributes> = None;
    if let Err(e) = MFCreateAttributes(&mut attrs, 1) {
        result.error = Some(format!("MFCreateAttributes: {e}"));
        log.push(result.error.clone().unwrap());
        result.log = log;
        let _ = MFShutdown();
        return result;
    }
    let attrs = attrs.unwrap();
    if let Err(e) = attrs.SetGUID(
        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
        &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
    ) {
        result.error = Some(format!("SetGUID source type: {e}"));
        log.push(result.error.clone().unwrap());
        result.log = log;
        let _ = MFShutdown();
        return result;
    }

    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count: u32 = 0;
    if let Err(e) = MFEnumDeviceSources(&attrs, &mut activates, &mut count) {
        result.error = Some(format!("MFEnumDeviceSources: {e}"));
        log.push(result.error.clone().unwrap());
        result.log = log;
        let _ = MFShutdown();
        return result;
    }
    log.push(format!("Video capture devices: {count}"));
    if count == 0 || activates.is_null() {
        result.error = Some("No camera".into());
        result.log = log;
        let _ = MFShutdown();
        return result;
    }

    let slice = std::slice::from_raw_parts(activates, count as usize);
    let activate = match &slice[0] {
        Some(a) => a.clone(),
        None => {
            result.error = Some("First IMFActivate is null".into());
            result.log = log;
            CoTaskMemFree(Some(activates as *const _));
            let _ = MFShutdown();
            return result;
        }
    };

    if let Ok(name) = get_activate_string(&activate, &MF_DEVSOURCE_ATTRIBUTE_FRIENDLY_NAME) {
        result.device_name = Some(name.clone());
        log.push(format!("Device: {name}"));
    }

    let source: IMFMediaSource = match activate.ActivateObject() {
        Ok(s) => s,
        Err(e) => {
            result.error = Some(format!("ActivateObject(IMFMediaSource): {e}"));
            log.push(result.error.clone().unwrap());
            result.log = log;
            CoTaskMemFree(Some(activates as *const _));
            let _ = MFShutdown();
            return result;
        }
    };
    result.opened = true;
    log.push("IMFMediaSource activated".into());

    // Try exposure control
    let mut cam_control: Option<IAMCameraControl> = None;
    if let Ok(cc) = source.cast::<IAMCameraControl>() {
        cam_control = Some(cc);
        log.push("IAMCameraControl via cast on source: ok".into());
    } else {
        log.push("IAMCameraControl via cast on source: failed".into());
        let services = [
            (MR_VIDEO_RENDER_SERVICE, "MR_VIDEO_RENDER_SERVICE"),
            (MR_VIDEO_ACCELERATION_SERVICE, "MR_VIDEO_ACCELERATION_SERVICE"),
            (MR_CAPTURE_POLICY_VOLUME_SERVICE, "MR_CAPTURE_POLICY_VOLUME_SERVICE"),
        ];
        for (svc, name) in services {
            let mut unk: Option<windows::core::IUnknown> = None;
            let hr = MFGetService(&source, &svc, &IAMCameraControl::IID, &mut unk as *mut _ as *mut _);
            if hr.is_ok() {
                if let Some(u) = unk {
                    if let Ok(cc) = u.cast::<IAMCameraControl>() {
                        cam_control = Some(cc);
                        log.push(format!("IAMCameraControl via MFGetService({name}): ok"));
                        break;
                    }
                }
            } else {
                log.push(format!("IAMCameraControl via MFGetService({name}): {hr:?}"));
            }
        }
    }

    if let Some(cc) = &cam_control {
        let mut min = 0i32;
        let mut max = 0i32;
        let mut step = 0i32;
        let mut def = 0i32;
        let mut caps = 0i32;
        let hr = cc.GetRange(CameraControl_Exposure.0, &mut min, &mut max, &mut step, &mut def, &mut caps);
        if hr.is_ok() {
            result.exposure_range = Some((min, max, step, def, caps));
            log.push(format!("Exposure range: min={min} max={max} step={step} def={def} caps={caps}"));
            let target = if def != 0 { def } else { (min + max) / 2 };
            let set_hr = cc.Set(CameraControl_Exposure.0, target, CameraControl_Flags_Manual.0);
            if set_hr.is_ok() {
                result.manual_set_ok = true;
                log.push(format!("Set exposure {target} manual: ok"));
                let mut val = 0i32;
                let mut flags = 0i32;
                if cc.Get(CameraControl_Exposure.0, &mut val, &mut flags).is_ok() {
                    log.push(format!("Get exposure: val={val} flags={flags}"));
                    result.manual_flag_verified = (flags & CameraControl_Flags_Manual.0) == CameraControl_Flags_Manual.0;
                    log.push(format!("Manual flag verified: {}", result.manual_flag_verified));
                } else {
                    log.push("Get exposure after set failed".into());
                }
            } else {
                log.push(format!("Set exposure manual failed: {set_hr:?}"));
            }
        } else {
            log.push(format!("GetRange exposure failed: {hr:?}"));
        }
    } else {
        log.push("No IAMCameraControl available; coarse mode will be required".into());
    }

    result.coarse_mode = !result.manual_flag_verified;
    if result.coarse_mode {
        log.push("Coarse mode enabled (manual exposure lock not verified)".into());
    }

    let manual_lock = result.manual_flag_verified;
    match capture_samples(&source, 2, manual_lock) {
        Ok(samples) => {
            for (i, s) in samples.iter().enumerate() {
                log.push(format!(
                    "Sample {i}: mean={:.2} p05={:.2} p95={:.2} over={:.3} under={:.3} idx={:.2} mode={}",
                    s.mean_luma, s.p05, s.p95, s.overexposed_ratio, s.underexposed_ratio, s.index, s.mode
                ));
            }
            result.samples = samples;
        }
        Err(e) => {
            log.push(format!("Frame capture failed: {e}"));
            if result.error.is_none() {
                result.error = Some(format!("No camera: {e}"));
            }
        }
    }

    let _ = source.Shutdown();
    drop(activate);
    CoTaskMemFree(Some(activates as *const _));
    let _ = MFShutdown();

    result.log = log;
    result
}

unsafe fn sample_ambient_impl(manual_lock: bool) -> Result<AmbientInfo, String> {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::*;

    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET).map_err(|e| format!("MFStartup: {e}"))?;

    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 1).map_err(|e| format!("MFCreateAttributes: {e}"))?;
    let attrs = attrs.ok_or("attrs null")?;
    attrs
        .SetGUID(&MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE, &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID)
        .map_err(|e| format!("SetGUID: {e}"))?;
    let mut activates: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFEnumDeviceSources(&attrs, &mut activates, &mut count).map_err(|e| format!("MFEnumDeviceSources: {e}"))?;
    if count == 0 || activates.is_null() {
        let _ = MFShutdown();
        return Err("No camera".into());
    }
    let slice = std::slice::from_raw_parts(activates, count as usize);
    let activate = slice[0].clone().ok_or("no activate")?;
    let source: IMFMediaSource = activate.ActivateObject().map_err(|e| format!("ActivateObject: {e}"))?;

    // Best effort: if manual lock requested, try to set it (runtime sampling path)
    if manual_lock {
        let _ = try_lock_exposure(&source);
    }

    let samples = capture_samples(&source, 1, manual_lock)?;
    let _ = source.Shutdown();
    drop(activate);
    CoTaskMemFree(Some(activates as *const _));
    let _ = MFShutdown();
    samples.into_iter().next().ok_or_else(|| "no sample".into())
}

unsafe fn try_lock_exposure(source: &windows::Win32::Media::MediaFoundation::IMFMediaSource) -> bool {
    use windows::core::Interface;
    use windows::Win32::Media::DirectShow::{CameraControl_Exposure, CameraControl_Flags_Manual, IAMCameraControl};
    use windows::Win32::Media::MediaFoundation::*;
    let mut cam_control: Option<IAMCameraControl> = source.cast::<IAMCameraControl>().ok();
    if cam_control.is_none() {
        let services = [MR_VIDEO_RENDER_SERVICE, MR_VIDEO_ACCELERATION_SERVICE, MR_CAPTURE_POLICY_VOLUME_SERVICE];
        for svc in services {
            let mut unk: Option<windows::core::IUnknown> = None;
            if MFGetService(source, &svc, &IAMCameraControl::IID, &mut unk as *mut _ as *mut _).is_ok() {
                if let Some(u) = unk {
                    if let Ok(cc) = u.cast::<IAMCameraControl>() {
                        cam_control = Some(cc);
                        break;
                    }
                }
            }
        }
    }
    if let Some(cc) = cam_control {
        let mut min = 0i32; let mut max = 0i32; let mut step = 0i32; let mut def = 0i32; let mut caps = 0i32;
        if cc.GetRange(CameraControl_Exposure.0, &mut min, &mut max, &mut step, &mut def, &mut caps).is_ok() {
            let target = if def != 0 { def } else { (min + max) / 2 };
            if cc.Set(CameraControl_Exposure.0, target, CameraControl_Flags_Manual.0).is_ok() {
                let mut val = 0i32; let mut flags = 0i32;
                if cc.Get(CameraControl_Exposure.0, &mut val, &mut flags).is_ok() {
                    return (flags & CameraControl_Flags_Manual.0) == CameraControl_Flags_Manual.0;
                }
            }
        }
    }
    false
}

unsafe fn capture_samples(
    source: &windows::Win32::Media::MediaFoundation::IMFMediaSource,
    n: usize,
    manual_lock: bool,
) -> Result<Vec<AmbientInfo>, String> {
    use windows::Win32::Media::MediaFoundation::*;

    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 2).map_err(|e| format!("MFCreateAttributes reader: {e}"))?;
    let attrs = attrs.ok_or("reader attrs null")?;
    let _ = attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1);

    let reader = MFCreateSourceReaderFromMediaSource(source, &attrs)
        .map_err(|e| format!("MFCreateSourceReaderFromMediaSource: {e}"))?;

    let out_type = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
    out_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| format!("Set major: {e}"))?;
    out_type.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| format!("Set subtype: {e}"))?;
    let frame_size: u64 = ((320u64) << 32) | (240u64);
    let _ = out_type.SetUINT64(&MF_MT_FRAME_SIZE, frame_size);
    let _ = reader.SetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32, None, &out_type);
    let _actual = reader
        .GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)
        .map_err(|e| format!("GetCurrentMediaType: {e}"))?;

    let mut out = Vec::new();
    for _ in 0..n.max(1) {
        let mut stream_index = 0u32;
        let mut flags = 0u32;
        let mut timestamp = 0i64;
        let mut sample: Option<IMFSample> = None;
        let mut got = false;
        for _attempt in 0..30 {
            reader
                .ReadSample(
                    MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32,
                    0,
                    Some(&mut stream_index),
                    Some(&mut flags),
                    Some(&mut timestamp),
                    Some(&mut sample),
                )
                .map_err(|e| format!("ReadSample: {e}"))?;
            if let Some(s) = &sample {
                let buffer = s.ConvertToContiguousBuffer().map_err(|e| format!("ConvertToContiguousBuffer: {e}"))?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur_len = 0u32;
                buffer
                    .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
                    .map_err(|e| format!("Lock: {e}"))?;
                let data = std::slice::from_raw_parts(ptr, cur_len as usize);
                let info = compute_ambient_from_rgb32(data, 320, 240, manual_lock);
                let _ = buffer.Unlock();
                out.push(info);
                got = true;
                break;
            }
            if (flags as i32 & MF_SOURCE_READERF_ENDOFSTREAM.0) != 0 {
                break;
            }
        }
        if !got {
            return Err("No sample read from camera".into());
        }
    }
    Ok(out)
}

fn compute_ambient_from_rgb32(data: &[u8], width: usize, height: usize, manual_lock: bool) -> AmbientInfo {
    let stride = width * 4;
    let mut lumas: Vec<u8> = Vec::with_capacity((width * height) / 4);
    let mut over = 0usize;
    let mut under = 0usize;
    let mut sum: f64 = 0.0;
    let mut y = 0usize;
    while y < height {
        let mut x = 0usize;
        while x < width {
            let idx = y * stride + x * 4;
            if idx + 2 < data.len() {
                let b = data[idx] as f32;
                let g = data[idx + 1] as f32;
                let r = data[idx + 2] as f32;
                let l = (0.2126 * r + 0.7152 * g + 0.0722 * b).round().clamp(0.0, 255.0) as u8;
                lumas.push(l);
                sum += l as f64;
                if l > 250 {
                    over += 1;
                }
                if l < 5 {
                    under += 1;
                }
            }
            x += 2;
        }
        y += 2;
    }
    if lumas.is_empty() {
        return AmbientInfo {
            index: 0.0,
            mean_luma: 0.0,
            p05: 0.0,
            p95: 0.0,
            overexposed_ratio: 0.0,
            underexposed_ratio: 0.0,
            mode: if manual_lock { "locked".into() } else { "coarse".into() },
            status: "no pixels".into(),
        };
    }
    lumas.sort_unstable();
    let n = lumas.len();
    let mean = (sum / n as f64) as f32;
    let p05 = lumas[(n - 1) * 5 / 100] as f32;
    let p95 = lumas[(n - 1) * 95 / 100] as f32;
    let over_r = over as f32 / n as f32;
    let under_r = under as f32 / n as f32;

    let (index, mode, status) = if manual_lock {
        // Perceptual remap: mid-light should sit ~35-55 instead of bottom-compressed range
        let norm = (mean / 255.0).clamp(0.0, 1.0);
        let mut idx = norm.powf(0.5) * 100.0;
        if over_r > 0.2 {
            idx = (idx + (over_r * 30.0)).min(100.0);
        }
        if under_r > 0.3 {
            idx = (idx - (under_r * 20.0)).max(0.0);
        }
        (idx, "locked".to_string(), "Exposure locked".to_string())
    } else {
        // Coarse mode: clipping-ratio-weighted index only (auto-exposure makes mean unreliable)
        let idx = (50.0 + over_r * 80.0 - under_r * 80.0).clamp(0.0, 100.0);
        (idx, "coarse".to_string(), "Coarse mode - exposure not locked".to_string())
    };

    AmbientInfo {
        index,
        mean_luma: mean,
        p05,
        p95,
        overexposed_ratio: over_r,
        underexposed_ratio: under_r,
        mode,
        status,
    }
}

unsafe fn get_activate_string(
    activate: &windows::Win32::Media::MediaFoundation::IMFActivate,
    key: &windows::core::GUID,
) -> windows::core::Result<String> {
    let len = activate.GetStringLength(key)?;
    let mut buf = vec![0u16; (len + 1) as usize];
    let mut out_len = 0u32;
    activate.GetString(key, &mut buf, Some(&mut out_len))?;
    if let Some(pos) = buf.iter().position(|&c| c == 0) {
        buf.truncate(pos);
    } else if (out_len as usize) < buf.len() {
        buf.truncate(out_len as usize);
    }
    Ok(String::from_utf16_lossy(&buf))
}
