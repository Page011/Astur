//! Services as the rest of the program sees them: ids, events, commands,
//! and the value shapes. `SERVICE_SCHEMAS` is the one source of truth for
//! each service's fields: the services crate's values are checked against
//! it in tests, and the Luau type generator reads it.

use crate::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ServiceId {
    System,
    Battery,
    Audio,
    Network,
    Media,
    Window,
    Workspaces,
}

impl ServiceId {
    pub const ALL: [ServiceId; 7] = [
        ServiceId::System,
        ServiceId::Battery,
        ServiceId::Audio,
        ServiceId::Network,
        ServiceId::Media,
        ServiceId::Window,
        ServiceId::Workspaces,
    ];

    /// Name in Luau (`rsb.services.<name>`), logs and permissions.
    pub fn name(self) -> &'static str {
        match self {
            ServiceId::System => "system",
            ServiceId::Battery => "battery",
            ServiceId::Audio => "audio",
            ServiceId::Network => "network",
            ServiceId::Media => "media",
            ServiceId::Window => "window",
            ServiceId::Workspaces => "workspaces",
        }
    }

    pub fn from_name(name: &str) -> Option<ServiceId> {
        ServiceId::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Health of a service as widgets see it (`<service>.status` in Luau).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ServiceStatus {
    /// Nobody subscribed.
    Off,
    /// Subscribed, no value yet.
    Starting,
    Ok,
    /// No value or liveness signal within the service's stale time. Widgets
    /// show `--`, never the old value as if it were fresh.
    Stale,
    /// The thing does not exist here (no window manager, no audio device).
    /// Widgets show their empty state; it is not an error.
    Unavailable,
    /// Failed; `error` says why. Retried with backoff.
    Error,
}

impl ServiceStatus {
    pub fn name(self) -> &'static str {
        match self {
            ServiceStatus::Off => "off",
            ServiceStatus::Starting => "starting",
            ServiceStatus::Ok => "ok",
            ServiceStatus::Stale => "stale",
            ServiceStatus::Unavailable => "unavailable",
            ServiceStatus::Error => "error",
        }
    }
}

