//! Fake values and command effects for audio devices, the microphone, the
//! media timeline and power. The power fake only changes its value: nothing
//! here can lock, sleep or end the session, and device commands name only
//! the fake's own ids.

use rsb_core::services::{AudioCommand, AudioDeviceCommand, MediaCommand, MicrophoneCommand, PowerCommand};
use rsb_core::value::Value;

use super::{map, set_field, text};
use crate::real::audio::{BAD_VOLUME, MAX_ID, NO_DEVICE};
use crate::real::{policy_config, power_mode};

/// Moment the sample media position was "read" (ms since 1970). Fixed, so
/// renders of the sample are the same every time.
const SAMPLE_POSITION_AT: i64 = 1_700_000_000_000;

fn device(id: &str, name: &str, kind: &str, default: bool, volume: f64, bluetooth: bool) -> Value {
    map([
        ("id", text(id)),
        ("name", text(name)),
        ("kind", text(kind)),
        ("default", Value::Bool(default)),
        ("volume", Value::Num(volume)),
        ("muted", Value::Bool(false)),
        ("bluetooth", Value::Bool(bluetooth)),
    ])
}

pub(crate) fn sample_audio() -> Value {
    map([
        ("available", Value::Bool(true)),
        ("volume", Value::Num(35.0)),
        ("muted", Value::Bool(false)),
        (
            "devices",
            Value::List(vec![
                device("fake-out-speakers", "Speakers", "output", true, 35.0, false),
                device("fake-out-headset", "Headset", "output", false, 60.0, true),
            ]),
        ),
        ("inputs", Value::List(vec![device("fake-in-mic", "Microphone", "input", true, 80.0, false)])),
        ("switch_available", Value::Bool(true)),
    ])
}

pub(crate) fn sample_microphone() -> Value {
    map([
        ("available", Value::Bool(true)),
        ("muted", Value::Bool(false)),
        ("volume", Value::Num(80.0)),
        ("in_use", Value::Bool(false)),
    ])
}

pub(crate) fn sample_power() -> Value {
    map([
        ("mode", Value::Str("balanced".into())),
        ("mode_available", Value::Bool(true)),
        ("can_sleep", Value::Bool(true)),
    ])
}

/// The media sample: a four-minute track a minute in, no cover, no app icon.
pub(crate) fn sample_media() -> Value {
    map([
        ("available", Value::Bool(true)),
        ("playing", Value::Bool(true)),
        ("title", text("Sample Track")),
        ("artist", text("Sample Artist")),
        ("album", text("Sample Album")),
        ("source", text("sample.player")),
        ("can_play_pause", Value::Bool(true)),
        ("can_next", Value::Bool(true)),
        ("can_previous", Value::Bool(true)),
        ("position", Value::Num(60.0)),
        ("position_at", Value::Int(SAMPLE_POSITION_AT)),
        ("length", Value::Num(240.0)),
        ("can_seek", Value::Bool(true)),
        ("art", Value::Nil),
        ("app_icon", Value::Nil),
    ])
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn list_mut<'a>(value: &'a mut Value, key: &str) -> Option<&'a mut Vec<Value>> {
    match value {
        Value::Map(m) => match m.get_mut(key) {
            Some(Value::List(l)) => Some(l),
            _ => None,
        },
        _ => None,
    }
}

/// The default output's entry follows the plain audio commands, as the real
/// service's list does.
pub(crate) fn apply_audio(value: &mut Value, command: &AudioCommand) {
    let volume = value.get("volume").and_then(Value::as_f64).unwrap_or(0.0);
    let muted = flag(value, "muted");
    let (new_volume, new_muted) = match command {
        AudioCommand::SetVolume(v) => (f64::from(*v).clamp(0.0, 100.0), muted),
        AudioCommand::ChangeVolume(d) => ((volume + f64::from(*d)).clamp(0.0, 100.0), muted),
        AudioCommand::SetMute(m) => (volume, *m),
        AudioCommand::ToggleMute => (volume, !muted),
    };
    set_field(value, "volume", Value::Num(new_volume));
    set_field(value, "muted", Value::Bool(new_muted));
    if let Some(outputs) = list_mut(value, "devices")
        && let Some(d) = outputs.iter_mut().find(|d| flag(d, "default"))
    {
        set_field(d, "volume", Value::Num(new_volume));
        set_field(d, "muted", Value::Bool(new_muted));
    }
}

