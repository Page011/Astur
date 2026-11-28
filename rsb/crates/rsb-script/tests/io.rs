//! I/O from widgets: calls that wait inside tasks, resumed by the hub's
//! replies; every request carries the widget's grant.

mod common;

use std::time::Duration;

use common::*;
use rsb_core::io::{ExecResult, HttpMethod, HttpResponse, IoError, IoEventKind, IoId, IoRequest, ProgramRef};
use rsb_core::msg::{HubRequest, ScriptIn, WidgetState};

fn widget(perms: &str, body: &str) -> String {
    format!(
        "local rsb = require(\"@rsb\")\nreturn rsb.widget {{\n  permissions = {perms},\n  setup = function(props, ctx)\n{body}\n  end,\n}}\n"
    )
}

/// The `Io` requests among the recorded hub requests: (id, owner, request, grant).
fn io_requests(h: &Harness) -> Vec<(IoId, u32, IoRequest, std::sync::Arc<rsb_core::io::IoGrant>)> {
    h.hub
        .lock()
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            HubRequest::Io { id, owner, request, grant } => Some((*id, owner.0, request.clone(), grant.clone())),
            _ => None,
        })
        .collect()
}

fn ok_exec(stdout: &str) -> IoEventKind {
    IoEventKind::ExecDone(Ok(ExecResult {
        code: Some(0),
        stdout: stdout.into(),
        stderr: String::new(),
        timed_out: false,
        truncated: false,
    }))
}

#[test]
fn exec_run_waits_in_a_task_and_resumes_with_the_result() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/git.luau",
        &widget(
            "{ exec = { \"git\" } }",
            r#"
    local out = rsb.state("none")
    return rsb.row {
        rsb.text { text = function() return out.value end },
        on_click = function()
            local r = rsb.exec.run({ "git", "status", "--short" }, { timeout = "5s", cwd = "repo", max_output = 1000 })
            out.value = `{r.ok} {r.code} {r.stdout} {r.timed_out}`
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "my/git")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    assert_eq!(h.texts(1), vec!["none"], "the task is waiting");
    let reqs = io_requests(&h);
    assert_eq!(reqs.len(), 1);
    let (io_id, owner, request, grant) = &reqs[0];
    assert_eq!(*owner, 1);
    let IoRequest::ExecRun(spec) = request else { panic!("{request:?}") };
    assert_eq!(spec.program, ProgramRef::Name("git".into()));
    assert_eq!(spec.args, vec!["status", "--short"]);
    assert_eq!(spec.timeout, Some(Duration::from_secs(5)));
    assert_eq!(spec.max_output, 1000);
    assert!(spec.raw_tail.is_none());
    let cwd = spec.cwd.as_ref().unwrap().to_string_lossy().replace('\\', "/");
    assert!(cwd.ends_with("rsb/widgets/my/repo"), "{cwd}");
    assert_eq!(grant.widget, "my/git");
    assert!(grant.permissions.exec.contains(&"git".to_string()));
    assert!(grant.widget_dir.to_string_lossy().ends_with("rsb\\widgets\\my"), "{:?}", grant.widget_dir);
    h.io_reply(*io_id, 1, ok_exec(" M a.txt"));
    assert_eq!(h.texts(1), vec!["true 0  M a.txt false"]);
}

#[test]
fn a_dot_working_folder_is_the_widgets_own_folder() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/dot.luau",
        &widget(
            "{ exec = { \"git\" } }",
            r#"
    return rsb.row {
        rsb.text 'x',
        on_click = function()
            rsb.exec.run({ "git", "status" }, { cwd = "." })
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "my/dot")]);
    let id = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, id);
    let reqs = io_requests(&h);
    let IoRequest::ExecRun(first) = &reqs[0].2 else { panic!("{:?}", reqs[0].2) };
    let cwd = first.cwd.as_ref().expect("cwd = \".\" is the widget folder").to_string_lossy().replace('\\', "/");
    assert!(cwd.ends_with("rsb/widgets/my"), "{cwd}");
}

