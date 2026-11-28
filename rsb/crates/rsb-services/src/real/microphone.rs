//! The default recording device (MMDevice eCapture, an MTA thread) and whether an app is
//! recording (the CapabilityAccessManager consent store in the registry, read only).
//!
//! The consent store is not a documented API: an app subkey of
//! `...\ConsentStore\microphone` (or of its `NonPackaged` subkey) whose
//! `LastUsedTimeStop` is 0 while `LastUsedTimeStart` is not is recording.
//! The key is watched with `RegNotifyChangeKeyValue`; any read error means
//! "not in use" and one log line without a key name (the names are app paths).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::services::{MicrophoneCommand, ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;
use windows::Win32::Foundation::{ERROR_NO_MORE_ITEMS, ERROR_SUCCESS, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, RRF_RT_REG_QWORD, RegCloseKey,
    RegEnumKeyExW, RegGetValueW, RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use windows::Win32::System::Threading::WaitForMultipleObjects;
use windows::core::{PCWSTR, PWSTR, w};

use super::Worker;
use super::endpoints::{self, Endpoint, Signals};
use crate::util::map;
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);
/// The consent store key for the microphone, under HKEY_CURRENT_USER.
pub(crate) const CONSENT_KEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone");
/// Most app subkeys looked at under each of the two keys.
const MAX_APPS: u32 = 1024;

/// The consent store's rule for "recording now".
pub(crate) fn recording(start: u64, stop: u64) -> bool {
    start != 0 && stop == 0
}

/// An open registry key, closed on drop.
struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by this module and is closed once.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn open(parent: HKEY, path: PCWSTR, access: windows::Win32::System::Registry::REG_SAM_FLAGS) -> Option<Key> {
    let mut key = HKEY::default();
    // SAFETY: `path` is a NUL-terminated string; `key` receives a new handle on success.
    let r = unsafe { RegOpenKeyExW(parent, path, None, access, &mut key) };
    (r == ERROR_SUCCESS).then_some(Key(key))
}

