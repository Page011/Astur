//! Fake Start-menu apps (names are made up).

use std::collections::HashSet;

use rsb_core::services::AppsCommand;
use rsb_core::value::Value;

use super::map;
use crate::real::apps::{check_launch_id, check_uri};

pub(crate) fn sample() -> Value {
    let app = |id: &str, name: &str| {
        map([("id", Value::Str(id.into())), ("name", Value::Str(name.into())), ("icon", Value::Nil)])
    };
    map([
        ("ready", Value::Bool(true)),
        (
            "apps",
            Value::List(vec![
                app("sample.calculator", "Calculator"),
                app("sample.editor", "Editor"),
                app("sample.terminal", "Terminal"),
            ]),
        ),
    ])
}

/// Launching changes nothing in the list (the world records it); an id the list does not hold and a
/// page the real service would not open are refused with the real text. A value set without an app
/// list has nothing to name: its launches only record.
pub(crate) fn apply(value: &mut Value, command: &AppsCommand) -> Result<(), String> {
    match command {
        AppsCommand::OpenUri(uri) => check_uri(uri),
        AppsCommand::Launch(id) => {
            let Some(Value::List(apps)) = value.get("apps") else { return Ok(()) };
            let ids: HashSet<String> =
                apps.iter().filter_map(|a| a.get("id").and_then(Value::as_str)).map(str::to_string).collect();
            check_launch_id(id, &ids)
        }
    }
}
