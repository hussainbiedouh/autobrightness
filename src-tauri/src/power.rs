use crate::state::PowerInfo;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

pub fn read_power() -> PowerInfo {
    unsafe {
        let mut status = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut status).is_err() {
            return PowerInfo {
                on_ac: None,
                battery_percent: None,
                battery_flag: None,
                valid: false,
                should_pause: false,
                status: "GetSystemPowerStatus failed".into(),
            };
        }
        let ac = status.ACLineStatus;
        let pct = status.BatteryLifePercent;
        let flag = status.BatteryFlag;
        let on_ac = match ac {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let pct_opt = if pct <= 100 { Some(pct) } else { None };
        let valid = on_ac == Some(false)
            && pct_opt.map(|p| p >= 1).unwrap_or(false)
            && flag != 128
            && flag != 255;
        PowerInfo {
            on_ac,
            battery_percent: pct_opt,
            battery_flag: Some(flag),
            valid,
            should_pause: false, // computed by caller with threshold/settings
            status: if valid {
                "ok".into()
            } else {
                format!("ac={ac} pct={pct} flag={flag} (invalid for pause logic)")
            },
        }
    }
}

pub fn should_pause(
    on_ac: Option<bool>,
    battery_percent: Option<u8>,
    battery_flag: Option<u8>,
    enabled: bool,
    threshold: u8,
) -> bool {
    if !enabled {
        return false;
    }
    let ac_ok = on_ac == Some(false);
    let pct_ok = matches!(battery_percent, Some(p) if (1..=100).contains(&p) && p <= threshold);
    let flag = battery_flag.unwrap_or(255);
    let flag_ok = flag != 128 && flag != 255;
    ac_ok && pct_ok && flag_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_truth_table() {
        // AC on/off x percent 0/24/25/100 x flags 128/255/6
        let flags = [128u8, 255u8, 6u8];
        let pcts = [0u8, 24u8, 25u8, 100u8];
        let acs = [Some(true), Some(false)];
        for ac in acs {
            for pct in pcts {
                for flag in flags {
                    let res = should_pause(ac, Some(pct), Some(flag), true, 25);
                    let expected = ac == Some(false)
                        && (1..=100).contains(&pct)
                        && pct <= 25
                        && flag != 128
                        && flag != 255;
                    assert_eq!(
                        res, expected,
                        "ac={ac:?} pct={pct} flag={flag} expected={expected} got={res}"
                    );
                }
            }
        }
        // disabled never pauses
        assert!(!should_pause(Some(false), Some(10), Some(6), false, 25));
        // bogus/none percent
        assert!(!should_pause(Some(false), None, Some(6), true, 25));
        assert!(!should_pause(Some(false), Some(0), Some(6), true, 25));
    }
}
