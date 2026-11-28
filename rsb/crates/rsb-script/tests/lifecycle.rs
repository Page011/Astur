//! Instances over time: reloads, timers and the wall clock, error policy,
//! restarts, options, events, status.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::diag::SourcePos;
use rsb_core::element::InstanceId;
use rsb_core::msg::{ScriptIn, WidgetSource, WidgetState};
use rsb_core::value::Value;
use rsb_script::Limits;

fn widget(body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {{}},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

fn counter(tag: &str) -> String {
    format!(
        r#"
    local n = rsb.state(0)
    rsb.log.info("setup ran {tag}")
    return rsb.row {{ rsb.text {{ text = function() return tostring(n.value) end }}, on_click = function() n.value += 1 end }}
"#
    )
}

#[test]
fn saving_one_widget_file_restarts_only_its_instances() {
    let mut h = Harness::new();
    h.user("widgets/one.luau", &widget(&counter("one")));
    h.user("widgets/two.luau", &widget(&counter("two")));
    h.start(&[(1, "one"), (2, "two"), (3, "one#b")]);
    for id in [1, 2, 3] {
        h.click(id, first_click(h.root(id).unwrap()).unwrap());
    }
    assert_eq!([h.texts(1), h.texts(2), h.texts(3)], [["1"], ["1"], ["1"]]);
    let setups = log_lines("setup ran one").len();
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/one.luau".into(),
        source: Some(widget(&counter("one").replace("tostring(n.value)", "'v' .. tostring(n.value)"))),
    });
    assert_eq!(h.texts(1), vec!["v0"], "the saved file's instances restarted with the new code");
    assert_eq!(h.texts(3), vec!["v0"]);
    assert_eq!(h.texts(2), vec!["1"], "another widget's state is untouched");
    assert_eq!(log_lines("setup ran one").len(), setups + 2);
    assert_eq!(log_lines("setup ran two").len(), 1);
}

#[test]
fn a_module_change_restarts_the_instances_that_required_it() {
    let mut h = Harness::new();
    h.user("widgets/lib/fmt.luau", "return { f = function(x) return 'a' .. x end }");
    h.user("widgets/uses.luau", &widget("local fmt = (require('./lib/fmt') :: any) return rsb.text(fmt.f('1'))"));
    h.user("widgets/plain.luau", &widget(&counter("plain")));
    h.start(&[(1, "uses"), (2, "plain")]);
    assert_eq!(h.texts(1), vec!["a1"]);
    let n = log_lines("setup ran plain").len();
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/lib/fmt.luau".into(),
        source: Some("return { f = function(x) return 'b' .. x end }".into()),
    });
    assert_eq!(h.texts(1), vec!["b1"]);
    assert_eq!(log_lines("setup ran plain").len(), n, "the other widget did not restart");
}

#[test]
fn a_broken_save_shows_a_plate_and_the_fix_clears_it() {
    let mut h = Harness::new();
    let good = widget("return rsb.text 'ok'");
    h.user("widgets/fix.luau", &good);
    h.start(&[(1, "fix")]);
    assert_eq!(h.texts(1), vec!["ok"]);
    // Syntax error.
    h.send(ScriptIn::FileChanged { rel_path: "widgets/fix.luau".into(), source: Some(widget("return rsb.text(")) });
    let p = h.plate(1).unwrap();
    assert!(p.starts_with("RSB ERROR - widgets/fix.luau:"), "{p}");
    assert!(h.root(1).is_none(), "the widget draws nothing under its plate");
    // Runtime error in setup.
    h.send(ScriptIn::FileChanged { rel_path: "widgets/fix.luau".into(), source: Some(widget("error('bad')")) });
    assert_eq!(h.plate(1).unwrap(), "RSB ERROR - widgets/fix.luau:5 bad; fix it and save");
    // Fixed.
    h.send(ScriptIn::FileChanged { rel_path: "widgets/fix.luau".into(), source: Some(good) });
    assert_eq!(h.texts(1), vec!["ok"]);
    assert!(h.plate(1).is_none(), "plate cleared");
    // Deleting the file of a widget with no built-in of that name is a plate again.
    h.send(ScriptIn::FileChanged { rel_path: "widgets/fix.luau".into(), source: None });
    assert_eq!(h.plate(1).unwrap(), "RSB ERROR - unknown widget 'fix'");
}

