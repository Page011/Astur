//! The hidden test flags that inject events at set times: a schedule built
//! from the flags, a thread that plays it into the UI's inbox, a pointer
//! script parser and the fake service storm.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::services::ServiceId;
use rsb_core::value::Value;
use rsb_core::wake::Wake;
use rsb_services::fake::FakeWorld;

use crate::args::RunOpts;
use crate::msg::{SimAction, UiMsg};

/// `<ms> <bar> move|down|up|wheel|leave <x> <y> [left|right|middle|<notches>]`
/// per line; blank lines and `#` comments are ignored. Positions are
/// bar-relative physical pixels.
pub fn parse_pointer_script(text: &str) -> Result<Vec<(u64, SimAction)>, String> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = |m: &str| format!("pointer script line {}: {m}", i + 1);
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 5 {
            return Err(at("use <ms> <bar> <move|down|up|wheel|leave> <x> <y> [button|notches]"));
        }
        let ms: u64 = words[0].parse().map_err(|_| at("the time is not a number"))?;
        let x: f32 = words[3].parse().map_err(|_| at("x is not a number"))?;
        let y: f32 = words[4].parse().map_err(|_| at("y is not a number"))?;
        let arg = words.get(5).copied();
        let button = |a: Option<&str>| match a.unwrap_or("left") {
            "left" => Ok(Button::Left),
            "right" => Ok(Button::Right),
            "middle" => Ok(Button::Middle),
            other => Err(at(&format!("'{other}' is not left, right or middle"))),
        };
        let kind = match words[2] {
            "move" => PointerKind::Move,
            "leave" => PointerKind::Leave,
            "down" => PointerKind::Down(button(arg)?),
            "up" => PointerKind::Up(button(arg)?),
            "wheel" => {
                let notches = arg.ok_or_else(|| at("wheel needs the notches"))?;
                PointerKind::Wheel(notches.parse().map_err(|_| at("the notches are not a number"))?)
            }
            other => return Err(at(&format!("unknown event '{other}'"))),
        };
        out.push((ms, SimAction::Pointer { bar: words[1].to_string(), event: PointerEvent { kind, x, y } }));
    }
    Ok(out)
}

/// Every timed action the flags ask for, in time order.
pub fn schedule(opts: &RunOpts, pointer: Vec<(u64, SimAction)>) -> Vec<(Duration, SimAction)> {
    let ms = Duration::from_millis;
    let mut out: Vec<(Duration, SimAction)> = Vec::new();
    for &t in &opts.simulate_taskbar_created {
        out.push((ms(t), SimAction::TaskbarCreated));
    }
    for &(t, dpi) in &opts.simulate_dpi {
        out.push((ms(t), SimAction::Dpi(dpi)));
    }
    for &t in &opts.simulate_display_change {
        out.push((ms(t), SimAction::DisplayChange));
    }
    for &(t, dark) in &opts.simulate_theme {
        out.push((ms(t), SimAction::Theme(dark)));
    }
    for &(t, on) in &opts.simulate_fullscreen {
        out.push((ms(t), SimAction::Fullscreen(on)));
    }
    for &(t, kind) in &opts.simulate_clock_change {
        out.push((ms(t), SimAction::Clock(kind)));
    }
    if let Some((t, for_ms)) = opts.test_fullscreen_window {
        out.push((ms(t), SimAction::FullscreenWindow { for_ms }));
    }
    if let Some(t) = opts.test_render_failure {
        out.push((ms(t), SimAction::RenderFailure));
    }
    if let Some(t) = opts.test_panic {
        out.push((ms(t), SimAction::Panic));
    }
    out.extend(pointer.into_iter().map(|(t, a)| (ms(t), a)));
    if let Some(s) = opts.seconds {
        out.push((Duration::from_secs_f64(s.max(0.0)), SimAction::Quit));
    }
    out.sort_by_key(|(t, _)| *t);
    out
}

