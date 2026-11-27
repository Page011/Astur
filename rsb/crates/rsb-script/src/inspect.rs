//! What `rsb inspect` reads from the script runtime: how often and how long
//! a widget's script ran, and what its reactive scope holds. Every type
//! turns itself into the JSON shape of the reply with `to_json`, so the
//! binary needs no serde.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use rsb_core::config::WidgetOptions;
use rsb_core::element::InstanceId;
use rsb_core::value::Value as Lv;
use serde_json::{Map, Value, json};

use crate::shared::Inst;

/// Scopes the reactive walk looks at before it stops and says so.
pub const WALK_LIMIT: usize = 10_000;

/// What one widget's scope tree holds, counted when asked for. A walk that
/// hits its node limit says so with `walk_cut`, and the counts then read
/// "at least".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadInfo {
    /// `service.field` names the widget reads, e.g. `clock.time`.
    pub service_fields: Vec<String>,
    pub signals: u32,
    pub computeds: u32,
    pub effects: u32,
    pub timers: u32,
    pub listeners: u32,
    pub tasks: u32,
    pub walk_cut: bool,
}

impl ReadInfo {
    pub fn to_json(&self) -> Value {
        json!({
            "service_fields": self.service_fields,
            "signals": self.signals,
            "computeds": self.computeds,
            "effects": self.effects,
            "timers": self.timers,
            "listeners": self.listeners,
            "tasks": self.tasks,
            "walk_cut": self.walk_cut,
        })
    }
}

/// Script time of one widget instance, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScriptTimes {
    /// Since the widget started.
    pub ms: f64,
    /// The longest single call.
    pub ms_max: f64,
    pub calls: u64,
    /// In the last second.
    pub ms_last_s: f64,
    /// In the last 60 seconds.
    pub ms_last_60s: f64,
}

impl ScriptTimes {
    pub fn to_json(&self) -> Value {
        json!({
            "ms": self.ms,
            "ms_max": self.ms_max,
            "calls": self.calls,
            "ms_last_s": self.ms_last_s,
            "ms_last_60s": self.ms_last_60s,
        })
    }
}

/// Everything the script thread knows about one widget instance for
/// `rsb inspect`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InspectInfo {
    pub script: ScriptTimes,
    /// Trees the instance has sent since it loaded.
    pub trees: u64,
    /// None until the first tree or run.
    pub last_tree_ms_ago: Option<u64>,
    pub last_run_ms_ago: Option<u64>,
    pub reads: ReadInfo,
    /// The options the widget runs with, as a JSON object of
    /// `{value, source, at}` by name.
    pub options: Value,
    /// Names of the services the instance subscribes to, sorted.
    pub subscriptions: Vec<String>,
}

impl InspectInfo {
    /// The fields of the widget entry that come from the script thread:
    /// `script`, `trees`, `last_tree_ms_ago`, `last_run_ms_ago`, `reads`,
    /// `options`, `subscriptions`.
    pub fn to_json(&self) -> Value {
        json!({
            "script": self.script.to_json(),
            "trees": self.trees,
            "last_tree_ms_ago": self.last_tree_ms_ago,
            "last_run_ms_ago": self.last_run_ms_ago,
            "reads": self.reads.to_json(),
            "options": self.options,
            "subscriptions": self.subscriptions,
        })
    }
}

/// When each widget instance last ran a script and last sent a tree, and
/// how much script time it used in each of the last 60 seconds. Every time
/// comes from the clock reads the runtime already makes: one per turn (the
/// start of the turn) and one per tree flush.
#[derive(Default)]
pub(crate) struct Activity {
    base: Option<Instant>,
    turn_at: Option<Instant>,
    per: HashMap<InstanceId, InstActivity>,
}

#[derive(Default)]
struct InstActivity {
    tree_at: Option<Instant>,
    run_at: Option<Instant>,
    /// The instance's total script time when the last turn ended.
    seen: Duration,
    /// One slot per second: (second number + 1, nanoseconds of script time in it).
    ring: Option<Box<[(u64, u64); 60]>>,
    /// The option defaults and the names of the options that have none, as the
    /// widget file declared them.
    declared: Option<(Lv, Vec<String>)>,
}

impl Activity {
    fn second(&self, at: Instant) -> u64 {
        self.base.map_or(0, |b| at.saturating_duration_since(b).as_secs())
    }

    /// A script turn starts at `now`.
    pub(crate) fn begin_turn(&mut self, now: Instant) {
        self.base.get_or_insert(now);
        self.turn_at = Some(now);
    }

    /// The widget file of `id` loaded and declared these option defaults and
    /// optional option names.
    pub(crate) fn declared(&mut self, id: InstanceId, defaults: Lv, optional: Vec<String>) {
        self.per.entry(id).or_default().declared = Some((defaults, optional));
    }

