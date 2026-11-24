//! Battery and power source. No thread of its own: Windows calls back on one
//! of its own threads when the power source, the charge level or battery
//! saver changes (that callback only reads and publishes), and the hub's
//! timer re-reads every minute as a liveness check.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::Mutex;
use std::time::Duration;

use rsb_core::services::{ServiceCommand, ServiceId};
use rsb_core::value::Value;
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE};
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, GetSystemPowerStatus, HPOWERNOTIFY, PowerSettingRegisterNotification,
    PowerSettingUnregisterNotification, SYSTEM_POWER_STATUS,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_POWER_SAVING_STATUS,
};
use windows::Win32::UI::WindowsAndMessaging::DEVICE_NOTIFY_CALLBACK;
use windows::core::GUID;

use crate::util::map;
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);

/// `SYSTEM_POWER_STATUS` as a service value.
pub(crate) fn read_battery() -> Option<Value> {
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: a live out pointer.
    unsafe { GetSystemPowerStatus(&mut s) }.ok()?;
    Some(battery_value(&s))
}

pub(crate) fn battery_value(s: &SYSTEM_POWER_STATUS) -> Value {
    // BatteryFlag: 128 = no battery, 255 = unknown, 8 = charging.
    let present = s.BatteryFlag != 255 && s.BatteryFlag & 128 == 0;
    let percent = (present && s.BatteryLifePercent <= 100).then(|| f64::from(s.BatteryLifePercent));
    let seconds = (present && s.BatteryLifeTime != u32::MAX).then(|| i64::from(s.BatteryLifeTime));
    map([
        ("present", Value::Bool(present)),
        ("percent", percent.map_or(Value::Nil, Value::Num)),
        ("charging", Value::Bool(present && s.BatteryFlag & 8 != 0)),
        ("plugged", Value::Bool(s.ACLineStatus == 1)),
        ("saver", Value::Bool(s.SystemStatusFlag & 1 != 0)),
        ("seconds_left", seconds.map_or(Value::Nil, Value::Int)),
    ])
}

/// What the callback reads through a raw pointer. The documentation does not
/// promise that unregistering waits for a callback already running, so this is
/// never freed (a few bytes per registration): `Drop` empties `out` instead,
/// and a late callback finds nothing to publish to.
struct CallbackContext {
    out: Mutex<Option<Publisher>>,
}

unsafe extern "system" fn on_power_setting(context: *const c_void, _kind: u32, _setting: *const c_void) -> u32 {
    // SAFETY: `context` is the pointer to the leaked CallbackContext we registered; it is never freed.
    let ctx = unsafe { &*context.cast::<CallbackContext>() };
    let out = ctx.out.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(out) = out.as_ref()
        && let Some(v) = read_battery()
    {
        out.publish(v);
    }
    0
}

struct Registration {
    handles: Vec<HPOWERNOTIFY>,
    context: &'static CallbackContext,
    /// Windows keeps the pointer, and a callback may still be running when
    /// the unregister returns: leaked on purpose, like the context.
    _params: ManuallyDrop<Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>>,
}

// SAFETY: the parameters hold raw pointers that only the callback dereferences, into data that is never freed.
unsafe impl Send for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        // A callback that starts or is running from here on finds nothing to publish to.
        *self.context.out.lock().unwrap_or_else(|e| e.into_inner()) = None;
        for h in self.handles.drain(..) {
            // SAFETY: a handle returned by PowerSettingRegisterNotification, unregistered once.
            let _ = unsafe { PowerSettingUnregisterNotification(h) };
        }
    }
}

fn register(out: Publisher) -> Result<Registration, String> {
    let context: &'static CallbackContext = Box::leak(Box::new(CallbackContext { out: Mutex::new(Some(out)) }));
    let params = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(on_power_setting),
        Context: std::ptr::from_ref(context).cast_mut().cast(),
    });
    let mut reg = Registration { handles: Vec::new(), context, _params: ManuallyDrop::new(params) };
    let recipient = HANDLE((&raw const **reg._params).cast_mut().cast());
    for guid in [GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_POWER_SAVING_STATUS] {
        let guid: GUID = guid;
        let mut handle: *mut c_void = std::ptr::null_mut();
        // SAFETY: the parameters struct and context outlive the registration (see Registration).
        let r = unsafe { PowerSettingRegisterNotification(&guid, DEVICE_NOTIFY_CALLBACK, recipient, &mut handle) };
        if r != ERROR_SUCCESS {
            return Err(format!("PowerSettingRegisterNotification failed ({})", r.0));
        }
        reg.handles.push(HPOWERNOTIFY(handle as isize));
    }
    Ok(reg)
}

