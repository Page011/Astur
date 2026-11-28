//! The one workspaces model both providers are mapped to, with the caps
//! applied (provider data is untrusted).

use rsb_core::value::Value;
use serde_json::Value as Json;

use crate::util::{cut_chars, map, text};
use crate::{MAX_MONITORS, MAX_WORKSPACE_NAME, MAX_WORKSPACES_PER_MONITOR};

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Workspace {
    pub name: String,
    pub display_name: String,
    pub focused: bool,
    pub visible: bool,
    pub window_count: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Monitor {
    pub name: String,
    pub handle: Option<i64>,
    pub device: Option<String>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Provider {
    None,
    Glaze,
    Komorebi,
}

impl Provider {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Provider::None => "none",
            Provider::Glaze => "glazewm",
            Provider::Komorebi => "komorebi",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Model {
    pub provider: Provider,
    pub connected: bool,
    pub monitors: Vec<Monitor>,
}

impl Model {
    pub(crate) fn none() -> Model {
        Model { provider: Provider::None, connected: false, monitors: Vec::new() }
    }

    /// The service value (`SERVICE_SCHEMAS` shape).
    pub(crate) fn to_value(&self) -> Value {
        let monitors = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let workspaces = m
                    .workspaces
                    .iter()
                    .map(|w| {
                        map([
                            ("name", text(&w.name)),
                            ("display_name", text(&w.display_name)),
                            ("focused", Value::Bool(w.focused)),
                            ("visible", Value::Bool(w.visible)),
                            ("has_windows", Value::Bool(w.window_count > 0)),
                            ("window_count", Value::Int(i64::from(w.window_count))),
                        ])
                    })
                    .collect();
                map([
                    ("index", Value::Int(i as i64)),
                    ("name", text(&m.name)),
                    ("handle", m.handle.map_or(Value::Nil, Value::Int)),
                    ("device", m.device.as_deref().map_or(Value::Nil, text)),
                    ("workspaces", Value::List(workspaces)),
                ])
            })
            .collect();
        map([
            ("provider", text(self.provider.name())),
            ("connected", Value::Bool(self.connected)),
            ("monitors", Value::List(monitors)),
        ])
    }

    /// The workspace to switch to for next (`delta` 1) or previous (-1) on
    /// monitor `monitor` (the focused one when None), wrapping around.
    pub(crate) fn cycle_target(&self, monitor: Option<u32>, delta: i32) -> Option<String> {
        let m = match monitor {
            Some(i) => self.monitors.get(i as usize)?,
            None => self
                .monitors
                .iter()
                .find(|m| m.workspaces.iter().any(|w| w.focused))
                .or_else(|| self.monitors.first())?,
        };
        let n = m.workspaces.len() as i64;
        if n == 0 {
            return None;
        }
        // The shown workspace of that monitor is the starting point (the focused one on the focused monitor).
        let at =
            m.workspaces.iter().position(|w| w.focused).or_else(|| m.workspaces.iter().position(|w| w.visible))? as i64;
        let to = (at + i64::from(delta)).rem_euclid(n) as usize;
        Some(m.workspaces[to].name.clone())
    }
}

fn clean_name(s: &str) -> String {
    cut_chars(s, MAX_WORKSPACE_NAME)
}

fn str_of(v: &Json, key: &str) -> Option<String> {
    v.get(key).and_then(Json::as_str).map(str::to_string)
}

fn count_windows(node: &Json, depth: usize) -> u32 {
    if depth > 32 {
        return 0;
    }
    let mut n = 0;
    if let Some(children) = node.get("children").and_then(Json::as_array) {
        for c in children {
            if c.get("type").and_then(Json::as_str) == Some("window") {
                n += 1;
            } else {
                n += count_windows(c, depth + 1);
            }
        }
    }
    n
}

/// GlazeWM-compatible `query monitors` data: `{"monitors":[...]}`.
pub(crate) fn from_glaze(data: &Json) -> Model {
    let mut monitors = Vec::new();
    for (i, m) in data.get("monitors").and_then(Json::as_array).into_iter().flatten().take(MAX_MONITORS).enumerate() {
        let mut workspaces = Vec::new();
        for w in m.get("children").and_then(Json::as_array).into_iter().flatten() {
            if w.get("type").and_then(Json::as_str) != Some("workspace") {
                continue;
            }
            if workspaces.len() >= MAX_WORKSPACES_PER_MONITOR {
                break;
            }
            let name = clean_name(&str_of(w, "name").unwrap_or_default());
            let display = str_of(w, "displayName")
                .filter(|d| !d.is_empty())
                .map(|d| clean_name(&d))
                .unwrap_or_else(|| name.clone());
            workspaces.push(Workspace {
                display_name: display,
                name,
                focused: w.get("hasFocus").and_then(Json::as_bool).unwrap_or(false),
                visible: w.get("isDisplayed").and_then(Json::as_bool).unwrap_or(false),
                window_count: count_windows(w, 0),
            });
        }
        monitors.push(Monitor {
            name: str_of(m, "hardwareId")
                .filter(|s| !s.is_empty())
                .map(|s| clean_name(&s))
                .unwrap_or_else(|| format!("Monitor {}", i + 1)),
            handle: m.get("handle").and_then(Json::as_i64),
            device: None,
            workspaces,
        });
    }
    Model { provider: Provider::Glaze, connected: true, monitors }
}

/// The `elements` array of a komorebi ring (`{"elements":[...],"focused":n}`).
fn elems(v: Option<&Json>) -> &[Json] {
    v.and_then(|x| x.get("elements")).and_then(Json::as_array).map_or(&[], Vec::as_slice)
}

fn komorebi_windows(w: &Json) -> u32 {
    let in_containers: usize = elems(w.get("containers")).iter().map(|c| elems(c.get("windows")).len()).sum();
    let monocle = w.get("monocle_container").filter(|m| !m.is_null()).map_or(0, |m| elems(m.get("windows")).len());
    let maximized = usize::from(w.get("maximized_window").is_some_and(|m| !m.is_null()));
    let floating = elems(w.get("floating_windows")).len();
    (in_containers + monocle + maximized + floating).min(u32::MAX as usize) as u32
}

/// komorebi's `state` object of a notification. Missing or unknown fields
/// are tolerated: versions differ.
pub(crate) fn from_komorebi(state: &Json) -> Model {
    let monitors_json = state.get("monitors");
    let focused_monitor = monitors_json.and_then(|m| m.get("focused")).and_then(Json::as_u64).unwrap_or(0) as usize;
    let mut monitors = Vec::new();
    for (i, m) in elems(monitors_json).iter().take(MAX_MONITORS).enumerate() {
        let wss = m.get("workspaces");
        let shown = wss.and_then(|w| w.get("focused")).and_then(Json::as_u64).unwrap_or(0) as usize;
        let mut workspaces = Vec::new();
        for (j, w) in elems(wss).iter().take(MAX_WORKSPACES_PER_MONITOR).enumerate() {
            let name = str_of(w, "name")
                .filter(|n| !n.is_empty())
                .map(|n| clean_name(&n))
                .unwrap_or_else(|| (j + 1).to_string());
            workspaces.push(Workspace {
                display_name: name.clone(),
                name,
                visible: j == shown,
                focused: j == shown && i == focused_monitor,
                window_count: komorebi_windows(w),
            });
        }
        let name = str_of(m, "name")
            .filter(|s| !s.is_empty())
            .map(|s| clean_name(&s))
            .unwrap_or_else(|| format!("Monitor {}", i + 1));
        let device = name.starts_with("DISPLAY").then(|| format!("\\\\.\\{name}"));
        monitors.push(Monitor { name, handle: None, device, workspaces });
    }
    Model { provider: Provider::Komorebi, connected: true, monitors }
}

#[cfg(test)]
mod tests {
    use rsb_core::services::{ServiceId, check_value};
    use serde_json::json;

    use super::*;

    fn glaze_sample() -> Json {
        json!({"monitors":[
            {"type":"monitor","handle":65537,"hardwareId":"MON-A","children":[
                {"type":"workspace","id":"1","name":"1","displayName":null,"isDisplayed":true,"hasFocus":true,"children":[
                    {"type":"window","handle":1},
                    {"type":"split","children":[{"type":"window","handle":2},{"type":"split","children":[{"type":"window","handle":3}]}]}
                ]},
                {"type":"workspace","name":"2","displayName":"Two","isDisplayed":false,"hasFocus":false,"children":[]},
                {"type":"not-a-workspace","name":"x"}
            ]},
            {"type":"monitor","handle":65539,"children":[
                {"type":"workspace","name":"3","displayName":"","isDisplayed":true,"hasFocus":false,"children":[{"type":"window"}]}
            ]}
        ]})
    }

    #[test]
    fn glaze_state_maps_to_the_model() {
        let m = from_glaze(&glaze_sample());
        assert_eq!(m.provider, Provider::Glaze);
        assert_eq!(m.monitors.len(), 2);
        assert_eq!(m.monitors[0].name, "MON-A");
        assert_eq!(m.monitors[0].handle, Some(65537));
        assert_eq!(m.monitors[1].name, "Monitor 2");
        let ws = &m.monitors[0].workspaces;
        assert_eq!(ws.len(), 2, "non-workspace children are skipped");
        assert_eq!(
            (ws[0].window_count, ws[0].focused, ws[0].visible),
            (3, true, true),
            "windows counted through nested containers"
        );
        assert_eq!(ws[0].display_name, "1", "a null displayName falls back to the name");
        assert_eq!(ws[1].display_name, "Two");
        assert_eq!(m.monitors[1].workspaces[0].display_name, "3", "an empty displayName falls back to the name");
        assert_eq!(check_value(ServiceId::Workspaces, &m.to_value()), Ok(()));
    }

    #[test]
    fn glaze_caps_hold() {
        let many_ws: Vec<Json> = (0..100)
            .map(|i| json!({"type":"workspace","name":"w".repeat(100) + &i.to_string(),"children":[]}))
            .collect();
        let monitors: Vec<Json> = (0..40).map(|_| json!({"type":"monitor","children": many_ws})).collect();
        let m = from_glaze(&json!({"monitors": monitors}));
        assert_eq!(m.monitors.len(), crate::MAX_MONITORS);
        assert_eq!(m.monitors[0].workspaces.len(), crate::MAX_WORKSPACES_PER_MONITOR);
        assert_eq!(m.monitors[0].workspaces[0].name.chars().count(), crate::MAX_WORKSPACE_NAME);
        assert_eq!(check_value(ServiceId::Workspaces, &m.to_value()), Ok(()));
    }

    #[test]
    fn deeply_nested_windows_are_cut_off() {
        let mut node = json!({"type":"window"});
        for _ in 0..200 {
            node = json!({"type":"split","children":[node]});
        }
        let m = from_glaze(&json!({"monitors":[{"children":[{"type":"workspace","name":"1","children":[node]}]}]}));
        assert_eq!(
            m.monitors[0].workspaces[0].window_count, 0,
            "beyond depth 32 nothing is counted, and nothing overflows the stack"
        );
    }

    fn komorebi_sample() -> Json {
        json!({"monitors":{"focused":1,"elements":[
            {"name":"DISPLAY1","device":"AAA","device_id":"AAA-1","workspaces":{"focused":0,"elements":[
                {"name":"main","containers":{"elements":[{"windows":{"elements":[{},{}]}},{"windows":{"elements":[{}]}}]},
                 "monocle_container":null,"maximized_window":null,"floating_windows":{"elements":[{}]}},
                {"name":null,"containers":{"elements":[]},"monocle_container":{"windows":{"elements":[{}]}},"maximized_window":{"hwnd":1},"floating_windows":{"elements":[]}}
            ]}},
            {"name":"DISPLAY2","workspaces":{"focused":1,"elements":[
                {"name":"a","containers":{"elements":[]}},
                {"name":"b","containers":{"elements":[]}}
            ]}}
        ]}})
    }

    #[test]
    fn komorebi_state_maps_to_the_model() {
        let m = from_komorebi(&komorebi_sample());
        assert_eq!(m.provider, Provider::Komorebi);
        assert_eq!(m.monitors.len(), 2);
        assert_eq!(m.monitors[0].device.as_deref(), Some("\\\\.\\DISPLAY1"));
        let a = &m.monitors[0].workspaces;
        assert_eq!(a[0].name, "main");
        assert_eq!(a[0].window_count, 4, "two containers' windows plus one floating");
        assert_eq!(a[1].name, "2", "a null name becomes its 1-based position");
        assert_eq!(a[1].window_count, 2, "monocle container plus maximized window");
        assert!(a[0].visible && !a[1].visible);
        assert!(!a[0].focused, "the shown workspace of an unfocused monitor is visible, not focused");
        let b = &m.monitors[1].workspaces;
        assert!(b[1].visible && b[1].focused, "monitor 1 is the focused monitor, its workspace 1 the shown one");
        assert_eq!(check_value(ServiceId::Workspaces, &m.to_value()), Ok(()));
    }

    #[test]
    fn komorebi_tolerates_missing_fields() {
        let m = from_komorebi(&json!({}));
        assert!(m.monitors.is_empty());
        let m = from_komorebi(&json!({"monitors":{"elements":[{"workspaces":{"elements":[{}]}}]}}));
        assert_eq!(m.monitors[0].workspaces[0].name, "1");
        assert_eq!(check_value(ServiceId::Workspaces, &m.to_value()), Ok(()));
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let m = from_glaze(&glaze_sample());
        assert_eq!(m.cycle_target(None, 1).as_deref(), Some("2"));
        assert_eq!(m.cycle_target(None, -1).as_deref(), Some("2"), "two workspaces: previous wraps to the other");
        assert_eq!(m.cycle_target(Some(1), 1).as_deref(), Some("3"), "a one-workspace monitor wraps to itself");
        assert_eq!(m.cycle_target(Some(5), 1), None, "an unknown monitor has no target");
        assert_eq!(Model::none().cycle_target(None, 1), None);
    }
}