#[test]
fn exec_shell_passes_the_line_through_unchanged_and_needs_cmd() {
    let mut h = Harness::new();
    h.user(
        "widgets/sh.luau",
        &widget(
            "{ exec = { \"cmd\" } }",
            "return rsb.row { rsb.text 'x', on_click = function() rsb.exec.shell('echo \"a b\" | find \"a\"') end }",
        ),
    );
    h.user(
        "widgets/nosh.luau",
        &widget("{}", "return rsb.row { rsb.text 'x', on_click = function() rsb.exec.shell('ver') end }"),
    );
    h.start(&[(1, "sh"), (2, "nosh")]);
    let c1 = first_click(h.root(1).unwrap()).unwrap();
    h.click(1, c1);
    let reqs = io_requests(&h);
    let IoRequest::ExecRun(spec) = &reqs[0].2 else { panic!() };
    assert_eq!(spec.program, ProgramRef::Name("cmd".into()));
    assert_eq!(spec.raw_tail.as_deref(), Some("echo \"a b\" | find \"a\""));
    let c2 = first_click(h.root(2).unwrap()).unwrap();
    h.click(2, c2);
    assert_eq!(h.plate(2).unwrap(), "PERMISSION - nosh needs exec:cmd; add it to permissions");
}

#[test]
fn waiting_calls_outside_a_task_say_where_they_can_be_used() {
    let mut h = Harness::new();
    let want = "waits for a result; call it inside rsb.task.spawn, rsb.poll, a timer or a handler";
    h.user("widgets/w1.luau", &widget("{ exec = { \"git\" } }", "rsb.exec.run({ 'git' }) return nil"));
    h.user(
        "widgets/w2.luau",
        &widget(
            "{ net = { 'example.com' } }",
            "return rsb.text { text = function() return rsb.http.get('https://example.com').body end }",
        ),
    );
    h.user(
        "widgets/w3.luau",
        &widget(
            "{}",
            "local c = rsb.computed(function() rsb.task.sleep(1) return 1 end) return rsb.text(tostring(c.value))",
        ),
    );
    h.user("widgets/w4.luau", &widget("{}", "rsb.effect(function() rsb.file.read('x.txt') end) return nil"));
    h.start(&[(1, "w1"), (2, "w2"), (3, "w3"), (4, "w4")]);
    assert!(h.plate(1).unwrap().contains(&format!("rsb.exec.run {want}")), "{:?}", h.plate(1));
    // Errors in function props, computeds and effects are reported and the widget keeps running.
    let st = h.status();
    assert!(st[1].errors >= 1 && st[2].errors >= 1 && st[3].errors >= 1, "{st:?}");
    assert!(
        log_lines("w2: error: ").iter().any(|l| l.contains(&format!("rsb.http.get {want}"))),
        "{:?}",
        log_lines("w2: error")
    );
    assert!(log_lines("w3: error: ").iter().any(|l| l.contains(&format!("rsb.task.sleep {want}"))));
    assert!(log_lines("w4: error: ").iter().any(|l| l.contains(&format!("rsb.file.read {want}"))));
    assert!(io_requests(&h).is_empty(), "nothing was sent");
}

#[test]
fn http_get_and_post_resume_with_a_response() {
    let mut h = Harness::new();
    h.user(
        "widgets/web.luau",
        &widget(
            "{ net = { 'api.example.com', 'localhost:3000' } }",
            r#"
    local out = rsb.state("-")
    return rsb.row {
        rsb.text { text = function() return out.value end },
        on_click = function()
            local r = rsb.http.get("https://api.example.com/v1/x?y=1", { headers = { Accept = "application/json" }, cache = "10m", timeout = 3 })
            local data = r:json() :: any
            out.value = `{r.ok} {r.status} {data.a[2]} {r.headers["content-type"]} {r.from_cache}`
        end,
        on_right_click = function()
            local r = rsb.http.post("http://localhost:3000/hook", { n = 1 })
            out.value = `{r.ok} {r.status} {r.error}`
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "web")]);
    let root = h.root(1).unwrap().clone();
    h.click(1, first_click(&root).unwrap());
    let reqs = io_requests(&h);
    let IoRequest::Http(req) = &reqs[0].2 else { panic!() };
    assert_eq!(req.method, HttpMethod::Get);
    assert_eq!(req.url, "https://api.example.com/v1/x?y=1");
    assert_eq!(req.headers, vec![("Accept".to_string(), "application/json".to_string())]);
    assert_eq!(req.timeout, Duration::from_secs(3));
    assert_eq!(req.cache_for, Some(Duration::from_secs(600)));
    assert_eq!(req.max_bytes, rsb_core::io::limits::HTTP_DEFAULT_BYTES);
    assert!(req.body.is_none());
    h.io_reply(
        reqs[0].0,
        1,
        IoEventKind::Http(Ok(HttpResponse {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            body: br#"{"a":[1,2]}"#.to_vec(),
            from_cache: true,
        })),
    );
    assert_eq!(h.texts(1), vec!["true 200 2 application/json true"]);

    h.take_requests();
    let right = h.root(1).unwrap().handlers.right_click.unwrap().0;
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: rsb_core::element::InstanceId(1),
        handler: rsb_core::element::HandlerId(right),
        kind: rsb_core::hit::InputKind::Click(rsb_core::hit::Button::Right),
        x: 0.0,
        y: 0.0,
        gesture: 1,
    }));
    let reqs = io_requests(&h);
    let IoRequest::Http(req) = &reqs[0].2 else { panic!() };
    assert_eq!(req.method, HttpMethod::Post);
    assert_eq!(req.body.as_deref(), Some(&b"{\"n\":1}"[..]));
    assert!(req.headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
    h.io_reply(reqs[0].0, 1, IoEventKind::Http(Err(IoError::Timeout)));
    assert_eq!(h.texts(1), vec!["false 0 timed out"]);
}

