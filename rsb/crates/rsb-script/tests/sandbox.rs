//! What a hostile or careless widget cannot do, and what happens when it tries.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::msg::{ScriptIn, WidgetState};
use rsb_core::time::ThreadTime;
use rsb_core::value::Value;
use rsb_script::Limits;

/// A widget file whose `setup` is `body`; the body may return an element.
fn widget(body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {{}},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

fn plate_of(h: &Harness, id: u32) -> String {
    h.plate(id).unwrap_or_else(|| panic!("no plate; texts = {:?}", h.texts(id)))
}

#[test]
fn removed_and_replaced_globals() {
    let mut h = Harness::new();
    h.user(
        "widgets/g.luau",
        &widget(
            r#"
    local g: { [string]: any } = {}
    g.loadstring = (loadstring :: any)
    g.getfenv = (getfenv :: any)
    g.setfenv = (setfenv :: any)
    g.dofile = (dofile :: any)
    g.load = (load :: any)
    g.io = (io :: any)
    g.execute = ((os :: any).execute)
    g.exit = ((os :: any).exit)
    g.getenv = ((os :: any).getenv)
    g.remove = ((os :: any).remove)
    g.rename = ((os :: any).rename)
    g.tmpname = ((os :: any).tmpname)
    g.collect = pcall(collectgarbage, "collect")
    local list = {}
    for k, v in g do
        if v then table.insert(list, k) end
    end
    table.sort(list)
    return rsb.text(table.concat(list, ","))
"#,
        ),
    );
    h.start(&[(1, "g")]);
    assert_eq!(h.texts(1), vec![""], "no dangerous global is reachable (plate: {:?})", h.plate(1));
}

#[test]
fn print_goes_to_the_log_with_escapes_and_the_widget_ref() {
    let mut h = Harness::new();
    h.user("widgets/p.luau", &widget("print('a\\nb\\27[31m', 1, true) rsb.log.warn('w') return nil"));
    h.start(&[(1, "p")]);
    let lines = log_lines("p: a");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].ends_with("p: a\\nb\\x1b[31m 1 true"), "{}", lines[0]);
    assert!(!lines[0].contains('\n') && !lines[0].contains('\x1b'));
    assert!(log_lines("p: w").iter().any(|l| l.starts_with("WARN")));
}

#[test]
fn a_misspelt_rsb_name_says_which_file_and_line() {
    let mut h = Harness::new();
    h.user("widgets/typo.luau", &widget("return (rsb :: any).txt 'hello'"));
    h.user("widgets/typo2.luau", &widget("local x = (rsb :: any).services.audoi\nreturn nil"));
    h.user("widgets/typo3.luau", &widget("return (rsb :: any).stat(1)"));
    h.start(&[(1, "typo"), (2, "typo2"), (3, "typo3")]);
    assert_eq!(
        plate_of(&h, 1),
        "RSB ERROR - widgets/typo.luau:5 rsb.txt does not exist; did you mean rsb.text? Fix it and save"
    );
    assert_eq!(
        plate_of(&h, 2),
        "RSB ERROR - widgets/typo2.luau:5 rsb.services.audoi does not exist; did you mean rsb.services.audio? Fix it and save"
    );
    assert_eq!(
        plate_of(&h, 3),
        "RSB ERROR - widgets/typo3.luau:5 rsb.stat does not exist; did you mean rsb.state? Fix it and save"
    );
}

#[test]
fn chunk_names_never_hold_a_host_path() {
    let mut h = Harness::new();
    h.user("widgets/boom.luau", &widget("error('nope')"));
    h.builtin("widgets/bi.luau", &widget("error('nope')"));
    h.start(&[(1, "boom"), (2, "bi")]);
    assert_eq!(plate_of(&h, 1), "RSB ERROR - widgets/boom.luau:5 nope; fix it and save");
    assert_eq!(plate_of(&h, 2), "RSB ERROR - builtin/bi.luau:5 nope; restarting - check its settings in bar.toml");
    for plate in h.plates.values() {
        let t = plate.as_ref().unwrap().text.clone();
        assert!(!t.contains(":\\") && !t.contains("Users"), "{t}");
    }
}