/// Applies a device command. An id that is not in the published lists (or
/// is over `MAX_ID`, or holds a control character) is refused with nothing
/// changed, like the real service. A value a test set without any device
/// list has no devices to name: its commands are only recorded.
pub(crate) fn apply_audio_device(value: &mut Value, command: &AudioDeviceCommand) -> Result<(), String> {
    if ["devices", "inputs"].iter().all(|k| list_mut(value, k).is_none()) {
        return Ok(());
    }
    let id = match command {
        AudioDeviceCommand::SetDefault(id)
        | AudioDeviceCommand::SetVolume(id, _)
        | AudioDeviceCommand::SetMuted(id, _) => id,
    };
    let listed = id.len() <= MAX_ID
        && !id.chars().any(char::is_control)
        && ["devices", "inputs"].iter().any(|k| {
            list_mut(value, k).is_some_and(|l| l.iter().any(|d| d.get("id").and_then(Value::as_str) == Some(id)))
        });
    if !listed {
        return Err(NO_DEVICE.into());
    }
    if let AudioDeviceCommand::SetDefault(_) = command
        && !flag(value, "switch_available")
    {
        return Err(policy_config::UNAVAILABLE.into());
    }
    if let AudioDeviceCommand::SetVolume(_, v) = command
        && !v.is_finite()
    {
        return Err(BAD_VOLUME.into());
    }
    for key in ["devices", "inputs"] {
        let Some(list) = list_mut(value, key) else { continue };
        let Some(i) = list.iter().position(|d| d.get("id").and_then(Value::as_str) == Some(id.as_str())) else {
            continue;
        };
        match command {
            AudioDeviceCommand::SetVolume(_, v) => {
                set_field(&mut list[i], "volume", Value::Num(f64::from(*v).clamp(0.0, 100.0)))
            }
            AudioDeviceCommand::SetMuted(_, m) => set_field(&mut list[i], "muted", Value::Bool(*m)),
            AudioDeviceCommand::SetDefault(_) => {
                for (n, d) in list.iter_mut().enumerate() {
                    set_field(d, "default", Value::Bool(n == i));
                }
                // The default first, the rest in order, as the real list.
                let d = list.remove(i);
                list.insert(0, d);
            }
        }
        // The plain fields mirror the default output.
        if key == "devices" {
            let first = list.iter().find(|d| flag(d, "default")).cloned();
            if let Some(d) = first {
                let (v, m) = (d.get("volume").cloned(), d.get("muted").cloned());
                if let (Some(v), Some(m)) = (v, m) {
                    set_field(value, "volume", v);
                    set_field(value, "muted", m);
                }
            }
        }
        break;
    }
    Ok(())
}

pub(crate) fn apply_microphone(value: &mut Value, command: &MicrophoneCommand) {
    let muted = flag(value, "muted");
    match command {
        MicrophoneCommand::SetMuted(m) => set_field(value, "muted", Value::Bool(*m)),
        MicrophoneCommand::ToggleMute => set_field(value, "muted", Value::Bool(!muted)),
    }
}

/// Only the mode changes, and only when the fake says it can; the session
/// actions leave the value alone (the world records them).
pub(crate) fn apply_power(value: &mut Value, command: &PowerCommand) -> Result<(), String> {
    if let PowerCommand::SetMode(mode) = command {
        if !flag(value, "mode_available") {
            return Err(power_mode::UNAVAILABLE.into());
        }
        set_field(value, "mode", Value::Str(mode.name().into()));
    }
    Ok(())
}

/// Media commands. Seeking needs `can_seek` and stays inside the track.
pub(crate) fn apply_media(value: &mut Value, command: &MediaCommand) -> Result<(), String> {
    let playing = flag(value, "playing");
    match command {
        MediaCommand::PlayPause => set_field(value, "playing", Value::Bool(!playing)),
        MediaCommand::Play => set_field(value, "playing", Value::Bool(true)),
        MediaCommand::Pause => set_field(value, "playing", Value::Bool(false)),
        MediaCommand::Next | MediaCommand::Previous => {}
        MediaCommand::Seek(ms) => {
            if !flag(value, "can_seek") {
                return Err("this player cannot seek".into());
            }
            let to = *ms as f64 / 1000.0;
            let to = value.get("length").and_then(Value::as_f64).map_or(to, |l| to.min(l));
            set_field(value, "position", Value::Num(to));
        }
    }
    Ok(())
}

/// Moves the media position on by `seconds` (what the clock would do while
/// playing), never past the end.
pub(crate) fn advance_media(value: &mut Value, seconds: f64) {
    let position = value.get("position").and_then(Value::as_f64).unwrap_or(0.0) + seconds;
    let position = value.get("length").and_then(Value::as_f64).map_or(position, |l| position.min(l)).max(0.0);
    set_field(value, "position", Value::Num(position));
}
