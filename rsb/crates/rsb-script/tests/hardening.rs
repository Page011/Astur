//! What one widget must not be able to do to another, or to the machine,
//! through the script API: other widgets' timers, tasks, subscriptions and
//! requests; a built-in's modules; the shell; the widgets folder.

mod common;

use common::*;
use rsb_core::msg::{HubRequest, ScriptIn};
use rsb_core::value::Value;

fn widget(perms: &str, body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {perms},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

fn click(h: &mut Harness, id: u32) {
    let handler = first_click(h.root(id).expect("a tree")).expect("a click handler");
    h.click(id, handler);
}

#[test]
fn a_widget_cannot_cancel_another_widgets_timers_tasks_or_subscriptions() {
    let mut h = Harness::new();
    h.user(
        "widgets/hc_victim.luau",
        &widget(
            "{}",
            r#"
    local t = rsb.state(0)
    local k = rsb.state(0)
    local e = rsb.state(0)
    rsb.timer.every("1s", function() t.value += 1 end)
    rsb.task.spawn(function()
        while true do
            rsb.task.sleep("1s")
            k.value += 1
        end
    end)
    rsb.events.on("hc_ping", function() e.value += 1 end)
    return rsb.text { text = function() return `{t.value} {k.value} {e.value}` end }
"#,
        ),
    );
    h.user(
        "widgets/hc_attacker.luau",
        &widget(
            "{}",
            r#"
    local th = rsb.timer.after("1h", function() end)
    local eh = rsb.events.on("hc_unused", function() end)
    return rsb.row {
        rsb.text "x",
        on_click = function()
            for i = 1, 60 do
                rsb.task.cancel({ id = i })
                rawset(th, "id", i)
                th:cancel()
                rawset(eh, "id", i)
                eh:cancel()
            end
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "hc_victim"), (2, "hc_attacker")]);
    click(&mut h, 2);
    for _ in 0..3 {
        h.advance(1000);
    }
    assert_eq!(h.texts(1), vec!["3 3 0"], "the timer and the task kept running");
    h.send(ScriptIn::Trigger { event: "hc_ping".into(), data: Value::Nil });
    h.run_due();
    assert_eq!(h.texts(1), vec!["3 3 1"], "the subscription still answers");
}

#[test]
fn a_widget_cannot_cancel_another_widgets_file_watch() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/hw_victim.luau",
        &widget("{ file = { '~/hw.txt' } }", "local w = rsb.file.watch('~/hw.txt') return rsb.text('v')"),
    );
    h.user(
        "widgets/my/hw_attacker.luau",
        &widget(
            "{ file = { '~/hw2.txt' } }",
            r#"
    local w = rsb.file.watch('~/hw2.txt')
    return rsb.row {
        rsb.text "x",
        on_click = function()
            for i = 1, 20 do
                rawset(w, "id", i)
                w:cancel()
            end
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "my/hw_victim"), (2, "my/hw_attacker")]);
    let watches: Vec<u64> = h
        .hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Io { id, owner, .. } if owner.0 == 1 => Some(id.0),
            _ => None,
        })
        .collect();
    assert_eq!(watches.len(), 1);
    h.take_requests();
    click(&mut h, 2);
    let cancelled: Vec<u64> = h
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            HubRequest::Cancel { id } => Some(id.0),
            _ => None,
        })
        .collect();
    assert!(!cancelled.contains(&watches[0]), "the victim's watch was cancelled: {cancelled:?}");
}

#[test]
fn a_built_in_never_loads_a_module_from_the_users_widgets_folder() {
    let mut h = Harness::new();
    h.builtin("widgets/lib/hs_shared.luau", "return { tag = 'built-in module' }");
    h.builtin("widgets/hs_builtin.luau", &widget("{}", "local m = require('./lib/hs_shared') return rsb.text(m.tag)"));
    h.user("widgets/lib/hs_shared.luau", "return { tag = 'USER module' }");
    h.user("widgets/hs_user.luau", &widget("{}", "local m = require('./lib/hs_shared') return rsb.text(m.tag)"));
    h.start(&[(1, "hs_builtin"), (2, "hs_user")]);
    assert_eq!(h.texts(1), vec!["built-in module"], "a user file of the same name did not run inside the built-in");
    assert_eq!(h.texts(2), vec!["USER module"], "a user widget still uses its own folder's module");
}

