//! Audio output and input devices (MMDevice / Core Audio, on an MTA
//! thread): the default output's volume and mute, the list of active outputs
//! and inputs with a volume callback on each (at most 16 of a kind), and the
//! commands that name a device. Change callbacks only signal the thread; the
//! thread does every COM call, so a callback never re-enters the audio
//! engine.
//!
//! Every device command is checked against the list last published before it
//! is queued: an id Windows never gave out (a path, a GUID of something
//! else, a control character) never reaches a COM call. Names are values for
//! widgets and are never logged.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::services::{AudioCommand, AudioDeviceCommand, ServiceCommand, ServiceId, ServiceStatus};
use rsb_core::value::Value;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eCapture, eConsole, eMultimedia, eRender};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::WaitForMultipleObjects;

use super::Worker;
use super::endpoints::{self, Endpoint, Signals};
use super::policy_config;
use crate::util::{map, text};
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);
/// Longest device id accepted in a command (Windows ids are far shorter).
pub(crate) const MAX_ID: usize = 1024;
/// What a command naming an unpublished id is refused with (the fake says the same).
pub(crate) const NO_DEVICE: &str = "no audio device with that id";
pub(crate) const BAD_VOLUME: &str = "the volume is not a number";

enum Queued {
    Default(AudioCommand),
    Device(AudioDeviceCommand),
}

/// The ids last published, and whether switching works: what a command is
/// checked against on the hub thread.
#[derive(Default)]
struct Known {
    outputs: Vec<String>,
    inputs: Vec<String>,
    switch_available: bool,
}

impl Known {
    fn check(&self, id: &str) -> Result<(), String> {
        let listed = id.len() <= MAX_ID
            && !id.chars().any(char::is_control)
            && (self.outputs.iter().any(|d| d == id) || self.inputs.iter().any(|d| d == id));
        if listed { Ok(()) } else { Err(NO_DEVICE.into()) }
    }
}

struct Shared {
    signals: Arc<Signals>,
    commands: Mutex<VecDeque<Queued>>,
    known: Mutex<Known>,
}

/// The active endpoints, each with its callback registered.
#[derive(Default)]
struct Devices {
    outputs: Vec<Endpoint>,
    inputs: Vec<Endpoint>,
}

impl Devices {
    fn load(enumerator: &IMMDeviceEnumerator, signals: &Arc<Signals>) -> Devices {
        Devices {
            outputs: endpoints::list(enumerator, eRender, eMultimedia, signals),
            inputs: endpoints::list(enumerator, eCapture, eConsole, signals),
        }
    }

    fn find(&self, id: &str) -> Option<&Endpoint> {
        self.outputs.iter().chain(self.inputs.iter()).find(|e| e.id == id)
    }

    fn default_output(&self) -> Option<&Endpoint> {
        self.outputs.first().filter(|e| e.default)
    }
}

fn percent(v: f32) -> f64 {
    (f64::from(v) * 100.0).round()
}

fn device_value(e: &Endpoint, kind: &str) -> Value {
    let (volume, muted) = e.level().unwrap_or((0.0, false));
    map([
        ("id", Value::Str(e.id.clone())),
        ("name", Value::Str(e.name.clone())),
        ("kind", text(kind)),
        ("default", Value::Bool(e.default)),
        ("volume", Value::Num(percent(volume))),
        ("muted", Value::Bool(muted)),
        ("bluetooth", Value::Bool(e.bluetooth)),
    ])
}

