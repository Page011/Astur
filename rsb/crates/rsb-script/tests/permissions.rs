//! Permissions: what is granted and logged, what is denied and how it looks.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::io::{IoRequest, ProgramRef};
use rsb_core::msg::{HubRequest, ScriptIn, WidgetState};
use rsb_core::services::{AudioCommand, MediaCommand, ServiceCommand, ServiceId, WorkspaceCommand};
use rsb_core::value::Value;

fn widget(perms: &str, body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  options = {{ program = \"git\", other = \"x\" }},\n  permissions = {perms},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

fn click_right(h: &mut Harness, id: u32) {
    let right = h.root(id).unwrap().handlers.right_click.unwrap().0;
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: rsb_core::element::InstanceId(id),
        handler: rsb_core::element::HandlerId(right),
        kind: rsb_core::hit::InputKind::Click(rsb_core::hit::Button::Right),
        x: 0.0,
        y: 0.0,
    }));
}

#[test]
fn the_grant_is_logged_once_per_load_with_broad_programs_marked() {
    let mut h = Harness::new();
    h.user(
        "widgets/granted.luau",
        &widget(
            "{ net = { 'api.open-meteo.com' }, exec = { 'cmd', 'git' }, file = { '~/x.txt' }, control = { 'audio' } }",
            "return nil",
        ),
    );
    h.start(&[(1, "granted")]);
    let lines = log_lines("granted granted:");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(
        lines[0],
        "INFO granted granted: exec:cmd (can run any program), exec:git (can run any program), file:~/x.txt, net:api.open-meteo.com, control:audio, read:window, read:media, read:network"
    );
}

#[test]
fn builtins_get_exactly_what_they_declare_and_users_also_get_read() {
    let mut h = Harness::new();
    let src = widget("{}", "return rsb.text { text = function() return rsb.services.window.title end }");
    h.builtin("widgets/bwin.luau", &src);
    h.user("widgets/uwin.luau", &src);
    h.start(&[(1, "bwin"), (2, "uwin")]);
    let b = h.status();
    // A built-in that reads a private service without declaring it: the read is denied when the function prop runs;
    // that is a stop, not a logged error.
    assert!(matches!(b[0].state, WidgetState::Stopped(_)), "{:?}", b[0].state);
    assert_eq!(h.plate(1).unwrap(), "PERMISSION - bwin needs read:window; restarting");
    assert!(matches!(b[1].state, WidgetState::Running));
    assert!(!log_lines("permission denied: bwin needs read:window").is_empty());
    assert!(!log_lines("granted bwin: nothing").is_empty());
    // The plate says it restarts: it does.
    h.advance(1500);
    assert!(h.status().iter().find(|w| w.instance == InstanceId(1)).unwrap().restarts >= 1);
    let reqs = h.requests();
    assert!(reqs.iter().filter(|r| r.contains("Subscribe") && r.contains("Window")).count() == 1, "{reqs:?}");
}

