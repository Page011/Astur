//! `rsb.exec.*` through the hub: argument quoting, PATH lookup, timeouts,
//! output and line caps, streaming order, process-tree kills and the caps on
//! how many programs run at once. Only the helper program, `cmd` and
//! `where` run, all read-only.

mod common;

use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::io::{ExecResult, ExecSpec, IoError, IoEventKind, IoId, IoRequest, ProgramRef, limits};
use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_services::fake::FakeWorld;

fn hub() -> (rsb_services::Hub, Collector) {
    hub_with(Box::new(FakeWorld::new()), fast_timing())
}

fn run_ok(args: &[&str], timeout: Option<Duration>, max_output: usize) -> ExecResult {
    let (hub, col) = hub();
    let dir = test_dir("exec-run");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecRun(helper_spec(args, timeout, max_output)), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).expect("the run finishes");
    hub.shutdown(WAIT);
    match done {
        IoEventKind::ExecDone(Ok(r)) => r,
        other => panic!("expected a result, got {other:?}"),
    }
}

fn run_err(spec: ExecSpec, perms: rsb_core::permissions::Permissions) -> IoError {
    let (hub, col) = hub();
    let dir = test_dir("exec-err");
    let g = grant("test/exec", &dir, perms);
    io(&hub, 1, 1, IoRequest::ExecRun(spec), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).expect("the run finishes");
    hub.shutdown(WAIT);
    match done {
        IoEventKind::ExecDone(Err(e)) => e,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn lines_of(s: &str) -> Vec<&str> {
    s.lines().collect()
}

#[test]
fn arguments_with_spaces_and_quotes_arrive_unchanged() {
    let tricky = ["a b", "say \"hi\"", "back\\", "tail\\\\\"x", "", "plain", "tab\there", "C:\\Program Files\\x y\\"];
    let mut args = vec!["args"];
    args.extend(tricky);
    let r = run_ok(&args, Some(Duration::from_secs(10)), 1 << 16);
    assert_eq!(r.code, Some(0));
    let got = lines_of(&r.stdout);
    let want: Vec<String> = tricky.iter().map(|a| format!("[{a}]")).collect();
    assert_eq!(got, want.iter().map(String::as_str).collect::<Vec<_>>());
}

#[test]
fn raw_tail_is_passed_to_cmd_exactly_as_written() {
    let (hub, col) = hub();
    let dir = test_dir("exec-raw");
    let g = grant("test/exec", &dir, perms(&["cmd"], &[], &[]));
    let spec = ExecSpec {
        program: ProgramRef::Name("cmd".into()),
        args: Vec::new(),
        raw_tail: Some("echo one&echo \"two  words\"".into()),
        cwd: None,
        timeout: Some(Duration::from_secs(10)),
        max_output: 4096,
    };
    io(&hub, 1, 1, IoRequest::ExecRun(spec), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    hub.shutdown(WAIT);
    let IoEventKind::ExecDone(Ok(r)) = done else { panic!("{done:?}") };
    assert_eq!(r.code, Some(0));
    assert_eq!(
        lines_of(&r.stdout),
        vec!["one", "\"two  words\""],
        "cmd saw the tail unquoted, so & separated two commands"
    );
}

#[test]
fn a_bare_name_is_found_on_path() {
    let (hub, col) = hub();
    let dir = test_dir("exec-path");
    let g = grant("test/exec", &dir, perms(&["where"], &[], &[]));
    let spec = ExecSpec {
        program: ProgramRef::Name("where".into()),
        args: vec!["cmd".into()],
        raw_tail: None,
        cwd: None,
        timeout: Some(Duration::from_secs(10)),
        max_output: 4096,
    };
    io(&hub, 1, 1, IoRequest::ExecRun(spec), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    hub.shutdown(WAIT);
    let IoEventKind::ExecDone(Ok(r)) = done else { panic!("{done:?}") };
    assert_eq!(r.code, Some(0));
    assert!(r.stdout.to_lowercase().contains("cmd.exe"));
}

#[test]
fn the_current_folder_is_never_searched() {
    let dir = test_dir("exec-cwd");
    std::fs::copy(helper_exe(), dir.join("planted.exe")).unwrap();
    let spec = ExecSpec {
        program: ProgramRef::Name("planted".into()),
        args: vec!["exit".into(), "0".into()],
        raw_tail: None,
        cwd: Some(dir.clone()),
        timeout: Some(Duration::from_secs(5)),
        max_output: 100,
    };
    let e = run_err(spec, perms(&["planted"], &[], &[]));
    assert!(matches!(e, IoError::Spawn(_)), "{e:?}");
    assert!(!e.to_string().contains(&*dir.to_string_lossy()), "no folder in the message: {e}");
}

#[test]
fn an_ungranted_program_is_refused_again_here() {
    let spec = helper_spec(&["exit", "0"], None, 100);
    let e = run_err(spec, perms(&["git"], &[], &[]));
    assert!(matches!(&e, IoError::NotPermitted(n) if n.starts_with("exec:")), "{e:?}");
}

#[test]
fn a_relative_program_path_and_a_missing_one_are_errors_without_paths() {
    let mut spec = helper_spec(&["exit", "0"], None, 100);
    spec.program = ProgramRef::Path("relative\\x.exe".into());
    let e = run_err(spec, perms(&["relative\\x.exe"], &[], &[]));
    assert!(matches!(e, IoError::BadRequest(_) | IoError::NotPermitted(_)), "{e:?}");

    let missing = test_dir("exec-missing").join("nothing.exe");
    let mut spec = helper_spec(&["exit", "0"], None, 100);
    spec.program = ProgramRef::Path(missing.clone());
    let e = run_err(spec, perms(&[missing.to_string_lossy().as_ref()], &[], &[]));
    assert!(matches!(e, IoError::Spawn(_)), "{e:?}");
    assert!(!e.to_string().contains(&*missing.to_string_lossy()), "{e}");
}

#[test]
fn the_exit_code_comes_back() {
    let r = run_ok(&["exit", "7"], Some(Duration::from_secs(10)), 100);
    assert_eq!(r.code, Some(7));
    assert!(!r.timed_out && !r.truncated);
}

#[test]
fn stdout_and_stderr_are_kept_apart() {
    let r = run_ok(&["stderr"], Some(Duration::from_secs(10)), 1000);
    assert_eq!(r.stdout.trim(), "to stdout");
    assert_eq!(r.stderr.trim(), "to stderr");
}

#[test]
fn a_program_over_its_timeout_is_killed() {
    let t = Instant::now();
    let r = run_ok(&["sleep", "30000"], Some(Duration::from_millis(300)), 100);
    let took = t.elapsed();
    println!("timeout kill: asked 300 ms, finished after {} ms", took.as_millis());
    assert!(r.timed_out);
    assert_eq!(r.code, None);
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[test]
fn output_over_the_cap_is_cut_and_marked() {
    let r = run_ok(&["flood", "200000"], Some(Duration::from_secs(10)), 10_000);
    assert!(r.truncated);
    assert_eq!(r.stdout.len(), 10_000);
    assert_eq!(r.code, Some(0), "the rest is drained so the program is not left blocked");
}

#[test]
fn the_working_folder_defaults_to_the_programs_own_folder_not_the_widgets() {
    let (hub, col) = hub();
    let dir = test_dir("exec-cwd-default").join("my-widget");
    std::fs::create_dir_all(&dir).unwrap();
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecRun(helper_spec(&["cwd-name"], Some(Duration::from_secs(10)), 100)), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    hub.shutdown(WAIT);
    let IoEventKind::ExecDone(Ok(r)) = done else { panic!("{done:?}") };
    let program_folder = helper_exe().parent().and_then(|p| p.file_name()).unwrap().to_string_lossy().into_owned();
    assert_eq!(r.stdout.trim(), program_folder);
    assert_ne!(r.stdout.trim(), "my-widget");
}

#[test]
fn an_explicit_working_folder_is_used() {
    let (hub, col) = hub();
    let dir = test_dir("exec-cwd-explicit").join("my-widget");
    std::fs::create_dir_all(&dir).unwrap();
    let g = grant("test/exec", &dir, helper_perms());
    let mut spec = helper_spec(&["cwd-name"], Some(Duration::from_secs(10)), 100);
    spec.cwd = Some(dir.clone());
    io(&hub, 1, 1, IoRequest::ExecRun(spec), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    hub.shutdown(WAIT);
    let IoEventKind::ExecDone(Ok(r)) = done else { panic!("{done:?}") };
    assert_eq!(r.stdout.trim(), "my-widget");
}

#[test]
fn standard_input_is_empty() {
    let r = run_ok(&["stdin-eof"], Some(Duration::from_secs(10)), 100);
    assert_eq!(r.stdout.trim(), "stdin read 0");
}

fn line_texts(events: &[IoEventKind]) -> Vec<(bool, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            IoEventKind::ExecLine { stderr, line } => Some((*stderr, line.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn spawned_lines_stream_in_order_and_done_follows_the_last_one() {
    let (hub, col) = hub();
    let dir = test_dir("exec-stream");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["lines", "6", "15"], None, 1 << 16)), &g);
    let (before, done) = finish(&col, 1, Duration::from_secs(15)).expect("done arrives");
    let got = line_texts(&before);
    assert_eq!(
        got.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>(),
        vec!["line 0", "line 1", "line 2", "line 3", "line 4", "line 5"]
    );
    assert!(matches!(done, IoEventKind::ExecDone(Ok(ExecResult { code: Some(0), .. }))), "{done:?}");
    // Nothing for this request follows the done event.
    let after = col.drain(Duration::from_millis(300));
    assert!(!after.iter().any(|e| matches!(e, HubEvent::Io(ev) if ev.id == IoId(1))), "events after done: {after:?}");
    hub.shutdown(WAIT);
}

#[test]
fn spawned_stderr_lines_are_marked() {
    let (hub, col) = hub();
    let dir = test_dir("exec-stream-err");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["stderr"], None, 1000)), &g);
    let (before, _) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    let mut got = line_texts(&before);
    got.sort();
    assert_eq!(got, vec![(false, "to stdout".to_string()), (true, "to stderr".to_string())]);
    hub.shutdown(WAIT);
}

#[test]
fn a_line_over_the_cap_is_cut_and_the_next_line_is_normal() {
    let (hub, col) = hub();
    let dir = test_dir("exec-longline");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["longline", "150000"], None, 1000)), &g);
    let (before, _) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    let got = line_texts(&before);
    assert_eq!(got.len(), 2, "{} lines", got.len());
    assert_eq!(got[0].1.len(), limits::EXEC_MAX_LINE + " [cut]".len());
    assert!(got[0].1.ends_with(" [cut]"));
    assert_eq!(got[1].1, "after");
    hub.shutdown(WAIT);
}