/// Pushed by the hub to the script thread whenever a value or the status
/// changes.
#[derive(Clone, Debug, PartialEq)]
pub struct ServiceEvent {
    pub service: ServiceId,
    /// Increases with every event of this service.
    pub version: u64,
    pub status: ServiceStatus,
    /// The full current value (a `Value::Map` with the schema's fields), or
    /// None when only the status changed.
    pub value: Option<Value>,
    /// Short reason for `Error` / `Unavailable`, no private data.
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServiceCommand {
    Audio(AudioCommand),
    Media(MediaCommand),
    Workspaces(WorkspaceCommand),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioCommand {
    /// 0..=100
    SetVolume(f32),
    /// Added to the current volume, result clamped to 0..=100.
    ChangeVolume(f32),
    SetMute(bool),
    ToggleMute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkspaceCommand {
    /// By workspace `name`.
    Focus(String),
    /// Next / previous workspace on the given monitor index (None = the
    /// focused monitor).
    FocusNext(Option<u32>),
    FocusPrevious(Option<u32>),
}

/// One field of a service value or a nested type.
#[derive(Clone, Copy, Debug)]
pub struct FieldSchema {
    pub name: &'static str,
    /// Luau type text: `number`, `boolean`, `string`, `string?`,
    /// `{Workspace}`, or a quoted-literal union.
    pub ty: &'static str,
    pub doc: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct TypeSchema {
    pub name: &'static str,
    pub doc: &'static str,
    pub fields: &'static [FieldSchema],
}

#[derive(Clone, Copy, Debug)]
pub struct CommandSchema {
    /// Method name in Luau: `rsb.services.audio:set_volume(50)`.
    pub name: &'static str,
    /// Luau parameter list after `self`, e.g. `percent: number`.
    pub params: &'static str,
    pub doc: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct ServiceSchema {
    pub id: ServiceId,
    /// Luau type name, e.g. `AudioService`.
    pub type_name: &'static str,
    pub doc: &'static str,
    pub fields: &'static [FieldSchema],
    pub commands: &'static [CommandSchema],
    /// Sampled on a timer (has `:interval(d)`); otherwise event driven.
    pub sampled: bool,
}

const fn f(name: &'static str, ty: &'static str, doc: &'static str) -> FieldSchema {
    FieldSchema { name, ty, doc }
}

const fn c(name: &'static str, params: &'static str, doc: &'static str) -> CommandSchema {
    CommandSchema { name, params, doc }
}

/// Fields every service table has besides its own, filled by the script
/// runtime from `ServiceEvent`.
pub static COMMON_FIELDS: &[FieldSchema] = &[
    f("status", "ServiceStatus", "\"off\", \"starting\", \"ok\", \"stale\", \"unavailable\" or \"error\""),
    f("stale", "boolean", "true when status is \"stale\": show a placeholder, not the old value"),
    f("error", "string?", "why the service is unavailable or failing"),
];

pub static SERVICE_TYPES: &[TypeSchema] = &[
    TypeSchema {
        name: "Monitor",
        doc: "A monitor as the window manager reports it",
        fields: &[
            f("index", "number", "0-based position in the window manager's list"),
            f("name", "string", "the window manager's name for it"),
            f("handle", "number?", "HMONITOR value when the window manager reports it; matches ctx.monitor.handle"),
            f("device", "string?", "Windows device name, e.g. \\\\.\\DISPLAY1, when known"),
            f("workspaces", "{Workspace}", "workspaces on this monitor, in order"),
        ],
    },
    TypeSchema {
        name: "Workspace",
        doc: "One workspace",
        fields: &[
            f("name", "string", "unique name; pass it to focus()"),
            f("display_name", "string", "label to show"),
            f("focused", "boolean", "has keyboard focus"),
            f("visible", "boolean", "shown on its monitor"),
            f("has_windows", "boolean", "at least one window"),
            f("window_count", "number", "number of windows"),
        ],
    },
];

pub static SERVICE_SCHEMAS: &[ServiceSchema] = &[
    ServiceSchema {
        id: ServiceId::System,
        type_name: "SystemService",
        doc: "CPU and memory load, sampled while someone reads it (default every 2 s)",
        fields: &[
            f("cpu", "number", "CPU load in percent, 0-100, over the last interval"),
            f("memory", "number", "memory in use in percent, 0-100"),
            f("memory_used", "number", "memory in use, bytes"),
            f("memory_total", "number", "installed memory visible to Windows, bytes"),
        ],
        commands: &[],
        sampled: true,
    },
    ServiceSchema {
        id: ServiceId::Battery,
        type_name: "BatteryService",
        doc: "Battery and power source",
        fields: &[
            f("present", "boolean", "false on a PC without a battery: show nothing or a plug"),
            f("percent", "number?", "charge 0-100 when present"),
            f("charging", "boolean", "charging now"),
            f("plugged", "boolean", "on mains power"),
            f("saver", "boolean", "battery saver is on"),
            f("seconds_left", "number?", "estimated seconds of battery left, when Windows knows"),
        ],
        commands: &[],
        sampled: false,
    },
    ServiceSchema {
        id: ServiceId::Audio,
        type_name: "AudioService",
        doc: "Default output device volume",
        fields: &[
            f("available", "boolean", "an output device exists"),
            f("volume", "number", "0-100"),
            f("muted", "boolean", "muted"),
        ],
        commands: &[
            c("set_volume", "percent: number", "set the volume, 0-100"),
            c("change_volume", "delta: number", "add delta (may be negative) to the volume"),
            c("set_muted", "muted: boolean", "mute or unmute"),
            c("toggle_mute", "", "flip mute"),
        ],
        sampled: false,
    },
    ServiceSchema {
        id: ServiceId::Network,
        type_name: "NetworkService",
        doc: "Connection state and throughput (rates sampled while read, default every 2 s)",
        fields: &[
            f("connected", "boolean", "any network connected"),
            f("internet", "boolean", "Windows reports internet access"),
            f("kind", "NetworkKind", "\"wifi\", \"ethernet\", \"cellular\", \"other\" or \"none\""),
            f("ssid", "string?", "Wi-Fi network name (never logged)"),
            f("signal", "number?", "Wi-Fi signal 0-100"),
            f("down_bps", "number", "bytes per second received, all adapters"),
            f("up_bps", "number", "bytes per second sent, all adapters"),
        ],
        commands: &[],
        sampled: true,
    },
    ServiceSchema {
        id: ServiceId::Media,
        type_name: "MediaService",
        doc: "The current media session (Windows media controls)",
        fields: &[
            f("available", "boolean", "a media session exists"),
            f("playing", "boolean", "playing now"),
            f("title", "string", "track title, empty when unknown (never logged)"),
            f("artist", "string", "artist, empty when unknown (never logged)"),
            f("album", "string", "album, empty when unknown"),
            f("source", "string", "app id of the player"),
            f("can_play_pause", "boolean", "play/pause is allowed"),
            f("can_next", "boolean", "next is allowed"),
            f("can_previous", "boolean", "previous is allowed"),
        ],
        commands: &[
            c("play_pause", "", "toggle play and pause"),
            c("play", "", "play"),
            c("pause", "", "pause"),
            c("next", "", "next track"),
            c("previous", "", "previous track"),
        ],
        sampled: false,
    },
    ServiceSchema {
        id: ServiceId::Window,
        type_name: "WindowService",
        doc: "The foreground window",
        fields: &[
            f("has_window", "boolean", "a normal window has focus (false on the desktop)"),
            f("title", "string", "window title (never logged)"),
            f("process", "string", "program file name without folder, e.g. \"Code.exe\""),
        ],
        commands: &[],
        sampled: false,
    },
    ServiceSchema {
        id: ServiceId::Workspaces,
        type_name: "WorkspacesService",
        doc: "Workspaces from a running window manager (GlazeWM-compatible WebSocket, e.g. Astur or GlazeWM, or komorebi)",
        fields: &[
            f("provider", "WorkspaceProvider", "\"glazewm\", \"komorebi\" or \"none\""),
            f("connected", "boolean", "talking to a window manager now"),
            f("monitors", "{Monitor}", "monitors with their workspaces; empty when none"),
        ],
        commands: &[
            c("focus", "name: string", "switch to the workspace with this name"),
            c("focus_next", "monitor: number?", "next workspace on a monitor (default: the focused one)"),
            c("focus_previous", "monitor: number?", "previous workspace on a monitor"),
        ],
        sampled: false,
    },
];

/// Luau literal unions named by fields above.
pub static SERVICE_ENUMS: &[(&str, &[&str])] = &[
    ("ServiceStatus", &["off", "starting", "ok", "stale", "unavailable", "error"]),
    ("NetworkKind", &["wifi", "ethernet", "cellular", "other", "none"]),
    ("WorkspaceProvider", &["glazewm", "komorebi", "none"]),
];

pub fn schema(id: ServiceId) -> &'static ServiceSchema {
    SERVICE_SCHEMAS.iter().find(|s| s.id == id).expect("every service has a schema")
}

/// Checks a service value against its schema: a map with exactly the
/// schema's fields, each of the right type (optional fields may be nil or
/// missing). Used by the services crate's tests and, in debug builds, by the
/// hub. The error names the field path (`monitors[1].workspaces[2].name`).
pub fn check_value(id: ServiceId, value: &Value) -> Result<(), String> {
    check_fields(schema(id).fields, value, "")
}

fn check_fields(fields: &[FieldSchema], value: &Value, at: &str) -> Result<(), String> {
    let Value::Map(map) = value else {
        return Err(format!("{}expected a table, got {}", prefix(at), value.type_name()));
    };
    for key in map.keys() {
        if !fields.iter().any(|f| f.name == key) {
            return Err(format!("{}unexpected field '{key}'", prefix(at)));
        }
    }
    for f in fields {
        let path = if at.is_empty() { f.name.to_string() } else { format!("{at}.{}", f.name) };
        check_type(f.ty, map.get(f.name).unwrap_or(&Value::Nil), &path)?;
    }
    Ok(())
}

fn prefix(at: &str) -> String {
    if at.is_empty() { String::new() } else { format!("{at}: ") }
}

fn check_type(ty: &str, v: &Value, at: &str) -> Result<(), String> {
    if let Some(inner) = ty.strip_suffix('?') {
        return if matches!(v, Value::Nil) { Ok(()) } else { check_type(inner, v, at) };
    }
    let wrong = || Err(format!("{at}: expected {ty}, got {}", v.type_name()));
    match ty {
        "number" => match v {
            Value::Int(_) => Ok(()),
            Value::Num(n) if n.is_finite() => Ok(()),
            _ => wrong(),
        },
        "boolean" => {
            if matches!(v, Value::Bool(_)) {
                Ok(())
            } else {
                wrong()
            }
        }
        "string" => {
            if matches!(v, Value::Str(_)) {
                Ok(())
            } else {
                wrong()
            }
        }
        _ => {
            if let Some(elem) = ty.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
                let items: &[Value] = match v {
                    Value::List(l) => l,
                    Value::Map(m) if m.is_empty() => &[],
                    _ => return wrong(),
                };
                for (i, item) in items.iter().enumerate() {
                    check_type(elem, item, &format!("{at}[{}]", i + 1))?;
                }
                return Ok(());
            }
            if let Some((_, values)) = SERVICE_ENUMS.iter().find(|(n, _)| *n == ty) {
                return match v {
                    Value::Str(s) if values.contains(&s.as_str()) => Ok(()),
                    Value::Str(s) => Err(format!("{at}: '{s}' is not one of {}", values.join(", "))),
                    _ => wrong(),
                };
            }
            if let Some(t) = SERVICE_TYPES.iter().find(|t| t.name == ty) {
                return check_fields(t.fields, v, at);
            }
            Err(format!("{at}: schema names unknown type '{ty}'"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_service_has_one_schema() {
        for id in ServiceId::ALL {
            assert_eq!(SERVICE_SCHEMAS.iter().filter(|s| s.id == id).count(), 1, "{id:?}");
            assert_eq!(ServiceId::from_name(id.name()), Some(id));
        }
    }

    fn map(pairs: &[(&str, Value)]) -> Value {
        Value::Map(pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    #[test]
    fn values_are_checked_against_the_schema() {
        let sys = map(&[
            ("cpu", Value::Num(23.0)),
            ("memory", Value::Int(41)),
            ("memory_used", Value::Int(1 << 33)),
            ("memory_total", Value::Int(1 << 34)),
        ]);
        assert_eq!(check_value(ServiceId::System, &sys), Ok(()));
        let mut missing = sys.clone();
        if let Value::Map(m) = &mut missing {
            m.remove("cpu");
        }
        assert!(check_value(ServiceId::System, &missing).unwrap_err().contains("cpu"));
        let mut extra = sys.clone();
        if let Value::Map(m) = &mut extra {
            m.insert("gpu".into(), Value::Int(1));
        }
        assert!(check_value(ServiceId::System, &extra).unwrap_err().contains("unexpected field 'gpu'"));
        let bad = map(&[("cpu", Value::Str("x".into()))]);
        assert!(check_value(ServiceId::System, &bad).is_err());
        assert!(check_value(ServiceId::System, &Value::Num(f64::NAN)).is_err());

        let battery = map(&[
            ("present", Value::Bool(false)),
            ("charging", Value::Bool(false)),
            ("plugged", Value::Bool(true)),
            ("saver", Value::Bool(false)),
        ]);
        assert_eq!(check_value(ServiceId::Battery, &battery), Ok(()), "optional fields may be missing");

        let ws = |name: Value| {
            map(&[
                ("name", name),
                ("display_name", Value::Str("1".into())),
                ("focused", Value::Bool(true)),
                ("visible", Value::Bool(true)),
                ("has_windows", Value::Bool(false)),
                ("window_count", Value::Int(0)),
            ])
        };
        let workspaces = |name: Value| {
            map(&[
                ("provider", Value::Str("glazewm".into())),
                ("connected", Value::Bool(true)),
                (
                    "monitors",
                    Value::List(vec![map(&[
                        ("index", Value::Int(0)),
                        ("name", Value::Str("m".into())),
                        ("workspaces", Value::List(vec![ws(name)])),
                    ])]),
                ),
            ])
        };
        assert_eq!(check_value(ServiceId::Workspaces, &workspaces(Value::Str("1".into()))), Ok(()));
        let err = check_value(ServiceId::Workspaces, &workspaces(Value::Int(1))).unwrap_err();
        assert!(err.starts_with("monitors[1].workspaces[1].name"), "{err}");
        let mut none = workspaces(Value::Str("1".into()));
        if let Value::Map(m) = &mut none {
            m.insert("provider".into(), Value::Str("yabai".into()));
        }
        assert!(check_value(ServiceId::Workspaces, &none).unwrap_err().contains("not one of"));
    }

    #[test]
    fn every_schema_type_is_known() {
        fn known(ty: &str) -> bool {
            let ty = ty.strip_suffix('?').unwrap_or(ty);
            let ty = ty.strip_prefix('{').and_then(|t| t.strip_suffix('}')).unwrap_or(ty);
            matches!(ty, "number" | "boolean" | "string")
                || SERVICE_ENUMS.iter().any(|(n, _)| *n == ty)
                || SERVICE_TYPES.iter().any(|t| t.name == ty)
        }
        for f in SERVICE_SCHEMAS
            .iter()
            .flat_map(|s| s.fields)
            .chain(SERVICE_TYPES.iter().flat_map(|t| t.fields))
            .chain(COMMON_FIELDS)
        {
            assert!(known(f.ty), "{}: {}", f.name, f.ty);
        }
    }
}
