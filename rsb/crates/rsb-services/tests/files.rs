//! `rsb.file.read` / `watch` and `files::open_permitted`: the grant is
//! checked on the opened handle's final path (junctions), the size cap holds,
//! a briefly locked file is retried, and a watch reports one event per save.

mod common;

use std::fs::OpenOptions;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::element::InstanceId;
use rsb_core::io::{FileRequest, IoError, IoEventKind, IoGrant, IoId, IoRequest, limits};
use rsb_core::msg::{HubEvent, HubPort, HubRequest};
use rsb_services::Hub;
use rsb_services::fake::FakeWorld;
use rsb_services::files::{open_permitted, read_text};

struct Setup {
    root: PathBuf,
    widget: PathBuf,
    outside: PathBuf,
}

fn setup(name: &str) -> Setup {
    let root = test_dir(name);
    let widget = root.join("widget");
    let outside = root.join("outside");
    std::fs::create_dir_all(&widget).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    Setup { root, widget, outside }
}

fn pattern(dir: &Path) -> String {
    format!("{}/**", dir.to_string_lossy().replace('\\', "/"))
}

fn own_grant(s: &Setup) -> Arc<IoGrant> {
    grant("t/files", &s.widget, perms(&[], &[], &[]))
}

fn hub() -> (Hub, Collector) {
    hub_with(Box::new(FakeWorld::new()), fast_timing())
}

fn read(hub: &Hub, col: &Collector, id: u64, path: &Path, max: usize, g: &Arc<IoGrant>) -> Result<String, IoError> {
    io(hub, id, 1, IoRequest::ReadFile(FileRequest { path: path.to_path_buf(), max_bytes: max }), g);
    match finish(col, id, Duration::from_secs(10)).expect("an answer") {
        (_, IoEventKind::File(r)) => r,
        (_, other) => panic!("{other:?}"),
    }
}

fn junction(link: &Path, target: &Path) {
    let out = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("cmd runs");
    assert!(out.status.success(), "mklink failed: {}", String::from_utf8_lossy(&out.stdout));
}

fn no_path_in(e: &IoError, s: &Setup) {
    let text = e.to_string();
    let root = s.root.to_string_lossy().to_lowercase();
    assert!(!text.to_lowercase().contains(&root), "{text}");
    assert!(!text.contains(":\\") && !text.contains(":/"), "{text}");
}

#[test]
fn a_file_in_the_widgets_folder_is_read() {
    let s = setup("files-read");
    std::fs::write(s.widget.join("a.txt"), "hello\nworld").unwrap();
    let (hub, col) = hub();
    assert_eq!(read(&hub, &col, 1, &s.widget.join("a.txt"), 1000, &own_grant(&s)).unwrap(), "hello\nworld");
    hub.shutdown(WAIT);
}

#[test]
fn bytes_that_are_not_utf8_are_replaced_not_refused() {
    let s = setup("files-lossy");
    std::fs::write(s.widget.join("b.txt"), [b'a', 0xFF, b'b']).unwrap();
    let (hub, col) = hub();
    assert_eq!(read(&hub, &col, 1, &s.widget.join("b.txt"), 1000, &own_grant(&s)).unwrap(), "a\u{FFFD}b");
    hub.shutdown(WAIT);
}

#[test]
fn a_file_over_the_cap_is_refused() {
    let s = setup("files-cap");
    std::fs::write(s.widget.join("big.txt"), vec![b'x'; 5000]).unwrap();
    let (hub, col) = hub();
    let g = own_grant(&s);
    let e = read(&hub, &col, 1, &s.widget.join("big.txt"), 1000, &g).unwrap_err();
    assert_eq!(e, IoError::TooLarge { limit: 1000 });
    assert_eq!(
        read(&hub, &col, 2, &s.widget.join("big.txt"), 5000, &g).unwrap().len(),
        5000,
        "exactly at the cap is fine"
    );
    hub.shutdown(WAIT);
}

