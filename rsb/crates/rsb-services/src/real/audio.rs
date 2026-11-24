//! Default output device volume and mute (MMDevice / Core Audio, on an MTA
//! thread). Change callbacks only signal the thread; the thread does every
//! COM call, so a callback never re-enters the audio engine.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::services::{AudioCommand, ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;
use windows::Win32::Foundation::{PROPERTYKEY, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, eMultimedia, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};
use windows::core::{PCWSTR, implement};

use super::Worker;
use crate::util::map;
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);

/// What the callbacks share with the thread.
struct Shared {
    wake: Owned,
    commands: Mutex<VecDeque<AudioCommand>>,
    /// The default device (or a device's state) changed: attach again.
    device_changed: AtomicBool,
}

fn wake_of(s: &Shared) {
    // SAFETY: `wake` is a live event owned by the shared state.
    let _ = unsafe { SetEvent(s.wake.get()) };
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    shared: Arc<Shared>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        wake_of(&self.shared);
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceCallback {
    shared: Arc<Shared>,
}

impl DeviceCallback_Impl {
    fn changed(&self) -> windows::core::Result<()> {
        self.shared.device_changed.store(true, Ordering::Release);
        wake_of(&self.shared);
        Ok(())
    }
}

impl IMMNotificationClient_Impl for DeviceCallback_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        self.changed()
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eMultimedia { self.changed() } else { Ok(()) }
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The attached endpoint and its volume callback registration.
struct Endpoint {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // SAFETY: the callback was registered on this endpoint when attached.
        let _ = unsafe { self.volume.UnregisterControlChangeNotify(&self.callback) };
    }
}

fn attach(enumerator: &IMMDeviceEnumerator, shared: &Arc<Shared>) -> Option<Endpoint> {
    // SAFETY: plain COM calls on live interfaces from an MTA thread.
    unsafe {
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia).ok()?;
        let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        let callback: IAudioEndpointVolumeCallback = VolumeCallback { shared: shared.clone() }.into();
        volume.RegisterControlChangeNotify(&callback).ok()?;
        Some(Endpoint { volume, callback })
    }
}

fn read(endpoint: &Endpoint) -> Option<(f32, bool)> {
    // SAFETY: plain COM calls on a live interface.
    unsafe {
        let v = endpoint.volume.GetMasterVolumeLevelScalar().ok()?;
        let m = endpoint.volume.GetMute().ok()?.as_bool();
        Some((v, m))
    }
}

fn apply(endpoint: &Endpoint, command: AudioCommand) {
    let Some((volume, muted)) = read(endpoint) else { return };
    // SAFETY: plain COM calls on a live interface; a null event context is allowed.
    unsafe {
        match command {
            AudioCommand::SetVolume(p) => {
                let _ = endpoint.volume.SetMasterVolumeLevelScalar((p / 100.0).clamp(0.0, 1.0), std::ptr::null());
            }
            AudioCommand::ChangeVolume(d) => {
                let _ =
                    endpoint.volume.SetMasterVolumeLevelScalar((volume + d / 100.0).clamp(0.0, 1.0), std::ptr::null());
            }
            AudioCommand::SetMute(m) => {
                let _ = endpoint.volume.SetMute(m, std::ptr::null());
            }
            AudioCommand::ToggleMute => {
                let _ = endpoint.volume.SetMute(!muted, std::ptr::null());
            }
        }
    }
}

fn publish(out: &Publisher, endpoint: Option<&Endpoint>) {
    match endpoint.and_then(read) {
        Some((v, muted)) => out.publish(map([
            ("available", Value::Bool(true)),
            ("volume", Value::Num((f64::from(v) * 100.0).round())),
            ("muted", Value::Bool(muted)),
        ])),
        None => out.publish_as(
            map([("available", Value::Bool(false)), ("volume", Value::Num(0.0)), ("muted", Value::Bool(false))]),
            ServiceStatus::Unavailable,
            Some("no audio output device".into()),
        ),
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
            let client: IMMNotificationClient = DeviceCallback { shared: shared.clone() }.into();
            // SAFETY: both interfaces are live; unregistered below.
            let registered = unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }.is_ok();
            let mut endpoint = attach(&enumerator, &shared);
            publish(&out, endpoint.as_ref());
            loop {
                crate::count_source_wake();
                // SAFETY: both handles are live events.
                let waited = unsafe {
                    WaitForMultipleObjects(&[stop.get(), shared.wake.get()], false, REREAD.as_millis() as u32)
                };
                if waited == WAIT_OBJECT_0 {
                    break;
                }
                if shared.device_changed.swap(false, Ordering::AcqRel) {
                    endpoint = None;
                    endpoint = attach(&enumerator, &shared).or(endpoint);
                }
                let pending: Vec<AudioCommand> =
                    shared.commands.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                if let Some(ep) = &endpoint {
                    for c in pending {
                        apply(ep, c);
                    }
                }
                // A new reading, or (on the minute) the liveness check: an unchanged value still counts as alive.
                publish(&out, endpoint.as_ref());
            }
            drop(endpoint);
            if registered {
                // SAFETY: unregisters the client registered above.
                let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&client) };
            }
        }
    }
    // SAFETY: balances the successful CoInitializeEx above.
    unsafe { CoUninitialize() };
}

pub(crate) struct AudioService {
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
}

impl AudioService {
    pub(crate) fn new() -> AudioService {
        AudioService { shared: None, worker: None }
    }
}

impl Service for AudioService {
    fn id(&self) -> ServiceId {
        ServiceId::Audio
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = Arc::new(Shared {
            wake: auto_event()?,
            commands: Mutex::new(VecDeque::new()),
            device_changed: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-audio", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Audio(c), Some(shared)) = (command, &self.shared) else {
            return Err("not an audio command, or the service is stopped".into());
        };
        shared.commands.lock().unwrap_or_else(|e| e.into_inner()).push_back(c);
        signal(&shared.wake);
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
