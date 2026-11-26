//! Service commands that need a user's click or key, the permissions the new services and the popup features
//! check, and what a widget sees when it breaks one.

mod common;

use common::*;
use rsb_core::msg::{HubRequest, WidgetState};
use rsb_core::services::{AudioDeviceCommand, ServiceCommand};

fn commands(h: &Harness) -> Vec<ServiceCommand> {
    h.hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Command { command, .. } => Some(command.clone()),
            _ => None,
        })
        .collect()
}

/// A widget that calls `call` from setup, an effect, a timer and (separately) a click, noting each error.
fn probe(perms: &str, call: &str) -> String {
    pwidget(
        perms,
        &format!(
            r#"
    local out = rsb.state("")
    local fire = rsb.state(0)
    local function try(where: string)
        local ok, e = pcall(function() {call} end)
        out.value = out:peek() .. where .. "=" .. (ok and "ok" or (string.gsub(tostring(e), "\n.*", ""))) .. "|"
    end
    try("setup")
    rsb.effect(function() if fire.value > 0 then try("effect") end end)
    rsb.timer.after("1s", function() fire.value = 1 try("timer") end)
    return rsb.row {{
        rsb.text {{ text = function() return out.value end }},
        on_click = function() {call} end,
    }}
"#
        ),
    )
}

#[test]
fn a_gesture_only_command_raises_in_setup_an_effect_and_a_timer_with_the_exact_message() {
    let mut h = Harness::new();
    h.user("widgets/c.luau", &probe("{ control = { 'audio' } }", "rsb.services.audio:set_default('speakers')"));
    h.start(&[(1, "c")]);
    h.advance(1100);
    h.advance(100);
    let text = h.texts(1).join("");
    let msg = "audio:set_default only works inside a click or key handler";
    for where_ in ["setup", "effect", "timer"] {
        let line = text.split('|').find(|l| l.starts_with(&format!("{where_}="))).unwrap_or_else(|| panic!("{text}"));
        assert!(line.ends_with(msg), "{line}");
    }
    assert!(commands(&h).is_empty(), "nothing reached the hub");
}