#[test]
fn require_inside_the_folder_works_and_outside_is_refused() {
    let mut h = Harness::new();
    h.user("widgets/my/lib/x.luau", "return { v = 41 }");
    h.user("widgets/my/w.luau", &widget("local x = (require('./lib/x') :: any) return rsb.text(tostring(x.v + 1))"));
    h.user("widgets/my/out.luau", &widget("local x = require('../other/y') return nil"));
    h.user("widgets/other/y.luau", "return 1");
    h.user("widgets/my/abs.luau", &widget("local x = require('C:/Windows/x') return nil"));
    h.user("widgets/my/bare.luau", &widget("local x = require('lib/x') return nil"));
    h.user("widgets/my/cyc.luau", &widget("local x = require('./a') return nil"));
    h.user("widgets/my/a.luau", "return require('./b')");
    h.user("widgets/my/b.luau", "return require('./a')");
    h.start(&[(1, "my/w"), (2, "my/out"), (3, "my/abs"), (4, "my/bare"), (5, "my/cyc")]);
    assert_eq!(h.texts(1), vec!["42"]);
    assert!(plate_of(&h, 2).contains("leaves the widget's folder"), "{}", plate_of(&h, 2));
    assert!(plate_of(&h, 3).contains("use \"@rsb\" or a path starting with"), "{}", plate_of(&h, 3));
    assert!(plate_of(&h, 4).contains("use \"@rsb\""), "{}", plate_of(&h, 4));
    assert!(plate_of(&h, 5).contains("cycle"), "{}", plate_of(&h, 5));
}

#[test]
fn modules_are_cached_per_instance_and_reload_with_their_user() {
    let mut h = Harness::new();
    h.user("widgets/my/lib.luau", "local n = 0 return { next = function() n += 1 return n end }");
    h.user(
        "widgets/my/w.luau",
        &widget("local l = (require('./lib') :: any) local l2 = (require('./lib') :: any) return rsb.text(`{l.next()}{l2.next()}`)"),
    );
    h.start(&[(1, "my/w")]);
    assert_eq!(h.texts(1), vec!["12"], "one module table per instance");
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/my/lib.luau".into(),
        source: Some("return { next = function() return 7 end }".into()),
    });
    assert_eq!(h.texts(1), vec!["77"], "the instance that required it reloaded");
}

#[test]
fn collectgarbage_only_counts_and_sort_refuses_a_huge_uncompared_table() {
    let mut h = Harness::new();
    h.user(
        "widgets/gc.luau",
        &widget(
            r#"
    local ok1 = pcall(collectgarbage, "collect")
    local n = collectgarbage("count")
    local t = table.create(100001, 1)
    local ok2 = pcall(table.sort, t)
    local small = { 3, 1, 2 }
    table.sort(small)
    local ok3 = pcall(table.sort, { 5, 4, 3 }, function(a, b) return a < b end)
    return rsb.text(`{ok1} {type(n)} {ok2} {small[1]}{small[2]}{small[3]} {ok3}`)
"#,
        ),
    );
    h.start(&[(1, "gc")]);
    assert_eq!(h.texts(1), vec!["false number false 123 true"], "{:?}", h.plate(1));
}

#[test]
fn globals_do_not_leak_between_instances_of_one_file_or_between_files() {
    let mut h = Harness::new();
    let src = "local rsb = require(\"@rsb\")\ncounter = (counter or 0) + 1\nreturn rsb.widget { setup = function() counter = counter + 10 return rsb.text(tostring(counter)) end }\n";
    h.user("widgets/leak.luau", src);
    h.user("widgets/other.luau", src);
    h.start(&[(1, "leak"), (2, "leak#b"), (3, "other")]);
    assert_eq!(h.texts(1), vec!["11"]);
    assert_eq!(h.texts(2), vec!["11"], "a second instance of the same file starts clean");
    assert_eq!(h.texts(3), vec!["11"], "another VM starts clean");
}

