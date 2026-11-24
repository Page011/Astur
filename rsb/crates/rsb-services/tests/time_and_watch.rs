//! The real clock against `GetLocalTime`, the thread CPU clock against
//! `GetThreadTimes`, and the folder watcher's debounce.

mod common;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use rsb_core::time::{Clock, LocalTime, ThreadTime, TimeZone};
use rsb_services::clock::{SystemClock, ThreadCycleTime};
use rsb_services::watch::DirWatcher;
use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

fn seconds_of_day(h: u8, m: u8, s: u8) -> i64 {
    i64::from(h) * 3600 + i64::from(m) * 60 + i64::from(s)
}

fn same_instant(a: &LocalTime, h: u16, m: u16, s: u16) -> bool {
    // Within one second, across a minute or hour boundary too.
    let diff =
        (seconds_of_day(a.hour, a.minute, a.second) - seconds_of_day(h as u8, m as u8, s as u8)).rem_euclid(86_400);
    diff <= 1 || diff >= 86_399
}

#[test]
fn local_time_matches_get_local_time() {
    // SAFETY: no arguments; returns by value.
    let before = unsafe { GetLocalTime() };
    let ours = SystemClock.now(TimeZone::Local);
    // SAFETY: as above.
    let after = unsafe { GetLocalTime() };
    println!(
        "clock: ours {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} (offset {} min), GetLocalTime {:02}:{:02}:{:02}.{:03}",
        ours.year,
        ours.month,
        ours.day,
        ours.hour,
        ours.minute,
        ours.second,
        ours.millis,
        ours.utc_offset_min,
        before.wHour,
        before.wMinute,
        before.wSecond,
        before.wMilliseconds
    );
    assert_eq!((ours.year, ours.month, ours.day), (i32::from(before.wYear), before.wMonth as u8, before.wDay as u8));
    assert!(same_instant(&ours, before.wHour, before.wMinute, before.wSecond));
    assert!(same_instant(&ours, after.wHour, after.wMinute, after.wSecond));
    assert_eq!(ours.weekday, before.wDayOfWeek as u8);
}

#[test]
fn the_local_offset_matches_the_time_zone_rules() {
    let mut tz = TIME_ZONE_INFORMATION::default();
    // SAFETY: a live out pointer.
    let id = unsafe { GetTimeZoneInformation(&mut tz) };
    // 2 = daylight time in effect.
    let bias = if id == 2 { tz.Bias + tz.DaylightBias } else { tz.Bias + tz.StandardBias };
    let ours = SystemClock.now(TimeZone::Local);
    println!("offset: ours {} min, zone rules {} min", ours.utc_offset_min, -bias);
    assert_eq!(ours.utc_offset_min, -bias);
}

#[test]
fn utc_and_fixed_offsets_follow_get_system_time() {
    // SAFETY: no arguments; returns by value.
    let sys: SYSTEMTIME = unsafe { GetSystemTime() };
    let utc = SystemClock.now(TimeZone::Utc);
    assert_eq!(utc.utc_offset_min, 0);
    assert_eq!((utc.year, utc.month, utc.day), (i32::from(sys.wYear), sys.wMonth as u8, sys.wDay as u8));
    assert!(same_instant(&utc, sys.wHour, sys.wMinute, sys.wSecond));
    let plus = SystemClock.now(TimeZone::Offset(330));
    assert_eq!(plus.utc_offset_min, 330);
    let shifted = (seconds_of_day(utc.hour, utc.minute, utc.second) + 330 * 60).rem_euclid(86_400);
    let got = seconds_of_day(plus.hour, plus.minute, plus.second);
    assert!(
        ((shifted - got).rem_euclid(86_400) <= 1) || ((got - shifted).rem_euclid(86_400) <= 1),
        "{shifted} vs {got}"
    );
}

#[test]
fn the_monotonic_clock_never_goes_back() {
    let a = SystemClock.mono();
    std::thread::sleep(Duration::from_millis(5));
    assert!(SystemClock.mono() > a);
}

fn thread_times_ms() -> f64 {
    let (mut c, mut e, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the pseudo handle of the calling thread is valid; four live out pointers.
    unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) }.expect("GetThreadTimes");
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    (ticks(k) + ticks(u)) as f64 / 10_000.0
}