#[test]
fn streamed_lines_over_the_rate_are_dropped() {
    let (hub, col) = hub();
    let dir = test_dir("exec-rate");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["lines", "1000", "0"], None, 1000)), &g);
    let (before, done) = finish(&col, 1, Duration::from_secs(30)).unwrap();
    let delivered = line_texts(&before).len();
    println!(
        "rate cap: 1000 lines printed at once, {delivered} delivered (cap {} a second)",
        limits::EXEC_MAX_LINES_PER_SEC
    );
    assert!(delivered >= 1 && delivered <= 2 * limits::EXEC_MAX_LINES_PER_SEC as usize, "{delivered}");
    assert!(delivered < 1000);
    assert!(matches!(done, IoEventKind::ExecDone(Ok(_))), "{done:?}");
    hub.shutdown(WAIT);
}

fn first_line(col: &Collector, id: u64) -> String {
    col.wait(Duration::from_secs(15), |e| match e {
        HubEvent::Io(ev) if ev.id == IoId(id) => match &ev.kind {
            IoEventKind::ExecLine { line, .. } => Some(line.clone()),
            _ => None,
        },
        _ => None,
    })
    .expect("the program printed its child's id")
}

#[test]
fn cancel_owner_kills_the_whole_tree_including_a_grandchild() {
    let (hub, col) = hub();
    let dir = test_dir("exec-tree");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["spawn-child", "60000"], None, 1000)), &g);
    let child: u32 = first_line(&col, 1).trim().parse().expect("a process id");
    assert!(process_alive(child), "the grandchild runs while the widget lives");
    let t = Instant::now();
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(1) });
    assert!(eventually(Duration::from_secs(3), || !process_alive(child)), "the grandchild died with the tree");
    println!("cancel owner: grandchild gone after {} ms", t.elapsed().as_millis());
    // The owner is gone: no done event is sent for it.
    let after = col.drain(Duration::from_millis(400));
    assert!(
        !after.iter().any(|e| matches!(e, HubEvent::Io(ev) if matches!(ev.kind, IoEventKind::ExecDone(_)))),
        "{after:?}"
    );
    hub.shutdown(WAIT);
}