#[test]
fn elements_are_opaque() {
    let mut h = Harness::new();
    h.user("widgets/o.luau", &widget("return rsb.row { { kind = 'text' } :: any }"));
    h.user(
        "widgets/o2.luau",
        &widget("local e = rsb.text 'x' local w = pcall(function() (e :: any).kind = 'box' end) local r = pcall(function() return (e :: any).kind end) return rsb.text(`{type(e)} {w} {r}`)"),
    );
    h.start(&[(1, "o"), (2, "o2")]);
    assert!(plate_of(&h, 1).contains("make elements with rsb.text, rsb.box"), "{}", plate_of(&h, 1));
    assert_eq!(h.texts(2), vec!["userdata false false"], "{:?}", h.plate(2));
}

// ---------------------------------------------------------------- budgets

fn timed(h: &mut Harness, id: u32, name: &str, src: &str) -> Duration {
    h.user(&format!("widgets/{name}.luau"), src);
    let t = Instant::now();
    h.start(&[(id, name)]);
    t.elapsed()
}

#[test]
fn a_runaway_loop_is_stopped_within_twelve_ms_of_a_ten_ms_budget() {
    // No thread clock: the budget is wall time, so the latency is measured directly.
    let src = widget("return rsb.row { rsb.text 'x', on_click = function() while true do end end }");
    let mut h = Harness::new();
    h.user("widgets/loop.luau", &src);
    let mut times = Vec::new();
    for _ in 0..8 {
        h.send(ScriptIn::FileChanged { rel_path: "widgets/loop.luau".into(), source: Some(src.clone()) });
        if h.root(1).is_none() {
            h.start(&[(1, "loop")]);
        }
        let id = first_click(h.root(1).unwrap()).unwrap();
        let t = Instant::now();
        h.click(1, id);
        times.push(t.elapsed());
        assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    }
    times.sort();
    eprintln!("budget kill latency, 8 runs, wall: min {:?} median {:?} max {:?}", times[0], times[4], times[7]);
    // 10 ms budget plus the interrupt's check interval; the teardown after it is not part of the kill.
    assert!(times[4] <= Duration::from_millis(12), "median {:?}", times[4]);
    assert!(times[7] <= Duration::from_millis(20), "max {:?}", times[7]);
    assert_eq!(plate_of(&h, 1), "STOPPED - loop ran over 10 ms; fix it and save");
    assert!(log_lines("stopped loop: ").iter().any(|l| l.contains("ms in on_click")));
}

