//! The `rsb` module: every documented name exists, reserved and misspelled
//! names say what to do, and the small helpers behave as typed.

mod common;

use common::*;
use rsb_script::api::{API_ITEMS, HINTS, RESERVED, RESERVED_PROPS};

fn widget(body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {{}},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

#[test]
fn every_api_item_exists_at_runtime_with_its_documented_kind() {
    let mut lines = Vec::new();
    for item in API_ITEMS {
        let expect = if item.ty.starts_with('(') || item.ty.starts_with('<') {
            "function"
        } else if item.ty == "number" {
            "number"
        } else if item.ty == "boolean" {
            "boolean"
        } else {
            "table"
        };
        lines.push(format!("check(\"{}\", function() return {} end, \"{expect}\")", item.path, access(item.path)));
    }
    let body = format!(
        "local missing = {{}}\nlocal function check(path: string, get: () -> any, expect: string)\n  local ok, v = pcall(get)\n  if not ok then table.insert(missing, path .. \": \" .. tostring(v)) elseif type(v) ~= expect then table.insert(missing, path .. \" is \" .. type(v) .. \", expected \" .. expect) end\nend\n{}\nreturn rsb.text(table.concat(missing, \"; \"))",
        lines.join("\n")
    );
    let mut h = Harness::new();
    h.user("widgets/names.luau", &widget(&body));
    h.start(&[(1, "names")]);
    assert_eq!(h.texts(1), vec![""], "{:?} {:?}", h.texts(1), h.plate(1));
    assert!(API_ITEMS.len() > 50);
}

fn access(path: &str) -> String {
    let mut out = String::from("(rsb :: any)");
    for seg in path.split('.') {
        out.push('.');
        out.push_str(seg);
    }
    out
}

#[test]
fn reserved_names_say_they_are_not_available_yet() {
    let mut h = Harness::new();
    let mut checks = String::new();
    for name in RESERVED {
        checks.push_str(&format!("t(\"{name}\", function() return (rsb :: any).{name} end)\n"));
    }
    let body = format!(
        "local out = {{}}\nlocal function t(n: string, f: () -> any) local ok, e = pcall(f) table.insert(out, `{{ok}}|{{tostring(e)}}`) end\n{checks}return rsb.text(table.concat(out, \"@@\"))"
    );
    h.user("widgets/res.luau", &widget(&body));
    h.start(&[(1, "res")]);
    let text = h.texts(1).join("");
    for (name, line) in RESERVED.iter().zip(text.split("@@")) {
        assert!(line.starts_with("false|"), "{name}: {line}");
        assert!(line.ends_with(&format!("rsb.{name} is not available in this version")), "{name}: {line}");
    }
    assert_eq!(text.split("@@").count(), RESERVED.len());
}

#[test]
fn hint_names_point_at_the_right_function_and_typos_get_a_suggestion() {
    let mut h = Harness::new();
    let mut checks = String::new();
    for (name, _) in HINTS {
        checks.push_str(&format!("t(function() return (rsb :: any).{name} end)\n"));
    }
    checks.push_str("t(function() return (rsb :: any).stat end)\n");
    checks.push_str("t(function() return (rsb :: any).timer.evry end)\n");
    checks.push_str("t(function() return (rsb :: any).services.audoi end)\n");
    checks.push_str("t(function() return (rsb :: any).services.audio.volum end)\n");
    checks.push_str("t(function() return (rsb :: any).zzzzzzzz end)\n");
    let body = format!(
        "local out = {{}}\nlocal function t(f: () -> any) local ok, e = pcall(f) table.insert(out, tostring(e)) end\n{checks}return rsb.text(table.concat(out, \"@@\"))"
    );
    h.user("widgets/hints.luau", &widget(&body));
    h.start(&[(1, "hints")]);
    let text = h.texts(1).join("");
    let lines: Vec<&str> = text.split("@@").collect();
    for (i, (name, to)) in HINTS.iter().enumerate() {
        assert!(lines[i].ends_with(&format!("rsb.{name} does not exist; use rsb.{to}")), "{}", lines[i]);
    }
    let n = HINTS.len();
    assert!(lines[n].ends_with("rsb.stat does not exist; did you mean rsb.state?"), "{}", lines[n]);
    assert!(lines[n + 1].ends_with("rsb.timer.evry does not exist; did you mean rsb.timer.every?"), "{}", lines[n + 1]);
    assert!(
        lines[n + 2].ends_with("rsb.services.audoi does not exist; did you mean rsb.services.audio?"),
        "{}",
        lines[n + 2]
    );
    assert!(
        lines[n + 3].ends_with("rsb.services.audio.volum does not exist; did you mean rsb.services.audio.volume?"),
        "{}",
        lines[n + 3]
    );
    assert!(lines[n + 4].ends_with("rsb.zzzzzzzz does not exist"), "{}", lines[n + 4]);
}

#[test]
fn reserved_props_and_unknown_props_raise_at_the_call() {
    let mut h = Harness::new();
    let mut checks = String::new();
    for p in RESERVED_PROPS {
        checks.push_str(&format!("t(function() return rsb.text {{ text = 'x', {p} = 1 }} end)\n"));
    }
    checks.push_str("t(function() return rsb.text { txt = 'x' } end)\n");
    checks.push_str("t(function() return rsb.text { color = 'red' } end)\n");
    checks.push_str("t(function() return rsb.row { 1, rsb.text 'a', {} } end)\n");
    checks.push_str("t(function() return rsb.text { on_click = 5 } end)\n");
    checks.push_str("t(function() return rsb.icon {} end)\n");
    checks.push_str("t(function() return rsb.image {} end)\n");
    checks.push_str("t(function() return rsb.text { 'child' } end)\n");
    let body = format!(
        "local out = {{}}\nlocal function t(f: () -> any) local ok, e = pcall(f) table.insert(out, tostring(e)) end\n{checks}return rsb.text(table.concat(out, \"@@\"))"
    );
    h.user("widgets/props.luau", &widget(&body));
    h.start(&[(1, "props")]);
    let text = h.texts(1).join("");
    let lines: Vec<&str> = text.split("@@").collect();
    for (i, p) in RESERVED_PROPS.iter().enumerate() {
        assert!(lines[i].ends_with(&format!("{p} is not available in this version")), "{}", lines[i]);
    }
    let n = RESERVED_PROPS.len();
    assert!(lines[n].ends_with("unknown prop 'txt' for rsb.text; did you mean 'text'?"), "{}", lines[n]);
    assert!(lines[n + 1].ends_with("style properties go in style = { color = ... }"), "{}", lines[n + 1]);
    assert!(lines[n + 2].contains("a child must be an element"), "{}", lines[n + 2]);
    assert!(lines[n + 3].ends_with("on_click must be a function"), "{}", lines[n + 3]);
    assert!(lines[n + 4].ends_with("rsb.icon needs a name or a glyph"), "{}", lines[n + 4]);
    assert!(lines[n + 5].ends_with("rsb.image needs a src"), "{}", lines[n + 5]);
    assert!(lines[n + 6].ends_with("rsb.text cannot have children"), "{}", lines[n + 6]);
}

#[test]
fn props_become_classes_style_ids_and_visibility() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/shape.luau",
        &widget(
            r##"
    local on = rsb.state(true)
    return rsb.column {
        id = "root",
        key = "k",
        class = { "a", "b" },
        style = { font_size = 14, color = "#fff", opacity = 0.5, font_weight = 600, padding = 4 },
        visible = function() return on.value end,
        rsb.icon "volume_high",
        rsb.icon { glyph = "X" },
        rsb.text { text = 5, class = function() return on.value and "x y" or "z" end },
        rsb.image { src = "logo.png", width = 16 },
        on_wheel = function() end,
        on_enter = function() end,
        on_leave = function() end,
        on_right_click = function() end,
        on_middle_click = function() end,
    }
"##,
        ),
    );
    h.start(&[(1, "my/shape")]);
    let r = h.root(1).unwrap();
    assert_eq!(r.direction, rsb_core::style::FlexDirection::Column);
    assert_eq!(r.id.as_deref(), Some("root"));
    assert_eq!(r.key.as_deref(), Some("k"));
    assert_eq!(r.classes.iter().map(|c| c.to_string()).collect::<Vec<_>>(), vec!["a", "b"]);
    let style: std::collections::BTreeMap<_, _> = r.style.iter().cloned().collect();
    assert_eq!(style["font-size"], "14px");
    assert_eq!(style["color"], "#fff");
    assert_eq!(style["opacity"], "0.5");
    assert_eq!(style["font-weight"], "600");
    assert_eq!(style["padding"], "4px");
    let pos = r.pos.as_ref().expect("inline style records where the element was made");
    assert_eq!(&*pos.file, "widgets/my/shape.luau");
    assert_eq!(pos.line, 7);
    assert!(r.handlers.wheel.is_some() && r.handlers.enter.is_some() && r.handlers.leave.is_some());
    assert!(r.handlers.right_click.is_some() && r.handlers.middle_click.is_some() && r.handlers.click.is_none());
    use rsb_core::element::{ElementKind, IconRef};
    assert_eq!(r.children[0].kind, ElementKind::Icon);
    assert_eq!(r.children[0].icon, Some(IconRef::Named("volume_high".into())));
    assert_eq!(r.children[1].icon, Some(IconRef::Glyph("X".into())));
    assert_eq!(r.children[2].text.as_deref(), Some("5"));
    assert_eq!(r.children[2].classes.iter().map(|c| c.to_string()).collect::<Vec<_>>(), vec!["x", "y"]);
    assert_eq!(r.children[3].kind, ElementKind::Image);
    let img = r.children[3].image.as_ref().unwrap();
    assert!(img.path.ends_with("logo.png") && img.path.contains("rsb"), "{}", img.path);
    assert_eq!(img.width, Some(16.0));
    assert!(r.children[2].pos.is_none(), "no inline style, no position");
}

