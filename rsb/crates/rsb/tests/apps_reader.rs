//! The Start menu is read by this program started again (`rsb --apps-reader <icon side>`): the list
//! and the icons come through a pipe, and every shell library that reading loads leaves with the
//! process. The test runs the real child against the real shell (read only: nothing is started)
//! and checks what comes through; it prints counts and times, never an app's name.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::Instant;

use rsb_services::apps_reader::{Frame, read_frame};

const EXE: &str = env!("CARGO_BIN_EXE_rsb");

fn child(args: &[&str]) -> std::process::Child {
    Command::new(EXE)
        .arg("--apps-reader")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rsb starts")
}

#[test]
fn the_child_lists_the_start_menu_sends_icons_for_listed_apps_only_and_ends_with_exit_code_0() {
    let started = Instant::now();
    let mut c = child(&["32"]);
    // No keys to skip: stdin is closed at once.
    drop(c.stdin.take());
    let mut out = c.stdout.take().unwrap();
    let Frame::List(apps) = read_frame(&mut out).expect("the list comes first") else {
        panic!("the first frame is not the list")
    };
    let listed_at = started.elapsed();
    assert!(!apps.is_empty(), "the Start menu of this PC has apps");
    let ids: std::collections::HashSet<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids.len(), apps.len(), "every id once");
    assert!(apps.windows(2).all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase()), "sorted by name");
    let mut icons = 0;
    loop {
        match read_frame(&mut out).expect("the stream ends with an end frame") {
            Frame::Icon { id, image } => {
                assert!(ids.contains(id.as_str()), "an icon of an app that is not in the list");
                assert!(image.width == 32 && image.height == 32, "{}x{}", image.width, image.height);
                icons += 1;
            }
            Frame::End => break,
            Frame::List(_) => panic!("a second list"),
        }
    }
    let mut rest = Vec::new();
    out.read_to_end(&mut rest).unwrap();
    assert!(rest.is_empty(), "nothing after the end");
    let status = c.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    println!(
        "apps reader child: {} apps listed after {} ms, {icons} icons, whole run {} ms",
        apps.len(),
        listed_at.as_millis(),
        started.elapsed().as_millis()
    );
    assert!(icons > 0, "some icons came through");
}

#[test]
fn icons_already_held_are_skipped_and_the_child_only_ever_writes_to_its_pipe() {
    // First run: learn the keys of the first icons.
    let mut c = child(&["32"]);
    drop(c.stdin.take());
    let mut out = c.stdout.take().unwrap();
    let Frame::List(apps) = read_frame(&mut out).unwrap() else { panic!("no list") };
    let mut first = Vec::new();
    while let Ok(Frame::Icon { id, .. }) = read_frame(&mut out) {
        first.push(id);
        if first.len() == 5 {
            break;
        }
    }
    let _ = c.kill();
    let _ = c.wait();
    assert_eq!(first.len(), 5, "at least five icons");
    // The keys are the child's own function of the id; ask for the same ones to be skipped.
    let keys: Vec<String> = first.iter().map(|id| rsb_services::apps_reader::icon_key(id)).collect();
    let mut c = child(&["32"]);
    {
        use std::io::Write;
        let mut stdin = c.stdin.take().unwrap();
        stdin.write_all(keys.join("\n").as_bytes()).unwrap();
    }
    let mut out = c.stdout.take().unwrap();
    let Frame::List(again) = read_frame(&mut out).unwrap() else { panic!("no list") };
    assert_eq!(again.len(), apps.len());
    loop {
        match read_frame(&mut out).unwrap() {
            Frame::Icon { id, .. } => assert!(!first.contains(&id), "an icon that was asked to be skipped came again"),
            Frame::End => break,
            Frame::List(_) => panic!("a second list"),
        }
    }
    assert_eq!(c.wait().unwrap().code(), Some(0));
}

#[test]
fn a_missing_or_silly_icon_side_is_refused_with_exit_code_2_and_no_frames() {
    for args in [&[][..], &["x"], &["8"], &["100000"]] {
        let mut c = child(args);
        drop(c.stdin.take());
        let mut out = Vec::new();
        c.stdout.take().unwrap().read_to_end(&mut out).unwrap();
        assert!(out.is_empty(), "{args:?} wrote {} bytes", out.len());
        assert_eq!(c.wait().unwrap().code(), Some(2), "{args:?}");
    }
}