#[test]
fn pcall_and_xpcall_cannot_swallow_a_stop() {
    let mut h = Harness::new();
    h.user(
        "widgets/pc.luau",
        &widget(
            r#"
    return rsb.row {
        rsb.text "x",
        on_click = function()
            local ok = pcall(function() while true do end end)
            -- would run on if the stop were swallowed
            marker = "survived"
            rsb.log.info("after pcall")
            local ok2 = xpcall(function() while true do end end, function(e) return e end)
            rsb.log.info("after xpcall")
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "pc")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    assert!(log_lines("pc: after").is_empty(), "{:?}", log_lines("pc: after"));
    assert_eq!(plate_of(&h, 1), "STOPPED - pc ran over 10 ms; fix it and save");
}

#[test]
fn a_swallowed_stop_cannot_keep_using_the_api() {
    let mut h = Harness::new();
    h.user(
        "widgets/sw.luau",
        &widget(
            r#"
    return rsb.row {
        rsb.text "x",
        on_click = function()
            pcall(function() while true do end end)
            pcall(function() rsb.log.info("log after stop") end)
            pcall(function() rsb.exec.run({ "cmd" }) end)
            for i = 1, 1000 do pcall(rsb.json.encode, { i }) end
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "sw")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)), "the first runaway stopped the widget");
    assert_eq!(plate_of(&h, 1), "STOPPED - sw ran over 10 ms; fix it and save");
    assert!(log_lines("sw: log after stop").is_empty());
    assert!(!h.requests().iter().any(|r| r.contains("ExecRun")));
}

#[test]
fn deep_recursion_is_an_error_not_a_crash() {
    let mut h = Harness::new();
    h.user(
        "widgets/rec.luau",
        &widget(
            r#"
    local function f(n: number): number return f(n + 1) + 1 end
    return rsb.row { rsb.text "x", on_click = function() f(1) end }
"#,
        ),
    );
    h.start(&[(1, "rec")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    // A stack overflow is an ordinary error: the widget keeps running with class `error`.
    let st = h.status();
    assert_eq!(st[0].errors, 1, "{st:?}");
    assert!(matches!(st[0].state, WidgetState::Running), "{:?}", st[0].state);
    assert!(h.root(1).unwrap().classes.iter().any(|c| &**c == "error"));
    assert!(!log_lines("rec: error: ").is_empty());
}

#[test]
fn a_coroutine_loop_is_stopped() {
    let mut h = Harness::new();
    h.user(
        "widgets/co.luau",
        &widget(
            r#"
    return rsb.row { rsb.text "x", on_click = function()
        local co = coroutine.wrap(function() while true do end end)
        co()
    end }
"#,
        ),
    );
    h.start(&[(1, "co")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(plate_of(&h, 1), "STOPPED - co ran over 10 ms; fix it and save");
}

#[test]
fn the_memory_cap_stops_a_string_bomb() {
    let mut h = Harness::new();
    h.user(
        "widgets/mem.luau",
        &widget(
            "return rsb.row { rsb.text 'x', on_click = function() local s = string.rep('x', 200 * 1024 * 1024) end }",
        ),
    );
    h.user(
        "widgets/mem2.luau",
        &widget(
            "return rsb.row { rsb.text 'x', on_click = function() local t = {} for i = 1, 100000000 do t[i] = { i } end end }",
        ),
    );
    h.start(&[(1, "mem"), (2, "mem2")]);
    for id in [1, 2] {
        let c = first_click(h.root(id).unwrap()).unwrap();
        h.click(id, c);
    }
    assert_eq!(plate_of(&h, 1), "STOPPED - mem used over 32 MB; fix it and save");
    assert!(
        plate_of(&h, 2) == "STOPPED - mem2 used over 32 MB; fix it and save"
            || plate_of(&h, 2).starts_with("STOPPED - mem2 ran over"),
        "{}",
        plate_of(&h, 2)
    );
    assert!(!log_lines("stopped mem: used over 32 MB").is_empty());
}

#[test]
fn memory_in_a_vm_is_bounded_by_the_total() {
    let limits = Limits { total_vm_memory: 1_500_000, ..Limits::default() };
    let mut h = Harness::with(limits, None);
    h.user("widgets/a.luau", &widget("return rsb.text 'a'"));
    h.user("widgets/b.luau", &widget("return rsb.text 'b'"));
    h.start(&[(1, "a"), (2, "b")]);
    // The first fits; the second VM would take the total past the cap.
    assert!(h.root(1).is_some());
    assert!(plate_of(&h, 2).contains("cannot load"), "{}", plate_of(&h, 2));
}

#[test]
fn setup_has_its_own_fifty_ms_budget() {
    let mut h = Harness::new();
    let took =
        timed(&mut h, 1, "slowsetup", &widget("local t = os.clock() while os.clock() - t < 0.2 do end return nil"));
    assert_eq!(plate_of(&h, 1), "STOPPED - slowsetup ran over 50 ms while loading; fix it and save");
    assert!(took < Duration::from_millis(80), "{took:?}");
}

#[test]
fn a_preempted_call_is_not_stopped_but_250_ms_of_wall_time_always_is() {
    // The fake thread clock only advances 1 ms per read: the call is "preempted".
    let tt: Arc<dyn ThreadTime> = Arc::new(StepTime { now: Default::default(), step: Duration::ZERO });
    let mut h = Harness::with(Limits::default(), Some(tt));
    h.user(
        "widgets/pre.luau",
        &widget(
            r#"
    return rsb.row { rsb.text "x", on_click = function()
        local t = os.clock()
        while os.clock() - t < 0.05 do end -- 50 ms of wall time, zero CPU
        marker = true
    end, on_right_click = function()
        local t = os.clock()
        while true do end
    end }
"#,
        ),
    );
    h.start(&[(1, "pre")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert!(matches!(h.state_of(1), WidgetState::Running), "{:?}", h.plate(1));
    assert_eq!(h.status()[0].errors, 0);
    // The wall cap stops it whatever the CPU clock says.
    let right = h.root(1).unwrap().handlers.right_click.map(|x| x.0).unwrap();
    let t = Instant::now();
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: rsb_core::element::InstanceId(1),
        handler: rsb_core::element::HandlerId(right),
        kind: rsb_core::hit::InputKind::Click(rsb_core::hit::Button::Right),
        x: 0.0,
        y: 0.0,
    }));
    let e = t.elapsed();
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    assert!(e >= Duration::from_millis(240) && e < Duration::from_millis(330), "{e:?}");
    assert_eq!(plate_of(&h, 1), "STOPPED - pre ran over 250 ms; fix it and save");
}

#[test]
fn cpu_time_over_the_budget_is_stopped_from_the_thread_clock() {
    // Every read of the thread clock adds 3 ms, so a loop that reads the Luau clock and
    // the interrupt's own reads pass 10 ms of "CPU" quickly.
    let tt: Arc<dyn ThreadTime> = Arc::new(StepTime { now: Default::default(), step: Duration::from_millis(3) });
    let mut h = Harness::with(Limits::default(), Some(tt));
    h.user("widgets/cpu.luau", &widget("return rsb.row { rsb.text 'x', on_click = function() while true do end end }"));
    h.start(&[(1, "cpu")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(plate_of(&h, 1), "STOPPED - cpu ran over 10 ms; fix it and save");
}

#[test]
fn one_instance_cannot_use_more_than_250_ms_of_cpu_in_a_second() {
    // 20 ms of CPU per call (thread clock steps), well over budget? No: budget is 10 ms, so use
    // a generous per-call budget and many calls.
    let limits = Limits { callback_budget: Duration::from_millis(100), ..Limits::default() };
    let mut h = Harness::with(limits, None);
    h.user(
        "widgets/burn.luau",
        &widget(
            r#"
    return rsb.row { rsb.text "x", on_click = function()
        local t = os.clock()
        while os.clock() - t < 0.04 do end
    end }
"#,
        ),
    );
    h.start(&[(1, "burn")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    for _ in 0..4 {
        h.click(1, id);
        h.clock.advance(Duration::from_millis(10));
    }
    assert!(matches!(h.state_of(1), WidgetState::Running), "160 ms is inside the window");
    for _ in 0..4 {
        h.click(1, id);
        h.clock.advance(Duration::from_millis(10));
    }
    assert_eq!(plate_of(&h, 1), "STOPPED - burn used too much CPU; fix it and save");
    assert!(!log_lines("stopped burn: ").iter().all(|l| !l.contains("ms CPU in 1 s")));
}

#[test]
fn a_bare_yield_loop_hits_the_cpu_window() {
    let limits = Limits {
        callback_budget: Duration::from_millis(100),
        turn_budget: Duration::from_millis(2),
        ..Limits::default()
    };
    let mut h = Harness::with(limits, None);
    h.user(
        "widgets/yl.luau",
        &widget(
            r#"
    rsb.task.spawn(function()
        while true do
            local t = os.clock()
            while os.clock() - t < 0.004 do end
            coroutine.yield()
        end
    end)
    return rsb.text "x"
"#,
        ),
    );
    h.start(&[(1, "yl")]);
    for _ in 0..200 {
        if matches!(h.state_of(1), WidgetState::Stopped(_)) {
            break;
        }
        h.clock.advance(Duration::from_millis(1));
        h.run_due();
    }
    assert_eq!(plate_of(&h, 1), "STOPPED - yl used too much CPU; fix it and save");
}

#[test]
fn an_effect_that_changes_what_it_reads_is_an_update_loop() {
    let mut h = Harness::new();
    h.user(
        "widgets/ul.luau",
        &widget(
            r#"
    local a = rsb.state(0)
    rsb.effect(function() a.value = a.value + 1 end)
    return rsb.text "x"
"#,
        ),
    );
    h.start(&[(1, "ul")]);
    assert_eq!(plate_of(&h, 1), "STOPPED - ul is in an update loop (an effect changes what it reads)");
    assert!(!log_lines("stopped ul: update loop").is_empty());
    assert!(h.root(1).is_none() || h.plate(1).is_some());
}

#[test]
fn a_builtin_update_loop_says_it_restarts() {
    let mut h = Harness::new();
    h.builtin(
        "widgets/ulb.luau",
        &widget(
            r#"
    local a = rsb.state(0)
    rsb.effect(function() a.value = a.value + 1 end)
    return rsb.text "x"
"#,
        ),
    );
    h.start(&[(1, "ulb")]);
    assert_eq!(plate_of(&h, 1), "STOPPED - ulb is in an update loop (an effect changes what it reads); restarting");
    h.advance(1500);
    assert!(
        h.status().iter().find(|w| w.instance == InstanceId(1)).unwrap().restarts >= 1,
        "it restarts, as the plate says"
    );
}

#[test]
fn an_update_loop_that_starts_later_is_stopped_too() {
    let mut h = Harness::new();
    h.user(
        "widgets/ul2.luau",
        &widget(
            r#"
    local a = rsb.state(0)
    local b = rsb.state(0)
    rsb.effect(function() if b.value > 0 then a.value = a.value + 1 end end)
    rsb.effect(function() if a.value > 0 then b.value = b.value + 1 end end)
    return rsb.row { rsb.text "x", on_click = function() b.value = 1 end }
"#,
        ),
    );
    h.start(&[(1, "ul2")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(plate_of(&h, 1), "STOPPED - ul2 is in an update loop (an effect changes what it reads)");
}

#[test]
fn event_ping_pong_is_rate_limited_and_other_widgets_keep_updating() {
    let mut h = Harness::new();
    let pong = |me: &str, other: &str| {
        widget(&format!(
            r#"
    local n = rsb.state(0)
    rsb.events.on("{me}", function() n.value += 1 rsb.events.emit("{other}") end)
    rsb.events.on("start", function() rsb.events.emit("{other}") end)
    return rsb.text {{ text = function() return tostring(n.value) end }}
"#
        ))
    };
    h.user("widgets/ping.luau", &pong("ping", "pong"));
    h.user("widgets/pong.luau", &pong("pong", "ping"));
    h.user(
        "widgets/tick.luau",
        &widget("local n = rsb.state(0) rsb.timer.every('1s', function() n.value += 1 end) return rsb.text { text = function() return tostring(n.value) end }"),
    );
    h.start(&[(1, "ping"), (2, "pong"), (3, "tick")]);
    h.send(ScriptIn::Trigger { event: "start".into(), data: Value::Nil });
    // Each handler runs on the turn after its event: one hop per turn.
    for _ in 0..500 {
        h.advance(10);
    }
    let a: u32 = h.texts(1)[0].parse().unwrap();
    let b: u32 = h.texts(2)[0].parse().unwrap();
    // 5 s of simulated time, 50 events a second per widget: at most ~250 each (+ slack for window edges).
    assert!(a <= 320 && b <= 320, "ping {a} pong {b}");
    assert!(a > 20 && b > 20, "it did run: ping {a} pong {b}");
    assert_eq!(h.texts(3)[0], "5", "the timer widget kept ticking");
    assert!(matches!(h.state_of(1), WidgetState::Running));
    assert!(!log_lines("events are limited to 50 a second").is_empty());
}

#[test]
fn an_element_tree_nested_65_deep_stops_the_instance() {
    let mut h = Harness::new();
    h.user(
        "widgets/deep.luau",
        &widget(
            r#"
    local e = rsb.text "leaf"
    for i = 1, 64 do e = rsb.box { e } end
    return e
"#,
        ),
    );
    h.user(
        "widgets/ok.luau",
        &widget(
            r#"
    local e = rsb.text "leaf"
    for i = 1, 63 do e = rsb.box { e } end
    return e
"#,
        ),
    );
    h.start(&[(1, "deep"), (2, "ok")]);
    assert_eq!(plate_of(&h, 1), "STOPPED - deep nests elements deeper than 64 levels; fix it and save");
    assert!(h.root(2).is_some() && h.plate(2).is_none());
}

#[test]
fn too_many_nodes_stop_the_instance() {
    let mut h = Harness::new();
    h.user(
        "widgets/wide.luau",
        &widget("local kids = {} for i = 1, 2100 do kids[i] = rsb.text(tostring(i)) end return rsb.row(kids)"),
    );
    h.start(&[(1, "wide")]);
    assert_eq!(plate_of(&h, 1), "STOPPED - wide draws over 2000 elements; fix it and save");
}

#[test]
fn json_encode_refuses_cycles_and_depth_and_decode_the_size() {
    let mut h = Harness::new();
    h.user(
        "widgets/j.luau",
        &widget(
            r#"
    local t: any = {}
    t.self = t
    local ok1, e1 = pcall(rsb.json.encode, t)
    local deep: any = {}
    local cur = deep
    for i = 1, 70 do cur.n = {} cur = cur.n end
    local ok2, e2 = pcall(rsb.json.encode, deep)
    local ok3 = pcall(rsb.json.decode, string.rep("[", 70) .. string.rep("]", 70))
    local ok4 = pcall(rsb.json.decode, "{")
    local good = rsb.json.decode('{"a":[1,2,{"b":null}]}') :: any
    local fn_ok = pcall(rsb.json.encode, { f = function() end })
    return rsb.text(`{ok1} {tostring(e1):find("contains itself") ~= nil} {ok2} {tostring(e2):find("nested over 64") ~= nil} {ok3} {ok4} {good.a[2]} {fn_ok} {rsb.json.encode({ 1, 2 })}`)
"#,
        ),
    );
    h.start(&[(1, "j")]);
    assert_eq!(h.texts(1), vec!["false true false true false false 2 false [1,2]"], "{:?}", h.plate(1));
}

#[test]
fn a_big_json_input_is_refused_by_size() {
    let mut h = Harness::new();
    h.user("widgets/big.luau", &widget("local ok = pcall(rsb.json.decode, string.rep(' ', 2 * 1024 * 1024 + 1) .. '1') return rsb.text(tostring(ok))"));
    h.start(&[(1, "big")]);
    assert_eq!(h.texts(1), vec!["false"], "{:?}", h.plate(1));
}

// ---------------------------------------------------------------- the same names in every VM

#[test]
fn the_standard_library_is_read_only() {
    let mut h = Harness::new();
    h.user(
        "widgets/ro.luau",
        &widget(
            r#"
    local a = pcall(function() (string :: any).len = nil end)
    local b = pcall(function() (table :: any).insert = nil end)
    local c = pcall(function() (math :: any).pi = 3 end)
    local d = pcall(function() (rsb :: any).text = nil end)
    local e = pcall(function() (rsb.services.audio :: any).volume = 3 end)
    local f = pcall(function() (rsb.services.audio :: any).nothing = 3 end)
    return rsb.text(`{a} {b} {c} {d} {e} {f}`)
"#,
        ),
    );
    h.start(&[(1, "ro")]);
    assert_eq!(h.texts(1), vec!["false false false false false false"], "{:?}", h.plate(1));
}

#[test]
fn heartbeat_wraps_every_call_into_a_vm() {
    let mut h = Harness::new();
    h.user(
        "widgets/hb.luau",
        &widget("return rsb.row { rsb.text 'x', on_click = function() rsb.exec.shell('x') end }"),
    );
    h.start(&[(1, "hb")]);
    assert!(h.heartbeat.busy(Instant::now()).is_none(), "idle between calls");
    let calls_before = {
        h.heartbeat.begin(rsb_core::element::InstanceId(0), Instant::now());
        let c = h.heartbeat.busy(Instant::now()).unwrap().2;
        h.heartbeat.end();
        c
    };
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    h.heartbeat.begin(rsb_core::element::InstanceId(0), Instant::now());
    let calls_after = h.heartbeat.busy(Instant::now()).unwrap().2;
    h.heartbeat.end();
    assert!(
        calls_after >= calls_before + 3,
        "handler call, task resume and render each counted: {calls_before} -> {calls_after}"
    );
    // Every request the runtime sent from inside a VM call went out while the heartbeat was busy.
    let busy = h.hub_busy.lock().unwrap().clone();
    let sends = h.hub.lock().unwrap().len();
    assert_eq!(busy.len(), sends);
}