#[test]
fn visible_false_hides_with_display_none() {
    let mut h = Harness::new();
    h.user(
        "widgets/vis.luau",
        &widget("local v = rsb.state(false) return rsb.row { visible = v, rsb.text 'x', on_click = function() v.value = true end }"),
    );
    h.start(&[(1, "vis")]);
    assert!(h.root(1).unwrap().style.iter().any(|(k, v)| k == "display" && v == "none"));
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert!(!h.root(1).unwrap().style.iter().any(|(k, _)| k == "display"));
}

#[test]
fn setup_returning_nil_draws_nothing() {
    let mut h = Harness::new();
    h.user("widgets/nothing.luau", &widget("return nil"));
    h.start(&[(1, "nothing")]);
    let t = h.latest.get(&rsb_core::element::InstanceId(1)).expect("a tree with no root is still sent");
    assert!(t.root.is_none());
    assert!(h.plate(1).is_none());
}

#[test]
fn a_function_child_list_is_rebuilt_and_each_keeps_item_state() {
    let mut h = Harness::new();
    h.user(
        "widgets/each.luau",
        &widget(
            r#"
    local list = rsb.state({ { id = "a", n = 1 }, { id = "b", n = 2 } })
    local renders = 0
    return rsb.row {
        rsb.each(list, function(item) renders += 1 rsb.log.info("render " .. item.id) return rsb.text { text = item.id .. item.n } end, function(item) return item.id end),
        rsb.text "end",
        on_click = function()
            list.value = { { id = "b", n = 2 }, { id = "c", n = 3 }, { id = "a", n = 9 } }
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "each")]);
    assert_eq!(h.texts(1), vec!["a1", "b2", "end"]);
    assert_eq!(log_lines("each: render").len(), 2);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(h.texts(1), vec!["b2", "c3", "a9", "end"]);
    let renders: Vec<String> = log_lines("each: render");
    // b unchanged: not rendered again; c new; a changed.
    assert_eq!(renders.len(), 4, "{renders:?}");
    assert!(renders[2].ends_with("render c") && renders[3].ends_with("render a"), "{renders:?}");
}