#[test]
fn a_user_file_overrides_a_builtin_and_deleting_it_switches_back() {
    let mut h = Harness::new();
    h.builtin("widgets/clock.luau", &widget("return rsb.text 'builtin'"));
    h.builtin("widgets/other.luau", &widget("return rsb.text 'o'"));
    h.start(&[(1, "clock"), (2, "other")]);
    assert_eq!(h.texts(1), vec!["builtin"]);
    assert_eq!(h.status()[0].source, WidgetSource::Builtin);
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/clock.luau".into(),
        source: Some(widget("return rsb.text 'mine'")),
    });
    assert_eq!(h.texts(1), vec!["mine"]);
    let st = h.status();
    assert_eq!(st[0].source, WidgetSource::User("widgets/clock.luau".into()));
    assert!(!st[0].shared_vm);
    assert!(st[1].shared_vm, "the other built-in keeps its VM");
    // The replacement is said out loud: in the log (once) and in the status.
    assert!(st[0].replaces_builtin && !st[1].replaces_builtin, "{st:?}");
    let said = log_lines("widgets/clock.luau replaces the built-in clock widget");
    assert_eq!(said.len(), 1, "{said:?}");
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/clock.luau".into(),
        source: Some(widget("return rsb.text 'again'")),
    });
    assert_eq!(h.texts(1), vec!["again"]);
    assert_eq!(log_lines("widgets/clock.luau replaces the built-in clock widget").len(), 1, "not once a reload");
    h.send(ScriptIn::FileChanged { rel_path: "widgets/clock.luau".into(), source: None });
    assert_eq!(h.texts(1), vec!["builtin"]);
    assert_eq!(h.status()[0].source, WidgetSource::Builtin);
    assert!(!h.status()[0].replaces_builtin);
}

#[test]
fn a_file_saved_with_a_capital_extension_reloads_its_widget() {
    let mut h = Harness::new();
    h.start(&[(1, "mine")]);
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/mine.luau".into(),
        source: Some(widget("return rsb.text 'one'")),
    });
    assert_eq!(h.texts(1), vec!["one"]);
    // The file watcher reports the spelling on disk.
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/mine.LUAU".into(),
        source: Some(widget("return rsb.text 'two'")),
    });
    assert_eq!(h.texts(1), vec!["two"]);
}

#[test]
fn a_user_widget_that_shares_no_name_with_a_builtin_replaces_nothing() {
    let mut h = Harness::new();
    h.builtin("widgets/clock.luau", &widget("return rsb.text 'builtin'"));
    h.send(ScriptIn::FileChanged { rel_path: "widgets/mine.luau".into(), source: Some(widget("return rsb.text 'x'")) });
    h.start(&[(1, "mine")]);
    assert_eq!(h.texts(1), vec!["x"]);
    assert!(!h.status()[0].replaces_builtin);
}

#[test]
fn a_file_that_appears_starts_an_instance_that_had_no_widget() {
    let mut h = Harness::new();
    h.start(&[(1, "later")]);
    assert_eq!(h.plate(1).unwrap(), "RSB ERROR - unknown widget 'later'");
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/later.luau".into(),
        source: Some(widget("return rsb.text 'here'")),
    });
    assert_eq!(h.texts(1), vec!["here"]);
    assert!(h.plate(1).is_none());
}

