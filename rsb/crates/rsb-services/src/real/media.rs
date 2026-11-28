//! The current media session through the Windows media transport controls
//! (WinRT, on an MTA thread). Session, playback, property and timeline events
//! only wake the thread; it re-reads and publishes. Titles and artists are
//! values for widgets and are never logged.
//!
//! The cover and the player's own icon are pictures: their bytes are read
//! with a 2 s deadline (at most 4 MiB), decoded by the safe path in
//! `images`, published as images under `rsb-image:media/...` and referred to
//! from the value by key. Nothing is written to disk or the log.

use std::collections::{HashMap, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rsb_core::pixels::RgbaImage;
use rsb_core::services::{MediaCommand, ServiceCommand, ServiceId};
use rsb_core::value::Value;
use windows::ApplicationModel::AppInfo;
use windows::Foundation::{Size, TypedEventHandler};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionMediaProperties, GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGPIXELSX, ReleaseDC};
use windows::Win32::System::Threading::{SetEvent, WaitForMultipleObjects};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{HSTRING, Interface};

use super::winrt::{TIMED_OUT, await_op};
use super::{Worker, images};
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
/// The cover and the app icon each get this long, stream and decode together.
const PICTURE_WAIT: Duration = images::DECODE_DEADLINE;
/// Largest cover or icon file read.
const MAX_PICTURE_BYTES: u64 = 4 << 20;
/// Cover reads tried for one track (the player may fill it in a moment after the title).
const ART_ATTEMPTS: u32 = 4;
/// App icons kept to publish again when the player changes back.
const ICONS_KEPT: usize = 8;
/// 100 ns ticks between 1601-01-01 (WinRT time) and 1970-01-01.
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

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

/// Where in the track the player says it is: seconds, the wall-clock moment
/// that was true at (ms since 1970), the track length in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Timeline {
    position: f64,
    at_ms: i64,
    length: Option<f64>,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A timeline from WinRT's raw values (100 ns ticks). None when the player
/// reports nothing at all (all zeros).
fn timeline_from(start: i64, end: i64, position: i64, updated: i64, now_ms: i64) -> Option<Timeline> {
    let length = (end > start).then(|| (end - start) as f64 / 1e7);
    let pos = (position - start).max(0) as f64 / 1e7;
    if length.is_none() && pos == 0.0 {
        return None;
    }
    let pos = length.map_or(pos, |l| pos.min(l));
    let at_ms = if updated > UNIX_EPOCH_TICKS { (updated - UNIX_EPOCH_TICKS) / 10_000 } else { now_ms };
    Some(Timeline { position: pos, at_ms, length })
}

/// A player that refreshes its timeline every second would republish the
/// value every second: while playing, a reading that agrees with the
/// previous one advanced by the elapsed time (within a second) keeps the
/// previous position and moment, so the value is unchanged.
fn settle(prev: Option<Timeline>, new: Timeline, playing: bool) -> Timeline {
    if let Some(p) = prev
        && playing
        && p.length == new.length
    {
        let expected = p.position + (new.at_ms - p.at_ms) as f64 / 1000.0;
        if (expected - new.position).abs() < 1.0 {
            return p;
        }
    }
    new
}

fn read_timeline(session: &GlobalSystemMediaTransportControlsSession) -> Option<Timeline> {
    let t = session.GetTimelineProperties().ok()?;
    timeline_from(
        t.StartTime().ok()?.Duration,
        t.EndTime().ok()?.Duration,
        t.Position().ok()?.Duration,
        t.LastUpdatedTime().ok()?.UniversalTime,
        now_ms(),
    )
}

/// What one read of the session gives, before the pictures.
struct Snapshot {
    value: Value,
    props: GlobalSystemMediaTransportControlsSessionMediaProperties,
    /// Hash of title, artist, album and player: a new one is a new track.
    track: u64,
    source: String,
    playing: bool,
    timeline: Option<Timeline>,
}

fn hash_of(parts: &[&str]) -> u64 {
    let mut h = DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    h.finish()
}

