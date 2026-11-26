//! The hidden test flags that inject events at set times: a schedule built
//! from the flags, a thread that plays it into the UI's inbox, a pointer
//! script parser and the fake service storm.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rsb_core::hit::{Button, PointerEvent, PointerKind};
use rsb_core::key::{Key, KeyEvent, Modifiers, TextInput};
use rsb_core::services::ServiceId;
use rsb_core::value::Value;
use rsb_core::wake::Wake;
use rsb_services::fake::FakeWorld;

use crate::args::RunOpts;
use crate::msg::{SimAction, SimSurface, UiMsg};

/// `main` / `bar:main`, `popup:<name>`, `dim:<name>`. Only a popup when
/// `any` is false (keys go to popup windows only).
pub(crate) fn parse_surface(word: &str, any: bool) -> Result<SimSurface, String> {
    let named = |n: &str| (!n.is_empty()).then(|| n.to_string()).ok_or_else(|| format!("'{word}' names no surface"));
    if let Some(n) = word.strip_prefix("popup:") {
        return Ok(SimSurface::Popup(named(n)?));
    }
    if !any {
        return Err(format!("'{word}' is not popup:<name>"));
    }
    if let Some(n) = word.strip_prefix("dim:") {
        return Ok(SimSurface::Dim(named(n)?));
    }
    Ok(SimSurface::Bar(named(word.strip_prefix("bar:").unwrap_or(word))?))
}

/// The key a script line names: a `KeyboardEvent.key` name, or one character.
fn parse_key(name: &str) -> Option<Key> {
    Some(match name {
        "Enter" => Key::Enter,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Insert" => Key::Insert,
        "Space" => Key::Space,
        "ArrowUp" => Key::ArrowUp,
        "ArrowDown" => Key::ArrowDown,
        "ArrowLeft" => Key::ArrowLeft,
        "ArrowRight" => Key::ArrowRight,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        f if f.len() >= 2 && f.starts_with('F') && f[1..].bytes().all(|b| b.is_ascii_digit()) => {
            let n: u8 = f[1..].parse().ok().filter(|n| (1..=24).contains(n))?;
            Key::F(n)
        }
        c if c.chars().count() == 1 => Key::Char(c.chars().next()?),
        _ => return None,
    })
}

/// `\uXXXX` escapes, `\s` for a space and `\\` in text that has to sit on one
/// whitespace-separated line.
fn unescape(text: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut it = text.chars();
    let mut units: Vec<u16> = Vec::new();
    let flush = |units: &mut Vec<u16>, out: &mut String| {
        out.extend(char::decode_utf16(units.drain(..)).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)));
    };
    while let Some(c) = it.next() {
        if c != '\\' {
            flush(&mut units, &mut out);
            out.push(c);
            continue;
        }
        match it.next() {
            Some('u') => {
                let hex: String = it.by_ref().take(4).collect();
                let unit = u16::from_str_radix(&hex, 16).map_err(|_| format!("'\\u{hex}' is not a \\uXXXX escape"))?;
                units.push(unit);
            }
            Some('s') => {
                flush(&mut units, &mut out);
                out.push(' ');
            }
            Some('\\') => {
                flush(&mut units, &mut out);
                out.push('\\');
            }
            other => return Err(format!("unknown escape '\\{}'", other.map(String::from).unwrap_or_default())),
        }
    }
    flush(&mut units, &mut out);
    Ok(out)
}