#[test]
fn a_scale_only_change_updates_ctx_in_place_without_a_restart() {
    let mut h = Harness::new();
    h.user(
        "widgets/sc2.luau",
        &widget("local n = rsb.state(0) rsb.timer.every('1s', function() n.value = ctx.monitor.scale end) rsb.log.info('sc2 setup') return rsb.text { text = function() return tostring(n.value) end }"),
    );
    let mut spec = Harness::spec(1, "sc2");
    h.start_specs(vec![spec.clone()]);
    let before = log_lines("sc2 setup").len();
    spec.monitor.scale = 1.5;
    h.start_specs(vec![spec.clone()]);
    h.advance(1000);
    assert_eq!(log_lines("sc2 setup").len(), before, "no restart");
    assert_eq!(h.texts(1), vec!["1.5"], "ctx.monitor.scale follows the monitor");
    assert_eq!(h.status()[0].restarts, 0);
    // A change of monitor index is a different instance.
    spec.monitor.index = 1;
    h.start_specs(vec![spec]);
    assert_eq!(log_lines("sc2 setup").len(), before + 1);
}

#[test]
fn options_overlay_defaults_and_mistakes_are_warnings_at_their_bar_toml_line() {
    let mut h = Harness::new();
    h.user(
        "widgets/opts.luau",
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n  options = { format = \"HH:mm\", count = 3, flag = false },\n  optional = { \"city\" },\n  setup = function(props, ctx)\n    return rsb.text(`{props.format} {props.count} {props.flag} {props.city}`)\n  end,\n}\n",
    );
    let spec = Harness::spec_with(
        1,
        "opts",
        &[
            ("format", Value::Str("hh:mm".into())),
            ("count", Value::Str("many".into())),
            ("flagg", Value::Bool(true)),
            ("city", Value::Str("Armidale".into())),
            ("zzzzzz", Value::Int(1)),
        ],
    );
    h.start_specs(vec![spec]);
    assert_eq!(h.texts(1), vec!["hh:mm 3 false Armidale"]);
    let msgs: Vec<String> = h.diags.iter().map(|d| format!("{d}")).collect();
    assert!(
        msgs.contains(
            &"warning: bar.toml:8:3: option 'count' of widget 'opts' must be a number, got a string".to_string()
        ),
        "{msgs:#?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("bar.toml:9:3")
            && m.contains("unknown option 'flagg'")
            && m.contains("did you mean 'flag'?")),
        "{msgs:#?}"
    );
    assert!(
        msgs.iter().any(|m| m.contains("bar.toml:11:3")
            && m.contains("unknown option 'zzzzzz'")
            && !m.contains("did you mean")),
        "{msgs:#?}"
    );
    assert!(h.diags.iter().all(|d| d.severity == rsb_core::diag::Severity::Warning));
    assert!(h.plate(1).is_none(), "warnings never stop the widget");
}

#[test]
fn integer_and_float_options_are_both_numbers() {
    let mut h = Harness::new();
    h.user(
        "widgets/num.luau",
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {\n  options = { step = 2, ratio = 0.5 },\n  setup = function(props) return rsb.text(`{props.step + 1} {props.ratio * 2}`) end,\n}\n",
    );
    h.start_specs(vec![Harness::spec_with(1, "num", &[("step", Value::Int(5)), ("ratio", Value::Int(3))])]);
    assert_eq!(h.texts(1), vec!["6 6"], "{:?}", h.diags);
}

#[test]
fn widget_spec_mistakes_are_errors_with_suggestions() {
    let mut h = Harness::new();
    h.user("widgets/s1.luau", "local rsb = require(\"@rsb\")\nreturn rsb.widget { setups = function() end }\n");
    h.user(
        "widgets/s2.luau",
        "local rsb = require(\"@rsb\")\nreturn rsb.widget { permissions = {}, setup = function() end, optons = {} }\n",
    );
    h.user("widgets/s3.luau", "return 5\n");
    h.user("widgets/s4.luau", "local rsb = require(\"@rsb\")\nreturn rsb.widget { permissions = { net = { 'https://x.example' } }, setup = function() end }\n");
    h.user(
        "widgets/s5.luau",
        "local rsb = require(\"@rsb\")\nreturn rsb.widget { permissions = { nett = {} }, setup = function() end }\n",
    );
    h.start(&[(1, "s1"), (2, "s2"), (3, "s3"), (4, "s4"), (5, "s5")]);
    assert!(
        h.plate(1).unwrap().contains("unknown field 'setups' in rsb.widget; did you mean 'setup'?"),
        "{}",
        h.plate(1).unwrap()
    );
    assert!(
        h.plate(2).unwrap().contains("unknown field 'optons' in rsb.widget; did you mean 'options'?"),
        "{}",
        h.plate(2).unwrap()
    );
    assert!(h.plate(3).unwrap().contains("must return rsb.widget"), "{}", h.plate(3).unwrap());
    assert!(h.plate(4).unwrap().contains("permissions.net[1]"), "{}", h.plate(4).unwrap());
    assert!(h.plate(5).unwrap().contains("unknown permission 'nett'; did you mean 'net'?"), "{}", h.plate(5).unwrap());
}

