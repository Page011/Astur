//! Timing checks for the inspector's share of the UI thread. Release only
//! (`cargo test --release -p rsb-core --test inspect_micro -- --ignored
//! --nocapture`); each prints its numbers. A debug build is many times slower
//! and proves nothing here.

mod common;

use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::scene::Scene;

const BASE: &str = "bar { font-size: 10px }";

fn session() -> &'static str {
    // SM_REMOTESESSION is not available to a plain test; the environment says it.
    if std::env::var("SESSIONNAME").is_ok_and(|s| s.starts_with("RDP")) { "rdp" } else { "console" }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1]
}

/// A widget of about `n` nodes: rows of three texts.
fn widget_of(n: usize) -> rsb_core::element::Element {
    let rows = n.saturating_sub(1) / 4;
    boxed((0..rows).map(|i| boxed(vec![text(&format!("a{i}")), text("b"), text("c")])).collect())
}

fn built_frames(s: &mut Scene, t0: Instant, frames: u32, version: &mut u64) -> Duration {
    let started = Instant::now();
    for i in 0..frames {
        *version += 1;
        // A tree that differs each time, so every frame is a built one.
        let mut root = widget_of(60);
        root.children[0].children[0] = text(&format!("n{}", *version));
        s.set_tree(tree(1, *version, Some(root)));
        s.frame(t0 + Duration::from_millis(u64::from(i)), &mut rsb_core::layout::FixedMeasure);
    }
    started.elapsed()
}

#[test]
#[ignore = "timing: run in release"]
fn inspect_accounting_micro() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 1200, 1.0, Some(widget_of(60)), None, None);
    frame(&mut s, t0);
    let nodes = s.nodes_of(InstanceId(1), 1000, 64, false).total;

    // 20,000 built frames of a 60-node widget, with the accounting on and off, in
    // alternating blocks so a slow minute of the PC lands on both.
    let (mut on, mut off) = (Vec::new(), Vec::new());
    let mut version = 1;
    for round in 0..10 {
        for arm in [round % 2 == 0, round % 2 != 0] {
            s.set_accounting(arm);
            let d = built_frames(&mut s, t0, 1000, &mut version);
            if arm { on.push(d) } else { off.push(d) }
        }
    }
    on.sort();
    off.sort();
    let per_frame = |d: Duration| d.as_secs_f64() * 1e6 / 1000.0;
    let (m_on, m_off) = (per_frame(on[on.len() / 2]), per_frame(off[off.len() / 2]));

    // The step on its own, 20,000 times.
    s.set_accounting(true);
    let probe = s.accounting_probe(InstanceId(1), 20_000, t0 + Duration::from_secs(5));
    let step_us = probe.as_secs_f64() * 1e6 / 20_000.0;
    println!(
        "inspect_accounting_micro ({}): {nodes} nodes; whole built frame median on {m_on:.1} us, off {m_off:.1} us \
         (difference {:.2} us, inside the noise of timing whole frames); accounting step alone {step_us:.3} us per frame over 20000",
        session(),
        m_on - m_off,
    );
    assert!(step_us <= 0.2, "the accounting step costs {step_us:.3} us per frame; the budget is 0.2");
}

#[test]
#[ignore = "timing: run in release"]
fn inspect_copy_time_2000_nodes() {
    let t0 = Instant::now();
    let mut s = scene_with(BASE, 1200, 1.0, Some(widget_of(2000)), None, None);
    frame(&mut s, t0);
    let mut times = Vec::new();
    for with_style in [false, true] {
        times.clear();
        let mut total = 0;
        let mut copied = 0;
        for _ in 0..300 {
            let t = Instant::now();
            let c = s.nodes_of(InstanceId(1), 600, 64, with_style);
            times.push(t.elapsed());
            total = c.total;
            copied = c.nodes.len();
            std::hint::black_box(&c);
        }
        times.sort();
        let (p50, p95, max) = (percentile(&times, 0.5), percentile(&times, 0.95), *times.last().unwrap());
        println!(
            "inspect_copy_time_2000_nodes ({}): style {with_style}, {copied} of {total} nodes copied, p50 {p50:?}, p95 {p95:?}, max {max:?}",
            session()
        );
        assert!(p95 < Duration::from_millis(1), "p95 {p95:?}");
    }
}