/// `<ms> popup:<name> key <Key> [down|up] [ctrl+shift+alt] [repeat]`,
/// `<ms> popup:<name> text <text>` (`\uXXXX`, `\s` for a space, `\\`) and
/// `<ms> popup:<name> ime start|update <text> <cursor>|commit <text>|end`.
/// Blank lines and `#` comments are ignored.
pub fn parse_key_script(text: &str) -> Result<Vec<(u64, SimAction)>, String> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let at = |m: &str| format!("key script line {}: {m}", i + 1);
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() < 3 {
            return Err(at("use <ms> popup:<name> key|text|ime ..."));
        }
        let ms: u64 = words[0].parse().map_err(|_| at("the time is not a number"))?;
        let popup = match parse_surface(words[1], false).map_err(|e| at(&e))? {
            SimSurface::Popup(n) => n,
            _ => return Err(at("keys go to popup:<name>")),
        };
        let rest = &words[3..];
        let action = match words[2] {
            "key" => {
                let name = rest.first().ok_or_else(|| at("key needs a key name"))?;
                let key = parse_key(name).ok_or_else(|| at(&format!("'{name}' is not a key name")))?;
                let (mut down, mut repeat, mut mods) = (true, false, Modifiers::default());
                for w in &rest[1..] {
                    match *w {
                        "down" => down = true,
                        "up" => down = false,
                        "repeat" => repeat = true,
                        m => {
                            for part in m.split('+') {
                                match part {
                                    "ctrl" => mods.ctrl = true,
                                    "shift" => mods.shift = true,
                                    "alt" => mods.alt = true,
                                    "meta" => mods.meta = true,
                                    _ => return Err(at(&format!("'{m}' is not down, up, repeat or ctrl+shift+alt"))),
                                }
                            }
                        }
                    }
                }
                SimAction::Key { popup, event: KeyEvent { key, mods, down, repeat } }
            }
            "text" => {
                let text = rest.first().ok_or_else(|| at("text needs the text"))?;
                SimAction::Text { popup, input: TextInput::Chars(unescape(text).map_err(|e| at(&e))?) }
            }
            "ime" => {
                let input = match rest {
                    ["start"] => TextInput::CompositionStart,
                    ["end"] => TextInput::CompositionEnd,
                    ["commit", t] => TextInput::CompositionCommit(unescape(t).map_err(|e| at(&e))?),
                    ["update", t, c] => TextInput::CompositionUpdate {
                        text: unescape(t).map_err(|e| at(&e))?,
                        cursor: c.parse().map_err(|_| at("the cursor is not a number"))?,
                    },
                    _ => return Err(at("use ime start | update <text> <cursor> | commit <text> | end")),
                };
                SimAction::Text { popup, input }
            }
            other => return Err(at(&format!("unknown event '{other}'"))),
        };
        out.push((ms, action));
    }
    Ok(out)
}

/// `<ms> <surface> move|down|up|wheel|hwheel|leave <x> <y> [left|right|middle|<notches>]`
/// per line; blank lines and `#` comments are ignored. The surface is a bar
/// name (`main` or `bar:main`), `popup:<name>` or `dim:<name>`; positions are
/// relative to that window, in physical pixels.
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
            return Err(at("use <ms> <surface> <move|down|up|wheel|hwheel|leave> <x> <y> [button|notches]"));
        }
        let ms: u64 = words[0].parse().map_err(|_| at("the time is not a number"))?;
        let surface = parse_surface(words[1], true).map_err(|e| at(&e))?;
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
            "hwheel" => {
                let notches = arg.ok_or_else(|| at("hwheel needs the notches"))?;
                PointerKind::HWheel(notches.parse().map_err(|_| at("the notches are not a number"))?)
            }
            other => return Err(at(&format!("unknown event '{other}'"))),
        };
        out.push((ms, SimAction::Pointer { surface, event: PointerEvent { kind, x, y } }));
    }
    Ok(out)
}