// ---------------------------------------------------------------- time

#[test]
fn timers_for_whole_units_line_up_with_the_wall_clock() {
    let mut h = Harness::new();
    h.user(
        "widgets/tm.luau",
        &widget(
            r#"
    local n = rsb.state(0)
    local s = rsb.state(0)
    rsb.timer.every("1m", function() n.value += 1 end)
    rsb.timer.every("7s", function() s.value += 1 end)
    return rsb.text { text = function() return `{n.value} {s.value}` end }
"#,
        ),
    );
    h.start(&[(1, "tm")]);
    // Start is 09:05:07.250: the next minute is 52.75 s away.
    for _ in 0..52 {
        h.advance(1000);
    }
    assert_eq!(h.texts(1), vec!["0 7"], "a timer that missed ticks fires once per turn, never in a burst");
    h.advance(749);
    assert_eq!(h.texts(1)[0].split(' ').next(), Some("0"));
    h.advance(1);
    assert_eq!(h.texts(1)[0].split(' ').next(), Some("1"), "fires at 09:06:00.000");
    h.advance(60_000);
    assert_eq!(h.texts(1)[0].split(' ').next(), Some("2"));
}

#[test]
fn a_clock_signal_ticks_on_the_boundary_and_a_wall_jump_refreshes_it_at_once() {
    let mut h = Harness::new();
    h.user(
        "widgets/clk.luau",
        &widget(
            r#"
    local now = rsb.clock("minute")
    local utc = rsb.clock("hour", "utc")
    return rsb.text { text = function() return rsb.time.format(now.value, "HH:mm") .. " " .. rsb.time.format(utc.value, "HH") end }
"#,
        ),
    );
    h.start(&[(1, "clk")]);
    assert_eq!(h.texts(1), vec!["09:05 23"]);
    h.advance(52_000);
    assert_eq!(h.texts(1), vec!["09:05 23"], "not yet");
    h.advance(750);
    assert_eq!(h.texts(1), vec!["09:06 23"]);
    // The wall clock jumps (resume from sleep): nothing moves until the runtime is told.
    h.clock.set_wall(rsb_core::time::LocalTime { hour: 11, minute: 30, second: 0, millis: 0, ..start_time() });
    h.run_due();
    assert_eq!(h.texts(1), vec!["09:06 23"]);
    h.send(ScriptIn::ClockChanged);
    assert_eq!(h.texts(1), vec!["11:30 01"], "refreshed at once");
    // Aligned timers are re-armed against the new wall time: the next tick is a minute away.
    h.advance(59_000);
    assert_eq!(h.texts(1), vec!["11:30 01"]);
    h.advance(1_000);
    assert_eq!(h.texts(1), vec!["11:31 01"]);
}

#[test]
fn aligned_timers_are_re_armed_by_a_clock_change() {
    let mut h = Harness::new();
    h.user("widgets/rearm.luau", &widget("local n = rsb.state(0) rsb.timer.every('1m', function() n.value += 1 end) return rsb.text { text = function() return tostring(n.value) end }"));
    h.start(&[(1, "rearm")]);
    h.clock.set_wall(rsb_core::time::LocalTime { minute: 30, second: 30, millis: 0, ..start_time() });
    h.send(ScriptIn::ClockChanged);
    h.advance(29_999);
    assert_eq!(h.texts(1), vec!["0"]);
    h.advance(1);
    assert_eq!(h.texts(1), vec!["1"], "next minute boundary of the new time, 30 s later");
}