#[test]
fn an_ejected_copy_of_a_built_in_is_a_user_widget_and_uses_its_own_modules() {
    let mut h = Harness::new();
    h.builtin("widgets/lib/he_shared.luau", "return { tag = 'built-in module' }");
    h.builtin("widgets/he.luau", &widget("{}", "local m = require('./lib/he_shared') return rsb.text(m.tag)"));
    // `rsb eject he` copies the widget and its helper into the config folder.
    h.user("widgets/he.luau", &widget("{}", "local m = require('./lib/he_shared') return rsb.text(m.tag)"));
    h.user("widgets/lib/he_shared.luau", "return { tag = 'edited module' }");
    h.start(&[(1, "he")]);
    assert_eq!(h.texts(1), vec!["edited module"]);
}

#[test]
fn emitted_data_is_capped_and_one_widget_cannot_flood_the_queue() {
    let mut h = Harness::new();
    h.user(
        "widgets/he_sub.luau",
        &widget(
            "{}",
            r#"
    local n = rsb.state(0)
    for i = 1, 8 do
        rsb.events.on("he_big", function() n.value += 1 end)
    end
    return rsb.text { text = function() return tostring(n.value) end }
"#,
        ),
    );
    h.user(
        "widgets/he_emit.luau",
        &widget(
            "{}",
            r#"
    local msg = rsb.state("")
    return rsb.row {
        rsb.text { text = function() return msg.value end },
        on_click = function()
            local ok, e = pcall(rsb.events.emit, "he_big", { text = string.rep("x", 100000) })
            msg.value = tostring(ok) .. " " .. tostring(e)
            for i = 1, 30 do
                pcall(rsb.events.emit, "he_big", { text = string.rep("y", 60000) })
            end
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "he_sub"), (2, "he_emit")]);
    click(&mut h, 2);
    assert!(h.texts(2)[0].starts_with("false ") && h.texts(2)[0].contains("larger than 64 KiB"), "{:?}", h.texts(2));
    for _ in 0..5 {
        h.advance(100);
    }
    // 1 MiB of data a second at 60 KB a time is 17 emits, each delivered to 8 subscriptions.
    assert_eq!(h.texts(1), vec!["136"]);
    assert!(log_lines("he_emit: events are limited to").len() <= 1);
}

#[test]
fn open_url_needs_a_click_and_is_rate_limited() {
    let mut h = Harness::new();
    h.user(
        "widgets/ou.luau",
        &widget(
            "{ net = { 'open-meteo.com' } }",
            r#"
    rsb.timer.every("1s", function() pcall(rsb.open_url, "https://open-meteo.com/timer") end)
    return rsb.row {
        rsb.text "x",
        on_click = function() rsb.open_url("https://open-meteo.com/click") end,
        on_right_click = function() rsb.open_url("https://open-meteo.com/a\"b") end,
    }
"#,
        ),
    );
    h.start(&[(1, "ou")]);
    let opens = |h: &Harness| h.requests().iter().filter(|r| r.contains("OpenUrl")).count();
    h.advance(5000);
    assert_eq!(opens(&h), 0, "a timer cannot open a page");
    click(&mut h, 1);
    assert_eq!(opens(&h), 1);
    click(&mut h, 1);
    assert_eq!(opens(&h), 1, "a second click within two seconds opens nothing");
    h.advance(2100);
    click(&mut h, 1);
    assert_eq!(opens(&h), 2, "and one more after the gap");
    assert!(!h.requests().iter().any(|r| r.contains("timer")), "{:?}", h.requests());
}

#[test]
fn open_url_refuses_text_a_launcher_could_read_as_arguments() {
    let mut h = Harness::new();
    h.user(
        "widgets/ou2.luau",
        &widget(
            "{ net = { 'open-meteo.com' } }",
            r#"
    return rsb.row {
        rsb.text "x",
        on_click = function() rsb.open_url("https://open-meteo.com/a\"b") end,
    }
"#,
        ),
    );
    h.start(&[(1, "ou2")]);
    click(&mut h, 1);
    assert!(!h.requests().iter().any(|r| r.contains("OpenUrl")), "{:?}", h.requests());
    assert!(log_lines("ou2: error: ").iter().any(|l| l.contains("% escape")), "{:?}", log_lines("ou2:"));
}