pub(crate) struct BatteryService {
    out: Option<Publisher>,
    registration: Option<Registration>,
}

impl BatteryService {
    pub(crate) fn new() -> BatteryService {
        BatteryService { out: None, registration: None }
    }
}

impl Service for BatteryService {
    fn id(&self) -> ServiceId {
        ServiceId::Battery
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let v = read_battery().ok_or("GetSystemPowerStatus failed")?;
        out.publish(v);
        // A failed registration is not fatal: the minute timer still keeps the value fresh.
        match register(out.clone()) {
            Ok(r) => self.registration = Some(r),
            Err(e) => log::warn!("battery: no change notifications ({e}); re-reading every minute"),
        }
        self.out = Some(out);
        Ok(())
    }

    fn command(&mut self, _command: ServiceCommand) -> Result<(), String> {
        Err("the battery service has no commands".into())
    }

    fn stop(&mut self) {
        self.registration = None;
        self.out = None;
    }

    fn stale_after(&self) -> Duration {
        Duration::from_secs(180)
    }

    fn tick_after(&self) -> Option<Duration> {
        self.out.as_ref().map(|_| REREAD)
    }

    fn tick(&mut self) {
        if let (Some(out), Some(v)) = (&self.out, read_battery()) {
            out.publish(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use rsb_core::services::check_value;

    use super::*;

    fn status(flag: u8, ac: u8, percent: u8, life: u32, system: u8) -> SYSTEM_POWER_STATUS {
        SYSTEM_POWER_STATUS {
            ACLineStatus: ac,
            BatteryFlag: flag,
            BatteryLifePercent: percent,
            SystemStatusFlag: system,
            BatteryLifeTime: life,
            BatteryFullLifeTime: u32::MAX,
        }
    }

    #[test]
    fn a_pc_without_a_battery_says_so() {
        let v = battery_value(&status(128, 1, 255, u32::MAX, 0));
        assert_eq!(check_value(ServiceId::Battery, &v), Ok(()));
        assert_eq!(v.get("present"), Some(&Value::Bool(false)));
        assert_eq!(v.get("percent"), Some(&Value::Nil));
        assert_eq!(v.get("seconds_left"), Some(&Value::Nil));
        assert_eq!(v.get("plugged"), Some(&Value::Bool(true)));
        assert_eq!(v.get("charging"), Some(&Value::Bool(false)));
    }

    #[test]
    fn a_charging_battery_reports_its_level() {
        let v = battery_value(&status(8, 1, 55, u32::MAX, 0));
        assert_eq!(check_value(ServiceId::Battery, &v), Ok(()));
        assert_eq!(v.get("present"), Some(&Value::Bool(true)));
        assert_eq!(v.get("percent"), Some(&Value::Num(55.0)));
        assert_eq!(v.get("charging"), Some(&Value::Bool(true)));
        assert_eq!(v.get("seconds_left"), Some(&Value::Nil), "unknown time left stays nil");
    }

    #[test]
    fn a_discharging_battery_in_saver_mode_has_time_left() {
        let v = battery_value(&status(2, 0, 20, 3600, 1));
        assert_eq!(check_value(ServiceId::Battery, &v), Ok(()));
        assert_eq!(v.get("plugged"), Some(&Value::Bool(false)));
        assert_eq!(v.get("saver"), Some(&Value::Bool(true)));
        assert_eq!(v.get("seconds_left"), Some(&Value::Int(3600)));
    }

    #[test]
    fn an_unknown_status_is_not_a_battery() {
        let v = battery_value(&status(255, 255, 255, u32::MAX, 0));
        assert_eq!(v.get("present"), Some(&Value::Bool(false)));
        assert_eq!(v.get("plugged"), Some(&Value::Bool(false)));
    }
}