#[test]
fn timers_have_limits() {
    let mut h = Harness::new();
    h.user(
        "widgets/tl.luau",
        &widget(
            r#"
    local n = rsb.state(0)
    rsb.timer.every(0.01, function() n.value += 1 end)
    local ok = true
    for i = 1, 70 do ok = pcall(rsb.timer.after, 5, function() end) end
    rsb.log.info("last timer ok: " .. tostring(ok))
    return rsb.text { text = function() return tostring(n.value) end }
"#,
        ),
    );
    h.start(&[(1, "tl")]);
    assert!(log_lines("tl: last timer ok: false").len() == 1, "{:?}", log_lines("tl:"));
    assert_eq!(log_lines("timers faster than 100 ms").len(), 1, "one warning");
    for _ in 0..10 {
        h.advance(100);
    }
    assert_eq!(h.texts(1), vec!["10"], "clamped to 100 ms");
}

#[test]
fn timer_cancel_and_one_shot() {
    let mut h = Harness::new();
    h.user(
        "widgets/tc.luau",
        &widget(
            r#"
    local out = rsb.state("")
    local t1 = rsb.timer.every("1s", function() out.value ..= "e" end)
    rsb.timer.after("2500ms", function() out.value ..= "o" end)
    rsb.timer.after(3.5, function() t1:cancel() out.value ..= "x" end)
    return rsb.text { text = function() return out.value end }
"#,
        ),
    );
    h.start(&[(1, "tc")]);
    for _ in 0..8 {
        h.advance(1000);
    }
    let t = h.texts(1)[0].clone();
    assert!(t.matches('o').count() == 1 && t.matches('x').count() == 1, "{t}");
    assert!(t.ends_with('x'), "no tick after the cancel: {t}");
}

// ---------------------------------------------------------------- error policy