#[test]
fn control_commands_need_control_and_send_the_right_command() {
    let mut h = Harness::new();
    h.user(
        "widgets/vol.luau",
        &widget(
            "{ control = { 'audio', 'media', 'workspaces' } }",
            r#"
    return rsb.row { rsb.text 'x',
        on_click = function()
            rsb.services.audio:change_volume(5)
            rsb.services.audio:set_volume(150)
            rsb.services.audio:set_muted(true)
            rsb.services.audio:toggle_mute()
            rsb.services.media:play_pause()
            rsb.services.media:next()
            rsb.services.workspaces:focus("2")
            rsb.services.workspaces:focus_next(1)
            rsb.services.workspaces:focus_previous()
        end,
        on_right_click = function()
            rsb.log.info(pcall(rsb.services.audio.change_volume, rsb.services.audio, "x"))
            rsb.log.info(pcall(rsb.services.audio.nothing))
        end,
    }
"#,
        ),
    );
    h.user(
        "widgets/novol.luau",
        &widget("{}", "return rsb.row { rsb.text 'x', on_click = function() rsb.services.audio:toggle_mute() end }"),
    );
    h.start(&[(1, "vol"), (2, "novol")]);
    h.take_requests();
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let cmds: Vec<ServiceCommand> = h
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            HubRequest::Command { owner, command } => {
                assert_eq!(owner.0, 1);
                Some(command)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        cmds,
        vec![
            ServiceCommand::Audio(AudioCommand::ChangeVolume(5.0)),
            ServiceCommand::Audio(AudioCommand::SetVolume(100.0)),
            ServiceCommand::Audio(AudioCommand::SetMute(true)),
            ServiceCommand::Audio(AudioCommand::ToggleMute),
            ServiceCommand::Media(MediaCommand::PlayPause),
            ServiceCommand::Media(MediaCommand::Next),
            ServiceCommand::Workspaces(WorkspaceCommand::Focus("2".into())),
            ServiceCommand::Workspaces(WorkspaceCommand::FocusNext(Some(1))),
            ServiceCommand::Workspaces(WorkspaceCommand::FocusPrevious(None)),
        ]
    );
    click_right(&mut h, 1);
    assert!(matches!(h.state_of(1), WidgetState::Running));
    h.click(2, first_click(h.root(2).unwrap()).unwrap());
    assert_eq!(h.plate(2).unwrap(), "PERMISSION - novol needs control:audio; add it to permissions");
    assert!(!log_lines("permission denied: novol needs control:audio").is_empty());
    assert!(h.take_requests().iter().all(|r| !matches!(r, HubRequest::Command { .. })), "no command reached the hub");
}

#[test]
fn interval_sends_a_subscribe_with_the_wish_and_replaces_it() {
    let mut h = Harness::new();
    h.user(
        "widgets/iv.luau",
        &widget(
            "{}",
            r#"
    rsb.services.system:interval("500ms")
    return rsb.row { rsb.text { text = function() return tostring(rsb.services.system.cpu) end },
        on_click = function() rsb.services.system:interval(5) end }
"#,
        ),
    );
    h.start(&[(1, "iv")]);
    let subs: Vec<Option<Duration>> = h
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            HubRequest::Subscribe { service: ServiceId::System, interval, .. } => Some(interval),
            _ => None,
        })
        .collect();
    assert_eq!(subs, vec![Some(Duration::from_millis(500))], "reading after the wish does not subscribe again");
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let subs: Vec<Option<Duration>> = h
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            HubRequest::Subscribe { interval, .. } => Some(interval),
            _ => None,
        })
        .collect();
    assert_eq!(subs, vec![Some(Duration::from_secs(5))]);
}

#[test]
fn an_unloaded_widget_cancels_everything_it_started() {
    let mut h = Harness::new();
    h.user(
        "widgets/cancel.luau",
        &widget("{}", "return rsb.text { text = function() return tostring(rsb.services.system.cpu) end }"),
    );
    h.start(&[(1, "cancel")]);
    h.take_requests();
    h.send(ScriptIn::SetInstances(vec![]));
    let reqs = h.take_requests();
    assert!(reqs.iter().any(|r| matches!(r, HubRequest::CancelOwner { owner } if owner.0 == 1)), "{reqs:?}");
}

#[test]
fn dollar_option_grants_only_what_the_user_wrote_in_bar_toml() {
    let mut h = Harness::new();
    let src = widget(
        "{ exec = { '$option:program' } }",
        "return rsb.row { rsb.text 'x', on_click = function() rsb.exec.run({ props.program, 'status' }) end }",
    );
    h.user("widgets/opt.luau", &src);
    let user_set = Harness::spec_with(1, "opt#a", &[("program", Value::Str("git".into()))]);
    let defaulted = Harness::spec(2, "opt#b");
    h.start_specs(vec![user_set, defaulted]);
    let granted = log_lines("granted opt#a:");
    assert!(granted[0].contains("exec:git") && !granted[0].contains("$option"), "{granted:?}");
    assert!(log_lines("opt#a: $option:program = git").len() == 1, "{:?}", log_lines("opt#a:"));
    let none = log_lines("granted opt#b:");
    assert!(!none[0].contains("exec:"), "a widget's own default never grants: {none:?}");
    h.take_requests();
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let reqs: Vec<_> = h
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            HubRequest::Io { request: IoRequest::ExecRun(s), .. } => Some(s),
            _ => None,
        })
        .collect();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].program, ProgramRef::Name("git".into()));
    h.click(2, first_click(h.root(2).unwrap()).unwrap());
    assert_eq!(h.plate(2).unwrap(), "PERMISSION - opt#b needs exec:git; add it to permissions");
}