#[test]
fn function_props_rerun_only_their_own_element() {
    let mut h = Harness::new();
    h.user(
        "widgets/fp.luau",
        &widget(
            r#"
    local a, b = rsb.state(0), rsb.state(0)
    return rsb.row {
        rsb.text { text = function() rsb.log.info("run a") return tostring(a.value) end },
        rsb.text { text = function() rsb.log.info("run b") return tostring(b.value) end },
        on_click = function() a.value += 1 end,
    }
"#,
        ),
    );
    h.start(&[(1, "fp")]);
    assert_eq!(log_lines("fp: run a").len(), 1);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(log_lines("fp: run a").len(), 2);
    assert_eq!(log_lines("fp: run b").len(), 1, "the other element is untouched");
    assert_eq!(h.texts(1), vec!["1", "0"]);
}

#[test]
fn reading_a_signal_in_setup_warns_once() {
    let mut h = Harness::new();
    h.user(
        "widgets/warn.luau",
        &widget("local s = rsb.state(1) local v = s.value local w = s.value return rsb.text(tostring(v + w))"),
    );
    h.start(&[(1, "warn")]);
    assert_eq!(h.texts(1), vec!["2"]);
    let w = log_lines("warn: a signal was read in setup");
    assert_eq!(w.len(), 1, "{w:?}");
    assert!(w[0].contains("warn: "), "{}", w[0]);
}

