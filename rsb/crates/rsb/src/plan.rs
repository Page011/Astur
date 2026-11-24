//! From a `Config` and the monitors to what runs: which bar windows exist,
//! and which widget instances (with stable ids) fill their zones. Pure, so
//! it is unit-tested without windows.

use std::collections::{HashMap, HashSet};

use rsb_core::config::{BarSpec, Config, MonitorSelector, WidgetOptions, ZoneItem};
use rsb_core::diag::{DiagKind, Diagnostic};
use rsb_core::element::InstanceId;
use rsb_core::msg::{InstanceSpec, MonitorRef};
use rsb_core::scene::{SlotSpec, ZoneSlots};
use rsb_win::MonitorInfo;

/// One bar on one monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct BarPlan {
    pub spec: BarSpec,
    pub monitor: MonitorInfo,
}

impl BarPlan {
    /// Identity of the window: the same bar name on the same monitor.
    pub fn key(&self) -> String {
        format!("{}|{}", self.spec.name, self.monitor.device)
    }
}

fn selector_matches(sel: &MonitorSelector, m: &MonitorInfo) -> bool {
    match sel {
        MonitorSelector::All => true,
        MonitorSelector::Primary => m.primary,
        MonitorSelector::Index(i) => m.index == *i,
        MonitorSelector::Device(d) => m.device.eq_ignore_ascii_case(d),
    }
}

/// Every bar of the config on every monitor its selectors match, in config
/// order, plus one warning per bar that matches none (that bar is not shown).
pub fn plan_bars(config: &Config, monitors: &[MonitorInfo]) -> (Vec<BarPlan>, Vec<Diagnostic>) {
    let mut plans = Vec::new();
    let mut diags = Vec::new();
    for bar in &config.bars {
        let before = plans.len();
        for m in monitors {
            if bar.monitors.iter().any(|s| selector_matches(s, m)) {
                plans.push(BarPlan { spec: bar.clone(), monitor: m.clone() });
            }
        }
        if plans.len() == before {
            diags.push(Diagnostic::warning(
                DiagKind::Config,
                bar.pos.clone(),
                format!("no monitor matches the 'monitors' of bar '{}'; it is not shown", bar.name),
            ));
        }
    }
    (plans, diags)
}

/// Hands out instance ids: the same key (bar, monitor, widget ref, n-th
/// time that ref appears in the bar) keeps its id across config reloads, so
/// moving a widget between zones or editing an unrelated line does not
/// restart it. Ids are never reused.
#[derive(Debug, Default)]
pub struct InstanceTable {
    ids: HashMap<String, InstanceId>,
    next: u32,
}

impl InstanceTable {
    pub fn new() -> InstanceTable {
        InstanceTable { ids: HashMap::new(), next: 1 }
    }

    fn id_for(&mut self, key: &str) -> InstanceId {
        if let Some(id) = self.ids.get(key) {
            return *id;
        }
        let id = InstanceId(self.next);
        self.next += 1;
        self.ids.insert(key.to_string(), id);
        id
    }

    fn retain(&mut self, keep: &HashSet<String>) {
        self.ids.retain(|k, _| keep.contains(k));
    }
}

/// One widget in one bar.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub spec: InstanceSpec,
    pub slot: SlotSpec,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BarInstances {
    pub left: Vec<Placed>,
    pub center: Vec<Placed>,
    pub right: Vec<Placed>,
}

impl BarInstances {
    pub fn slots(&self) -> ZoneSlots {
        let of = |z: &[Placed]| z.iter().map(|p| p.slot.clone()).collect();
        ZoneSlots { left: of(&self.left), center: of(&self.center), right: of(&self.right) }
    }

    pub fn all(&self) -> impl Iterator<Item = &Placed> {
        self.left.iter().chain(&self.center).chain(&self.right)
    }
}

/// The plain `[widget.clock]` table first, then the instance's own
/// `[widget."clock#utc"]`, key by key.
pub fn options_for(config: &Config, item: &ZoneItem) -> WidgetOptions {
    let mut out = WidgetOptions::default();
    let plain = config.widgets.get(&item.widget.name);
    let own = item.widget.instance.as_ref().and_then(|_| config.widgets.get(&item.widget.to_string()));
    for table in [plain, own].into_iter().flatten() {
        for (k, v) in &table.values {
            out.values.insert(k.clone(), v.clone());
        }
    }
    out
}

