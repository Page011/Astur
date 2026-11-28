//! Widgets load, draw, react and report: the everyday paths.

mod common;

use common::*;
use rsb_core::element::ElementKind;
use rsb_core::msg::{HubRequest, WidgetSource, WidgetState};
use rsb_core::services::{ServiceEvent, ServiceId, ServiceStatus};
use rsb_core::value::Value;

const HELLO: &str = r#"
--!strict
local rsb = require("@rsb")
return rsb.widget {
    setup = function(props, ctx)
        return rsb.row {
            class = "hello",
            rsb.text "hi",
            rsb.text { text = ctx.id },
        }
    end,
}
"#;

#[test]
fn a_user_widget_draws_its_tree() {
    let mut h = Harness::new();
    h.user("widgets/hello.luau", HELLO);
    h.start(&[(1, "hello")]);
    let root = h.root(1).expect("a tree was sent");
    assert_eq!(root.kind, ElementKind::Box);
    assert_eq!(root.classes.iter().map(|c| c.to_string()).collect::<Vec<_>>(), vec!["hello"]);
    assert_eq!(h.texts(1), vec!["hi", "hello"]);
    assert!(h.plate(1).is_none());
    let status = h.status();
    assert_eq!(status[0].state, WidgetState::Running);
    assert_eq!(status[0].source, WidgetSource::User("widgets/hello.luau".into()));
    assert!(!status[0].shared_vm);
}

#[test]
fn a_builtin_widget_runs_in_the_shared_vm() {
    let mut h = Harness::new();
    h.builtin("widgets/hello.luau", HELLO);
    h.builtin("widgets/other.luau", HELLO);
    h.start(&[(1, "hello"), (2, "other")]);
    assert!(h.root(1).is_some() && h.root(2).is_some());
    let st = h.status();
    assert!(st.iter().all(|s| s.shared_vm && s.source == WidgetSource::Builtin));
}

#[test]
fn unknown_widget_gets_a_did_you_mean_at_its_bar_toml_position() {
    let mut h = Harness::new();
    h.builtin("widgets/clock.luau", HELLO);
    let mut spec = Harness::spec(1, "clok");
    spec.pos = Some(rsb_core::diag::SourcePos::new("bar.toml", 7, 12));
    h.start_specs(vec![spec]);
    assert_eq!(h.plate(1).unwrap(), "RSB ERROR - bar.toml:7:12 unknown widget 'clok'; did you mean 'clock'?");
}

#[test]
fn state_and_function_props_update_only_what_changed() {
    let mut h = Harness::new();
    h.user(
        "widgets/counter.luau",
        r#"
local rsb = require("@rsb")
return rsb.widget {
    setup = function()
        local n = rsb.state(0)
        local other = rsb.state("x")
        return rsb.row {
            rsb.text { text = function() return `n={n.value}` end },
            rsb.text { text = function() return other.value end },
            on_click = function() n.value = n.value + 1 end,
        }
    end,
}
"#,
    );
    h.start(&[(1, "counter")]);
    assert_eq!(h.texts(1), vec!["n=0", "x"]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    let before = h.tree_batches;
    h.click(1, id);
    assert_eq!(h.texts(1), vec!["n=1", "x"]);
    assert_eq!(h.tree_batches, before + 1, "one tree batch per turn");
    h.click(1, id);
    assert_eq!(h.texts(1), vec!["n=2", "x"]);
}

#[test]
fn a_batch_of_writes_is_one_update() {
    let mut h = Harness::new();
    h.user(
        "widgets/b.luau",
        r#"
local rsb = require("@rsb")
return rsb.widget {
    setup = function()
        local a, b = rsb.state(0), rsb.state(0)
        local runs = rsb.state(0)
        local count = 0
        rsb.effect(function()
            local _ = a.value + b.value
            count += 1
            rsb.untrack(function() runs.value = count end)
        end)
        return rsb.row {
            rsb.text { text = function() return tostring(runs.value) end },
            on_click = function() rsb.batch(function() a.value = 1; b.value = 2 end) end,
        }
    end,
}
"#,
    );
    h.start(&[(1, "b")]);
    assert_eq!(h.texts(1), vec!["1"]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(h.texts(1), vec!["2"], "two writes, one effect run");
}

#[test]
fn service_fields_are_tracked_and_stale_is_visible() {
    let mut h = Harness::new();
    h.user(
        "widgets/cpu.luau",
        r#"
local rsb = require("@rsb")
return rsb.widget {
    permissions = {},
    setup = function()
        local sys = rsb.services.system
        return rsb.text { text = function()
            if sys.stale then return "--" end
            return `{sys.cpu}%`
        end }
    end,
}
"#,
    );
    h.start(&[(1, "cpu")]);
    assert_eq!(h.texts(1), vec!["nil%"]);
    let reqs = h.take_requests();
    assert!(
        reqs.iter().any(|r| matches!(r, HubRequest::Subscribe { service: ServiceId::System, .. })),
        "reading subscribes: {reqs:?}"
    );
    let ev = |status, cpu: f64| ServiceEvent {
        service: ServiceId::System,
        version: 1,
        status,
        value: Some(map(&[("cpu", Value::Num(cpu))])),
        error: None,
    };
    h.send(rsb_core::msg::ScriptIn::Hub(rsb_core::msg::HubEvent::Service(ev(ServiceStatus::Ok, 12.0))));
    assert_eq!(h.texts(1), vec!["12%"]);
    h.send(rsb_core::msg::ScriptIn::Hub(rsb_core::msg::HubEvent::Service(ev(ServiceStatus::Stale, 12.0))));
    assert_eq!(h.texts(1), vec!["--"]);
}