#[test]
fn cancel_by_id_reports_cancelled_and_kills_the_tree() {
    let (hub, col) = hub();
    let dir = test_dir("exec-cancel");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["spawn-child", "60000"], None, 1000)), &g);
    let child: u32 = first_line(&col, 1).trim().parse().unwrap();
    hub.port().send(HubRequest::Cancel { id: IoId(1) });
    let (_, done) = finish(&col, 1, Duration::from_secs(5)).expect("a done event");
    assert!(matches!(done, IoEventKind::ExecDone(Err(IoError::Cancelled))), "{done:?}");
    assert!(eventually(Duration::from_secs(3), || !process_alive(child)));
    hub.shutdown(WAIT);
}

#[test]
fn a_timeout_in_run_kills_the_grandchild_too() {
    let (hub, col) = hub();
    let dir = test_dir("exec-tree-timeout");
    let g = grant("test/exec", &dir, helper_perms());
    io(
        &hub,
        1,
        1,
        IoRequest::ExecRun(helper_spec(&["spawn-child", "60000"], Some(Duration::from_millis(800)), 1000)),
        &g,
    );
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    let IoEventKind::ExecDone(Ok(r)) = done else { panic!("{done:?}") };
    assert!(r.timed_out);
    let child: u32 = r.stdout.trim().parse().expect("the child's id was printed first");
    assert!(eventually(Duration::from_secs(3), || !process_alive(child)));
    hub.shutdown(WAIT);
}