#[test]
fn the_global_maximum_applies_to_any_requested_cap() {
    let s = setup("files-maxcap");
    let file = s.widget.join("huge.bin");
    let f = std::fs::File::create(&file).unwrap();
    f.set_len(limits::FILE_MAX_BYTES as u64 + 1).unwrap();
    let e = read_text(std::fs::File::open(&file).unwrap(), usize::MAX).unwrap_err();
    assert_eq!(e, IoError::TooLarge { limit: limits::FILE_MAX_BYTES });
}

#[test]
fn a_missing_file_is_not_found_without_its_path() {
    let s = setup("files-missing");
    let (hub, col) = hub();
    let e = read(&hub, &col, 1, &s.widget.join("nope.txt"), 1000, &own_grant(&s)).unwrap_err();
    assert_eq!(e, IoError::NotFound);
    no_path_in(&e, &s);
    hub.shutdown(WAIT);
}

#[test]
fn a_folder_is_not_a_file() {
    let s = setup("files-dir");
    std::fs::create_dir_all(s.widget.join("sub")).unwrap();
    let g = own_grant(&s);
    let e = open_permitted(&s.widget.join("sub"), &g).unwrap_err();
    assert!(matches!(e, IoError::Io(_) | IoError::Denied), "{e:?}");
}

#[test]
fn outside_the_grant_is_refused_and_a_pattern_opens_it() {
    let s = setup("files-grant");
    std::fs::write(s.outside.join("x.txt"), "data").unwrap();
    let (hub, col) = hub();
    let e = read(&hub, &col, 1, &s.outside.join("x.txt"), 100, &own_grant(&s)).unwrap_err();
    assert!(matches!(e, IoError::NotPermitted(_)), "{e:?}");
    no_path_in(&e, &s);
    let granted = grant("t/files", &s.widget, perms(&[], &[&pattern(&s.outside)], &[]));
    assert_eq!(read(&hub, &col, 2, &s.outside.join("x.txt"), 100, &granted).unwrap(), "data");
    hub.shutdown(WAIT);
}

#[test]
fn dot_dot_segments_are_refused() {
    let s = setup("files-dotdot");
    std::fs::write(s.outside.join("x.txt"), "data").unwrap();
    let sneaky = s.widget.join("..").join("outside").join("x.txt");
    let e = open_permitted(&sneaky, &own_grant(&s)).unwrap_err();
    assert!(matches!(e, IoError::NotPermitted(_)), "{e:?}");
}

#[test]
fn a_junction_inside_the_folder_cannot_lead_outside() {
    let s = setup("files-junction");
    std::fs::write(s.outside.join("secret.txt"), "secret").unwrap();
    let link = s.widget.join("link");
    junction(&link, &s.outside);
    // The path looks like it is inside the widget's folder.
    let e = open_permitted(&link.join("secret.txt"), &own_grant(&s)).unwrap_err();
    assert!(matches!(&e, IoError::NotPermitted(n) if n.contains("resolves outside")), "{e:?}");
    no_path_in(&e, &s);
    // Granting the real place makes the same path work.
    let granted = grant("t/files", &s.widget, perms(&[], &[&pattern(&s.outside)], &[]));
    let file = open_permitted(&link.join("secret.txt"), &granted).expect("permitted by its final path");
    assert_eq!(read_text(file, 100).unwrap(), "secret");
}

#[test]
fn a_junction_to_a_permitted_place_works_and_one_back_inside_works() {
    let s = setup("files-junction-in");
    std::fs::create_dir_all(s.widget.join("real")).unwrap();
    std::fs::write(s.widget.join("real").join("ok.txt"), "fine").unwrap();
    junction(&s.widget.join("alias"), &s.widget.join("real"));
    let file = open_permitted(&s.widget.join("alias").join("ok.txt"), &own_grant(&s)).expect("stays inside");
    assert_eq!(read_text(file, 100).unwrap(), "fine");
}