/// Instances for every planned bar. `scales[i]` is bar i's DPI scale.
pub fn build_instances(
    config: &Config,
    plans: &[BarPlan],
    scales: &[f32],
    table: &mut InstanceTable,
) -> Vec<BarInstances> {
    let mut keys = HashSet::new();
    let mut out = Vec::new();
    for (i, plan) in plans.iter().enumerate() {
        let mut seen: HashMap<String, u32> = HashMap::new();
        let monitor = MonitorRef {
            index: plan.monitor.index,
            handle: plan.monitor.handle,
            device: plan.monitor.device.clone(),
            primary: plan.monitor.primary,
            scale: scales.get(i).copied().unwrap_or(1.0),
        };
        let mut zone = |items: &[ZoneItem]| -> Vec<Placed> {
            items
                .iter()
                .map(|item| {
                    let text = item.widget.to_string();
                    let n = seen.entry(text.clone()).and_modify(|n| *n += 1).or_insert(1);
                    let key = format!("{}|{}|{text}|{n}", plan.spec.name, plan.monitor.device);
                    keys.insert(key.clone());
                    let id = table.id_for(&key);
                    Placed {
                        spec: InstanceSpec {
                            instance: id,
                            widget: item.widget.clone(),
                            options: options_for(config, item),
                            bar: plan.spec.name.clone(),
                            monitor: monitor.clone(),
                            pos: item.pos.clone(),
                        },
                        slot: SlotSpec { instance: id, class: item.widget.css_class(), id: item.widget.css_id() },
                    }
                })
                .collect()
        };
        out.push(BarInstances {
            left: zone(&plan.spec.left),
            center: zone(&plan.spec.center),
            right: zone(&plan.spec.right),
        });
    }
    table.retain(&keys);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsb_core::config::{default_config, load_config};
    use rsb_core::geom::RectPx;

    fn monitor(index: u32, device: &str, primary: bool) -> MonitorInfo {
        MonitorInfo {
            index,
            handle: 100 + u64::from(index),
            device: device.to_string(),
            primary,
            rect: RectPx::new(0, 0, 1920, 1080),
            work: RectPx::new(0, 0, 1920, 1080),
            dpi: 96,
        }
    }

    fn two() -> Vec<MonitorInfo> {
        vec![monitor(0, "\\\\.\\DISPLAY1", true), monitor(1, "\\\\.\\DISPLAY2", false)]
    }

    fn config(text: &str) -> Config {
        let load = load_config(Some((text, "bar.toml")), &|n| crate::assets::preset_toml(n).map(str::to_string));
        assert!(load.diagnostics.is_empty(), "{:?}", load.diagnostics);
        load.config.unwrap()
    }

    #[test]
    fn selectors_pick_monitors_and_a_miss_is_a_warning() {
        let cfg = config(
            "[[bar]]\nname = \"a\"\nmonitors = \"*\"\n[[bar]]\nname = \"b\"\nmonitors = \"primary\"\n\
             [[bar]]\nname = \"c\"\nmonitors = \"DISPLAY2\"\n[[bar]]\nname = \"d\"\nmonitors = [1, 5]\n\
             [[bar]]\nname = \"e\"\nmonitors = 7\n",
        );
        let (plans, diags) = plan_bars(&cfg, &two());
        let keys: Vec<String> = plans.iter().map(BarPlan::key).collect();
        assert_eq!(
            keys,
            ["a|\\\\.\\DISPLAY1", "a|\\\\.\\DISPLAY2", "b|\\\\.\\DISPLAY1", "c|\\\\.\\DISPLAY2", "d|\\\\.\\DISPLAY2"]
        );
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("bar 'e'") && diags[0].message.contains("not shown"));
        assert!(diags[0].pos.as_ref().is_some_and(|p| p.line > 1), "the warning points into bar.toml");
    }

    #[test]
    fn the_default_config_gives_one_bar_per_monitor() {
        let (plans, diags) = plan_bars(&default_config(), &two());
        assert_eq!((plans.len(), diags.len()), (2, 0));
    }

    #[test]
    fn ids_survive_a_zone_move_and_are_never_reused() {
        let mut table = InstanceTable::new();
        let cfg = config("[[bar]]\nleft = [\"clock\", \"cpu\"]\nright = [\"memory\"]\ncenter = []\n");
        let (plans, _) = plan_bars(&cfg, &two()[..1]);
        let a = build_instances(&cfg, &plans, &[1.0], &mut table);
        let ids: Vec<u32> = a[0].all().map(|p| p.spec.instance.0).collect();
        assert_eq!(ids, [1, 2, 3]);
        // Move the clock to the right: same ids, new zones.
        let cfg = config("[[bar]]\nleft = [\"cpu\"]\nright = [\"memory\", \"clock\"]\ncenter = []\n");
        let (plans, _) = plan_bars(&cfg, &two()[..1]);
        let b = build_instances(&cfg, &plans, &[1.0], &mut table);
        assert_eq!(b[0].left[0].spec.instance, InstanceId(2));
        assert_eq!(b[0].right[1].spec.instance, InstanceId(1));
        // Remove the clock, add it back: a fresh id.
        let cfg = config("[[bar]]\nleft = [\"cpu\"]\nright = [\"memory\"]\ncenter = []\n");
        let (plans, _) = plan_bars(&cfg, &two()[..1]);
        build_instances(&cfg, &plans, &[1.0], &mut table);
        let cfg = config("[[bar]]\nleft = [\"cpu\", \"clock\"]\nright = [\"memory\"]\ncenter = []\n");
        let (plans, _) = plan_bars(&cfg, &two()[..1]);
        let c = build_instances(&cfg, &plans, &[1.0], &mut table);
        assert_eq!(c[0].left[1].spec.instance, InstanceId(4));
    }

    #[test]
    fn the_same_widget_twice_gets_two_instances_with_merged_options() {
        let mut table = InstanceTable::new();
        let cfg = config(
            "[[bar]]\nleft = [\"clock\", \"clock#utc\"]\nright = []\ncenter = []\n\
             [widget.clock]\nformat = \"HH:mm\"\nalt_format = \"x\"\n[widget.\"clock#utc\"]\nformat = \"HH:mm 'UTC'\"\n",
        );
        let (plans, _) = plan_bars(&cfg, &two()[..1]);
        let out = build_instances(&cfg, &plans, &[1.25], &mut table);
        let [plain, utc] = &out[0].left[..] else { panic!("two instances") };
        assert_ne!(plain.spec.instance, utc.spec.instance);
        assert_eq!(plain.spec.options.values["format"].value.as_str(), Some("HH:mm"));
        assert_eq!(utc.spec.options.values["format"].value.as_str(), Some("HH:mm 'UTC'"));
        assert_eq!(utc.spec.options.values["alt_format"].value.as_str(), Some("x"), "the plain table fills the gaps");
        assert_eq!(utc.spec.monitor.scale, 1.25);
        assert_eq!((utc.slot.class.as_str(), utc.slot.id.as_str()), ("clock", "clock-utc"));
        assert!(utc.spec.pos.is_some(), "positions are carried for 'unknown widget' messages");
    }

    #[test]
    fn two_bars_on_one_monitor_have_separate_instances() {
        let mut table = InstanceTable::new();
        let cfg = config(
            "[[bar]]\nname = \"top\"\nposition = \"top\"\nleft = [\"clock\"]\nright = []\ncenter = []\n\
             [[bar]]\nname = \"bottom\"\nposition = \"bottom\"\nleft = [\"clock\"]\nright = []\ncenter = []\n",
        );
        let (plans, _) = plan_bars(&cfg, &two()[..1]);
        let out = build_instances(&cfg, &plans, &[1.0, 1.0], &mut table);
        assert_eq!(out.len(), 2);
        assert_ne!(out[0].left[0].spec.instance, out[1].left[0].spec.instance);
        assert_eq!(out[1].left[0].spec.bar, "bottom");
    }
}