#[test]
fn a_descendant_holding_the_pipe_does_not_hang_the_result() {
    let (hub, col) = hub();
    let dir = test_dir("exec-leak");
    let g = grant("test/exec", &dir, helper_perms());
    let t = Instant::now();
    io(&hub, 1, 1, IoRequest::ExecRun(helper_spec(&["leak-pipe"], Some(Duration::from_secs(20)), 1000)), &g);
    let (_, done) = finish(&col, 1, Duration::from_secs(15)).unwrap();
    let took = t.elapsed();
    println!("leaked pipe: result after {} ms", took.as_millis());
    let IoEventKind::ExecDone(Ok(r)) = done else { panic!("{done:?}") };
    assert_eq!(r.code, Some(0));
    assert!(took < Duration::from_secs(6), "{took:?}");
    let child: u32 = r.stdout.trim().parse().unwrap();
    assert!(eventually(Duration::from_secs(3), || !process_alive(child)), "the descendant was killed with the job");
    hub.shutdown(WAIT);
}

fn spawn_sleeper(hub: &rsb_services::Hub, id: u64, owner: u32, g: &std::sync::Arc<rsb_core::io::IoGrant>) {
    io(hub, id, owner, IoRequest::ExecSpawn(helper_spec(&["sleep", "60000"], None, 100)), g);
}

fn busy(col: &Collector, id: u64) -> bool {
    matches!(finish(col, id, Duration::from_millis(1500)), Some((_, IoEventKind::ExecDone(Err(IoError::Busy)))))
}

#[test]
fn at_most_eight_programs_per_widget_instance() {
    let (hub, col) = hub();
    let dir = test_dir("exec-cap-instance");
    let g = grant("test/exec", &dir, helper_perms());
    for id in 1..=limits::EXEC_MAX_PROCESSES_PER_INSTANCE as u64 {
        spawn_sleeper(&hub, id, 1, &g);
    }
    spawn_sleeper(&hub, 100, 1, &g);
    assert!(busy(&col, 100), "the ninth program of one instance is refused");
    // Another instance is not affected.
    spawn_sleeper(&hub, 200, 2, &g);
    assert!(!busy(&col, 200));
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(1) });
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(2) });
    hub.shutdown(WAIT);
}

#[test]
fn at_most_thirty_two_programs_in_all() {
    let (hub, col) = hub();
    let dir = test_dir("exec-cap-total");
    let g = grant("test/exec", &dir, helper_perms());
    let per = limits::EXEC_MAX_PROCESSES_PER_INSTANCE as u64;
    let owners = (limits::EXEC_MAX_PROCESSES_TOTAL / limits::EXEC_MAX_PROCESSES_PER_INSTANCE) as u64;
    for owner in 1..=owners {
        for n in 0..per {
            spawn_sleeper(&hub, owner * 1000 + n, owner as u32, &g);
        }
    }
    spawn_sleeper(&hub, 9000, 99, &g);
    assert!(busy(&col, 9000), "the 33rd program is refused whoever asks");
    for owner in 1..=owners {
        hub.port().send(HubRequest::CancelOwner { owner: InstanceId(owner as u32) });
    }
    // Room again once they are gone.
    assert!(eventually(Duration::from_secs(5), || {
        spawn_sleeper(&hub, 9001, 99, &g);
        !busy(&col, 9001)
    }));
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(99) });
    hub.shutdown(WAIT);
}

#[test]
fn shutdown_kills_what_is_still_running() {
    let (hub, col) = hub();
    let dir = test_dir("exec-shutdown");
    let g = grant("test/exec", &dir, helper_perms());
    io(&hub, 1, 1, IoRequest::ExecSpawn(helper_spec(&["spawn-child", "60000"], None, 1000)), &g);
    let child: u32 = first_line(&col, 1).trim().parse().unwrap();
    let t = Instant::now();
    hub.shutdown(WAIT);
    println!("shutdown with a running tree took {} ms", t.elapsed().as_millis());
    assert!(t.elapsed() < Duration::from_secs(3));
    assert!(eventually(Duration::from_secs(3), || !process_alive(child)));
}
