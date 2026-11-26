//! More limits and edge paths: logging caps, deadlines inside Rust callbacks,
//! handler ids, images, the shared VM's memory.

mod common;

use common::*;
use rsb_core::element::{HandlerId, InstanceId};
use rsb_core::hit::{Button, HandlerCall, InputKind};
use rsb_core::msg::{ScriptIn, WidgetState};
use rsb_core::value::Value;
use rsb_script::Limits;

fn widget(body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {{}},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

#[test]
fn log_lines_are_limited_per_instance_and_the_drops_are_counted() {
    let mut h = Harness::new();
    h.user(
        "widgets/chatty.luau",
        &widget("return rsb.row { rsb.text 'x', on_click = function() for i = 1, 100 do rsb.log.info('chatty line ' .. i) end end }"),
    );
    h.start(&[(1, "chatty")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    assert_eq!(log_lines("chatty: chatty line").len(), 20);
    let dropped = log_lines("chatty: 1 log lines dropped");
    assert_eq!(dropped.len(), 1, "{:?}", log_lines("chatty:"));
    h.advance(61_000);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    assert_eq!(log_lines("chatty: chatty line").len(), 40, "a new second, a new allowance");
    assert!(
        log_lines("chatty: ")
            .iter()
            .any(|l| l.contains("log lines dropped (limit 20 a second)") && !l.contains(" 1 log")),
        "{:?}",
        log_lines("chatty: ")
    );
}

#[test]
fn all_instances_together_log_at_most_200_lines_a_second() {
    let mut h = Harness::new();
    let src = widget(
        "return rsb.row { rsb.text 'x', on_click = function() for i = 1, 20 do rsb.log.info('crowd line') end end }",
    );
    let names: Vec<String> = (0..15).map(|i| format!("crowd{i}")).collect();
    for n in &names {
        h.user(&format!("widgets/{n}.luau"), &src);
    }
    let list: Vec<(u32, &str)> = names.iter().enumerate().map(|(i, n)| (i as u32 + 1, n.as_str())).collect();
    h.start(&list);
    for (id, _) in &list {
        h.click(*id, first_click(h.root(*id).unwrap()).unwrap());
    }
    let n = log_lines("crowd line").len();
    assert_eq!(n, 200, "15 x 20 lines in one second: {n}");
    // The 100 refused lines are not lost without a trace: one warning says how many.
    let said = log_lines("script log lines dropped across all widgets");
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with("WARN") && said[0].contains("(limit 200 a second)"), "{said:?}");
}

#[test]
fn a_widget_over_its_own_log_limit_does_not_use_up_the_shared_allowance() {
    let mut h = Harness::new();
    h.user(
        "widgets/loud.luau",
        &widget("return rsb.row { rsb.text 'x', on_click = function() for i = 1, 1000 do rsb.log.info('loud line') end end }"),
    );
    h.user(
        "widgets/quiet.luau",
        &widget("return rsb.row { rsb.text 'x', on_click = function() rsb.log.info('quiet line') end }"),
    );
    h.start(&[(1, "loud"), (2, "quiet")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    h.click(2, first_click(h.root(2).unwrap()).unwrap());
    assert_eq!(log_lines("loud: loud line").len(), 20);
    assert_eq!(log_lines("quiet: quiet line").len(), 1, "the quiet widget's line is not starved");
}

#[test]
fn a_huge_json_walk_is_stopped_by_the_deadline_check_inside_the_callback() {
    let mut h = Harness::new();
    h.user(
        "widgets/jb.luau",
        &widget(
            "return rsb.row { rsb.text 'x', on_click = function() local t = table.create(1500000, 1) rsb.json.encode(t) end }",
        ),
    );
    h.start(&[(1, "jb")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let plate = h.plate(1).expect("stopped");
    assert!(plate == "STOPPED - jb ran over 10 ms; fix it and save" || plate.contains("used over 32 MB"), "{plate}");
}

#[test]
fn a_big_json_decode_is_stopped_once_it_has_run_over_the_budget() {
    let mut h = Harness::new();
    h.user(
        "widgets/jd.luau",
        &widget(
            "return rsb.row { rsb.text 'x', on_click = function() local s = '[' .. string.rep('1,', 990000) .. '1]' rsb.json.decode(s) end }",
        ),
    );
    h.start(&[(1, "jd")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let plate = h.plate(1).expect("stopped");
    assert_eq!(plate, "STOPPED - jd ran over 10 ms; fix it and save");
}

#[test]
fn old_and_unknown_handler_ids_are_ignored_and_new_ones_never_reuse_numbers() {
    let mut h = Harness::new();
    h.user(
        "widgets/hid.luau",
        &widget(
            r#"
    local on = rsb.state(true)
    return rsb.row {
        function() return if on.value then { rsb.box { on_click = function() rsb.log.info("hid first") on.value = false end } } else { rsb.box { on_click = function() rsb.log.info("hid second") end } } end,
    }
"#,
        ),
    );
    h.start(&[(1, "hid")]);
    let first = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, first);
    assert_eq!(log_lines("hid: hid first").len(), 1);
    let second = first_click(h.root(1).unwrap()).unwrap();
    assert!(second > first, "{first} -> {second}");
    // A click on the frame the scene drew before the rebuild reaches no handler at all.
    h.click(1, first);
    h.click(1, 4242);
    assert!(log_lines("hid: hid second").is_empty());
    assert!(matches!(h.state_of(1), WidgetState::Running));
    assert_eq!(h.status()[0].errors, 0);
    h.click(1, second);
    assert_eq!(log_lines("hid: hid second").len(), 1);
}

#[test]
fn wheel_and_enter_leave_handlers_get_their_events() {
    let mut h = Harness::new();
    h.user(
        "widgets/wh.luau",
        &widget(
            r#"
    local out = rsb.state("")
    return rsb.row {
        rsb.text { text = function() return out.value end },
        on_wheel = function(e) out.value = `w{e.delta},{e.x},{e.y}` end,
        on_enter = function() out.value = "in" end,
        on_leave = function() out.value ..= "out" end,
        on_middle_click = function(e) out.value = e.button end,
    }
"#,
        ),
    );
    h.start(&[(1, "wh")]);
    let hs = h.root(1).unwrap().handlers;
    let call = |kind, handler: HandlerId| {
        ScriptIn::Input(HandlerCall { instance: InstanceId(1), handler, kind, x: 3.0, y: 4.5, gesture: 0 })
    };
    h.send(call(InputKind::Wheel(-1.0), hs.wheel.unwrap()));
    assert_eq!(h.texts(1), vec!["w-1,3,4.5"]);
    h.send(call(InputKind::Enter, hs.enter.unwrap()));
    assert_eq!(h.texts(1), vec!["in"]);
    h.send(call(InputKind::Leave, hs.leave.unwrap()));
    assert_eq!(h.texts(1), vec!["inout"]);
    h.send(call(InputKind::Click(Button::Middle), hs.middle_click.unwrap()));
    assert_eq!(h.texts(1), vec!["middle"]);
}

#[test]
fn an_image_outside_the_widget_folder_needs_file_permission() {
    let mut h = Harness::new();
    h.user("widgets/img1.luau", &widget("return rsb.image { src = 'C:/Pictures/a.png' }"));
    h.user("widgets/my/img2.luau", &widget("return rsb.image { src = '../../secret.png' }"));
    h.user(
        "widgets/img3.luau",
        "local rsb = require(\"@rsb\")\nreturn rsb.widget { permissions = { file = { '~/Pictures/*.png' } }, setup = function() return rsb.image { src = '~/Pictures/a.png', width = 16, height = 16 } end }\n",
    );
    h.start(&[(1, "img1"), (2, "my/img2"), (3, "img3")]);
    assert_eq!(h.plate(1).unwrap(), "PERMISSION - img1 needs file:C:/Pictures/a.png; add it to permissions");
    assert!(h.plate(2).unwrap().starts_with("PERMISSION - my/img2 needs file:"), "{:?}", h.plate(2));
    assert!(h.plate(3).is_none());
    let tree = h.latest.get(&InstanceId(3)).unwrap();
    assert!(tree.grant.is_some(), "the tree carries the grant for the image loader");
    let img = tree.root.as_ref().unwrap().image.as_ref().unwrap();
    assert_eq!(&*img.path, "C:\\Users\\tester\\Pictures\\a.png");
}

#[test]
fn a_changed_option_is_a_different_instance_and_restarts() {
    let mut h = Harness::new();
    h.user(
        "widgets/opt.luau",
        "local rsb = require(\"@rsb\")\nreturn rsb.widget { options = { word = 'a' }, setup = function(props) rsb.log.info('opt setup ' .. props.word) return rsb.text(props.word) end }\n",
    );
    h.start_specs(vec![Harness::spec_with(1, "opt", &[("word", Value::Str("b".into()))])]);
    assert_eq!(h.texts(1), vec!["b"]);
    h.start_specs(vec![Harness::spec_with(1, "opt", &[("word", Value::Str("b".into()))])]);
    assert_eq!(log_lines("opt: opt setup b").len(), 1, "same options: no restart");
    h.start_specs(vec![Harness::spec_with(1, "opt", &[("word", Value::Str("c".into()))])]);
    assert_eq!(h.texts(1), vec!["c"]);
}

#[test]
fn the_shared_vm_is_rebuilt_when_its_memory_stays_high_after_a_stop() {
    let holder = widget(
        "local keep = table.create(100000, 1) rsb.log.info('holder setup') return rsb.text { text = function() return tostring(#keep) end }",
    );
    let hog = widget(
        "return rsb.row { rsb.text 'x', on_click = function() local t = {} for i = 1, 1000000 do t[i] = { i } end end }",
    );
    // First measure what the two widgets use together, then size the shared VM so that is 94% of it.
    let mut probe = Harness::new();
    probe.builtin("widgets/holder.luau", &holder);
    probe.builtin("widgets/hog.luau", &hog);
    probe.start(&[(1, "holder"), (2, "hog")]);
    let used = probe.status()[0].vm_bytes;
    let limits = Limits { shared_vm_memory: used * 100 / 94, ..Limits::default() };
    let mut h = Harness::with(limits, None);
    h.builtin("widgets/holder.luau", &holder);
    h.builtin("widgets/hog.luau", &hog);
    h.start(&[(1, "holder"), (2, "hog")]);
    assert_eq!(h.texts(1), vec!["100000"], "{:?} {:?}", h.plate(1), h.plate(2));
    let setups = log_lines("holder setup").len();
    h.click(2, first_click(h.root(2).unwrap()).unwrap());
    assert!(!log_lines("rebuilding the shared widget VM").is_empty());
    assert_eq!(log_lines("holder setup").len(), setups + 1, "the other instance was reloaded in the new VM");
    assert_eq!(h.texts(1), vec!["100000"]);
    assert!(matches!(h.state_of(1), WidgetState::Running));
    assert!(matches!(h.state_of(2), WidgetState::Stopped(_)), "the stopped one stays stopped: {:?}", h.plate(2));
}

#[test]
fn a_turn_is_fair_between_instances_and_leaves_the_rest_for_the_next_turn() {
    use std::time::Duration;
    let limits = Limits {
        turn_budget: Duration::from_millis(5),
        callback_budget: Duration::from_millis(50),
        cpu_window_budget: Duration::from_secs(1),
        ..Limits::default()
    };
    let mut h = Harness::with(limits, None);
    // A has 60 queued events that each burn about 1 ms; B has one cheap event.
    let hog = widget(
        r#"
    local n = rsb.state(0)
    for i = 1, 60 do
        rsb.events.on("go", function() local t = os.clock() while os.clock() - t < 0.001 do end n.value += 1 end)
    end
    return rsb.text { text = function() return tostring(n.value) end }
"#,
    );
    let quick = widget(
        "local n = rsb.state(0) rsb.events.on('go', function() n.value += 1 end) return rsb.text { text = function() return tostring(n.value) end }",
    );
    h.user("widgets/fhog.luau", &hog);
    h.user("widgets/fquick.luau", &quick);
    h.start(&[(1, "fhog"), (2, "fquick")]);
    h.send(ScriptIn::Trigger { event: "go".into(), data: Value::Nil });
    assert_eq!(h.texts(2), vec!["1"], "the cheap instance ran in the first turn");
    let first: u32 = h.texts(1)[0].parse().unwrap();
    assert!((1..60).contains(&first), "the busy instance was cut off at the turn budget: {first}");
    let mut turns = 1;
    while h.texts(1)[0] != "60" && turns < 100 {
        h.run_due();
        turns += 1;
    }
    assert_eq!(h.texts(1), vec!["60"]);
    eprintln!("60 x 1 ms of work finished over {turns} turns of 5 ms (first turn ran {first})");
    assert!(turns > 3);
}

#[test]
fn building_a_big_tree_of_bindings_is_not_an_update_loop() {
    let mut h = Harness::new();
    h.user(
        "widgets/big.luau",
        &widget(
            r#"
    local s = rsb.state(0)
    local kids = {}
    for i = 1, 1500 do kids[i] = rsb.text { text = function() return tostring(s.value + i) end } end
    return rsb.row(kids)
"#,
        ),
    );
    // 1500 bindings run once each while the tree is built: well over the 1000 re-run cap, and fine.
    h.start(&[(1, "big")]);
    assert!(h.plate(1).is_none() || !h.plate(1).unwrap().contains("update loop"), "{:?}", h.plate(1));
    assert_eq!(h.texts(1).len(), 1500);
}
