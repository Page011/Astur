//! Fake Bluetooth radio and paired devices (names are made up).

use std::collections::HashMap;

use rsb_core::services::BluetoothCommand;
use rsb_core::value::Value;

use super::{map, set_field};
use crate::real::bluetooth::{Published, check_command};

pub(crate) fn sample() -> Value {
    let device = |id: &str, name: &str, kind: &str, connected: bool, battery: Option<f64>, audio: bool| {
        map([
            ("id", Value::Str(id.into())),
            ("name", Value::Str(name.into())),
            ("kind", Value::Str(kind.into())),
            ("connected", Value::Bool(connected)),
            ("battery", battery.map_or(Value::Nil, Value::Num)),
            ("can_connect", Value::Bool(audio)),
        ])
    };
    map([
        ("available", Value::Bool(true)),
        ("enabled", Value::Bool(true)),
        ("can_toggle", Value::Bool(true)),
        (
            "devices",
            Value::List(vec![
                device("bt-1", "Sample Headphones", "audio", true, Some(27.0), true),
                device("bt-2", "Sample Mouse", "input", false, None, false),
                device("bt-3", "Sample Speaker", "audio", false, None, true),
            ]),
        ),
    ])
}

/// Behaves as the real service does for a command it accepts: switching the radio off disconnects
/// everything, connect and disconnect move the mark of a device that can be connected, forget drops
/// the pairing. A command the real service would refuse (a device that is not in the list, one that
/// cannot be connected, a radio that cannot be switched) is refused here with the same text and
/// changes nothing. A value set without a device list has nothing to name: its commands only record.
pub(crate) fn apply(value: &mut Value, command: &BluetoothCommand) -> Result<(), String> {
    let Some(Value::List(devices)) = value.get("devices").cloned() else { return Ok(()) };
    let published = Published {
        devices: devices
            .iter()
            .filter_map(|d| {
                Some((d.get("id")?.as_str()?.to_string(), d.get("can_connect").and_then(Value::as_bool) == Some(true)))
            })
            .collect::<HashMap<_, _>>(),
        can_toggle: value.get("can_toggle").and_then(Value::as_bool).unwrap_or(false),
    };
    check_command(command, &published)?;
    let named = |d: &Value, id: &str| d.get("id").and_then(Value::as_str) == Some(id);
    let enabled = value.get("enabled").and_then(Value::as_bool).unwrap_or(false);
    let set_connected = |id: &str, on: bool| -> Vec<Value> {
        devices
            .iter()
            .cloned()
            .map(|mut d| {
                if named(&d, id) && d.get("can_connect").and_then(Value::as_bool) == Some(true) {
                    set_field(&mut d, "connected", Value::Bool(on));
                }
                d
            })
            .collect()
    };
    match command {
        BluetoothCommand::SetEnabled(on) => {
            set_field(value, "enabled", Value::Bool(*on));
            if !*on {
                let off = devices
                    .into_iter()
                    .map(|mut d| {
                        set_field(&mut d, "connected", Value::Bool(false));
                        d
                    })
                    .collect();
                set_field(value, "devices", Value::List(off));
            }
        }
        BluetoothCommand::Connect(id) if enabled => set_field(value, "devices", Value::List(set_connected(id, true))),
        BluetoothCommand::Disconnect(id) => set_field(value, "devices", Value::List(set_connected(id, false))),
        BluetoothCommand::Forget(id) => {
            let kept = devices.into_iter().filter(|d| !named(d, id)).collect();
            set_field(value, "devices", Value::List(kept));
        }
        BluetoothCommand::Connect(_) | BluetoothCommand::OpenSettings | BluetoothCommand::Rescan => {}
    }
    Ok(())
}