#[test]
fn http_is_checked_against_net_before_anything_is_sent() {
    let mut h = Harness::new();
    let call = |url: &str| {
        widget(
            "{ net = { 'api.example.com' } }",
            &format!("return rsb.row {{ rsb.text 'x', on_click = function() rsb.http.get('{url}') end }}"),
        )
    };
    h.user("widgets/h1.luau", &call("https://evil.example/x"));
    h.user("widgets/h2.luau", &call("http://api.example.com/x"));
    h.user("widgets/h3.luau", &call("https://user@api.example.com/x"));
    h.user("widgets/h4.luau", &call("https://api.example.com:8443/x"));
    h.start(&[(1, "h1"), (2, "h2"), (3, "h3"), (4, "h4")]);
    for id in 1..=4 {
        let c = first_click(h.root(id).unwrap()).unwrap();
        h.click(id, c);
    }
    assert_eq!(h.plate(1).unwrap(), "PERMISSION - h1 needs net:evil.example; add it to permissions");
    assert!(!log_lines("permission denied: h1 needs net:evil.example").is_empty());
    // Plain http to a non-loopback host and user info in a URL are refused outright (an error, not a missing permission).
    let st = h.status();
    assert!(st[1].errors == 1 && st[2].errors == 1, "{st:?}");
    assert!(h.plate(2).is_none() && h.plate(3).is_none());
    assert!(log_lines("h2: error: ").iter().any(|l| l.contains("http://")), "{:?}", log_lines("h2:"));
    assert_eq!(h.plate(4).unwrap(), "PERMISSION - h4 needs net:api.example.com:8443; add it to permissions");
    assert!(io_requests(&h).is_empty(), "nothing reached the hub");
}