#[test]
fn a_briefly_locked_file_is_retried() {
    let s = setup("files-share");
    let path = s.widget.join("locked.txt");
    std::fs::write(&path, "content").unwrap();
    // An exclusive hold, like an editor mid-save.
    let holder = OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(25));
        drop(holder);
    });
    let t = Instant::now();
    let file = open_permitted(&path, &own_grant(&s)).expect("opened after the hold ended");
    println!(
        "sharing retry: opened after {} ms (retries: {} x {} ms)",
        t.elapsed().as_millis(),
        limits::FILE_SHARING_RETRIES,
        limits::FILE_SHARING_RETRY_DELAY.as_millis()
    );
    assert_eq!(read_text(file, 100).unwrap(), "content");
    release.join().unwrap();
}

#[test]
fn a_file_that_stays_locked_gives_up_after_the_retries_without_a_path() {
    let s = setup("files-share-stuck");
    let path = s.widget.join("locked.txt");
    std::fs::write(&path, "content").unwrap();
    let _holder = OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
    let t = Instant::now();
    let e = open_permitted(&path, &own_grant(&s)).unwrap_err();
    let took = t.elapsed();
    assert!(matches!(&e, IoError::Io(m) if m.contains("in use")), "{e:?}");
    no_path_in(&e, &s);
    let floor = limits::FILE_SHARING_RETRY_DELAY * limits::FILE_SHARING_RETRIES;
    assert!(took >= floor.mul_f32(0.9), "{took:?} should be at least {floor:?}");
    assert!(took < Duration::from_secs(1), "{took:?}");
}