#[test]
fn thread_cycle_time_agrees_with_get_thread_times_over_a_busy_loop() {
    let clock = ThreadCycleTime::new();
    println!("thread clock calibrated at {:.3} cycles per ns", clock.cycles_per_ns());
    // The busy loop runs on this thread; GetThreadTimes steps in 15.6 ms, so use 200 ms.
    let mut worst = 0.0f64;
    let mut results = Vec::new();
    for _ in 0..3 {
        let (t0, c0) = (thread_times_ms(), clock.now());
        let start = Instant::now();
        let mut x = 1u64;
        while start.elapsed() < Duration::from_millis(200) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            std::hint::black_box(x);
        }
        let (t1, c1) = (thread_times_ms(), clock.now());
        let reference = t1 - t0;
        let ours = (c1 - c0).as_secs_f64() * 1000.0;
        let err = (ours - reference).abs() / reference.max(1.0);
        results.push((reference, ours, err));
        worst = worst.max(err);
        if err <= 0.20 {
            break;
        }
    }
    println!("thread cpu over a 200 ms busy loop (GetThreadTimes ms, ours ms, error): {results:?}");
    assert!(results.iter().any(|r| r.2 <= 0.20), "{results:?}");
}

#[test]
fn thread_cycle_time_does_not_count_sleep() {
    let clock = ThreadCycleTime::new();
    let c0 = clock.now();
    std::thread::sleep(Duration::from_millis(200));
    let used = (clock.now() - c0).as_secs_f64() * 1000.0;
    println!("thread cpu during a 200 ms sleep: {used:.3} ms");
    assert!(used < 5.0, "{used}");
}

fn watcher(dir: &std::path::Path, recursive: bool) -> (DirWatcher, mpsc::Receiver<(Instant, Vec<PathBuf>)>) {
    let (tx, rx) = mpsc::channel();
    let w = DirWatcher::start(
        dir,
        recursive,
        Duration::from_millis(60),
        Box::new(move |paths| {
            let _ = tx.send((Instant::now(), paths));
        }),
    )
    .expect("the watcher starts");
    (w, rx)
}

#[test]
fn a_burst_of_changes_is_one_batch_of_unique_paths() {
    let dir = test_dir("watch-burst");
    let (w, rx) = watcher(&dir, false);
    let start = Instant::now();
    for i in 0..5 {
        std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        std::fs::write(dir.join(format!("f{i}.txt")), "yy").unwrap();
    }
    let (at, paths) = rx.recv_timeout(WAIT).expect("a batch arrives");
    println!(
        "watch debounce: 10 writes in {} ms, one batch of {} paths after {} ms",
        start.elapsed().as_millis(),
        paths.len(),
        (at - start).as_millis()
    );
    assert_eq!(paths.len(), 5, "{paths:?}");
    assert!(paths.iter().all(|p| p.starts_with(&dir)));
    assert!(rx.recv_timeout(Duration::from_millis(400)).is_err(), "and nothing else");

    // A later change is a new batch.
    std::fs::write(dir.join("f0.txt"), "again").unwrap();
    let (_, second) = rx.recv_timeout(WAIT).expect("the next batch");
    assert_eq!(second, vec![dir.join("f0.txt")]);
    drop(w);
}

#[test]
fn recursion_is_a_choice() {
    let dir = test_dir("watch-recursive");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let (flat, flat_rx) = watcher(&dir, false);
    let (deep, deep_rx) = watcher(&dir, true);
    std::fs::write(dir.join("sub").join("n.txt"), "x").unwrap();
    let (_, seen) = deep_rx.recv_timeout(WAIT).expect("the recursive watcher sees it");
    assert!(seen.contains(&dir.join("sub").join("n.txt")), "{seen:?}");
    // The flat one sees the folder's own change at most, never the file inside it.
    while let Ok((_, paths)) = flat_rx.recv_timeout(Duration::from_millis(400)) {
        assert!(!paths.contains(&dir.join("sub").join("n.txt")), "{paths:?}");
    }
    drop((flat, deep));
}

#[test]
fn dropping_a_watcher_stops_it_quickly() {
    let dir = test_dir("watch-drop");
    let (w, rx) = watcher(&dir, false);
    let t = Instant::now();
    drop(w);
    println!("watcher stop took {} ms", t.elapsed().as_millis());
    assert!(t.elapsed() < Duration::from_millis(500));
    std::fs::write(dir.join("late.txt"), "x").unwrap();
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "nothing after the drop");
}

#[test]
fn watching_a_folder_that_does_not_exist_fails_cleanly() {
    let dir = test_dir("watch-missing").join("nope");
    assert!(DirWatcher::start(&dir, false, Duration::from_millis(50), Box::new(|_| {})).is_err());
}