fn publish(out: &Publisher, shared: &Shared, devices: &Devices, switch_available: bool) {
    {
        let mut known = shared.known.lock().unwrap_or_else(|e| e.into_inner());
        known.outputs = devices.outputs.iter().map(|e| e.id.clone()).collect();
        known.inputs = devices.inputs.iter().map(|e| e.id.clone()).collect();
        known.switch_available = switch_available;
    }
    let lists = (
        Value::List(devices.outputs.iter().map(|e| device_value(e, "output")).collect()),
        Value::List(devices.inputs.iter().map(|e| device_value(e, "input")).collect()),
    );
    let level = devices.default_output().and_then(Endpoint::level);
    let value = |available: bool, volume: f64, muted: bool| {
        map([
            ("available", Value::Bool(available)),
            ("volume", Value::Num(volume)),
            ("muted", Value::Bool(muted)),
            ("devices", lists.0.clone()),
            ("inputs", lists.1.clone()),
            ("switch_available", Value::Bool(switch_available)),
        ])
    };
    match level {
        Some((v, muted)) => out.publish(value(true, percent(v), muted)),
        None => {
            out.publish_as(value(false, 0.0, false), ServiceStatus::Unavailable, Some("no audio output device".into()))
        }
    }
}

fn apply(devices: &Devices, command: Queued) {
    match command {
        Queued::Default(c) => {
            let Some(ep) = devices.default_output() else { return };
            let Some((volume, muted)) = ep.level() else { return };
            match c {
                AudioCommand::SetVolume(p) => ep.set_volume(p),
                AudioCommand::ChangeVolume(d) => ep.set_volume(percent(volume) as f32 + d),
                AudioCommand::SetMute(m) => ep.set_muted(m),
                AudioCommand::ToggleMute => ep.set_muted(!muted),
            }
        }
        Queued::Device(c) => match c {
            AudioDeviceCommand::SetVolume(id, p) => {
                if let Some(ep) = devices.find(&id) {
                    ep.set_volume(p);
                }
            }
            AudioDeviceCommand::SetMuted(id, m) => {
                if let Some(ep) = devices.find(&id) {
                    ep.set_muted(m);
                }
            }
            AudioDeviceCommand::SetDefault(id) => {
                // The device may have gone since the command was checked.
                if devices.find(&id).is_some()
                    && let Err(e) = policy_config::set_default(&id)
                {
                    log::warn!("audio: {e}");
                }
            }
        },
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
            let switch_available = policy_config::available();
            let mut devices = Devices::load(&enumerator, &shared.signals);
            publish(&out, &shared, &devices, switch_available);
            loop {
                crate::count_source_wake();
                // SAFETY: both handles are live events.
                let waited = unsafe {
                    WaitForMultipleObjects(&[stop.get(), shared.signals.wake.get()], false, REREAD.as_millis() as u32)
                };
                if waited == WAIT_OBJECT_0 {
                    break;
                }
                if shared.signals.device_changed.swap(false, std::sync::atomic::Ordering::AcqRel) {
                    // A device change arrives as several callbacks: let them settle, then list once.
                    std::thread::sleep(Duration::from_millis(50));
                    shared.signals.device_changed.store(false, std::sync::atomic::Ordering::Release);
                    devices = Devices::load(&enumerator, &shared.signals);
                }
                let pending: Vec<Queued> =
                    shared.commands.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
                let switched = pending.iter().any(|c| matches!(c, Queued::Device(AudioDeviceCommand::SetDefault(_))));
                for c in pending {
                    apply(&devices, c);
                }
                if switched {
                    // The default moved: list again now instead of waiting for the notification.
                    devices = Devices::load(&enumerator, &shared.signals);
                }
                // A new reading, or (on the minute) the liveness check: an unchanged value still counts as alive.
                publish(&out, &shared, &devices, switch_available);
            }
            drop(devices);
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
            signals: Arc::new(Signals { wake: auto_event()?, device_changed: AtomicBool::new(false) }),
            commands: Mutex::new(VecDeque::new()),
            known: Mutex::new(Known::default()),
        });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-audio", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let Some(shared) = &self.shared else { return Err("the audio service is stopped".into()) };
        let queued = match command {
            ServiceCommand::Audio(c) => Queued::Default(c),
            ServiceCommand::AudioDevice(c) => {
                let known = shared.known.lock().unwrap_or_else(|e| e.into_inner());
                match &c {
                    AudioDeviceCommand::SetDefault(id) => {
                        known.check(id)?;
                        if !known.switch_available {
                            return Err(super::policy_config::UNAVAILABLE.into());
                        }
                    }
                    AudioDeviceCommand::SetVolume(id, p) => {
                        known.check(id)?;
                        if !p.is_finite() {
                            return Err(BAD_VOLUME.into());
                        }
                    }
                    AudioDeviceCommand::SetMuted(id, _) => known.check(id)?,
                }
                Queued::Device(c)
            }
            _ => return Err("not an audio command".into()),
        };
        shared.commands.lock().unwrap_or_else(|e| e.into_inner()).push_back(queued);
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

    fn service(known: Known) -> AudioService {
        let shared = Shared {
            signals: Arc::new(Signals { wake: auto_event().unwrap(), device_changed: AtomicBool::new(false) }),
            commands: Mutex::new(VecDeque::new()),
            known: Mutex::new(known),
        };
        AudioService { shared: Some(Arc::new(shared)), worker: None }
    }

    fn known() -> Known {
        Known {
            outputs: vec!["{0.0.0.00000000}.{aaaa}".into()],
            inputs: vec!["{0.0.1.00000000}.{bbbb}".into()],
            switch_available: true,
        }
    }

    fn queued(s: &AudioService) -> usize {
        s.shared.as_ref().unwrap().commands.lock().unwrap().len()
    }

    #[test]
    fn ids_that_were_not_published_are_refused_before_anything_is_queued() {
        let mut s = service(known());
        let long = "a".repeat(MAX_ID + 1);
        let bad = [
            r"C:\Windows\System32\cmd.exe",
            r"{00000000-0000-0000-0000-000000000000}\cmd.exe",
            r"..\..\evil",
            "{0.0.0.00000000}.{aaaa}\n",
            "{0.0.0.00000000}.{AAAA}",
            "",
            long.as_str(),
        ];
        for id in bad {
            for c in [
                AudioDeviceCommand::SetDefault(id.into()),
                AudioDeviceCommand::SetVolume(id.into(), 10.0),
                AudioDeviceCommand::SetMuted(id.into(), true),
            ] {
                let e = s.command(ServiceCommand::AudioDevice(c)).unwrap_err();
                assert_eq!(e, "no audio device with that id");
            }
        }
        assert_eq!(queued(&s), 0, "nothing reached the thread");
    }

    #[test]
    fn published_ids_of_either_kind_are_accepted() {
        let mut s = service(known());
        for id in ["{0.0.0.00000000}.{aaaa}", "{0.0.1.00000000}.{bbbb}"] {
            s.command(ServiceCommand::AudioDevice(AudioDeviceCommand::SetMuted(id.into(), true))).unwrap();
            s.command(ServiceCommand::AudioDevice(AudioDeviceCommand::SetVolume(id.into(), 30.0))).unwrap();
        }
        assert_eq!(queued(&s), 4);
    }

    #[test]
    fn switching_without_the_interface_says_so_and_queues_nothing() {
        let mut k = known();
        k.switch_available = false;
        let mut s = service(k);
        let e = s
            .command(ServiceCommand::AudioDevice(AudioDeviceCommand::SetDefault("{0.0.0.00000000}.{aaaa}".into())))
            .unwrap_err();
        assert_eq!(e, "switching the output is not available on this Windows");
        assert_eq!(queued(&s), 0);
    }

    #[test]
    fn a_volume_that_is_not_a_number_is_refused() {
        let mut s = service(known());
        let c = AudioDeviceCommand::SetVolume("{0.0.0.00000000}.{aaaa}".into(), f32::NAN);
        assert!(s.command(ServiceCommand::AudioDevice(c)).is_err());
        assert_eq!(queued(&s), 0);
    }

    #[test]
    fn a_stopped_service_and_foreign_commands_are_refused() {
        let mut s = AudioService::new();
        assert!(s.command(ServiceCommand::Audio(AudioCommand::ToggleMute)).is_err());
        let mut s = service(known());
        assert!(s.command(ServiceCommand::Audio(AudioCommand::ToggleMute)).is_ok());
        assert!(
            s.command(ServiceCommand::Microphone(rsb_core::services::MicrophoneCommand::ToggleMute)).is_err(),
            "another service's command"
        );
    }
}
