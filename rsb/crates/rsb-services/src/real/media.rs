//! The current media session through the Windows media transport controls
//! (WinRT, on an MTA thread). Session, playback and property events only
//! wake the thread; it re-reads and publishes. Titles and artists are values
//! for widgets and are never logged.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsb_core::services::{MediaCommand, ServiceCommand, ServiceId};
use rsb_core::value::Value;
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::HRESULT;

use super::Worker;
use crate::util::{cut_chars, map, text};
use crate::win::{Owned, auto_event, signal};
use crate::{Publisher, Service};

const REREAD: Duration = Duration::from_secs(60);
/// Longest title, artist or album kept.
const MAX_TEXT: usize = 256;
/// How long a player may take to hand over its properties.
const PROPERTIES_WAIT: Duration = Duration::from_secs(2);
/// How long the session list may take to open.
const MANAGER_WAIT: Duration = Duration::from_secs(5);
/// `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`.
const TIMED_OUT: HRESULT = HRESULT(0x8007_05B4_u32 as i32);

/// Waits for a WinRT operation for at most `wait`. A player that never
/// answers must not hold the media thread (and so its stop) for ever: the
/// operation is cancelled and the caller gets `TIMED_OUT`. `start` hands the
/// operation a callback for its result (`op.when(done)`), `cancel` cancels it
/// (`op.Cancel()`); the operation type is not named because the `windows`
/// crate does not export it.
fn wait_within<T: Send + 'static>(
    start: impl FnOnce(Box<dyn FnOnce(windows::core::Result<T>) + Send>) -> windows::core::Result<()>,
    cancel: impl FnOnce(),
    wait: Duration,
) -> windows::core::Result<T> {
    let (tx, rx) = mpsc::channel();
    start(Box::new(move |result| {
        let _ = tx.send(result);
    }))?;
    match rx.recv_timeout(wait) {
        Ok(result) => result,
        Err(_) => {
            cancel();
            Err(windows::core::Error::from(TIMED_OUT))
        }
    }
}

struct Shared {
    wake: Owned,
    commands: Mutex<VecDeque<MediaCommand>>,
}

fn wake(s: &Shared) {
    // SAFETY: `wake` is a live event owned by the shared state.
    let _ = unsafe { SetEvent(s.wake.get()) };
}

fn empty_value() -> Value {
    map([
        ("available", Value::Bool(false)),
        ("playing", Value::Bool(false)),
        ("title", text("")),
        ("artist", text("")),
        ("album", text("")),
        ("source", text("")),
        ("can_play_pause", Value::Bool(false)),
        ("can_next", Value::Bool(false)),
        ("can_previous", Value::Bool(false)),
    ])
}

fn read(session: &GlobalSystemMediaTransportControlsSession) -> windows::core::Result<Value> {
    let op = session.TryGetMediaPropertiesAsync()?;
    let props = wait_within(|done| op.when(done), || drop(op.Cancel()), PROPERTIES_WAIT)?;
    let info = session.GetPlaybackInfo()?;
    let controls = info.Controls()?;
    let playing = info.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let s = |h: windows::core::Result<windows::core::HSTRING>| {
        cut_chars(&h.map(|h| h.to_string()).unwrap_or_default(), MAX_TEXT)
    };
    Ok(map([
        ("available", Value::Bool(true)),
        ("playing", Value::Bool(playing)),
        ("title", Value::Str(s(props.Title()))),
        ("artist", Value::Str(s(props.Artist()))),
        ("album", Value::Str(s(props.AlbumTitle()))),
        ("source", Value::Str(s(session.SourceAppUserModelId()))),
        ("can_play_pause", Value::Bool(controls.IsPlayPauseToggleEnabled().unwrap_or(false))),
        ("can_next", Value::Bool(controls.IsNextEnabled().unwrap_or(false))),
        ("can_previous", Value::Bool(controls.IsPreviousEnabled().unwrap_or(false))),
    ]))
}

/// Event registrations on one session, removed when it is replaced.
struct SessionEvents {
    session: GlobalSystemMediaTransportControlsSession,
    tokens: Vec<(bool, i64)>,
}

impl SessionEvents {
    fn attach(session: GlobalSystemMediaTransportControlsSession, shared: &Arc<Shared>) -> SessionEvents {
        let mut tokens = Vec::new();
        let s1 = shared.clone();
        if let Ok(t) = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s1);
            Ok(())
        })) {
            tokens.push((true, t));
        }
        let s2 = shared.clone();
        if let Ok(t) = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s2);
            Ok(())
        })) {
            tokens.push((false, t));
        }
        SessionEvents { session, tokens }
    }
}

impl Drop for SessionEvents {
    fn drop(&mut self) {
        for (props, token) in self.tokens.drain(..) {
            let _ = if props {
                self.session.RemoveMediaPropertiesChanged(token)
            } else {
                self.session.RemovePlaybackInfoChanged(token)
            };
        }
    }
}