fn read(session: &GlobalSystemMediaTransportControlsSession) -> windows::core::Result<Snapshot> {
    let op = session.TryGetMediaPropertiesAsync()?;
    let props = await_op!(op, PROPERTIES_WAIT)?;
    let info = session.GetPlaybackInfo()?;
    let controls = info.Controls()?;
    let playing = info.PlaybackStatus()? == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;
    let s = |h: windows::core::Result<HSTRING>| cut_chars(&h.map(|h| h.to_string()).unwrap_or_default(), MAX_TEXT);
    let (title, artist, album, source) =
        (s(props.Title()), s(props.Artist()), s(props.AlbumTitle()), s(session.SourceAppUserModelId()));
    let track = hash_of(&[&title, &artist, &album, &source]);
    let value = map([
        ("available", Value::Bool(true)),
        ("playing", Value::Bool(playing)),
        ("title", Value::Str(title)),
        ("artist", Value::Str(artist)),
        ("album", Value::Str(album)),
        ("source", Value::Str(source.clone())),
        ("can_play_pause", Value::Bool(controls.IsPlayPauseToggleEnabled().unwrap_or(false))),
        ("can_next", Value::Bool(controls.IsNextEnabled().unwrap_or(false))),
        ("can_previous", Value::Bool(controls.IsPreviousEnabled().unwrap_or(false))),
        ("can_seek", Value::Bool(controls.IsPlaybackPositionEnabled().unwrap_or(false))),
    ]);
    Ok(Snapshot { value, props, track, source, playing, timeline: read_timeline(session) })
}

/// Reads at most `MAX_PICTURE_BYTES` from a stream reference, within the
/// time left before `deadline`.
fn read_stream(reference: &IRandomAccessStreamReference, deadline: Instant) -> Option<Vec<u8>> {
    let left = |d: Instant| {
        let l = d.saturating_duration_since(Instant::now());
        (!l.is_zero()).then_some(l)
    };
    let op = reference.OpenReadAsync().ok()?;
    let stream = await_op!(op, left(deadline)?).ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > MAX_PICTURE_BYTES {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    let load = reader.LoadAsync(size as u32).ok()?;
    let got = await_op!(load, left(deadline)?).ok()?;
    let mut bytes = vec![0u8; got as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

fn fetch_art(props: &GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<RgbaImage> {
    let deadline = Instant::now() + PICTURE_WAIT;
    let reference = props.Thumbnail().ok()?;
    let bytes = read_stream(&reference, deadline)?;
    images::decode(&bytes, images::MAX_SIDE).ok()
}

/// The system's scale for 32 px icons (1.0 at 96 dpi).
fn dpi_scale() -> f32 {
    // SAFETY: the screen DC is obtained and released here; nothing is drawn.
    unsafe {
        let hdc = GetDC(None);
        let dpi = GetDeviceCaps(Some(hdc), LOGPIXELSX);
        ReleaseDC(None, hdc);
        if dpi > 0 { dpi as f32 / 96.0 } else { 1.0 }
    }
}

/// The player app's logo, when Windows has a package for it.
fn fetch_app_icon(source: &str) -> Option<RgbaImage> {
    if source.is_empty() {
        return None;
    }
    let deadline = Instant::now() + PICTURE_WAIT;
    let info = AppInfo::GetFromAppUserModelId(&HSTRING::from(source)).ok()?;
    let side = (32.0 * dpi_scale()).round();
    let reference = info.DisplayInfo().ok()?.GetLogo(Size { Width: side, Height: side }).ok()?;
    let bytes = read_stream(&reference.cast::<IRandomAccessStreamReference>().ok()?, deadline)?;
    images::decode(&bytes, images::MAX_SIDE).ok()
}

/// The pictures of the current track and player, and what is published of
/// them.
#[derive(Default)]
struct Pictures {
    art_track: u64,
    art_attempts: u32,
    art_key: Option<Arc<str>>,
    next_art: u64,
    icon_source: Option<u64>,
    icon_key: Option<Arc<str>>,
    icons: HashMap<u64, Option<RgbaImage>>,
}

impl Pictures {
    /// Adds `art` and `app_icon` to `value`, fetching and publishing what is
    /// new.
    fn add_to(&mut self, value: &mut Value, snap: &Snapshot, out: &Publisher) {
        if self.art_track != snap.track {
            self.art_track = snap.track;
            self.art_attempts = 0;
            self.art_key = None;
        }
        if self.art_key.is_none() && self.art_attempts < ART_ATTEMPTS {
            self.art_attempts += 1;
            if let Some(image) = fetch_art(&snap.props) {
                self.next_art += 1;
                let key: Arc<str> = format!("rsb-image:media/{}", self.next_art).into();
                out.publish_image(key.clone(), image);
                self.art_key = Some(key);
            }
        }

        let source = hash_of(&[&snap.source]);
        if self.icon_source != Some(source) {
            self.icon_source = Some(source);
            if self.icons.len() >= ICONS_KEPT {
                self.icons.clear();
            }
            let image = self.icons.entry(source).or_insert_with(|| fetch_app_icon(&snap.source)).clone();
            self.icon_key = image.map(|image| {
                let key: Arc<str> = format!("rsb-image:media/app-{source:016x}").into();
                out.publish_image(key.clone(), image);
                key
            });
        }

        if let Value::Map(m) = value {
            m.insert("art".into(), self.art_key.as_ref().map_or(Value::Nil, |k| Value::Str(k.to_string())));
            m.insert("app_icon".into(), self.icon_key.as_ref().map_or(Value::Nil, |k| Value::Str(k.to_string())));
        }
    }
}

/// Event registrations on one session, removed when it is replaced.
struct SessionEvents {
    session: GlobalSystemMediaTransportControlsSession,
    tokens: Vec<(Kind, i64)>,
}

#[derive(Clone, Copy)]
enum Kind {
    Properties,
    Playback,
    Timeline,
}

impl SessionEvents {
    fn attach(session: GlobalSystemMediaTransportControlsSession, shared: &Arc<Shared>) -> SessionEvents {
        let mut tokens = Vec::new();
        let s1 = shared.clone();
        if let Ok(t) = session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s1);
            Ok(())
        })) {
            tokens.push((Kind::Properties, t));
        }
        let s2 = shared.clone();
        if let Ok(t) = session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s2);
            Ok(())
        })) {
            tokens.push((Kind::Playback, t));
        }
        let s3 = shared.clone();
        if let Ok(t) = session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
            wake(&s3);
            Ok(())
        })) {
            tokens.push((Kind::Timeline, t));
        }
        SessionEvents { session, tokens }
    }
}