fn qword(key: &Key, name: PCWSTR) -> Option<u64> {
    let mut value = 0u64;
    let mut size = size_of::<u64>() as u32;
    // SAFETY: `value` is 8 writable bytes, as `size` says.
    let r = unsafe {
        RegGetValueW(
            key.0,
            PCWSTR::null(),
            name,
            RRF_RT_REG_QWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(value)
}

/// Whether any app subkey of `parent` (skipping `skip`) is recording.
fn any_recording(parent: &Key, skip: Option<&str>) -> Result<bool, ()> {
    let mut index = 0u32;
    while index < MAX_APPS {
        let mut name = [0u16; 512];
        let mut len = name.len() as u32;
        // SAFETY: `name` is writable for `len` UTF-16 units.
        let r =
            unsafe { RegEnumKeyExW(parent.0, index, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) };
        if r == ERROR_NO_MORE_ITEMS {
            return Ok(false);
        }
        if r != ERROR_SUCCESS {
            return Err(());
        }
        index += 1;
        let text = String::from_utf16_lossy(&name[..len as usize]);
        if skip.is_some_and(|s| s.eq_ignore_ascii_case(&text)) {
            continue;
        }
        // A subkey that cannot be opened (removed meanwhile, or not ours) is not recording.
        if let Some(app) = open(parent.0, PCWSTR(name.as_ptr()), KEY_READ) {
            let start = qword(&app, w!("LastUsedTimeStart")).unwrap_or(0);
            let stop = qword(&app, w!("LastUsedTimeStop")).unwrap_or(0);
            if recording(start, stop) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Reads the consent store: Ok(true) when an app is recording, Ok(false)
/// when none is or the store is absent, Err when it could not be read.
pub(crate) fn in_use() -> Result<bool, ()> {
    let Some(root) = open(HKEY_CURRENT_USER, CONSENT_KEY, KEY_READ) else { return Ok(false) };
    if any_recording(&root, Some("NonPackaged"))? {
        return Ok(true);
    }
    match open(root.0, w!("NonPackaged"), KEY_READ) {
        Some(non_packaged) => any_recording(&non_packaged, None),
        None => Ok(false),
    }
}

struct Shared {
    signals: Arc<Signals>,
    commands: Mutex<VecDeque<MicrophoneCommand>>,
    /// The registry watch signals this one.
    registry: Owned,
    /// One log line per run about an unreadable consent store.
    logged_read_error: AtomicBool,
}

/// The default recording device, with the volume callback registered.
fn attach(enumerator: &IMMDeviceEnumerator, signals: &Arc<Signals>) -> Option<Endpoint> {
    endpoints::list(enumerator, eCapture, eConsole, signals).into_iter().find(|e| e.default)
}

fn publish(out: &Publisher, shared: &Shared, endpoint: Option<&Endpoint>) {
    let in_use = in_use().unwrap_or_else(|()| {
        if !shared.logged_read_error.swap(true, Ordering::Relaxed) {
            log::debug!("microphone: the consent store could not be read; reporting not in use");
        }
        false
    });
    match endpoint.and_then(Endpoint::level) {
        Some((v, muted)) => out.publish(map([
            ("available", Value::Bool(true)),
            ("muted", Value::Bool(muted)),
            ("volume", Value::Num((f64::from(v) * 100.0).round())),
            ("in_use", Value::Bool(in_use)),
        ])),
        None => out.publish_as(
            map([
                ("available", Value::Bool(false)),
                ("muted", Value::Bool(false)),
                ("volume", Value::Num(0.0)),
                ("in_use", Value::Bool(in_use)),
            ]),
            ServiceStatus::Unavailable,
            Some("no recording device".into()),
        ),
    }
}

fn apply(endpoint: &Endpoint, command: MicrophoneCommand) {
    let Some((_, muted)) = endpoint.level() else { return };
    match command {
        MicrophoneCommand::SetMuted(m) => endpoint.set_muted(m),
        MicrophoneCommand::ToggleMute => endpoint.set_muted(!muted),
    }
}

fn run(out: Publisher, shared: Arc<Shared>, stop: Arc<Owned>) {
    // SAFETY: initialises COM for this thread as an MTA; balanced below.
    let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if com.is_err() {
        out.status(ServiceStatus::Error, Some("COM could not be started".into()));
        return;
    }
    // SAFETY: standard COM activation from an initialised thread.
    let enumerator: Result<IMMDeviceEnumerator, _> = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) };
    match enumerator {
        Err(_) => out.status(ServiceStatus::Error, Some("the audio device list could not be opened".into())),
        Ok(enumerator) => {
            let client = endpoints::device_client(&shared.signals);
            // SAFETY: both interfaces are live; unregistered below.
            let registered = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }.is_ok();
            let watched = open(HKEY_CURRENT_USER, CONSENT_KEY, KEY_READ | KEY_NOTIFY);
            let arm = |key: &Option<Key>| {
                if let Some(k) = key {
                    // SAFETY: the key is open with KEY_NOTIFY; the event is live; asynchronous mode signals it.
                    let _ = unsafe {
                        RegNotifyChangeKeyValue(
                            k.0,
                            true,
                            REG_NOTIFY_CHANGE_LAST_SET,
                            Some(shared.registry.get()),
                            true,
                        )
                    };
                }
            };
            arm(&watched);
            let mut endpoint = attach(&enumerator, &shared.signals);
            publish(&out, &shared, endpoint.as_ref());
            loop {
                crate::count_source_wake();
                // SAFETY: all handles are live events.
                let waited = unsafe {
                    WaitForMultipleObjects(
                        &[stop.get(), shared.signals.wake.get(), shared.registry.get()],
                        false,
                        REREAD.as_millis() as u32,
                    )
                };
                if waited == WAIT_OBJECT_0 {
                    break;
                }
                if waited.0 == WAIT_OBJECT_0.0 + 2 {
                    // Re-arm before reading, so a change during the read is not lost.
                    arm(&watched);
                    // Apps start and stop in bursts (several subkeys are written): let them settle.
                    std::thread::sleep(Duration::from_millis(50));
                }
                if shared.signals.device_changed.swap(false, Ordering::AcqRel) {
                    endpoint = attach(&enumerator, &shared.signals);
                }
                let pending: Vec<MicrophoneCommand> =
                    shared.commands.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                if let Some(ep) = &endpoint {
                    for c in pending {
                        apply(ep, c);
                    }
                }
                publish(&out, &shared, endpoint.as_ref());
            }
            drop(endpoint);
            drop(watched);
            if registered {
                // SAFETY: unregisters the client registered above.
                let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&client) };
            }
        }
    }
    // SAFETY: balances the successful CoInitializeEx above.
    unsafe { CoUninitialize() };
}

pub(crate) struct MicrophoneService {
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
}

impl MicrophoneService {
    pub(crate) fn new() -> MicrophoneService {
        MicrophoneService { shared: None, worker: None }
    }
}

impl Service for MicrophoneService {
    fn id(&self) -> ServiceId {
        ServiceId::Microphone
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = Arc::new(Shared {
            signals: Arc::new(Signals { wake: auto_event()?, device_changed: AtomicBool::new(false) }),
            commands: Mutex::new(VecDeque::new()),
            registry: auto_event()?,
            logged_read_error: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-microphone", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Microphone(c), Some(shared)) = (command, &self.shared) else {
            return Err("not a microphone command, or the service is stopped".into());
        };
        shared.commands.lock().unwrap_or_else(|e| e.into_inner()).push_back(c);
        signal(&shared.signals.wake);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(mut w) = self.worker.take() {
            w.stop();
        }
        self.shared = None;
    }

    fn stale_after(&self) -> Duration {
        Duration::from_secs(180)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_is_recording_while_it_has_started_and_not_stopped() {
        assert!(recording(133_000_000_000_000_000, 0));
        assert!(!recording(133_000_000_000_000_000, 133_000_000_000_000_001), "stopped");
        assert!(!recording(0, 0), "never used");
        assert!(!recording(0, 5));
    }
}
