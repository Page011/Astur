//! Fake Wi-Fi scan list (names are made up).

use std::collections::HashSet;

use rsb_core::services::WifiCommand;
use rsb_core::value::Value;

use super::{map, set_field};
use crate::real::wifi::check_command;

pub(crate) fn sample() -> Value {
    let net = |ssid: &str, signal: f64, secure: bool, connected: bool, known: bool| {
        map([
            ("ssid", Value::Str(ssid.into())),
            ("signal", Value::Num(signal)),
            ("secure", Value::Bool(secure)),
            ("connected", Value::Bool(connected)),
            ("known", Value::Bool(known)),
        ])
    };
    map([
        ("available", Value::Bool(true)),
        ("scanning", Value::Bool(false)),
        (
            "networks",
            Value::List(vec![
                net("Sample Home", 82.0, true, true, true),
                net("Sample Cafe", 41.0, false, false, true),
                net("Sample Guest", 23.0, true, false, false),
            ]),
        ),
        ("access", Value::Str("allowed".into())),
    ])
}

/// Connect moves the connected mark to a saved network of that name; disconnect clears it; a scan
/// ends at once. A command the real service would refuse (a name that is not a saved network, a scan
/// without location access) is refused here with the same text and changes nothing. A value set
/// without a network list has nothing to name: its commands only record.
pub(crate) fn apply(value: &mut Value, command: &WifiCommand) -> Result<(), String> {
    let Some(Value::List(nets)) = value.get("networks").cloned() else { return Ok(()) };
    let saved = |n: &Value| n.get("known").and_then(Value::as_bool).unwrap_or(false);
    let known: HashSet<String> = nets
        .iter()
        .filter(|n| saved(n))
        .filter_map(|n| n.get("ssid").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let allowed = value.get("access").and_then(Value::as_str) == Some("allowed");
    check_command(command, allowed, &known)?;
    match command {
        WifiCommand::Scan => set_field(value, "scanning", Value::Bool(false)),
        WifiCommand::Connect(ssid) => {
            let named = |n: &Value| n.get("ssid").and_then(Value::as_str) == Some(ssid.as_str());
            let moved = nets
                .into_iter()
                .map(|mut n| {
                    let on = named(&n);
                    set_field(&mut n, "connected", Value::Bool(on));
                    n
                })
                .collect();
            set_field(value, "networks", Value::List(moved));
        }
        WifiCommand::Disconnect => {
            let cleared = nets
                .into_iter()
                .map(|mut n| {
                    set_field(&mut n, "connected", Value::Bool(false));
                    n
                })
                .collect();
            set_field(value, "networks", Value::List(cleared));
        }
    }
    Ok(())
}
