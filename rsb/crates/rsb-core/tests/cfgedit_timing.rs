//! How long planning a change takes (debug build, printed for the notes).

#[path = "cfgedit_common.rs"]
mod common;

use std::time::Instant;

use common::*;
use rsb_core::contract::Zone;
use serde_json::json;

fn percentile(sorted: &[u128], p: f64) -> u128 {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
fn planning_a_change_takes_milliseconds() {
    let cases: Vec<(&str, Vec<rsb_core::contract::Op>)> = vec![
        ("set preset", vec![set("preset", json!("glass"))]),
        ("set height on the template", vec![set("bar.main.height", json!(40))]),
        ("zone move", vec![mv("status", None, "main", Zone::Left, Some(0))]),
        (
            "four changes",
            vec![
                set("preset", json!("fluent")),
                set("bar.main.height", json!(40)),
                add("clock#utc", "main", Zone::Left, None),
                set("bar.main.floating", json!(true)),
            ],
        ),
    ];
    for (label, ops) in cases {
        let mut us: Vec<u128> = Vec::new();
        for _ in 0..100 {
            let t = Instant::now();
            let p = plan(TEMPLATE_BAR, &ops).unwrap();
            us.push(t.elapsed().as_micros());
            assert!(!p.files.is_empty());
        }
        us.sort_unstable();
        println!(
            "plan_change {label}: p50 {} us, p95 {} us (100 runs, debug build, {})",
            percentile(&us, 0.5),
            percentile(&us, 0.95),
            session()
        );
        assert!(percentile(&us, 0.95) < 50_000, "{label}");
    }
}

fn session() -> &'static str {
    if std::env::var("SESSIONNAME").is_ok_and(|s| s.starts_with("RDP")) {
        "remote desktop session"
    } else {
        "console session"
    }
}