#[test]
fn a_gesture_only_command_runs_in_a_click_and_a_second_one_within_two_seconds_is_dropped() {
    let mut h = Harness::new();
    h.user("widgets/c.luau", &probe("{ control = { 'audio' } }", "rsb.services.audio:set_default('speakers')"));
    h.start(&[(1, "c")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    assert_eq!(commands(&h), vec![ServiceCommand::AudioDevice(AudioDeviceCommand::SetDefault("speakers".into()))]);
    h.click(1, click);
    assert_eq!(commands(&h).len(), 1, "one per two seconds");
    assert_eq!(log_lines("c: audio:set_default runs at most once every 2 seconds").len(), 1);
    h.advance(2100);
    h.click(1, click);
    assert_eq!(commands(&h).len(), 2, "and one more after the gap {:?} {:?}", log_lines("c:"), h.plate(1));
}

#[test]
fn a_click_without_an_input_id_is_not_a_gesture() {
    let mut h = Harness::new();
    h.user("widgets/c.luau", &probe("{ control = { 'audio' } }", "rsb.services.audio:set_default('speakers')"));
    h.start(&[(1, "c")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click_gesture(1, click, 0);
    assert!(commands(&h).is_empty());
}

#[test]
fn a_command_after_the_handler_waited_is_outside_the_gesture() {
    let mut h = Harness::new();
    h.user(
        "widgets/c.luau",
        &pwidget(
            "{ control = { 'audio' } }",
            r#"
    return rsb.row {
        rsb.text "x",
        on_click = function()
            rsb.task.sleep("100ms")
            rsb.services.audio:set_default("speakers")
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "c")]);
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    h.advance(200);
    assert!(commands(&h).is_empty());
    assert_eq!(h.status()[0].errors, 1, "the late call raised inside the task");
}

#[test]
fn device_commands_check_their_ids_and_volume_needs_no_click() {
    let mut h = Harness::new();
    h.user(
        "widgets/c.luau",
        &pwidget(
            "{ control = { 'audio' } }",
            r#"
    local out = {}
    local a = rsb.services.audio
    a:set_device_volume("dev-1", 140)
    a:set_device_muted("dev-1", true)
    for _, id in { "", string.rep("x", 1025), "a\nb", 5 } do
        local ok, e = pcall(function() a:set_device_volume(id, 10) end)
        table.insert(out, (string.gsub(tostring(e), "\n.*", "")))
    end
    local ok, e = pcall(function() a:set_device_muted("dev-1", "yes") end)
    table.insert(out, (string.gsub(tostring(e), "\n.*", "")))
    return rsb.text(table.concat(out, "@@"))
"#,
        ),
    );
    h.start(&[(1, "c")]);
    assert_eq!(
        commands(&h),
        vec![
            ServiceCommand::AudioDevice(AudioDeviceCommand::SetVolume("dev-1".into(), 100.0)),
            ServiceCommand::AudioDevice(AudioDeviceCommand::SetMuted("dev-1".into(), true)),
        ]
    );
    let text = h.texts(1).join("");
    let lines: Vec<&str> = text.split("@@").collect();
    assert!(lines.len() == 5, "{text} {:?}", h.plate(1));
    for l in &lines[..3] {
        assert!(l.ends_with("no device with that id"), "{l}");
    }
    assert!(lines[3].ends_with("id must be a string, got number"), "{}", lines[3]);
    assert!(lines[4].ends_with("muted must be a boolean, got string"), "{}", lines[4]);
}

/// (the call, what the widget lacks) for every new service and command.
const DENIED: &[(&str, &str)] = &[
    ("rsb.services.microphone:toggle_mute()", "control:microphone"),
    ("rsb.services.microphone:set_muted(true)", "control:microphone"),
    ("rsb.services.bluetooth:set_enabled(true)", "control:bluetooth"),
    ("rsb.services.bluetooth:open_settings()", "control:bluetooth"),
    ("rsb.services.bluetooth:rescan()", "control:bluetooth"),
    ("rsb.services.apps:launch('x')", "control:apps"),
    ("rsb.services.apps:open_uri('ms-settings:bluetooth')", "control:apps"),
    ("rsb.services.power:lock()", "control:power"),
    ("rsb.services.power:set_mode('balanced')", "control:power"),
    ("rsb.services.power:sleep()", "control:power"),
    ("rsb.services.power:restart()", "control:session"),
    ("rsb.services.power:shut_down()", "control:session"),
    ("rsb.services.power:sign_out()", "control:session"),
    ("rsb.services.wifi:scan()", "control:wifi"),
    ("rsb.services.wifi:connect('home')", "control:wifi"),
    ("rsb.services.audio:set_default('x')", "control:audio"),
    ("rsb.services.audio:set_device_volume('x', 5)", "control:audio"),
    ("rsb.services.media:seek(10)", "control:media"),
    ("local _ = rsb.services.wifi.networks", "read:wifi"),
    ("local _ = rsb.services.bluetooth.devices", "read:bluetooth"),
    ("local _ = rsb.services.apps.apps", "read:apps"),
];

#[test]
fn every_new_command_and_private_read_is_denied_without_its_permission_with_the_exact_plate() {
    for (call, need) in DENIED {
        let mut h = Harness::new();
        h.user("widgets/c.luau", &pwidget("{}", &format!("{call}\n    return rsb.text 'x'")));
        h.start(&[(1, "c")]);
        assert_eq!(
            h.plate(1).as_deref(),
            Some(format!("PERMISSION - c needs {need}; add it to permissions").as_str()),
            "{call}"
        );
        assert!(commands(&h).is_empty(), "{call}");
        assert!(matches!(h.state_of(1), WidgetState::Stopped(_)), "{call}");
    }
}

#[test]
fn a_widget_that_may_lock_cannot_end_the_session_and_the_denial_is_logged() {
    // Declared control:audio only: the session actions name their own permission whatever else is held.
    let mut h = Harness::new();
    h.user(
        "widgets/c.luau",
        &pwidget("{ control = { 'audio', 'media' } }", "rsb.services.power:restart()\n    return rsb.text 'x'"),
    );
    h.start(&[(1, "c")]);
    assert_eq!(h.plate(1).as_deref(), Some("PERMISSION - c needs control:session; add it to permissions"));
    assert!(log_lines("permission denied: c needs control:session").len() == 1);
}

#[test]
fn a_service_image_key_needs_the_read_permission_of_the_service_that_made_it() {
    let mut h = Harness::new();
    h.user(
        "widgets/c.luau",
        &pwidget(
            "{}",
            r#"return rsb.row { rsb.image { key = "m", src = "rsb-image:media/12", width = 20 }, rsb.image { key = "a", src = "rsb-image:apps/abc" } }"#,
        ),
    );
    h.start(&[(1, "c")]);
    // media is readable by default; the app list is private.
    assert_eq!(h.plate(1).as_deref(), Some("PERMISSION - c needs read:apps; add it to permissions"));

    let mut h = Harness::new();
    h.user(
        "widgets/my/c.luau",
        &pwidget("{}", r#"return rsb.row { rsb.image { src = "rsb-image:media/12", width = 20 }, rsb.image { src = "logo.svg" } }"#),
    );
    h.start(&[(1, "my/c")]);
    let root = h.root(1).unwrap();
    let key = root.children[0].image.as_ref().unwrap();
    assert_eq!(&*key.path, "rsb-image:media/12");
    assert_eq!(key.width, Some(20.0));
    let svg = root.children[1].image.as_ref().unwrap();
    assert!(svg.path.ends_with("logo.svg"), "an svg path is resolved like any image path: {}", svg.path);
}

#[test]
fn an_image_key_for_no_service_is_an_error_not_a_path() {
    let mut h = Harness::new();
    h.user("widgets/c.luau", &pwidget("{}", r#"return rsb.image { src = "rsb-image:nothing/1" }"#));
    h.start(&[(1, "c")]);
    assert!(h.root(1).is_none(), "no tree was made");
    assert_eq!(log_lines("c: error: rsb.image: the image key names no service").len(), 1);
}

// What the permission checks allow for the services that need a declared grant.

#[test]
fn declared_permissions_let_the_new_commands_through_and_session_actions_need_a_click() {
    let mut h = Harness::new();
    h.user(
        "widgets/c.luau",
        &probe(
            "{ control = { 'power', 'session', 'microphone', 'bluetooth', 'apps', 'wifi', 'media' }, read = { 'apps', 'wifi', 'bluetooth' } }",
            "rsb.services.power:restart()",
        ),
    );
    h.start(&[(1, "c")]);
    h.advance(1100);
    h.advance(100);
    let text = h.texts(1).join("");
    assert!(text.contains("setup=") && text.contains("only works inside a click or key handler"), "{text}");
    assert!(commands(&h).is_empty());
    let click = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, click);
    assert_eq!(commands(&h).len(), 1);

    let mut h = Harness::new();
    h.user(
        "widgets/m.luau",
        &pwidget(
            "{ control = { 'microphone', 'media', 'wifi', 'power' }, read = { 'wifi' } }",
            r#"
    rsb.services.microphone:toggle_mute()
    rsb.services.microphone:set_muted(true)
    rsb.services.media:seek(12.5)
    rsb.services.wifi:scan()
    rsb.services.power:lock()
    rsb.services.power:set_mode("performance")
    local _ = rsb.services.wifi.networks
    return rsb.text "x"
"#,
        ),
    );
    h.start(&[(1, "m")]);
    assert_eq!(commands(&h).len(), 6, "{:?}", h.plate(1));
    assert!(commands(&h).iter().any(|c| format!("{c:?}").contains("Seek(12500)")));
}