#[test]
fn an_uncaught_error_logs_marks_the_root_for_ten_seconds_and_the_widget_keeps_running() {
    let mut h = Harness::new();
    h.user(
        "widgets/err.luau",
        &widget("local n = rsb.state(0) return rsb.row { rsb.text { text = function() return tostring(n.value) end }, on_click = function() n.value += 1 error('boom ' .. n.value) end }"),
    );
    h.start(&[(1, "err")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    assert!(h.root(1).unwrap().classes.iter().any(|c| &**c == "error"));
    assert_eq!(h.texts(1), vec!["1"], "the write before the error stands");
    assert!(log_lines("err: error: widgets/err.luau:5: boom 1").len() == 1, "{:?}", log_lines("err:"));
    assert!(matches!(h.state_of(1), WidgetState::Running));
    assert_eq!(h.status()[0].errors, 1);
    h.advance(9_000);
    assert!(h.root(1).unwrap().classes.iter().any(|c| &**c == "error"));
    h.advance(1_100);
    assert!(!h.root(1).unwrap().classes.iter().any(|c| &**c == "error"), "class removed after 10 s");
}

#[test]
fn ten_errors_in_ten_seconds_stop_the_widget_fewer_do_not() {
    let mut h = Harness::new();
    h.user("widgets/many.luau", &widget("return rsb.row { rsb.text 'x', on_click = function() error('again') end }"));
    h.start(&[(1, "many")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    for _ in 0..9 {
        h.click(1, id);
        h.advance(500);
    }
    assert!(matches!(h.state_of(1), WidgetState::Running), "9 errors");
    h.click(1, id);
    assert_eq!(h.plate(1).unwrap(), "STOPPED - many had too many errors; fix it and save");
    assert!(!log_lines("stopped many: too many errors").is_empty());
}

#[test]
fn spread_out_errors_never_stop_the_widget() {
    let mut h = Harness::new();
    h.user("widgets/slow.luau", &widget("return rsb.row { rsb.text 'x', on_click = function() error('again') end }"));
    h.start(&[(1, "slow")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    for _ in 0..30 {
        h.click(1, id);
        h.advance(2_000);
    }
    assert!(matches!(h.state_of(1), WidgetState::Running));
    assert_eq!(h.status()[0].errors, 30);
}

#[test]
fn errors_in_effects_computeds_timers_and_function_props_are_not_stops() {
    let mut h = Harness::new();
    h.user(
        "widgets/mix.luau",
        &widget(
            r#"
    local a = rsb.state(0)
    rsb.effect(function() if a.value > 0 then error("effect") end end)
    local c = rsb.computed(function() if a.value > 0 then error("computed") end return 1 end)
    rsb.timer.after(1, function() error("timer") end)
    return rsb.row {
        rsb.text { text = function() if a.value > 0 then error("prop") end return "p" end },
        rsb.text { text = function() return tostring(c.value) end },
        on_click = function() a.value = 1 end,
    }
"#,
        ),
    );
    h.start(&[(1, "mix")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    h.advance(1000);
    for what in ["effect", "computed", "timer", "prop"] {
        assert!(log_lines("mix: error: ").iter().any(|l| l.contains(what)), "{what}: {:?}", log_lines("mix:"));
    }
    assert!(matches!(h.state_of(1), WidgetState::Running));
}

#[test]
fn a_failing_task_does_not_stop_siblings() {
    let mut h = Harness::new();
    h.user(
        "widgets/sib.luau",
        &widget(
            r#"
    local out = rsb.state("")
    rsb.task.spawn(function() error("one") end)
    rsb.task.spawn(function() rsb.task.sleep(1) out.value = "two" end)
    return rsb.text { text = function() return out.value end }
"#,
        ),
    );
    h.start(&[(1, "sib")]);
    h.advance(1000);
    assert_eq!(h.texts(1), vec!["two"]);
    assert_eq!(h.status()[0].errors, 1);
}

// ---------------------------------------------------------------- built-in restarts

#[test]
fn a_stopped_builtin_restarts_after_one_second_doubling_to_the_cap() {
    let limits = Limits { builtin_restart_max: Duration::from_secs(8), ..Limits::default() };
    let mut h = Harness::with(limits, None);
    h.builtin("widgets/bad.luau", &widget("while true do end return nil"));
    h.start(&[(1, "bad")]);
    assert_eq!(h.plate(1).unwrap(), "STOPPED - bad ran over 50 ms while loading; restarting");
    let mut waits = Vec::new();
    for _ in 0..6 {
        let before = log_lines("restarting bad in").len();
        // The wait before the next restart is the last logged number of seconds.
        let last = log_lines("restarting bad in").len();
        assert!(last >= before);
        let secs: u64 = log_lines("restarting bad in").last().unwrap().rsplit(' ').nth(1).unwrap().parse().unwrap();
        waits.push(secs);
        h.advance(secs * 1000 - 1);
        assert_eq!(h.status()[0].restarts, waits.len() as u32 - 1, "not before its time");
        h.advance(1);
    }
    let all: Vec<u64> =
        log_lines("restarting bad in").iter().map(|l| l.rsplit(' ').nth(1).unwrap().parse().unwrap()).collect();
    assert_eq!(&all[..6], &[1, 2, 4, 8, 8, 8], "{all:?}");
    assert_eq!(h.status()[0].restarts, 6);
}

#[test]
fn a_builtin_that_recovers_clears_its_plate_and_a_user_widget_never_restarts_itself() {
    let mut h = Harness::new();
    h.builtin("widgets/flaky.luau", &widget("while true do end return nil"));
    h.user("widgets/mine.luau", &widget("while true do end return nil"));
    h.start(&[(1, "flaky"), (2, "mine")]);
    assert!(h.plate(1).unwrap().ends_with("restarting"));
    assert!(h.plate(2).unwrap().ends_with("fix it and save"));
    h.builtin("widgets/flaky.luau", &widget("return rsb.text 'back'"));
    h.advance(1000);
    assert_eq!(h.texts(1), vec!["back"]);
    assert!(h.plate(1).is_none(), "plate cleared: {:?}", h.plate(1));
    h.advance(10_000);
    assert!(matches!(h.state_of(2), WidgetState::Stopped(_)));
    assert_eq!(h.status()[1].restarts, 0);
}

// ---------------------------------------------------------------- events, theme, status

#[test]
fn trigger_runs_event_handlers_on_the_next_turn_with_the_data() {
    let mut h = Harness::new();
    h.user(
        "widgets/ev.luau",
        &widget(
            r#"
    local out = rsb.state("")
    rsb.events.on("deploy", function(d: any) out.value = `{d.state} {d.n} {d.list[2]} {type(d.nothing)}` end)
    rsb.events.on("plain", function(d) out.value = tostring(d) end)
    return rsb.text { text = function() return out.value end }
"#,
        ),
    );
    h.start(&[(1, "ev")]);
    let data = map(&[
        ("state", Value::Str("ok".into())),
        ("n", Value::Int(3)),
        ("list", Value::List(vec![Value::Int(1), Value::Int(2)])),
        ("nothing", Value::Nil),
    ]);
    h.send(ScriptIn::Trigger { event: "deploy".into(), data });
    assert_eq!(h.texts(1), vec!["ok 3 2 nil"]);
    h.send(ScriptIn::Trigger { event: "plain".into(), data: Value::Nil });
    assert_eq!(h.texts(1), vec!["nil"]);
    h.send(ScriptIn::Trigger { event: "unknown".into(), data: Value::Nil });
}

#[test]
fn events_reach_other_widgets_and_unsubscribe_on_unload() {
    let mut h = Harness::new();
    h.user(
        "widgets/a.luau",
        &widget("rsb.timer.after(1, function() rsb.events.emit('hello', { from = 'a' }) end) return rsb.text 'a'"),
    );
    h.user("widgets/b.luau", &widget("local out = rsb.state('-') rsb.events.on('hello', function(d: any) out.value = d.from end) return rsb.text { text = function() return out.value end }"));
    h.start(&[(1, "a"), (2, "b")]);
    h.advance(1000);
    h.run_due();
    assert_eq!(h.texts(2), vec!["a"]);
    h.send(ScriptIn::SetInstances(vec![Harness::spec(1, "a")]));
    h.send(ScriptIn::Trigger { event: "hello".into(), data: Value::Nil });
    assert_eq!(h.status().len(), 1);
}

#[test]
fn theme_is_reactive() {
    let mut h = Harness::new();
    h.user(
        "widgets/th.luau",
        &widget("return rsb.text { text = function() return if rsb.theme.dark then 'dark' else 'light' end }"),
    );
    h.start(&[(1, "th")]);
    assert_eq!(h.texts(1), vec!["dark"]);
    h.send(ScriptIn::Theme { dark: false });
    assert_eq!(h.texts(1), vec!["light"]);
    // A widget started later sees the current theme.
    h.user(
        "widgets/th2.luau",
        &widget("return rsb.text { text = function() return if rsb.theme.dark then 'dark' else 'light' end }"),
    );
    h.start(&[(1, "th"), (2, "th2")]);
    assert_eq!(h.texts(2), vec!["light"]);
}

#[test]
fn services_seen_before_a_widget_starts_are_replayed_into_its_vm() {
    use rsb_core::msg::HubEvent;
    use rsb_core::services::{ServiceEvent, ServiceId, ServiceStatus};
    let mut h = Harness::new();
    h.send(ScriptIn::Hub(HubEvent::Service(ServiceEvent {
        service: ServiceId::System,
        version: 1,
        status: ServiceStatus::Ok,
        value: Some(map(&[("cpu", Value::Num(42.0)), ("memory", Value::Num(10.0))])),
        error: None,
    })));
    h.user(
        "widgets/late.luau",
        &widget(
            "return rsb.text { text = function() return `{rsb.services.system.cpu} {rsb.services.system.status}` end }",
        ),
    );
    h.start(&[(1, "late")]);
    assert_eq!(h.texts(1), vec!["42 ok"]);
}

#[test]
fn status_reports_numbers_per_instance() {
    let mut h = Harness::new();
    h.user("widgets/st.luau", &widget(&counter("st")));
    h.builtin("widgets/bi.luau", &widget("return rsb.text 'b'"));
    h.start(&[(1, "st"), (2, "bi")]);
    for _ in 0..3 {
        h.click(1, first_click(h.root(1).unwrap()).unwrap());
    }
    let st = h.status();
    assert_eq!(st[0].widget, "st");
    assert_eq!(st[0].bar, "main");
    assert_eq!(st[0].state, WidgetState::Running);
    assert!(st[0].calls >= 6, "{}", st[0].calls);
    assert!(st[0].script_time > Duration::ZERO && st[0].script_time_max <= st[0].script_time);
    assert!(st[0].vm_bytes > 100_000 && st[0].vm_bytes < 4_000_000, "{}", st[0].vm_bytes);
    assert!(!st[0].shared_vm && st[1].shared_vm);
    assert_eq!(st[0].errors, 0);
    assert_eq!(st[0].restarts, 0);
    assert_eq!(st[0].source, WidgetSource::User("widgets/st.luau".into()));
}

#[test]
fn a_user_widget_placed_twice_reports_its_one_vm_as_shared() {
    let mut h = Harness::new();
    h.user("widgets/pair.luau", &widget("return rsb.text 'x'"));
    h.user("widgets/solo.luau", &widget("return rsb.text 'y'"));
    let mut right = Harness::spec(2, "pair");
    right.bar = "right".into();
    h.start_specs(vec![Harness::spec(1, "pair"), right, Harness::spec(3, "solo")]);
    let st = h.status();
    let by_id = |id: u32| st.iter().find(|s| s.instance == InstanceId(id)).unwrap();
    assert!(by_id(1).shared_vm && by_id(2).shared_vm, "two copies run in one VM");
    assert_eq!(by_id(1).vm_bytes, by_id(2).vm_bytes);
    assert!(!by_id(3).shared_vm, "a widget placed once has its own VM");
    // Taking one copy away leaves the other with a VM of its own.
    h.start_specs(vec![Harness::spec(1, "pair"), Harness::spec(3, "solo")]);
    let st = h.status();
    assert!(st.iter().all(|s| !s.shared_vm), "{:?}", st.iter().map(|s| s.shared_vm).collect::<Vec<_>>());
}

#[test]
fn removing_an_instance_frees_a_user_vm() {
    let mut h = Harness::new();
    h.user("widgets/free.luau", &widget("return rsb.text 'x'"));
    h.start(&[(1, "free")]);
    let with = h.status()[0].vm_bytes;
    assert!(with > 0);
    h.send(ScriptIn::SetInstances(vec![]));
    assert!(h.status().is_empty());
    // And the same file can start again.
    h.start(&[(1, "free")]);
    assert_eq!(h.texts(1), vec!["x"]);
}

#[test]
fn many_user_widgets_stay_within_the_per_vm_size_the_budget_assumes() {
    let mut h = Harness::new();
    for i in 0..20 {
        h.user(&format!("widgets/w{i}.luau"), &widget("return rsb.text 'x'"));
    }
    let list: Vec<(u32, String)> = (0..20).map(|i| (i + 1, format!("w{i}"))).collect();
    let refs: Vec<(u32, &str)> = list.iter().map(|(i, n)| (*i, n.as_str())).collect();
    h.start(&refs);
    let st = h.status();
    let avg = st.iter().map(|s| s.vm_bytes).sum::<usize>() / st.len();
    eprintln!(
        "average user VM after load: {} KB (20 widgets, {} build)",
        avg / 1024,
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );
    assert!(avg < 1_500_000, "{avg}");
    let _ = (SourcePos::new("x", 1, 1), InstanceId(1));
}