#[test]
fn at_most_four_requests_per_widget_are_in_flight() {
    let mut h = Harness::new();
    h.user(
        "widgets/many.luau",
        &widget(
            "{ net = { 'api.example.com' } }",
            r#"
    local out = rsb.state("")
    return rsb.row {
        rsb.text { text = function() return out.value end },
        on_click = function()
            for i = 1, 6 do
                rsb.task.spawn(function()
                    local r = rsb.http.get("https://api.example.com/" .. i)
                    out.value ..= `{i}:{if r.ok then "ok" else r.error} `
                end)
            end
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "many")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    assert_eq!(io_requests(&h).len(), 4);
    h.run_due();
    let t = h.texts(1)[0].clone();
    assert!(t.contains("5:too many at once") && t.contains("6:too many at once"), "{t}");
}

#[test]
fn file_read_returns_text_or_nil_and_a_reason() {
    let mut h = Harness::new();
    h.user(
        "widgets/my/rd2.luau",
        &widget(
            "{ file = { '~/notes.txt' } }",
            r#"
    local out = rsb.state("")
    return rsb.row {
        rsb.text { text = function() return out.value end },
        on_click = function()
            local a, ea = rsb.file.read("~/notes.txt", { max_bytes = 100 })
            local b, eb = rsb.file.read("data.txt")
            out.value = `{a} {ea} | {b} {eb}`
        end,
    }
"#,
        ),
    );
    h.start(&[(1, "my/rd2")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let first_req = io_requests(&h)[0].clone();
    let IoRequest::ReadFile(f0) = &first_req.2 else { panic!() };
    assert_eq!(f0.path.to_string_lossy(), r"C:\Users\tester\notes.txt");
    assert_eq!(f0.max_bytes, 100);
    let first = first_req.0;
    h.io_reply(first, 1, IoEventKind::File(Ok("hello".into())));
    let reqs = io_requests(&h);
    assert_eq!(reqs.len(), 2, "the second read went out when the task resumed");
    let IoRequest::ReadFile(f) = &reqs[1].2 else { panic!() };
    assert!(f.path.to_string_lossy().ends_with("rsb\\widgets\\my\\data.txt"), "{:?}", f.path);
    h.io_reply(reqs[1].0, 1, IoEventKind::File(Err(IoError::NotFound)));
    assert_eq!(h.texts(1), vec!["hello nil | nil not found"]);
}

#[test]
fn file_paths_outside_the_grant_are_denied_with_the_path_as_written() {
    let mut h = Harness::new();
    let read = |p: &str| {
        widget(
            "{ file = { '~/notes.txt' } }",
            &format!("return rsb.row {{ rsb.text 'x', on_click = function() rsb.file.read('{p}') end }}"),
        )
    };
    let cases: &[(&str, &str, &str)] = &[
        ("d1", "~/other.txt", "file:~/other.txt"),
        ("d2", "../../../secrets.txt", "file:~/AppData/Roaming/secrets.txt"),
        ("d3", "\\\\\\\\server\\\\share\\\\x", "file:"),
        ("d4", "C:/Windows/win.ini", "file:C:/Windows/win.ini"),
        ("d5", "~/notes.txt:stream", "file:"),
    ];
    for (name, p, _) in cases {
        h.user(&format!("widgets/my/{name}.luau"), &read(p));
    }
    let names: Vec<String> = cases.iter().map(|(n, ..)| format!("my/{n}")).collect();
    let widgets: Vec<(u32, &str)> = names.iter().enumerate().map(|(i, n)| (i as u32 + 1, n.as_str())).collect();
    h.start(&widgets);
    for (i, (name, _, need)) in cases.iter().enumerate() {
        let id = i as u32 + 1;
        h.click(id, first_click(h.root(id).unwrap()).unwrap());
        let plate = h.plate(id).unwrap_or_else(|| panic!("{name}: no plate"));
        assert!(plate.starts_with(&format!("PERMISSION - my/{name} needs {need}")), "{plate}");
        assert!(plate.ends_with("; add it to permissions"), "{plate}");
    }
    assert!(io_requests(&h).is_empty());
}

#[test]
fn file_watch_keeps_a_signal_current() {
    let mut h = Harness::new();
    h.user(
        "widgets/watch.luau",
        &widget(
            "{ file = { '~/count.txt' } }",
            r#"
    local f = rsb.file.watch("~/count.txt")
    return rsb.text { text = function() return `{f.value} {f.error}` end }
"#,
        ),
    );
    h.start(&[(1, "watch")]);
    assert_eq!(h.texts(1), vec!["nil nil"]);
    let reqs = io_requests(&h);
    assert!(matches!(reqs[0].2, IoRequest::WatchFile(_)));
    h.io_reply(reqs[0].0, 1, IoEventKind::File(Ok("5".into())));
    assert_eq!(h.texts(1), vec!["5 nil"]);
    h.io_reply(reqs[0].0, 1, IoEventKind::File(Err(IoError::NotFound)));
    assert_eq!(h.texts(1), vec!["nil not found"]);
    h.io_reply(reqs[0].0, 1, IoEventKind::File(Ok("6".into())));
    assert_eq!(h.texts(1), vec!["6 nil"]);
    // Unloading cancels the watch.
    h.take_requests();
    h.send(ScriptIn::SetInstances(vec![]));
    let reqs = h.take_requests();
    assert!(reqs.iter().any(|r| matches!(r, HubRequest::CancelOwner { owner } if owner.0 == 1)), "{reqs:?}");
}

#[test]
fn spawned_processes_stream_lines_and_report_the_exit() {
    let mut h = Harness::new();
    h.user(
        "widgets/sp.luau",
        &widget(
            "{ exec = { 'ping' } }",
            r#"
    local out = rsb.state("")
    local p = rsb.exec.spawn({ "ping", "-t", "x" }, {
        on_line = function(line, stream) out.value ..= `[{stream}:{line}]` end,
        on_exit = function(code) out.value ..= `<exit {code}>` end,
    })
    local last = rsb.exec.lines({ "ping", "y" })
    return rsb.row { rsb.text { text = function() return out.value end }, rsb.text { text = function() return tostring(last.value) end } }
"#,
        ),
    );
    h.start(&[(1, "sp")]);
    let reqs = io_requests(&h);
    assert_eq!(reqs.len(), 2);
    let (a, b) = (reqs[0].0, reqs[1].0);
    assert!(matches!(&reqs[0].2, IoRequest::ExecSpawn(s) if s.timeout.is_none() && s.args == ["-t", "x"]));
    h.io_reply(a, 1, IoEventKind::ExecLine { stderr: false, line: "one".into() });
    h.io_reply(a, 1, IoEventKind::ExecLine { stderr: true, line: "two".into() });
    h.io_reply(b, 1, IoEventKind::ExecLine { stderr: false, line: "feed".into() });
    h.io_reply(a, 1, ok_exec(""));
    assert_eq!(h.texts(1), vec!["[stdout:one][stderr:two]<exit 0>", "feed"]);
}

#[test]
fn a_denial_found_only_by_the_hub_stops_the_widget_with_the_same_plate() {
    let mut h = Harness::new();
    h.user(
        "widgets/redir.luau",
        &widget(
            "{ net = { 'api.example.com' } }",
            "return rsb.row { rsb.text 'x', on_click = function() rsb.http.get('https://api.example.com/r') end }",
        ),
    );
    h.start(&[(1, "redir")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let id = io_requests(&h)[0].0;
    h.io_reply(id, 1, IoEventKind::Http(Err(IoError::NotPermitted("net:evil.example".into()))));
    assert_eq!(h.plate(1).unwrap(), "PERMISSION - redir needs net:evil.example; add it to permissions");
    assert!(matches!(h.state_of(1), WidgetState::Stopped(_)));
    assert!(!log_lines("permission denied: redir needs net:evil.example").is_empty());
}

#[test]
fn open_url_needs_https_and_net() {
    let mut h = Harness::new();
    h.user(
        "widgets/open.luau",
        &widget(
            "{ net = { 'open-meteo.com' } }",
            "return rsb.row { rsb.text 'x', on_click = function() rsb.open_url('https://open-meteo.com') end, on_right_click = function() pcall(rsb.open_url, 'http://open-meteo.com') rsb.open_url('https://elsewhere.example') end }",
        ),
    );
    h.start(&[(1, "open")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let reqs = io_requests(&h);
    assert!(matches!(&reqs[0].2, IoRequest::OpenUrl(u) if u == "https://open-meteo.com"));
    h.take_requests();
    let right = h.root(1).unwrap().handlers.right_click.unwrap().0;
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: rsb_core::element::InstanceId(1),
        handler: rsb_core::element::HandlerId(right),
        kind: rsb_core::hit::InputKind::Click(rsb_core::hit::Button::Right),
        x: 0.0,
        y: 0.0,
        gesture: 1,
    }));
    assert!(io_requests(&h).is_empty());
    assert_eq!(h.plate(1).unwrap(), "PERMISSION - open needs net:elsewhere.example; add it to permissions");
}

#[test]
fn poll_runs_now_and_every_period_and_keeps_the_last_value_on_error() {
    let mut h = Harness::new();
    h.user(
        "widgets/poll.luau",
        &widget(
            "{ net = { 'api.example.com' } }",
            r#"
    local p = rsb.poll("5m", function()
        local r = rsb.http.get("https://api.example.com/t")
        if not r.ok then error("http " .. r.status) end
        return r.body
    end)
    return rsb.text { text = function() return `{p.value} / {p.error}` end }
"#,
        ),
    );
    h.start(&[(1, "poll")]);
    assert_eq!(h.texts(1), vec!["nil / nil"]);
    let reply = |h: &mut Harness, status: u16, body: &str| {
        let id = io_requests(h).last().unwrap().0;
        h.take_requests();
        h.io_reply(
            id,
            1,
            IoEventKind::Http(Ok(HttpResponse {
                status,
                headers: vec![],
                body: body.as_bytes().to_vec(),
                from_cache: false,
            })),
        );
    };
    reply(&mut h, 200, "21");
    assert_eq!(h.texts(1), vec!["21 / nil"]);
    h.advance(5 * 60 * 1000);
    assert_eq!(io_requests(&h).len(), 1, "the timer started the next run");
    reply(&mut h, 500, "");
    let t = h.texts(1)[0].clone();
    assert!(t.starts_with("21 / ") && t.contains("http 500"), "{t}");
    assert!(matches!(h.state_of(1), WidgetState::Running));
    h.advance(5 * 60 * 1000);
    reply(&mut h, 200, "22");
    assert_eq!(h.texts(1), vec!["22 / nil"]);
}

#[test]
fn sleep_spawn_and_cancel() {
    let mut h = Harness::new();
    h.user(
        "widgets/tasks.luau",
        &widget(
            "{}",
            r#"
    local out = rsb.state("")
    local t1
    return rsb.row {
        rsb.text { text = function() return out.value end },
        on_click = function()
            t1 = rsb.task.spawn(function() rsb.task.sleep("2s") out.value ..= "a" end)
            rsb.task.spawn(function() rsb.task.sleep(1) out.value ..= "b" end)
            rsb.task.spawn(function() rsb.task.sleep(3) out.value ..= "c" end)
        end,
        on_right_click = function() rsb.task.cancel(t1) t1:cancel() end,
    }
"#,
        ),
    );
    h.start(&[(1, "tasks")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    h.advance(1000);
    assert_eq!(h.texts(1), vec!["b"]);
    let right = h.root(1).unwrap().handlers.right_click.unwrap().0;
    h.send(ScriptIn::Input(rsb_core::hit::HandlerCall {
        instance: rsb_core::element::InstanceId(1),
        handler: rsb_core::element::HandlerId(right),
        kind: rsb_core::hit::InputKind::Click(rsb_core::hit::Button::Right),
        x: 0.0,
        y: 0.0,
        gesture: 1,
    }));
    h.advance(1000);
    h.advance(1000);
    assert_eq!(h.texts(1), vec!["bc"], "a was cancelled");
}

#[test]
fn too_many_tasks_is_an_error() {
    let mut h = Harness::new();
    h.user(
        "widgets/tm.luau",
        &widget(
            "{}",
            "return rsb.row { rsb.text 'x', on_click = function() for i = 1, 70 do rsb.task.spawn(function() rsb.task.sleep(100) end) end end }",
        ),
    );
    h.start(&[(1, "tm")]);
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    assert!(
        log_lines("tm: error: ").iter().any(|l| l.contains("too many tasks (at most 64 at once)")),
        "{:?}",
        log_lines("tm:")
    );
    assert!(matches!(h.state_of(1), WidgetState::Running));
}

#[test]
fn a_bare_yield_resumes_on_the_next_turn() {
    let mut h = Harness::new();
    h.user(
        "widgets/yl.luau",
        &widget(
            "{}",
            r#"
    local out = rsb.state(0)
    rsb.task.spawn(function()
        for i = 1, 3 do coroutine.yield() out.value = i end
    end)
    return rsb.text { text = function() return tostring(out.value) end }
"#,
        ),
    );
    h.start(&[(1, "yl")]);
    // One resume per turn: the task never runs twice in one turn.
    assert_eq!(h.texts(1), vec!["1"]);
    h.run_due();
    assert_eq!(h.texts(1), vec!["2"]);
    h.run_due();
    assert_eq!(h.texts(1), vec!["3"]);
    h.run_due();
    assert_eq!(h.texts(1), vec!["3"]);
}

#[test]
fn every_request_from_a_vm_call_goes_out_while_the_heartbeat_is_busy() {
    let mut h = Harness::new();
    h.user(
        "widgets/hbio.luau",
        &widget(
            "{ exec = { 'git' } }",
            "return rsb.row { rsb.text 'x', on_click = function() rsb.exec.run({ 'git' }) end }",
        ),
    );
    h.start(&[(1, "hbio")]);
    h.hub_busy.lock().unwrap().clear();
    h.hub.lock().unwrap().clear();
    h.click(1, first_click(h.root(1).unwrap()).unwrap());
    let busy = h.hub_busy.lock().unwrap().clone();
    assert!(!busy.is_empty() && busy.iter().all(|b| *b), "{busy:?}");
}