impl Drop for SessionEvents {
    fn drop(&mut self) {
        for (kind, token) in self.tokens.drain(..) {
            let _ = match kind {
                Kind::Properties => self.session.RemoveMediaPropertiesChanged(token),
                Kind::Playback => self.session.RemovePlaybackInfoChanged(token),
                Kind::Timeline => self.session.RemoveTimelinePropertiesChanged(token),
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
        // WinRT time spans are 100 ns units.
        MediaCommand::Seek(ms) => session
            .TryChangePlaybackPositionAsync(i64::try_from(ms).unwrap_or(i64::MAX).saturating_mul(10_000))
            .map(drop),
    };
}

fn with_timeline(value: &mut Value, timeline: Option<Timeline>) {
    if let (Value::Map(m), Some(t)) = (value, timeline) {
        m.insert("position".into(), Value::Num((t.position * 1000.0).round() / 1000.0));
        m.insert("position_at".into(), Value::Int(t.at_ms));
        m.insert("length".into(), t.length.map_or(Value::Nil, |l| Value::Num((l * 1000.0).round() / 1000.0)));
    }
}

fn run(out: Publisher, shared: Arc<Shared>, stop: Arc<Owned>) {
    // SAFETY: initialises the WinRT apartment of this thread as MTA; balanced at the end.
    let ro = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
    let manager =
        GlobalSystemMediaTransportControlsSessionManager::RequestAsync().and_then(|op| await_op!(op, MANAGER_WAIT));
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
    let mut pictures = Pictures::default();
    let mut last_timeline: Option<(u64, Timeline)> = None;
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
            Some(Ok(snap)) => {
                let mut value = snap.value.clone();
                let timeline = snap.timeline.map(|new| {
                    let prev = last_timeline.filter(|(track, _)| *track == snap.track).map(|(_, t)| t);
                    settle(prev, new, snap.playing)
                });
                last_timeline = timeline.map(|t| (snap.track, t));
                with_timeline(&mut value, timeline);
                pictures.add_to(&mut value, &snap, &out);
                out.publish(value);
            }
            _ => {
                last_timeline = None;
                out.publish(empty_value());
            }
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
    drop(pictures);
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
    use super::*;

    const SEC: i64 = 10_000_000;

    #[test]
    fn a_timeline_is_seconds_into_the_track_with_the_moment_it_was_true() {
        let at = UNIX_EPOCH_TICKS + 5_000 * 10_000;
        let t = timeline_from(0, 200 * SEC, 30 * SEC, at, 1).unwrap();
        assert_eq!((t.position, t.length, t.at_ms), (30.0, Some(200.0), 5_000));
        // A track that starts at an offset counts from there; a position past the end is cut to the length.
        let t = timeline_from(10 * SEC, 110 * SEC, 500 * SEC, at, 1).unwrap();
        assert_eq!((t.position, t.length), (100.0, Some(100.0)));
        // No update time: the moment is now.
        let t = timeline_from(0, 60 * SEC, 0, 0, 42).unwrap();
        assert_eq!(t.at_ms, 42);
    }

    #[test]
    fn a_player_with_no_timeline_gives_none_and_a_live_stream_has_no_length() {
        assert_eq!(timeline_from(0, 0, 0, 0, 1), None);
        let live = timeline_from(0, 0, 90 * SEC, 0, 1).unwrap();
        assert_eq!((live.position, live.length), (90.0, None));
    }

    #[test]
    fn a_reading_that_follows_the_clock_leaves_the_value_unchanged_and_a_seek_does_not() {
        let first = Timeline { position: 10.0, at_ms: 1_000_000, length: Some(200.0) };
        // One second later the player says 11.0: the same line, so the old reading stays.
        let second = Timeline { position: 11.0, at_ms: 1_001_000, length: Some(200.0) };
        assert_eq!(settle(Some(first), second, true), first);
        // A jump of ten seconds is a seek.
        let seek = Timeline { position: 21.0, at_ms: 1_001_000, length: Some(200.0) };
        assert_eq!(settle(Some(first), seek, true), seek);
        // Paused: any change counts.
        assert_eq!(settle(Some(first), second, false), second);
        // A new length is a new track even at the same position.
        let other = Timeline { length: Some(180.0), ..second };
        assert_eq!(settle(Some(first), other, true), other);
        assert_eq!(settle(None, second, true), second);
    }

    #[test]
    fn the_same_track_and_player_hash_alike_and_different_ones_do_not() {
        assert_eq!(hash_of(&["a", "b"]), hash_of(&["a", "b"]));
        assert_ne!(hash_of(&["a", "b"]), hash_of(&["b", "a"]));
        assert_ne!(hash_of(&["ab", ""]), hash_of(&["a", "b"]), "parts are not run together");
    }

    #[test]
    fn a_packaged_apps_logo_decodes_and_an_unknown_app_gives_none() {
        // SAFETY: initialises the WinRT apartment of this test thread; the thread ends with the test.
        let _ = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
        assert!(fetch_app_icon("").is_none(), "no source, no icon");
        assert!(fetch_app_icon("no.such.package_0000000000000!App").is_none(), "an app Windows has no package for");
        // Windows' own packaged apps, when this install has any of them.
        let found = ["Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", "Microsoft.Windows.Photos_8wekyb3d8bbwe!App"]
            .into_iter()
            .find_map(fetch_app_icon);
        match found {
            Some(icon) => {
                println!("app icon: decoded {} x {} px, {} bytes", icon.width, icon.height, icon.bytes());
                assert!(icon.width >= 1 && icon.width <= images::MAX_SIDE && icon.height <= images::MAX_SIDE);
                assert!(icon.data.as_chunks::<4>().0.iter().any(|p| p[3] != 0), "the logo is not blank");
            }
            None => println!("app icon: no packaged app to try on this install"),
        }
    }
}
