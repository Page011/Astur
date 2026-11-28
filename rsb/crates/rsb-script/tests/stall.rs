//! The hidden stall hook and what happens after a call that the interrupt
//! cannot see ends. Its own test binary: the hook is process-wide.

mod common;

use std::time::Instant;

use common::*;
use rsb_core::msg::WidgetState;

fn widget(body: &str) -> String {
    format!("local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  setup = function()\n{body}\n  end,\n}}\n")
}

#[test]
fn a_stall_the_interrupt_cannot_see_stops_the_widget_when_the_call_returns() {
    rsb_script::set_test_stall_widget(Some("stall"));
    let mut h = Harness::new();
    h.user(
        "widgets/stall.luau",
        &widget("return rsb.row { rsb.text 'x', on_click = function() __test_stall(400) end }"),
    );
    h.user("widgets/calm.luau", &widget("return rsb.row { rsb.text 'x', on_click = function() local ok = pcall(function() return __test_stall end) rsb.log.info('calm sees stall hook: ' .. tostring(__test_stall ~= nil)) end }"));
    h.start(&[(1, "stall"), (2, "calm")]);
    h.click(2, first_click(h.root(2).unwrap()).unwrap());
    assert_eq!(log_lines("calm sees stall hook: false").len(), 1, "only the named widget's VM has it");
    let id = first_click(h.root(1).unwrap()).unwrap();
    let t = Instant::now();
    h.click(1, id);
    let took = t.elapsed();
    assert!(took.as_millis() >= 400, "{took:?}");
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    assert_eq!(h.plate(1).unwrap(), "STOPPED - stall ran over 250 ms; fix it and save");
    assert!(matches!(h.state_of(2), WidgetState::Running));
    rsb_script::set_test_stall_widget(None);
}
