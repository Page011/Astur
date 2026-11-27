//! What the UI thread copies for `rsb inspect`: each bar's frame cost and
//! root values, and the node tree of the widgets asked for. Only this runs on
//! the UI thread; it holds no lock, makes no call that waits and copies at
//! most `MAX_NODES` nodes per widget instance.

use std::time::Instant;

use rsb_core::config::WidgetRef;
use rsb_core::element::InstanceId;
use rsb_core::scene::{InspectNode, InstanceCost, InstanceNodes, Scene};

use super::InspectQuery;

/// Nodes the UI thread copies for one widget instance; the rest are
/// counted, not copied. 600 fill a reply of 64 KB without styles.
pub const MAX_NODES: usize = 600;

/// The custom properties a preset's `tune` list names when the caller has
/// none to give.
pub const DEFAULT_ROOT_VARS: [&str; 3] = ["accent", "fg", "bg"];

/// One bar as the UI thread holds it.
pub struct BarView<'a> {
    pub name: &'a str,
    pub monitor: u32,
    pub scale: f32,
    pub scene: &'a Scene,
    /// Every widget instance on the bar with its ref as written.
    pub instances: Vec<(InstanceId, &'a WidgetRef)>,
}

/// An open popup, menu or tooltip surface: the scene that draws it.
pub struct PopupView<'a> {
    pub instance: InstanceId,
    pub name: &'a str,
    pub scene: &'a Scene,
}

/// The UI thread's answer, owned so it can cross to the pipe thread.
#[derive(Clone, Debug, Default)]
pub struct UiPart {
    pub bars: Vec<UiBar>,
}

#[derive(Clone, Debug)]
pub struct UiBar {
    pub name: String,
    pub monitor: u32,
    pub scale: f32,
    /// `dark` or `light`.
    pub scheme: &'static str,
    /// Nodes on the bar, the root included.
    pub nodes: usize,
    pub frames: u64,
    pub style_us: u64,
    pub layout_us: u64,
    pub paint_us: u64,
    pub frame_us_max: u32,
    pub no_tree_frames: u64,
    pub last_frame_ms_ago: Option<u64>,
    pub us_last_60s: u64,
    /// The custom properties asked for on the bar's root, as written there.
    pub root_vars: Vec<(String, Option<String>)>,
    /// The bar's own root node, with its style; only when asked for.
    pub root: Option<InspectNode>,
    pub instances: Vec<UiInstance>,
}

#[derive(Clone, Debug)]
pub struct UiInstance {
    pub id: InstanceId,
    pub widget: String,
    pub cost: InstanceCost,
    /// Nodes the widget has on the bar.
    pub nodes: usize,
    /// A newer tree or style waits for the next frame.
    pub pending: bool,
    /// Its tree; only for the widgets the query names.
    pub tree: Option<UiTree>,
}

/// One widget's copied nodes, with the content of each open popup it declares
/// (parallel to `nodes.popups`).
#[derive(Clone, Debug)]
pub struct UiTree {
    pub nodes: InstanceNodes,
    pub open: Vec<Option<InstanceNodes>>,
}

/// Copies what `query` asks for. `vars` names the custom properties to read
/// on each bar's root (without the leading `--`); `now` is the time the
/// frame ages are measured to.
pub fn ui_part(
    query: &InspectQuery,
    bars: &[BarView<'_>],
    popups: &[PopupView<'_>],
    vars: &[&str],
    now: Instant,
) -> UiPart {
    let mut out = Vec::new();
    for bar in bars {
        if query.bar.as_deref().is_some_and(|b| b != bar.name) {
            continue;
        }
        let t = bar.scene.frame_totals();
        let root_vars = vars.iter().map(|v| ((*v).to_string(), bar.scene.root_var(v))).collect();
        let instances = bar
            .instances
            .iter()
            .map(|(id, widget)| {
                let wanted = query.widget.as_ref().is_some_and(|w| w == *widget);
                let tree = wanted.then(|| {
                    let nodes = bar.scene.nodes_of(*id, MAX_NODES, query.depth, query.style);
                    let open = nodes
                        .popups
                        .iter()
                        .map(|p| {
                            if !p.open {
                                return None;
                            }
                            popups
                                .iter()
                                .find(|v| v.instance == *id && v.name == p.name)
                                .map(|v| v.scene.popup_nodes(MAX_NODES, query.depth, query.style))
                        })
                        .collect();
                    UiTree { nodes, open }
                });
                UiInstance {
                    id: *id,
                    widget: widget.to_string(),
                    cost: bar.scene.instance_cost(*id),
                    nodes: bar.scene.nodes_of(*id, 0, 1, false).total,
                    pending: bar.scene.pending(),
                    tree,
                }
            })
            .collect();
        out.push(UiBar {
            name: bar.name.to_string(),
            monitor: bar.monitor,
            scale: bar.scale,
            scheme: if bar.scene.dark() { "dark" } else { "light" },
            nodes: bar.scene.node_count(),
            frames: t.frames,
            style_us: t.style_us,
            layout_us: t.layout_us,
            paint_us: t.paint_us,
            frame_us_max: t.frame_us_max,
            no_tree_frames: t.no_tree_frames,
            last_frame_ms_ago: t.last_frame_at.map(|at| now.saturating_duration_since(at).as_millis() as u64),
            us_last_60s: bar.scene.frame_us_last_60s(now),
            root_vars,
            root: (query.style && query.widget.is_none() && query.bar.is_some())
                .then(|| bar.scene.root_node(true))
                .flatten(),
            instances,
        });
    }
    UiPart { bars: out }
}