#[test]
fn computed_values_cache_and_follow_their_sources() {
    let mut h = Harness::new();
    h.user(
        "widgets/comp.luau",
        &widget(
            r#"
    local a = rsb.state(2)
    local runs = 0
    local double = rsb.computed(function() runs += 1 return a.value * 2 end)
    local quad = rsb.computed(function() return double.value * 2 end)
    return rsb.row {
        rsb.text { text = function() return `{double.value} {quad.value} {double:peek()}` end },
        rsb.text { text = function() return tostring(runs) end },
        on_click = function() a.value = 5 end,
    }
"#,
        ),
    );
    h.start(&[(1, "comp")]);
    assert_eq!(h.texts(1), vec!["4 8 4", "1"]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(h.texts(1)[0], "10 20 10");
}

#[test]
fn effects_run_cleanups_and_the_owner_tree_keeps_one_timer() {
    let mut h = Harness::new();
    h.user(
        "widgets/owner.luau",
        &widget(
            r#"
    local a = rsb.state(0)
    local fired = rsb.state(0)
    local cleaned = rsb.state(0)
    rsb.effect(function()
        local _ = a.value
        rsb.timer.every("1s", function() fired.value += 1 end)
        return function() cleaned.value += 1 end
    end)
    return rsb.row {
        rsb.text { text = function() return `{fired.value} {cleaned.value}` end },
        on_click = function() for i = 1, 100 do a.value += 1 rsb.batch(function() end) end end,
    }
"#,
        ),
    );
    h.start(&[(1, "owner")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    // 100 separate writes in one handler are one effect run; do 100 clicks for 100 runs.
    for _ in 0..100 {
        h.click(1, id);
    }
    h.advance(1000);
    let t = h.texts(1)[0].clone();
    let (fired, cleaned) = t.split_once(' ').unwrap();
    assert_eq!(fired, "1", "one timer after 100 re-runs, not a pile: {t}");
    assert_eq!(cleaned, "100", "{t}");
}

/// Median wall time of one click turn (handler, effects, tree cut) on `src`.
fn median_click_turn(src: &str, name: &str, n: usize) -> std::time::Duration {
    let mut h = Harness::new();
    h.user(&format!("widgets/{name}.luau"), src);
    h.start(&[(1, name)]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    let mut times = Vec::new();
    for _ in 0..n {
        let t = std::time::Instant::now();
        h.rt.handle(rsb_core::msg::ScriptIn::Input(rsb_core::hit::HandlerCall {
            instance: rsb_core::element::InstanceId(1),
            handler: rsb_core::element::HandlerId(id),
            kind: rsb_core::hit::InputKind::Click(rsb_core::hit::Button::Left),
            x: 0.0,
            y: 0.0,
            gesture: 0,
        }));
        times.push(t.elapsed());
    }
    h.drain();
    times.sort();
    times[n / 2]
}

#[test]
fn fifty_bindings_update_in_under_a_hundred_microseconds_per_change() {
    // Same click, same tree size; only the 50 texts differ: 50 function props that read the
    // state, or 50 plain texts. The difference is what the bindings cost per change.
    let bound = widget(
        r#"
    local s = rsb.state(0)
    local kids = {}
    for i = 1, 50 do
        kids[i] = rsb.text { text = function() return tostring(s.value + i) end }
    end
    return rsb.row { rsb.row(kids), on_click = function() s.value += 1 end }
"#,
    );
    let plain = widget(
        r#"
    local s = rsb.state(0)
    local kids = {}
    for i = 1, 50 do
        kids[i] = rsb.text { text = tostring(i) }
    end
    kids[51] = rsb.text { text = function() return tostring(s.value) end }
    return rsb.row { rsb.row(kids), on_click = function() s.value += 1 end }
"#,
    );
    let n = 300;
    let with_bindings = median_click_turn(&bound, "bound", n);
    let without = median_click_turn(&plain, "plain", n);
    let per_change = with_bindings.saturating_sub(without);
    let build = if cfg!(debug_assertions) { "debug" } else { "release" };
    eprintln!(
        "50 bindings, one change ({build}, {n} runs, median): whole turn {with_bindings:?}; same tree with 1 binding {without:?}; 49 extra bindings cost {per_change:?}"
    );
    let bound = if cfg!(debug_assertions) {
        std::time::Duration::from_millis(5)
    } else {
        std::time::Duration::from_micros(100)
    };
    assert!(per_change < bound, "{per_change:?}");
}

#[test]
fn time_and_format_helpers() {
    let mut h = Harness::new();
    h.user(
        "widgets/t.luau",
        &widget(
            r#"
    local now = rsb.time.now()
    local utc = rsb.time.now("utc")
    local plus = rsb.time.now("+02:00")
    local bad, e = pcall(rsb.time.now, "Mars/Base")
    return rsb.column {
        rsb.text(rsb.time.format(now, "yyyy-MM-dd HH:mm:ss")),
        rsb.text(rsb.time.format(utc, "HH:mm")),
        rsb.text(rsb.time.format(plus, "HH:mm")),
        rsb.text(rsb.time.precision("HH:mm:ss") .. " " .. rsb.time.precision("ddd d MMM") .. " " .. rsb.time.precision("HH:mm")),
        rsb.text(`{bad}`),
        rsb.text(rsb.format.bytes(1536) .. " " .. rsb.format.rate(1048576) .. " " .. rsb.format.percent(23.4) .. " " .. rsb.format.bytes(12)),
    }
"#,
        ),
    );
    h.start(&[(1, "t")]);
    assert_eq!(
        h.texts(1),
        vec!["2026-11-24 09:05:07", "23:05", "01:05", "second hour minute", "false", "1.5 KB 1.0 MB/s 23% 12 B"],
        "{:?}",
        h.plate(1)
    );
}

#[test]
fn durations_take_seconds_or_unit_strings() {
    let mut h = Harness::new();
    h.user(
        "widgets/d.luau",
        &widget(
            r#"
    local out = {}
    for _, d in { 2, 0.5, "500ms", "2s", "1m", "1h", "x", -1, "", {} :: any } do
        local ok = pcall(rsb.timer.after, d :: any, function() end)
        table.insert(out, tostring(ok))
    end
    return rsb.text(table.concat(out, " "))
"#,
        ),
    );
    h.start(&[(1, "d")]);
    assert_eq!(h.texts(1), vec!["true true true true true true false false false false"], "{:?}", h.plate(1));
}

#[test]
fn context_and_props_reach_setup() {
    use rsb_core::value::Value;
    let mut h = Harness::new();
    h.user(
        "widgets/my/ctx.luau",
        &widget(
            r#"
    return rsb.column {
        rsb.text(`{ctx.id}|{ctx.name}|{ctx.bar}|{ctx.dir}`),
        rsb.text(`{ctx.monitor.index} {ctx.monitor.primary} {ctx.monitor.scale} {ctx.monitor.device}`),
    }
"#,
        ),
    );
    let _ = Value::Nil;
    h.start(&[(1, "my/ctx")]);
    assert_eq!(h.texts(1), vec!["my/ctx|my/ctx|main|widgets/my", "0 true 1 \\\\.\\DISPLAY1"], "{:?}", h.plate(1));
}

#[test]
fn a_text_element_draws_one_line() {
    let mut h = Harness::new();
    h.user(
        "widgets/ol.luau",
        &widget("return rsb.row { rsb.text('first\\r\\nsecond'), rsb.text('hi\\n'), rsb.text('\\nlead'), rsb.text('plain') }"),
    );
    h.start(&[(1, "ol")]);
    assert_eq!(h.texts(1), vec!["first", "hi", "", "plain"]);
}

#[test]
fn reading_inside_untrack_in_setup_does_not_warn_but_a_plain_read_does() {
    let mut h = Harness::new();
    h.user(
        "widgets/ut1.luau",
        &widget(
            "local s = rsb.state(1) local v = rsb.untrack(function() return s.value end) return rsb.text(tostring(v))",
        ),
    );
    h.user("widgets/ut2.luau", &widget("local s = rsb.state(1) return rsb.text(tostring(s.value))"));
    h.start(&[(1, "ut1"), (2, "ut2")]);
    assert_eq!(h.texts(1), vec!["1"]);
    assert!(log_lines("ut1: a signal was read in setup").is_empty(), "{:?}", log_lines("ut1:"));
    assert_eq!(log_lines("ut2: a signal was read in setup").len(), 1, "{:?}", log_lines("ut2:"));
}