fn file_events(col: &Collector, id: u64, quiet: Duration) -> Vec<Result<String, IoError>> {
    col.drain(quiet)
        .into_iter()
        .filter_map(|e| match e {
            HubEvent::Io(ev) if ev.id == IoId(id) => match ev.kind {
                IoEventKind::File(r) => Some(r),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn next_file(col: &Collector, id: u64, timeout: Duration) -> Option<Result<String, IoError>> {
    col.wait(timeout, |e| match e {
        HubEvent::Io(ev) if ev.id == IoId(id) => match &ev.kind {
            IoEventKind::File(r) => Some(r.clone()),
            _ => None,
        },
        _ => None,
    })
}

fn watch(hub: &Hub, id: u64, owner: u32, path: &Path, g: &Arc<IoGrant>) {
    io(hub, id, owner, IoRequest::WatchFile(FileRequest { path: path.to_path_buf(), max_bytes: 10_000 }), g);
}

#[test]
fn a_watch_sends_the_content_then_one_event_per_save() {
    let s = setup("files-watch");
    let path = s.widget.join("w.txt");
    std::fs::write(&path, "v0").unwrap();
    let (hub, col) = hub();
    watch(&hub, 1, 1, &path, &own_grant(&s));
    assert_eq!(next_file(&col, 1, WAIT), Some(Ok("v0".into())), "the first event is the current content");
    for n in 1..=3 {
        std::thread::sleep(Duration::from_millis(300));
        std::fs::write(&path, format!("v{n}")).unwrap();
        assert_eq!(next_file(&col, 1, WAIT), Some(Ok(format!("v{n}"))), "save {n}");
    }
    let extra = file_events(&col, 1, Duration::from_millis(400));
    assert!(extra.is_empty(), "no stray events after the saves: {extra:?}");
    hub.shutdown(WAIT);
}

#[test]
fn a_watch_follows_a_file_that_comes_and_goes() {
    let s = setup("files-watch-missing");
    let path = s.widget.join("later.txt");
    let (hub, col) = hub();
    watch(&hub, 1, 1, &path, &own_grant(&s));
    assert_eq!(next_file(&col, 1, WAIT), Some(Err(IoError::NotFound)));
    std::fs::write(&path, "now here").unwrap();
    assert_eq!(next_file(&col, 1, WAIT), Some(Ok("now here".into())));
    std::fs::remove_file(&path).unwrap();
    assert_eq!(next_file(&col, 1, WAIT), Some(Err(IoError::NotFound)));
    hub.shutdown(WAIT);
}

#[test]
fn other_files_in_the_folder_do_not_fire_the_watch() {
    let s = setup("files-watch-other");
    let path = s.widget.join("mine.txt");
    std::fs::write(&path, "mine").unwrap();
    let (hub, col) = hub();
    watch(&hub, 1, 1, &path, &own_grant(&s));
    assert!(next_file(&col, 1, WAIT).is_some());
    std::fs::write(s.widget.join("other.txt"), "x").unwrap();
    assert!(file_events(&col, 1, Duration::from_millis(500)).is_empty());
    hub.shutdown(WAIT);
}

#[test]
fn a_watch_outside_the_grant_reports_it_and_a_cancelled_one_goes_quiet() {
    let s = setup("files-watch-cancel");
    std::fs::write(s.outside.join("o.txt"), "x").unwrap();
    let (hub, col) = hub();
    watch(&hub, 1, 1, &s.outside.join("o.txt"), &own_grant(&s));
    assert!(matches!(next_file(&col, 1, WAIT), Some(Err(IoError::NotPermitted(_)))));

    let path = s.widget.join("c.txt");
    std::fs::write(&path, "a").unwrap();
    watch(&hub, 2, 2, &path, &own_grant(&s));
    assert!(next_file(&col, 2, WAIT).is_some());
    hub.port().send(HubRequest::CancelOwner { owner: InstanceId(2) });
    std::thread::sleep(Duration::from_millis(200));
    std::fs::write(&path, "b").unwrap();
    assert!(file_events(&col, 2, Duration::from_millis(500)).is_empty(), "nothing after the owner is cancelled");
    hub.shutdown(WAIT);
}

/// The first file event of each of `ids`, whatever order they arrive in.
fn first_events(
    col: &Collector,
    ids: &[u64],
    timeout: Duration,
) -> std::collections::HashMap<u64, Result<String, IoError>> {
    let mut got = std::collections::HashMap::new();
    let end = Instant::now() + timeout;
    while got.len() < ids.len() {
        let left = end.saturating_duration_since(Instant::now());
        let hit = col.wait(left, |e| match e {
            HubEvent::Io(ev) if ids.contains(&ev.id.0) => match &ev.kind {
                IoEventKind::File(r) => Some((ev.id.0, r.clone())),
                _ => None,
            },
            _ => None,
        });
        match hit {
            Some((id, r)) => {
                got.entry(id).or_insert(r);
            }
            None => panic!("only {} of {} watches answered", got.len(), ids.len()),
        }
    }
    got
}

#[test]
fn watches_are_capped_per_widget_and_in_all() {
    let s = setup("files-watch-caps");
    let g = own_grant(&s);
    let (hub, col) = hub();
    let mut next_id = 0u64;
    let mut open = |owner: u32, n: usize| {
        let mut ids = Vec::new();
        for _ in 0..n {
            next_id += 1;
            let path = s.widget.join(format!("f{next_id}.txt"));
            std::fs::write(&path, "x").unwrap();
            watch(&hub, next_id, owner, &path, &g);
            ids.push(next_id);
        }
        ids
    };
    let per = limits::FILE_MAX_WATCHES_PER_INSTANCE;
    let first = open(1, per);
    for (id, r) in first_events(&col, &first, Duration::from_secs(20)) {
        assert_eq!(r, Ok("x".into()), "watch {id} started");
    }
    let over = open(1, 1);
    assert_eq!(first_events(&col, &over, WAIT)[&over[0]], Err(IoError::Busy), "the 17th watch of one widget");
    // Three more widgets fill the total of 64.
    let mut all = Vec::new();
    for owner in 2..=(limits::FILE_MAX_WATCHES_TOTAL / per) as u32 {
        all.extend(open(owner, per));
    }
    for (id, r) in first_events(&col, &all, Duration::from_secs(30)) {
        assert_eq!(r, Ok("x".into()), "watch {id} started");
    }
    let total_over = open(9, 1);
    assert_eq!(first_events(&col, &total_over, WAIT)[&total_over[0]], Err(IoError::Busy), "the 65th watch overall");
    for owner in 1..=9 {
        hub.port().send(HubRequest::CancelOwner { owner: InstanceId(owner) });
    }
    hub.shutdown(WAIT);
}