#[test]
fn a_file_directly_in_widgets_has_no_folder_of_its_own() {
    let mut h = Harness::new();
    let read = "return rsb.row { rsb.text 'x', on_click = function() rsb.file.read('hf_data.txt') end }";
    h.user("widgets/hf_top.luau", &widget("{}", read));
    h.user("widgets/my/hf_nested.luau", &widget("{}", read));
    h.builtin("widgets/hf_builtin.luau", &widget("{ read = {} }", read));
    h.start(&[(1, "hf_top"), (2, "my/hf_nested"), (3, "hf_builtin")]);
    let reads = |h: &Harness| h.requests().iter().filter(|r| r.contains("ReadFile")).count();
    click(&mut h, 1);
    click(&mut h, 3);
    assert_eq!(reads(&h), 0, "no read was sent for a widget with no folder of its own");
    assert!(
        log_lines("hf_top: error: ").iter().any(|l| l.contains("no folder of its own")),
        "{:?}",
        log_lines("hf_top:")
    );
    assert!(log_lines("hf_builtin: error: ").iter().any(|l| l.contains("no folder of its own")));
    click(&mut h, 2);
    assert_eq!(reads(&h), 1, "a widget in its own folder reads next to its file");
    let grants: Vec<String> = h
        .hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Io { grant, owner, .. } if owner.0 == 2 => {
                Some(grant.widget_dir.to_string_lossy().into_owned())
            }
            _ => None,
        })
        .collect();
    assert!(grants[0].ends_with("widgets\\my"), "{grants:?}");
}

#[test]
fn a_rooted_path_is_refused_not_read_as_relative() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/hr.luau",
        &widget(
            "{}",
            r#"
    return rsb.row {
        rsb.text "x",
        on_click = function()
            pcall(function() rsb.file.read("/Users/x/notes.txt") end)
            local ok, e = pcall(rsb.file.read, "\\Users\\x\\notes.txt")
            rsb.log.info("rooted " .. tostring(ok) .. " " .. tostring(e))
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "my/hr")]);
    click(&mut h, 1);
    assert!(!h.requests().iter().any(|r| r.contains("ReadFile")), "{:?}", h.requests());
    assert!(
        log_lines("my/hr: rooted false").iter().any(|l| l.contains("cannot start with /")),
        "{:?}",
        log_lines("rooted")
    );
}

#[test]
fn shared_metatables_cannot_be_rewritten_by_a_widget() {
    let mut h = Harness::new();
    h.user(
        "widgets/hm.luau",
        &widget(
            "{}",
            r#"
    local out = {}
    local function try(name: string, f: () -> ())
        table.insert(out, name .. "=" .. tostring((pcall(f))))
    end
    try("rsb", function() getmetatable(rsb :: any).__index = function() end end)
    try("task", function() getmetatable((rsb :: any).task).__index = function() end end)
    try("service", function() getmetatable((rsb :: any).services.system).__index = function() end end)
    local task = rsb.task.spawn(function() end)
    try("handle", function() getmetatable(task :: any).__index.cancel = nil end)
    try("handle_mt", function() getmetatable(task :: any).__index = {} end)
    try("signal", function() getmetatable(rsb.state(1) :: any).__index = function() end end)
    return rsb.text(table.concat(out, " "))
"#,
        ),
    );
    h.start(&[(1, "hm")]);
    assert_eq!(h.texts(1), vec!["rsb=false task=false service=false handle=false handle_mt=false signal=false"]);
    // Widgets keep working: signals, handles and services still read normally.
    h.user(
        "widgets/hm2.luau",
        &widget(
            "{}",
            "local s = rsb.state(2) rsb.task.spawn(function() end):cancel() return rsb.text(tostring(s.value))",
        ),
    );
    h.start(&[(1, "hm"), (2, "hm2")]);
    assert_eq!(h.texts(2), vec!["2"]);
}

#[test]
fn error_text_from_setup_cannot_forge_or_reorder_log_lines() {
    let mut h = Harness::new();
    h.user(
        "widgets/hl.luau",
        &widget("{}", "error('first\\nERROR hl: forged\\r\\27[31m\\u{2028}x\\u{202e}y')\n    return rsb.text 'never'"),
    );
    h.start(&[(1, "hl")]);
    let lines = log_lines("hl: ");
    let line = lines.iter().find(|l| l.contains("first")).unwrap_or_else(|| panic!("{lines:?}"));
    assert!(line.contains("first\\nERROR hl: forged\\r\\x1b[31m\\u{2028}x\\u{202e}y"), "{line}");
    assert!(!line.contains('\n') && !line.contains('\r') && !line.contains('\u{2028}') && !line.contains('\u{202e}'));
}