/// Plays a schedule into the inbox. Stops early when told to. It sleeps on a
/// condition variable until the next action is due, so a run with only
/// `--seconds` costs no CPU while it waits (a polling loop would show up in
/// every idle measurement).
pub struct Player {
    stop: Arc<(Mutex<bool>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn start(plan: Vec<(Duration, SimAction)>, tx: Sender<UiMsg>, waker: Arc<dyn Wake>) -> Player {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("rsb-sim".into())
            .spawn(move || {
                let origin = Instant::now();
                let (lock, cond) = &*thread_stop;
                for (at, action) in plan {
                    let mut stopped = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while !*stopped {
                        let left = at.saturating_sub(origin.elapsed());
                        if left.is_zero() {
                            break;
                        }
                        stopped = cond.wait_timeout(stopped, left).unwrap_or_else(|e| e.into_inner()).0;
                    }
                    if *stopped {
                        return;
                    }
                    drop(stopped);
                    if tx.send(UiMsg::Sim(action)).is_err() {
                        return;
                    }
                    waker.wake();
                }
            })
            .expect("the test thread starts");
        Player { stop, thread: Some(thread) }
    }

    pub fn stop(mut self) {
        let (lock, cond) = &*self.stop;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cond.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A running service storm: `stop` ends it at once and waits for the thread.
pub struct Storm {
    stop: Sender<()>,
    thread: JoinHandle<()>,
}

impl Storm {
    pub fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.thread.join();
    }
}

/// Publishes a new system value `hz` times a second for `secs` seconds, then goes silent.
pub fn service_storm(world: FakeWorld, hz: f64, secs: f64) -> Storm {
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let thread = std::thread::Builder::new()
        .name("rsb-storm".into())
        .spawn(move || {
            let period = Duration::from_secs_f64(1.0 / hz.clamp(0.1, 1000.0));
            let (start, mut n) = (Instant::now(), 0u32);
            while start.elapsed().as_secs_f64() < secs {
                n += 1;
                let fields = [
                    ("cpu", Value::Num(f64::from(n % 97))),
                    ("memory", Value::Num(40.0 + f64::from(n % 7))),
                    ("memory_used", Value::Int(6_900_000_000 + i64::from(n))),
                    ("memory_total", Value::Int(16_900_000_000)),
                ];
                world.set(ServiceId::System, Value::Map(fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect()));
                // The wait is the sleep: a stop order (or a dropped sender) ends it early.
                if !matches!(stopped.recv_timeout(period), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                    break;
                }
            }
            world.go_silent(ServiceId::System);
        })
        .expect("the storm thread starts");
    Storm { stop, thread }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ClockKind;

    #[test]
    fn a_storm_with_a_long_limit_stops_when_told_to() {
        let storm = service_storm(FakeWorld::new(), 1.0, 100_000.0);
        let t = Instant::now();
        storm.stop();
        assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    }

    #[test]
    fn the_pointer_script_parses_every_event() {
        let text = "# hover, click, wheel, leave\n100 main move 40 10\n150 main down 40 10 left\n160 main up 40 10\n\
                    200 main wheel 300 10 -2.5  # volume down\n250 main up 5 5 right\n300 main leave 0 0\n\n";
        let parsed = parse_pointer_script(text).unwrap();
        assert_eq!(parsed.len(), 6);
        let kinds: Vec<PointerKind> = parsed
            .iter()
            .map(|(_, a)| match a {
                SimAction::Pointer { event, .. } => event.kind,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                PointerKind::Move,
                PointerKind::Down(Button::Left),
                PointerKind::Up(Button::Left),
                PointerKind::Wheel(-2.5),
                PointerKind::Up(Button::Right),
                PointerKind::Leave
            ]
        );
        assert_eq!(parsed[0].0, 100);
    }

    #[test]
    fn a_bad_pointer_line_names_its_line() {
        for (text, needle) in [
            ("100 main move 4", "line 1"),
            ("x main move 4 4", "time"),
            ("1 main flick 4 4", "unknown event 'flick'"),
            ("1 main down 4 4 purple", "left, right or middle"),
            ("1 main wheel 4 4", "notches"),
            ("\n\n1 main move a 4", "line 3"),
        ] {
            let e = parse_pointer_script(text).unwrap_err();
            assert!(e.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn the_schedule_is_sorted_and_ends_with_quit() {
        let opts = RunOpts {
            seconds: Some(2.0),
            simulate_dpi: vec![(900, 120), (300, 96)],
            simulate_theme: vec![(500, false)],
            simulate_clock_change: vec![(100, ClockKind::Resume)],
            test_fullscreen_window: Some((700, 1000)),
            ..RunOpts::default()
        };
        let plan = schedule(&opts, Vec::new());
        let times: Vec<u128> = plan.iter().map(|(t, _)| t.as_millis()).collect();
        assert_eq!(times, [100, 300, 500, 700, 900, 2000]);
        assert_eq!(plan.last().map(|(_, a)| a.clone()), Some(SimAction::Quit));
        assert_eq!(plan[3].1, SimAction::FullscreenWindow { for_ms: 1000 });
        assert!(schedule(&RunOpts::default(), Vec::new()).is_empty());
    }
}