#[test]
fn exec_program_names_and_paths_are_checked() {
    let mut h = Harness::new();
    let run = |cmd: &str| {
        widget(
            "{ exec = { 'git', 'C:/Tools/tool.exe' } }",
            &format!("return rsb.row {{ rsb.text 'x', on_click = function() rsb.exec.run({cmd}) end }}"),
        )
    };
    let cases: &[(&str, &str, Option<&str>)] = &[
        ("e1", "{ 'GIT.EXE', 'log' }", None),
        ("e2", "{ 'C:\\\\Tools\\\\tool.exe' }", None),
        ("e3", "{ 'powershell' }", Some("PERMISSION - e3 needs exec:powershell; add it to permissions")),
        (
            "e4",
            "{ 'C:\\\\Other\\\\tool.exe' }",
            Some("PERMISSION - e4 needs exec:C:/Other/tool.exe; add it to permissions"),
        ),
        ("e5", "{ 'git.bat' }", None),
        ("e6", "{ '..\\\\git' }", None),
        ("e7", "{}", None),
        ("e8", "{ 7 }", None),
    ];
    for (n, cmd, _) in cases {
        h.user(&format!("widgets/{n}.luau"), &run(cmd));
    }
    let list: Vec<(u32, &str)> = cases.iter().enumerate().map(|(i, c)| (i as u32 + 1, c.0)).collect();
    h.start(&list);
    h.take_requests();
    for (i, (n, _, plate)) in cases.iter().enumerate() {
        let id = i as u32 + 1;
        h.click(id, first_click(h.root(id).unwrap()).unwrap());
        if let Some(p) = plate {
            assert_eq!(h.plate(id).as_deref(), Some(*p), "{n}");
        }
    }
    let ran: Vec<_> = h
        .take_requests()
        .into_iter()
        .filter_map(|r| match r {
            HubRequest::Io { request: IoRequest::ExecRun(s), .. } => Some(s.program),
            _ => None,
        })
        .collect();
    assert_eq!(ran.len(), 2, "only e1 and e2 ran: {ran:?}");
    assert_eq!(ran[0], ProgramRef::Name("GIT.EXE".into()));
    // Refused for other reasons (batch file, path, empty, not a string): errors, no request.
    let st = h.status();
    for i in [4, 5, 6, 7] {
        assert_eq!(st[i].errors, 1, "e{}: {:?}", i + 1, st[i]);
    }
    assert!(log_lines("e5: error: ").iter().any(|l| l.contains("git.bat")), "{:?}", log_lines("e5: error"));
}

#[test]
fn a_denial_stops_the_instance_and_other_instances_keep_running() {
    let mut h = Harness::new();
    h.user("widgets/deny.luau", &widget("{}", "return rsb.row { rsb.text 'x', on_click = function() pcall(rsb.http.get, 'https://x.example/') rsb.log.info('after') end }"));
    h.user("widgets/fine.luau", &widget("{}", "local n = rsb.state(0) rsb.timer.every('1s', function() n.value += 1 end) return rsb.text { text = function() return tostring(n.value) end }"));
    h.start(&[(1, "deny"), (2, "fine")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    // pcall inside a task: the http call is not even inside a task here (handlers are tasks, so it is); the denial wins.
    assert_eq!(h.plate(1).unwrap(), "PERMISSION - deny needs net:x.example; add it to permissions");
    assert!(log_lines("deny: after").is_empty(), "pcall could not swallow the denial");
    h.advance(1000);
    assert_eq!(h.texts(2), vec!["1"]);
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    // A stopped user widget waits for its file: saving it starts it again.
    h.send(ScriptIn::FileChanged {
        rel_path: "widgets/deny.luau".into(),
        source: Some(widget("{ net = { 'x.example' } }", "return rsb.text 'fixed'")),
    });
    assert_eq!(h.texts(1), vec!["fixed"]);
    assert!(h.plate(1).is_none(), "the plate is told to clear: {:?}", h.plates.get(&rsb_core::element::InstanceId(1)));
}
