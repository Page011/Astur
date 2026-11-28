//! The Windows-backed services.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;
use std::time::Duration;

use rsb_core::services::ServiceId;

use crate::win::{Owned, manual_event, signal};
use crate::{HubOptions, Service, ServiceFactory};

pub(crate) mod apps;
pub(crate) mod audio;
pub(crate) mod battery;
pub(crate) mod bluetooth;
pub(crate) mod endpoints;
pub(crate) mod images;
pub(crate) mod media;
pub(crate) mod microphone;
pub(crate) mod network;
pub(crate) mod policy_config;
pub(crate) mod power;
pub(crate) mod power_mode;
pub(crate) mod system;
pub(crate) mod wifi;
pub(crate) mod window;
pub(crate) mod winrt;

/// Makes the real services.
pub struct RealFactory;

impl ServiceFactory for RealFactory {
    fn create(&self, id: ServiceId, options: &HubOptions) -> Box<dyn Service> {
        match id {
            ServiceId::System => Box::new(system::SystemService::new()),
            ServiceId::Battery => Box::new(battery::BatteryService::new()),
            ServiceId::Audio => Box::new(audio::AudioService::new()),
            ServiceId::Network => Box::new(network::NetworkService::new()),
            ServiceId::Media => Box::new(media::MediaService::new()),
            ServiceId::Window => Box::new(window::WindowService::new()),
            ServiceId::Workspaces => Box::new(crate::workspaces::WorkspacesService::new(options.workspaces.clone())),
            ServiceId::Microphone => Box::new(microphone::MicrophoneService::new()),
            ServiceId::Bluetooth => Box::new(bluetooth::BluetoothService::new()),
            ServiceId::Apps => Box::new(apps::AppsService::new()),
            ServiceId::Power => Box::new(power::PowerService::new(options.power_actions.clone())),
            ServiceId::Wifi => Box::new(wifi::WifiService::new()),
        }
    }
}

/// One service thread and the event that stops it. Stopping signals the
/// event and waits at most 1 s; a thread stuck in a system call is left to
/// exit with the process.
pub(crate) struct Worker {
    pub(crate) stop: Arc<Owned>,
    thread: Option<JoinHandle<()>>,
    done: Receiver<()>,
}

impl Worker {
    /// Runs `body(stop_event)` on a new thread called `name`.
    pub(crate) fn spawn(name: &str, body: impl FnOnce(Arc<Owned>) + Send + 'static) -> Result<Worker, String> {
        let stop = Arc::new(manual_event()?);
        let (done_tx, done) = mpsc::channel();
        let thread_stop = stop.clone();
        let thread = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                body(thread_stop);
                let _ = done_tx.send(());
            })
            .map_err(|e| format!("could not start the {name} thread: {e}"))?;
        Ok(Worker { stop, thread: Some(thread), done })
    }

    pub(crate) fn stop(&mut self) {
        signal(&self.stop);
        if self.done.recv_timeout(Duration::from_secs(1)).is_ok() {
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        } else {
            log::warn!("a service thread did not stop within 1 s; leaving it to exit with the process");
        }
    }
}