/// Every timed action the flags ask for, in time order.
pub fn schedule(
    opts: &RunOpts,
    pointer: Vec<(u64, SimAction)>,
    keys: Vec<(u64, SimAction)>,
) -> Vec<(Duration, SimAction)> {
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
    for (t, name, action) in &opts.simulate_popup {
        out.push((ms(*t), SimAction::Popup { name: name.clone(), action: *action }));
    }
    for &t in &opts.simulate_outside_press {
        out.push((ms(t), SimAction::OutsidePress));
    }
    for (t, popup) in &opts.simulate_deactivate {
        out.push((ms(*t), SimAction::Deactivate { popup: popup.clone() }));
    }
    for &(t, locked) in &opts.simulate_session {
        out.push((ms(t), SimAction::Session { locked }));
    }
    for (t, popup, dpi) in &opts.simulate_popup_dpi {
        out.push((ms(*t), SimAction::PopupDpi { popup: popup.clone(), dpi: *dpi }));
    }
    if let Some((t, for_ms)) = opts.test_ui_stall {
        out.push((ms(t), SimAction::UiStall { for_ms }));
    }
    out.extend(pointer.into_iter().map(|(t, a)| (ms(t), a)));
    out.extend(keys.into_iter().map(|(t, a)| (ms(t), a)));
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
    fn pointer_surfaces_are_bars_popups_and_dims() {
        let parsed =
            parse_pointer_script("1 main move 1 2\n2 bar:top down 1 2\n3 popup:menu up 4 5\n4 dim:power down 0 0")
                .unwrap();
        let surfaces: Vec<SimSurface> = parsed
            .iter()
            .map(|(_, a)| match a {
                SimAction::Pointer { surface, .. } => surface.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            surfaces,
            [
                SimSurface::Bar("main".into()),
                SimSurface::Bar("top".into()),
                SimSurface::Popup("menu".into()),
                SimSurface::Dim("power".into())
            ]
        );
        assert!(parse_pointer_script("1 popup: move 1 2").unwrap_err().contains("names no surface"));
    }

    #[test]
    fn the_key_script_parses_keys_text_and_ime() {
        let text = "# a launcher session\n\
                    100 popup:launcher key Escape\n\
                    120 popup:launcher key v down ctrl\n\
                    130 popup:launcher key ArrowDown up shift+alt repeat\n\
                    140 popup:launcher text ab\\u00e9\\s\\ud83d\\ude00\n\
                    150 popup:launcher ime start\n\
                    160 popup:launcher ime update \\u3042 1\n\
                    170 popup:launcher ime commit \\u3042\n\
                    180 popup:launcher ime end\n";
        let parsed = parse_key_script(text).unwrap();
        assert_eq!(parsed.len(), 8);
        match &parsed[0].1 {
            SimAction::Key { popup, event } => {
                assert_eq!(popup, "launcher");
                assert_eq!((event.key, event.down, event.repeat), (Key::Escape, true, false));
            }
            other => panic!("{other:?}"),
        }
        match &parsed[1].1 {
            SimAction::Key { event, .. } => {
                assert!(event.mods.ctrl && !event.mods.shift && event.key == Key::Char('v'))
            }
            other => panic!("{other:?}"),
        }
        match &parsed[2].1 {
            SimAction::Key { event, .. } => {
                assert!(!event.down && event.repeat && event.mods.shift && event.mods.alt && !event.mods.ctrl);
            }
            other => panic!("{other:?}"),
        }
        match &parsed[3].1 {
            SimAction::Text { input: TextInput::Chars(t), .. } => assert_eq!(t, "ab\u{e9} \u{1f600}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(&parsed[4].1, SimAction::Text { input: TextInput::CompositionStart, .. }));
        assert!(matches!(&parsed[5].1, SimAction::Text { input: TextInput::CompositionUpdate { cursor: 1, .. }, .. }));
        assert!(
            matches!(&parsed[6].1, SimAction::Text { input: TextInput::CompositionCommit(t), .. } if t == "\u{3042}")
        );
        assert!(matches!(&parsed[7].1, SimAction::Text { input: TextInput::CompositionEnd, .. }));
    }

    #[test]
    fn a_bad_key_line_names_its_line() {
        for (text, needle) in [
            ("1 popup:a key", "key name"),
            ("1 popup:a key Foo", "not a key name"),
            ("1 popup:a key v sideways", "not down, up, repeat"),
            ("1 main key v", "popup:<name>"),
            ("x popup:a key v", "time"),
            ("\n1 popup:a text a\\qb", "line 2"),
            ("1 popup:a ime wobble", "ime start"),
            ("1 popup:a flick", "unknown event"),
        ] {
            let e = parse_key_script(text).unwrap_err();
            assert!(e.contains(needle), "{text:?}: {e}");
        }
    }

    #[test]
    fn the_popup_flags_are_scheduled_with_the_rest() {
        use rsb_core::popup::PopupAction;
        let opts = RunOpts {
            simulate_popup: vec![(50, "launcher".into(), PopupAction::Open)],
            simulate_outside_press: vec![60],
            simulate_deactivate: vec![(70, "launcher".into())],
            simulate_session: vec![(80, true)],
            simulate_popup_dpi: vec![(90, "launcher".into(), 144)],
            test_ui_stall: Some((95, 10)),
            ..RunOpts::default()
        };
        let key = (
            55,
            SimAction::Key {
                popup: "launcher".into(),
                event: KeyEvent { key: Key::Enter, mods: Modifiers::default(), down: true, repeat: false },
            },
        );
        let plan = schedule(&opts, Vec::new(), vec![key]);
        let times: Vec<u128> = plan.iter().map(|(t, _)| t.as_millis()).collect();
        assert_eq!(times, [50, 55, 60, 70, 80, 90, 95]);
        assert!(matches!(plan[0].1, SimAction::Popup { action: PopupAction::Open, .. }));
        assert!(matches!(plan[2].1, SimAction::OutsidePress));
        assert!(matches!(plan[4].1, SimAction::Session { locked: true }));
        assert!(matches!(plan[5].1, SimAction::PopupDpi { dpi: 144, .. }));
        assert!(matches!(plan[6].1, SimAction::UiStall { for_ms: 10 }));
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
        let plan = schedule(&opts, Vec::new(), Vec::new());
        let times: Vec<u128> = plan.iter().map(|(t, _)| t.as_millis()).collect();
        assert_eq!(times, [100, 300, 500, 700, 900, 2000]);
        assert_eq!(plan.last().map(|(_, a)| a.clone()), Some(SimAction::Quit));
        assert_eq!(plan[3].1, SimAction::FullscreenWindow { for_ms: 1000 });
        assert!(schedule(&RunOpts::default(), Vec::new(), Vec::new()).is_empty());
    }
}