    /// The options `id` runs with: the value, where it came from (`default`,
    /// `user` for the user's `bar.toml`, `preset`) and where it was written.
    pub(crate) fn options(&self, id: InstanceId, set: &WidgetOptions) -> Value {
        let mut out = Map::new();
        let declared = self.per.get(&id).and_then(|a| a.declared.as_ref());
        let defaults = declared.map(|(d, _)| d);
        let optional = declared.map_or(&[][..], |(_, o)| o.as_slice());
        if let Some(Lv::Map(m)) = defaults {
            for (k, v) in m {
                out.insert(k.clone(), json!({"value": v.to_json(), "source": "default", "at": Value::Null}));
            }
        }
        for (k, sp) in &set.values {
            let accepted = match defaults.and_then(|d| d.get(k)) {
                Some(d) => d.type_name() == sp.value.type_name(),
                None => optional.contains(k),
            };
            if accepted {
                let source = if &*sp.pos.file == "bar.toml" { "user" } else { "preset" };
                out.insert(k.clone(), json!({"value": sp.value.to_json(), "source": source, "at": sp.pos.to_string()}));
            }
        }
        Value::Object(out)
    }

    /// A tree of `id` was cut at `now`.
    pub(crate) fn tree_sent(&mut self, id: InstanceId, now: Instant) {
        self.per.entry(id).or_default().tree_at = Some(now);
    }

    /// The turn ended: an instance whose total script time grew ran in it.
    pub(crate) fn end_turn(&mut self, instances: &BTreeMap<InstanceId, Inst>) {
        let Some(now) = self.turn_at else { return };
        let second = self.second(now);
        if self.per.len() > instances.len() {
            // Instances that went away.
            self.per.retain(|id, _| instances.contains_key(id));
        }
        for (id, inst) in instances {
            let total = inst.script_time;
            let known = self.per.get(id).map(|a| a.seen);
            if known.is_none() && total.is_zero() {
                continue;
            }
            let a = self.per.entry(*id).or_default();
            // A restarted instance starts its total again from zero.
            let spent = total.saturating_sub(a.seen);
            a.seen = total;
            if spent.is_zero() {
                continue;
            }
            a.run_at = Some(now);
            let ring = a.ring.get_or_insert_with(|| Box::new([(0, 0); 60]));
            let slot = &mut ring[(second % 60) as usize];
            if slot.0 != second + 1 {
                *slot = (second + 1, 0);
            }
            slot.1 += spent.as_nanos() as u64;
        }
    }

    pub(crate) fn tree_ms_ago(&self, id: InstanceId, now: Instant) -> Option<u64> {
        let at = self.per.get(&id)?.tree_at?;
        Some(now.saturating_duration_since(at).as_millis() as u64)
    }

    pub(crate) fn run_ms_ago(&self, id: InstanceId, now: Instant) -> Option<u64> {
        let at = self.per.get(&id)?.run_at?;
        Some(now.saturating_duration_since(at).as_millis() as u64)
    }

    /// Script time in the last 60 seconds up to `now`.
    pub(crate) fn last_60s(&self, id: InstanceId, now: Instant) -> Duration {
        let (Some(a), second) = (self.per.get(&id), self.second(now)) else { return Duration::ZERO };
        let Some(ring) = &a.ring else { return Duration::ZERO };
        let ns: u64 =
            ring.iter().filter(|(s, _)| *s != 0 && second + 1 >= *s && second + 1 - *s < 60).map(|(_, ns)| ns).sum();
        Duration::from_nanos(ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_json_names_are_the_documented_ones() {
        let info = InspectInfo {
            script: ScriptTimes { ms: 38.2, ms_max: 0.9, calls: 3611, ms_last_s: 0.3, ms_last_60s: 21.0 },
            trees: 1204,
            last_tree_ms_ago: Some(400),
            last_run_ms_ago: None,
            reads: ReadInfo { service_fields: vec!["clock.time".into()], signals: 1, effects: 4, ..Default::default() },
            options: json!({}),
            subscriptions: vec!["clock".into()],
        };
        let json = info.to_json();
        assert_eq!(json["script"]["calls"], 3611);
        assert_eq!(json["script"]["ms_last_60s"], 21.0);
        assert_eq!(json["last_run_ms_ago"], Value::Null);
        assert_eq!(json["reads"]["service_fields"][0], "clock.time");
        assert_eq!(json["reads"]["walk_cut"], false);
        assert_eq!(json["subscriptions"][0], "clock");
        assert!(json["options"].is_object());
        for key in ["signals", "computeds", "effects", "timers", "listeners", "tasks"] {
            assert!(json["reads"][key].is_number(), "{key}");
        }
    }
}