fn apply(session: &GlobalSystemMediaTransportControlsSession, command: MediaCommand) {
    // The request is made and not waited for: a player that never answers must not hold this thread.
    let _ = match command {
        MediaCommand::PlayPause => session.TryTogglePlayPauseAsync().map(drop),
        MediaCommand::Play => session.TryPlayAsync().map(drop),
        MediaCommand::Pause => session.TryPauseAsync().map(drop),
        MediaCommand::Next => session.TrySkipNextAsync().map(drop),
        MediaCommand::Previous => session.TrySkipPreviousAsync().map(drop),
    };
}

fn run(out: Publisher, shared: Arc<Shared>, stop: Arc<Owned>) {
    // SAFETY: initialises the WinRT apartment of this thread as MTA; balanced at the end.
    let ro = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| wait_within(|done| op.when(done), || drop(op.Cancel()), MANAGER_WAIT));
    let manager = match manager {
        Ok(m) => m,
        Err(_) => {
            out.status(
                rsb_core::services::ServiceStatus::Error,
                Some("the media session list could not be opened".into()),
            );
            if ro.is_ok() {
                // SAFETY: balances RoInitialize.
                unsafe { RoUninitialize() };
            }
            return;
        }
    };
    let s = shared.clone();
    let token = manager
        .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s);
            Ok(())
        }))
        .ok();
    let mut events: Option<SessionEvents> = None;
    let mut current_id = String::new();
    loop {
        // The current session may have changed: attach to the new one.
        let session = manager.GetCurrentSession().ok();
        let id =
            session.as_ref().and_then(|s| s.SourceAppUserModelId().ok()).map(|h| h.to_string()).unwrap_or_default();
        if session.is_none() || id != current_id || events.is_none() {
            events = session.clone().map(|s| SessionEvents::attach(s, &shared));
            current_id = id;
        }
        let pending: Vec<MediaCommand> = shared.commands.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
        if let Some(e) = &events {
            for c in pending {
                apply(&e.session, c);
            }
        }
        // A player that does not answer in time publishes nothing this turn, so
        // the hub marks the source stale (and restarts it) instead of the bar
        // claiming nothing is playing.
        match events.as_ref().map(|e| read(&e.session)) {
            Some(Err(e)) if e.code() == TIMED_OUT => {}
            Some(Ok(v)) => out.publish(v),
            _ => out.publish(empty_value()),
        }

        crate::count_source_wake();
        // SAFETY: both handles are live events.
        let waited =
            unsafe { WaitForMultipleObjects(&[stop.get(), shared.wake.get()], false, REREAD.as_millis() as u32) };
        if waited == WAIT_OBJECT_0 {
            break;
        }
        // Property events arrive in bursts (title, artist, art): let them settle.
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(events);
    if let Some(t) = token {
        let _ = manager.RemoveCurrentSessionChanged(t);
    }
    // The proxies are released while the apartment still exists: releasing them after RoUninitialize
    // crashed the process at exit.
    drop(manager);
    if ro.is_ok() {
        // SAFETY: balances RoInitialize.
        unsafe { RoUninitialize() };
    }
}

pub(crate) struct MediaService {
    shared: Option<Arc<Shared>>,
    worker: Option<Worker>,
}

impl MediaService {
    pub(crate) fn new() -> MediaService {
        MediaService { shared: None, worker: None }
    }
}

impl Service for MediaService {
    fn id(&self) -> ServiceId {
        ServiceId::Media
    }

    fn start(&mut self, out: Publisher) -> Result<(), String> {
        let shared = Arc::new(Shared { wake: auto_event()?, commands: Mutex::new(VecDeque::new()) });
        let thread_shared = shared.clone();
        self.worker = Some(Worker::spawn("rsb-media", move |stop| run(out, thread_shared, stop))?);
        self.shared = Some(shared);
        Ok(())
    }

    fn command(&mut self, command: ServiceCommand) -> Result<(), String> {
        let (ServiceCommand::Media(c), Some(shared)) = (command, &self.shared) else {
            return Err("not a media command, or the service is stopped".into());
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use super::*;

    #[test]
    fn an_operation_that_never_finishes_times_out_is_cancelled_and_a_finished_one_returns() {
        // The callback is kept and never called: the operation never completes.
        let kept = Mutex::new(None);
        let cancelled = AtomicBool::new(false);
        let t = Instant::now();
        let e = wait_within::<u32>(
            |done| {
                *kept.lock().unwrap() = Some(done);
                Ok(())
            },
            || cancelled.store(true, Ordering::Relaxed),
            Duration::from_millis(300),
        )
        .expect_err("the operation is still running");
        assert_eq!(e.code(), TIMED_OUT);
        assert!(cancelled.load(Ordering::Relaxed), "a timed-out operation is cancelled");
        assert!(t.elapsed() >= Duration::from_millis(300) && t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());

        let finished = wait_within::<u32>(
            |done| {
                done(Ok(7));
                Ok(())
            },
            || panic!("a finished operation is not cancelled"),
            Duration::from_secs(5),
        );
        assert_eq!(finished.unwrap(), 7);

        let failed = wait_within::<u32>(|_| Err(windows::core::Error::from(TIMED_OUT)), || {}, Duration::from_secs(5));
        assert_eq!(failed.unwrap_err().code(), TIMED_OUT);
    }
}
